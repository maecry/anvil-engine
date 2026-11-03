//! The sample project's startup scene through the whole runtime (headless, 256x256), compared
//! with `crates/renderer/tests/golden/sample_project.png` (ADR-0015 §1). Gated by
//! `ANVIL_GPU_TESTS=1`; `ANVIL_BLESS=1` rewrites the reference.

use std::path::Path;

use runtime::{Application, Context, RunOptions, Runtime};

#[path = "../../renderer/tests/golden/compare.rs"]
mod compare;

struct Idle;

impl Application for Idle {
    fn start(&mut self, _: &mut Context) {}
    fn update(&mut self, _: &mut Context, _: f32) {}
}

#[test]
fn sample_project() {
    if std::env::var("ANVIL_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
        return;
    }
    let _ = foundation::init_logging();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sample-project");
    let tmp = tempfile::tempdir().unwrap();
    let shot = tmp.path().join("shot.png");
    let mut opts = RunOptions::from_env(&dir);
    (opts.headless, opts.hot_reload, opts.vsync) = (true, false, false);
    (opts.window.width, opts.window.height) = (256, 256);
    opts.exit_after_frames = Some(8);
    opts.screenshot = Some(shot.clone());
    let report = Runtime::run(opts, Idle).unwrap();
    assert_eq!(
        (report.validation_errors, report.validation_warnings),
        (0, 0)
    );
    let mut r = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&shot).unwrap()))
        .read_info()
        .unwrap();
    let mut rgba = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut rgba).unwrap();
    rgba.truncate(info.buffer_size());
    compare::check(
        "sample_project",
        (info.width, info.height),
        &rgba,
        compare::DEFAULT,
    );
}
