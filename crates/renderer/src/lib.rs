//! Forward PBR renderer over the Anvil RHI, built on the render graph.
//!
//! Specification: `docs/specs/layers/LAYER-005-renderer.md`, `docs/specs/modules/MODULE-010-render-world.md`,
//! ADR-0011. The app extracts a [`RenderScene`] every frame and calls [`Renderer::render`]; the
//! renderer owns its pipelines, targets and per-frame upload rings. Generic over
//! [`rhi::Device`] (ADR-0005), no Vulkan types are exposed.

mod cluster;
mod cull;
mod gpu;
mod ibl;
pub mod mesh;
mod passes;
mod post;
mod renderer;
mod scene;
mod shadow;

pub use mesh::{MeshData, Vertex};
pub use renderer::{
    AlphaMode, DebugView, Material, MaterialDesc, MaterialId, Mesh, MeshId, PostSettings,
    RenderError, RenderPath, Renderer, RendererStats, Texture, TextureId,
};
pub use scene::{
    Camera, Environment, Light, MAX_LIGHTS, MAX_SPOT_SHADOWS, MeshInstance, RenderScene,
};
