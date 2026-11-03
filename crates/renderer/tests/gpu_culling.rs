//! GPU-driven culling against the CPU path on a real device (gated by `ANVIL_GPU_TESTS=1`,
//! validation and sync validation on). A wall hides most of a field of spheres: with LOD off the
//! GPU-culled image must be pixel-identical to the CPU path's, occlusion culling must reject
//! instances, and a static scene must upload nothing after the first frame.

use foundation::math::{Mat4, Quat, Vec3};
use renderer::{
    Camera, Environment, Light, MaterialDesc, MeshInstance, PostSettings, RenderScene, Renderer,
    RendererStats, mesh,
};
use rhi::{
    Backend, BufferDesc, BufferUsage, CommandEncoder, Device, DeviceDesc, Format, Instance,
    InstanceDesc, Memory, SwapchainHandle, SwapchainImage, TextureDesc, TextureState, TextureUsage,
    ValidationMode,
};
use rhi_vulkan::Vulkan;

const SIZE: (u32, u32) = (320, 200);

fn scene(wall_height: f32, ids: &Ids) -> RenderScene {
    let mut meshes = vec![MeshInstance::new(
        ids.plane,
        ids.mat,
        Mat4::from_translation(Vec3::ZERO),
    )];
    // The wall: a flat slab across the whole view at z = 0.
    meshes.push(MeshInstance::new(
        ids.cube,
        ids.mat,
        Mat4::from_scale_rotation_translation(
            Vec3::new(60.0, wall_height, 1.0),
            Quat::IDENTITY,
            Vec3::new(0.0, wall_height * 0.5, 0.0),
        ),
    ));
    // Spheres behind the wall (hidden) and a few in front (visible).
    for i in 0..400u32 {
        let (x, z) = ((i % 20) as f32 * 2.5 - 24.0, -3.0 - (i / 20) as f32 * 2.5);
        meshes.push(MeshInstance::new(
            ids.sphere,
            ids.mat,
            Mat4::from_translation(Vec3::new(x, 0.8, z)),
        ));
    }
    for i in 0..5u32 {
        meshes.push(MeshInstance::new(
            ids.sphere,
            ids.mat,
            Mat4::from_translation(Vec3::new(i as f32 * 2.0 - 4.0, 0.6, 5.0)),
        ));
    }
    RenderScene {
        camera: Camera {
            position: Vec3::new(0.0, 2.0, 12.0),
            rotation: Quat::from_rotation_x(-0.05),
            fov_y: 1.0,
            near: 0.1,
            exposure: 1.0,
        },
        meshes,
        lights: vec![Light::Directional {
            direction: Vec3::new(0.3, -1.0, -0.4),
            color: Vec3::ONE,
            intensity: 3.0,
            cast_shadows: true,
        }],
        environment: Environment::default(),
    }
}

struct Ids {
    plane: renderer::MeshId,
    cube: renderer::MeshId,
    sphere: renderer::MeshId,
    mat: renderer::MaterialId,
}

