//! Importers, ids, cache, dependencies, corrupt input and hot reload on real files in temp
//! projects (CPU only).

use std::path::Path;
use std::time::{Duration, Instant};

use assets::fixtures::{FixtureMaterial, checker_png, cube, glb, quad};
use assets::{
    Asset, AssetError, AssetEvent, AssetId, AssetServer, LoadState, MaterialAsset, SCENE_VERSION,
};

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// Updates until nothing is pending (10 s limit), returning every event.
fn settle(s: &mut AssetServer) -> Vec<AssetEvent> {
    let end = Instant::now() + Duration::from_secs(10);
    let mut all = Vec::new();
    loop {
        all.extend(s.update());
        if !s.is_busy() {
            return all;
        }
        assert!(Instant::now() < end, "assets did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn model() -> Vec<u8> {
    let mut c = cube(1.0);
    c.material = Some(0);
    let mut g = quad(10.0, 4.0);
    g.material = Some(1);
    g.translation = [0.0, -0.5, 0.0];
    glb(
        &[c, g],
        &[
            FixtureMaterial {
                base_color: [1.0, 0.5, 0.25, 1.0],
                textured: true,
                metallic: 0.0,
                roughness: 0.6,
            },
            FixtureMaterial {
                base_color: [0.3; 4],
                textured: false,
                metallic: 0.0,
                roughness: 0.9,
            },
        ],
        Some(&checker_png(8, 2, [255, 255, 255], [10, 10, 10])),
    )
}

fn load_one(s: &mut AssetServer, path: &str) -> (AssetId, LoadState) {
    let id = s.load_path(path).unwrap();
    settle(s);
    (id, s.state(id).unwrap())
}

#[test]
fn meta_created_and_id_survives_move() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "assets/a/tex.png",
        &checker_png(4, 2, [1; 3], [2; 3]),
    );
    let id = AssetServer::new(dir.path())
        .unwrap()
        .id_for_path("assets/a/tex.png")
        .unwrap();
    assert!(dir.path().join("assets/a/tex.png.meta").exists());
    // Same id on reopen, and after moving the file together with its meta.
    std::fs::create_dir_all(dir.path().join("assets/b")).unwrap();
    for ext in ["png", "png.meta"] {
        std::fs::rename(
            dir.path().join(format!("assets/a/tex.{ext}")),
            dir.path().join(format!("assets/b/moved.{ext}")),
        )
        .unwrap();
    }
    let s = AssetServer::new(dir.path()).unwrap();
    assert_eq!(s.id_for_path("assets/b/moved.png"), Some(id));
    assert_eq!(s.id_for_path("assets/a/tex.png"), None);
}

#[test]
fn duplicate_meta_gets_new_id() {
    let dir = tempfile::tempdir().unwrap();
    let png = checker_png(4, 2, [1; 3], [2; 3]);
    write(dir.path(), "assets/a.png", &png);
    let a = AssetServer::new(dir.path())
        .unwrap()
        .id_for_path("assets/a.png")
        .unwrap();
    write(dir.path(), "assets/b.png", &png);
    std::fs::copy(
        dir.path().join("assets/a.png.meta"),
        dir.path().join("assets/b.png.meta"),
    )
    .unwrap();
    let s = AssetServer::new(dir.path()).unwrap();
    assert_eq!(s.id_for_path("assets/a.png"), Some(a));
    assert_ne!(s.id_for_path("assets/b.png"), Some(a));
}

