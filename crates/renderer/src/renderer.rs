//! The [`Renderer`]: owns pipelines, persistent textures, built-in textures and the per-frame
//! upload rings, and builds the frame as a render graph (ADR-0011 §3, ADR-0012).

use std::time::{Duration, Instant};

use foundation::Handle;
use foundation::Pool;
use foundation::math::{Mat4, Vec3};
use rayon::prelude::*;
use render_graph::{
    BufId, GpuTimings, GraphBuilder, GraphError, GraphStats, Pass, RenderGraph, TexId,
    TransientBufferDesc, TransientDesc,
};
use rhi::{
    AddressMode, BlendFactor, BlendState, BufferDesc, BufferHandle, BufferState, BufferUsage,
    CommandEncoder, CompareOp, ComputePipelineDesc, CullMode, DepthBias, DepthState, Device,
    Filter, Format, FrameToken, FrontFace, GraphicsPipelineDesc, Memory, PipelineHandle,
    PrimitiveTopology, RhiError, SamplerDesc, SamplerHandle, ShaderModuleDesc, ShaderModuleHandle,
    ShaderStageDesc, SwapchainImage, TextureDesc, TextureHandle, TextureState, TextureUsage,
    TextureViewDesc,
};

use crate::arena::{Arena, ArenaSlot};

use crate::cluster::{CLUSTER_FAR, ClusterGrid, GRID, LightSphere, spot_bounds};
use crate::cull::{Frustum, bounding_sphere, transform_sphere};
use crate::debug::DebugLines;
use crate::gpu::{
    COUNTS_BYTES, CULL_VIEWS, GpuCullParams, GpuFrame, GpuInstance, GpuLight, GpuMaterial, GpuMesh,
    SHADOW_VIEWS,
};
use crate::gpu_cull::{
    CullBuffers, CullLayout, CullPipelines, CullStats, Pyramid, camera_cull_pass, hiz_passes,
    reset_pass, shadow_cull_pass, stats_pass,
};
use crate::ibl::Ibl;
use crate::mesh::{MAX_LODS, generate_lods};
use crate::passes::{
    ArenaRef, DebugInputs, Draw, DrawSource, GbufferInputs, GeometryInputs, GpuDraws,
    LightingInputs, Pipes, PrepassInputs, ShadedInputs, ShadowInputs, SkyboxInputs, TonemapInputs,
    debug_pass, forward_pass, gbuffer_pass, lighting_pass, prepass, shadow_pass, skybox_pass,
    tonemap_pass, transparent_pass,
};
use crate::post::{
    BloomInputs, BloomTargets, DOWN_NAMES, TaaInputs, UP_NAMES, bloom_passes, jitter_pixels,
    jitter_projection, ssao_blur_pass, ssao_pass, taa_pass,
};
use crate::prep::{self, Resolved};
use crate::shadow::{
    CASCADES, POINT_SHADOW_SIZE, cascade_splits, fit_directional, fit_spot, point_face_view_proj,
    select_point_shadows,
};
use crate::streaming::{self, PlanItem};
use crate::{Light, MAX_LIGHTS, MAX_POINT_SHADOWS, MAX_SPOT_SHADOWS, MeshData, RenderScene};

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
static CULL_CAMERA_COMP: &[u8] = spv!("cull_camera.comp");
static CULL_SHADOW_COMP: &[u8] = spv!("cull_shadow.comp");
static HIZ_COMP: &[u8] = spv!("hiz.comp");
static DEBUG_VERT: &[u8] = spv!("debug.vert");
static DEBUG_FRAG: &[u8] = spv!("debug.frag");

/// Size of each per-frame-in-flight upload ring: frame data, lights, materials, cluster lists,
/// culling parameters and the changed instances of the frame.
// shortcut: fixed 16 MiB ring, render fails when a frame's uploads overflow it (frame data is small; changed instances above 70 000 per frame go through `Device::upload_buffer` instead).
const RING_BYTES: u64 = 16 << 20;
/// Mesh records the GPU mesh table holds. shortcut: fixed size (320 KiB), `create_mesh` fails
/// beyond it; grow like the arena when a scene needs more meshes.
const MESH_TABLE_CAPACITY: u32 = 4096;
/// Instance buffer capacity at creation; it doubles when the scene outgrows it.
const INITIAL_INSTANCES: u32 = 4096;
/// A changed-instance run is merged with the next one when at most this many unchanged
/// instances separate them (one copy instead of two).
const RUN_GAP: u32 = 4;
/// Culling views of the camera passes (see `cull.glsl`): phase 1, phase 2, or both.
const PHASE1_VIEWS: &[usize] = &[0];
const PHASE2_VIEWS: &[usize] = &[1];
const CAMERA_VIEWS: &[usize] = &[0, 1];
/// `0, 1, 2, ...`: single-view slices for the geometry passes.
static VIEW_IDS: [usize; CULL_VIEWS] = {
    let mut a = [0; CULL_VIEWS];
    let mut i = 0;
    while i < CULL_VIEWS {
        a[i] = i;
        i += 1;
    }
    a
};

/// The culling view of shadow view `k` (4 cascades, 4 spot lights, then 6 faces per point light).
fn shadow_cull_view(k: usize) -> &'static [usize] {
    &VIEW_IDS[2 + k..3 + k]
}
/// Default LOD error threshold: a level is used while its projected geometric error stays under
/// this many pixels. `ANVIL_LOD=0` disables LODs, `ANVIL_LOD=<px>` overrides.
const DEFAULT_LOD_PIXELS: f32 = 0.5;
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
const POINT_NAMES: [&str; MAX_POINT_SHADOWS] = [
    "shadow.point0",
    "shadow.point1",
    "shadow.point2",
    "shadow.point3",
];
const SPOT_NAMES: [&str; MAX_SPOT_SHADOWS] = [
    "shadow.spot0",
    "shadow.spot1",
    "shadow.spot2",
    "shadow.spot3",
];
pub(crate) const HDR_FORMAT: Format = Format::Rgba16Float;
const DEPTH_FORMAT: Format = Format::D32Sfloat;

/// A mesh in the shared arena (vertices and indices of all its levels of detail) with its
/// object-space bounding sphere and GPU record. Handle: [`MeshId`].
pub struct Mesh {
    slot: ArenaSlot,
    record: GpuMesh,
    center: Vec3,
    radius: f32,
}

/// A streamed texture (ADR-0013 §8, LAYER-007 §18). Handle: [`TextureId`].
///
/// The CPU keeps the whole mip chain; the GPU image holds the levels from `base` down, so its
/// bindless slot changes when the residency does (materials look the slot up every frame).
pub struct Texture {
    /// The GPU image: CPU level `base` and every coarser one.
    texture: TextureHandle,
    index: u32,
    label: String,
    width: u32,
    height: u32,
    srgb: bool,
    /// The full chain, level 0 first.
    mips: Vec<Vec<u8>>,
    /// Resident base level and the coarsest level residency may drop to.
    base: u32,
    floor: u32,
    /// Frame of the last residency change.
    changed: u64,
    /// Pixels the largest object using this texture covered recently (a slowly decaying maximum).
    pixels: f32,
    /// `suffix[b]`: resident bytes with base level `b`.
    suffix: Vec<u64>,
}

/// A material record (GPU layout plus the alpha mode that selects pipelines). Handle: [`MaterialId`].
pub struct Material {
    /// Factors; the texture slots are filled in every frame from `textures`.
    gpu: GpuMaterial,
    /// Base color, metallic-roughness, normal, occlusion and emissive textures (defaults resolved).
    textures: [TextureId; 5],
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
    /// Level of detail of each instance tinted: 0 green, 1 yellow, 2 orange, 3 red (GPU culling).
    Lod = 4,
    /// Instances that occlusion culling phase 2 drew this frame tinted magenta (GPU culling).
    Occlusion,
    /// Resident mip level of each material's base color texture tinted: 0 green, 1 light green,
    /// 2 yellow, 3 orange, 4 red, 5 and up magenta (texture streaming).
    Mips,
}

impl DebugView {
    /// Parses the `ANVIL_DEBUG_VIEW` names: `clusters`, `cascades`, `ssao`, `lod`, `occlusion`,
    /// `mips`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "clusters" => Self::Clusters,
            "cascades" => Self::Cascades,
            "ssao" => Self::Ssao,
            "lod" => Self::Lod,
            "occlusion" => Self::Occlusion,
            "mips" => Self::Mips,
            _ => return None,
        })
    }
}

