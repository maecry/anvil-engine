//! Project lifecycle: create a project from code (copying the sample's assets), build a scene,
//! save, reload in a fresh world, then run it headless (GPU part gated by `ANVIL_GPU_TESTS=1`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use assets::AssetId;
use foundation::math::Vec3;
use runtime::{Application, Context, PROJECT_VERSION, Project, RunOptions, Runtime};
use scene::{Camera, Light, LightKind, MeshRenderer, SceneFile, Transform, World};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (p, name) = (e.path(), e.file_name());
        if name == ".anvil" {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &to.join(&name));
        } else {
            std::fs::copy(&p, to.join(&name)).unwrap();
        }
    }
}

fn sample_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sample-project")
}

fn gpu_enabled() -> bool {
    let on = std::env::var("ANVIL_GPU_TESTS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
    }
    on
}

fn sorted(mut f: SceneFile) -> SceneFile {
    f.entities.sort_by_key(|e| e.uuid.0);
    f
}

struct Probe {
    updates: Arc<AtomicU64>,
}

impl Application for Probe {
    fn start(&mut self, ctx: &mut Context) {
        ctx.world.spawn_entity("probe");
    }

    fn update(&mut self, _: &mut Context, dt: f32) {
        assert!(dt >= 0.0);
        self.updates.fetch_add(1, Ordering::Relaxed);
    }
}

fn headless(dir: &Path, frames: u64, shot: &Path) -> (runtime::RunReport, u64) {
    let _ = foundation::init_logging();
    let mut opts = RunOptions::from_env(dir);
    (opts.headless, opts.hot_reload) = (true, false);
    opts.window.width = 640;
    opts.window.height = 360;
    opts.exit_after_frames = Some(frames);
    opts.screenshot = Some(shot.to_owned());
    let updates = Arc::new(AtomicU64::new(0));
    let report = Runtime::run(
        opts,
        Probe {
            updates: updates.clone(),
        },
    )
    .unwrap();
    (report, updates.load(Ordering::Relaxed))
}

/// Number of pixels of the PNG whose color is not the clear black.
fn lit_pixels(png_path: &Path) -> usize {
    let dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(png_path).unwrap(),
    ));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf[..info.buffer_size()]
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
        .count()
}

#[test]
fn project_file_round_trip_and_version() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::new("demo", "assets/main.scene");
    p.save(dir.path()).unwrap();
    assert_eq!(Project::load(dir.path()).unwrap(), p);
    std::fs::write(
        dir.path().join(runtime::PROJECT_FILE),
        format!(
            "(version: {}, name: \"x\", startup_scene: \"y\")",
            PROJECT_VERSION + 1
        ),
    )
    .unwrap();
    assert!(Project::load(dir.path()).is_err());
}

#[test]
fn create_save_reopen_run() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&sample_dir().join("assets"), &dir.path().join("assets"));

    // Build a scene from code against the copied assets: same ids as the sample (metas copied).
    let server = assets::AssetServer::new(dir.path()).unwrap();
    let mesh = server
        .list()
        .into_iter()
        .find(|i| i.kind == assets::AssetKind::Mesh)
        .expect("sample has a glTF mesh")
        .id;
    let material = server
        .id_for_path("assets/materials/bricks.material")
        .unwrap();
    let mut w = World::new();
    let cam = w.spawn_entity("cam");
    let _ = w.insert_one(cam, Transform::from_translation(Vec3::new(0.0, 1.0, 4.0)));
    let _ = w.insert_one(cam, Camera::default());
    let sun = w.spawn_entity("sun");
    let _ = w.insert_one(
        sun,
        Light {
            kind: LightKind::Directional,
            intensity: 3.0,
            ..Light::default()
        },
    );
    let parent = w.spawn_entity("parent");
    let child = w.spawn_entity("child");
    w.set_parent(child, Some(parent)).unwrap();
    let _ = w.insert_one(child, MeshRenderer { mesh, material });
    let _ = w.insert_one(child, Transform::from_translation(Vec3::X));
    let saved = SceneFile::from_world(&w);
    std::fs::write(
        dir.path().join("assets/scenes/custom.scene"),
        saved.to_ron(),
    )
    .unwrap();
    Project::new("temp", "assets/scenes/custom.scene")
        .save(dir.path())
        .unwrap();

    // Reopen into a fresh world and compare.
    let project = Project::load(dir.path()).unwrap();
    let text = std::fs::read_to_string(dir.path().join(&project.startup_scene)).unwrap();
    let mut w2 = World::new();
    SceneFile::from_ron(&text)
        .unwrap()
        .spawn_into(&mut w2)
        .unwrap();
    assert_eq!(sorted(SceneFile::from_world(&w2)), sorted(saved));

    if !gpu_enabled() {
        return;
    }
    let shot = dir.path().join("shot.png");
    let (report, updates) = headless(dir.path(), 12, &shot);
    assert_eq!((report.frames, updates), (12, 12));
    assert_eq!(
        (report.validation_errors, report.validation_warnings),
        (0, 0)
    );
    let lit = lit_pixels(&shot);
    assert!(
        lit > 640 * 360 / 2,
        "screenshot is mostly lit ({lit} pixels)"
    );
}

