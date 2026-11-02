//! M3 vertical slice: a grid of spheres with varying metallic/roughness, a textured cube and a
//! ground plane, lit by a directional, a point and a spot light, with a fly camera
//! (WASD + Space/Ctrl, Shift = faster, mouse-look while the right button is held).
//!
//! Env: `ANVIL_EXIT_AFTER_FRAMES=n` renders n frames and exits (code 1 on validation errors).
//! `ANVIL_SCREENSHOT=path.png` (with `ANVIL_EXIT_AFTER_FRAMES`) writes the last frame as a PNG.
//! See ADR-0009 §7 and ADR-0011 §11.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::Context;
use foundation::math::{Mat4, Quat, Vec3};
use platform::{App, AppError, Control, Event, KeyCode, MouseButton, Window, WindowConfig};
use renderer::{Camera, Light, MaterialDesc, MeshInstance, RenderScene, Renderer, mesh};
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
                vsync: std::env::var("ANVIL_VSYNC").as_deref() != Ok("0"),
                preferred_format: rhi::Format::Bgra8Srgb,
            },
        )?;
        let format = device.swapchain_format(swapchain)?;
        let mut r = Renderer::new(&device, format, (w.max(1), h.max(1)))?;

        let sphere = r.create_mesh(&device, "sphere", &mesh::uv_sphere(48, 24))?;
        let cube = r.create_mesh(&device, "cube", &mesh::cube())?;
        let plane = r.create_mesh(&device, "plane", &mesh::plane(40.0))?;
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

        let mut instances = vec![
            MeshInstance {
                mesh: plane,
                material: ground,
                transform: Mat4::IDENTITY,
            },
            MeshInstance {
                mesh: cube,
                material: checker_mat,
                transform: Mat4::from_rotation_translation(
                    Quat::from_rotation_y(0.5),
                    Vec3::new(-4.2, 0.5, 0.5),
                ),
            },
        ];
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
                instances.push(MeshInstance {
                    mesh: sphere,
                    material,
                    transform: Mat4::from_translation(Vec3::new(
                        (col as f32 - 2.0) * 1.4,
                        0.5,
                        -(row as f32) * 1.4,
                    )),
                });
            }
        }
        let lights = vec![
            Light::Directional {
                direction: Vec3::new(-0.5, -1.0, -0.4),
                color: Vec3::new(1.0, 0.96, 0.9),
                intensity: 3.0,
            },
            Light::Point {
                position: Vec3::new(-3.0, 2.0, 2.0),
                color: Vec3::new(1.0, 0.5, 0.3),
                intensity: 20.0,
                range: 10.0,
            },
            Light::Spot {
                position: Vec3::new(4.0, 4.0, 3.0),
                direction: Vec3::new(-0.8, -1.0, -1.0),
                color: Vec3::new(0.4, 0.6, 1.0),
                intensity: 40.0,
                range: 15.0,
                inner_cone: 0.2,
                outer_cone: 0.4,
            },
        ];
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
    }

    fn render(&mut self, window: &Window) -> anyhow::Result<bool> {
        if window.is_minimized() || self.gfx.is_none() {
            return Ok(false);
        }
        self.update();
        let last_frame = self.exit_after.is_some_and(|n| self.rendered + 1 >= n);
        let screenshot = self.screenshot.clone().filter(|_| last_frame);
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
        let frame = g.device.begin_frame()?;
        let image = match g.device.acquire(g.swapchain)? {
            Acquire::Image(i) => i,
            Acquire::OutOfDate => {
                let (w, h) = window.inner_size();
                g.device.resize_swapchain(g.swapchain, w, h)?;
                return Ok(false);
            }
            Acquire::Minimized => return Ok(false),
        };
        let scene = RenderScene {
            camera: scene_camera,
            meshes: g.instances.clone(),
            lights: g.lights.clone(),
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
        g.renderer
            .render(&g.device, &mut cmd, frame.slot, &scene, &image)?;
        if let Some(rb) = readback {
            cmd.texture_barrier(
                image.texture,
                TextureState::ColorAttachment,
                TextureState::TransferSrc,
            );
            cmd.copy_texture_to_buffer(image.texture, rb, 0);
            cmd.texture_barrier(
                image.texture,
                TextureState::TransferSrc,
                TextureState::Present,
            );
        } else {
            cmd.texture_barrier(
                image.texture,
                TextureState::ColorAttachment,
                TextureState::Present,
            );
        }
        let finished = cmd.finish()?;
        let (extent, format) = (image.extent, image.format);
        g.device.submit(finished, &image)?;
        let status = g.device.present(g.swapchain, image)?;
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
        self.gfx = Some(Self::init(window)?);
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
    platform::run(
        WindowConfig {
            title: "Anvil PBR scene".into(),
            ..WindowConfig::default()
        },
        PbrScene {
            gfx: None,
            camera: FlyCamera {
                position: Vec3::new(0.0, 3.0, 9.0),
                yaw: 0.0,
                pitch: -0.3,
            },
            keys: Vec::new(),
            looking: false,
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
