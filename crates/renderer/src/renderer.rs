//! The [`Renderer`]: owns pipelines, persistent textures, built-in textures and the per-frame
//! upload rings, and builds the frame as a render graph (ADR-0011 §3, ADR-0012).

use std::time::{Duration, Instant};

use foundation::Handle;
use foundation::Pool;
use foundation::math::{Mat4, Vec3};
use render_graph::{GraphBuilder, GraphError, GraphStats, RenderGraph, TransientDesc};
use rhi::{
    AddressMode, BlendFactor, BlendState, BufferDesc, BufferHandle, BufferUsage, CompareOp,
    CullMode, DepthBias, DepthState, Device, Filter, Format, FrameToken, FrontFace,
    GraphicsPipelineDesc, Memory, PipelineHandle, PrimitiveTopology, RhiError, SamplerDesc,
    SamplerHandle, ShaderModuleDesc, ShaderModuleHandle, ShaderStageDesc, SwapchainImage,
    TextureDesc, TextureHandle, TextureState, TextureUsage,
};

use crate::cluster::{CLUSTER_FAR, ClusterGrid, GRID, LightSphere, spot_bounds};
use crate::cull::{Frustum, bounding_sphere, transform_sphere};
use crate::gpu::{GpuFrame, GpuInstance, GpuLight, GpuMaterial};
use crate::ibl::Ibl;
use crate::passes::{
    Draw, GbufferInputs, GeometryInputs, LightingInputs, Pipes, PrepassInputs, ShadedInputs,
    ShadowInputs, SkyboxInputs, TonemapInputs, forward_pass, gbuffer_pass, lighting_pass, prepass,
    shadow_pass, skybox_pass, tonemap_pass, transparent_pass,
};
use crate::post::{
    BloomInputs, BloomTargets, DOWN_NAMES, TaaInputs, UP_NAMES, bloom_passes, jitter_pixels,
    jitter_projection, ssao_blur_pass, ssao_pass, taa_pass,
};
use crate::shadow::{CASCADES, cascade_splits, fit_directional, fit_spot};
use crate::{Light, MAX_LIGHTS, MAX_SPOT_SHADOWS, MeshData, RenderScene};

macro_rules! spv {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $name, ".spv")) as &[u8]
    };
}
static FORWARD_VERT: &[u8] = spv!("forward.vert");
static FORWARD_FRAGS: [&[u8]; 2] = [spv!("forward.frag"), spv!("forward_mask.frag")];
static PREPASS_FRAGS: [&[u8]; 2] = [spv!("prepass.frag"), spv!("prepass_mask.frag")];
static GBUFFER_FRAGS: [&[u8]; 2] = [spv!("gbuffer.frag"), spv!("gbuffer_mask.frag")];
static LIGHTING_FRAG: &[u8] = spv!("lighting.frag");
static SSAO_FRAG: &[u8] = spv!("ssao.frag");
static SSAO_BLUR_FRAG: &[u8] = spv!("ssao_blur.frag");
static TAA_FRAG: &[u8] = spv!("taa.frag");
static BLOOM_DOWN_FRAG: &[u8] = spv!("bloom_down.frag");
static BLOOM_UP_FRAG: &[u8] = spv!("bloom_up.frag");
static SHADOW_VERT: &[u8] = spv!("shadow.vert");
static SHADOW_FRAGS: [&[u8]; 2] = [spv!("shadow.frag"), spv!("shadow_mask.frag")];
pub(crate) static TONEMAP_VERT: &[u8] = spv!("tonemap.vert");
static TONEMAP_FRAG: &[u8] = spv!("tonemap.frag");

/// Size of each per-frame-in-flight upload ring.
// shortcut: fixed 8 MiB ring, render fails when a scene overflows it; grow or chain rings when scenes get bigger.
const RING_BYTES: u64 = 8 << 20;
/// Resolution of each directional cascade (ADR-0012 §11).
const SHADOW_SIZE: u32 = 2048;
/// Resolution of each spot shadow map; mirrored by `SPOT_SHADOW_SIZE` in `lighting.glsl`.
const SPOT_SHADOW_SIZE: u32 = 1024;
/// The cascades cover the camera frustum up to this distance (meters).
const SHADOW_DISTANCE: f32 = 80.0;
/// Weight of the logarithmic scheme in the practical cascade split.
const CASCADE_LAMBDA: f32 = 0.75;
const CASCADE_NAMES: [&str; CASCADES] = [
    "shadow.cascade0",
    "shadow.cascade1",
    "shadow.cascade2",
    "shadow.cascade3",
];
const SPOT_NAMES: [&str; MAX_SPOT_SHADOWS] = [
    "shadow.spot0",
    "shadow.spot1",
    "shadow.spot2",
    "shadow.spot3",
];
pub(crate) const HDR_FORMAT: Format = Format::Rgba16Float;
const DEPTH_FORMAT: Format = Format::D32Sfloat;

/// A GPU mesh (vertex buffer read through BDA, index buffer) with its object-space bounding
/// sphere. Handle: [`MeshId`].
pub struct Mesh {
    vertices: BufferHandle,
    indices: BufferHandle,
    vertex_address: u64,
    index_count: u32,
    center: Vec3,
    radius: f32,
}

/// A GPU texture with its bindless slot. Handle: [`TextureId`].
pub struct Texture {
    texture: TextureHandle,
    index: u32,
}

/// A material record (GPU layout plus the alpha mode that selects pipelines). Handle: [`MaterialId`].
pub struct Material {
    gpu: GpuMaterial,
    alpha_mode: AlphaMode,
    double_sided: bool,
}

/// How a material's alpha is used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AlphaMode {
    /// Alpha ignored.
    Opaque,
    /// Fragments with alpha below the cutoff are discarded (prepass, G-buffer, forward, shadows).
    Mask(f32),
    /// Alpha blended in the forward transparent pass, sorted back to front, no depth write.
    /// Blended objects receive shadows but do not cast them.
    Blend,
}

/// Which shading path lights the opaque geometry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderPath {
    /// G-buffer-free: the forward pass lights every fragment (default).
    #[default]
    Forward,
    /// G-buffer fill followed by a fullscreen lighting pass.
    Deferred,
}

/// Debug visualisations of the lighting data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugView {
    /// Normal image.
    #[default]
    None,
    /// Lights per cluster as a heatmap.
    Clusters,
    /// Directional shadow cascades tinted.
    Cascades,
    /// The screen-space ambient occlusion factor (white when SSAO is off).
    Ssao,
}

