//! egui on the Anvil RHI plus reusable diagnostics panels (ADR-0014 §8, LAYER-010).
//!
//! [`EguiInput`] feeds winit events to egui, [`EguiRenderer`] draws egui's output through the
//! RHI (no Vulkan types), [`panels`] are diagnostics widgets over plain data, and [`Overlay`]
//! bundles them for applications.

mod gpu;
mod input;
mod overlay;
pub mod panels;

pub use egui;
pub use gpu::{EguiRenderer, EguiTarget};
pub use input::EguiInput;
pub use overlay::Overlay;
