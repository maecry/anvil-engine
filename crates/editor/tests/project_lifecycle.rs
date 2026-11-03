//! The M6 exit criterion (ADR-0014 §10): a project is created, saved, reopened and executed.
//!
//! 1. `anvil-editor` runs a script headless: new project from the template, import a glTF from
//!    the sample project, instantiate it, create + edit + save + assign a material, move it, play
//!    and stop, save the scene, screenshot.
//! 2. A second `anvil-editor` process opens that project and asserts the entities, then
//!    screenshots.
//! 3. `anvil-player` runs the project headless: exit 0, no validation messages, a lit image.
//!
//! GPU test, gated by `ANVIL_GPU_TESTS=1`; sync validation is on for every process.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assets::{MaterialAsset, Meta};
use scene::SceneFile;

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sample-project")
}

fn gpu_enabled() -> bool {
    let on = std::env::var("ANVIL_GPU_TESTS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
    }
    on
}

/// Applies the headless, validated environment shared by every process of the test.
fn headless(cmd: &mut Command) -> &mut Command {
    cmd.env("ANVIL_WINDOW", "0")
        .env("ANVIL_VSYNC", "0")
        .env("ANVIL_SYNC_VALIDATION", "1")
        .env_remove("ANVIL_EXIT_AFTER_FRAMES")
        .env_remove("ANVIL_SCREENSHOT")
        .env_remove("ANVIL_EDITOR_SCRIPT")
        .env_remove("ANVIL_OVERLAY")
}

/// Runs a command, requires success and `validation_errors: 0, validation_warnings: 0` in its log.
fn run_checked(what: &str, cmd: &mut Command) -> String {
    let out: Output = cmd
        .output()
        .unwrap_or_else(|e| panic!("{what}: spawn: {e}"));
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let tail = || log.lines().rev().take(40).collect::<Vec<_>>().join("\n");
    assert!(
        out.status.success(),
        "{what} failed ({:?}):\n{}",
        out.status,
        tail()
    );
    assert!(
        log.contains("validation_errors: 0, validation_warnings: 0"),
        "{what}: validation messages or no report:\n{}",
        tail()
    );
    assert!(
        !log.contains(" ERROR "),
        "{what} logged errors:\n{}",
        tail()
    );
    log
}

fn run_editor(script: &str, dir: &Path, project: Option<&Path>) -> String {
    let script_path = dir.join(format!("script-{}.ron", script.len()));
    std::fs::write(&script_path, script).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anvil-editor"));
    headless(&mut cmd).env("ANVIL_EDITOR_SCRIPT", &script_path);
    if let Some(p) = project {
        cmd.arg(p);
    }
    run_checked("anvil-editor", &mut cmd)
}

/// The built `anvil-player` next to the test binaries; builds it first (a no-op when current),
/// because cargo does not build other packages' binaries for this package's tests.
fn player_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_owned();
    let mut build = Command::new(env!("CARGO"));
    build
        .args(["build", "-p", "player", "--manifest-path"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../player/Cargo.toml"));
    if profile_dir.file_name().is_some_and(|n| n == "release") {
        build.arg("--release");
    }
    assert!(build.status().unwrap().success(), "cargo build -p player");
    let player = profile_dir.join(format!("anvil-player{}", std::env::consts::EXE_SUFFIX));
    assert!(player.exists(), "{} not found", player.display());
    player
}

struct Image {
    w: usize,
    px: Vec<u8>,
}

fn read_png(path: &Path) -> Image {
    let mut r = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
        .read_info()
        .unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    Image {
        w: info.width as usize,
        px: buf,
    }
}

impl Image {
    fn pixels(&self) -> impl Iterator<Item = [u8; 3]> + '_ {
        self.px
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[0], p[1], p[2]])
    }

    fn lit(&self) -> usize {
        self.pixels().filter(|p| p.iter().any(|c| *c > 8)).count()
    }

    /// Pixels where red clearly dominates: the red material shows.
    fn reddish(&self) -> usize {
        self.pixels()
            .filter(|[r, g, b]| *r > 120 && *r > g.saturating_add(40) && *r > b.saturating_add(40))
            .count()
    }

    fn width(&self) -> usize {
        self.w
    }
}