/// Which debug geometry the renderer draws by itself (`ANVIL_DEBUG_DRAW`); the application can
/// always add its own lines through [`Renderer::debug_lines`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugDraw {
    /// Nothing.
    #[default]
    None,
    /// Bounding spheres of the instances within 60 m of the camera (objects over 50 m radius,
    /// like a ground plane, are skipped).
    Bounds,
    /// The four directional shadow cascade volumes.
    Cascades,
    /// Point-light ranges and spot-light cones.
    Lights,
}

impl DebugDraw {
    /// Parses the `ANVIL_DEBUG_DRAW` names: `bounds`, `cascades`, `lights`.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "bounds" => Self::Bounds,
            "cascades" => Self::Cascades,
            "lights" => Self::Lights,
            _ => return None,
        })
    }
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

/// CPU time of each preparation stage of one frame (ADR-0013 §2); each stage is also a
/// `tracing` span named `renderer.<stage>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageTimes {
    /// Cluster light assignment.
    pub clusters: Duration,
    /// Per-instance world bounds and instance data packing.
    pub pack: Duration,
    /// Camera frustum culling.
    pub cull: Duration,
    /// Sorting the camera draw lists.
    pub sort: Duration,
    /// Culling and ordering the shadow-view draw lists.
    pub shadow_lists: Duration,
    /// Copying frame data into the upload ring.
    pub upload: Duration,
    /// Texture streaming: need estimate, residency plan and the uploads it triggers.
    pub streaming: Duration,
    /// Declaring the render graph (passes and bodies).
    pub graph_build: Duration,
    /// [`RenderGraph::compile`].
    pub compile: Duration,
    /// [`RenderGraph::execute`] (command recording).
    pub execute: Duration,
}

/// Guard that times a stage and keeps its span entered until dropped.
struct StageGuard<'a> {
    start: Instant,
    out: &'a mut Duration,
    _span: tracing::span::EnteredSpan,
}

impl<'a> StageGuard<'a> {
    fn new(span: tracing::Span, out: &'a mut Duration) -> Self {
        Self {
            start: Instant::now(),
            out,
            _span: span.entered(),
        }
    }
}

impl Drop for StageGuard<'_> {
    fn drop(&mut self) {
        *self.out += self.start.elapsed();
    }
}

/// Texture streaming counters (ADR-0013 §8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureStats {
    /// Textures.
    pub textures: u32,
    /// Bytes resident on the GPU.
    pub resident_bytes: u64,
    /// The VRAM budget (`ANVIL_TEXTURE_BUDGET_MB`).
    pub budget_bytes: u64,
    /// Bytes if every texture were fully resident.
    pub full_bytes: u64,
    /// Textures that want a finer level than they have (the upload budget deferred them).
    pub pending_uploads: u32,
    /// Bytes still to upload for them.
    pub pending_bytes: u64,
    /// Textures re-uploaded this frame (promotions and demotions).
    pub uploads: u32,
    /// Bytes uploaded this frame.
    pub uploaded_bytes: u64,
    /// Textures dropped to a coarser level so far.
    pub evictions: u64,
}

/// Per-frame counters (ADR-0011 §11), logged once per second at `debug`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RendererStats {
    /// Draws of the camera view: with GPU culling the indirect commands of both phases (read
    /// back, `frames_in_flight` old) plus the blended draws; otherwise the CPU draws.
    pub draws: u32,
    /// Triangles of the camera view after LOD selection (same delay and composition as `draws`).
    pub triangles: u64,
    /// Lights uploaded.
    pub lights: u32,
    /// Bytes of the frame's upload ring in use.
    pub ring_bytes_used: u64,
    /// Render graph counters, including compile time.
    pub graph: GraphStats,
    /// Instances rejected by the camera frustum test (read back with GPU culling).
    pub culled: u32,
    /// Draws recorded into all shadow maps (read back with GPU culling).
    pub shadow_draws: u32,
    /// The frame used GPU-driven culling, LOD and indirect draws (`ANVIL_GPU_CULLING`).
    pub gpu_culling: bool,
    /// Counters of the GPU culling passes of an earlier frame (see [`CullStats::frame`]); zero
    /// without GPU culling. `visible`, `frustum_culled` and `occlusion_culled` live here.
    pub cull: CullStats,
    /// Instances re-uploaded to the persistent instance buffer this frame.
    pub instances_uploaded: u32,
    /// Instances in the scene (slots of the instance buffer).
    pub instances: u32,
    /// Point lights that rendered a cube shadow map this frame.
    pub point_shadows: u32,
    /// Texture streaming counters.
    pub textures: TextureStats,
    /// CPU time of each preparation stage.
    pub stages: StageTimes,
    /// Per-pass GPU times of an earlier frame (`frames_in_flight` old, see
    /// [`GpuTimings::frame`]); empty until the first one retires or without timestamp support.
    pub gpu: GpuTimings,
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
    /// Shadow pipelines of the cube faces: the face images are mirrored, so the culled winding is
    /// the opposite of the other shadow maps.
    shadow_cube_pipes: Pipes,
    tonemap: PipelineHandle,
    debug_pipeline: PipelineHandle,
    cascades: [ShadowMap; CASCADES],
    spots: [ShadowMap; MAX_SPOT_SHADOWS],
    points: [PointShadow; MAX_POINT_SHADOWS],
    debug_lines: DebugLines,
    debug_draw: DebugDraw,
    /// VRAM budget for texture residency and the per-frame upload budget (bytes).
    texture_budget: u64,
    upload_budget: u64,
    /// Textures demoted so far.
    evictions: u64,
    texture_stats: TextureStats,
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
    arena: Arena,
    /// GPU table of mesh records, indexed by mesh handle index.
    mesh_table: BufferHandle,
    mesh_table_address: u64,
    /// The persistent GPU scene: per-slot instance data and last frame's visible flags.
    scene: SceneBuffers,
    cull_pipes: CullPipelines,
    pyramid: Option<Pyramid>,
    /// Per frame slot: counters readback buffer, and the frame whose counters it holds.
    stats_readback: Vec<(BufferHandle, Option<u64>)>,
    cull_stats: CullStats,
    gpu_culling: bool,
    lod_pixels: f32,
    meshes: Pool<Mesh>,
    textures: Pool<Texture>,
    materials: Pool<Material>,
    rings: Vec<(BufferHandle, u64)>,
    ring_used: u64,
    stats: RendererStats,
    /// Latest per-pass GPU timings read back.
    gpu: GpuTimings,
    /// The CPU side of the persistent scene: change detection and per-slot items.
    cache: prep::SceneCache,
    last_log: Instant,
}

/// The persistent instance buffers (ADR-0013 §6).
struct SceneBuffers {
    instances: BufferHandle,
    address: u64,
    /// One u32 per instance: in the camera's visible set last frame.
    visible: BufferHandle,
    visible_address: u64,
    capacity: u32,
}

/// Builds the [`GeometryInputs`] of the geometry passes of a frame.
struct GeometryBuilder {
    addresses: [u64; 4],
    arena: ArenaRef,
    instances: BufId,
    /// GPU culling: command buffer, counts buffer and the command layout.
    gpu: Option<(BufId, BufId, CullLayout)>,
}

impl GeometryBuilder {
    /// CPU-recorded draws.
    fn cpu<'a>(&self, pipes: Pipes, draws: &'a [Draw]) -> GeometryInputs<'a> {
        GeometryInputs {
            pipes,
            addresses: self.addresses,
            arena: self.arena,
            instances: self.instances,
            source: DrawSource::Cpu(draws),
        }
    }

    /// The GPU commands of the culling `views` when GPU culling is on, else the CPU `draws`.
    fn pick<'a>(
        &self,
        pipes: Pipes,
        draws: &'a [Draw],
        views: &'static [usize],
    ) -> GeometryInputs<'a> {
        match self.gpu {
            Some((cmds, counts, layout)) => GeometryInputs {
                source: DrawSource::Gpu(GpuDraws {
                    cmds,
                    counts,
                    layout,
                    views,
                }),
                ..self.cpu(pipes, draws)
            },
            None => self.cpu(pipes, draws),
        }
    }
}

/// One copy of changed instances from the frame's ring into the instance buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UploadRun {
    ring_offset: u64,
    dst_offset: u64,
    size: u64,
}

