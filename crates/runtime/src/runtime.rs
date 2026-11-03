//! The frame loop: windowed through `platform`, or headless against an offscreen target.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assets::AssetServer;
use platform::{App, AppError, Control, Event, KeyCode, MouseButton, Window, WindowConfig};
use rhi::{
    Acquire, Backend, BufferDesc, BufferUsage, CommandEncoder, Device, DeviceDesc, Format,
    HasDisplayHandle, Instance, InstanceDesc, Memory, PresentStatus, SwapchainDesc,
    SwapchainHandle, SwapchainImage, TextureDesc, TextureState, TextureUsage,
};
use rhi_vulkan::{Vulkan, VulkanDevice, VulkanInstance, VulkanSurface};
use scene::World;

use crate::{Project, RuntimeCore, RuntimeError};

/// Keys and mouse state for [`Application::update`].
#[derive(Clone, Debug, Default)]
pub struct Input {
    keys: Vec<KeyCode>,
    buttons: Vec<MouseButton>,
    mouse_delta: (f64, f64),
}

impl Input {
    /// Is `key` held?
    pub fn key_down(&self, key: KeyCode) -> bool {
        self.keys.contains(&key)
    }

    /// Is the mouse button held?
    pub fn button_down(&self, button: MouseButton) -> bool {
        self.buttons.contains(&button)
    }

    /// Raw mouse motion accumulated since the previous frame.
    pub fn mouse_delta(&self) -> (f64, f64) {
        self.mouse_delta
    }

    fn event(&mut self, e: &Event) {
        match e {
            Event::Key { code, pressed } => {
                self.keys.retain(|k| k != code);
                if *pressed {
                    self.keys.push(*code);
                }
            }
            Event::MouseButton { button, pressed } => {
                self.buttons.retain(|b| b != button);
                if *pressed {
                    self.buttons.push(*button);
                }
            }
            Event::MouseMotion { dx, dy } => {
                self.mouse_delta.0 += dx;
                self.mouse_delta.1 += dy;
            }
            _ => {}
        }
    }
}

/// Frame timing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Time {
    /// Seconds since the previous frame (clamped to 0.1).
    pub dt: f32,
    /// Seconds since start.
    pub elapsed: f32,
    /// Frames rendered so far.
    pub frame: u64,
}

/// What an [`Application`] sees.
pub struct Context<'a> {
    /// The entities.
    pub world: &'a mut World,
    /// The asset server (load assets, query states).
    pub assets: &'a mut AssetServer,
    /// Input state.
    pub input: &'a Input,
    /// Timing.
    pub time: Time,
    /// Set to true to end the run after this frame.
    pub quit: bool,
}

/// Game logic. Both methods run on the main thread.
pub trait Application: 'static {
    /// Called once after the renderer exists, before the startup scene is requested.
    fn start(&mut self, ctx: &mut Context);
    /// Called every frame after `assets.update` and before the transform systems.
    fn update(&mut self, ctx: &mut Context, dt: f32);
}

/// How to run a project. [`RunOptions::from_env`] reads the `ANVIL_*` conventions of the
/// examples.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Project directory (contains `project.anvil`).
    pub project_dir: PathBuf,
    /// No window: render into an offscreen target.
    pub headless: bool,
    /// Window title and size (headless uses the size).
    pub window: WindowConfig,
    /// Present with vsync (`ANVIL_VSYNC=0` turns it off).
    pub vsync: bool,
    /// Stop after this many frames (headless defaults to 60).
    pub exit_after_frames: Option<u64>,
    /// Write the last frame here as PNG (needs `exit_after_frames`).
    pub screenshot: Option<PathBuf>,
    /// Watch `assets/` and reload changed files.
    pub hot_reload: bool,
}

impl RunOptions {
    /// Options from env: `ANVIL_WINDOW=0` (headless) or `WxH`, `ANVIL_VSYNC`,
    /// `ANVIL_EXIT_AFTER_FRAMES`, `ANVIL_SCREENSHOT`, `ANVIL_HOT_RELOAD=1`.
    pub fn from_env(project_dir: impl Into<PathBuf>) -> Self {
        let headless = std::env::var("ANVIL_WINDOW").as_deref() == Ok("0");
        let mut window = WindowConfig::default();
        if headless {
            (window.width, window.height) = (1280, 720);
        } else {
            window = window.with_env_size();
        }
        let exit_after_frames = std::env::var("ANVIL_EXIT_AFTER_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok());
        let screenshot = std::env::var_os("ANVIL_SCREENSHOT").map(PathBuf::from);
        if screenshot.is_some() && exit_after_frames.is_none() {
            tracing::warn!("ANVIL_SCREENSHOT needs ANVIL_EXIT_AFTER_FRAMES, ignoring");
        }
        Self {
            project_dir: project_dir.into(),
            headless,
            window,
            vsync: std::env::var("ANVIL_VSYNC").as_deref() != Ok("0"),
            exit_after_frames,
            screenshot,
            hot_reload: std::env::var("ANVIL_HOT_RELOAD").as_deref() == Ok("1"),
        }
    }
}

/// Outcome of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    /// Frames rendered.
    pub frames: u64,
    /// Validation errors seen by the Vulkan instance (the goal is zero).
    pub validation_errors: u64,
    /// Validation warnings.
    pub validation_warnings: u64,
}

