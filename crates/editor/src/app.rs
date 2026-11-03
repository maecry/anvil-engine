//! The editor's frame loop: window or headless, the egui pass, the viewport render and the
//! script runner.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use debug::{EguiInput, EguiRenderer, EguiTarget, egui};
use platform::{App, AppError, Control, Event, RawWindowEvent, Window, WindowConfig};
use rhi::{
    Acquire, Backend, BufferDesc, BufferUsage, ColorAttachment, CommandEncoder, Device, DeviceDesc,
    Format, HasDisplayHandle, Instance, InstanceDesc, LoadOp, Memory, PresentStatus, RenderingDesc,
    StoreOp, SwapchainDesc, SwapchainHandle, SwapchainImage, TextureDesc, TextureHandle,
    TextureState, TextureUsage,
};
use rhi_vulkan::{Vulkan, VulkanDevice, VulkanInstance, VulkanSurface};

use crate::EditorError;
use crate::command::{EditorCommand, parse_script};
use crate::editor::{Editor, Outcome, VIEWPORT_FORMAT};

/// Longest a scripted command may wait for assets.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const HEADLESS_SIZE: (u32, u32) = (1600, 900);

/// How to run the editor ([`EditorOptions::from_env`] reads the `ANVIL_*` conventions).
#[derive(Clone, Debug)]
pub struct EditorOptions {
    /// Project to open at startup.
    pub project_dir: Option<PathBuf>,
    /// No window: render into an offscreen target.
    pub headless: bool,
    /// Window title and size.
    pub window: WindowConfig,
    /// Present with vsync.
    pub vsync: bool,
    /// Stop after this many frames (headless without a script defaults to 60).
    pub exit_after_frames: Option<u64>,
    /// `ANVIL_EDITOR_SCRIPT`: a RON list of [`EditorCommand`]s.
    pub script: Option<PathBuf>,
}

impl EditorOptions {
    /// Options from env: `ANVIL_WINDOW` (`0` = headless, or `WxH`), `ANVIL_VSYNC`,
    /// `ANVIL_EXIT_AFTER_FRAMES`, `ANVIL_EDITOR_SCRIPT`.
    pub fn from_env(project_dir: Option<PathBuf>) -> Self {
        let headless = std::env::var("ANVIL_WINDOW").as_deref() == Ok("0");
        let mut window = WindowConfig {
            title: "Anvil Editor".into(),
            width: 1600,
            height: 900,
            ..WindowConfig::default()
        };
        window = if headless {
            (window.width, window.height) = HEADLESS_SIZE;
            window
        } else {
            window.with_env_size()
        };
        Self {
            project_dir,
            headless,
            window,
            vsync: std::env::var("ANVIL_VSYNC").as_deref() != Ok("0"),
            exit_after_frames: std::env::var("ANVIL_EXIT_AFTER_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok()),
            script: std::env::var_os("ANVIL_EDITOR_SCRIPT").map(PathBuf::from),
        }
    }
}

/// Outcome of an editor run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorReport {
    /// Frames rendered.
    pub frames: u64,
    /// Validation errors seen by the Vulkan instance.
    pub validation_errors: u64,
    /// Validation warnings.
    pub validation_warnings: u64,
    /// Script commands that failed (the run stops at the first).
    pub command_errors: u64,
}

/// Executes a script one command per frame.
struct Script {
    cmds: VecDeque<EditorCommand>,
    wait: u32,
    blocked_since: Option<Instant>,
}

