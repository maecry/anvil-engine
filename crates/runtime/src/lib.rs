//! Composition root: window, Vulkan device and swapchain, [`renderer::Renderer`], the ECS
//! world and the asset server, run in a fixed frame order.
//!
//! Specification: `docs/specs/layers/LAYER-010-runtime-and-editor.md`, ADR-0014 §5 and §7.
//!
//! - [`Runtime::run`] drives a game: [`Application::start`] once, then per frame input,
//!   `assets.update`, [`Application::update`], transform systems, extract into a
//!   [`renderer::RenderScene`], render, present.
//! - [`RuntimeCore`] is everything except the window and the loop (world, assets, the
//!   `AssetId -> MeshId/TextureId/MaterialId` bridge and the extract); the editor embeds it with
//!   its own loop and offscreen target.
//! - `ANVIL_WINDOW=0` runs headless against an offscreen target (see [`RunOptions::from_env`]).

mod core;
mod error;
mod project;
mod runtime;

pub use core::RuntimeCore;
pub use error::RuntimeError;
pub use project::{PROJECT_FILE, PROJECT_VERSION, Project};
pub use runtime::{Application, Context, Input, RunOptions, RunReport, Runtime, Time};
