use std::collections::HashMap;

use assets::{PrefabAsset, SCENE_VERSION};
use foundation::math::{Quat, Vec3};
use hecs::Entity;
use serde::{Deserialize, Serialize};

use crate::{Camera, EntityUuid, Light, MeshRenderer, Name, Transform, World};

/// Scene and hierarchy errors.
#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    /// The RON text is malformed.
    #[error("scene parse: {0}")]
    Parse(String),
    /// The file is from a newer engine.
    #[error("scene version {found} is newer than the supported {supported}")]
    Version {
        /// Version found.
        found: u32,
        /// Newest supported.
        supported: u32,
    },
    /// A parent link is dangling, duplicated or cyclic.
    #[error("scene hierarchy: {0}")]
    Hierarchy(String),
}

/// Saved form of one entity. Only the built-in components are saved.
// shortcut: no registry for game-defined components yet, add serde-registered components when a game needs saved state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityData {
    /// Stable identity.
    pub uuid: EntityUuid,
    /// Name.
    #[serde(default)]
    pub name: Option<String>,
    /// Parent entity.
    #[serde(default)]
    pub parent: Option<EntityUuid>,
    /// Local transform.
    #[serde(default)]
    pub transform: Option<Transform>,
    /// Mesh and material.
    #[serde(default)]
    pub mesh_renderer: Option<MeshRenderer>,
    /// Light.
    #[serde(default)]
    pub light: Option<Light>,
    /// Camera.
    #[serde(default)]
    pub camera: Option<Camera>,
}

/// A `.scene` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneFile {
    /// Format version.
    pub version: u32,
    /// Entities.
    #[serde(default)]
    pub entities: Vec<EntityData>,
}

impl SceneFile {
    /// Captures the world's entities that have an [`EntityUuid`].
    pub fn from_world(world: &World) -> Self {
        let entities = world
            .query::<(Entity, &EntityUuid)>()
            .iter()
            .map(|(e, uuid)| {
                let r = world.entity(e).ok();
                EntityData {
                    uuid: *uuid,
                    name: r
                        .as_ref()
                        .and_then(|r| r.get::<&Name>())
                        .map(|n| n.0.clone()),
                    parent: world
                        .parent_of(e)
                        .and_then(|p| world.get::<&EntityUuid>(p).ok().map(|u| *u)),
                    transform: r.as_ref().and_then(|r| r.get::<&Transform>()).map(|c| *c),
                    mesh_renderer: r
                        .as_ref()
                        .and_then(|r| r.get::<&MeshRenderer>())
                        .map(|c| *c),
                    light: r.as_ref().and_then(|r| r.get::<&Light>()).map(|c| *c),
                    camera: r.as_ref().and_then(|r| r.get::<&Camera>()).map(|c| *c),
                }
            })
            .collect();
        Self {
            version: SCENE_VERSION,
            entities,
        }
    }

    /// Pretty RON text.
    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).unwrap_or_default()
    }

    /// Parses RON text, rejecting newer versions before reading the rest.
    ///
    /// # Errors
    /// [`SceneError::Parse`] or [`SceneError::Version`].
    pub fn from_ron(text: &str) -> Result<Self, SceneError> {
        #[derive(Deserialize)]
        struct Head {
            version: u32,
        }
        let head: Head = ron::from_str(text).map_err(|e| SceneError::Parse(e.to_string()))?;
        if head.version > SCENE_VERSION {
            return Err(SceneError::Version {
                found: head.version,
                supported: SCENE_VERSION,
            });
        }
        ron::from_str(text).map_err(|e| SceneError::Parse(e.to_string()))
    }

    /// Spawns the entities into `world` (additive) and returns them in file order.
    ///
    /// # Errors
    /// [`SceneError::Hierarchy`] for duplicate uuids, dangling parents or cycles; nothing is
    /// left spawned in that case.
    pub fn spawn_into(&self, world: &mut World) -> Result<Vec<Entity>, SceneError> {
        let mut by_uuid: HashMap<EntityUuid, Entity> = HashMap::new();
        let mut spawned = Vec::new();
        let result = (|| {
            for d in &self.entities {
                if by_uuid.contains_key(&d.uuid) || world.find_uuid(d.uuid).is_some() {
                    return Err(SceneError::Hierarchy(format!(
                        "duplicate uuid {:?}",
                        d.uuid
                    )));
                }
                let mut b = hecs::EntityBuilder::new();
                b.add(d.uuid)
                    .add(d.transform.unwrap_or_default())
                    .add(crate::GlobalTransform::default());
                if let Some(n) = &d.name {
                    b.add(Name(n.clone()));
                }
                if let Some(c) = d.mesh_renderer {
                    b.add(c);
                }
                if let Some(c) = d.light {
                    b.add(c);
                }
                if let Some(c) = d.camera {
                    b.add(c);
                }
                let e = world.spawn(b.build());
                by_uuid.insert(d.uuid, e);
                spawned.push(e);
            }
            for d in &self.entities {
                if let Some(p) = d.parent {
                    let parent = by_uuid.get(&p).ok_or_else(|| {
                        SceneError::Hierarchy(format!("parent {p:?} does not exist"))
                    })?;
                    world.set_parent(by_uuid[&d.uuid], Some(*parent))?;
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            for e2 in spawned {
                let _ = world.despawn(e2);
            }
            return Err(e);
        }
        Ok(spawned)
    }
}

/// Instantiates a glTF prefab: a root entity named `name` carrying `root` and one child entity
/// per prefab node (with `MeshRenderer`s). Returns the root.
pub fn instantiate_prefab(
    world: &mut World,
    prefab: &PrefabAsset,
    name: &str,
    root: Transform,
) -> Entity {
    let top = world.spawn_entity(name);
    let _ = world.insert_one(top, root);
    let mut made: Vec<Entity> = Vec::with_capacity(prefab.nodes.len());
    for n in &prefab.nodes {
        let e = world.spawn_entity(&n.name);
        let _ = world.insert_one(
            e,
            Transform {
                translation: Vec3::from(n.translation),
                rotation: Quat::from_array(n.rotation).normalize(),
                scale: Vec3::from(n.scale),
            },
        );
        if let Some(m) = n.mesh {
            let _ = world.insert_one(
                e,
                MeshRenderer {
                    mesh: m.mesh,
                    material: m.material,
                },
            );
        }
        // Parents come first in a prefab, so the index is already made.
        let parent = n
            .parent
            .and_then(|p| made.get(p as usize))
            .copied()
            .unwrap_or(top);
        let _ = world.set_parent(e, Some(parent));
        made.push(e);
    }
    top
}
