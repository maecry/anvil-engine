//! Windowing, input and surface integration.
//!
//! Specification: `docs/specs/layers/LAYER-002-platform.md`.

use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton as WMouse, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode as WKey, PhysicalKey};
use winit::window::WindowId;

/// Opaque error returned by application callbacks.
pub type AppError = Box<dyn std::error::Error + Send + Sync>;

/// Errors from [`run`].
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The OS event loop could not be created or failed while running.
    #[error("event loop error: {0}")]
    EventLoop(String),
    /// The window could not be created.
    #[error("window creation failed: {0}")]
    WindowCreation(String),
    /// `on_start` or `on_frame` returned an error.
    #[error("application error: {0}")]
    App(AppError),
}

/// Initial window properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowConfig {
    /// Window title.
    pub title: String,
    /// Initial inner width in logical pixels.
    pub width: u32,
    /// Initial inner height in logical pixels.
    pub height: u32,
    /// Whether the user can resize the window.
    pub resizable: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Anvil Engine".into(),
            width: 1280,
            height: 720,
            resizable: true,
        }
    }
}

/// What the application wants the loop to do after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Keep running.
    Continue,
    /// Leave the loop; `on_exit` is called and `run` returns.
    Exit,
}

/// Engine keyboard keys (M1: only the keys the engine uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    /// The Escape key.
    Escape,
    /// Any key not yet mapped.
    Other,
}

/// Engine mouse buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Wheel button.
    Middle,
    /// Any other button.
    Other,
}

/// Platform events delivered to the application.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Inner size changed, in physical pixels. `0` in either axis means minimised.
    Resized {
        /// Width in physical pixels.
        width: u32,
        /// Height in physical pixels.
        height: u32,
    },
    /// DPI scale factor changed.
    ScaleFactorChanged {
        /// New scale factor.
        scale: f64,
    },
    /// The user asked to close the window.
    CloseRequested,
    /// Window gained or lost keyboard focus.
    Focused {
        /// True when focused.
        focused: bool,
    },
    /// Keyboard key press or release.
    Key {
        /// Key.
        code: KeyCode,
        /// True on press.
        pressed: bool,
    },
    /// Mouse button press or release.
    MouseButton {
        /// Button.
        button: MouseButton,
        /// True on press.
        pressed: bool,
    },
    /// Cursor moved, in physical pixels.
    CursorMoved {
        /// X position.
        x: f64,
        /// Y position.
        y: f64,
    },
}

/// Application driven by [`run`]. All callbacks run on the main thread.
pub trait App: 'static {
    /// Called once the window exists; create GPU objects here.
    ///
    /// # Errors
    /// An error is logged and ends the loop; `run` returns it.
    fn on_start(&mut self, window: &Window) -> Result<(), AppError>;
    /// Called for each translated event.
    fn on_event(&mut self, window: &Window, event: &Event) -> Control;
    /// Called once per redraw (continuous mode). Check [`Window::is_minimized`].
    ///
    /// # Errors
    /// An error is logged and ends the loop; `run` returns it.
    fn on_frame(&mut self, window: &Window) -> Result<(), AppError>;
    /// Called once before the window is dropped; destroy GPU objects here.
    fn on_exit(&mut self);
}

/// Cheaply clonable, thread-safe handle to the OS window.
#[derive(Clone)]
pub struct Window(Arc<winit::window::Window>);

