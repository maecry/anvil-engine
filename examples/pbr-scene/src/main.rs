//! M3/M4b showcase: a grid of spheres with varying metallic/roughness, a textured cube, an
//! alpha-tested fence, transparent glass spheres and a ground plane, lit by a cascaded-shadow sun,
//! a point light and a shadow-casting spot light, with a fly camera (WASD + Space/Ctrl,
//! Shift = faster, mouse-look while the right button is held).
//!
//! Keys: `T` animate sun, `G` forward/deferred, `L` stress lights (256 animated point lights),
//! `O` SSAO, `Y` TAA, `B` bloom, `C` cluster light-count heatmap, `V` cascade tint, `Esc` quit.
//!
//! Env: `ANVIL_EXIT_AFTER_FRAMES=n` renders n frames and exits (code 1 on validation errors).
//! `ANVIL_SCREENSHOT=path.png` (with `ANVIL_EXIT_AFTER_FRAMES`) writes the last frame as a PNG.
//! `ANVIL_RENDER_PATH=forward|deferred`, `ANVIL_STRESS_LIGHTS=1`,
//! `ANVIL_DEBUG_VIEW=clusters|cascades|ssao|lod|occlusion`, `ANVIL_SSAO=0|1`, `ANVIL_TAA=0|1`,
//! `ANVIL_BLOOM=0|1` (default 1), `ANVIL_TOGGLE_POST=n` (cycle all combinations every n frames) select the initial state headlessly; `ANVIL_CAMERA_ORBIT=1`
//! slowly orbits the camera (a function of the frame number, to inspect TAA in motion).
//! `ANVIL_BENCH=path.json` runs the benchmark protocol (ADR-0013 §1): the camera orbits, vsync
//! is off, the window defaults to 1920x1080 (`ANVIL_WINDOW=WxH`), `ANVIL_BENCH_WARMUP` (120) plus
//! `ANVIL_BENCH_FRAMES` (1000) frames are rendered, results go to the JSON file.
//! See ADR-0009 §7, ADR-0011 §11 and ADR-0012 §11.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::Context;
use foundation::math::{Mat4, Quat, Vec3};
use platform::{App, AppError, Control, Event, KeyCode, MouseButton, Window, WindowConfig};
use renderer::{
    AlphaMode, Camera, DebugView, Environment, Light, MaterialDesc, MeshInstance, PostSettings,
    RenderPath, RenderScene, Renderer, bench::Bench, mesh,
};
use rhi::{
    Acquire, Backend, BufferDesc, BufferUsage, CommandEncoder, Device, DeviceDesc,
    HasDisplayHandle, Instance, InstanceDesc, Memory, PresentStatus, SwapchainDesc,
    SwapchainHandle, TextureState,
};
use rhi_vulkan::{Vulkan, VulkanDevice, VulkanInstance, VulkanSurface};

const MOVE_SPEED: f32 = 4.0;
const LOOK_SENSITIVITY: f32 = 0.0025;

/// Procedural checker texture, tightly packed RGBA8.
fn checker(size: u32, cell: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / cell) + (y / cell)).is_multiple_of(2);
            data.extend_from_slice(if on {
                &[230, 230, 230, 255]
            } else {
                &[200, 60, 40, 255]
            });
        }
    }
    data
}

/// Procedural fence: opaque vertical slats separated by fully transparent gaps (alpha-tested).
fn fence(size: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let slat = x % 16 < 11 && y > size / 16 + (x % 16) / 2 % 3;
            let grain = 90 + (x * 7 + y * 3) % 40;
            data.extend_from_slice(&[
                grain as u8 + 60,
                grain as u8 + 20,
                40,
                if slat { 255 } else { 0 },
            ]);
        }
    }
    data
}