/// Post-processing switches (ADR-0012 §11); a disabled effect adds no graph passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostSettings {
    /// Half-resolution SSAO multiplying the ambient/IBL term.
    pub ssao: bool,
    /// Temporal anti-aliasing (Halton jitter, velocity, YCoCg-clamped history).
    pub taa: bool,
    /// Bloom added before tonemapping.
    pub bloom: bool,
}

impl Default for PostSettings {
    fn default() -> Self {
        Self {
            ssao: true,
            taa: true,
            bloom: true,
        }
    }
}

/// Bloom strength mixed into the HDR image before tonemapping.
const BLOOM_STRENGTH: f32 = 0.025;

/// The two TAA history textures (ping-pong) at the target size.
struct History {
    textures: [TextureHandle; 2],
    extent: (u32, u32),
}

/// Handle to a mesh created with [`Renderer::create_mesh`].
pub type MeshId = Handle<Mesh>;
/// Handle to a texture created with [`Renderer::create_texture`].
pub type TextureId = Handle<Texture>;
/// Handle to a material created with [`Renderer::create_material`].
pub type MaterialId = Handle<Material>;

/// glTF metallic-roughness material (ADR-0011 §5). Missing textures use 1x1 built-in defaults.
/// Textures must outlive the material and be created with the right color space: base color and
/// emissive sRGB, the rest linear.
// shortcut: bindless indices are resolved at creation; destroying a texture before its materials leaves stale indices.
#[derive(Clone, Copy, Debug)]
pub struct MaterialDesc<'a> {
    /// Debug label.
    pub label: &'a str,
    /// Base color factor (linear RGBA).
    pub base_color: [f32; 4],
    /// Base color texture (sRGB).
    pub base_color_texture: Option<TextureId>,
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// Metallic (B) / roughness (G) texture (linear).
    pub metallic_roughness_texture: Option<TextureId>,
    /// Tangent-space normal texture (linear).
    pub normal_texture: Option<TextureId>,
    /// Normal map xy scale.
    pub normal_scale: f32,
    /// Occlusion texture (R, linear).
    pub occlusion_texture: Option<TextureId>,
    /// Occlusion strength.
    pub occlusion_strength: f32,
    /// Emissive factor (linear RGB).
    pub emissive: [f32; 3],
    /// Emissive texture (sRGB).
    pub emissive_texture: Option<TextureId>,
    /// Alpha handling.
    pub alpha_mode: AlphaMode,
    /// Render both faces (back faces get a flipped normal).
    pub double_sided: bool,
}

impl Default for MaterialDesc<'_> {
    fn default() -> Self {
        Self {
            label: "material",
            base_color: [1.0; 4],
            base_color_texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
        }
    }
}

/// Per-frame counters (ADR-0011 §11), logged once per second at `debug`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RendererStats {
    /// Draw calls issued.
    pub draws: u32,
    /// Triangles submitted.
    pub triangles: u64,
    /// Lights uploaded.
    pub lights: u32,
    /// Bytes of the frame's upload ring in use.
    pub ring_bytes_used: u64,
    /// Render graph counters, including compile time.
    pub graph: GraphStats,
    /// Instances rejected by the camera frustum test.
    pub culled: u32,
    /// Draws recorded into all shadow maps.
    pub shadow_draws: u32,
    /// CPU time of the cluster light assignment.
    pub cluster_time: Duration,
}

