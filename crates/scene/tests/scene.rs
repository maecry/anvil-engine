//! Hierarchy, transform system, serialization and prefab instantiation.

use assets::{AssetId, PrefabAsset, PrefabMesh, PrefabNode};
use foundation::math::{Quat, Vec3};
use scene::{
    Camera, GlobalTransform, Light, LightKind, MeshRenderer, SceneError, SceneFile, Transform,
    World, instantiate_prefab, update_transforms,
};

fn at(x: f32) -> Transform {
    Transform::from_translation(Vec3::new(x, 0.0, 0.0))
}

#[test]
fn hierarchy_stays_consistent() {
    let mut w = World::new();
    let (a, b, c) = (
        w.spawn_entity("a"),
        w.spawn_entity("b"),
        w.spawn_entity("c"),
    );
    w.set_parent(b, Some(a)).unwrap();
    w.set_parent(c, Some(a)).unwrap();
    assert_eq!(w.children_of(a), vec![b, c]);
    w.set_parent(c, Some(b)).unwrap();
    assert_eq!(w.children_of(a), vec![b]);
    assert_eq!(w.parent_of(c), Some(b));
    assert!(w.set_parent(a, Some(c)).is_err(), "cycle rejected");
    assert!(w.set_parent(a, Some(a)).is_err());
    w.set_parent(c, None).unwrap();
    assert_eq!(w.parent_of(c), None);
    assert!(w.children_of(b).is_empty());
    w.despawn_recursive(a);
    assert!(!w.contains(a) && !w.contains(b) && w.contains(c));
}

#[test]
fn transforms_parents_first() {
    let mut w = World::new();
    // Spawn the child before the parent so insertion order cannot hide ordering bugs.
    let child = w.spawn_entity("child");
    let parent = w.spawn_entity("parent");
    w.set_parent(child, Some(parent)).unwrap();
    let _ = w.insert_one(
        parent,
        Transform {
            translation: Vec3::new(10.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::splat(2.0),
        },
    );
    let _ = w.insert_one(child, at(1.0));
    update_transforms(&mut w);
    let g = *w.get::<&GlobalTransform>(child).unwrap();
    // +X of the child is rotated to -Z by the parent, scaled by 2.
    let p = g.matrix.transform_point3(Vec3::ZERO);
    assert!((p - Vec3::new(10.0, 0.0, -2.0)).length() < 1e-4, "{p:?}");
    assert_eq!(g.prev, g.matrix, "first update: prev equals current");
    let _ = w.insert_one(parent, at(11.0));
    update_transforms(&mut w);
    let g2 = *w.get::<&GlobalTransform>(child).unwrap();
    assert_eq!(g2.prev, g.matrix);
}

fn sample_world() -> World {
    let mut w = World::new();
    let root = w.spawn_entity("root");
    let kid = w.spawn_entity("kid");
    let sun = w.spawn_entity("sun");
    let cam = w.spawn_entity("camera");
    w.set_parent(kid, Some(root)).unwrap();
    let _ = w.insert_one(kid, at(3.0));
    let _ = w.insert_one(
        kid,
        MeshRenderer {
            mesh: AssetId::random(),
            material: AssetId::NIL,
        },
    );
    let _ = w.insert_one(
        sun,
        Light {
            kind: LightKind::Directional,
            cast_shadows: true,
            ..Light::default()
        },
    );
    let _ = w.insert_one(cam, Camera::default());
    w
}

fn sorted(mut f: SceneFile) -> SceneFile {
    f.entities.sort_by_key(|e| e.uuid.0);
    f
}

#[test]
fn ron_round_trip_into_fresh_world() {
    let w = sample_world();
    let text = SceneFile::from_world(&w).to_ron();
    assert!(text.contains("version: 1"));
    let mut w2 = World::new();
    SceneFile::from_ron(&text)
        .unwrap()
        .spawn_into(&mut w2)
        .unwrap();
    assert_eq!(
        sorted(SceneFile::from_world(&w)),
        sorted(SceneFile::from_world(&w2))
    );
    // Parent links survive through the uuids.
    let kid = w2
        .query::<(hecs::Entity, &scene::Name)>()
        .iter()
        .find(|(_, n)| n.0 == "kid")
        .unwrap()
        .0;
    assert!(w2.parent_of(kid).is_some());
}

#[test]
fn newer_version_and_bad_links_are_rejected() {
    assert!(matches!(
        SceneFile::from_ron("(version: 99, entities: [])"),
        Err(SceneError::Version { found: 99, .. })
    ));
    assert!(matches!(
        SceneFile::from_ron("(version: 1, entities: ["),
        Err(SceneError::Parse(_))
    ));
    let mut f = SceneFile::from_world(&sample_world());
    f.entities.retain(|e| e.parent.is_none());
    let kid_less = f.entities.len();
    f.entities[0].parent = Some(scene::EntityUuid(12345));
    let mut w = World::new();
    assert!(matches!(
        f.spawn_into(&mut w),
        Err(SceneError::Hierarchy(_))
    ));
    assert_eq!(
        w.len(),
        0,
        "failed load leaves nothing behind (had {kid_less} entities)"
    );
}

#[test]
fn prefab_instantiation() {
    let (m, mat) = (AssetId::random(), AssetId::random());
    let node = |name: &str, parent| PrefabNode {
        name: name.into(),
        parent,
        translation: [1.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
        mesh: Some(PrefabMesh {
            mesh: m,
            material: mat,
        }),
    };
    let prefab = PrefabAsset {
        nodes: vec![node("a", None), node("b", Some(0))],
    };
    let mut w = World::new();
    let top = instantiate_prefab(&mut w, &prefab, "model", at(5.0));
    update_transforms(&mut w);
    assert_eq!(w.len(), 3);
    let leaf = w
        .query::<(hecs::Entity, &scene::Name)>()
        .iter()
        .find(|(_, n)| n.0 == "b")
        .unwrap()
        .0;
    let x = w
        .get::<&GlobalTransform>(leaf)
        .unwrap()
        .matrix
        .transform_point3(Vec3::ZERO)
        .x;
    assert!((x - 7.0).abs() < 1e-5, "5 + 1 + 1, got {x}");
    assert!(w.get::<&MeshRenderer>(leaf).is_ok());
    assert_eq!(w.children_of(top).len(), 1);
}