/// 256 small colored point lights drifting over the ground, a function of the frame number so
/// runs are reproducible.
fn stress_lights(frame: u64) -> Vec<Light> {
    let t = frame as f32 / 60.0;
    (0..256)
        .map(|i| {
            let (gx, gz) = ((i % 16) as f32, (i / 16) as f32);
            let phase = i as f32 * 0.7;
            let hue = i as f32 / 256.0 * std::f32::consts::TAU;
            Light::Point {
                position: Vec3::new(
                    (gx - 7.5) * 1.5 + (t * 0.6 + phase).sin() * 0.8,
                    0.25 + 0.15 * (t + phase).sin().abs(),
                    (gz - 11.0) * 1.2 + (t * 0.5 + phase).cos() * 0.8,
                ),
                color: Vec3::new(
                    0.5 + 0.5 * hue.cos(),
                    0.5 + 0.5 * (hue + 2.1).cos(),
                    0.5 + 0.5 * (hue + 4.2).cos(),
                ),
                intensity: 2.0,
                range: 2.5,
            }
        })
        .collect()
}

/// Sun azimuth (radians) at start; `T` animates it.
const SUN_START: f32 = 0.35;

/// Directional light pointing down, azimuth `a`.
fn sun_direction(a: f32) -> Vec3 {
    Vec3::new(a.cos() * 0.7, -0.6, a.sin() * 0.6)
}

fn sun(a: f32) -> Light {
    Light::Directional {
        direction: sun_direction(a),
        color: Vec3::new(1.0, 0.96, 0.9),
        intensity: 2.5,
        cast_shadows: true,
    }
}

struct Gfx {
    instance: VulkanInstance,
    surface: VulkanSurface,
    device: VulkanDevice,
    swapchain: SwapchainHandle,
    renderer: Renderer,
    instances: Vec<MeshInstance>,
    lights: Vec<Light>,
}

struct FlyCamera {
    position: Vec3,
    yaw: f32,
    pitch: f32,
}

