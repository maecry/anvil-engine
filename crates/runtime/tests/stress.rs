//! Stress tests (ADR-0015 §3, MASTER-000 §15.5), gated by `ANVIL_GPU_TESTS=1`. Counts are
//! moderate by default; `ANVIL_STRESS=1` multiplies them by 10 for local soak runs. On a
//! software rasterizer (lavapipe, `GpuKind::Cpu`) the frame-heavy counts shrink so the whole file
//! stays well under a minute. Every test ends with `assert_no_leaks` and zero validation messages.
//!
//! shortcut: the 50 swapchain resizes of the ADR are not covered, they need a window and CI has
//! none; upgrade when a windowed test harness exists.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use assets::AssetServer;
use assets::fixtures::checker_png;
use foundation::math::{Mat4, Quat, Vec3};
use renderer::{
    Camera, Environment, Light, MaterialDesc, MeshInstance, PostSettings, RenderScene, Renderer,
    mesh,
};
use rhi::{
    Backend, BufferDesc, BufferUsage, CommandEncoder, Device, DeviceDesc, Format, GpuKind,
    Instance, InstanceDesc, Memory, SwapchainHandle, SwapchainImage, TextureDesc, TextureState,
    TextureUsage, ValidationMode,
};
use rhi_vulkan::{Vulkan, VulkanDevice, VulkanInstance, testing::assert_no_leaks};
use runtime::RuntimeCore;

const SIZE: (u32, u32) = (256, 256);

/// Test-wide texture budget (MB): far below the streaming test's texture set. Set once, before
/// any test creates a Vulkan instance, so no thread reads the environment while it changes.
const TEXTURE_BUDGET_MB: &str = "4";

struct Rig {
    inst: VulkanInstance,
    dev: VulkanDevice,
    target: SwapchainImage,
    software: bool,
    // Last field, so it drops after the device: one stress test at a time (concurrent devices
    // under sync validation crashed lavapipe in CI, and overlapping runs skew the stress counts).
    _serial: MutexGuard<'static, ()>,
}

/// Multiplies a count by 10 under `ANVIL_STRESS=1`.
fn scaled(n: u32) -> u32 {
    if std::env::var("ANVIL_STRESS").as_deref() == Ok("1") {
        n * 10
    } else {
        n
    }
}

fn rig() -> Option<Rig> {
    if std::env::var("ANVIL_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
        return None;
    }
    static SERIAL: Mutex<()> = Mutex::new(());
    // A failed (panicked) earlier test poisons the lock; the next test can still run.
    let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    static ENV: Once = Once::new();
    ENV.call_once(|| {
        let _ = foundation::init_logging();
        // SAFETY: every test passes through this `Once` before touching Vulkan, so no other
        // thread reads or writes the environment concurrently.
        unsafe { std::env::set_var("ANVIL_TEXTURE_BUDGET_MB", TEXTURE_BUDGET_MB) };
    });
    let inst = Vulkan::create_instance(&InstanceDesc {
        validation: ValidationMode::On,
        sync_validation: true,
        ..InstanceDesc::default()
    })
    .unwrap();
    let dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let software = dev.diagnostics().info.kind == GpuKind::Cpu;
    let format = Format::Rgba8Srgb;
    let texture = dev
        .create_texture(&TextureDesc {
            label: Some("stress.target"),
            width: SIZE.0,
            height: SIZE.1,
            format,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::TRANSFER_SRC,
            mip_levels: 1,
            array_layers: 1,
            cube: false,
        })
        .unwrap();
    let target = SwapchainImage {
        swapchain: SwapchainHandle::NULL,
        texture,
        index: 0,
        extent: SIZE,
        format,
    };
    Some(Rig {
        inst,
        dev,
        target,
        software,
        _serial: serial,
    })
}

impl Rig {
    fn renderer(&self) -> Renderer {
        let mut r = Renderer::new(&self.dev, self.target.format, SIZE).unwrap();
        r.set_post(PostSettings {
            ssao: false,
            taa: false,
            bloom: false,
        });
        r
    }

    /// An empty submitted frame: retires deferred destroys.
    fn empty_frame(&mut self) {
        let frame = self.dev.begin_frame().unwrap();
        let cmd = self.dev.begin_commands(&frame).unwrap();
        let fin = cmd.finish().unwrap();
        self.dev.submit_unwindowed(fin).unwrap();
    }

    /// Renders one frame and waits for it (one shared target).
    fn render(&mut self, r: &mut Renderer, scene: &RenderScene) {
        let frame = self.dev.begin_frame().unwrap();
        let mut cmd = self.dev.begin_commands(&frame).unwrap();
        r.render(
            &self.dev,
            &mut cmd,
            &frame,
            scene,
            &self.target,
            TextureState::TransferSrc,
        )
        .unwrap();
        let fin = cmd.finish().unwrap();
        self.dev.submit_unwindowed(fin).unwrap();
        self.dev.wait_idle().unwrap();
    }