impl Script {
    /// Runs the next command. `Err` = the script failed.
    fn step(&mut self, ed: &mut Editor, device: &VulkanDevice) -> Result<(), EditorError> {
        if self.wait > 0 {
            self.wait -= 1;
            return Ok(());
        }
        let Some(cmd) = self.cmds.front().cloned() else {
            return Ok(());
        };
        if let EditorCommand::WaitFrames(n) = cmd {
            self.wait = n;
            self.cmds.pop_front();
            return Ok(());
        }
        match ed.apply(device, cmd.clone())? {
            Outcome::Done => {
                tracing::info!(command = ?cmd, "script command applied");
                self.cmds.pop_front();
                self.blocked_since = None;
            }
            Outcome::Wait => {
                let since = *self.blocked_since.get_or_insert_with(Instant::now);
                if since.elapsed() > COMMAND_TIMEOUT {
                    return Err(EditorError::cmd(format!(
                        "timed out waiting to apply {cmd:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}

struct ViewportTarget {
    texture: TextureHandle,
    extent: (u32, u32),
    egui_id: egui::TextureId,
}

enum Ui {
    Window(Box<EguiInput>),
    Headless(egui::Context),
}

struct Gfx {
    instance: VulkanInstance,
    surface: Option<VulkanSurface>,
    device: VulkanDevice,
    swapchain: Option<SwapchainHandle>,
    offscreen: Option<SwapchainImage>,
    egui: EguiRenderer,
    viewport: Option<ViewportTarget>,
    ui: Ui,
    pixels_per_point: f32,
}

struct EditorApp {
    opts: EditorOptions,
    script: Option<Script>,
    gfx: Option<Gfx>,
    editor: Option<Editor>,
    started: Instant,
    last: Instant,
    rendered: u64,
    report: Arc<Mutex<EditorReport>>,
    failure: Arc<Mutex<Option<String>>>,
}

/// Runs the editor until it quits, the window closes, a script command fails or
/// `exit_after_frames` is reached.
///
/// # Errors
/// Script parse failures, device, renderer or window failures, and a failed startup project.
pub fn run(opts: EditorOptions) -> Result<EditorReport, EditorError> {
    let script = match &opts.script {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| EditorError::io(p, &e))?;
            let cmds = parse_script(&text)
                .map_err(|e| EditorError::cmd(format!("{}: {e}", p.display())))?;
            tracing::info!(script = %p.display(), commands = cmds.len(), "editor script loaded");
            Some(Script {
                cmds: cmds.into(),
                wait: 0,
                blocked_since: None,
            })
        }
        None => None,
    };
    let report = Arc::new(Mutex::new(EditorReport::default()));
    let failure = Arc::new(Mutex::new(None));
    let mut app = EditorApp {
        opts,
        script,
        gfx: None,
        editor: None,
        started: Instant::now(),
        last: Instant::now(),
        rendered: 0,
        report: report.clone(),
        failure: failure.clone(),
    };
    if app.opts.headless {
        if app.script.is_none() {
            app.opts.exit_after_frames.get_or_insert(60);
        }
        app.init(None)?;
        let result = (|| -> Result<(), EditorError> {
            while !app.frame(None)? {}
            Ok(())
        })();
        app.teardown();
        result?;
    } else {
        let config = app.opts.window.clone();
        platform::run(config, app)?;
    }
    if let Some(msg) = failure.lock().ok().and_then(|f| f.clone()) {
        return Err(EditorError::Command(msg));
    }
    let r = *report
        .lock()
        .map_err(|_| EditorError::Io("report lock".into()))?;
    Ok(r)
}

fn lock_err(r: &Arc<Mutex<EditorReport>>, f: impl FnOnce(&mut EditorReport)) {
    if let Ok(mut r) = r.lock() {
        f(&mut r);
    }
}

impl EditorApp {
    fn init(&mut self, window: Option<&Window>) -> Result<(), EditorError> {
        let display = match window {
            Some(w) => Some(
                w.display_handle()
                    .map_err(|e| EditorError::Io(e.to_string()))?
                    .as_raw(),
            ),
            None => None,
        };
        let instance = Vulkan::create_instance(&InstanceDesc {
            app_name: "Anvil Editor".into(),
            display,
            ..InstanceDesc::default()
        })?;
        let surface = match window {
            Some(w) => Some(instance.create_surface(Arc::new(w.clone()))?),
            None => None,
        };
        let device = instance.create_device(&DeviceDesc::default(), surface.as_ref())?;
        let (swapchain, offscreen, format) = match (&surface, window) {
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
                (Some(sc), None, device.swapchain_format(sc)?)
            }
            _ => {
                let (width, height) = (self.opts.window.width, self.opts.window.height);
                let format = Format::Rgba8Srgb;
                let texture = device.create_texture(&TextureDesc {
                    label: Some("editor.offscreen"),
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
                (None, Some(image), format)
            }
        };
        let (ui, pixels_per_point) = match window {
            Some(w) => (
                Ui::Window(Box::new(EguiInput::new(w))),
                w.scale_factor() as f32,
            ),
            None => (Ui::Headless(egui::Context::default()), 1.0),
        };
        let egui = EguiRenderer::new(&device, format)?;
        let mut editor = Editor::new(&device)?;
        if let Some(dir) = &self.opts.project_dir {
            editor.apply(&device, EditorCommand::OpenProject { path: dir.clone() })?;
        }
        self.gfx = Some(Gfx {
            instance,
            surface,
            device,
            swapchain,
            offscreen,
            egui,
            viewport: None,
            ui,
            pixels_per_point,
        });
        self.editor = Some(editor);
        self.started = Instant::now();
        self.last = Instant::now();
        Ok(())
    }

    /// Recreates the viewport texture when the panel's pixel size changed.
    fn ensure_viewport(gfx: &mut Gfx, ed: &mut Editor) -> Result<(), EditorError> {
        let want = ed.viewport_rect.map_or((960, 540), |r| {
            let px = |v: f32| ((v * gfx.pixels_per_point).round() as u32).clamp(16, 4096);
            (px(r.width()), px(r.height()))
        });
        if gfx.viewport.as_ref().is_some_and(|v| v.extent == want) {
            return Ok(());
        }
        let texture = gfx.device.create_texture(&TextureDesc::new_2d(
            Some("editor.viewport"),
            want.0,
            want.1,
            VIEWPORT_FORMAT,
            TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        ))?;
        let egui_id = gfx.egui.register_texture(texture);
        if let Some(old) = gfx.viewport.replace(ViewportTarget {
            texture,
            extent: want,
            egui_id,
        }) {
            gfx.egui.unregister_texture(old.egui_id);
            gfx.device.destroy_texture(old.texture);
        }
        Ok(())
    }

    /// One frame. Returns true when the run should end.
    fn frame(&mut self, window: Option<&Window>) -> Result<bool, EditorError> {
        let (Some(gfx), Some(ed)) = (self.gfx.as_mut(), self.editor.as_mut()) else {
            return Ok(true);
        };
        let _span = tracing::info_span!("editor_frame", frame = self.rendered).entered();
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        ed.history.push(dt * 1e3);

        Self::ensure_viewport(gfx, ed)?;
        let Some(vp) = &gfx.viewport else {
            return Ok(true);
        };
        let (vp_texture, vp_extent) = (vp.texture, vp.extent);
        ed.viewport_tex = Some(vp.egui_id);

        // Acquire first: nothing below may be thrown away by an early return.
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
                    return Ok(ed.quit_requested());
                }
                Acquire::Minimized => return Ok(ed.quit_requested()),
            },
            (None, Some(img)) => img,
            (None, None) => return Ok(true),
        };

        // Assets, script, then the UI (which queues commands) and those commands.
        ed.begin_frame(device);
        if let Some(script) = self.script.as_mut() {
            if let Err(e) = script.step(ed, device) {
                tracing::error!(error = %e, "script command failed");
                lock_err(&self.report, |r| r.command_errors += 1);
                if let Ok(mut f) = self.failure.lock() {
                    *f = Some(e.to_string());
                }
                ed.quit = true;
            } else if script.cmds.is_empty() && self.opts.headless {
                ed.quit = true;
            }
        }
        let diag = device.diagnostics();
        let out = match &mut gfx.ui {
            Ui::Window(input) => {
                let Some(w) = window else { return Ok(true) };
                input.run(w, |ctx| ed.ui(ctx, &diag))
            }
            Ui::Headless(ctx) => {
                let (w, h) = (image.extent.0 as f32, image.extent.1 as f32);
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(w, h),
                        )),
                        time: Some(self.started.elapsed().as_secs_f64()),
                        ..egui::RawInput::default()
                    },
                    |ctx| ed.ui(ctx, &diag),
                )
            }
        };
        gfx.pixels_per_point = out.pixels_per_point;
        ed.apply_pending(device);

        let scene = ed.render_scene(device);
        gfx.egui.update_textures(device, &out.textures_delta)?;
        let ctx = match &gfx.ui {
            Ui::Window(input) => input.context().clone(),
            Ui::Headless(ctx) => ctx.clone(),
        };
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);

