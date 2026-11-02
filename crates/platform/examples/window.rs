//! Opens a window, logs events, exits on close or Escape.

use platform::{App, AppError, Control, Event, KeyCode, Window, WindowConfig};

struct Demo;

impl App for Demo {
    fn on_start(&mut self, w: &Window) -> Result<(), AppError> {
        tracing::info!(size = ?w.inner_size(), scale = w.scale_factor(), "start");
        Ok(())
    }
    fn on_event(&mut self, _: &Window, e: &Event) -> Control {
        match e {
            Event::Resized { width, height } => tracing::info!(width, height, "resized"),
            Event::CloseRequested => {
                tracing::info!("close requested");
                return Control::Exit;
            }
            Event::Key {
                code: KeyCode::Escape,
                pressed: true,
            } => return Control::Exit,
            Event::Focused { focused } => tracing::info!(focused, "focus"),
            _ => {}
        }
        Control::Continue
    }
    fn on_frame(&mut self, _: &Window) -> Result<(), AppError> {
        Ok(())
    }
    fn on_exit(&mut self) {
        tracing::info!("exit");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    foundation::init_logging()?;
    platform::run(
        WindowConfig {
            title: "Anvil window".into(),
            ..Default::default()
        },
        Demo,
    )?;
    Ok(())
}