#[test]
fn gltf_import_golden() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "assets/model.glb", &model());
    let mut s = AssetServer::new(dir.path()).unwrap();
    let (root, state) = load_one(&mut s, "assets/model.glb");
    assert_eq!(state, LoadState::Loaded);
    let Some(Asset::Prefab(prefab)) = s.get(root) else {
        panic!("prefab")
    };
    assert_eq!(prefab.nodes.len(), 2);
    assert_eq!(prefab.nodes[0].name, "cube");
    assert_eq!(prefab.nodes[1].translation, [0.0, -0.5, 0.0]);
    let m0 = prefab.nodes[0].mesh.unwrap();
    assert_eq!(m0.mesh, AssetId::derive(root, "mesh/0/0"));
    assert_eq!(m0.material, AssetId::derive(root, "material/0"));

    let Some(Asset::Mesh(mesh)) = s.get(m0.mesh) else {
        panic!("mesh")
    };
    assert_eq!((mesh.positions.len(), mesh.indices.len()), (24, 36));
    for t in &mesh.tangents {
        let l = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
        assert!((l - 1.0).abs() < 1e-3, "generated tangent is unit length");
    }
    let Some(Asset::Material(mat)) = s.get(m0.material) else {
        panic!("material")
    };
    assert_eq!(mat.base_color, [1.0, 0.5, 0.25, 1.0]);
    assert_eq!(
        mat.base_color_texture,
        Some(AssetId::derive(root, "image/0"))
    );
    let Some(Asset::Texture(tex)) = s.get(AssetId::derive(root, "image/0")) else {
        panic!("texture")
    };
    assert_eq!((tex.width, tex.height, tex.rgba8.len()), (8, 8, 256));
    // Sub-assets are listed in the meta and survive a restart without importing.
    let meta = std::fs::read_to_string(dir.path().join("assets/model.glb.meta")).unwrap();
    assert!(meta.contains("mesh/0/0") && meta.contains("image/0"));
    let s2 = AssetServer::new(dir.path()).unwrap();
    assert!(
        s2.list()
            .iter()
            .any(|i| i.id == m0.mesh && i.parent == Some(root))
    );
}

#[test]
fn corrupt_files_fail_without_panic() {
    let dir = tempfile::tempdir().unwrap();
    let glb_bytes = model();
    write(
        dir.path(),
        "assets/trunc.glb",
        &glb_bytes[..glb_bytes.len() / 2],
    );
    write(
        dir.path(),
        "assets/garbage.png",
        b"definitely not a png at all",
    );
    write(dir.path(), "assets/garbage.ktx2", &[0xAB; 200]);
    write(
        dir.path(),
        "assets/bad.material",
        b"(version: 1, base_color: ",
    );
    write(
        dir.path(),
        "assets/future.material",
        format!("(version: {})", assets::MATERIAL_VERSION + 1).as_bytes(),
    );
    write(
        dir.path(),
        "assets/future.scene",
        format!("(version: {}, entities: [])", SCENE_VERSION + 1).as_bytes(),
    );
    let mut s = AssetServer::new(dir.path()).unwrap();
    for p in ["trunc.glb", "garbage.png", "garbage.ktx2", "bad.material"] {
        let (_, st) = load_one(&mut s, &format!("assets/{p}"));
        assert!(
            matches!(st, LoadState::Failed(_)),
            "{p} must fail, got {st:?}"
        );
    }
    for p in ["future.material", "future.scene"] {
        let (_, st) = load_one(&mut s, &format!("assets/{p}"));
        assert!(
            matches!(st, LoadState::Failed(AssetError::Version { .. })),
            "{p}: {st:?}"
        );
    }
    assert_eq!(s.stats().failed, 6);
}

#[test]
fn failed_dependency_propagates() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "assets/bad.png", b"garbage");
    write(
        dir.path(),
        "assets/good.png",
        &checker_png(4, 2, [1; 3], [2; 3]),
    );
    let s = AssetServer::new(dir.path()).unwrap();
    let bad = s.id_for_path("assets/bad.png").unwrap();
    let good = s.id_for_path("assets/good.png").unwrap();
    for (name, tex) in [("bad", bad), ("good", good)] {
        let m = MaterialAsset {
            base_color_texture: Some(tex),
            ..MaterialAsset::default()
        };
        write(
            dir.path(),
            &format!("assets/{name}.material"),
            ron::to_string(&m).unwrap().as_bytes(),
        );
    }
    let mut s = AssetServer::new(dir.path()).unwrap();
    let (_, st) = load_one(&mut s, "assets/bad.material");
    assert!(
        matches!(st, LoadState::Failed(AssetError::Dependency { id, .. }) if id == bad),
        "{st:?}"
    );
    let (_, st) = load_one(&mut s, "assets/good.material");
    assert_eq!(st, LoadState::Loaded);
}

