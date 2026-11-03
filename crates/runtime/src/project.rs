use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::RuntimeError;

/// File name of a project's manifest, in the project root.
pub const PROJECT_FILE: &str = "project.anvil";
/// Newest `project.anvil` version this build reads.
pub const PROJECT_VERSION: u32 = 1;

/// `project.anvil` (RON): the project's name and the scene the player starts with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Format version.
    pub version: u32,
    /// Display name.
    pub name: String,
    /// Startup scene, relative to the project directory (e.g. `assets/scenes/main.scene`).
    pub startup_scene: String,
}

impl Project {
    /// A new manifest.
    pub fn new(name: &str, startup_scene: &str) -> Self {
        Self {
            version: PROJECT_VERSION,
            name: name.to_owned(),
            startup_scene: startup_scene.to_owned(),
        }
    }

    /// Reads `<dir>/project.anvil`.
    ///
    /// # Errors
    /// [`RuntimeError::Project`] when missing, malformed or newer than supported.
    pub fn load(dir: &Path) -> Result<Self, RuntimeError> {
        let path = dir.join(PROJECT_FILE);
        let err = |m: String| RuntimeError::Project(format!("{}: {m}", path.display()));
        let text = std::fs::read_to_string(&path).map_err(|e| err(e.to_string()))?;
        let p: Project = ron::from_str(&text).map_err(|e| err(e.to_string()))?;
        if p.version > PROJECT_VERSION {
            return Err(err(format!(
                "version {} is newer than the supported {PROJECT_VERSION}",
                p.version
            )));
        }
        Ok(p)
    }

    /// Writes `<dir>/project.anvil`.
    ///
    /// # Errors
    /// [`RuntimeError::Io`] on write failure.
    pub fn save(&self, dir: &Path) -> Result<(), RuntimeError> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| RuntimeError::Project(e.to_string()))?;
        std::fs::write(dir.join(PROJECT_FILE), text).map_err(|e| RuntimeError::Io(e.to_string()))
    }
}
