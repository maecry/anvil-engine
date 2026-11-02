//! Foundational types shared by Anvil Engine crates.
//!
//! Specification: `docs/specs/layers/LAYER-001-foundation.md`.

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
