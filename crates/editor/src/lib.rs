//! The Anvil editor: viewport, hierarchy, inspector, asset browser, material editor and the
//! diagnostics panels, in the same process as the engine (ADR-0014 §9, LAYER-010).
//!
//! - [`Editor`] holds the state (a [`runtime::RuntimeCore`] plus selection, camera, material
//!   draft, play snapshot). Every change to the project goes through [`Editor::apply`] as an
//!   [`EditorCommand`]; the UI only queues commands.
//! - [`run`] opens the window (or runs headless with `ANVIL_WINDOW=0`), drives the frame loop
//!   and, with `ANVIL_EDITOR_SCRIPT=file.ron`, executes a command script (see [`parse_script`]).
//! - The editor only uses the public APIs of `runtime`, `scene`, `assets`, `renderer` and
//!   `debug` (FR-TOOL-006).

mod app;
mod command;
mod editor;
mod error;
pub mod pick;
mod template;
mod ui;

pub use app::{EditorOptions, EditorReport, run};
pub use command::{AssetRef, ComponentKind, EditorCommand, EntityRef, parse_script};
pub use editor::{Editor, FlyCamera, Outcome, VIEWPORT_FORMAT};
pub use error::EditorError;
pub use template::{DEFAULT_SCENE, create_project, default_scene};
