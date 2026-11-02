//! M5 large-count workload (ADR-0013 §1): about 50 000 instances of a sphere, a cube and a torus
//! with mixed materials spread over a large field, lit by a cascaded-shadow sun and a few local
//! lights, seen from a camera that flies a fixed path (a function of the frame number, so runs
//! are reproducible). Every instance is a separate CPU-recorded draw: it exposes the CPU-side
//! limits of the M4 renderer on purpose (see `docs/benchmarks/BENCH-001-baseline.md`).
//!
//! Esc quits. Env: `ANVIL_INSTANCES=n` (default 50000), `ANVIL_RENDER_PATH=forward|deferred`,
//! `ANVIL_SSAO|ANVIL_TAA|ANVIL_BLOOM=0|1`, `ANVIL_EXIT_AFTER_FRAMES=n`,
//! `ANVIL_SCREENSHOT=path.png` (with `ANVIL_EXIT_AFTER_FRAMES`: writes the last frame),
//! `ANVIL_BENCH=path.json` (benchmark protocol, see `pbr-scene`), `ANVIL_WINDOW=WxH`,
//! `ANVIL_VSYNC=0`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::Context;
use foundation::math::{Mat4, Quat, Vec3};
use platform::{App, AppError, Control, Event, KeyCode, Window, WindowConfig};
use renderer::{
    Camera, Environment, Light, MaterialDesc, MeshData, MeshInstance, PostSettings, RenderPath,
    RenderScene, Renderer, Vertex, bench::Bench, mesh,
};
use rhi::{
    Acquire, Backend, BufferDesc, BufferUsage, CommandEncoder, Device, DeviceDesc,
    HasDisplayHandle, Instance, InstanceDesc, Memory, PresentStatus, SwapchainDesc,
    SwapchainHandle, TextureState,
};
use rhi_vulkan::{Vulkan, VulkanDevice, VulkanInstance, VulkanSurface};

/// Instances in the field unless `ANVIL_INSTANCES` says otherwise.
const DEFAULT_INSTANCES: usize = 50_000;
/// Distance between neighbouring grid cells in meters.
const SPACING: f32 = 4.0;
/// Materials the instances cycle through.
const MATERIALS: usize = 16;

