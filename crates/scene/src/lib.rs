//! ECS world, hierarchy, transforms and scene serialization.
//!
//! Specification: `docs/specs/layers/LAYER-010-runtime-and-editor.md`, ADR-0014 §2-3. GPU-free:
//! meshes and materials are referenced by [`assets::AssetId`]; the runtime resolves them.
//!
//! The world is a [`hecs::World`] wrapper; systems are plain functions the runtime calls in a
//! fixed order ([`update_transforms`]). Scenes are RON files with a `version` field.

mod components;
mod file;
mod world;

pub use components::{
    Camera, Children, EntityUuid, GlobalTransform, Light, LightKind, MeshRenderer, Name, Parent,
    Transform,
};
pub use file::{EntityData, SceneError, SceneFile, instantiate_prefab};
pub use world::{World, update_transforms};