impl FlyCamera {
    fn rotation(&self) -> Quat {
        Quat::from_euler(foundation::math::EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }
}

struct PbrScene {
    gfx: Option<Gfx>,
    camera: FlyCamera,
    keys: Vec<KeyCode>,
    looking: bool,
    sun_angle: f32,
    animate_sun: bool,
    stress: bool,
    path: RenderPath,
    debug: DebugView,
    post: PostSettings,
    orbit: bool,
    /// `ANVIL_TOGGLE_POST=n`: step through all 8 SSAO/TAA/bloom combinations every n frames.
    toggle_post: Option<u64>,
    bench: Option<Bench>,
    last: Instant,
    rendered: u64,
    exit_after: Option<u64>,
    screenshot: Option<String>,
    failed: Arc<AtomicBool>,
}

impl PbrScene {
    fn init(window: &Window) -> anyhow::Result<Gfx> {
        let display = window.display_handle().context("display handle")?.as_raw();
        let instance = Vulkan::create_instance(&InstanceDesc {
            app_name: "pbr-scene".to_owned(),
            display: Some(display),
            ..InstanceDesc::default()
        })?;
        let surface = instance.create_surface(Arc::new(window.clone()))?;
        let device = instance.create_device(&DeviceDesc::default(), Some(&surface))?;
        let (w, h) = window.inner_size();
        let swapchain = device.create_swapchain(
            &surface,
            &SwapchainDesc {
                width: w.max(1),
                height: h.max(1),
                // Benchmarks never wait for vblank.
                vsync: std::env::var("ANVIL_VSYNC").as_deref() != Ok("0")
                    && std::env::var_os("ANVIL_BENCH").is_none(),
                preferred_format: rhi::Format::Bgra8Srgb,
            },
        )?;
        let format = device.swapchain_format(swapchain)?;
        let mut r = Renderer::new(&device, format, (w.max(1), h.max(1)))?;

        let sphere = r.create_mesh(&device, "sphere", &mesh::uv_sphere(48, 24))?;
        let cube = r.create_mesh(&device, "cube", &mesh::cube())?;
        let plane = r.create_mesh(&device, "plane", &mesh::plane(400.0))?;
        let checker_tex = r.create_texture(&device, "checker", &checker(64, 8), 64, 64, true)?;

        let ground = r.create_material(&MaterialDesc {
            label: "ground",
            base_color: [0.2, 0.22, 0.25, 1.0],
            metallic: 0.0,
            roughness: 0.9,
            ..MaterialDesc::default()
        });
        let checker_mat = r.create_material(&MaterialDesc {
            label: "checker",
            base_color_texture: Some(checker_tex),
            metallic: 0.0,
            roughness: 0.6,
            ..MaterialDesc::default()
        });

        let fence_tex = r.create_texture(&device, "fence", &fence(64), 64, 64, true)?;
        let fence_mat = r.create_material(&MaterialDesc {
            label: "fence",
            base_color_texture: Some(fence_tex),
            metallic: 0.0,
            roughness: 0.8,
            alpha_mode: AlphaMode::Mask(0.5),
            double_sided: true,
            ..MaterialDesc::default()
        });
        let quad = r.create_mesh(&device, "quad", &mesh::plane(1.0))?;

        let mut instances = vec![
            MeshInstance::new(
                quad,
                fence_mat,
                Mat4::from_scale_rotation_translation(
                    Vec3::new(3.0, 1.0, 2.0),
                    Quat::from_rotation_y(-0.4)
                        * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                    Vec3::new(3.6, 1.0, 1.4),
                ),
            ),
            MeshInstance::new(plane, ground, Mat4::IDENTITY),
            MeshInstance::new(
                cube,
                checker_mat,
                Mat4::from_rotation_translation(
                    Quat::from_rotation_y(0.5),
                    Vec3::new(-4.2, 0.5, 0.5),
                ),
            ),
        ];
        // Tinted glass spheres in front of the grid (alpha-blended, sorted back to front).
        for (i, color) in [
            [0.2, 0.6, 1.0, 0.35],
            [1.0, 0.3, 0.3, 0.35],
            [0.3, 1.0, 0.4, 0.35],
        ]
        .into_iter()
        .enumerate()
        {
            let material = r.create_material(&MaterialDesc {
                label: "glass",
                base_color: color,
                metallic: 0.0,
                roughness: 0.05,
                alpha_mode: AlphaMode::Blend,
                ..MaterialDesc::default()
            });
            instances.push(MeshInstance::new(
                sphere,
                material,
                Mat4::from_translation(Vec3::new(-1.2 + i as f32 * 0.7, 0.5, 3.0 - i as f32 * 0.5)),
            ));
        }
        // Metallic increases along +X, roughness along -Z.
        for row in 0..4 {
            for col in 0..5 {
                let metallic = col as f32 / 4.0;
                let roughness = 0.1 + 0.3 * row as f32;
                let material = r.create_material(&MaterialDesc {
                    label: "sphere",
                    base_color: [1.0, 0.78, 0.34, 1.0],
                    metallic,
                    roughness,
                    ..MaterialDesc::default()
                });
                instances.push(MeshInstance::new(
                    sphere,
                    material,
                    Mat4::from_translation(Vec3::new(
                        (col as f32 - 2.0) * 1.4,
                        0.5,
                        -(row as f32) * 1.4,
                    )),
                ));
            }
        }
        let lights = vec![
            sun(SUN_START),
            // Warm point light between the cube and the spheres.
            Light::Point {
                position: Vec3::new(-2.6, 1.2, 1.8),
                color: Vec3::new(1.0, 0.45, 0.15),
                intensity: 25.0,
                range: 8.0,
            },
            // Cool spot light on the right-hand spheres.
            Light::Spot {
                position: Vec3::new(3.5, 3.5, 2.5),
                direction: Vec3::new(-0.35, -1.0, -0.9),
                color: Vec3::new(0.3, 0.5, 1.0),
                intensity: 90.0,
                range: 14.0,
                inner_cone: 0.25,
                outer_cone: 0.5,
                cast_shadows: true,
            },
        ];
        tracing::info!(
            "keys: WASD/Space/Ctrl move, Shift fast, RMB look, T sun animation, G forward/deferred, \
             L stress lights, O SSAO, Y TAA, B bloom, C cluster heatmap, V cascade tint, Esc quit; \
             env: ANVIL_RENDER_PATH, ANVIL_STRESS_LIGHTS, ANVIL_DEBUG_VIEW, ANVIL_SSAO, ANVIL_TAA, \
             ANVIL_BLOOM, ANVIL_CAMERA_ORBIT"
        );
        Ok(Gfx {
            instance,
            surface,
            device,
            swapchain,
            renderer: r,
            instances,
            lights,
        })
    }