/// Deterministic hash to 0..1 (no RNG dependency).
fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// A torus around the Y axis: ring radius `major`, tube radius `minor`.
fn torus(major: f32, minor: f32, segments: u32, sides: u32) -> MeshData {
    let mut m = MeshData::default();
    for i in 0..=segments {
        let u = i as f32 / segments as f32 * std::f32::consts::TAU;
        let (su, cu) = u.sin_cos();
        for j in 0..=sides {
            let v = j as f32 / sides as f32 * std::f32::consts::TAU;
            let (sv, cv) = v.sin_cos();
            let n = Vec3::new(cv * cu, sv, cv * su);
            m.vertices.push(Vertex {
                pos: [
                    (major + minor * cv) * cu,
                    minor * sv,
                    (major + minor * cv) * su,
                ],
                normal: n.to_array(),
                uv: [i as f32 / segments as f32, j as f32 / sides as f32],
                tangent: [-su, 0.0, cu, 1.0],
            });
        }
    }
    let stride = sides + 1;
    for i in 0..segments {
        for j in 0..sides {
            let a = i * stride + j;
            let b = a + stride;
            m.indices.extend([a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    m
}

/// Camera at frame `frame`: a slow loop over the field at low altitude, looking ahead.
fn camera_at(frame: u64) -> Camera {
    let pos = |f: f32| {
        let a = f * 0.004;
        Vec3::new(
            220.0 * a.sin(),
            9.0 + 3.0 * (2.0 * a).sin(),
            170.0 * a.cos(),
        )
    };
    let f = frame as f32;
    let (p, ahead) = (pos(f), pos(f + 40.0));
    let dir = (ahead - p).normalize();
    let yaw = (-dir.x).atan2(-dir.z);
    let pitch = -0.28;
    Camera {
        position: p,
        rotation: Quat::from_euler(foundation::math::EulerRot::YXZ, yaw, pitch, 0.0),
        fov_y: 60f32.to_radians(),
        near: 0.1,
        exposure: 1.0,
    }
}

/// Sun plus a few local lights that travel with the camera.
fn lights_at(cam: &Camera) -> Vec<Light> {
    let fwd = cam.rotation * Vec3::NEG_Z;
    let right = cam.rotation * Vec3::X;
    let mut v = vec![Light::Directional {
        direction: Vec3::new(0.45, -0.75, 0.35),
        color: Vec3::new(1.0, 0.95, 0.85),
        intensity: 3.0,
        cast_shadows: true,
    }];
    for (i, color) in [[1.0, 0.4, 0.2], [0.2, 0.5, 1.0], [0.3, 1.0, 0.4]]
        .into_iter()
        .enumerate()
    {
        let side = i as f32 - 1.0;
        v.push(Light::Point {
            position: cam.position + fwd * (14.0 + 6.0 * i as f32) + right * (side * 7.0)
                - Vec3::Y * 4.0,
            color: Vec3::from(color),
            intensity: 60.0,
            range: 16.0,
        });
    }
    v.push(Light::Spot {
        position: cam.position + Vec3::Y * 4.0,
        direction: fwd - Vec3::Y * 0.4,
        color: Vec3::new(1.0, 0.9, 0.7),
        intensity: 150.0,
        range: 40.0,
        inner_cone: 0.2,
        outer_cone: 0.45,
        cast_shadows: false,
    });
    v
}

struct Gfx {
    instance: VulkanInstance,
    surface: VulkanSurface,
    device: VulkanDevice,
    swapchain: SwapchainHandle,
    renderer: Renderer,
    instances: Vec<MeshInstance>,
}

struct ManyDraws {
    gfx: Option<Gfx>,
    path: RenderPath,
    post: PostSettings,
    bench: Option<Bench>,
    rendered: u64,
    exit_after: Option<u64>,
    screenshot: Option<String>,
    failed: Arc<AtomicBool>,
}

impl ManyDraws {
    fn init(window: &Window) -> anyhow::Result<Gfx> {
        let display = window.display_handle().context("display handle")?.as_raw();
        let instance = Vulkan::create_instance(&InstanceDesc {
            app_name: "many-draws".to_owned(),
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
                vsync: std::env::var("ANVIL_VSYNC").as_deref() != Ok("0")
                    && std::env::var_os("ANVIL_BENCH").is_none(),
                preferred_format: rhi::Format::Bgra8Srgb,
            },
        )?;
        let format = device.swapchain_format(swapchain)?;
        let mut r = Renderer::new(&device, format, (w.max(1), h.max(1)))?;

        let meshes = [
            r.create_mesh(&device, "sphere", &mesh::uv_sphere(16, 8))?,
            r.create_mesh(&device, "cube", &mesh::cube())?,
            r.create_mesh(&device, "torus", &torus(0.35, 0.15, 24, 12))?,
        ];
        let ground_mesh = r.create_mesh(&device, "ground", &mesh::plane(1500.0))?;
        let ground = r.create_material(&MaterialDesc {
            label: "ground",
            base_color: [0.16, 0.2, 0.14, 1.0],
            roughness: 0.95,
            ..MaterialDesc::default()
        });
        let materials: Vec<_> = (0..MATERIALS as u32)
            .map(|i| {
                let hue = i as f32 / MATERIALS as f32 * std::f32::consts::TAU;
                r.create_material(&MaterialDesc {
                    label: "field",
                    base_color: [
                        0.55 + 0.45 * hue.cos(),
                        0.55 + 0.45 * (hue + 2.1).cos(),
                        0.55 + 0.45 * (hue + 4.2).cos(),
                        1.0,
                    ],
                    metallic: if i % 4 == 0 { 1.0 } else { 0.0 },
                    roughness: 0.2 + 0.7 * hash01(i + 9000),
                    ..MaterialDesc::default()
                })
            })
            .collect();

        let count = std::env::var("ANVIL_INSTANCES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_INSTANCES);
        let cols = (count as f32 * 1.25).sqrt().ceil() as usize; // wider than deep
        let mut instances = Vec::with_capacity(count + 1);
        instances.push(MeshInstance::new(ground_mesh, ground, Mat4::IDENTITY));
        for n in 0..count {
            let (gx, gz) = ((n % cols) as f32, (n / cols) as f32);
            let h = n as u32 * 4;
            let scale = 0.7 + 0.9 * hash01(h);
            let at = Vec3::new(
                (gx - cols as f32 * 0.5) * SPACING + (hash01(h + 1) - 0.5) * 1.5,
                scale * 0.5,
                (gz - (count / cols) as f32 * 0.5) * SPACING + (hash01(h + 2) - 0.5) * 1.5,
            );
            instances.push(MeshInstance::new(
                meshes[n % 3],
                materials[(hash01(h + 3) * MATERIALS as f32) as usize % MATERIALS],
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(scale),
                    Quat::from_rotation_y(hash01(h + 3) * std::f32::consts::TAU),
                    at,
                ),
            ));
        }
        tracing::info!(instances = instances.len(), "many-draws field built");
        Ok(Gfx {
            instance,
            surface,
            device,
            swapchain,
            renderer: r,
            instances,
        })
    }

    fn render(&mut self, window: &Window) -> anyhow::Result<bool> {
        if window.is_minimized() || self.gfx.is_none() {
            return Ok(false);
        }
        let started = Instant::now();
        let last_frame = self.exit_after.is_some_and(|n| self.rendered + 1 >= n);
        let screenshot = self.screenshot.clone().filter(|_| last_frame);
        let camera = camera_at(self.rendered);
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
        g.renderer.set_render_path(self.path);
        g.renderer.set_post(self.post);
        let scene = RenderScene {
            camera,
            meshes: g.instances.clone(),
            lights: lights_at(&camera),
            environment: Environment::default(),
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

impl App for ManyDraws {
    fn on_start(&mut self, window: &Window) -> Result<(), AppError> {
        let g = Self::init(window)?;
        self.bench = Bench::from_env("many-draws", g.device.info());
        if let Some(b) = &self.bench {
            self.exit_after = Some(b.total_frames());
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
        title: "Anvil many draws".into(),
        ..WindowConfig::default()
    };
    if std::env::var_os("ANVIL_BENCH").is_some() {
        (window.width, window.height) = (1920, 1080);
        // A fixed-size window keeps tiling compositors from resizing it.
        window.resizable = false;
    }
    platform::run(
        window.with_env_size(),
        ManyDraws {
            gfx: None,
            path: match std::env::var("ANVIL_RENDER_PATH").as_deref() {
                Ok("deferred") => RenderPath::Deferred,
                _ => RenderPath::Forward,
            },
            post: PostSettings {
                ssao: env_flag("ANVIL_SSAO"),
                taa: env_flag("ANVIL_TAA"),
                bloom: env_flag("ANVIL_BLOOM"),
            },
            bench: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torus_faces_outward() {
        let m = torus(0.35, 0.15, 24, 12);
        assert_eq!(m.indices.len(), 24 * 12 * 6);
        for tri in m.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.vertices[i as usize]);
            let (pa, pb, pc) = (Vec3::from(a.pos), Vec3::from(b.pos), Vec3::from(c.pos));
            let face = (pb - pa).cross(pc - pa);
            if face.length() > 1e-9 {
                assert!(face.dot(Vec3::from(a.normal)) > 0.0, "inward triangle");
            }
        }
    }

    #[test]
    fn hash_is_unit_range_and_deterministic() {
        assert!((0..1000).all(|n| (0.0..1.0).contains(&hash01(n))));
        assert_eq!(hash01(7), hash01(7));
    }
}