/// Merges ascending `slots` into runs `(first, end)` (end exclusive), joining runs that are at
/// most `gap` slots apart.
fn merge_runs(slots: &[u32], gap: u32) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &s in slots {
        match runs.last_mut() {
            Some(r) if s <= r.1 + gap => r.1 = s + 1,
            _ => runs.push((s, s + 1)),
        }
    }
    runs
}

/// A persistent depth texture rendered by a shadow pass and sampled by the shaders.
struct ShadowMap {
    texture: TextureHandle,
    index: u32,
}

/// A depth cube map of a point light: `cube` is sampled through `index` (a `textureCube` slot),
/// each of the six `faces` is a one-layer view the passes render into.
struct PointShadow {
    cube: TextureHandle,
    faces: [TextureHandle; 6],
    index: u32,
}

fn point_shadow<D: Device>(d: &D, label: &str) -> Result<PointShadow, RhiError> {
    let usage = TextureUsage::DEPTH_ATTACHMENT | TextureUsage::SAMPLED;
    let cube = d.create_texture(&TextureDesc {
        label: Some(label),
        width: POINT_SHADOW_SIZE,
        height: POINT_SHADOW_SIZE,
        format: DEPTH_FORMAT,
        usage,
        mip_levels: 1,
        array_layers: 6,
        cube: true,
    })?;
    let mut faces = [TextureHandle::NULL; 6];
    let made = (|| {
        for (f, face) in faces.iter_mut().enumerate() {
            *face = d.create_texture_view(
                cube,
                &TextureViewDesc {
                    label: Some("shadow.point.face"),
                    base_mip: 0,
                    mip_count: 1,
                    base_layer: f as u32,
                    layer_count: 1,
                    usage,
                },
            )?;
        }
        d.texture_index(cube)
    })();
    match made {
        Ok(index) => Ok(PointShadow { cube, faces, index }),
        Err(e) => {
            for f in faces.into_iter().filter(|f| *f != TextureHandle::NULL) {
                d.destroy_texture(f);
            }
            d.destroy_texture(cube);
            Err(e)
        }
    }
}

/// Per-frame decay of a texture's covered-pixels maximum: about a quarter second to halve at 60 fps.
const PIXEL_DECAY: f32 = 0.95;

/// Bytes from the environment variable `name` given in MiB (fractions allowed), or `default_mb`.
fn env_megabytes(name: &str, default_mb: f64) -> u64 {
    let mb = std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v >= 0.0)
        .unwrap_or(default_mb);
    (mb * (1u64 << 20) as f64) as u64
}

/// Creates the GPU image of `tex`'s chain from level `base` down and uploads it.
fn make_resident<D: Device>(
    device: &D,
    tex: &Texture,
    base: u32,
) -> Result<(TextureHandle, u32), RhiError> {
    let (w, h) = streaming::level_size(tex.width, tex.height, base);
    let texture = device.create_texture(&TextureDesc {
        label: Some(&tex.label),
        width: w,
        height: h,
        format: if tex.srgb {
            Format::Rgba8Srgb
        } else {
            Format::Rgba8Unorm
        },
        usage: TextureUsage::SAMPLED | TextureUsage::TRANSFER_DST,
        mip_levels: tex.mips.len() as u32 - base,
        array_layers: 1,
        cube: false,
    })?;
    let levels: Vec<&[u8]> = tex.mips[base as usize..]
        .iter()
        .map(Vec::as_slice)
        .collect();
    match device
        .upload_texture_mips(texture, &levels)
        .and_then(|()| device.texture_index(texture))
    {
        Ok(index) => Ok((texture, index)),
        Err(e) => {
            device.destroy_texture(texture);
            Err(e)
        }
    }
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
    make_pipeline_topology(
        d,
        label,
        vert,
        frag,
        color,
        depth,
        cull,
        blend,
        PrimitiveTopology::TriangleList,
    )
}

#[allow(clippy::too_many_arguments)]
fn make_pipeline_topology<D: Device>(
    d: &D,
    label: &str,
    vert: &[u8],
    frag: &[u8],
    color: &[Format],
    depth: Option<DepthState>,
    cull: CullMode,
    blend: &[BlendState],
    topology: PrimitiveTopology,
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
        topology,
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
        mip_levels: 1,
        array_layers: 1,
        cube: false,
    })?;
    Ok(ShadowMap {
        texture,
        index: d.texture_index(texture)?,
    })
}

fn make_compute<D: Device>(d: &D, label: &str, spirv: &[u8]) -> Result<PipelineHandle, RhiError> {
    let module = d.create_shader_module(&ShaderModuleDesc {
        spirv: &spirv_words(spirv),
        label: Some(label),
    })?;
    let pipeline = d.create_compute_pipeline(&ComputePipelineDesc {
        label: Some(label),
        shader: ShaderStageDesc {
            module,
            entry: "main",
        },
    });
    d.destroy_shader_module(module);
    pipeline
}

