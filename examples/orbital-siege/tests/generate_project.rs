//! Regenerates `examples/orbital-siege/project` (ignored by default):
//! `cargo test -p orbital-siege --test generate_project -- --ignored`.
//! Everything is computed here, no third-party content. Existing `.meta` files are kept, so
//! asset ids stay stable. `project_is_complete` (always run) checks the game's names exist.

use assets::fixtures::{FixtureMesh, glb};
use assets::{AssetServer, MaterialAsset};
use std::f32::consts::{PI, TAU};
use std::path::{Path, PathBuf};

/// (name, base color, metallic, roughness, emissive). Names are used by `src/main.rs`.
type Row = (&'static str, [f32; 3], f32, f32, [f32; 3]);
const MATERIALS: &[Row] = &[
    ("hull", [0.55, 0.62, 0.75], 0.8, 0.3, [0.0; 3]),
    ("hull_dark", [0.08, 0.09, 0.13], 0.7, 0.4, [0.0; 3]),
    ("glow_cyan", [0.1, 0.8, 1.0], 0.0, 0.4, [0.1, 1.6, 2.4]),
    ("spinner", [0.9, 0.1, 0.55], 0.2, 0.35, [1.6, 0.1, 0.8]),
    ("gunship", [1.0, 0.45, 0.05], 0.5, 0.3, [0.8, 0.25, 0.0]),
    ("crawler", [0.3, 0.9, 0.2], 0.1, 0.45, [0.1, 0.7, 0.1]),
    ("eye", [1.0, 1.0, 0.8], 0.0, 0.5, [4.0, 3.6, 2.0]),
    ("shield", [0.2, 0.75, 0.65], 0.0, 0.5, [0.05, 0.4, 0.3]),
    ("shield_hurt", [0.9, 0.5, 0.15], 0.0, 0.6, [0.5, 0.15, 0.0]),
    ("bolt_player", [0.4, 1.0, 1.0], 0.0, 0.4, [2.0, 6.0, 7.0]),
    ("bolt_enemy", [1.0, 0.3, 0.1], 0.0, 0.4, [7.0, 1.5, 0.3]),
    ("ground", [0.025, 0.035, 0.07], 0.6, 0.55, [0.0; 3]),
    ("rail", [0.1, 0.3, 0.6], 0.0, 0.5, [0.1, 0.5, 1.4]),
    ("grid", [0.05, 0.12, 0.25], 0.0, 0.5, [0.03, 0.12, 0.3]),
    ("spark", [1.0, 0.8, 0.3], 0.0, 0.5, [6.0, 3.5, 0.8]),
    ("hud_score", [1.0, 0.8, 0.2], 0.3, 0.4, [1.2, 0.8, 0.1]),
    ("hud_wave", [0.6, 0.3, 1.0], 0.2, 0.4, [0.5, 0.2, 1.2]),
];

fn mesh(name: &'static str, grid: Vec<Vec<([f32; 3], [f32; 3])>>) -> FixtureMesh {
    // `grid` rows are rings of (position, normal); consecutive rings are stitched.
    let mut m = FixtureMesh {
        name,
        translation: [0.0; 3],
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        indices: vec![],
        material: None,
    };
    let w = grid[0].len() as u16;
    for ring in &grid {
        for (p, n) in ring {
            m.positions.push(*p);
            m.normals.push(*n);
            m.uvs.push([0.0, 0.0]);
        }
    }
    for r in 0..grid.len() as u16 - 1 {
        for c in 0..w - 1 {
            let a = r * w + c;
            m.indices.extend([a, a + 1, a + w, a + 1, a + w + 1, a + w]);
        }
    }
    m
}

const SEG: usize = 24;

fn sphere() -> FixtureMesh {
    let rings = (0..=16)
        .map(|i| {
            let t = PI * i as f32 / 16.0;
            (0..=SEG)
                .map(|j| {
                    let p = TAU * j as f32 / SEG as f32;
                    let n = [t.sin() * p.cos(), t.cos(), t.sin() * p.sin()];
                    (n.map(|v| v * 0.5), n)
                })
                .collect()
        })
        .collect();
    mesh("sphere", rings)
}

/// Radius 0.5, height 1, tip at +Y. Two rings give the smooth side; the base is a flat disc.
fn cone() -> FixtureMesh {
    let side = |y: f32, r: f32| -> Vec<_> {
        (0..=SEG)
            .map(|j| {
                let p = TAU * j as f32 / SEG as f32;
                let n = [p.cos(), 0.5, p.sin()];
                let l = (1.25f32).sqrt();
                ([r * p.cos(), y, r * p.sin()], n.map(|v| v / l))
            })
            .collect()
    };
    let mut m = mesh("cone", vec![side(0.5, 0.0), side(-0.5, 0.5)]);
    cap(&mut m, -0.5, [0.0, -1.0, 0.0]);
    m
}

/// Radius 0.5, height 1.
fn cylinder() -> FixtureMesh {
    let side = |y: f32| -> Vec<_> {
        (0..=SEG)
            .map(|j| {
                let p = TAU * j as f32 / SEG as f32;
                ([0.5 * p.cos(), y, 0.5 * p.sin()], [p.cos(), 0.0, p.sin()])
            })
            .collect()
    };
    let mut m = mesh("cylinder", vec![side(0.5), side(-0.5)]);
    cap(&mut m, 0.5, [0.0, 1.0, 0.0]);
    cap(&mut m, -0.5, [0.0, -1.0, 0.0]);
    m
}

fn cap(m: &mut FixtureMesh, y: f32, n: [f32; 3]) {
    let c = m.positions.len() as u16;
    m.positions.push([0.0, y, 0.0]);
    m.normals.push(n);
    m.uvs.push([0.0; 2]);
    for j in 0..=SEG {
        let p = TAU * j as f32 / SEG as f32;
        m.positions.push([0.5 * p.cos(), y, 0.5 * p.sin()]);
        m.normals.push(n);
        m.uvs.push([0.0; 2]);
    }
    for j in 0..SEG as u16 {
        let (a, b) = (c + 1 + j, c + 2 + j);
        m.indices
            .extend(if n[1] > 0.0 { [c, b, a] } else { [c, a, b] });
    }
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn project_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("project")
}

#[test]
#[ignore = "writes into examples/orbital-siege/project"]
fn generate_project() {
    let root = project_dir();
    let mut cube = assets::fixtures::cube(1.0);
    cube.name = "cube";
    // Order matters: the game addresses meshes as `mesh/<index>/0` (cube, sphere, cone, cylinder).
    write(
        &root,
        "assets/models/shapes.glb",
        &glb(&[cube, sphere(), cone(), cylinder()], &[], None),
    );
    for (name, c, metallic, roughness, e) in MATERIALS {
        let m = MaterialAsset {
            base_color: [c[0], c[1], c[2], 1.0],
            metallic: *metallic,
            roughness: *roughness,
            emissive: *e,
            ..MaterialAsset::default()
        };
        let text = ron::ser::to_string_pretty(&m, ron::ser::PrettyConfig::default()).unwrap();
        write(
            &root,
            &format!("assets/materials/{name}.material"),
            text.as_bytes(),
        );
    }
    // Empty startup scene: the game spawns everything in `start`.
    write(
        &root,
        "assets/scenes/main.scene",
        scene::SceneFile::from_world(&scene::World::new())
            .to_ron()
            .as_bytes(),
    );
    write(&root, ".gitignore", b".anvil/\n");
    runtime::Project::new("Orbital Siege", "assets/scenes/main.scene")
        .save(&root)
        .unwrap();

    // Import the glb once so its sub-asset ids are written to the .meta.
    let mut server = AssetServer::new(&root).unwrap();
    let id = server.load_path("assets/models/shapes.glb").unwrap();
    while server.is_busy() {
        server.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(matches!(server.get(id), Some(assets::Asset::Prefab(_))));
}

#[test]
fn project_is_complete() {
    let root = project_dir();
    assert!(root.join("project.anvil").exists(), "run the generator");
    let server = AssetServer::new(&root).unwrap();
    let glb = server.id_for_path("assets/models/shapes.glb").unwrap();
    for i in 0..4 {
        let id = assets::AssetId::derive(glb, &format!("mesh/{i}/0"));
        assert!(server.list().iter().any(|a| a.id == id), "mesh {i}");
    }
    for (name, ..) in MATERIALS {
        assert!(
            server
                .id_for_path(&format!("assets/materials/{name}.material"))
                .is_some(),
            "{name}"
        );
    }
}