    /// Destroys the target, then checks for leaks and validation messages.
    fn finish(mut self) {
        self.dev.wait_idle().unwrap();
        self.dev.destroy_texture(self.target.texture);
        assert_no_leaks(&mut self.dev);
        let c = self.inst.validation_counts();
        assert_eq!((c.errors, c.warnings), (0, 0), "validation messages: {c:?}");
    }
}

fn camera(position: Vec3, pitch: f32) -> Camera {
    Camera {
        position,
        rotation: Quat::from_rotation_x(pitch),
        fov_y: 1.0,
        near: 0.1,
        exposure: 1.0,
    }
}

fn sun() -> Light {
    Light::Directional {
        direction: Vec3::new(0.3, -1.0, -0.4).normalize(),
        color: Vec3::ONE,
        intensity: 3.0,
        cast_shadows: false,
    }
}

/// Copies a project (without the import cache) so a test can modify it.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (p, name) = (e.path(), e.file_name());
        if name == ".anvil" {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &to.join(&name));
        } else {
            std::fs::copy(&p, to.join(&name)).unwrap();
        }
    }
}

/// Creates and destroys buffers, textures and materials in batches.
#[test]
fn churn() {
    let Some(mut rig) = rig() else { return };
    let mut r = rig.renderer();
    let (total, batch) = (scaled(2000), 100);
    for _ in 0..total / batch {
        let buffers: Vec<_> = (0..batch)
            .map(|_| {
                rig.dev
                    .create_buffer(&BufferDesc {
                        label: Some("churn.buffer"),
                        size: 4096,
                        usage: BufferUsage::STORAGE | BufferUsage::TRANSFER_DST,
                        memory: Memory::GpuOnly,
                    })
                    .unwrap()
            })
            .collect();
        let texels = [200u8; 16 * 16 * 4];
        let textures: Vec<_> = (0..batch)
            .map(|_| {
                r.create_texture(&rig.dev, "churn.texture", &texels, 16, 16, true)
                    .unwrap()
            })
            .collect();
        let materials: Vec<_> = textures
            .iter()
            .map(|t| {
                r.create_material(&MaterialDesc {
                    base_color_texture: Some(*t),
                    ..MaterialDesc::default()
                })
            })
            .collect();
        for b in buffers {
            rig.dev.destroy_buffer(b);
        }
        for m in materials {
            r.destroy_material(m);
        }
        for t in textures {
            r.destroy_texture(&rig.dev, t);
        }
        rig.empty_frame();
    }
    rig.dev.wait_idle().unwrap();
    r.destroy(&rig.dev);
    rig.finish();
}

/// 50,000 mesh instances through the GPU culling path (5,000 on a software rasterizer).
#[test]
fn large_scene() {
    let Some(mut rig) = rig() else { return };
    let mut r = rig.renderer();
    r.set_gpu_culling(true);
    let count = scaled(if rig.software { 5_000 } else { 50_000 });
    let sphere = r
        .create_mesh(&rig.dev, "sphere", &mesh::uv_sphere(8, 4))
        .unwrap();
    let m = r.create_material(&MaterialDesc {
        base_color: [0.7, 0.8, 0.9, 1.0],
        ..MaterialDesc::default()
    });
    // A 3D grid of unit spheres in front of the camera.
    let side = (count as f32).cbrt().ceil() as u32;
    let meshes = (0..count)
        .map(|i| {
            let (x, y, z) = (i % side, (i / side) % side, i / (side * side));
            let p = Vec3::new(x as f32, y as f32, -(z as f32)) * 2.5 - Vec3::splat(side as f32);
            MeshInstance::new(
                sphere,
                m,
                Mat4::from_translation(p) * Mat4::from_scale(Vec3::splat(0.8)),
            )
        })
        .collect();
    let scene = RenderScene {
        camera: camera(Vec3::new(0.0, 0.0, side as f32 * 3.0), 0.0),
        meshes,
        lights: vec![sun()],
        environment: Environment::default(),
    };
    for _ in 0..3 {
        rig.render(&mut r, &scene);
    }
    let stats = r.stats();
    assert_eq!(stats.instances, count, "{stats:?}");
    rig.dev.wait_idle().unwrap();
    r.destroy(&rig.dev);
    rig.finish();
}