/// Why [`Renderer::render`] failed.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// Device or upload failure.
    #[error(transparent)]
    Rhi(#[from] RhiError),
    /// The frame graph did not compile or run.
    #[error(transparent)]
    Graph(#[from] GraphError),
}

/// Forward renderer. All methods taking a device must be given the device it was created with.
pub struct Renderer {
    forward: Pipes,
    prepass: Pipes,
    gbuffer: Pipes,
    transparent: Pipes,
    lighting: PipelineHandle,
    shadow_pipes: Pipes,
    tonemap: PipelineHandle,
    cascades: [ShadowMap; CASCADES],
    spots: [ShadowMap; MAX_SPOT_SHADOWS],
    cluster_grid: Option<ClusterGrid>,
    ssao_pipeline: PipelineHandle,
    ssao_blur_pipeline: PipelineHandle,
    taa_pipeline: PipelineHandle,
    bloom_down_pipeline: PipelineHandle,
    bloom_up_pipeline: PipelineHandle,
    post: PostSettings,
    history: Option<History>,
    /// History texture written this frame; the other one holds the previous result.
    taa_write: usize,
    /// The previous frame resolved TAA into the history at the current extent.
    taa_valid: bool,
    prev_view_proj: Mat4,
    frame_index: u32,
    path: RenderPath,
    debug_view: DebugView,
    shadow_sampler: SamplerHandle,
    shadow_sampler_index: u32,
    ibl: Ibl,
    graph: RenderGraph,
    encode_srgb: bool,
    linear_repeat: SamplerHandle,
    linear_clamp: SamplerHandle,
    sampler_indices: (u32, u32),
    white_srgb: TextureId,
    white_linear: TextureId,
    flat_normal: TextureId,
    default_material: MaterialId,
    meshes: Pool<Mesh>,
    textures: Pool<Texture>,
    materials: Pool<Material>,
    rings: Vec<(BufferHandle, u64)>,
    ring_used: u64,
    stats: RendererStats,
    last_log: Instant,
}

/// A persistent depth texture rendered by a shadow pass and sampled by the shaders.
struct ShadowMap {
    texture: TextureHandle,
    index: u32,
}

fn spirv_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

pub(crate) fn make_pipeline<D: Device>(
    d: &D,
    label: &str,
    vert: &[u8],
    frag: &[u8],
    color: &[Format],
    depth: Option<DepthState>,
    cull: CullMode,
) -> Result<PipelineHandle, RhiError> {
    let blend = vec![BlendState::default(); color.len()];
    make_pipeline_blend(d, label, vert, frag, color, depth, cull, &blend)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_pipeline_blend<D: Device>(
    d: &D,
    label: &str,
    vert: &[u8],
    frag: &[u8],
    color: &[Format],
    depth: Option<DepthState>,
    cull: CullMode,
    blend: &[BlendState],
) -> Result<PipelineHandle, RhiError> {
    let mk = |bytes: &[u8], l: String| -> Result<ShaderModuleHandle, RhiError> {
        d.create_shader_module(&ShaderModuleDesc {
            spirv: &spirv_words(bytes),
            label: Some(&l),
        })
    };
    let vs = mk(vert, format!("{label}.vert"))?;
    let fs = mk(frag, format!("{label}.frag"))?;
    let pipeline = d.create_graphics_pipeline(&GraphicsPipelineDesc {
        label: Some(label),
        vertex: ShaderStageDesc {
            module: vs,
            entry: "main",
        },
        fragment: ShaderStageDesc {
            module: fs,
            entry: "main",
        },
        color_formats: color,
        depth,
        topology: PrimitiveTopology::TriangleList,
        cull,
        front_face: FrontFace::CounterClockwise,
        blend,
    });
    // Module destruction is deferred by the device; the pipeline no longer needs them.
    d.destroy_shader_module(vs);
    d.destroy_shader_module(fs);
    pipeline
}

/// The four pipeline variants of one stage: `mask * 2 + double_sided`. `frags` are the opaque and
/// the alpha-tested fragment shader; double-sided variants disable culling.
#[allow(clippy::too_many_arguments)]
fn make_variants<D: Device>(
    d: &D,
    label: &str,
    vert: &[u8],
    frags: &[&[u8]; 2],
    color: &[Format],
    depth: Option<DepthState>,
    cull: CullMode,
    blend: &[BlendState],
) -> Result<Pipes, RhiError> {
    let mut out = [PipelineHandle::NULL; 4];
    for (v, p) in out.iter_mut().enumerate() {
        let (mask, double) = (v / 2, v % 2 == 1);
        let name = format!(
            "{label}{}{}",
            ["", ".mask"][mask],
            [".1s", ".2s"][usize::from(double)]
        );
        let c = if double { CullMode::None } else { cull };
        *p = make_pipeline_blend(d, &name, vert, frags[mask], color, depth, c, blend)?;
    }
    Ok(out)
}

fn shadow_map<D: Device>(d: &D, label: &str, size: u32) -> Result<ShadowMap, RhiError> {
    let texture = d.create_texture(&TextureDesc {
        label: Some(label),
        width: size,
        height: size,
        format: DEPTH_FORMAT,
        usage: TextureUsage::DEPTH_ATTACHMENT | TextureUsage::SAMPLED,
    })?;
    Ok(ShadowMap {
        texture,
        index: d.texture_index(texture)?,
    })
}

fn gpu_only_buffer<D: Device>(
    d: &D,
    label: &str,
    usage: BufferUsage,
    data: &[u8],
) -> Result<BufferHandle, RhiError> {
    let b = d.create_buffer(&BufferDesc {
        label: Some(label),
        size: data.len() as u64,
        usage: usage | BufferUsage::TRANSFER_DST,
        memory: Memory::GpuOnly,
    })?;
    if let Err(e) = d.upload_buffer(b, 0, data) {
        d.destroy_buffer(b);
        return Err(e);
    }
    Ok(b)
}

impl Renderer {
    /// Creates pipelines, render targets, samplers, built-in textures and the upload rings.
    /// `swapchain_format` is the format passed to [`Renderer::render`] targets; `extent` the
    /// swapchain size in pixels.
    ///
    /// # Errors
    /// Any [`RhiError`] from resource creation.
    pub fn new<D: Device>(
        device: &D,
        swapchain_format: Format,
        extent: (u32, u32),
    ) -> Result<Self, RhiError> {
        let none = [BlendState::default()];
        let depth_state = |compare, write, bias| {
            Some(DepthState {
                format: DEPTH_FORMAT,
                compare,
                write,
                bias,
            })
        };
        // Reverse-Z: far = 0. The prepass writes depth; later passes test `GreaterOrEqual`
        // against it without writing, which needs the one invariant vertex shader.
        let prepass = make_variants(
            device,
            "renderer.prepass",
            FORWARD_VERT,
            &PREPASS_FRAGS,
            &[Format::Rg16Float, Format::Rg16Float], // view normal, velocity
            depth_state(CompareOp::GreaterOrEqual, true, None),
            CullMode::Back,
            &[BlendState::default(); 2],
        )?;
        let forward = make_variants(
            device,
            "renderer.forward",
            FORWARD_VERT,
            &FORWARD_FRAGS,
            &[HDR_FORMAT],
            depth_state(CompareOp::GreaterOrEqual, false, None),
            CullMode::Back,
            &none,
        )?;
        let gbuffer = make_variants(
            device,
            "renderer.gbuffer",
            FORWARD_VERT,
            &GBUFFER_FRAGS,
            &[Format::Rgba8Srgb, Format::Rgba16Float, Format::Rgba16Float],
            depth_state(CompareOp::GreaterOrEqual, false, None),
            CullMode::Back,
            &[BlendState::default(); 3],
        )?;
        // Premultiplied alpha: the shader scales the diffuse term by alpha, not the specular.
        let premultiplied = [BlendState {
            enabled: true,
            src_color: BlendFactor::One,
            dst_color: BlendFactor::OneMinusSrcAlpha,
            src_alpha: BlendFactor::One,
            dst_alpha: BlendFactor::OneMinusSrcAlpha,
            ..BlendState::default()
        }];
        let transparent = make_variants(
            device,
            "renderer.transparent",
            FORWARD_VERT,
            &[FORWARD_FRAGS[0]; 2],
            &[HDR_FORMAT],
            depth_state(CompareOp::GreaterOrEqual, false, None),
            CullMode::Back,
            &premultiplied,
        )?;
        let lighting = make_pipeline(
            device,
            "renderer.lighting",
            TONEMAP_VERT,
            LIGHTING_FRAG,
            &[HDR_FORMAT],
            None,
            CullMode::None,
        )?;
        let post_pipe = |label, frag, format| {
            make_pipeline(
                device,
                label,
                TONEMAP_VERT,
                frag,
                &[format],
                None,
                CullMode::None,
            )
        };
        let ssao_pipeline = post_pipe("renderer.ssao", SSAO_FRAG, Format::R8Unorm)?;
        let ssao_blur_pipeline = post_pipe("renderer.ssao_blur", SSAO_BLUR_FRAG, Format::R8Unorm)?;
        let taa_pipeline = post_pipe("renderer.taa", TAA_FRAG, HDR_FORMAT)?;
        let bloom_down_pipeline = post_pipe("renderer.bloom_down", BLOOM_DOWN_FRAG, HDR_FORMAT)?;
        let bloom_up_pipeline = post_pipe("renderer.bloom_up", BLOOM_UP_FRAG, HDR_FORMAT)?;
        let tonemap = make_pipeline(
            device,
            "renderer.tonemap",
            TONEMAP_VERT,
            TONEMAP_FRAG,
            &[swapchain_format],
            None,
            CullMode::None,
        )?;
        // Front-face culling: only back faces enter the map, so lit surfaces never self-shadow
        // (no acne) at the cost of light leaking through very thin casters.
        let shadow_pipes = make_variants(
            device,
            "renderer.shadow",
            SHADOW_VERT,
            &SHADOW_FRAGS,
            &[],
            depth_state(
                CompareOp::LessOrEqual,
                true,
                Some(DepthBias {
                    constant: 2.0,
                    slope: 2.5,
                    clamp: 0.0,
                }),
            ),
            CullMode::Front,
            &[],
        )?;
        let mut cascades = Vec::new();
        for name in CASCADE_NAMES {
            cascades.push(shadow_map(device, name, SHADOW_SIZE)?);
        }
        let mut spots = Vec::new();
        for name in SPOT_NAMES {
            spots.push(shadow_map(device, name, SPOT_SHADOW_SIZE)?);
        }
        let shadow_sampler = device.create_sampler(&SamplerDesc {
            label: Some("renderer.shadow_cmp"),
            mag_filter: Filter::Linear,
            min_filter: Filter::Linear,
            address_u: AddressMode::ClampToEdge,
            address_v: AddressMode::ClampToEdge,
            address_w: AddressMode::ClampToEdge,
            compare: Some(CompareOp::LessOrEqual),
        })?;
        let mut graph = RenderGraph::new(device.diagnostics().frames_in_flight);
        for m in cascades.iter().chain(&spots) {
            graph.track(m.texture, TextureState::Undefined);
        }
        let sampler = |label, address| {
            device.create_sampler(&SamplerDesc {
                label: Some(label),
                mag_filter: Filter::Linear,
                min_filter: Filter::Linear,
                address_u: address,
                address_v: address,
                address_w: address,
                compare: None,
            })
        };
        let linear_repeat = sampler("renderer.linear_repeat", AddressMode::Repeat)?;
        let linear_clamp = sampler("renderer.linear_clamp", AddressMode::ClampToEdge)?;
        let sampler_indices = (
            device.sampler_index(linear_repeat)?,
            device.sampler_index(linear_clamp)?,
        );
        let mut rings = Vec::new();
        let frames = device.diagnostics().frames_in_flight;
        for i in 0..frames {
            let b = device.create_buffer(&BufferDesc {
                label: Some(&format!("renderer.ring{i}")),
                size: RING_BYTES,
                usage: BufferUsage::STORAGE,
                memory: Memory::Upload,
            })?;
            rings.push((b, device.buffer_address(b)?));
        }
        let mut r = Self {
            forward,
            prepass,
            gbuffer,
            transparent,
            lighting,
            shadow_pipes,
            tonemap,
            cascades: cascades.try_into().ok().expect("4 cascades"),
            spots: spots.try_into().ok().expect("4 spot maps"),
            cluster_grid: None,
            ssao_pipeline,
            ssao_blur_pipeline,
            taa_pipeline,
            bloom_down_pipeline,
            bloom_up_pipeline,
            post: PostSettings::default(),
            history: None,
            taa_write: 0,
            taa_valid: false,
            prev_view_proj: Mat4::IDENTITY,
            frame_index: 0,
            path: RenderPath::default(),
            debug_view: DebugView::default(),
            shadow_sampler_index: device.sampler_index(shadow_sampler)?,
            shadow_sampler,
            ibl: Ibl::new(device, &mut graph)?,
            graph,
            encode_srgb: !swapchain_format.is_srgb(),
            linear_repeat,
            linear_clamp,
            sampler_indices,
            white_srgb: TextureId::NULL,
            white_linear: TextureId::NULL,
            flat_normal: TextureId::NULL,
            default_material: MaterialId::NULL,
            meshes: Pool::new(),
            textures: Pool::new(),
            materials: Pool::new(),
            rings,
            ring_used: 0,
            stats: RendererStats::default(),
            last_log: Instant::now(),
        };
        r.white_srgb = r.create_texture(device, "default.white_srgb", &[255; 4], 1, 1, true)?;
        r.white_linear =
            r.create_texture(device, "default.white_linear", &[255; 4], 1, 1, false)?;
        r.flat_normal = r.create_texture(
            device,
            "default.flat_normal",
            &[128, 128, 255, 255],
            1,
            1,
            false,
        )?;
        r.default_material = r.create_material(&MaterialDesc {
            label: "default",
            ..MaterialDesc::default()
        });
        tracing::debug!(?extent, ?swapchain_format, frames, "renderer created");
        Ok(r)
    }

    /// Uploads a mesh into GPU-only buffers.
    ///
    /// # Errors
    /// [`RhiError::InvalidDescriptor`] for an empty mesh, or any buffer creation error.
    pub fn create_mesh<D: Device>(
        &mut self,
        device: &D,
        label: &str,
        data: &MeshData,
    ) -> Result<MeshId, RhiError> {
        if data.vertices.is_empty() || data.indices.is_empty() {
            return Err(RhiError::InvalidDescriptor(format!(
                "mesh {label} is empty"
            )));
        }
        let vertices = gpu_only_buffer(
            device,
            &format!("{label}.vertices"),
            BufferUsage::STORAGE,
            bytemuck::cast_slice(&data.vertices),
        )?;
        let indices = gpu_only_buffer(
            device,
            &format!("{label}.indices"),
            BufferUsage::INDEX,
            bytemuck::cast_slice(&data.indices),
        )?;
        let (center, radius) = bounding_sphere(data.vertices.iter().map(|v| Vec3::from(v.pos)));
        let id = self.meshes.insert(Mesh {
            vertices,
            indices,
            vertex_address: device.buffer_address(vertices)?,
            index_count: data.indices.len() as u32,
            center,
            radius,
        });
        tracing::debug!(
            label,
            ?id,
            vertices = data.vertices.len(),
            indices = data.indices.len(),
            "mesh created"
        );
        Ok(id)
    }

    /// Destroys a mesh (deferred by the device).
    pub fn destroy_mesh<D: Device>(&mut self, device: &D, id: MeshId) {
        if let Some(m) = self.meshes.remove(id) {
            device.destroy_buffer(m.vertices);
            device.destroy_buffer(m.indices);
            tracing::debug!(?id, "mesh destroyed");
        }
    }

    /// Uploads a tightly packed RGBA8 image (`srgb` selects the color space).
    ///
    /// # Errors
    /// Any [`RhiError`] from texture creation or upload (e.g. wrong data length).
    pub fn create_texture<D: Device>(
        &mut self,
        device: &D,
        label: &str,
        rgba8: &[u8],
        width: u32,
        height: u32,
        srgb: bool,
    ) -> Result<TextureId, RhiError> {
        let texture = device.create_texture(&TextureDesc {
            label: Some(label),
            width,
            height,
            format: if srgb {
                Format::Rgba8Srgb
            } else {
                Format::Rgba8Unorm
            },
            usage: TextureUsage::SAMPLED | TextureUsage::TRANSFER_DST,
        })?;
        if let Err(e) = device.upload_texture(texture, rgba8) {
            device.destroy_texture(texture);
            return Err(e);
        }
        let index = device.texture_index(texture)?;
        let id = self.textures.insert(Texture { texture, index });
        tracing::debug!(label, ?id, width, height, srgb, index, "texture created");
        Ok(id)
    }

    /// Destroys a texture (deferred by the device).
    pub fn destroy_texture<D: Device>(&mut self, device: &D, id: TextureId) {
        if let Some(t) = self.textures.remove(id) {
            device.destroy_texture(t.texture);
            tracing::debug!(?id, "texture destroyed");
        }
    }

    /// Registers a material. Stale texture ids fall back to the built-in defaults.
    pub fn create_material(&mut self, desc: &MaterialDesc) -> MaterialId {
        let tex = |t: Option<TextureId>, default: TextureId| {
            t.and_then(|t| self.textures.get(t))
                .or_else(|| self.textures.get(default))
                .map_or(0, |t| t.index)
        };
        let m = GpuMaterial {
            base_color: desc.base_color,
            emissive: [desc.emissive[0], desc.emissive[1], desc.emissive[2], 0.0],
            metallic: desc.metallic,
            roughness: desc.roughness,
            normal_scale: desc.normal_scale,
            occlusion_strength: desc.occlusion_strength,
            base_color_tex: tex(desc.base_color_texture, self.white_srgb),
            mr_tex: tex(desc.metallic_roughness_texture, self.white_linear),
            normal_tex: tex(desc.normal_texture, self.flat_normal),
            occlusion_tex: tex(desc.occlusion_texture, self.white_linear),
            emissive_tex: tex(desc.emissive_texture, self.white_srgb),
            alpha_cutoff: match desc.alpha_mode {
                AlphaMode::Mask(c) => c,
                _ => 0.0,
            },
            pad: [0; 2],
        };
        let id = self.materials.insert(Material {
            gpu: m,
            alpha_mode: desc.alpha_mode,
            double_sided: desc.double_sided,
        });
        tracing::debug!(label = desc.label, ?id, "material created");
        id
    }

    /// Destroys a material record.
    pub fn destroy_material(&mut self, id: MaterialId) {
        if self.materials.remove(id).is_some() {
            tracing::debug!(?id, "material destroyed");
        }
    }

    /// Selects forward or deferred shading for the next frame.
    pub fn set_render_path(&mut self, path: RenderPath) {
        self.path = path;
    }

    /// Current shading path.
    pub fn render_path(&self) -> RenderPath {
        self.path
    }

    /// Selects the post-processing effects for the next frame.
    pub fn set_post(&mut self, post: PostSettings) {
        self.post = post;
    }

    /// Current post-processing effects.
    pub fn post(&self) -> PostSettings {
        self.post
    }

    /// Selects a debug visualisation for the next frame.
    pub fn set_debug_view(&mut self, view: DebugView) {
        self.debug_view = view;
    }

    /// Current debug visualisation.
    pub fn debug_view(&self) -> DebugView {
        self.debug_view
    }

    /// Counters of the last rendered frame.
    pub fn stats(&self) -> RendererStats {
        self.stats
    }

    /// Appends `data` to the frame's ring (16-byte aligned) and returns its device address.
    fn ring_push<D: Device>(
        &mut self,
        device: &D,
        slot: usize,
        data: &[u8],
    ) -> Result<u64, RhiError> {
        let (buffer, base) = self.rings[slot];
        let offset = self.ring_used.next_multiple_of(16);
        let end = offset + data.len() as u64;
        if end > RING_BYTES {
            return Err(RhiError::InvalidDescriptor(format!(
                "renderer upload ring overflow ({end} > {RING_BYTES} bytes)"
            )));
        }
        if !data.is_empty() {
            device.write_buffer(buffer, offset, data)?;
        }
        self.ring_used = end;
        Ok(base + offset)
    }

    /// Builds and records the frame as a render graph: environment bakes (when the environment
    /// changed), four shadow cascades and up to four spot shadows, the depth prepass, then
    /// either forward lighting or G-buffer + lighting, the skybox, blended geometry and tonemap
    /// into `target`. `frame` is the token `cmd` was begun with. `target` ends in `final_state`
    /// (`Present`, or `TransferSrc` to read it back); it is imported as `Undefined`.
    ///
    /// # Errors
    /// [`RenderError`] from ring uploads (including ring overflow), or the graph.
    pub fn render<D: Device>(
        &mut self,
        device: &D,
        cmd: &mut D::Commands,
        frame: &FrameToken,
        scene: &RenderScene,
        target: &SwapchainImage,
        final_state: TextureState,
    ) -> Result<(), RenderError> {
        let slot = frame.slot as usize;
        self.ring_used = 0;

        let (w, h) = (target.extent.0.max(1), target.extent.1.max(1));
        let view = scene.camera.view();
        let aspect = w as f32 / h as f32;
        let proj_unjit = scene.camera.projection(aspect);
        let view_proj_unjit = proj_unjit * view;
        // TAA: the main view (prepass, G-buffer/forward, transparent, SSAO) renders with a
        // sub-pixel jittered projection; shadows, culling and clusters use the plain one.
        let taa = self.post.taa;
        let jitter = if taa {
            jitter_pixels(self.frame_index)
        } else {
            (0.0, 0.0)
        };
        let proj = jitter_projection(proj_unjit, jitter, (w, h));
        let view_proj = proj * view;
        let prev_view_proj = if self.frame_index == 0 {
            view_proj_unjit
        } else {
            self.prev_view_proj
        };
        if !taa {
            self.taa_valid = false;
        } else if self.history.as_ref().is_none_or(|hs| hs.extent != (w, h)) {
            if let Some(old) = self.history.take() {
                for t in old.textures {
                    self.graph.forget(t);
                    device.destroy_texture(t);
                }
            }
            let mut textures = [TextureHandle::NULL; 2];
            for (i, t) in textures.iter_mut().enumerate() {
                *t = device.create_texture(&TextureDesc {
                    label: Some(["taa.history0", "taa.history1"][i]),
                    width: w,
                    height: h,
                    format: HDR_FORMAT,
                    usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
                })?;
                self.graph.track(*t, TextureState::Undefined);
            }
            self.history = Some(History {
                textures,
                extent: (w, h),
            });
            self.taa_valid = false;
        }
        let cam = scene.camera.position;
        let ibl = self.ibl.indices();
        let tan_y = (scene.camera.fov_y * 0.5).tan();
        let rot = scene.camera.rotation;
        let (right, up, forward) = (rot * Vec3::X, rot * Vec3::Y, rot * Vec3::NEG_Z);
        if scene.lights.len() > MAX_LIGHTS {
            tracing::warn!(
                lights = scene.lights.len(),
                max = MAX_LIGHTS,
                "too many lights, extra ignored"
            );
        }

        // Lights: directional first (looped by every fragment), then local lights (clustered).
        let lights_in = &scene.lights[..scene.lights.len().min(MAX_LIGHTS)];
        let mut lights: Vec<GpuLight> = Vec::with_capacity(lights_in.len());
        let mut cascade_dir = None;
        for l in lights_in {
            if let Light::Directional {
                direction,
                cast_shadows,
                ..
            } = *l
            {
                let mut g = GpuLight::new(l);
                if cast_shadows && cascade_dir.is_none() {
                    cascade_dir = Some(direction);
                    g.spot[2] = 0.0; // cascaded shadows
                }
                lights.push(g);
            }
        }
        let dir_count = lights.len();
        let mut spot_vp = [Mat4::IDENTITY; MAX_SPOT_SHADOWS];
        let mut spot_used = 0;
        let mut spheres = Vec::new();
        for l in lights_in {
            let (center, radius) = match *l {
                Light::Directional { .. } => continue,
                Light::Point {
                    position, range, ..
                } => (position, range),
                Light::Spot {
                    position,
                    direction,
                    range,
                    outer_cone,
                    ..
                } => {
                    if range > 0.0 {
                        spot_bounds(position, direction.normalize(), range, outer_cone)
                    } else {
                        (position, 0.0)
                    }
                }
            };
            let mut g = GpuLight::new(l);
            if let Light::Spot {
                position,
                direction,
                range,
                outer_cone,
                cast_shadows: true,
                ..
            } = *l
                && spot_used < MAX_SPOT_SHADOWS
            {
                spot_vp[spot_used] = fit_spot(position, direction, outer_cone, range);
                g.spot[2] = spot_used as f32;
                spot_used += 1;
            }
            spheres.push(LightSphere {
                center: view.transform_point3(center),
                // A light without range reaches everywhere.
                radius: if radius > 0.0 { radius } else { f32::INFINITY },
                index: (dir_count + spheres.len()) as u32,
            });
            lights.push(g);
        }

        // Cascades (stable-fit spheres, texel snapped) of the shadow-casting directional light.
        let splits = cascade_splits(scene.camera.near, SHADOW_DISTANCE, CASCADE_LAMBDA);
        let mut cascade_vp = [Mat4::IDENTITY; CASCADES];
        let mut cascade_texel = [0.0f32; 4];
        if let Some(dir) = cascade_dir {
            let mut start = scene.camera.near;
            for c in 0..CASCADES {
                let fit =
                    fit_directional(&scene.camera, aspect, dir, start, splits[c], SHADOW_SIZE);
                cascade_vp[c] = fit.view_proj;
                cascade_texel[c] = fit.texel_world;
                start = splits[c];
            }
        }

        // Clustered assignment.
        let t_cluster = Instant::now();
        let tan = (tan_y * aspect, tan_y);
        if !self
            .cluster_grid
            .as_ref()
            .is_some_and(|g| g.matches(scene.camera.near, tan))
        {
            self.cluster_grid = Some(ClusterGrid::new(scene.camera.near, tan));
        }
        let assignment = self
            .cluster_grid
            .as_ref()
            .map(|g| g.assign(&spheres))
            .expect("grid built above");
        let cluster_time = t_cluster.elapsed();

        // Visible instances per view: camera (opaque sorted front to back, grouped by pipeline
        // variant; blended sorted back to front) and each shadow map.
        struct Item {
            draw: Draw,
            center: Vec3,
            radius: f32,
            blend: bool,
        }
        let mut instances = Vec::with_capacity(scene.meshes.len());
        let mut items = Vec::with_capacity(scene.meshes.len());
        for inst in &scene.meshes {
            let Some(mesh) = self.meshes.get(inst.mesh) else {
                tracing::warn!(mesh = ?inst.mesh, "scene references a destroyed mesh, skipped");
                continue;
            };
            let material_id = if self.materials.get(inst.material).is_some() {
                inst.material
            } else {
                self.default_material
            };
            let Some(material) = self.materials.get(material_id) else {
                continue;
            };
            let (center, radius) = transform_sphere(&inst.transform, mesh.center, mesh.radius);
            let double = usize::from(material.double_sided);
            let (variant, blend) = match material.alpha_mode {
                AlphaMode::Opaque => (double, false),
                AlphaMode::Mask(_) => (2 + double, false),
                AlphaMode::Blend => (double, true),
            };
            items.push(Item {
                draw: Draw {
                    vertex_address: mesh.vertex_address,
                    index_buffer: mesh.indices,
                    index_count: mesh.index_count,
                    instance: instances.len() as u32,
                    variant,
                    depth: -view.transform_point3(center).z,
                },
                center,
                radius,
                blend,
            });
            instances.push(GpuInstance {
                model: inst.transform,
                normal_matrix: inst.transform.inverse().transpose(),
                prev_model: inst.prev_transform,
                material: material_id.index(),
                pad: [0; 3],
            });
        }
        let cam_frustum = Frustum::from_view_proj(view_proj_unjit);
        let (mut opaque, mut blended) = (Vec::new(), Vec::new());
        let mut culled = 0;
        let mut triangles = 0u64;
        for it in &items {
            if !cam_frustum.intersects_sphere(it.center, it.radius) {
                culled += 1;
                continue;
            }
            triangles += u64::from(it.draw.index_count / 3);
            if it.blend { &mut blended } else { &mut opaque }.push(it.draw);
        }
        opaque.sort_by(|a, b| a.variant.cmp(&b.variant).then(a.depth.total_cmp(&b.depth)));
        blended.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        // Shadow casters: everything but blended objects, grouped by variant.
        let shadow_draws = |vp: Mat4| -> Vec<Draw> {
            let f = Frustum::from_view_proj(vp);
            let mut v: Vec<Draw> = items
                .iter()
                .filter(|it| !it.blend && f.intersects_sphere(it.center, it.radius))
                .map(|it| it.draw)
                .collect();
            v.sort_by_key(|d| d.variant);
            v
        };
        let cascade_draws: Vec<Vec<Draw>> = (0..CASCADES)
            .map(|c| {
                if cascade_dir.is_some() {
                    shadow_draws(cascade_vp[c])
                } else {
                    Vec::new()
                }
            })
            .collect();
        let spot_draws: Vec<Vec<Draw>> = (0..MAX_SPOT_SHADOWS)
            .map(|i| {
                if i < spot_used {
                    shadow_draws(spot_vp[i])
                } else {
                    Vec::new()
                }
            })
            .collect();
        let shadow_draw_count = cascade_draws
            .iter()
            .chain(&spot_draws)
            .map(Vec::len)
            .sum::<usize>();

        let dense = self
            .materials
            .iter()
            .map(|(h, _)| h.index() + 1)
            .max()
            .unwrap_or(1) as usize;
        let mut materials = vec![bytemuck::Zeroable::zeroed(); dense];
        for (h, m) in self.materials.iter() {
            materials[h.index() as usize] = m.gpu;
        }

        let instances_addr = self.ring_push(device, slot, bytemuck::cast_slice(&instances))?;
        let lights_addr = self.ring_push(device, slot, bytemuck::cast_slice(&lights))?;
        let materials_addr = self.ring_push(device, slot, bytemuck::cast_slice(&materials))?;
        let ranges_addr = self.ring_push(device, slot, bytemuck::cast_slice(&assignment.ranges))?;
        let indices_addr =
            self.ring_push(device, slot, bytemuck::cast_slice(&assignment.indices))?;
        let split = |a: u64| [a as u32, (a >> 32) as u32];
        let frame_data = GpuFrame {
            view,
            proj,
            view_proj,
            inv_view_proj: view_proj.inverse(),
            unjit_view_proj: view_proj_unjit,
            inv_unjit_view_proj: view_proj_unjit.inverse(),
            prev_view_proj,
            cascade_vp,
            spot_vp,
            camera_pos: [cam.x, cam.y, cam.z, 1.0],
            cascade_splits: splits,
            cascade_texel,
            cascade_tex: std::array::from_fn(|i| self.cascades[i].index),
            spot_tex: std::array::from_fn(|i| self.spots[i].index),
            light_count: lights.len() as u32,
            dir_count: dir_count as u32,
            sampler_index: self.sampler_indices.0,
            shadow_sampler: self.shadow_sampler_index,
            exposure: scene.camera.exposure,
            debug_mode: self.debug_view as u32,
            pad0: 0,
            pad1: 0,
            sky_index: ibl.sky,
            irradiance_index: ibl.irradiance,
            brdf_lut_index: ibl.lut,
            ibl_sampler: ibl.sampler,
            prefilter: ibl.prefilter,
            env_intensity: scene.environment.intensity,
            pad2: 0,
            cam_right: [right.x, right.y, right.z, tan_y * aspect],
            cam_up: [up.x, up.y, up.z, tan_y],
            cam_forward: [forward.x, forward.y, forward.z, 0.0],
            cluster_ranges: split(ranges_addr),
            cluster_indices: split(indices_addr),
            cluster_params: [
                scene.camera.near,
                GRID.2 as f32 / (CLUSTER_FAR / scene.camera.near).ln(),
                w as f32,
                h as f32,
            ],
        };
        let addresses = [
            self.ring_push(device, slot, bytemuck::bytes_of(&frame_data))?,
            instances_addr,
            lights_addr,
            materials_addr,
        ];

        let mut b = GraphBuilder::<D>::new();
        let cascade_ids: [_; CASCADES] = std::array::from_fn(|i| {
            b.import(
                CASCADE_NAMES[i],
                self.cascades[i].texture,
                (SHADOW_SIZE, SHADOW_SIZE),
                None,
            )
        });
        let spot_ids: [_; MAX_SPOT_SHADOWS] = std::array::from_fn(|i| {
            b.import(
                SPOT_NAMES[i],
                self.spots[i].texture,
                (SPOT_SHADOW_SIZE, SPOT_SHADOW_SIZE),
                None,
            )
        });
        let ids = self.ibl.import(&mut b);
        let swapchain = b.import(
            "swapchain",
            target.texture,
            target.extent,
            Some(TextureState::Undefined),
        );
        let color_tex = |format| TransientDesc {
            width: w,
            height: h,
            format,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        };
        let hdr = b.transient("hdr", color_tex(HDR_FORMAT));
        let depth = b.transient(
            "depth",
            TransientDesc {
                format: DEPTH_FORMAT,
                usage: TextureUsage::DEPTH_ATTACHMENT | TextureUsage::SAMPLED,
                ..color_tex(DEPTH_FORMAT)
            },
        );
        let normal = b.transient("view_normal", color_tex(Format::Rg16Float));
        let velocity = b.transient("velocity", color_tex(Format::Rg16Float));
        let rebake = self.ibl.needs_bake(&scene.environment);
        if rebake {
            self.ibl.add_bakes(&mut b, &ids, &scene.environment);
        }
        let geometry = |pipes, draws| GeometryInputs {
            pipes,
            addresses,
            draws,
        };
        for c in 0..CASCADES {
            shadow_pass(
                &mut b,
                ShadowInputs {
                    name: CASCADE_NAMES[c],
                    texture: cascade_ids[c],
                    view_index: c as u32,
                    geometry: geometry(self.shadow_pipes, &cascade_draws[c]),
                },
            );
        }
        for i in 0..MAX_SPOT_SHADOWS {
            shadow_pass(
                &mut b,
                ShadowInputs {
                    name: SPOT_NAMES[i],
                    texture: spot_ids[i],
                    view_index: 4 + i as u32,
                    geometry: geometry(self.shadow_pipes, &spot_draws[i]),
                },
            );
        }
        prepass(
            &mut b,
            PrepassInputs {
                depth,
                normal,
                velocity,
                geometry: geometry(self.prepass, &opaque),
            },
        );
        // Half-resolution SSAO then a depth-aware blur/upsample to full resolution.
        let ssao = self.post.ssao.then(|| {
            let half = TransientDesc {
                width: w.div_ceil(2),
                height: h.div_ceil(2),
                ..color_tex(Format::R8Unorm)
            };
            let ao_half = b.transient("ssao.half", half);
            let ao = b.transient("ssao", color_tex(Format::R8Unorm));
            ssao_pass(
                &mut b,
                self.ssao_pipeline,
                depth,
                normal,
                ao_half,
                addresses,
            );
            ssao_blur_pass(
                &mut b,
                self.ssao_blur_pipeline,
                ao_half,
                depth,
                ao,
                addresses,
            );
            ao
        });
        let mut reads: Vec<_> = cascade_ids.into_iter().chain(spot_ids).collect();
        reads.extend([ids.sky, ids.irradiance, ids.lut]);
        reads.extend(ids.prefilter);
        match self.path {
            RenderPath::Forward => forward_pass(
                &mut b,
                ShadedInputs {
                    hdr,
                    depth,
                    reads: reads.clone(),
                    ssao,
                    geometry: geometry(self.forward, &opaque),
                },
            ),
            RenderPath::Deferred => {
                let gbuffer = [
                    b.transient("gbuffer.albedo", color_tex(Format::Rgba8Srgb)),
                    b.transient("gbuffer.normal", color_tex(Format::Rgba16Float)),
                    b.transient("gbuffer.emissive", color_tex(Format::Rgba16Float)),
                ];
                gbuffer_pass(
                    &mut b,
                    GbufferInputs {
                        targets: gbuffer,
                        depth,
                        geometry: geometry(self.gbuffer, &opaque),
                    },
                );
                lighting_pass(
                    &mut b,
                    LightingInputs {
                        pipeline: self.lighting,
                        hdr,
                        gbuffer,
                        depth,
                        reads: reads.clone(),
                        ssao,
                        addresses,
                    },
                );
            }
        }
        skybox_pass(
            &mut b,
            SkyboxInputs {
                pipeline: self.ibl.skybox,
                hdr,
                depth,
                sky: ids.sky,
                addresses,
            },
        );
        if !blended.is_empty() {
            transparent_pass(
                &mut b,
                ShadedInputs {
                    hdr,
                    depth,
                    reads,
                    ssao: None,
                    geometry: geometry(self.transparent, &blended),
                },
            );
        }
        // TAA resolves the lit HDR image into one of the two imported history textures.
        let mut color = hdr;
        if let Some(history) = self.history.as_ref().filter(|_| taa) {
            let prev = b.import(
                ["taa.history0", "taa.history1"][1 - self.taa_write],
                history.textures[1 - self.taa_write],
                history.extent,
                None,
            );
            let curr = b.import(
                ["taa.history0", "taa.history1"][self.taa_write],
                history.textures[self.taa_write],
                history.extent,
                None,
            );
            taa_pass(
                &mut b,
                TaaInputs {
                    pipeline: self.taa_pipeline,
                    hdr,
                    depth,
                    velocity,
                    history_prev: prev,
                    history_curr: curr,
                    reset: !self.taa_valid,
                    addresses,
                },
            );
            color = curr;
        }
        let bloom = self.post.bloom.then(|| {
            let size = |level: usize| ((w >> (level + 1)).max(1), (h >> (level + 1)).max(1));
            let mut tex = |name: &'static str, level: usize| {
                let (width, height) = size(level);
                let id = b.transient(
                    name,
                    TransientDesc {
                        width,
                        height,
                        ..color_tex(HDR_FORMAT)
                    },
                );
                (id, (width, height))
            };
            let targets = BloomTargets {
                down: std::array::from_fn(|l| tex(DOWN_NAMES[l], l)),
                up: std::array::from_fn(|l| tex(UP_NAMES[l], l)),
            };
            let result = targets.up[0].0;
            bloom_passes(
                &mut b,
                BloomInputs {
                    down_pipeline: self.bloom_down_pipeline,
                    up_pipeline: self.bloom_up_pipeline,
                    source: color,
                    source_extent: (w, h),
                    sampler_index: self.sampler_indices.1,
                    targets,
                },
            );
            result
        });
        tonemap_pass(
            &mut b,
            TonemapInputs {
                pipeline: self.tonemap,
                hdr: color,
                sampler_index: self.sampler_indices.1,
                exposure: scene.camera.exposure,
                encode_srgb: self.encode_srgb,
                target: swapchain,
                bloom,
                bloom_strength: BLOOM_STRENGTH,
            },
        );
        b.export(swapchain, final_state);
        let compiled = self.graph.compile(b)?;
        let compile_time = compiled.stats().compile_time;
        let mut graph_stats = self.graph.execute(compiled, device, cmd, frame)?;
        graph_stats.compile_time = compile_time;
        if rebake {
            self.ibl.mark_baked(&scene.environment);
        }
        self.prev_view_proj = view_proj_unjit;
        self.frame_index = self.frame_index.wrapping_add(1);
        if taa {
            self.taa_write = 1 - self.taa_write;
        }
        self.taa_valid = taa;

        self.stats = RendererStats {
            draws: (opaque.len() + blended.len()) as u32,
            triangles,
            lights: lights.len() as u32,
            ring_bytes_used: self.ring_used,
            graph: graph_stats,
            culled,
            shadow_draws: shadow_draw_count as u32,
            cluster_time,
        };
        if self.last_log.elapsed().as_secs() >= 1 {
            self.last_log = Instant::now();
            tracing::debug!(stats = ?self.stats, "renderer stats");
        }
        Ok(())
    }

    /// Destroys every GPU resource the renderer owns, including meshes and textures the app
    /// left alive.
    pub fn destroy<D: Device>(self, device: &D) {
        for (_, m) in self.meshes.iter() {
            device.destroy_buffer(m.vertices);
            device.destroy_buffer(m.indices);
        }
        for (_, t) in self.textures.iter() {
            device.destroy_texture(t.texture);
        }
        for (b, _) in self.rings {
            device.destroy_buffer(b);
        }
        self.graph.destroy(device);
        device.destroy_sampler(self.linear_repeat);
        device.destroy_sampler(self.linear_clamp);
        self.ibl.destroy(device);
        for m in self.cascades.iter().chain(&self.spots) {
            device.destroy_texture(m.texture);
        }
        device.destroy_sampler(self.shadow_sampler);
        if let Some(h) = self.history {
            for t in h.textures {
                device.destroy_texture(t);
            }
        }
        for p in self
            .shadow_pipes
            .into_iter()
            .chain(self.prepass)
            .chain(self.forward)
            .chain(self.gbuffer)
            .chain(self.transparent)
            .chain([
                self.lighting,
                self.tonemap,
                self.ssao_pipeline,
                self.ssao_blur_pipeline,
                self.taa_pipeline,
                self.bloom_down_pipeline,
                self.bloom_up_pipeline,
            ])
        {
            device.destroy_pipeline(p);
        }
        tracing::debug!("renderer destroyed");
    }
}
