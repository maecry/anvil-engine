//! `anvil-editor [project-dir]`
//!
//! Env: `ANVIL_WINDOW=0` runs headless (offscreen, 1600x900), `ANVIL_WINDOW=WxH` sets the window
//! size, `ANVIL_EDITOR_SCRIPT=file.ron` runs an `EditorCommand` script, `ANVIL_EXIT_AFTER_FRAMES=n`,
//! `ANVIL_VSYNC=0`. Exit code 1 on a failed script command or validation errors.

use anyhow::Context as _;
use editor::EditorOptions;

fn main() -> anyhow::Result<()> {
    foundation::init_logging()?;
    let dir = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let report = editor::run(EditorOptions::from_env(dir)).context("run editor")?;
    tracing::info!(?report, "editor finished");
    if report.validation_errors > 0 || report.command_errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}
