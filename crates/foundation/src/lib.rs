//! Foundational types shared by Anvil Engine crates.
//!
//! Specification: `docs/specs/layers/LAYER-001-foundation.md`.

/// Math types (`glam`) and the engine-wide conventions (ADR-0011 §1).
///
/// - World space is right-handed, Y-up, forward is -Z (the glTF convention).
/// - Depth is reverse-Z: near = 1, far = 0, `D32Sfloat` cleared to 0 and tested with
///   `GREATER_OR_EQUAL`, using an infinite-far projection (`camera::rh::proj::vulkan::perspective_infinite_reverse`).
/// - The Vulkan Y flip is folded into the projection matrix (`proj.y_axis.y` is negative, which glam's `camera::rh::proj::vulkan` builders do); viewports
///   keep a positive height.
/// - Matrices are column-major (glam), vectors are column vectors: `clip = proj * view * model * v`.
pub use glam as math;

mod error;
mod handle;
#[cfg(feature = "log-init")]
mod logging;
mod pool;
mod time;

pub use error::CoreError;
pub use handle::Handle;
#[cfg(feature = "log-init")]
pub use logging::init_logging;
pub use pool::Pool;
pub use time::{Clock, FrameCounter};
/// Re-exported so all crates share one `tracing` version.
pub use tracing;