        // Record: viewport scene -> egui over the window image -> optional readback.
        let screenshot = ed.take_screenshot();
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
        let target = SwapchainImage {
            swapchain: SwapchainHandle::NULL,
            texture: vp_texture,
            index: 0,
            extent: vp_extent,
            format: VIEWPORT_FORMAT,
        };
        ed.core.renderer.render(
            device,
            &mut cmd,
            &frame,
            &scene,
            &target,
            TextureState::ShaderRead,
        )?;
        // A headless target keeps the state the previous frame's readback left it in.
        let from = if gfx.swapchain.is_none() && self.rendered > 0 {
            TextureState::TransferSrc
        } else {
            TextureState::Undefined
        };
        cmd.texture_barrier(image.texture, from, TextureState::ColorAttachment);
        cmd.begin_rendering(&RenderingDesc {
            color: &[ColorAttachment {
                texture: image.texture,
                load: LoadOp::Clear,
                store: StoreOp::Store,
                clear: [0.08, 0.08, 0.09, 1.0],
            }],
            depth: None,
            width: image.extent.0,
            height: image.extent.1,
        });
        cmd.end_rendering();
        gfx.egui.render(
            device,
            &mut cmd,
            &frame,
            &EguiTarget {
                texture: image.texture,
                extent: image.extent,
                pixels_per_point: out.pixels_per_point,
            },
            &prims,
        )?;
        let final_state = if readback.is_some() || gfx.swapchain.is_none() {
            TextureState::TransferSrc
        } else {
            TextureState::Present
        };
        cmd.texture_barrier(image.texture, TextureState::ColorAttachment, final_state);
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
            runtime::write_png(&path, extent, format, pixels)?;
            tracing::info!(path = %path.display(), "screenshot written");
        }
        if status == PresentStatus::Recreate
            && let (Some(sc), Some(w)) = (gfx.swapchain, window)
        {
            let (w, h) = w.inner_size();
            device.resize_swapchain(sc, w, h)?;
        }
        self.rendered += 1;
        Ok(ed.quit_requested()
            || self
                .opts
                .exit_after_frames
                .is_some_and(|n| self.rendered >= n))
    }

    fn teardown(&mut self) {
        let (Some(gfx), Some(ed)) = (self.gfx.take(), self.editor.take()) else {
            return;
        };
        if let Err(e) = gfx.device.wait_idle() {
            tracing::error!(error = %e, "wait_idle failed");
        }
        gfx.egui.destroy(&gfx.device);
        if let Some(v) = gfx.viewport {
            gfx.device.destroy_texture(v.texture);
        }
        ed.core.destroy(&gfx.device);
        if let Some(sc) = gfx.swapchain {
            gfx.device.destroy_swapchain(sc);
        }
        if let Some(img) = gfx.offscreen {
            gfx.device.destroy_texture(img.texture);
        }
        tracing::info!(diag = ?gfx.device.diagnostics(), "diagnostics at exit");
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
        let frames = self.rendered;
        lock_err(&self.report, |r| {
            r.frames = frames;
            r.validation_errors = counts.errors;
            r.validation_warnings = counts.warnings;
        });
    }
}

impl App for EditorApp {
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
            Event::CloseRequested => return Control::Exit,
            _ => {}
        }
        Control::Continue
    }

    fn on_raw_event(&mut self, window: &Window, event: &RawWindowEvent) {
        if let Some(Gfx {
            ui: Ui::Window(input),
            ..
        }) = self.gfx.as_mut()
        {
            input.on_window_event(window, event);
        }
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
