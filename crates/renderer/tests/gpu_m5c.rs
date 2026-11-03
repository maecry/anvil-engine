//! M5c on a real device (gated by `ANVIL_GPU_TESTS=1`, validation and sync validation on): a point
//! light's cube shadow darkens the ground behind a sphere, texture streaming promotes a texture a
//! material shows up close, and debug lines draw without validation messages.

use foundation::math::{Mat4, Quat, Vec3};
use renderer::{
    Camera, DebugDraw, Environment, Light, MaterialDesc, MeshInstance, PostSettings, RenderScene,
    Renderer, RendererStats, mesh,
};
use rhi::{
    Backend, BufferDesc, BufferUsage, CommandEncoder, Device, DeviceDesc, Format, Instance,
    InstanceDesc, Memory, SwapchainHandle, SwapchainImage, TextureDesc, TextureState, TextureUsage,
    ValidationMode,
};
use rhi_vulkan::Vulkan;

const SIZE: (u32, u32) = (256, 160);

fn run(point_shadows: bool, frames: u32) -> (Vec<u8>, RendererStats) {
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
    r.set_post(PostSettings {
        ssao: false,
        taa: false,
        bloom: false,
    });
    r.set_debug_draw(DebugDraw::Bounds);
    let plane = r.create_mesh(&dev, "plane", &mesh::plane(40.0)).unwrap();
    let sphere = r
        .create_mesh(&dev, "sphere", &mesh::uv_sphere(24, 12))
        .unwrap();
    let texels: Vec<u8> = (0..1024 * 1024 * 4).map(|i| (i * 7 % 251) as u8).collect();
    let tex = r
        .create_texture(&dev, "big", &texels, 1024, 1024, true)
        .unwrap();
    let mat = r.create_material(&MaterialDesc {
        base_color_texture: Some(tex),
        ..MaterialDesc::default()
    });
    let scene = RenderScene {
        camera: Camera {
            position: Vec3::new(0.0, 3.0, 5.0),
            rotation: Quat::from_rotation_x(-0.5),
            fov_y: 1.0,
            near: 0.1,
            exposure: 1.0,
        },
        meshes: vec![
            MeshInstance::new(plane, mat, Mat4::IDENTITY),
            MeshInstance::new(
                sphere,
                mat,
                Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0)),
            ),
        ],
        lights: vec![Light::Point {
            position: Vec3::new(0.0, 2.5, 2.5),
            color: Vec3::ONE,
            intensity: 60.0,
            range: 12.0,
            cast_shadows: point_shadows,
        }],
        environment: Environment::default(),
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
    for _ in 0..frames {
        let frame = dev.begin_frame().unwrap();
        let mut cmd = dev.begin_commands(&frame).unwrap();
        r.debug_lines().line(Vec3::ZERO, Vec3::Y, Vec3::ONE);
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
        dev.wait_idle().unwrap();
    }
    let stats = r.stats();
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

#[test]
fn point_shadow_streaming_and_debug_lines() {
    if std::env::var("ANVIL_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
        return;
    }
    let (lit, s0) = run(false, 40);
    let (shadowed, s1) = run(true, 40);
    assert_eq!((s0.point_shadows, s1.point_shadows), (0, 1));
    // The sphere shadows the ground behind it: the image gets darker, never brighter.
    let sum = |v: &[u8]| v.iter().map(|&b| u64::from(b)).sum::<u64>();
    assert!(
        sum(&shadowed) + 2000 < sum(&lit),
        "{} vs {}",
        sum(&shadowed),
        sum(&lit)
    );
    // The big texture is shown up close, so it was refined beyond its 64 texel start level
    // (21 844 bytes resident) and uploads happened.
    let t = s1.textures;
    assert!(t.resident_bytes > 4 * 21_844, "{t:?}");
    assert!(t.resident_bytes <= t.budget_bytes && t.resident_bytes <= t.full_bytes);
}