#[test]
fn create_save_reopen_and_run_a_project() {
    if !gpu_enabled() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("my-project");
    let sample = sample();
    let (model, texture) = (
        sample.join("assets/models/crate.glb"),
        sample.join("assets/textures/bricks.png"),
    );
    let (shot_a, shot_b, shot_c) = (
        tmp.path().join("editor-a.png"),
        tmp.path().join("editor-b.png"),
        tmp.path().join("player.png"),
    );

    // 1. Create, edit, save.
    run_editor(
        &format!(
            r#"[
            NewProject(path: {project:?}),
            ImportFile(src: {model:?}, dest: "models/crate.glb"),
            ImportFile(src: {texture:?}, dest: "textures/bricks.png"),
            InstantiateModel(model: Path("assets/models/crate.glb"), name: "crate-instance", translation: (0.0, 0.5, 0.0)),
            CreateMaterial(path: "assets/materials/red.material"),
            EditMaterial(path: "assets/materials/red.material", material: (base_color: (0.9, 0.1, 0.1, 1.0), metallic: 0.0, roughness: 0.35)),
            SaveMaterial(path: "assets/materials/red.material"),
            AssignMaterial(entity: Name("crate"), material: Some(Path("assets/materials/red.material"))),
            SetTransform(entity: Name("crate-instance"), translation: (1.5, 0.0, 0.0), rotation_deg: (0.0, 30.0, 0.0), scale: (1.0, 1.0, 1.0)),
            CreateEntity(name: "marker", parent: Some(Name("crate-instance"))),
            Play,
            WaitFrames(3),
            Stop,
            AssertEntity(name: "marker", has_mesh: Some(false), parent: Some("crate-instance")),
            SaveScene,
            Screenshot(path: {shot_a:?}),
            Quit,
        ]"#
        ),
        tmp.path(),
        None,
    );
    // The saved files are what the second process and the player read.
    let scene_text = std::fs::read_to_string(project.join("assets/scenes/main.scene")).unwrap();
    let saved = SceneFile::from_ron(&scene_text).unwrap();
    let name_of = |n: &str| {
        saved
            .entities
            .iter()
            .find(|e| e.name.as_deref() == Some(n))
            .unwrap_or_else(|| panic!("{n} not saved"))
    };
    for n in [
        "Camera",
        "Sun",
        "Ground",
        "crate-instance",
        "crate",
        "marker",
    ] {
        name_of(n);
    }
    let material_meta: Meta = ron::from_str(
        &std::fs::read_to_string(project.join("assets/materials/red.material.meta")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        name_of("crate").mesh_renderer.unwrap().material,
        material_meta.id
    );
    assert_eq!(
        name_of("crate").parent,
        Some(name_of("crate-instance").uuid)
    );
    let red: MaterialAsset = ron::from_str(
        &std::fs::read_to_string(project.join("assets/materials/red.material")).unwrap(),
    )
    .unwrap();
    assert_eq!((red.base_color[0], red.roughness), (0.9, 0.35));
    let a = read_png(&shot_a);
    assert!(
        a.lit() > a.px.len() / 4 / 2,
        "editor screenshot is mostly black"
    );
    assert!(
        a.reddish() > 200,
        "no red crate in the editor window ({} px)",
        a.reddish()
    );

    // 2. Reopen in a second process and verify the entity set.
    run_editor(
        &format!(
            r#"[
            AssertEntityCount(at_least: 6),
            AssertEntity(name: "Camera"),
            AssertEntity(name: "Ground", has_mesh: Some(true)),
            AssertEntity(name: "crate-instance", has_mesh: Some(false), translation: Some((1.5, 0.0, 0.0))),
            AssertEntity(name: "crate", has_mesh: Some(true), parent: Some("crate-instance"), material: Some(Path("assets/materials/red.material"))),
            AssertEntity(name: "marker", parent: Some("crate-instance")),
            Screenshot(path: {shot_b:?}),
            Quit,
        ]"#
        ),
        tmp.path(),
        Some(&project),
    );
    assert!(
        read_png(&shot_b).reddish() > 200,
        "reopened project lost the material"
    );

    // 3. Run it in the player.
    let mut player = Command::new(player_exe());
    headless(&mut player)
        .env("ANVIL_EXIT_AFTER_FRAMES", "30")
        .env("ANVIL_SCREENSHOT", &shot_c)
        .arg(&project);
    run_checked("anvil-player", &mut player);
    let c = read_png(&shot_c);
    assert!(c.lit() > c.px.len() / 4 / 2, "player image is mostly black");
    assert!(
        c.reddish() > 200,
        "no red crate in the player image ({} px)",
        c.reddish()
    );
    assert!(c.width() >= 640);
}
