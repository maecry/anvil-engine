# LAYER-002: Platform

| Field | Value |
|---|---|
| Status | Accepted |
| Milestone | M1 |
| Crates | platform |
| Traces to | MASTER-000 §5.1 (Layer 1), §5.2 rules 1 and 5, §8.2, §14 M1 |
| Last updated | 2026-11-02 |

## 1. Purpose

Own the OS window and event loop, and hand out raw window/display handles so a graphics backend can create a surface without the platform crate knowing about Vulkan.

## 2. Scope

| In | Out |
|---|---|
| Event loop and application lifecycle (winit 0.30 `ApplicationHandler`) | Surface / swapchain creation (LAYER-004) |
| Window creation, resize, close, scale factor, minimize | Gamepad, IME, clipboard (later) |
| Keyboard and mouse input state (minimal in M1) | Filesystem, threads (not needed in M1; add with a requirement) |
| `raw-window-handle` 0.6 exposure | Rendering, GPU types, `ash` |

## 3. Crates

platform (Layer 1). Depends on foundation, `winit` 0.30, `raw-window-handle` 0.6, `thiserror`, `tracing`. No `ash`.

## 4. Responsibilities

- Run the event loop on the main thread and drive an application trait.
- Create the window on `resumed` (winit 0.30 requires window creation from `ActiveEventLoop`).
- Translate winit events into a small engine event type (so the app does not import winit).
- Implement `HasWindowHandle` + `HasDisplayHandle` on the window type.
- Report current inner size in physical pixels and scale factor; report minimized as size 0x0.
- Request redraws; support a continuous-redraw mode for M1.

## 5. Non-responsibilities

- Knowing what a swapchain is; recreating it (the app/renderer reacts to `Resized`, the backend also detects out-of-date).
- Frame pacing policy beyond requesting redraws.
- Input action mapping (later, runtime layer).

## 6. Dependencies

| Allowed | Forbidden |
|---|---|
| foundation, winit, raw-window-handle, thiserror, tracing | `ash`, `ash-window`, rhi*, renderer, editor and any layer 2+ crate |

Rule 1: platform must not depend on the editor.

## 7. Key concepts and data flow

```
 main()  -> platform::run(config, app)
              |
   winit EventLoop (main thread) --ApplicationHandler--> PlatformHandler<A>
              |   resumed        -> create Window (Arc<winit Window>) -> app.on_start(&Window)
              |   WindowEvent    -> map to Event -> app.on_event(&Window, Event)
              |   about_to_wait  -> window.request_redraw() (continuous mode)
              |   RedrawRequested-> app.on_frame(&Window)
              |   CloseRequested -> app.on_event(CloseRequested); app decides via ControlFlow
              v
 app (example/triangle) owns Renderer/Device; passes Arc<Window> to Device::create_swapchain
```

The application owns GPU objects; the platform crate calls into the app, never the reverse.

## 8. Public interface sketch

```rust
pub struct WindowConfig { pub title: String, pub width: u32, pub height: u32, pub resizable: bool }

pub trait App: 'static {
    fn on_start(&mut self, window: &Window) -> Result<(), AppError>;   // GPU init happens here
    fn on_event(&mut self, window: &Window, event: &Event) -> Control;
    fn on_frame(&mut self, window: &Window) -> Result<Control, AppError>; // Exit leaves the loop cleanly
    fn on_exit(&mut self);                                              // drop GPU objects before the window
}

pub enum Control { Continue, Exit }
pub enum Event {
    Resized { width: u32, height: u32 },       // physical pixels
    ScaleFactorChanged { scale: f64 },
    CloseRequested,
    Key { code: KeyCode, pressed: bool },      // KeyCode: engine enum; M1 may expose only a few keys (Escape)
    MouseButton { button: MouseButton, pressed: bool },
    CursorMoved { x: f64, y: f64 },
}

#[derive(Clone)]
pub struct Window { /* Arc<winit::window::Window> */ }
impl Window {
    pub fn inner_size(&self) -> (u32, u32);    // physical; (0,_) or (_,0) when minimized
    pub fn scale_factor(&self) -> f64;
    pub fn request_redraw(&self);
}
impl raw_window_handle::HasWindowHandle for Window { /* ... */ }
impl raw_window_handle::HasDisplayHandle for Window { /* ... */ }
// Window: Send + Sync (required by RHI swapchain, which stores Arc<W>).

pub fn run<A: App>(config: WindowConfig, app: A) -> Result<(), PlatformError>;
```

