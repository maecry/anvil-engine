//! `anvil-player <project-dir>`: loads a project and runs its startup scene.
//!
//! Env: `ANVIL_WINDOW=0` runs headless (offscreen), `ANVIL_WINDOW=WxH` sets the window size,
//! `ANVIL_EXIT_AFTER_FRAMES=n`, `ANVIL_SCREENSHOT=path.png` (with the frame count),
//! `ANVIL_VSYNC=0`, `ANVIL_HOT_RELOAD=1`. Exit code 1 when validation reported errors,
//! 3 when the GPU device was lost (`ANVIL_SIMULATE_DEVICE_LOST=<frame>` injects that).

use anyhow::{Context as _, bail};
use runtime::{Application, Context, RunOptions, Runtime, RuntimeError};

/// The player runs the scene as authored; there is no game logic.
struct Player;

impl Application for Player {
    fn start(&mut self, _: &mut Context) {}
    fn update(&mut self, _: &mut Context, _: f32) {}
}

fn main() -> anyhow::Result<()> {
    foundation::init_logging()?;
    let Some(dir) = std::env::args_os().nth(1) else {
        bail!("usage: anvil-player <project-dir>");
    };
    let report = match Runtime::run(RunOptions::from_env(dir), Player) {
        Ok(r) => r,
        Err(RuntimeError::DeviceLost) => {
            tracing::error!("GPU device lost, exiting with code 3");
            std::process::exit(3);
        }
        Err(e) => return Err(e).context("run project"),
    };
    tracing::info!(?report, "player finished");
    if report.validation_errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}
