use std::ops::{Deref, DerefMut};

use assets::AssetId;
use foundation::math::Mat4;
use hecs::Entity;

use crate::{Children, EntityUuid, GlobalTransform, Name, Parent, SceneError, Transform};

/// A [`hecs::World`] plus hierarchy helpers. Derefs to the inner world for queries and spawns.
#[derive(Default)]
pub struct World {
    ecs: hecs::World,
}

impl Deref for World {
    type Target = hecs::World;
    fn deref(&self) -> &hecs::World {
        &self.ecs
    }
}

impl DerefMut for World {
    fn deref_mut(&mut self) -> &mut hecs::World {
        &mut self.ecs
    }
}

impl World {
    /// An empty world.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns an entity with a name, identity transform and a fresh [`EntityUuid`].
    pub fn spawn_entity(&mut self, name: &str) -> Entity {
        self.ecs.spawn((
            Name(name.to_owned()),
            EntityUuid(AssetId::random().0),
            Transform::IDENTITY,
            GlobalTransform::default(),
        ))
    }

    /// Entity with this uuid.
    pub fn find_uuid(&self, uuid: EntityUuid) -> Option<Entity> {
        self.ecs
            .query::<(Entity, &EntityUuid)>()
            .iter()
            .find(|(_, u)| **u == uuid)
            .map(|(e, _)| e)
    }

    /// Parent of `e`.
    pub fn parent_of(&self, e: Entity) -> Option<Entity> {
        self.ecs.get::<&Parent>(e).ok().map(|p| p.0)
    }

    /// Children of `e`.
    pub fn children_of(&self, e: Entity) -> Vec<Entity> {
        self.ecs
            .get::<&Children>(e)
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    /// Re-parents `child` (`None` detaches it), keeping [`Parent`] and [`Children`] consistent.
    ///
    /// # Errors
    /// [`SceneError::Hierarchy`] for a missing entity or a cycle (parenting under itself or a
    /// descendant).
    pub fn set_parent(&mut self, child: Entity, parent: Option<Entity>) -> Result<(), SceneError> {
        if !self.ecs.contains(child) || parent.is_some_and(|p| !self.ecs.contains(p)) {
            return Err(SceneError::Hierarchy("entity does not exist".into()));
        }
        let mut up = parent;
        while let Some(p) = up {
            if p == child {
                return Err(SceneError::Hierarchy("parent cycle".into()));
            }
            up = self.parent_of(p);
        }
        if let Some(old) = self.parent_of(child)
            && let Ok(mut c) = self.ecs.get::<&mut Children>(old)
        {
            c.0.retain(|e| *e != child);
        }
        match parent {
            Some(p) => {
                let _ = self.ecs.insert_one(child, Parent(p));
                if let Ok(mut c) = self.ecs.get::<&mut Children>(p) {
                    c.0.push(child);
                } else {
                    let _ = self.ecs.insert_one(p, Children(vec![child]));
                }
            }
            None => {
                let _ = self.ecs.remove_one::<Parent>(child);
            }
        }
        Ok(())
    }

    /// Despawns `e` and all its descendants, and removes it from its parent's children.
    pub fn despawn_recursive(&mut self, e: Entity) {
        let _ = self.set_parent(e, None);
        let mut stack = vec![e];
        while let Some(x) = stack.pop() {
            stack.extend(self.children_of(x));
            let _ = self.ecs.despawn(x);
        }
    }
}

/// Computes every [`GlobalTransform`] from the local [`Transform`]s, parents first. Entities
/// without a `Transform` are skipped together with their subtree.
pub fn update_transforms(world: &mut World) {
    let roots: Vec<Entity> = world
        .ecs
        .query::<(Entity, &Transform)>()
        .without::<&Parent>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    let mut stack: Vec<(Entity, Mat4)> = roots.into_iter().map(|e| (e, Mat4::IDENTITY)).collect();
    while let Some((e, parent_matrix)) = stack.pop() {
        let Ok(local) = world.ecs.get::<&Transform>(e).map(|t| t.matrix()) else {
            continue;
        };
        let matrix = parent_matrix * local;
        let prev = world.ecs.get::<&GlobalTransform>(e).ok().map(|g| *g);
        let new = GlobalTransform {
            matrix,
            prev: prev.filter(|g| g.valid).map_or(matrix, |g| g.matrix),
            valid: true,
        };
        if let Ok(mut g) = world.ecs.get::<&mut GlobalTransform>(e) {
            *g = new;
        } else {
            let _ = world.ecs.insert_one(e, new);
        }
        stack.extend(world.children_of(e).into_iter().map(|c| (c, matrix)));
    }
}
