use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::AssetId;

/// Version of the importers and the cache payload layout; bump to invalidate every cache entry.
pub const IMPORTER_VERSION: u32 = 1;
/// Newest `.material` version this build reads.
pub const MATERIAL_VERSION: u32 = 1;
/// Newest `.scene` version this build reads.
pub const SCENE_VERSION: u32 = 1;

/// What an asset is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetKind {
    /// Triangle mesh.
    Mesh,
    /// 2D RGBA8 image.
    Texture,
    /// PBR material.
    Material,
    /// Node hierarchy (from a glTF file).
    Prefab,
    /// Serialized scene (parsed by the `scene` crate).
    Scene,
}

impl AssetKind {
    /// Kind and importer name of a source file by extension, `None` for non-assets.
    pub fn of_path(path: &Path) -> Option<(AssetKind, &'static str)> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "gltf" | "glb" => (AssetKind::Prefab, "gltf"),
            "png" | "jpg" | "jpeg" | "exr" => (AssetKind::Texture, "image"),
            "ktx2" => (AssetKind::Texture, "ktx2"),
            "material" => (AssetKind::Material, "material"),
            "scene" => (AssetKind::Scene, "scene"),
            _ => return None,
        })
    }
}

/// CPU mesh in the renderer's vertex attributes (the runtime interleaves them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshAsset {
    /// Object-space positions.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals.
    pub normals: Vec<[f32; 3]>,
    /// Texture coordinates (origin top-left).
    pub uvs: Vec<[f32; 2]>,
    /// Tangent xyz plus bitangent sign.
    pub tangents: Vec<[f32; 4]>,
    /// Triangle list.
    pub indices: Vec<u32>,
}

/// Tightly packed RGBA8 image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextureAsset {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba8: Vec<u8>,
}

/// Alpha handling of a material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum AlphaModeAsset {
    /// Alpha ignored.
    #[default]
    Opaque,
    /// Alpha tested against the cutoff.
    Mask(f32),
    /// Alpha blended.
    Blend,
}

/// glTF metallic-roughness material; also the `.material` file (RON). Texture slots are asset
/// ids; `None` uses the renderer's default for the slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialAsset {
    /// Format version.
    pub version: u32,
    /// Base color factor (linear RGBA).
    pub base_color: [f32; 4],
    /// Base color texture (sRGB).
    pub base_color_texture: Option<AssetId>,
    /// Metallic factor.
    pub metallic: f32,
    /// Roughness factor.
    pub roughness: f32,
    /// Metallic (B) / roughness (G) texture.
    pub metallic_roughness_texture: Option<AssetId>,
    /// Tangent-space normal texture.
    pub normal_texture: Option<AssetId>,
    /// Normal map scale.
    pub normal_scale: f32,
    /// Occlusion texture (R).
    pub occlusion_texture: Option<AssetId>,
    /// Occlusion strength.
    pub occlusion_strength: f32,
    /// Emissive factor (linear RGB).
    pub emissive: [f32; 3],
    /// Emissive texture (sRGB).
    pub emissive_texture: Option<AssetId>,
    /// Alpha mode.
    pub alpha_mode: AlphaModeAsset,
    /// Render both faces.
    pub double_sided: bool,
}

impl Default for MaterialAsset {
    fn default() -> Self {
        Self {
            version: MATERIAL_VERSION,
            base_color: [1.0; 4],
            base_color_texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha_mode: AlphaModeAsset::Opaque,
            double_sided: false,
        }
    }
}

impl MaterialAsset {
    /// The texture ids the material uses.
    pub fn textures(&self) -> Vec<AssetId> {
        [
            self.base_color_texture,
            self.metallic_roughness_texture,
            self.normal_texture,
            self.occlusion_texture,
            self.emissive_texture,
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// A mesh + material pair of a prefab node (`AssetId::NIL` material = renderer default).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrefabMesh {
    /// Mesh sub-asset.
    pub mesh: AssetId,
    /// Material sub-asset or NIL.
    pub material: AssetId,
}

/// One node of a prefab (a glTF node).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrefabNode {
    /// Node name.
    pub name: String,
    /// Index of the parent node in [`PrefabAsset::nodes`] (parents come first), `None` for roots.
    pub parent: Option<u32>,
    /// Local translation.
    pub translation: [f32; 3],
    /// Local rotation quaternion (xyzw).
    pub rotation: [f32; 4],
    /// Local scale.
    pub scale: [f32; 3],
    /// Mesh drawn by this node, if any.
    pub mesh: Option<PrefabMesh>,
}

/// Node hierarchy of a glTF file, instantiated into a world by the `scene` crate.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrefabAsset {
    /// Nodes, parents before children.
    pub nodes: Vec<PrefabNode>,
}

impl PrefabAsset {
    /// Mesh and material ids used.
    pub fn deps(&self) -> Vec<AssetId> {
        let mut v: Vec<AssetId> = self
            .nodes
            .iter()
            .filter_map(|n| n.mesh)
            .flat_map(|m| [m.mesh, m.material])
            .filter(|i| !i.is_nil())
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// A `.scene` file: the text (parsed by the `scene` crate) plus the assets it references.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneAsset {
    /// The RON source.
    pub text: String,
    /// Mesh and material ids referenced by entities.
    pub deps: Vec<AssetId>,
}

/// A loaded asset; cloning is cheap.
#[derive(Clone, Debug)]
pub enum Asset {
    /// Mesh.
    Mesh(Arc<MeshAsset>),
    /// Texture.
    Texture(Arc<TextureAsset>),
    /// Material.
    Material(Arc<MaterialAsset>),
    /// Prefab.
    Prefab(Arc<PrefabAsset>),
    /// Scene.
    Scene(Arc<SceneAsset>),
}

impl Asset {
    /// What kind of asset this is.
    pub fn kind(&self) -> AssetKind {
        match self {
            Asset::Mesh(_) => AssetKind::Mesh,
            Asset::Texture(_) => AssetKind::Texture,
            Asset::Material(_) => AssetKind::Material,
            Asset::Prefab(_) => AssetKind::Prefab,
            Asset::Scene(_) => AssetKind::Scene,
        }
    }

    /// Assets this one needs; the bool is true for hard dependencies (failure propagates, the
    /// asset only becomes `Loaded` once they are). Scene references are soft so one bad mesh
    /// does not fail the whole scene (the renderer shows fallbacks instead).
    pub fn deps(&self) -> (Vec<AssetId>, bool) {
        match self {
            Asset::Material(m) => (m.textures(), true),
            Asset::Prefab(p) => (p.deps(), true),
            Asset::Scene(s) => (s.deps.clone(), false),
            _ => (Vec::new(), true),
        }
    }
}
