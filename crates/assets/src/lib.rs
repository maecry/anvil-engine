//! Asset database, importers, imported-artifact cache, async loading and hot reload.
//!
//! Specification: `docs/specs/layers/LAYER-009-asset-system.md`, ADR-0014 §4-6. GPU-free: assets
//! are plain CPU data; the runtime pulls them and uploads (ADR-0014 §5).
//!
//! - Every source file under `<project>/assets/` gets a `<file>.meta` sidecar holding its
//!   [`AssetId`]; sub-assets (glTF meshes, materials, images) get deterministic derived ids.
//! - Imports run on rayon workers and return over a channel that [`AssetServer::update`] drains
//!   on the calling thread. Results are cached in `<project>/.anvil/imported/<id>.bin`.
//! - Nothing here panics on bad input: errors become [`LoadState::Failed`].

mod cache;
mod error;
#[cfg(feature = "fixtures")]
pub mod fixtures;
mod id;
mod import;
mod meta;
mod server;
mod types;

pub use error::AssetError;
pub use id::{AssetId, u128_str};
pub use meta::{META_VERSION, Meta, SubAsset};
pub use server::{AssetEvent, AssetInfo, AssetServer, AssetStats, LoadState};
pub use types::{
    AlphaModeAsset, Asset, AssetKind, IMPORTER_VERSION, MATERIAL_VERSION, MaterialAsset, MeshAsset,
    PrefabAsset, PrefabMesh, PrefabNode, SCENE_VERSION, SceneAsset, TextureAsset,
};
