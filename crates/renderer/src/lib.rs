//! Forward PBR renderer over the Anvil RHI.
//!
//! Specification: `docs/specs/layers/LAYER-005-renderer.md`, `docs/specs/modules/MODULE-010-render-world.md`,
//! ADR-0011. The app extracts a [`RenderScene`] every frame and calls [`Renderer::render`]; the
//! renderer owns its pipelines, targets and per-frame upload rings. Generic over
//! [`rhi::Device`] (ADR-0005), no Vulkan types are exposed.

mod gpu;
pub mod mesh;
mod passes;
mod renderer;
mod scene;
mod shadow;

pub use mesh::{MeshData, Vertex};
pub use renderer::{
    Material, MaterialDesc, MaterialId, Mesh, MeshId, Renderer, RendererStats, Texture, TextureId,
};
pub use scene::{Camera, Light, MAX_LIGHTS, MeshInstance, RenderScene};