struct Gfx {
    instance: VulkanInstance,
    surface: Option<VulkanSurface>,
    device: VulkanDevice,
    swapchain: Option<SwapchainHandle>,
    /// Headless target.
    offscreen: Option<SwapchainImage>,
}

/// Composition root: owns the window loop, device, swapchain, renderer, world and assets.
pub struct Runtime<A: Application> {
    opts: RunOptions,
    app: A,
    gfx: Option<Gfx>,
    core: Option<RuntimeCore>,
    input: Input,
    started: Instant,
    last: Instant,
    rendered: u64,
    report: Arc<Mutex<RunReport>>,
    /// Set when a frame hit `DeviceLost` (also across the windowed loop, which erases errors).
    lost: Arc<AtomicBool>,
}

impl<A: Application> Runtime<A> {
    /// Loads the project and runs `app` until it quits, the window closes or
    /// `exit_after_frames` is reached.
    ///
    /// # Errors
    /// Project, device, renderer or window failures.
    pub fn run(opts: RunOptions, app: A) -> Result<RunReport, RuntimeError> {
        let report = Arc::new(Mutex::new(RunReport::default()));
        let lost = Arc::new(AtomicBool::new(false));
        let mut rt = Runtime {
            opts,
            app,
            gfx: None,
            core: None,
            input: Input::default(),
            started: Instant::now(),
            last: Instant::now(),
            rendered: 0,
            report: report.clone(),
            lost: lost.clone(),
        };
        if rt.opts.headless {
            rt.opts.exit_after_frames.get_or_insert(60);
            rt.init(None)?;
            let (w, h) = (rt.opts.window.width, rt.opts.window.height);
            tracing::info!(w, h, "headless run");
            let result = (|| -> Result<(), RuntimeError> {
                if let (Some(gfx), Some(core)) = (&rt.gfx, rt.core.as_mut()) {
                    core.wait_for_assets(&gfx.device, Duration::from_secs(30));
                }
                while !rt.frame(None)? {}
                Ok(())
            })();
            rt.teardown();
            result?;
        } else {
            let config = rt.opts.window.clone();
            let ran = platform::run(config, rt);
            if lost.load(Ordering::Acquire) {
                return Err(RuntimeError::DeviceLost);
            }
            ran?;
        }
        let r = *report
            .lock()
            .map_err(|_| RuntimeError::Io("report lock".into()))?;
        Ok(r)
    }

    /// Creates the device, renderer and asset server (and the swapchain given a window), calls
    /// `Application::start` and requests the startup scene.
    fn init(&mut self, window: Option<&Window>) -> Result<(), RuntimeError> {
        let project = Project::load(&self.opts.project_dir)?;
        let mut assets = AssetServer::new(&self.opts.project_dir)?;
        if self.opts.hot_reload {
            assets.enable_hot_reload()?;
        }
        let display = match window {
            Some(w) => Some(
                w.display_handle()
                    .map_err(|e| RuntimeError::Io(e.to_string()))?
                    .as_raw(),
            ),
            None => None,
        };
        let instance = Vulkan::create_instance(&InstanceDesc {
            app_name: project.name.clone(),
            display,
            ..InstanceDesc::default()
        })?;
        let surface = match window {
            Some(w) => Some(instance.create_surface(Arc::new(w.clone()))?),
            None => None,
        };
        let device = instance.create_device(&DeviceDesc::default(), surface.as_ref())?;
        let (swapchain, offscreen, format, extent) = match (&surface, window) {
            (Some(surface), Some(w)) => {
                let (width, height) = (w.inner_size().0.max(1), w.inner_size().1.max(1));
                let sc = device.create_swapchain(
                    surface,
                    &SwapchainDesc {
                        width,
                        height,
                        vsync: self.opts.vsync,
                        preferred_format: Format::Bgra8Srgb,
                    },
                )?;
                (
                    Some(sc),
                    None,
                    device.swapchain_format(sc)?,
                    (width, height),
                )
            }
            _ => {
                let (width, height) = (self.opts.window.width, self.opts.window.height);
                let format = Format::Rgba8Srgb;
                let texture = device.create_texture(&TextureDesc {
                    label: Some("runtime.offscreen"),
                    width,
                    height,
                    format,
                    usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::TRANSFER_SRC,
                    mip_levels: 1,
                    array_layers: 1,
                    cube: false,
                })?;
                let image = SwapchainImage {
                    swapchain: SwapchainHandle::NULL,
                    texture,
                    index: 0,
                    extent: (width, height),
                    format,
                };
                (None, Some(image), format, (width, height))
            }
        };
        let mut core = RuntimeCore::new(&device, format, extent, assets)?;
        let mut ctx = Context {
            world: &mut core.world,
            assets: &mut core.assets,
            input: &self.input,
            time: Time::default(),
            quit: false,
        };
        self.app.start(&mut ctx);
        if let Err(e) = core.open_scene(&project.startup_scene) {
            tracing::error!(error = %e, scene = %project.startup_scene, "startup scene not found");
        }
        self.gfx = Some(Gfx {
            instance,
            surface,
            device,
            swapchain,
            offscreen,
        });
        self.core = Some(core);
        self.started = Instant::now();
        self.last = Instant::now();
        Ok(())
    }

