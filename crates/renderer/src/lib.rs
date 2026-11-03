//! Forward PBR renderer over the Anvil RHI, built on the render graph.
//!
//! Specification: `docs/specs/layers/LAYER-005-renderer.md`, `docs/specs/modules/MODULE-010-render-world.md`,
//! ADR-0011. The app extracts a [`RenderScene`] every frame and calls [`Renderer::render`]; the
//! renderer owns its pipelines, targets and per-frame upload rings. Generic over
//! [`rhi::Device`] (ADR-0005), no Vulkan types are exposed.
//!
//! Since M5b (ADR-0013 §6) meshes live in one shared arena, instances in a persistent GPU buffer
//! of which only the changed ones are re-uploaded, and opaque geometry is culled (frustum and
//! two-phase Hi-Z occlusion), LOD-selected and drawn by the GPU through indirect commands;
//! `ANVIL_GPU_CULLING=0` selects the CPU path.

mod arena;
pub mod bench;
mod cluster;
mod cull;
mod debug;
mod gpu;
mod gpu_cull;
mod ibl;
pub mod mesh;
mod passes;
mod post;
mod prep;
mod renderer;
mod scene;
mod shadow;
mod streaming;

pub use debug::DebugLines;
pub use gpu_cull::CullStats;
pub use mesh::{MAX_LODS, MeshData, MeshLod, Vertex, generate_lods};
pub use render_graph::GpuTimings;
pub use renderer::{
    AlphaMode, DebugDraw, DebugView, Material, MaterialDesc, MaterialId, Mesh, MeshId,
    PostSettings, RenderError, RenderPath, Renderer, RendererStats, StageTimes, Texture, TextureId,
    TextureStats,
};
pub use scene::{
    Camera, Environment, Light, MAX_LIGHTS, MAX_POINT_SHADOWS, MAX_SPOT_SHADOWS, MeshInstance,
    RenderScene,
};
