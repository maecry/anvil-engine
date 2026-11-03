use serde::{Deserialize, Serialize};

use crate::{AssetId, AssetKind};

/// Newest `.meta` version this build reads and writes.
pub const META_VERSION: u32 = 1;

/// A sub-asset produced by importing a source file (a glTF mesh, material or image).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubAsset {
    /// Deterministic id, derived from the parent id and `name`.
    pub id: AssetId,
    /// What it is.
    pub kind: AssetKind,
    /// Derivation name, e.g. `mesh/0/0`.
    pub name: String,
}

/// The `<file>.meta` sidecar (RON). Committed to version control.
// shortcut: no per-asset importer settings and no source hash here (the hash lives in the cache header so metas stay stable in VCS), add settings when an importer needs one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// Format version.
    pub version: u32,
    /// Identity of the source file.
    pub id: AssetId,
    /// Importer name (`gltf`, `image`, `ktx2`, `material`, `scene`).
    pub importer: String,
    /// Sub-assets found by the last import (empty for single-asset files).
    #[serde(default)]
    pub sub_assets: Vec<SubAsset>,
}