    /// One frame; a lost device is logged with the diagnostics snapshot (ADR-0015 §4).
    fn frame(&mut self, window: Option<&Window>) -> Result<bool, RuntimeError> {
        let r = self.frame_inner(window);
        if matches!(r, Err(RuntimeError::DeviceLost)) {
            self.lost.store(true, Ordering::Release);
            let diag = self.gfx.as_ref().map(|g| g.device.diagnostics());
            tracing::error!(frame = self.rendered, ?diag, "device lost");
        }
        r
    }

    /// One frame in the fixed order. Returns true when the run should end.
    fn frame_inner(&mut self, window: Option<&Window>) -> Result<bool, RuntimeError> {
        let (Some(gfx), Some(core)) = (self.gfx.as_mut(), self.core.as_mut()) else {
            return Ok(true);
        };
        let _span = tracing::info_span!("runtime_frame", frame = self.rendered).entered();
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;

        // 1. input was collected by on_event; 2. assets
        core.update_assets(&gfx.device);
        // 3. application
        let mut ctx = Context {
            world: &mut core.world,
            assets: &mut core.assets,
            input: &self.input,
            time: Time {
                dt,
                elapsed: (now - self.started).as_secs_f32(),
                frame: self.rendered,
            },
            quit: false,
        };
        self.app.update(&mut ctx, dt);
        let quit = ctx.quit;
        self.input.mouse_delta = (0.0, 0.0);
        // 4. systems, 5. extract
        core.run_systems();
        let scene = core.extract(&gfx.device);

        // 6. render, 7. present
        let last_frame = self
            .opts
            .exit_after_frames
            .is_some_and(|n| self.rendered + 1 >= n);
        let screenshot = self.opts.screenshot.clone().filter(|_| last_frame);
        let device = &mut gfx.device;
        let frame = device.begin_frame()?;
        let image = match (gfx.swapchain, gfx.offscreen.clone()) {
            (Some(sc), _) => match device.acquire(sc)? {
                Acquire::Image(i) => i,
                Acquire::OutOfDate => {
                    if let Some(w) = window {
                        let (w, h) = w.inner_size();
                        device.resize_swapchain(sc, w, h)?;
                    }
                    return Ok(quit);
                }
                Acquire::Minimized => return Ok(quit),
            },
            (None, Some(img)) => img,
            (None, None) => return Ok(true),
        };
        let readback = match &screenshot {
            Some(_) => Some(device.create_buffer(&BufferDesc {
                label: Some("screenshot.readback"),
                size: u64::from(image.extent.0) * u64::from(image.extent.1) * 4,
                usage: BufferUsage::TRANSFER_DST,
                memory: Memory::Readback,
            })?),
            None => None,
        };
        let mut cmd = device.begin_commands(&frame)?;
        // The graph leaves the image in `TransferSrc` for a readback, otherwise in `Present`.
        // (An offscreen target is only ever read back.)
        let final_state = if readback.is_some() || gfx.swapchain.is_none() {
            TextureState::TransferSrc
        } else {
            TextureState::Present
        };
        core.renderer
            .render(device, &mut cmd, &frame, &scene, &image, final_state)?;
        if let Some(rb) = readback {
            cmd.copy_texture_to_buffer(image.texture, rb, 0);
            if gfx.swapchain.is_some() {
                cmd.texture_barrier(
                    image.texture,
                    TextureState::TransferSrc,
                    TextureState::Present,
                );
            }
        }
        let finished = cmd.finish()?;
        let (extent, format) = (image.extent, image.format);
        let mut status = PresentStatus::Ok;
        if let Some(sc) = gfx.swapchain {
            device.submit(finished, &image)?;
            status = device.present(sc, image)?;
        } else {
            device.submit_unwindowed(finished)?;
            // One shared target and readback buffer, so frames must not overlap.
            device.wait_idle()?;
        }
        if let (Some(path), Some(rb)) = (screenshot, readback) {
            device.wait_idle()?;
            let mut pixels = vec![0u8; extent.0 as usize * extent.1 as usize * 4];
            device.read_buffer(rb, 0, &mut pixels)?;
            device.destroy_buffer(rb);
            write_png(&path, extent, format, pixels)?;
            tracing::info!(path = %path.display(), "screenshot written");
        }
        if status == PresentStatus::Recreate
            && let (Some(sc), Some(w)) = (gfx.swapchain, window)
        {
            let (w, h) = w.inner_size();
            device.resize_swapchain(sc, w, h)?;
        }
        self.rendered += 1;
        Ok(quit
            || self
                .opts
                .exit_after_frames
                .is_some_and(|n| self.rendered >= n))
    }

