//! The [`Renderer`]: owns pipelines, render targets, built-in textures and the per-frame upload
//! rings, and records the fixed pass sequence (ADR-0011 §3).

use std::time::Instant;

use foundation::Handle;
use foundation::Pool;
use foundation::math::{Mat4, Vec3};
use rhi::{
    AddressMode, BlendState, BufferDesc, BufferHandle, BufferUsage, CompareOp, CullMode, DepthBias,
    DepthState, Device, Filter, Format, FrontFace, GraphicsPipelineDesc, Memory, PipelineHandle,
    PrimitiveTopology, RhiError, SamplerDesc, SamplerHandle, ShaderModuleDesc, ShaderModuleHandle,
    ShaderStageDesc, SwapchainImage, TextureDesc, TextureHandle, TextureUsage,
};

use crate::gpu::{GpuFrame, GpuInstance, GpuLight, GpuMaterial};
use crate::ibl::Ibl;
use crate::passes::{
    Draw, ForwardInputs, ShadowInputs, TonemapInputs, forward_pass, shadow_pass, tonemap_pass,
};
use crate::shadow::fit_directional;
use crate::{Light, MAX_LIGHTS, MeshData, RenderScene};

static FORWARD_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/forward.vert.spv"));
static FORWARD_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/forward.frag.spv"));
static SHADOW_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shadow.vert.spv"));
static SHADOW_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shadow.frag.spv"));
pub(crate) static TONEMAP_VERT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/tonemap.vert.spv"));
static TONEMAP_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/tonemap.frag.spv"));

/// Size of each per-frame-in-flight upload ring.
// shortcut: fixed 4 MiB ring, render fails when a scene overflows it; grow or chain rings when scenes get bigger.
const RING_BYTES: u64 = 4 << 20;
/// Shadow map resolution (ADR-0011 §7).
const SHADOW_SIZE: u32 = 2048;
/// The shadow volume covers the camera frustum up to this distance (meters).
const SHADOW_DISTANCE: f32 = 50.0;
pub(crate) const HDR_FORMAT: Format = Format::Rgba16Float;
const DEPTH_FORMAT: Format = Format::D32Sfloat;

/// A GPU mesh (vertex buffer read through BDA, index buffer). Handle: [`MeshId`].
pub struct Mesh {
    vertices: BufferHandle,
    indices: BufferHandle,
    vertex_address: u64,
    index_count: u32,
}

/// A GPU texture with its bindless slot. Handle: [`TextureId`].
pub struct Texture {
    texture: TextureHandle,
    index: u32,
}

/// A material record (GPU layout). Handle: [`MaterialId`].
pub struct Material(GpuMaterial);

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
}

struct Targets {
    hdr: TextureHandle,
    hdr_index: u32,
    depth: TextureHandle,
    extent: (u32, u32),
    initialized: bool,
}