#[test]
fn cache_hit_and_reimport_on_change() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "assets/model.glb", &model());
    {
        let mut s = AssetServer::new(dir.path()).unwrap();
        load_one(&mut s, "assets/model.glb");
        let st = s.stats();
        assert_eq!((st.cache_hits, st.cache_misses), (0, 1));
    }
    assert!(dir.path().join(".anvil/imported").is_dir());
    {
        let mut s = AssetServer::new(dir.path()).unwrap();
        let (_, state) = load_one(&mut s, "assets/model.glb");
        assert_eq!(state, LoadState::Loaded);
        let st = s.stats();
        assert_eq!(
            (st.cache_hits, st.cache_misses),
            (1, 0),
            "second load hits the cache"
        );
    }
    // Change the source: the hash differs, so it is imported again.
    let mut c = cube(2.0);
    c.material = Some(0);
    write(
        dir.path(),
        "assets/model.glb",
        &glb(
            &[c],
            &[FixtureMaterial {
                base_color: [1.0; 4],
                textured: false,
                metallic: 0.0,
                roughness: 1.0,
            }],
            None,
        ),
    );
    let mut s = AssetServer::new(dir.path()).unwrap();
    let (root, state) = load_one(&mut s, "assets/model.glb");
    assert_eq!(state, LoadState::Loaded);
    assert_eq!(s.stats().cache_misses, 1);
    let Some(Asset::Prefab(p)) = s.get(root) else {
        panic!()
    };
    assert_eq!(p.nodes.len(), 1);
    // A damaged cache file is ignored and rewritten.
    let cache = dir.path().join(format!(".anvil/imported/{root}.bin"));
    std::fs::write(&cache, b"junk").unwrap();
    let mut s = AssetServer::new(dir.path()).unwrap();
    assert_eq!(load_one(&mut s, "assets/model.glb").1, LoadState::Loaded);
    assert_eq!(s.stats().cache_misses, 1);
}

#[test]
fn hot_reload_replaces_asset_and_notifies_dependents() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "assets/t.png",
        &checker_png(4, 2, [255, 0, 0], [0, 0, 255]),
    );
    let tex = AssetServer::new(dir.path())
        .unwrap()
        .id_for_path("assets/t.png")
        .unwrap();
    let m = MaterialAsset {
        base_color_texture: Some(tex),
        ..MaterialAsset::default()
    };
    write(
        dir.path(),
        "assets/m.material",
        ron::to_string(&m).unwrap().as_bytes(),
    );
    let mut s = AssetServer::new(dir.path()).unwrap();
    let (mat, st) = load_one(&mut s, "assets/m.material");
    assert_eq!(st, LoadState::Loaded);
    s.enable_hot_reload().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    write(
        dir.path(),
        "assets/t.png",
        &checker_png(8, 2, [0, 255, 0], [0, 0, 0]),
    );
    let end = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while !events.contains(&AssetEvent::Reloaded(mat)) {
        events.extend(s.update());
        assert!(Instant::now() < end, "no reload event, got {events:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let tex_pos = events
        .iter()
        .position(|e| *e == AssetEvent::Reloaded(tex))
        .unwrap();
    let mat_pos = events
        .iter()
        .position(|e| *e == AssetEvent::Reloaded(mat))
        .unwrap();
    assert!(tex_pos < mat_pos, "dependency first: {events:?}");
    let Some(Asset::Texture(t)) = s.get(tex) else {
        panic!()
    };
    assert_eq!(t.width, 8);
}