    fn teardown(&mut self) {
        let (Some(mut gfx), Some(core)) = (self.gfx.take(), self.core.take()) else {
            return;
        };
        // A lost device cannot be waited on (the wait may hang or fail); drop the rest as is.
        if !self.lost.load(Ordering::Acquire)
            && let Err(e) = gfx.device.wait_idle()
        {
            tracing::error!(error = %e, "wait_idle failed");
        }
        let stats = core.assets.stats();
        tracing::info!(?stats, "asset stats at exit");
        core.destroy(&gfx.device);
        if let Some(sc) = gfx.swapchain {
            gfx.device.destroy_swapchain(sc);
        }
        if let Some(img) = gfx.offscreen {
            gfx.device.destroy_texture(img.texture);
        }
        if !self.lost.load(Ordering::Acquire)
            && let Err(e) = gfx.device.retire_pending()
        {
            tracing::error!(error = %e, "retiring deferred destroys failed");
        }
        let d = gfx.device.diagnostics();
        tracing::info!(
            live = ?d.live,
            pending_destroys = d.pending_destroys,
            allocated_bytes = d.memory.allocated_bytes,
            "shutdown diagnostics"
        );
        let Gfx {
            instance,
            surface,
            device,
            ..
        } = gfx;
        drop(device);
        drop(surface);
        let counts = instance.validation_counts();
        tracing::info!(?counts, "final validation counts");
        if let Ok(mut r) = self.report.lock() {
            *r = RunReport {
                frames: self.rendered,
                validation_errors: counts.errors,
                validation_warnings: counts.warnings,
            };
        }
    }
}

/// Writes swapchain texels (BGRA or RGBA, 8 bit) as an RGBA PNG.
///
/// # Errors
/// [`RuntimeError::Io`] when the file cannot be written.
pub fn write_png(
    path: &std::path::Path,
    (w, h): (u32, u32),
    format: Format,
    mut pixels: Vec<u8>,
) -> Result<(), RuntimeError> {
    let io = |e: &dyn std::fmt::Display| RuntimeError::Io(format!("{}: {e}", path.display()));
    if matches!(format, Format::Bgra8Srgb | Format::Bgra8Unorm) {
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
    let file = std::fs::File::create(path).map_err(|e| io(&e))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&pixels))
        .map_err(|e| io(&e))
}

impl<A: Application> App for Runtime<A> {
    fn on_start(&mut self, window: &Window) -> Result<(), AppError> {
        self.init(Some(window))?;
        Ok(())
    }

    fn on_event(&mut self, _: &Window, event: &Event) -> Control {
        match event {
            Event::Resized { width, height } if *width > 0 && *height > 0 => {
                if let Some(g) = &self.gfx
                    && let Some(sc) = g.swapchain
                    && let Err(e) = g.device.resize_swapchain(sc, *width, *height)
                {
                    tracing::error!(error = %e, "resize failed");
                    return Control::Exit;
                }
            }
            Event::CloseRequested
            | Event::Key {
                code: KeyCode::Escape,
                pressed: true,
            } => return Control::Exit,
            _ => self.input.event(event),
        }
        Control::Continue
    }

    fn on_frame(&mut self, window: &Window) -> Result<Control, AppError> {
        if window.is_minimized() {
            return Ok(Control::Continue);
        }
        Ok(if self.frame(Some(window))? {
            Control::Exit
        } else {
            Control::Continue
        })
    }

    fn on_exit(&mut self) {
        self.teardown();
    }
}
