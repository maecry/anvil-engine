//! Windowing, input and surface integration.
//!
//! Specification: `docs/specs/layers/LAYER-002-platform.md`.

use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton as WMouse, WindowEvent};
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

impl WindowConfig {
    /// Applies env `ANVIL_WINDOW=WxH` (e.g. `1920x1080`) to the initial size; anything
    /// unparsable is ignored with a warning.
    pub fn with_env_size(mut self) -> Self {
        let Ok(v) = std::env::var("ANVIL_WINDOW") else {
            return self;
        };
        match parse_size(&v) {
            Some((w, h)) => (self.width, self.height) = (w, h),
            None => tracing::warn!(value = %v, "ANVIL_WINDOW must look like 1920x1080, ignored"),
        }
        self
    }
}

fn parse_size(v: &str) -> Option<(u32, u32)> {
    let (w, h) = v.split_once(['x', 'X'])?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// What the application wants the loop to do after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Keep running.
    Continue,
    /// Leave the loop; `on_exit` is called and `run` returns.
    Exit,
}

/// Engine keyboard keys: the keys the engine and examples use (physical positions; `Shift` and
/// `Ctrl` cover both sides).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    /// The Escape key.
    Escape,
    /// The A key.
    A,
    /// The B key.
    B,
    /// The C key.
    C,
    /// The D key.
    D,
    /// The E key.
    E,
    /// The F key.
    F,
    /// The G key.
    G,
    /// The H key.
    H,
    /// The I key.
    I,
    /// The J key.
    J,
    /// The K key.
    K,
    /// The L key.
    L,
    /// The M key.
    M,
    /// The N key.
    N,
    /// The O key.
    O,
    /// The P key.
    P,
    /// The Q key.
    Q,
    /// The R key.
    R,
    /// The S key.
    S,
    /// The T key.
    T,
    /// The U key.
    U,
    /// The V key.
    V,
    /// The W key.
    W,
    /// The X key.
    X,
    /// The Y key.
    Y,
    /// The Z key.
    Z,
    /// The 0 key (top row).
    Num0,
    /// The 1 key (top row).
    Num1,
    /// The 2 key (top row).
    Num2,
    /// The 3 key (top row).
    Num3,
    /// The 4 key (top row).
    Num4,
    /// The 5 key (top row).
    Num5,
    /// The 6 key (top row).
    Num6,
    /// The 7 key (top row).
    Num7,
    /// The 8 key (top row).
    Num8,
    /// The 9 key (top row).
    Num9,
    /// The ArrowUp key.
    ArrowUp,
    /// The ArrowDown key.
    ArrowDown,
    /// The ArrowLeft key.
    ArrowLeft,
    /// The ArrowRight key.
    ArrowRight,
    /// The Space key.
    Space,
    /// The Shift key.
    Shift,
    /// The Ctrl key.
    Ctrl,
    /// The Tab key.
    Tab,
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
    /// Raw (unaccelerated) mouse motion since the last event, in device units.
    MouseMotion {
        /// Horizontal motion.
        dx: f64,
        /// Vertical motion.
        dy: f64,
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
    /// Return [`Control::Exit`] to leave the loop cleanly (`on_exit` still runs).
    ///
    /// # Errors
    /// An error is logged and ends the loop; `run` returns it.
    fn on_frame(&mut self, window: &Window) -> Result<Control, AppError>;
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
        PhysicalKey::Code(WKey::KeyA) => KeyCode::A,
        PhysicalKey::Code(WKey::KeyB) => KeyCode::B,
        PhysicalKey::Code(WKey::KeyC) => KeyCode::C,
        PhysicalKey::Code(WKey::KeyD) => KeyCode::D,
        PhysicalKey::Code(WKey::KeyE) => KeyCode::E,
        PhysicalKey::Code(WKey::KeyF) => KeyCode::F,
        PhysicalKey::Code(WKey::KeyG) => KeyCode::G,
        PhysicalKey::Code(WKey::KeyH) => KeyCode::H,
        PhysicalKey::Code(WKey::KeyI) => KeyCode::I,
        PhysicalKey::Code(WKey::KeyJ) => KeyCode::J,
        PhysicalKey::Code(WKey::KeyK) => KeyCode::K,
        PhysicalKey::Code(WKey::KeyL) => KeyCode::L,
        PhysicalKey::Code(WKey::KeyM) => KeyCode::M,
        PhysicalKey::Code(WKey::KeyN) => KeyCode::N,
        PhysicalKey::Code(WKey::KeyO) => KeyCode::O,
        PhysicalKey::Code(WKey::KeyP) => KeyCode::P,
        PhysicalKey::Code(WKey::KeyQ) => KeyCode::Q,
        PhysicalKey::Code(WKey::KeyR) => KeyCode::R,
        PhysicalKey::Code(WKey::KeyS) => KeyCode::S,
        PhysicalKey::Code(WKey::KeyT) => KeyCode::T,
        PhysicalKey::Code(WKey::KeyU) => KeyCode::U,
        PhysicalKey::Code(WKey::KeyV) => KeyCode::V,
        PhysicalKey::Code(WKey::KeyW) => KeyCode::W,
        PhysicalKey::Code(WKey::KeyX) => KeyCode::X,
        PhysicalKey::Code(WKey::KeyY) => KeyCode::Y,
        PhysicalKey::Code(WKey::KeyZ) => KeyCode::Z,
        PhysicalKey::Code(WKey::Digit0) => KeyCode::Num0,
        PhysicalKey::Code(WKey::Digit1) => KeyCode::Num1,
        PhysicalKey::Code(WKey::Digit2) => KeyCode::Num2,
        PhysicalKey::Code(WKey::Digit3) => KeyCode::Num3,
        PhysicalKey::Code(WKey::Digit4) => KeyCode::Num4,
        PhysicalKey::Code(WKey::Digit5) => KeyCode::Num5,
        PhysicalKey::Code(WKey::Digit6) => KeyCode::Num6,
        PhysicalKey::Code(WKey::Digit7) => KeyCode::Num7,
        PhysicalKey::Code(WKey::Digit8) => KeyCode::Num8,
        PhysicalKey::Code(WKey::Digit9) => KeyCode::Num9,
        PhysicalKey::Code(WKey::ArrowUp) => KeyCode::ArrowUp,
        PhysicalKey::Code(WKey::ArrowDown) => KeyCode::ArrowDown,
        PhysicalKey::Code(WKey::ArrowLeft) => KeyCode::ArrowLeft,
        PhysicalKey::Code(WKey::ArrowRight) => KeyCode::ArrowRight,
        PhysicalKey::Code(WKey::Space) => KeyCode::Space,
        PhysicalKey::Code(WKey::ShiftLeft | WKey::ShiftRight) => KeyCode::Shift,
        PhysicalKey::Code(WKey::ControlLeft | WKey::ControlRight) => KeyCode::Ctrl,
        PhysicalKey::Code(WKey::Tab) => KeyCode::Tab,
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
                match self.app.on_frame(&window) {
                    Ok(Control::Continue) => {}
                    Ok(Control::Exit) => el.exit(),
                    Err(e) => self.fail(el, PlatformError::App(e)),
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

    fn device_event(&mut self, el: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let (Some(window), DeviceEvent::MouseMotion { delta: (dx, dy) }) =
            (self.window.clone(), event)
            && self.app.on_event(&window, &Event::MouseMotion { dx, dy }) == Control::Exit
        {
            el.exit();
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
    fn window_size_parsing() {
        assert_eq!(parse_size("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_size("0x5"), None);
        assert_eq!(parse_size("big"), None);
    }

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
        assert_eq!(map_key(PhysicalKey::Code(WKey::KeyA)), KeyCode::A);
        assert_eq!(map_key(PhysicalKey::Code(WKey::Digit7)), KeyCode::Num7);
        assert_eq!(map_key(PhysicalKey::Code(WKey::ShiftRight)), KeyCode::Shift);
        assert_eq!(map_key(PhysicalKey::Code(WKey::F5)), KeyCode::Other);
        assert_eq!(map_button(WMouse::Left), MouseButton::Left);
        assert_eq!(map_button(WMouse::Back), MouseButton::Other);
    }
}