/// More texture bytes than the budget, visible set cycling: textures get demoted and the
/// residency stays bounded.
#[test]
fn streaming_under_a_small_budget() {
    let Some(mut rig) = rig() else { return };
    let mut r = rig.renderer();
    let plane = r.create_mesh(&rig.dev, "plane", &mesh::plane(2.0)).unwrap();
    let count: u32 = if rig.software { 10 } else { 16 };
    let texels: Vec<u8> = (0..512 * 512 * 4).map(|i| (i * 7 % 251) as u8).collect();
    let mut textures = Vec::new();
    let mut materials = Vec::new();
    for _ in 0..count {
        let t = r
            .create_texture(&rig.dev, "stream", &texels, 512, 512, true)
            .unwrap();
        materials.push(r.create_material(&MaterialDesc {
            base_color_texture: Some(t),
            ..MaterialDesc::default()
        }));
        textures.push(t);
    }
    let mut last = r.stats().textures;
    for f in 0..scaled(if rig.software { 20 } else { 60 }) {
        // Four textures visible per frame, shifting by one each frame.
        let meshes = (0..4)
            .map(|k| {
                let m = materials[((f + k) % count) as usize];
                let x = k as f32 * 2.2 - 3.3;
                MeshInstance::new(
                    plane,
                    m,
                    Mat4::from_translation(Vec3::new(x, 0.0, 0.0))
                        * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
                )
            })
            .collect();
        let scene = RenderScene {
            camera: camera(Vec3::new(0.0, 0.0, 3.0 + (f % 5) as f32), 0.0),
            meshes,
            lights: vec![sun()],
            environment: Environment::default(),
        };
        rig.render(&mut r, &scene);
        last = r.stats().textures;
    }
    eprintln!("streaming: {last:?}");
    assert!(last.evictions > 0, "nothing was demoted: {last:?}");
    assert!(last.resident_bytes < last.full_bytes, "{last:?}");
    rig.dev.wait_idle().unwrap();
    for m in materials {
        r.destroy_material(m);
    }
    for t in textures {
        r.destroy_texture(&rig.dev, t);
    }
    r.destroy(&rig.dev);
    rig.finish();
}

/// A sample-project copy run through `RuntimeCore` with hot reload on.
struct Project {
    _dir: tempfile::TempDir,
    core: RuntimeCore,
}

impl Project {
    fn open(rig: &Rig, hot_reload: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sample-project"),
            dir.path(),
        );
        let mut assets = AssetServer::new(dir.path()).unwrap();
        if hot_reload {
            assets.enable_hot_reload().unwrap();
        }
        let mut core = RuntimeCore::new(&rig.dev, rig.target.format, SIZE, assets).unwrap();
        core.open_scene("assets/scenes/main.scene").unwrap();
        assert!(core.wait_for_assets(&rig.dev, Duration::from_secs(30)));
        Self { _dir: dir, core }
    }

    fn frame(&mut self, rig: &mut Rig) {
        self.core.update_assets(&rig.dev);
        self.core.run_systems();
        let scene = self.core.extract(&rig.dev);
        rig.render(&mut self.core.renderer, &scene);
    }
}

/// Rewrites a material and a texture file again and again; each change is reimported and
/// the GPU copies are replaced in place.
#[test]
fn hot_reload() {
    let Some(mut rig) = rig() else { return };
    let mut p = Project::open(&rig, true);
    // Let the watcher settle, as the asset tests do.
    std::thread::sleep(Duration::from_millis(300));
    let root = p._dir.path().to_owned();
    let material = root.join("assets/materials/bricks.material");
    let texture = root.join("assets/textures/bricks.png");
    let material_text = std::fs::read(&material).unwrap();
    let before = p.core.assets.stats().cache_misses;
    // shortcut: the asset watcher debounces changes by 200 ms, so each reload costs about a
    // quarter second; 100 reloads would take half a minute. Upgrade with a way to trigger a
    // reload without the file watcher.
    let n = scaled(20);
    for i in 0..n {
        let c = (i % 200) as u8;
        std::fs::write(&texture, checker_png(64, 16, [c, 90, 50], [235, 215, 180])).unwrap();
        std::fs::write(&material, &material_text).unwrap();
        let end = Instant::now() + Duration::from_millis(350);
        while Instant::now() < end {
            p.frame(&mut rig);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let misses = p.core.assets.stats().cache_misses - before;
    assert!(
        misses >= u64::from(n),
        "only {misses} reimports for {n} rounds"
    );
    rig.dev.wait_idle().unwrap();
    p.core.destroy(&rig.dev);
    rig.finish();
}

/// Thousands of frames of the sample scene: nothing may grow between a warmed-up frame and the
/// last one.
#[test]
fn long_session() {
    let Some(mut rig) = rig() else { return };
    let mut p = Project::open(&rig, false);
    let (frames, warm) = if rig.software { (300, 50) } else { (3000, 500) };
    let mut snap = None;
    for f in 0..scaled(frames) {
        p.frame(&mut rig);
        if f == scaled(warm) {
            let d = rig.dev.diagnostics();
            snap = Some((d.live, d.memory.allocated_bytes));
        }
    }
    let d = rig.dev.diagnostics();
    assert_eq!(
        Some((d.live, d.memory.allocated_bytes)),
        snap,
        "live objects or memory changed between frame {} and the last",
        scaled(warm)
    );
    rig.dev.wait_idle().unwrap();
    p.core.destroy(&rig.dev);
    rig.finish();
}
