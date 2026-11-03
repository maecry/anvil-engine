//! [`EditorCommand`]: every edit the editor can make, as data (ADR-0014 §9).
//!
//! The UI never touches the world or the files directly: it queues commands and
//! [`crate::Editor::apply`] runs them. Scripts (`ANVIL_EDITOR_SCRIPT`) are RON lists of the
//! same commands, which is also the hook for undo/redo later.
// shortcut: no undo/redo; record the inverse of each applied command here when it is needed.

use std::path::PathBuf;

use assets::{AssetId, MaterialAsset};
use scene::{Camera, EntityUuid, Light};
use serde::{Deserialize, Serialize};

/// How a command names an entity: scripts use names, the UI uses uuids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EntityRef {
    /// First entity with this [`scene::Name`] (in spawn order).
    Name(String),
    /// Stable identity.
    Uuid(EntityUuid),
}

/// How a command names an asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AssetRef {
    /// Source file, relative to the project (`assets/models/crate.glb`).
    Path(String),
    /// Asset id (also reaches sub-assets such as a glTF's meshes).
    Id(AssetId),
}

/// Built-in components that can be added and removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComponentKind {
    /// [`scene::MeshRenderer`].
    MeshRenderer,
    /// [`scene::Light`].
    Light,
    /// [`scene::Camera`].
    Camera,
}

/// One editor action.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EditorCommand {
    /// Creates a project from the template in `path` (must not hold a project) and opens it.
    NewProject {
        /// Project directory.
        path: PathBuf,
    },
    /// Opens the project in `path` and its startup scene.
    OpenProject {
        /// Project directory.
        path: PathBuf,
    },
    /// Copies the file `src` to `assets/<dest>` of the open project and registers it.
    ImportFile {
        /// Source file on disk.
        src: PathBuf,
        /// Destination relative to `assets/`.
        dest: String,
    },
    /// Selects an entity (`None` clears the selection).
    Select {
        /// Entity to select.
        entity: Option<EntityRef>,
    },
    /// Spawns an entity with a transform and selects it.
    CreateEntity {
        /// Name.
        name: String,
        /// Parent entity.
        #[serde(default)]
        parent: Option<EntityRef>,
    },
    /// Despawns an entity and its descendants.
    DeleteEntity {
        /// Entity.
        entity: EntityRef,
    },
    /// Renames an entity.
    Rename {
        /// Entity.
        entity: EntityRef,
        /// New name.
        name: String,
    },
    /// Sets the local transform; rotation is euler angles in degrees (x, y, z), applied in Y-X-Z
    /// order (yaw first), which is also what the inspector shows.
    SetTransform {
        /// Entity.
        entity: EntityRef,
        /// Local translation.
        translation: [f32; 3],
        /// Local rotation, degrees about X, Y, Z.
        rotation_deg: [f32; 3],
        /// Local scale.
        scale: [f32; 3],
    },
    /// Re-parents an entity keeping its world transform (`None` detaches).
    SetParent {
        /// Entity.
        entity: EntityRef,
        /// New parent.
        parent: Option<EntityRef>,
    },
    /// Adds a default component.
    AddComponent {
        /// Entity.
        entity: EntityRef,
        /// Component.
        component: ComponentKind,
    },
    /// Removes a component.
    RemoveComponent {
        /// Entity.
        entity: EntityRef,
        /// Component.
        component: ComponentKind,
    },
    /// Replaces the entity's [`Light`].
    SetLight {
        /// Entity.
        entity: EntityRef,
        /// New light.
        light: Light,
    },
    /// Replaces the entity's [`Camera`].
    SetCamera {
        /// Entity.
        entity: EntityRef,
        /// New camera.
        camera: Camera,
    },
    /// Sets the mesh of the entity's `MeshRenderer` (added when missing; `None` = default cube).
    AssignMesh {
        /// Entity.
        entity: EntityRef,
        /// Mesh asset.
        mesh: Option<AssetRef>,
    },
    /// Sets the material of the entity's `MeshRenderer` (added when missing; `None` = default).
    AssignMaterial {
        /// Entity.
        entity: EntityRef,
        /// Material asset.
        material: Option<AssetRef>,
    },
    /// Instantiates a glTF model as a new entity tree and selects its root. Waits for the model
    /// to load.
    InstantiateModel {
        /// The model (`.gltf` / `.glb`).
        model: AssetRef,
        /// Name of the root entity.
        name: String,
        /// Where to put the root.
        #[serde(default)]
        translation: [f32; 3],
    },
    /// Writes a default `.material` file (`path` relative to the project, `assets/...`) and opens
    /// it in the material editor.
    CreateMaterial {
        /// Material file.
        path: String,
    },
    /// Opens a `.material` file in the material editor.
    OpenMaterial {
        /// Material file.
        path: String,
    },
    /// Replaces the material editor's draft of `path` (opening it when needed) and previews it
    /// live. Missing fields of `material` take their defaults.
    EditMaterial {
        /// Material file.
        path: String,
        /// New contents.
        material: MaterialAsset,
    },
    /// Writes the draft of `path` to its file.
    SaveMaterial {
        /// Material file.
        path: String,
    },
    /// Writes an empty `.scene` file.
    CreateScene {
        /// Scene file, relative to the project.
        path: String,
    },
    /// Replaces the world with the contents of a `.scene` file.
    OpenScene {
        /// Scene file, relative to the project.
        path: String,
    },
    /// Writes the world to the open scene's file.
    SaveScene,
    /// Snapshots the world and runs it with the scene's camera.
    Play,
    /// Restores the world from the snapshot taken by [`EditorCommand::Play`].
    Stop,
    /// Writes the whole editor window as a PNG once assets have settled.
    Screenshot {
        /// Output file.
        path: PathBuf,
    },
    /// Script only: waits this many frames.
    WaitFrames(u32),
    /// Fails the run unless the entity matches.
    AssertEntity {
        /// Entity name.
        name: String,
        /// Expected presence of a `MeshRenderer`.
        #[serde(default)]
        has_mesh: Option<bool>,
        /// Expected parent name (`""` = no parent).
        #[serde(default)]
        parent: Option<String>,
        /// Expected material (source path or sub-asset id) of its `MeshRenderer`.
        #[serde(default)]
        material: Option<AssetRef>,
        /// Expected local translation (tolerance 1e-3).
        #[serde(default)]
        translation: Option<[f32; 3]>,
    },
    /// Fails the run when the world has fewer entities.
    AssertEntityCount {
        /// Minimum entity count.
        at_least: usize,
    },
    /// Ends the run after the current frame.
    Quit,
}

/// Parses a script: a RON list of commands.
///
/// # Errors
/// The RON error, rendered.
pub fn parse_script(text: &str) -> Result<Vec<EditorCommand>, String> {
    ron::from_str(text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_parses_and_roundtrips() {
        let text = r#"[
            NewProject(path: "/tmp/p"),
            CreateEntity(name: "a", parent: Some(Name("root"))),
            SetTransform(entity: Name("a"), translation: (1.0, 2.0, 3.0), rotation_deg: (0.0, 90.0, 0.0), scale: (1.0, 1.0, 1.0)),
            EditMaterial(path: "assets/m.material", material: (metallic: 0.25)),
            WaitFrames(3),
            Quit,
        ]"#;
        let cmds = parse_script(text).unwrap();
        assert_eq!(cmds.len(), 6);
        let again = ron::to_string(&cmds).unwrap();
        assert_eq!(parse_script(&again).unwrap(), cmds);
        assert!(parse_script("[Bogus]").is_err());
    }
}