/// Renders `frames` frames of the scene and returns the last image and the renderer's stats.
fn run(
    gpu_culling: bool,
    lod_pixels: f32,
    wall_height: f32,
    frames: u32,
) -> (Vec<u8>, Vec<RendererStats>) {
    let _ = foundation::init_logging();
    let inst = Vulkan::create_instance(&InstanceDesc {
        validation: ValidationMode::On,
        sync_validation: true,
        ..InstanceDesc::default()
    })
    .unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let format = Format::Rgba8Unorm;
    let mut r = Renderer::new(&dev, format, SIZE).unwrap();
    r.set_gpu_culling(gpu_culling);
    r.set_lod_pixels(lod_pixels);
    r.set_post(PostSettings {
        ssao: false,
        taa: false,
        bloom: false,
    });
    let ids = Ids {
        plane: r.create_mesh(&dev, "plane", &mesh::plane(200.0)).unwrap(),
        cube: r.create_mesh(&dev, "cube", &mesh::cube()).unwrap(),
        sphere: r
            .create_mesh(&dev, "sphere", &mesh::uv_sphere(24, 12))
            .unwrap(),
        mat: r.create_material(&MaterialDesc::default()),
    };
    let target = dev
        .create_texture(&TextureDesc {
            label: Some("test.target"),
            width: SIZE.0,
            height: SIZE.1,
            format,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::TRANSFER_SRC,
            mip_levels: 1,
            array_layers: 1,
            cube: false,
        })
        .unwrap();
    let image = SwapchainImage {
        swapchain: SwapchainHandle::NULL,
        texture: target,
        index: 0,
        extent: SIZE,
        format,
    };
    let rb = dev
        .create_buffer(&BufferDesc {
            label: Some("test.readback"),
            size: u64::from(SIZE.0 * SIZE.1 * 4),
            usage: BufferUsage::TRANSFER_DST,
            memory: Memory::Readback,
        })
        .unwrap();
    let scene = scene(wall_height, &ids);
    let mut stats = Vec::new();
    for _ in 0..frames {
        let frame = dev.begin_frame().unwrap();
        let mut cmd = dev.begin_commands(&frame).unwrap();
        r.render(
            &dev,
            &mut cmd,
            &frame,
            &scene,
            &image,
            TextureState::TransferSrc,
        )
        .unwrap();
        cmd.copy_texture_to_buffer(target, rb, 0);
        let fin = cmd.finish().unwrap();
        dev.submit_unwindowed(fin).unwrap();
        // One shared target and readback buffer (no swapchain acquire semaphore), so frames
        // must not overlap; this also keeps older sync-validation layers (CI) from flagging
        // slot reuse they cannot see through the timeline host wait.
        dev.wait_idle().unwrap();
        stats.push(r.stats());
    }
    dev.wait_idle().unwrap();
    let mut out = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
    dev.read_buffer(rb, 0, &mut out).unwrap();
    dev.destroy_buffer(rb);
    dev.destroy_texture(target);
    r.destroy(&dev);
    rhi_vulkan::testing::assert_no_leaks(&mut dev);
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0), "validation messages: {c:?}");
    (out, stats)
}

fn enabled() -> bool {
    let on = std::env::var("ANVIL_GPU_TESTS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
    }
    on
}

#[test]
fn gpu_culling_matches_cpu_and_culls_occluded_instances() {
    if !enabled() {
        return;
    }
    let (cpu, cpu_stats) = run(false, 0.0, 12.0, 8);
    let (gpu, gpu_stats) = run(true, 0.0, 12.0, 8);
    assert_eq!(cpu.len(), gpu.len());
    let differing = cpu.iter().zip(&gpu).filter(|(a, b)| a != b).count();
    assert_eq!(
        differing, 0,
        "GPU culling changed {differing} bytes of the image"
    );
    assert!(cpu.iter().any(|&b| b != 0), "rendered something");

    let (c, g) = (cpu_stats.last().unwrap(), gpu_stats.last().unwrap());
    eprintln!("gpu cull counters: {:?}, instances {}", g.cull, g.instances);
    assert!(!c.gpu_culling && g.gpu_culling);
    // The wall hides the sphere field: phase 2 rejects most of what the frustum keeps.
    assert!(
        g.cull.occlusion_culled > 100,
        "occlusion culled {}",
        g.cull.occlusion_culled
    );
    assert!(g.cull.visible < 50, "visible {}", g.cull.visible);
    assert_eq!(
        u64::from(g.cull.visible + g.cull.occlusion_culled + g.cull.frustum_culled),
        u64::from(g.instances),
        "every drawable instance is accounted for"
    );
    // A static scene: everything uploads once, then nothing; steady state draws only last
    // frame's visible set in phase 1 (no phase 2 draws).
    assert_eq!(gpu_stats[0].instances_uploaded, g.instances);
    assert!(gpu_stats[1..].iter().all(|s| s.instances_uploaded == 0));
    assert_eq!(
        g.cull.phase2_draws, 0,
        "a static view needs no phase 2 draws"
    );
    assert!(
        g.cull.camera_draws < c.draws / 4,
        "culled draws {} vs CPU {}",
        g.cull.camera_draws,
        c.draws
    );
}

#[test]
fn lod_reduces_triangles_and_keeps_the_picture_close() {
    if !enabled() {
        return;
    }
    // No wall: everything in the field is visible, so the LOD choice decides the triangle count.
    let (full, s0) = run(true, 0.0, 0.1, 6);
    let (lod, s1) = run(true, 2.0, 0.1, 6);
    let (t0, t1) = (
        s0.last().unwrap().cull.triangles,
        s1.last().unwrap().cull.triangles,
    );
    assert!(t1 < t0, "LOD: {t1} triangles vs {t0}");
    let diff: u64 = full
        .iter()
        .zip(&lod)
        .map(|(a, b)| u64::from(a.abs_diff(*b)))
        .sum();
    let mean = diff as f64 / full.len() as f64;
    assert!(mean < 2.0, "mean absolute difference {mean}");
}