#[test]
fn broken_assets_fall_back_instead_of_failing() {
    if !gpu_enabled() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/bad.png"), b"not an image").unwrap();
    std::fs::write(dir.path().join("assets/bad.glb"), b"glTF-but-not-really").unwrap();
    let server = assets::AssetServer::new(dir.path()).unwrap();
    let bad_tex = server.id_for_path("assets/bad.png").unwrap();
    let bad_glb = server.id_for_path("assets/bad.glb").unwrap();
    let mat = assets::MaterialAsset {
        base_color_texture: Some(bad_tex),
        ..assets::MaterialAsset::default()
    };
    std::fs::write(
        dir.path().join("assets/bad.material"),
        ron::to_string(&mat).unwrap(),
    )
    .unwrap();
    let bad_mat = server.id_for_path("assets/bad.material");
    drop(server);
    let bad_mat = bad_mat.unwrap_or_else(|| {
        assets::AssetServer::new(dir.path())
            .unwrap()
            .id_for_path("assets/bad.material")
            .unwrap()
    });
    let mut w = World::new();
    let cam = w.spawn_entity("cam");
    let _ = w.insert_one(cam, Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)));
    let _ = w.insert_one(cam, Camera::default());
    let e = w.spawn_entity("broken");
    // Unknown mesh id + failed material: default cube drawn in magenta.
    let _ = w.insert_one(
        e,
        MeshRenderer {
            mesh: bad_glb,
            material: bad_mat,
        },
    );
    let e2 = w.spawn_entity("unknown");
    let _ = w.insert_one(e2, Transform::from_translation(Vec3::new(1.5, 0.0, 0.0)));
    let _ = w.insert_one(
        e2,
        MeshRenderer {
            mesh: AssetId::random(),
            material: AssetId::random(),
        },
    );
    std::fs::create_dir_all(dir.path().join("assets/scenes")).unwrap();
    std::fs::write(
        dir.path().join("assets/scenes/s.scene"),
        SceneFile::from_world(&w).to_ron(),
    )
    .unwrap();
    Project::new("broken", "assets/scenes/s.scene")
        .save(dir.path())
        .unwrap();

    let shot = dir.path().join("shot.png");
    let (report, _) = headless(dir.path(), 6, &shot);
    assert_eq!(
        (report.validation_errors, report.validation_warnings),
        (0, 0)
    );
    assert!(lit_pixels(&shot) > 1000);
}

/// Rewrites the bricks texture mid-run and slows the frames down so the debounced reload lands.
struct Recolor {
    png: std::path::PathBuf,
}

impl Application for Recolor {
    fn start(&mut self, _: &mut Context) {}

    fn update(&mut self, ctx: &mut Context, _: f32) {
        if ctx.time.frame == 3 {
            let green = assets::fixtures::checker_png(64, 16, [0, 255, 0], [0, 200, 0]);
            std::fs::write(&self.png, green).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

#[test]
fn hot_reload_replaces_gpu_texture() {
    if !gpu_enabled() {
        return;
    }
    let _ = foundation::init_logging();
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&sample_dir().join("assets"), &dir.path().join("assets"));
    std::fs::copy(
        sample_dir().join("project.anvil"),
        dir.path().join("project.anvil"),
    )
    .unwrap();
    let mut opts = RunOptions::from_env(dir.path());
    (opts.headless, opts.hot_reload) = (true, true);
    (opts.window.width, opts.window.height) = (640, 360);
    opts.exit_after_frames = Some(40);
    let shot = dir.path().join("shot.png");
    opts.screenshot = Some(shot.clone());
    let report = Runtime::run(
        opts,
        Recolor {
            png: dir.path().join("assets/textures/bricks.png"),
        },
    )
    .unwrap();
    assert_eq!(
        (report.validation_errors, report.validation_warnings),
        (0, 0)
    );
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&shot).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let green = buf[..info.buffer_size()]
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| {
            i32::from(p[1]) > i32::from(p[0]) + 60 && i32::from(p[1]) > i32::from(p[2]) + 60
        })
        .count();
    assert!(green > 300, "the brick crate turned green ({green} pixels)");
}
