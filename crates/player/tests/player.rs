//! `anvil-player` as a process (headless, gated by `ANVIL_GPU_TESTS=1`): device loss exits with
//! code 3 and logs the event with diagnostics (ADR-0015 §4), and a normal run logs no WARN
//! (the per-frame `texture_barrier from-state mismatch` warning is fixed, ADR-0015 §10).

use std::path::Path;
use std::process::{Command, Output};

fn run(frames: u32, simulate_lost: Option<u32>) -> Option<(Output, String)> {
    if std::env::var("ANVIL_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
        return None;
    }
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sample-project");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_anvil-player"));
    cmd.arg(project)
        .env("ANVIL_WINDOW", "0")
        .env("ANVIL_VSYNC", "0")
        .env("ANVIL_EXIT_AFTER_FRAMES", frames.to_string())
        .env("RUST_LOG", "info")
        .env_remove("ANVIL_SIMULATE_DEVICE_LOST");
    if let Some(f) = simulate_lost {
        cmd.env("ANVIL_SIMULATE_DEVICE_LOST", f.to_string());
    }
    let out = cmd.output().expect("spawn anvil-player");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some((out, log))
}

#[test]
fn simulated_device_loss_exits_with_code_3() {
    let Some((out, log)) = run(30, Some(5)) else {
        return;
    };
    assert_eq!(out.status.code(), Some(3), "{log}");
    assert!(log.contains("device lost"), "{log}");
    assert!(
        log.contains("pending_destroys"),
        "diagnostics in the event: {log}"
    );
}

#[test]
fn headless_run_logs_no_warnings() {
    let Some((out, log)) = run(10, None) else {
        return;
    };
    assert_eq!(out.status.code(), Some(0), "{log}");
    let warns: Vec<&str> = log.lines().filter(|l| l.contains(" WARN ")).collect();
    assert!(warns.is_empty(), "WARN lines over 10 frames: {warns:#?}");
    assert!(log.contains("shutdown diagnostics"), "{log}");
}