impl Window {
    /// Inner size in physical pixels; `0` in an axis when minimised.
    #[must_use]
    pub fn inner_size(&self) -> (u32, u32) {
        let s = self.0.inner_size();
        (s.width, s.height)
    }
    /// Current DPI scale factor.
    #[must_use]
    pub fn scale_factor(&self) -> f64 {
        self.0.scale_factor()
    }
    /// True when the inner size is zero in either axis.
    #[must_use]
    pub fn is_minimized(&self) -> bool {
        is_zero_size(self.inner_size())
    }
    /// Ask for another `on_frame`.
    pub fn request_redraw(&self) {
        self.0.request_redraw();
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

fn is_zero_size((w, h): (u32, u32)) -> bool {
    w == 0 || h == 0
}

fn map_key(key: PhysicalKey) -> KeyCode {
    match key {
        PhysicalKey::Code(WKey::Escape) => KeyCode::Escape,
        _ => KeyCode::Other,
    }
}

fn map_button(b: WMouse) -> MouseButton {
    match b {
        WMouse::Left => MouseButton::Left,
        WMouse::Right => MouseButton::Right,
        WMouse::Middle => MouseButton::Middle,
        _ => MouseButton::Other,
    }
}

/// Translate a winit event; `None` for events the engine ignores.
fn map_event(e: &WindowEvent) -> Option<Event> {
    Some(match e {
        WindowEvent::Resized(PhysicalSize { width, height }) => Event::Resized {
            width: *width,
            height: *height,
        },
        WindowEvent::ScaleFactorChanged { scale_factor, .. } => Event::ScaleFactorChanged {
            scale: *scale_factor,
        },
        WindowEvent::CloseRequested => Event::CloseRequested,
        WindowEvent::Focused(focused) => Event::Focused { focused: *focused },
        WindowEvent::KeyboardInput { event, .. } => Event::Key {
            code: map_key(event.physical_key),
            pressed: event.state == ElementState::Pressed,
        },
        WindowEvent::MouseInput { state, button, .. } => Event::MouseButton {
            button: map_button(*button),
            pressed: *state == ElementState::Pressed,
        },
        WindowEvent::CursorMoved { position, .. } => Event::CursorMoved {
            x: position.x,
            y: position.y,
        },
        _ => return None,
    })
}

struct Handler<A: App> {
    config: WindowConfig,
    app: A,
    window: Option<Window>,
    started: bool,
    error: Option<PlatformError>,
}

impl<A: App> Handler<A> {
    fn fail(&mut self, el: &ActiveEventLoop, e: PlatformError) {
        tracing::error!("{e}");
        self.error.get_or_insert(e);
        el.exit();
    }
}

impl<A: App> ApplicationHandler for Handler<A> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = winit::window::Window::default_attributes()
            .with_title(self.config.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.config.width,
                self.config.height,
            ))
            .with_resizable(self.config.resizable);
        let window = match el.create_window(attrs) {
            Ok(w) => Window(Arc::new(w)),
            Err(e) => return self.fail(el, PlatformError::WindowCreation(e.to_string())),
        };
        tracing::info!(size = ?window.inner_size(), scale = window.scale_factor(), "window created");
        self.window = Some(window.clone());
        match self.app.on_start(&window) {
            Ok(()) => self.started = true,
            Err(e) => self.fail(el, PlatformError::App(e)),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if matches!(event, WindowEvent::RedrawRequested) {
            if self.started {
                if let Err(e) = self.app.on_frame(&window) {
                    self.fail(el, PlatformError::App(e));
                }
            }
        } else if let Some(ev) = map_event(&event) {
            if let Event::Resized { width, height } = ev {
                tracing::debug!(width, height, "resized");
            }
            if self.app.on_event(&window, &ev) == Control::Exit {
                el.exit();
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        if self.window.is_some() {
            self.app.on_exit();
        }
        self.window = None;
    }
}

/// Create the window and run `app` until it asks to exit.
///
/// # Errors
/// [`PlatformError`] if the event loop or window fails, or the app returns an error.
pub fn run<A: App>(config: WindowConfig, app: A) -> Result<(), PlatformError> {
    let _span = tracing::info_span!("platform_init").entered();
    let el = EventLoop::new().map_err(|e| PlatformError::EventLoop(e.to_string()))?;
    el.set_control_flow(ControlFlow::Poll);
    let mut h = Handler {
        config,
        app,
        window: None,
        started: false,
        error: None,
    };
    drop(_span);
    el.run_app(&mut h).map_err(|e| {
        tracing::warn!("event loop error: {e}");
        PlatformError::EventLoop(e.to_string())
    })?;
    h.error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _assert_bounds<T: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static>() {}
    const _: fn() = _assert_bounds::<Window>;

    #[test]
    fn config_defaults() {
        let c = WindowConfig::default();
        assert_eq!((c.width, c.height, c.resizable), (1280, 720, true));
        assert!(!c.title.is_empty());
    }

    #[test]
    fn zero_size_is_minimized() {
        assert!(is_zero_size((0, 600)));
        assert!(is_zero_size((800, 0)));
        assert!(!is_zero_size((800, 600)));
    }

    #[test]
    fn resize_maps_including_zero() {
        assert_eq!(
            map_event(&WindowEvent::Resized(PhysicalSize::new(0, 0))),
            Some(Event::Resized {
                width: 0,
                height: 0
            })
        );
        assert_eq!(
            map_event(&WindowEvent::Resized(PhysicalSize::new(640, 480))),
            Some(Event::Resized {
                width: 640,
                height: 480
            })
        );
    }

    #[test]
    fn close_focus_and_ignored() {
        assert_eq!(
            map_event(&WindowEvent::CloseRequested),
            Some(Event::CloseRequested)
        );
        assert_eq!(
            map_event(&WindowEvent::Focused(false)),
            Some(Event::Focused { focused: false })
        );
        assert_eq!(map_event(&WindowEvent::RedrawRequested), None);
    }

    #[test]
    fn key_and_button_mapping() {
        assert_eq!(map_key(PhysicalKey::Code(WKey::Escape)), KeyCode::Escape);
        assert_eq!(map_key(PhysicalKey::Code(WKey::KeyA)), KeyCode::Other);
        assert_eq!(map_button(WMouse::Left), MouseButton::Left);
        assert_eq!(map_button(WMouse::Back), MouseButton::Other);
    }
}
