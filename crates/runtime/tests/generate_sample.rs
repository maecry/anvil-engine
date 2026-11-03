//! Regenerates `samples/sample-project` (ignored by default):
//! `cargo test -p runtime --test generate_sample -- --ignored`.
//! Everything is computed here, no third-party content. Existing `.meta` files are kept, so
//! asset ids stay stable across regenerations; the scene gets fresh entity uuids.

use assets::fixtures::{FixtureMaterial, checker_png, cube, glb, quad};
use assets::{Asset, AssetId, AssetServer, MaterialAsset};
use foundation::math::{Quat, Vec3};
use runtime::Project;
use scene::{
    Camera, Light, LightKind, MeshRenderer, SceneFile, Transform, World, instantiate_prefab,
    update_transforms,
};

fn write(root: &std::path::Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

#[test]
#[ignore = "writes into samples/sample-project"]
fn generate_sample_project() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sample-project");

    // Source files.
    let mut crate_mesh = cube(1.0);
    crate_mesh.name = "crate";
    crate_mesh.material = Some(0);
    let mut ground = quad(30.0, 12.0);
    ground.translation = [0.0, -0.5, 0.0];
    ground.material = Some(1);
    let model = glb(
        &[crate_mesh, ground],
        &[
            FixtureMaterial {
                base_color: [1.0; 4],
                textured: true,
                metallic: 0.0,
                roughness: 0.6,
            },
            FixtureMaterial {
                base_color: [0.32, 0.36, 0.30, 1.0],
                textured: false,
                metallic: 0.0,
                roughness: 0.95,
            },
        ],
        Some(&checker_png(64, 8, [70, 120, 220], [235, 235, 245])),
    );
    write(&root, "assets/models/crate.glb", &model);
    write(
        &root,
        "assets/textures/bricks.png",
        &checker_png(64, 16, [200, 90, 50], [235, 215, 180]),
    );

    // Discover (creates .meta), then write the material that references the PNG's id.
    let server = AssetServer::new(&root).unwrap();
    let bricks_tex = server.id_for_path("assets/textures/bricks.png").unwrap();
    let material = MaterialAsset {
        base_color_texture: Some(bricks_tex),
        metallic: 0.0,
        roughness: 0.5,
        ..MaterialAsset::default()
    };
    let text = ron::ser::to_string_pretty(&material, ron::ser::PrettyConfig::default()).unwrap();
    write(&root, "assets/materials/bricks.material", text.as_bytes());

    // Import the glTF once so its sub-asset ids are written to the .meta.
    let mut server = AssetServer::new(&root).unwrap();
    let model_id = server.load_path("assets/models/crate.glb").unwrap();
    while server.is_busy() {
        server.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let Some(Asset::Prefab(prefab)) = server.get(model_id) else {
        panic!("crate.glb did not import: {:?}", server.state(model_id))
    };
    let crate_mesh_id = prefab.nodes[0].mesh.unwrap().mesh;

    // The scene.
    let mut w = World::new();
    let sun = w.spawn_entity("sun");
    let dir = Vec3::new(-0.45, -1.0, -0.55).normalize();
    let _ = w.insert_one(
        sun,
        Transform {
            rotation: Quat::from_rotation_arc(Vec3::NEG_Z, dir),
            ..Transform::IDENTITY
        },
    );
    let _ = w.insert_one(
        sun,
        Light {
            kind: LightKind::Directional,
            intensity: 3.0,
            color: [1.0, 0.96, 0.9],
            cast_shadows: true,
            ..Light::default()
        },
    );

    let cam = w.spawn_entity("camera");
    let view = foundation::math::camera::rh::view::look_at_mat4(
        Vec3::new(3.6, 2.4, 5.0),
        Vec3::new(0.3, 0.2, 0.0),
        Vec3::Y,
    )
    .inverse();
    let (_, rotation, translation) = view.to_scale_rotation_translation();
    let _ = w.insert_one(
        cam,
        Transform {
            translation,
            rotation,
            scale: Vec3::ONE,
        },
    );
    let _ = w.insert_one(cam, Camera::default());

    instantiate_prefab(&mut w, &prefab, "crate model", Transform::IDENTITY);

    let bricks = w.spawn_entity("brick crate");
    let _ = w.insert_one(
        bricks,
        Transform {
            translation: Vec3::new(1.9, -0.1, 0.8),
            rotation: Quat::from_rotation_y(0.5),
            scale: Vec3::splat(0.8),
        },
    );
    let bricks_mat = server
        .id_for_path("assets/materials/bricks.material")
        .unwrap();
    let _ = w.insert_one(
        bricks,
        MeshRenderer {
            mesh: crate_mesh_id,
            material: bricks_mat,
        },
    );
    update_transforms(&mut w);
    write(
        &root,
        "assets/scenes/main.scene",
        SceneFile::from_world(&w).to_ron().as_bytes(),
    );
    write(&root, ".gitignore", b".anvil/\n");
    Project::new("Sample Project", "assets/scenes/main.scene")
        .save(&root)
        .unwrap();
    let _ = AssetId::NIL;
}