    fn update(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        if self.animate_sun {
            self.sun_angle += dt * 0.5;
        }
        let down = |k| self.keys.contains(&k);
        let axis = |pos, neg| f32::from(u8::from(down(pos))) - f32::from(u8::from(down(neg)));
        let rot = self.camera.rotation();
        let dir =
            rot * Vec3::new(
                axis(KeyCode::D, KeyCode::A),
                0.0,
                axis(KeyCode::S, KeyCode::W),
            ) + Vec3::Y * axis(KeyCode::Space, KeyCode::Ctrl);
        let speed = MOVE_SPEED * if down(KeyCode::Shift) { 3.0 } else { 1.0 };
        self.camera.position += dir * speed * dt;
        if self.orbit {
            let a = self.rendered as f32 * 0.005;
            self.camera = FlyCamera {
                position: Vec3::new(9.0 * a.sin(), 2.2, 9.0 * a.cos()),
                yaw: a,
                pitch: -0.12,
            };
        }
    }

    fn render(&mut self, window: &Window) -> anyhow::Result<bool> {
        if window.is_minimized() || self.gfx.is_none() {
            return Ok(false);
        }
        let started = Instant::now();
        self.update();
        let last_frame = self.exit_after.is_some_and(|n| self.rendered + 1 >= n);
        let screenshot = self.screenshot.clone().filter(|_| last_frame);
        let sun_angle = self.sun_angle;
        let scene_camera = Camera {
            position: self.camera.position,
            rotation: self.camera.rotation(),
            fov_y: 60f32.to_radians(),
            near: 0.1,
            exposure: 1.0,
        };
        let Some(g) = self.gfx.as_mut() else {
            return Ok(false);
        };
        let _span = tracing::info_span!("app_frame", frame = self.rendered).entered();
        let wait = Instant::now();
        let frame = g.device.begin_frame()?;
        let acquired = g.device.acquire(g.swapchain)?;
        // Time blocked on the GPU is not CPU work; the bench reports CPU time without it.
        let blocked = wait.elapsed();
        let image = match acquired {
            Acquire::Image(i) => i,
            Acquire::OutOfDate => {
                let (w, h) = window.inner_size();
                g.device.resize_swapchain(g.swapchain, w, h)?;
                return Ok(false);
            }
            Acquire::Minimized => return Ok(false),
        };
        g.lights[0] = sun(sun_angle);
        g.renderer.set_render_path(self.path);
        g.renderer.set_debug_view(self.debug);
        let mut lights = g.lights.clone();
        if self.stress {
            lights.extend(stress_lights(self.rendered));
        }
        // The checker cube turns slowly; its previous transform feeds the TAA velocity.
        let mut meshes = g.instances.clone();
        meshes[CUBE].transform = cube_transform(self.rendered);
        meshes[CUBE].prev_transform = cube_transform(self.rendered.saturating_sub(1));
        if let Some(n) = self
            .toggle_post
            .filter(|n| *n > 0 && self.rendered > 0 && self.rendered.is_multiple_of(*n))
        {
            let step = (self.rendered / n) as u8 % 8;
            self.post = PostSettings {
                ssao: step & 1 != 0,
                taa: step & 2 != 0,
                bloom: step & 4 != 0,
            };
        }
        g.renderer.set_post(self.post);
        let scene = RenderScene {
            camera: scene_camera,
            meshes,
            lights,
            // The sky's sun follows the shadow-casting directional light.
            environment: Environment {
                sun_direction: sun_direction(sun_angle),
                ..Environment::default()
            },
        };
        let readback = match &screenshot {
            Some(_) => Some(g.device.create_buffer(&BufferDesc {
                label: Some("screenshot.readback"),
                size: u64::from(image.extent.0) * u64::from(image.extent.1) * 4,
                usage: BufferUsage::TRANSFER_DST,
                memory: Memory::Readback,
            })?),
            None => None,
        };
        let mut cmd = g.device.begin_commands(&frame)?;
        // The graph leaves the image in `TransferSrc` for a readback, otherwise in `Present`.
        let final_state = if readback.is_some() {
            TextureState::TransferSrc
        } else {
            TextureState::Present
        };
        g.renderer
            .render(&g.device, &mut cmd, &frame, &scene, &image, final_state)?;
        if let Some(rb) = readback {
            cmd.copy_texture_to_buffer(image.texture, rb, 0);
            cmd.texture_barrier(
                image.texture,
                TextureState::TransferSrc,
                TextureState::Present,
            );
        }
        let finished = cmd.finish()?;
        let (extent, format) = (image.extent, image.format);
        g.device.submit(finished, &image)?;
        let status = g.device.present(g.swapchain, image)?;
        if let Some(b) = self.bench.as_mut() {
            b.record(
                self.rendered,
                started.elapsed().saturating_sub(blocked),
                &g.renderer.stats(),
                &g.device.diagnostics(),
                extent,
            );
        }
        if let (Some(path), Some(rb)) = (screenshot, readback) {
            g.device.wait_idle()?;
            let mut pixels = vec![0u8; extent.0 as usize * extent.1 as usize * 4];
            g.device.read_buffer(rb, 0, &mut pixels)?;
            g.device.destroy_buffer(rb);
            write_png(&path, extent, format, pixels)?;
            tracing::info!(path = %path, "screenshot written");
        }
        if status == PresentStatus::Recreate {
            let (w, h) = window.inner_size();
            g.device.resize_swapchain(g.swapchain, w, h)?;
        }
        Ok(true)
    }