/// Forward renderer. All methods taking a device must be given the device it was created with.
pub struct Renderer {
    forward: PipelineHandle,
    shadow_pipeline: PipelineHandle,
    tonemap: PipelineHandle,
    shadow: TextureHandle,
    shadow_index: u32,
    shadow_initialized: bool,
    shadow_sampler: SamplerHandle,
    shadow_sampler_index: u32,
    ibl: Ibl,
    targets: Targets,
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

fn spirv_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

fn create_targets<D: Device>(d: &D, extent: (u32, u32)) -> Result<Targets, RhiError> {
    let (width, height) = (extent.0.max(1), extent.1.max(1));
    let hdr = d.create_texture(&TextureDesc {
        label: Some("renderer.hdr"),
        width,
        height,
        format: HDR_FORMAT,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
    })?;
    let depth = d.create_texture(&TextureDesc {
        label: Some("renderer.depth"),
        width,
        height,
        format: DEPTH_FORMAT,
        usage: TextureUsage::DEPTH_ATTACHMENT,
    })?;
    Ok(Targets {
        hdr,
        hdr_index: d.texture_index(hdr)?,
        depth,
        extent: (width, height),
        initialized: false,
    })
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
        blend: &[BlendState::default()][..color.len().min(1)],
    });
    // Module destruction is deferred by the device; the pipeline no longer needs them.
    d.destroy_shader_module(vs);
    d.destroy_shader_module(fs);
    pipeline
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
        let forward = make_pipeline(
            device,
            "renderer.forward",
            FORWARD_VERT,
            FORWARD_FRAG,
            &[HDR_FORMAT],
            Some(DepthState {
                format: DEPTH_FORMAT,
                compare: CompareOp::GreaterOrEqual, // reverse-Z
                write: true,
                bias: None,
            }),
            CullMode::Back,
        )?;
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
        let shadow_pipeline = make_pipeline(
            device,
            "renderer.shadow",
            SHADOW_VERT,
            SHADOW_FRAG,
            &[],
            Some(DepthState {
                format: DEPTH_FORMAT,
                compare: CompareOp::LessOrEqual,
                write: true,
                bias: Some(DepthBias {
                    constant: 2.0,
                    slope: 2.5,
                    clamp: 0.0,
                }),
            }),
            CullMode::Front,
        )?;
        let shadow = device.create_texture(&TextureDesc {
            label: Some("renderer.shadow"),
            width: SHADOW_SIZE,
            height: SHADOW_SIZE,
            format: DEPTH_FORMAT,
            usage: TextureUsage::DEPTH_ATTACHMENT | TextureUsage::SAMPLED,
        })?;
        let shadow_sampler = device.create_sampler(&SamplerDesc {
            label: Some("renderer.shadow_cmp"),
            mag_filter: Filter::Linear,
            min_filter: Filter::Linear,
            address_u: AddressMode::ClampToEdge,
            address_v: AddressMode::ClampToEdge,
            address_w: AddressMode::ClampToEdge,
            compare: Some(CompareOp::LessOrEqual),
        })?;
        let targets = create_targets(device, extent)?;
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
            shadow_pipeline,
            tonemap,
            shadow_index: device.texture_index(shadow)?,
            shadow,
            shadow_initialized: false,
            shadow_sampler_index: device.sampler_index(shadow_sampler)?,
            shadow_sampler,
            ibl: Ibl::new(device)?,
            targets,
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

    /// Recreates the size-dependent targets. Called automatically by [`Renderer::render`] when
    /// the swapchain image size differs.
    ///
    /// # Errors
    /// Any [`RhiError`] from texture creation.
    pub fn resize<D: Device>(&mut self, device: &D, extent: (u32, u32)) -> Result<(), RhiError> {
        if extent == self.targets.extent {
            return Ok(());
        }
        device.destroy_texture(self.targets.hdr);
        device.destroy_texture(self.targets.depth);
        self.targets = create_targets(device, extent)?;
        tracing::debug!(?extent, "renderer targets resized");
        Ok(())
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
        let id = self.meshes.insert(Mesh {
            vertices,
            indices,
            vertex_address: device.buffer_address(vertices)?,
            index_count: data.indices.len() as u32,
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
            pad: [0; 3],
        };
        let id = self.materials.insert(Material(m));
        tracing::debug!(label = desc.label, ?id, "material created");
        id
    }

    /// Destroys a material record.
    pub fn destroy_material(&mut self, id: MaterialId) {
        if self.materials.remove(id).is_some() {
            tracing::debug!(?id, "material destroyed");
        }
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

    /// Records the frame: forward opaque into HDR, then tonemap into `target`. `slot` is
    /// [`rhi::FrameToken::slot`]. `target` is left in [`rhi::TextureState::ColorAttachment`];
    /// the caller transitions it to `Present` (or `TransferSrc` for a screenshot).
    ///
    /// # Errors
    /// [`RhiError`] from target recreation or ring uploads (including ring overflow).
    pub fn render<D: Device>(
        &mut self,
        device: &D,
        cmd: &mut D::Commands,
        slot: u32,
        scene: &RenderScene,
        target: &SwapchainImage,
    ) -> Result<(), RhiError> {
        self.resize(device, target.extent)?;
        let slot = slot as usize;
        self.ring_used = 0;

        let (w, h) = self.targets.extent;
        let view = scene.camera.view();
        let proj = scene.camera.projection(w as f32 / h as f32);
        let cam = scene.camera.position;
        self.ibl.bake(cmd, &scene.environment);
        let ibl = self.ibl.indices();
        let aspect = w as f32 / h as f32;
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
        let lights: Vec<GpuLight> = scene
            .lights
            .iter()
            .take(MAX_LIGHTS)
            .map(GpuLight::new)
            .collect();
        let shadow_light = scene.lights.iter().take(MAX_LIGHTS).position(|l| {
            matches!(
                l,
                Light::Directional {
                    cast_shadows: true,
                    ..
                }
            )
        });
        let fit = shadow_light.and_then(|i| match scene.lights[i] {
            Light::Directional { direction, .. } => Some(fit_directional(
                &scene.camera,
                w as f32 / h as f32,
                direction,
                SHADOW_DISTANCE,
                SHADOW_SIZE,
            )),
            _ => None,
        });
        let frame = GpuFrame {
            view,
            proj,
            view_proj: proj * view,
            light_view_proj: fit.as_ref().map_or(Mat4::IDENTITY, |f| f.view_proj),
            camera_pos: [cam.x, cam.y, cam.z, 1.0],
            light_count: lights.len() as u32,
            sampler_index: self.sampler_indices.0,
            shadow_index: self.shadow_index,
            shadow_sampler: self.shadow_sampler_index,
            exposure: scene.camera.exposure,
            shadow_light: if fit.is_some() {
                shadow_light.map_or(u32::MAX, |i| i as u32)
            } else {
                u32::MAX
            },
            shadow_texel_world: fit.as_ref().map_or(0.0, |f| f.texel_world),
            pad: 0.0,
            sky_index: ibl.sky,
            irradiance_index: ibl.irradiance,
            brdf_lut_index: ibl.lut,
            ibl_sampler: ibl.sampler,
            prefilter: ibl.prefilter,
            env_intensity: scene.environment.intensity,
            pad0: 0,
            cam_right: [right.x, right.y, right.z, tan_y * aspect],
            cam_up: [up.x, up.y, up.z, tan_y],
            cam_forward: [forward.x, forward.y, forward.z, 0.0],
        };

        let mut instances = Vec::with_capacity(scene.meshes.len());
        let mut draws = Vec::with_capacity(scene.meshes.len());
        let mut triangles = 0u64;
        for inst in &scene.meshes {
            let Some(mesh) = self.meshes.get(inst.mesh) else {
                tracing::warn!(mesh = ?inst.mesh, "scene references a destroyed mesh, skipped");
                continue;
            };
            let material = if self.materials.get(inst.material).is_some() {
                inst.material
            } else {
                self.default_material
            };
            draws.push(Draw {
                vertex_address: mesh.vertex_address,
                index_buffer: mesh.indices,
                index_count: mesh.index_count,
                instance: instances.len() as u32,
            });
            triangles += u64::from(mesh.index_count / 3);
            instances.push(GpuInstance {
                model: inst.transform,
                normal_matrix: inst.transform.inverse().transpose(),
                material: material.index(),
                pad: [0; 3],
            });
        }
        let dense = self
            .materials
            .iter()
            .map(|(h, _)| h.index() + 1)
            .max()
            .unwrap_or(1) as usize;
        let mut materials = vec![bytemuck::Zeroable::zeroed(); dense];
        for (h, m) in self.materials.iter() {
            materials[h.index() as usize] = m.0;
        }

        let addresses = [
            self.ring_push(device, slot, bytemuck::bytes_of(&frame))?,
            self.ring_push(device, slot, bytemuck::cast_slice(&instances))?,
            self.ring_push(device, slot, bytemuck::cast_slice(&lights))?,
            self.ring_push(device, slot, bytemuck::cast_slice(&materials))?,
        ];

        let no_draws: &[Draw] = &[];
        shadow_pass(
            cmd,
            &ShadowInputs {
                pipeline: self.shadow_pipeline,
                texture: self.shadow,
                size: SHADOW_SIZE,
                initialized: self.shadow_initialized,
                addresses,
                draws: if fit.is_some() { &draws } else { no_draws },
            },
        );
        self.shadow_initialized = true;
        forward_pass(
            cmd,
            &ForwardInputs {
                pipeline: self.forward,
                hdr: self.targets.hdr,
                depth: self.targets.depth,
                extent: self.targets.extent,
                targets_initialized: self.targets.initialized,
                addresses,
                draws: &draws,
                skybox: self.ibl.skybox,
            },
        );
        self.targets.initialized = true;
        tonemap_pass(
            cmd,
            &TonemapInputs {
                pipeline: self.tonemap,
                hdr_index: self.targets.hdr_index,
                sampler_index: self.sampler_indices.1,
                exposure: scene.camera.exposure,
                encode_srgb: self.encode_srgb,
                target: target.texture,
                extent: target.extent,
            },
        );

        self.stats = RendererStats {
            draws: draws.len() as u32,
            triangles,
            lights: lights.len() as u32,
            ring_bytes_used: self.ring_used,
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
        device.destroy_texture(self.targets.hdr);
        device.destroy_texture(self.targets.depth);
        device.destroy_sampler(self.linear_repeat);
        device.destroy_sampler(self.linear_clamp);
        self.ibl.destroy(device);
        device.destroy_texture(self.shadow);
        device.destroy_sampler(self.shadow_sampler);
        device.destroy_pipeline(self.shadow_pipeline);
        device.destroy_pipeline(self.forward);
        device.destroy_pipeline(self.tonemap);
        tracing::debug!("renderer destroyed");
    }
}