Notes:
- `run` owns the `EventLoop` and creates `A` already constructed by the caller; GPU init is deferred to `on_start` because the window does not exist earlier.
- winit 0.30.13 `Window: Send + Sync + HasWindowHandle + HasDisplayHandle + 'static` with default features (verified 2026-11-02), by compile check.
- `EventLoop::run_app(&mut A: ApplicationHandler)`, `ActiveEventLoop::create_window(WindowAttributes)`, `ControlFlow::Poll`, `WindowEvent::KeyboardInput { device_id, event: KeyEvent, is_synthetic }` (verified 2026-11-02).
- Wayland: no extra handling expected beyond display handle; verify with a Wayland session and an X11 session.

## 9. Ownership and lifetime

- `Window` is `Arc`-shared. The RHI swapchain stores an `Arc<Window>` clone so the native window outlives the `VkSurfaceKHR` regardless of drop order.
- The app must still destroy GPU objects in `on_exit` (before the event loop drops), because device teardown ordering is the app's responsibility.
- `App` is owned by the platform handler for the duration of `run`.

## 10. Threading model

Event loop and all callbacks run on the main thread (platform requirement; macOS mandates it). M1 renders on the same thread inside `on_frame`. A dedicated render thread is a later decision (Open question 2); it would require the swapchain-owning thread to receive resize events through a channel.

## 11. Error handling

`PlatformError` (thiserror): `EventLoop(String)`, `WindowCreation(String)`. `AppError` is an opaque boxed error from the app (`Box<dyn Error + Send + Sync>`); returning `Err` from `on_start`/`on_frame` logs it and exits the loop with a non-zero result from `run`. No panics on user-visible failures.

## 12. Diagnostics and observability

`tracing` spans: `platform_init`, per-frame span is created by the app/renderer, not here. Log at `info`: window created (size, scale), `debug`: each resize, `warn`: event loop errors. Optional env `ANVIL_LOG` handled by foundation.

## 13. Testing strategy

- Unit: winit-to-engine event mapping as pure functions (key mapping, size mapping with zero size).
- Manual M1 checklist: resize, minimize/restore, close via button, Escape exits, move between HiDPI monitors (scale change).
- No automated window tests in CI (headless runners); an `xvfb` smoke job is an Open question.

## 14. Acceptance criteria

- [ ] Window opens on Linux (X11 and Wayland), titled and resizable.
- [ ] `Event::Resized` fires with physical size; minimize yields a zero-sized inner size.
- [ ] Closing the window calls `on_exit` and `run` returns `Ok(())`.
- [ ] `Window` satisfies `HasWindowHandle + HasDisplayHandle + Send + Sync` (compile test).
- [ ] `cargo tree -p platform` shows no `ash`.

## 15. Open questions

1. Continuous redraw (`Poll`) vs redraw on request: default continuous for M1; battery/CPU use acceptable?
2. Render thread separate from the main thread: M1 says no. Decide at M5.
3. Input representation: own `KeyCode` enum now or re-export winit's? Default: own minimal enum (keeps the winit dependency private).
4. Do we put `Window` + `App` in platform and the `run` helper in the same crate, or let the editor supply its own loop? Default: same crate.
5. xvfb or Wayland-headless smoke test in CI.

## 16. Related modules

None dedicated. Surface creation is MODULE-003-vulkan-swapchain (LAYER-004).