    fn teardown(&mut self) {
        let Some(g) = self.gfx.take() else { return };
        if let Err(e) = g.device.wait_idle() {
            tracing::error!(error = %e, "wait_idle failed");
        }
        g.renderer.destroy(&g.device);
        g.device.destroy_swapchain(g.swapchain);
        let diag = g.device.diagnostics();
        tracing::info!(?diag, "diagnostics at exit");
        let Gfx {
            instance,
            surface,
            device,
            ..
        } = g;
        drop(device);
        drop(surface);
        let counts = instance.validation_counts();
        tracing::info!(?counts, "final validation counts");
        if counts.errors > 0 {
            self.failed.store(true, Ordering::Relaxed);
        }
    }
}

/// Writes swapchain texels (BGRA or RGBA, 8 bit) as an RGBA PNG.
fn write_png(
    path: &str,
    (w, h): (u32, u32),
    format: rhi::Format,
    mut pixels: Vec<u8>,
) -> anyhow::Result<()> {
    if matches!(format, rhi::Format::Bgra8Srgb | rhi::Format::Bgra8Unorm) {
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
    }
    let file = std::fs::File::create(path).with_context(|| format!("create {path}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

impl App for PbrScene {
    fn on_start(&mut self, window: &Window) -> Result<(), AppError> {
        let g = Self::init(window)?;
        self.bench = Bench::from_env("pbr-scene", g.device.info());
        if let Some(b) = &self.bench {
            self.exit_after = Some(b.total_frames());
            self.orbit = true;
        }
        self.gfx = Some(g);
        Ok(())
    }

    fn on_event(&mut self, _: &Window, event: &Event) -> Control {
        match event {
            Event::Resized { width, height } if *width > 0 && *height > 0 => {
                if let Some(g) = &self.gfx
                    && let Err(e) = g.device.resize_swapchain(g.swapchain, *width, *height)
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
            Event::Key {
                code: KeyCode::T,
                pressed: true,
            } => self.animate_sun = !self.animate_sun,
            Event::Key {
                code: KeyCode::G,
                pressed: true,
            } => {
                self.path = match self.path {
                    RenderPath::Forward => RenderPath::Deferred,
                    RenderPath::Deferred => RenderPath::Forward,
                };
                tracing::info!(path = ?self.path, "render path");
            }
            Event::Key {
                code: KeyCode::L,
                pressed: true,
            } => {
                self.stress = !self.stress;
                tracing::info!(stress = self.stress, "stress lights");
            }
            Event::Key {
                code: KeyCode::O,
                pressed: true,
            } => self.post.ssao = !self.post.ssao,
            Event::Key {
                code: KeyCode::Y,
                pressed: true,
            } => self.post.taa = !self.post.taa,
            Event::Key {
                code: KeyCode::B,
                pressed: true,
            } => self.post.bloom = !self.post.bloom,
            Event::Key {
                code: KeyCode::C,
                pressed: true,
            } => self.debug = toggle(self.debug, DebugView::Clusters),
            Event::Key {
                code: KeyCode::V,
                pressed: true,
            } => self.debug = toggle(self.debug, DebugView::Cascades),
            Event::Key { code, pressed } => {
                self.keys.retain(|k| k != code);
                if *pressed {
                    self.keys.push(*code);
                }
            }
            Event::MouseButton {
                button: MouseButton::Right,
                pressed,
            } => self.looking = *pressed,
            Event::MouseMotion { dx, dy } if self.looking => {
                self.camera.yaw -= *dx as f32 * LOOK_SENSITIVITY;
                self.camera.pitch =
                    (self.camera.pitch - *dy as f32 * LOOK_SENSITIVITY).clamp(-1.55, 1.55);
            }
            _ => {}
        }
        Control::Continue
    }

    fn on_frame(&mut self, window: &Window) -> Result<Control, AppError> {
        if self.render(window)? {
            self.rendered += 1;
        }
        if self.exit_after.is_some_and(|n| self.rendered >= n) {
            return Ok(Control::Exit);
        }
        Ok(Control::Continue)
    }

    fn on_exit(&mut self) {
        self.teardown();
    }
}

/// `name=0` disables, anything else (or unset) enables.
fn env_flag(name: &str) -> bool {
    std::env::var(name).as_deref() != Ok("0")
}

/// The turning checker cube at frame `frame`; index of its instance in `Gfx::instances`.
const CUBE: usize = 2;
fn cube_transform(frame: u64) -> Mat4 {
    Mat4::from_rotation_translation(
        Quat::from_rotation_y(0.5 + frame as f32 * 0.015),
        Vec3::new(-4.2, 0.5, 0.5),
    )
}

fn toggle(current: DebugView, view: DebugView) -> DebugView {
    if current == view {
        DebugView::None
    } else {
        view
    }
}

fn main() -> anyhow::Result<()> {
    foundation::init_logging()?;
    let failed = Arc::new(AtomicBool::new(false));
    let exit_after = std::env::var("ANVIL_EXIT_AFTER_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok());
    let screenshot = std::env::var("ANVIL_SCREENSHOT").ok();
    if screenshot.is_some() && exit_after.is_none() {
        tracing::warn!("ANVIL_SCREENSHOT needs ANVIL_EXIT_AFTER_FRAMES, ignoring");
    }
    let mut window = WindowConfig {
        title: "Anvil PBR scene".into(),
        ..WindowConfig::default()
    };
    if std::env::var_os("ANVIL_BENCH").is_some() {
        (window.width, window.height) = (1920, 1080);
        // A fixed-size window keeps tiling compositors from resizing it.
        window.resizable = false;
    }
    platform::run(
        window.with_env_size(),
        PbrScene {
            gfx: None,
            camera: FlyCamera {
                position: Vec3::new(0.0, 2.2, 9.0),
                yaw: 0.0,
                pitch: -0.12,
            },
            keys: Vec::new(),
            looking: false,
            sun_angle: SUN_START,
            animate_sun: false,
            stress: std::env::var("ANVIL_STRESS_LIGHTS").as_deref() == Ok("1"),
            path: match std::env::var("ANVIL_RENDER_PATH").as_deref() {
                Ok("deferred") => RenderPath::Deferred,
                _ => RenderPath::Forward,
            },
            post: PostSettings {
                ssao: env_flag("ANVIL_SSAO"),
                taa: env_flag("ANVIL_TAA"),
                bloom: env_flag("ANVIL_BLOOM"),
            },
            orbit: std::env::var("ANVIL_CAMERA_ORBIT").as_deref() == Ok("1"),
            toggle_post: std::env::var("ANVIL_TOGGLE_POST")
                .ok()
                .and_then(|v| v.parse().ok()),
            debug: std::env::var("ANVIL_DEBUG_VIEW")
                .ok()
                .and_then(|v| DebugView::from_name(&v))
                .unwrap_or(DebugView::None),
            bench: None,
            last: Instant::now(),
            rendered: 0,
            exit_after,
            screenshot,
            failed: failed.clone(),
        },
    )?;
    if failed.load(Ordering::Relaxed) {
        std::process::exit(1);
    }
    Ok(())
}