/// Creates the instance and visible-flag buffers for `capacity` instances. The visible flags
/// start zeroed (nothing was visible last frame).
fn new_scene_buffers<D: Device>(device: &D, capacity: u32) -> Result<SceneBuffers, RhiError> {
    let usage = BufferUsage::STORAGE | BufferUsage::TRANSFER_DST;
    let instances = device.create_buffer(&BufferDesc {
        label: Some("scene.instances"),
        size: u64::from(capacity) * size_of::<GpuInstance>() as u64,
        usage,
        memory: Memory::GpuOnly,
    })?;
    let visible_bytes = u64::from(capacity) * 4;
    let visible = match device.create_buffer(&BufferDesc {
        label: Some("scene.visible"),
        size: visible_bytes,
        usage,
        memory: Memory::GpuOnly,
    }) {
        Ok(b) => b,
        Err(e) => {
            device.destroy_buffer(instances);
            return Err(e);
        }
    };
    device.upload_buffer(visible, 0, &vec![0u8; visible_bytes as usize])?;
    Ok(SceneBuffers {
        instances,
        address: device.buffer_address(instances)?,
        visible,
        visible_address: device.buffer_address(visible)?,
        capacity,
    })
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
        // Configure the worker pool (ANVIL_THREADS) before the first parallel frame.
        foundation::init_thread_pool();
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
        let shadow_bias = Some(DepthBias {
            constant: 2.0,
            slope: 2.5,
            clamp: 0.0,
        });
        let shadow_cube_pipes = make_variants(
            device,
            "renderer.shadow_cube",
            SHADOW_VERT,
            &SHADOW_FRAGS,
            &[],
            depth_state(CompareOp::LessOrEqual, true, shadow_bias),
            CullMode::Back,
            &[],
        )?;
        let debug_pipeline = make_pipeline_topology(
            device,
            "renderer.debug_lines",
            DEBUG_VERT,
            DEBUG_FRAG,
            &[HDR_FORMAT],
            depth_state(CompareOp::GreaterOrEqual, false, None),
            CullMode::None,
            &none,
            PrimitiveTopology::LineList,
        )?;
        let mut points = Vec::new();
        for name in POINT_NAMES {
            points.push(point_shadow(device, name)?);
        }
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
        for f in points.iter().flat_map(|p| p.faces) {
            graph.track(f, TextureState::Undefined);
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
                usage: BufferUsage::STORAGE | BufferUsage::TRANSFER_SRC,
                memory: Memory::Upload,
            })?;
            rings.push((b, device.buffer_address(b)?));
        }
        let arena = Arena::new(device)?;
        let mesh_table = device.create_buffer(&BufferDesc {
            label: Some("renderer.mesh_table"),
            size: u64::from(MESH_TABLE_CAPACITY) * size_of::<GpuMesh>() as u64,
            usage: BufferUsage::STORAGE | BufferUsage::TRANSFER_DST,
            memory: Memory::GpuOnly,
        })?;
        let cull_pipes = CullPipelines {
            camera: make_compute(device, "renderer.cull_camera", CULL_CAMERA_COMP)?,
            shadow: make_compute(device, "renderer.cull_shadow", CULL_SHADOW_COMP)?,
            hiz: make_compute(device, "renderer.hiz", HIZ_COMP)?,
        };
        let mut stats_readback = Vec::new();
        for i in 0..frames {
            let b = device.create_buffer(&BufferDesc {
                label: Some(&format!("renderer.cull_stats{i}")),
                size: COUNTS_BYTES,
                usage: BufferUsage::TRANSFER_DST,
                memory: Memory::Readback,
            })?;
            stats_readback.push((b, None));
        }
        let mut r = Self {
            forward,
            prepass,
            gbuffer,
            transparent,
            lighting,
            shadow_pipes,
            shadow_cube_pipes,
            tonemap,
            debug_pipeline,
            cascades: cascades.try_into().ok().expect("4 cascades"),
            spots: spots.try_into().ok().expect("4 spot maps"),
            points: points.try_into().ok().expect("4 point maps"),
            debug_lines: DebugLines::default(),
            debug_draw: DebugDraw::None,
            texture_budget: env_megabytes("ANVIL_TEXTURE_BUDGET_MB", 512.0),
            upload_budget: env_megabytes("ANVIL_TEXTURE_UPLOAD_MB", 8.0),
            evictions: 0,
            texture_stats: TextureStats::default(),
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
            arena,
            mesh_table,
            mesh_table_address: device.buffer_address(mesh_table)?,
            scene: new_scene_buffers(device, INITIAL_INSTANCES)?,
            cull_pipes,
            pyramid: None,
            stats_readback,
            cull_stats: CullStats::default(),
            gpu_culling: std::env::var("ANVIL_GPU_CULLING").as_deref() != Ok("0"),
            lod_pixels: std::env::var("ANVIL_LOD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_LOD_PIXELS),
            meshes: Pool::new(),
            textures: Pool::new(),
            materials: Pool::new(),
            rings,
            ring_used: 0,
            stats: RendererStats::default(),
            gpu: GpuTimings::default(),
            cache: prep::SceneCache::default(),
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

    /// Uploads a mesh into the shared mesh arena. `data.lods` are used when present, otherwise
    /// up to three coarser levels are generated with `meshopt` ([`generate_lods`]).
    ///
    /// # Errors
    /// [`RhiError::InvalidDescriptor`] for an empty mesh, bad LOD indices, a full arena or mesh
    /// table, or any upload error.
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
        let generated;
        let lods = if data.lods.is_empty() {
            generated = generate_lods(data);
            &generated
        } else {
            &data.lods
        };
        let lods = &lods[..lods.len().min(MAX_LODS - 1)];
        let vertex_count = data.vertices.len() as u32;
        let levels = std::iter::once(&data.indices).chain(lods.iter().map(|l| &l.indices));
        let mut record = GpuMesh {
            lod_count: 1 + lods.len() as u32,
            ..GpuMesh::default()
        };
        let mut all_indices: Vec<u32> = Vec::new();
        for (l, indices) in levels.enumerate() {
            if indices.is_empty()
                || indices.len() % 3 != 0
                || indices.iter().any(|&i| i >= vertex_count)
            {
                return Err(RhiError::InvalidDescriptor(format!(
                    "mesh {label}: level {l} has no triangles or an index out of range"
                )));
            }
            record.first_index[l] = all_indices.len() as u32;
            record.index_count[l] = indices.len() as u32;
            all_indices.extend_from_slice(indices);
        }
        for (l, lod) in lods.iter().enumerate() {
            record.lod_error[l + 1] = lod.error;
        }
        let (center, radius) = bounding_sphere(data.vertices.iter().map(|v| Vec3::from(v.pos)));
        record.sphere = [center.x, center.y, center.z, radius];
        let slot = self.arena.alloc(vertex_count, all_indices.len() as u32)?;
        record.vertex_offset = slot.vertex as i32;
        for r in &mut record.first_index[..record.lod_count as usize] {
            *r += slot.index;
        }
        let id = self.meshes.insert(Mesh {
            slot,
            record,
            center,
            radius,
        });
        let uploaded = if id.index() >= MESH_TABLE_CAPACITY {
            Err(RhiError::InvalidDescriptor(format!(
                "mesh table full ({MESH_TABLE_CAPACITY} meshes)"
            )))
        } else {
            let vertex_bytes = size_of::<crate::Vertex>() as u64;
            device
                .upload_buffer(
                    self.arena.vertices,
                    u64::from(slot.vertex) * vertex_bytes,
                    bytemuck::cast_slice(&data.vertices),
                )
                .and_then(|()| {
                    device.upload_buffer(
                        self.arena.indices,
                        u64::from(slot.index) * 4,
                        bytemuck::cast_slice(&all_indices),
                    )
                })
                .and_then(|()| {
                    device.upload_buffer(
                        self.mesh_table,
                        u64::from(id.index()) * size_of::<GpuMesh>() as u64,
                        bytemuck::bytes_of(&record),
                    )
                })
        };
        if let Err(e) = uploaded {
            self.meshes.remove(id);
            self.arena.free(slot);
            return Err(e);
        }
        tracing::debug!(
            label,
            ?id,
            vertices = data.vertices.len(),
            indices = data.indices.len(),
            lods = record.lod_count,
            arena_vertex = slot.vertex,
            arena_index = slot.index,
            "mesh created"
        );
        Ok(id)
    }

    /// Destroys a mesh: its arena range is reused. Instances that still reference it are skipped
    /// (with a warning) from the next frame on.
    pub fn destroy_mesh<D: Device>(&mut self, _device: &D, id: MeshId) {
        if let Some(m) = self.meshes.remove(id) {
            self.arena.free(m.slot);
            self.cache.invalidate();
            tracing::debug!(?id, "mesh destroyed");
        }
    }

    /// Registers a tightly packed RGBA8 image (`srgb` selects the color space). The full mip chain
    /// is generated on the CPU (box filter, in linear light for sRGB) and kept; the GPU holds only
    /// the levels the streaming policy wants resident (ADR-0013 §8): small textures (at most 128
    /// texels a side) fully, larger ones start at a 64 texel level and are refined by
    /// [`Renderer::render`] as materials that use them are seen up close.
    ///
    /// # Errors
    /// [`RhiError::InvalidDescriptor`] when `rgba8` is not `width * height * 4` bytes, or any
    /// [`RhiError`] from texture creation or upload.
    pub fn create_texture<D: Device>(
        &mut self,
        device: &D,
        label: &str,
        rgba8: &[u8],
        width: u32,
        height: u32,
        srgb: bool,
    ) -> Result<TextureId, RhiError> {
        if width == 0
            || height == 0
            || rgba8.len() as u64 != u64::from(width) * u64::from(height) * 4
        {
            return Err(RhiError::InvalidDescriptor(format!(
                "texture {label}: expected {width}x{height}x4 bytes, got {}",
                rgba8.len()
            )));
        }
        let started = Instant::now();
        let mips = streaming::mip_chain(rgba8, width, height, srgb);
        let (floor, initial) = streaming::residency_bounds(width, height);
        let mut suffix = vec![0u64; mips.len() + 1];
        for l in (0..mips.len()).rev() {
            suffix[l] = suffix[l + 1] + mips[l].len() as u64;
        }
        let mut tex = Texture {
            texture: TextureHandle::NULL,
            index: 0,
            label: label.to_owned(),
            width,
            height,
            srgb,
            mips,
            base: initial,
            floor,
            changed: u64::from(self.frame_index),
            pixels: 0.0,
            suffix,
        };
        (tex.texture, tex.index) = make_resident(device, &tex, initial)?;
        let resident = tex.suffix[initial as usize];
        let (levels, index) = (tex.mips.len(), tex.index);
        let id = self.textures.insert(tex);
        tracing::debug!(
            label,
            ?id,
            width,
            height,
            srgb,
            index,
            levels,
            base = initial,
            resident_bytes = resident,
            chain_ms = started.elapsed().as_secs_f64() * 1e3,
            "texture created"
        );
        Ok(id)
    }

    /// Re-creates the GPU image of `id` with base level `base` and uploads its levels. The old
    /// image is destroyed (deferred by the device); the bindless slot changes.
    fn set_residency<D: Device>(
        &mut self,
        device: &D,
        id: TextureId,
        base: u32,
    ) -> Result<(), RhiError> {
        let Some(t) = self.textures.get_mut(id) else {
            return Ok(());
        };
        let (texture, index) = make_resident(device, t, base)?;
        let old = std::mem::replace(&mut t.texture, texture);
        let (from, label) = (t.base, t.label.clone());
        (t.index, t.base, t.changed) = (index, base, u64::from(self.frame_index));
        device.destroy_texture(old);
        tracing::debug!(label, from, to = base, index, "texture residency changed");
        Ok(())
    }

    /// Texture streaming for this frame (ADR-0013 §8): estimates the pixels each material covers
    /// from the instances in view, derives the base level each texture wants, then plans and
    /// applies the residency changes under the VRAM and upload budgets.
    ///
    /// The estimate takes an object's projected diameter `2 r f / d` (radius `r`, distance `d`,
    /// focal length `f` in pixels) as the number of texels a mapping across the whole object
    /// needs. shortcut: tiling and UV scale are ignored (a texture repeated 8 times over a
    /// surface is under-resolved by 3 levels); upgrade to GPU feedback when assets carry UV
    /// density.
    fn update_streaming<D: Device>(
        &mut self,
        device: &D,
        scene: &RenderScene,
        view_proj: Mat4,
        focal: f32,
    ) -> Result<(), RhiError> {
        let dense = self
            .materials
            .iter()
            .map(|(h, _)| h.index() + 1)
            .max()
            .unwrap_or(1) as usize;
        let frustum = Frustum::from_view_proj(view_proj);
        let cam = scene.camera.position;
        let (meshes, materials, default) = (&self.meshes, &self.materials, self.default_material);
        let one = |inst: &crate::MeshInstance, acc: &mut [f32]| {
            let Some(mesh) = meshes.get(inst.mesh) else {
                return;
            };
            let (c, r) = transform_sphere(&inst.transform, mesh.center, mesh.radius);
            if !frustum.intersects_sphere(c, r) {
                return;
            }
            let px = 2.0 * r * focal / c.distance(cam).max(r).max(1e-3);
            let m = if materials.get(inst.material).is_some() {
                inst.material
            } else {
                default
            };
            let slot = &mut acc[m.index() as usize];
            *slot = slot.max(px);
        };
        let pixels = if scene.meshes.len() >= prep::PAR_MIN {
            scene
                .meshes
                .par_chunks(4096)
                .map(|chunk| {
                    let mut acc = vec![0.0f32; dense];
                    chunk.iter().for_each(|i| one(i, &mut acc));
                    acc
                })
                .reduce(
                    || vec![0.0; dense],
                    |mut a, b| {
                        a.iter_mut().zip(&b).for_each(|(x, y)| *x = x.max(*y));
                        a
                    },
                )
        } else {
            let mut acc = vec![0.0f32; dense];
            scene.meshes.iter().for_each(|i| one(i, &mut acc));
            acc
        };
        // Pixels per texture: the largest of the materials that use it.
        let mut tex_pixels: Vec<f32> = Vec::new();
        for (h, m) in self.materials.iter() {
            let px = pixels[h.index() as usize];
            for t in m.textures {
                let i = t.index() as usize;
                if tex_pixels.len() <= i {
                    tex_pixels.resize(i + 1, 0.0);
                }
                tex_pixels[i] = tex_pixels[i].max(px);
            }
        }
        let frame = u64::from(self.frame_index);
        let ids: Vec<TextureId> = self.textures.iter().map(|(id, _)| id).collect();
        let (mut items, mut full_bytes) = (Vec::new(), 0);
        for &id in &ids {
            let Some(t) = self.textures.get_mut(id) else {
                continue;
            };
            let px = tex_pixels.get(id.index() as usize).copied().unwrap_or(0.0);
            t.pixels = px.max(t.pixels * PIXEL_DECAY);
            full_bytes += t.suffix[0];
            items.push(PlanItem {
                base: t.base,
                want: streaming::wanted_base(t.width.max(t.height), t.pixels, t.floor),
                floor: t.floor,
                stable: frame.saturating_sub(t.changed),
                suffix: t.suffix.clone(),
            });
        }
        let plan = streaming::plan(&items, self.texture_budget, self.upload_budget);
        let floor_bytes: u64 = items.iter().map(|it| it.suffix[it.floor as usize]).sum();
        if floor_bytes > self.texture_budget {
            tracing::warn!(
                floor_bytes,
                budget = self.texture_budget,
                "texture budget below the always-resident tail of every texture"
            );
        }
        for &(i, base) in &plan.changes {
            self.set_residency(device, ids[i], base)?;
        }
        self.evictions += u64::from(plan.evictions);
        self.texture_stats = TextureStats {
            textures: ids.len() as u32,
            resident_bytes: plan.resident_bytes,
            budget_bytes: self.texture_budget,
            full_bytes,
            pending_uploads: plan.pending,
            pending_bytes: plan.pending_bytes,
            uploads: plan.changes.len() as u32,
            uploaded_bytes: plan.upload_bytes,
            evictions: self.evictions,
        };
        Ok(())
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
            t.filter(|t| self.textures.get(*t).is_some())
                .unwrap_or(default)
        };
        let textures = [
            tex(desc.base_color_texture, self.white_srgb),
            tex(desc.metallic_roughness_texture, self.white_linear),
            tex(desc.normal_texture, self.flat_normal),
            tex(desc.occlusion_texture, self.white_linear),
            tex(desc.emissive_texture, self.white_srgb),
        ];
        let m = GpuMaterial {
            base_color: desc.base_color,
            emissive: [desc.emissive[0], desc.emissive[1], desc.emissive[2], 0.0],
            metallic: desc.metallic,
            roughness: desc.roughness,
            normal_scale: desc.normal_scale,
            occlusion_strength: desc.occlusion_strength,
            base_color_tex: 0,
            mr_tex: 0,
            normal_tex: 0,
            occlusion_tex: 0,
            emissive_tex: 0,
            alpha_cutoff: match desc.alpha_mode {
                AlphaMode::Mask(c) => c,
                _ => 0.0,
            },
            base_mip: 0,
            pad: 0,
        };
        let id = self.materials.insert(Material {
            gpu: m,
            textures,
            alpha_mode: desc.alpha_mode,
            double_sided: desc.double_sided,
        });
        tracing::debug!(label = desc.label, ?id, "material created");
        id
    }

    /// Destroys a material record.
    pub fn destroy_material(&mut self, id: MaterialId) {
        if self.materials.remove(id).is_some() {
            self.cache.invalidate();
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

    /// Selects the debug geometry the renderer draws itself (`ANVIL_DEBUG_DRAW`), see
    /// [`DebugDraw`].
    pub fn set_debug_draw(&mut self, mode: DebugDraw) {
        self.debug_draw = mode;
    }

    /// Current built-in debug geometry.
    pub fn debug_draw(&self) -> DebugDraw {
        self.debug_draw
    }

    /// The per-frame debug line buffer: lines added before [`Renderer::render`] are drawn by that
    /// frame's `debug.lines` pass (after the transparent pass, depth tested, no depth write) and
    /// then discarded.
    pub fn debug_lines(&mut self) -> &mut DebugLines {
        &mut self.debug_lines
    }

    /// Switches GPU-driven culling, LOD and indirect draws on or off (default on; the
    /// `ANVIL_GPU_CULLING=0` environment variable sets the initial state). Off is the M4 CPU
    /// path: per-frame frustum culling and one draw call per instance at level of detail 0.
    pub fn set_gpu_culling(&mut self, on: bool) {
        self.gpu_culling = on;
    }

    /// Whether GPU-driven culling is requested.
    pub fn gpu_culling(&self) -> bool {
        self.gpu_culling
    }

    /// Sets the LOD error threshold in pixels (`0` always renders level 0; default 0.5, the
    /// `ANVIL_LOD` environment variable sets the initial value).
    pub fn set_lod_pixels(&mut self, pixels: f32) {
        self.lod_pixels = pixels.max(0.0);
    }

    /// The LOD error threshold in pixels.
    pub fn lod_pixels(&self) -> f32 {
        self.lod_pixels
    }

    /// Counters of the last rendered frame.
    pub fn stats(&self) -> RendererStats {
        self.stats.clone()
    }

    /// Appends `data` to the frame's ring (16-byte aligned) and returns its device address.
    fn ring_push<D: Device>(
        &mut self,
        device: &D,
        slot: usize,
        data: &[u8],
    ) -> Result<u64, RhiError> {
        let base = self.rings[slot].1;
        Ok(base + self.ring_push_offset(device, slot, data)?)
    }

    /// [`Self::ring_push`] returning the offset into the ring buffer (for buffer copies).
    fn ring_push_offset<D: Device>(
        &mut self,
        device: &D,
        slot: usize,
        data: &[u8],
    ) -> Result<u64, RhiError> {
        let buffer = self.rings[slot].0;
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
        Ok(offset)
    }

    /// Brings the persistent instance buffer up to date with `self.cache`: grows it when the
    /// scene outgrew it, then uploads the changed instances. Small changes become copy runs out
    /// of the frame's ring (returned, recorded by the `scene.upload` pass); a fresh or oversized
    /// update goes through [`Device::upload_buffer`] instead.
    fn sync_instances<D: Device>(
        &mut self,
        device: &D,
        slot: usize,
    ) -> Result<Vec<UploadRun>, RhiError> {
        let n = self.cache.len() as u32;
        if n == 0 {
            return Ok(Vec::new());
        }
        let stride = size_of::<GpuInstance>() as u64;
        let mut whole = false;
        if n > self.scene.capacity {
            let capacity = n.next_power_of_two();
            let old = std::mem::replace(&mut self.scene, new_scene_buffers(device, capacity)?);
            self.graph.forget_buffer(old.instances);
            self.graph.forget_buffer(old.visible);
            device.destroy_buffer(old.instances);
            device.destroy_buffer(old.visible);
            tracing::debug!(capacity, "instance buffer grown");
            whole = true;
        }
        if self.cache.changed.is_empty() && !whole {
            return Ok(Vec::new());
        }
        let runs = if whole {
            vec![(0, n)]
        } else {
            merge_runs(&self.cache.changed, RUN_GAP)
        };
        let bytes: u64 = runs.iter().map(|r| u64::from(r.1 - r.0) * stride).sum();
        // Leave the ring half free for the frame's other data.
        if whole || bytes > (RING_BYTES - self.ring_used) / 2 {
            let (first, end) = (runs[0].0, runs[runs.len() - 1].1);
            device.upload_buffer(
                self.scene.instances,
                u64::from(first) * stride,
                bytemuck::cast_slice(&self.cache.gpu[first as usize..end as usize]),
            )?;
            return Ok(Vec::new());
        }
        // The mirror is moved out so the ring can be borrowed mutably meanwhile.
        let mirror = std::mem::take(&mut self.cache.gpu);
        let out = runs
            .iter()
            .map(|&(first, end)| {
                let data = bytemuck::cast_slice(&mirror[first as usize..end as usize]);
                Ok(UploadRun {
                    ring_offset: self.ring_push_offset(device, slot, data)?,
                    dst_offset: u64::from(first) * stride,
                    size: data.len() as u64,
                })
            })
            .collect();
        self.cache.gpu = mirror;
        out
    }

    /// Makes sure the depth pyramid exists for a `w` x `h` target.
    fn ensure_pyramid<D: Device>(&mut self, device: &D, w: u32, h: u32) -> Result<(), RhiError> {
        if self.pyramid.as_ref().is_some_and(|p| p.source == (w, h)) {
            return Ok(());
        }
        if let Some(old) = self.pyramid.take() {
            old.destroy(device, &mut self.graph);
        }
        self.pyramid = Some(Pyramid::new(device, &mut self.graph, w, h)?);
        Ok(())
    }

    /// Adds the geometry of the selected [`DebugDraw`] mode to the frame's debug lines.
    fn queue_debug_draw(
        &mut self,
        scene: &RenderScene,
        lights: &[Light],
        cascade_vp: Option<[Mat4; CASCADES]>,
        frustum: &Frustum,
    ) {
        /// Bounding spheres are drawn for instances this near the camera, up to this radius.
        const BOUNDS_DISTANCE: f32 = 60.0;
        const BOUNDS_MAX_RADIUS: f32 = 50.0;
        let lines = &mut self.debug_lines;
        match self.debug_draw {
            DebugDraw::None => {}
            DebugDraw::Bounds => {
                let cam = scene.camera.position;
                for inst in &scene.meshes {
                    let Some(mesh) = self.meshes.get(inst.mesh) else {
                        continue;
                    };
                    let (c, r) = transform_sphere(&inst.transform, mesh.center, mesh.radius);
                    if r <= BOUNDS_MAX_RADIUS
                        && c.distance(cam) - r < BOUNDS_DISTANCE
                        && frustum.intersects_sphere(c, r)
                    {
                        // 2 % larger so bounds that hug the mesh are not hidden by its surface.
                        lines.sphere(c, r * 1.02, Vec3::new(0.2, 1.0, 1.0));
                    }
                }
            }
            DebugDraw::Cascades => {
                const TINT: [Vec3; CASCADES] = [
                    Vec3::new(1.0, 0.25, 0.25),
                    Vec3::new(0.25, 1.0, 0.25),
                    Vec3::new(0.3, 0.4, 1.0),
                    Vec3::new(1.0, 1.0, 0.25),
                ];
                for (vp, tint) in cascade_vp.iter().flatten().zip(TINT) {
                    lines.frustum(*vp, tint);
                }
            }
            DebugDraw::Lights => {
                for l in lights {
                    match *l {
                        Light::Point {
                            position,
                            color,
                            range,
                            ..
                        } if range > 0.0 => {
                            lines.sphere(position, range, color.normalize_or_zero())
                        }
                        Light::Spot {
                            position,
                            direction,
                            color,
                            range,
                            outer_cone,
                            ..
                        } if range > 0.0 => {
                            let (dir, color) = (direction.normalize(), color.normalize_or_zero());
                            let side = dir.any_orthonormal_vector();
                            let (u, v) = (side, dir.cross(side));
                            let rim = position + dir * range;
                            let radius = range * outer_cone.tan();
                            lines.circle(rim, u * radius, v * radius, color);
                            for i in 0..8 {
                                let a = i as f32 / 8.0 * std::f32::consts::TAU;
                                lines.line(
                                    position,
                                    rim + (u * a.cos() + v * a.sin()) * radius,
                                    color,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
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
        let mut stages = StageTimes::default();
        if let Some(t) = self.graph.gpu_timings(device, frame.slot)? {
            self.gpu = t;
        }
        // GPU culling counters of the frame that used this slot before (retired by begin_frame).
        if let (rb, Some(counted)) = self.stats_readback[slot] {
            let mut bytes = vec![0u8; COUNTS_BYTES as usize];
            device.read_buffer(rb, 0, &mut bytes)?;
            self.cull_stats = CullStats::parse(counted, &bytes);
        }

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
                    mip_levels: 1,
                    array_layers: 1,
                    cube: false,
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
        // Point lights with a cube shadow map: the best candidates by distance and intensity.
        let point_pick = select_point_shadows(lights_in, cam, SHADOW_DISTANCE);
        let mut point_vp = [Mat4::IDENTITY; MAX_POINT_SHADOWS * 6];
        let mut spheres = Vec::new();
        for (light_index, l) in lights_in.iter().enumerate() {
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
            if let Light::Point {
                position, range, ..
            } = *l
                && let Some(slot) = point_pick.iter().position(|&p| p == light_index)
            {
                for f in 0..6 {
                    point_vp[slot * 6 + f] = point_face_view_proj(position, range, f);
                }
                g.spot[2] = slot as f32;
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

        // Every shadow view with a light: 4 cascades, 4 spots, then 6 faces per point light.
        let point_used = point_pick.len();
        let mut shadow_vp: Vec<Option<Mat4>> = vec![None; SHADOW_VIEWS];
        for c in 0..CASCADES {
            shadow_vp[c] = cascade_dir.map(|_| cascade_vp[c]);
        }
        for i in 0..spot_used {
            shadow_vp[CASCADES + i] = Some(spot_vp[i]);
        }
        for k in 0..point_used * 6 {
            shadow_vp[CASCADES + MAX_SPOT_SHADOWS + k] = Some(point_vp[k]);
        }
        let view_mask = shadow_vp
            .iter()
            .enumerate()
            .fold(0u32, |m, (k, v)| m | (u32::from(v.is_some()) << k));

        // Clustered assignment.
        let cluster_guard = StageGuard::new(
            tracing::debug_span!("renderer.clusters"),
            &mut stages.clusters,
        );
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
        drop(cluster_guard);

        // The persistent scene: find the instances that changed since last frame and re-pack
        // only those (ADR-0013 §6). Packing runs on the worker pool (ADR-0013 §3).
        let pack_guard = StageGuard::new(tracing::debug_span!("renderer.pack"), &mut stages.pack);
        let (meshes, materials, default_material) =
            (&self.meshes, &self.materials, self.default_material);
        self.cache.update(&scene.meshes, |inst| {
            let Some(mesh) = meshes.get(inst.mesh) else {
                tracing::warn!(mesh = ?inst.mesh, "scene references a destroyed mesh, skipped");
                return None;
            };
            let material_id = if materials.get(inst.material).is_some() {
                inst.material
            } else {
                default_material
            };
            let material = materials.get(material_id)?;
            let double = usize::from(material.double_sided);
            let (variant, blend) = match material.alpha_mode {
                AlphaMode::Opaque => (double, false),
                AlphaMode::Mask(_) => (2 + double, false),
                AlphaMode::Blend => (double, true),
            };
            Some(Resolved {
                mesh: inst.mesh.index(),
                first_index: mesh.record.first_index[0],
                index_count: mesh.record.index_count[0],
                vertex_offset: mesh.record.vertex_offset,
                center: mesh.center,
                radius: mesh.radius,
                material: material_id.index(),
                variant,
                blend,
            })
        });
        let instances_uploaded = self.cache.changed.len() as u32;
        drop(pack_guard);
        let upload_guard =
            StageGuard::new(tracing::debug_span!("renderer.upload"), &mut stages.upload);
        let upload_runs = self.sync_instances(device, slot)?;
        drop(upload_guard);
        let streaming_guard = StageGuard::new(
            tracing::debug_span!("renderer.streaming"),
            &mut stages.streaming,
        );
        self.update_streaming(device, scene, view_proj_unjit, h as f32 / (2.0 * tan_y))?;
        drop(streaming_guard);

        // GPU-driven culling needs at least one opaque instance; otherwise the (empty) CPU path.
        let gpu_cull = self.gpu_culling && self.cache.variant_counts.iter().sum::<u32>() > 0;
        let cache = &self.cache;
        let cull_guard = StageGuard::new(tracing::debug_span!("renderer.cull"), &mut stages.cull);
        let cam_frustum = Frustum::from_view_proj(view_proj_unjit);
        let (mut opaque, mut blended) = (Vec::new(), Vec::new());
        let mut culled = 0u32;
        let mut triangles = 0u64;
        if gpu_cull {
            // Only blended objects (they need sorting) are culled on the CPU.
            for it in prep::cull_list(&cache.blended, &cam_frustum, &view) {
                triangles += u64::from(it.draw.index_count / 3);
                blended.push(it.draw);
            }
        } else {
            let visible = prep::cull(&cache.items, &cam_frustum, &view);
            culled = (cache.items.iter().flatten().count() - visible.len()) as u32;
            for it in visible {
                triangles += u64::from(it.draw.index_count / 3);
                if it.blend { &mut blended } else { &mut opaque }.push(it.draw);
            }
        }
        drop(cull_guard);
        let sort_guard = StageGuard::new(tracing::debug_span!("renderer.sort"), &mut stages.sort);
        prep::sort_camera(&mut opaque, &mut blended);
        drop(sort_guard);
        let shadow_guard = StageGuard::new(
            tracing::debug_span!("renderer.shadow_lists"),
            &mut stages.shadow_lists,
        );
        // One list per view, the views in parallel once there are enough instances. With GPU
        // culling the shadow views are culled by `cull_shadow.comp` instead.
        let items = &cache.items;
        let view_lists =
            |n: usize, used: &(dyn Fn(usize) -> Option<Mat4> + Sync)| -> Vec<Vec<Draw>> {
                if gpu_cull {
                    return vec![Vec::new(); n];
                }
                let one = |i| {
                    used(i).map_or_else(Vec::new, |vp| {
                        prep::shadow_draws(items, &Frustum::from_view_proj(vp))
                    })
                };
                if items.len() >= prep::PAR_MIN {
                    (0..n).into_par_iter().map(one).collect()
                } else {
                    (0..n).map(one).collect()
                }
            };
        let shadow_draws = view_lists(SHADOW_VIEWS, &|k| shadow_vp[k]);
        let shadow_draw_count = shadow_draws.iter().map(Vec::len).sum::<usize>();
        drop(shadow_guard);

        let dense = self
            .materials
            .iter()
            .map(|(h, _)| h.index() + 1)
            .max()
            .unwrap_or(1) as usize;
        let mut materials = vec![bytemuck::Zeroable::zeroed(); dense];
        for (h, m) in self.materials.iter() {
            let slot = |t: TextureId| self.textures.get(t).map_or(0, |t| t.index);
            let mut g: GpuMaterial = m.gpu;
            g.base_color_tex = slot(m.textures[0]);
            g.mr_tex = slot(m.textures[1]);
            g.normal_tex = slot(m.textures[2]);
            g.occlusion_tex = slot(m.textures[3]);
            g.emissive_tex = slot(m.textures[4]);
            g.base_mip = self.textures.get(m.textures[0]).map_or(0, |t| t.base);
            materials[h.index() as usize] = g;
        }

        let upload_guard =
            StageGuard::new(tracing::debug_span!("renderer.upload"), &mut stages.upload);
        let instances_addr = self.scene.address;
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
            point_vp,
            camera_pos: [cam.x, cam.y, cam.z, 1.0],
            cascade_splits: splits,
            cascade_texel,
            cascade_tex: std::array::from_fn(|i| self.cascades[i].index),
            spot_tex: std::array::from_fn(|i| self.spots[i].index),
            point_tex: std::array::from_fn(|i| self.points[i].index),
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
        // Debug geometry: the built-in modes and the application's lines go to the ring.
        self.queue_debug_draw(
            scene,
            lights_in,
            (cascade_dir.is_some()).then_some(cascade_vp),
            &cam_frustum,
        );
        let debug_verts = self.debug_lines.take();
        let debug_vertices = debug_verts.len() as u32;
        let debug_address = if debug_verts.is_empty() {
            0
        } else {
            self.ring_push(device, slot, bytemuck::cast_slice(&debug_verts))?
        };
        // GPU culling: parameters of the compute passes (planes of the camera and every shadow
        // view, the unjittered camera, LOD threshold, depth pyramid) and the command layout.
        let layout = CullLayout::new(self.cache.variant_counts, view_mask);
        let cull_params = if gpu_cull {
            self.ensure_pyramid(device, w, h)?;
            let pyr = self.pyramid.as_ref().expect("pyramid ensured above");
            let mut planes = [[[0.0f32; 4]; 6]; 1 + SHADOW_VIEWS];
            planes[0] = cam_frustum.planes();
            for (k, vp) in shadow_vp.iter().enumerate() {
                if let Some(vp) = vp {
                    planes[1 + k] = Frustum::from_view_proj(*vp).planes();
                }
            }
            let params = GpuCullParams {
                planes,
                view,
                proj: [
                    proj_unjit.x_axis.x,
                    proj_unjit.y_axis.y,
                    scene.camera.near,
                    self.lod_pixels,
                ],
                cam: [cam.x, cam.y, cam.z, h as f32 / (2.0 * tan_y)],
                pyramid: [
                    pyr.slot,
                    pyr.sizes[0].0,
                    pyr.sizes[0].1,
                    pyr.sizes.len() as u32,
                ],
                instances: split(self.scene.address),
                meshes: split(self.mesh_table_address),
                visible: split(self.scene.visible_address),
                instance_count: self.cache.len() as u32,
                view_mask,
                pad: [0; 2],
                region: layout.regions(),
                pad2: [0; 2],
            };
            self.ring_push(device, slot, bytemuck::bytes_of(&params))?
        } else {
            0
        };
        drop(upload_guard);

        let build_guard = StageGuard::new(
            tracing::debug_span!("renderer.graph_build"),
            &mut stages.graph_build,
        );
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
        // Cube faces of the lights that cast: each one-layer view is its own graph texture; the
        // shaders sample the whole cube through its own slot (like the depth pyramid).
        let point_ids: Vec<TexId> = (0..point_used * 6)
            .map(|k| {
                b.import(
                    format!("{}.face{}", POINT_NAMES[k / 6], k % 6),
                    self.points[k / 6].faces[k % 6],
                    (POINT_SHADOW_SIZE, POINT_SHADOW_SIZE),
                    None,
                )
            })
            .collect();
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
        // The persistent scene buffers, and (GPU culling) the indirect command lists.
        let inst_id = b.import_buffer("scene.instances", self.scene.instances, None);
        if !upload_runs.is_empty() {
            let (ring, runs) = (self.rings[slot].0, upload_runs);
            let dst = self.scene.instances;
            b.pass(
                Pass::new("scene.upload")
                    .label_color([0.6, 0.6, 0.3, 1.0])
                    .write_buffer(inst_id, BufferState::TransferDst),
                move |ctx| {
                    for r in &runs {
                        ctx.cmd.copy_buffer_to_buffer(
                            ring,
                            r.ring_offset,
                            dst,
                            r.dst_offset,
                            r.size,
                        );
                    }
                    Ok(())
                },
            );
        }
        let gpu_ids = gpu_cull.then(|| {
            let visible = b.import_buffer("scene.visible", self.scene.visible, None);
            let cmds = b.transient_buffer(
                "cull.cmds",
                TransientBufferDesc {
                    size: layout.buffer_bytes(),
                    usage: BufferUsage::STORAGE | BufferUsage::INDIRECT,
                },
            );
            let counts = b.transient_buffer(
                "cull.counts",
                TransientBufferDesc {
                    size: COUNTS_BYTES,
                    usage: BufferUsage::STORAGE
                        | BufferUsage::INDIRECT
                        | BufferUsage::TRANSFER_DST
                        | BufferUsage::TRANSFER_SRC,
                },
            );
            let pyr = self.pyramid.as_ref().expect("pyramid ensured above");
            let mips: Vec<TexId> = pyr
                .mips
                .iter()
                .zip(&pyr.sizes)
                .enumerate()
                .map(|(l, (&t, &size))| b.import(format!("hiz.mip{l}"), t, size, None))
                .collect();
            let c = CullBuffers {
                instances: inst_id,
                visible,
                cmds,
                counts,
                params: cull_params,
                instance_count: self.cache.len() as u32,
            };
            (c, mips)
        });
        if let Some((c, _)) = &gpu_ids {
            reset_pass(&mut b, c.counts);
            shadow_cull_pass(&mut b, self.cull_pipes.shadow, *c);
            camera_cull_pass(&mut b, self.cull_pipes.camera, *c, 0, &[]);
        }
        let geo = GeometryBuilder {
            addresses,
            arena: ArenaRef {
                vertex_address: self.arena.vertex_address,
                indices: self.arena.indices,
            },
            instances: inst_id,
            gpu: gpu_ids.as_ref().map(|(c, _)| (c.cmds, c.counts, layout)),
        };
        let geometry = |pipes, draws, views| geo.pick(pipes, draws, views);
        for c in 0..CASCADES {
            shadow_pass(
                &mut b,
                ShadowInputs {
                    name: CASCADE_NAMES[c].into(),
                    texture: cascade_ids[c],
                    view_index: c as u32,
                    geometry: geometry(self.shadow_pipes, &shadow_draws[c], shadow_cull_view(c)),
                },
            );
        }
        for i in 0..MAX_SPOT_SHADOWS {
            shadow_pass(
                &mut b,
                ShadowInputs {
                    name: SPOT_NAMES[i].into(),
                    texture: spot_ids[i],
                    view_index: (CASCADES + i) as u32,
                    geometry: geometry(
                        self.shadow_pipes,
                        &shadow_draws[CASCADES + i],
                        shadow_cull_view(CASCADES + i),
                    ),
                },
            );
        }
        for (k, &face) in point_ids.iter().enumerate() {
            let view = CASCADES + MAX_SPOT_SHADOWS + k;
            shadow_pass(
                &mut b,
                ShadowInputs {
                    name: format!("shadow.point{}.face{}", k / 6, k % 6).into(),
                    texture: face,
                    view_index: view as u32,
                    geometry: geometry(
                        self.shadow_cube_pipes,
                        &shadow_draws[view],
                        shadow_cull_view(view),
                    ),
                },
            );
        }
        // GPU culling: phase 1 draws last frame's visible set, the pyramid is built from that
        // depth, phase 2 draws what the pyramid does not hide. The CPU path is one prepass.
        prepass(
            &mut b,
            PrepassInputs {
                name: "prepass",
                phase2: false,
                depth,
                normal,
                velocity,
                geometry: geometry(self.prepass, &opaque, PHASE1_VIEWS),
            },
        );
        if let Some((c, mips)) = &gpu_ids {
            let pyr = self.pyramid.as_ref().expect("pyramid ensured above");
            hiz_passes(&mut b, self.cull_pipes.hiz, depth, (w, h), mips, &pyr.sizes);
            camera_cull_pass(&mut b, self.cull_pipes.camera, *c, 1, mips);
            prepass(
                &mut b,
                PrepassInputs {
                    name: "prepass.p2",
                    phase2: true,
                    depth,
                    normal,
                    velocity,
                    geometry: geometry(self.prepass, &opaque, PHASE2_VIEWS),
                },
            );
        }
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
        reads.extend(point_ids.iter().copied());
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
                    geometry: geometry(self.forward, &opaque, CAMERA_VIEWS),
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
                        geometry: geometry(self.gbuffer, &opaque, CAMERA_VIEWS),
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
                    geometry: geo.cpu(self.transparent, &blended),
                },
            );
        }
        if debug_vertices > 0 {
            debug_pass(
                &mut b,
                DebugInputs {
                    pipeline: self.debug_pipeline,
                    hdr,
                    depth,
                    frame_address: addresses[0],
                    vertex_address: debug_address,
                    vertex_count: debug_vertices,
                },
            );
        }
        if let Some((c, _)) = &gpu_ids {
            stats_pass(&mut b, c.counts, self.stats_readback[slot].0);
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
        drop(build_guard);
        let compile_guard = StageGuard::new(
            tracing::debug_span!("renderer.compile"),
            &mut stages.compile,
        );
        let compiled = self.graph.compile(b)?;
        drop(compile_guard);
        let compile_time = compiled.stats().compile_time;
        let execute_guard = StageGuard::new(
            tracing::debug_span!("renderer.execute"),
            &mut stages.execute,
        );
        let mut graph_stats = self.graph.execute(compiled, device, cmd, frame)?;
        drop(execute_guard);
        graph_stats.compile_time = compile_time;
        if rebake {
            self.ibl.mark_baked(&scene.environment);
        }
        self.stats_readback[slot].1 = gpu_cull.then_some(frame.frame);
        self.prev_view_proj = view_proj_unjit;
        self.frame_index = self.frame_index.wrapping_add(1);
        if taa {
            self.taa_write = 1 - self.taa_write;
        }
        self.taa_valid = taa;

        // With GPU culling the camera counters are the delayed readback (the CPU only knows the
        // blended draws); the CPU path counts exactly.
        let cs = if gpu_cull {
            self.cull_stats
        } else {
            CullStats::default()
        };
        self.stats = RendererStats {
            draws: (opaque.len() + blended.len()) as u32 + cs.camera_draws,
            triangles: triangles + u64::from(cs.triangles),
            gpu_culling: gpu_cull,
            cull: cs,
            instances_uploaded,
            instances: self.cache.len() as u32,
            lights: lights.len() as u32,
            point_shadows: point_used as u32,
            textures: self.texture_stats,
            ring_bytes_used: self.ring_used,
            graph: graph_stats,
            culled: culled + cs.frustum_culled,
            shadow_draws: shadow_draw_count as u32 + cs.shadow_draws,
            stages,
            gpu: self.gpu.clone(),
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
        self.arena.destroy(device);
        device.destroy_buffer(self.mesh_table);
        device.destroy_buffer(self.scene.instances);
        device.destroy_buffer(self.scene.visible);
        for (b, _) in self.stats_readback {
            device.destroy_buffer(b);
        }
        for (_, t) in self.textures.iter() {
            device.destroy_texture(t.texture);
        }
        for (b, _) in self.rings {
            device.destroy_buffer(b);
        }
        let mut graph = self.graph;
        if let Some(p) = self.pyramid {
            p.destroy(device, &mut graph);
        }
        graph.destroy(device);
        device.destroy_sampler(self.linear_repeat);
        device.destroy_sampler(self.linear_clamp);
        self.ibl.destroy(device);
        for m in self.cascades.iter().chain(&self.spots) {
            device.destroy_texture(m.texture);
        }
        for p in &self.points {
            for f in p.faces {
                device.destroy_texture(f);
            }
            device.destroy_texture(p.cube);
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
            .chain(self.shadow_cube_pipes)
            .chain(self.prepass)
            .chain(self.forward)
            .chain(self.gbuffer)
            .chain(self.transparent)
            .chain([
                self.cull_pipes.camera,
                self.cull_pipes.shadow,
                self.cull_pipes.hiz,
                self.lighting,
                self.tonemap,
                self.debug_pipeline,
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
