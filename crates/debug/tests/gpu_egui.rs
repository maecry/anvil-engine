//! Renders a fixed egui frame offscreen on a real device (gated by `ANVIL_GPU_TESTS=1`, validation
//! and sync validation on) and checks the pixels: opaque and premultiplied-alpha rects come out
//! in the right sRGB values, text draws, and the rest of the target is untouched.

use debug::{EguiRenderer, EguiTarget, egui};
use rhi::{
    Backend, BufferDesc, BufferUsage, ColorAttachment, CommandEncoder, Device, DeviceDesc, Format,
    Instance, InstanceDesc, LoadOp, Memory, RenderingDesc, StoreOp, TextureDesc, TextureState,
    TextureUsage, ValidationMode,
};
use rhi_vulkan::Vulkan;

const SIZE: (u32, u32) = (256, 128);

fn run(format: Format) -> Vec<u8> {
    let _ = foundation::init_logging();
    let inst = Vulkan::create_instance(&InstanceDesc {
        validation: ValidationMode::On,
        sync_validation: true,
        ..InstanceDesc::default()
    })
    .unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let mut gpu = EguiRenderer::new(&dev, format).unwrap();

    let ctx = egui::Context::default();
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SIZE.0 as f32, SIZE.1 as f32),
            )),
            ..egui::RawInput::default()
        },
        |ctx| {
            let p = ctx.layer_painter(egui::LayerId::background());
            let rect =
                |x0, x1| egui::Rect::from_min_max(egui::pos2(x0, 20.0), egui::pos2(x1, 60.0));
            p.rect_filled(rect(20.0, 100.0), 0.0, egui::Color32::from_rgb(200, 40, 40));
            p.rect_filled(
                rect(120.0, 200.0),
                0.0,
                egui::Color32::from_rgba_unmultiplied(200, 40, 40, 128),
            );
            p.text(
                egui::pos2(20.0, 80.0),
                egui::Align2::LEFT_TOP,
                "Anvil egui",
                egui::FontId::proportional(24.0),
                egui::Color32::WHITE,
            );
        },
    );
    gpu.update_textures(&dev, &out.textures_delta).unwrap();
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);

    let target = dev
        .create_texture(&TextureDesc::new_2d(
            Some("test.target"),
            SIZE.0,
            SIZE.1,
            format,
            TextureUsage::COLOR_ATTACHMENT | TextureUsage::TRANSFER_SRC,
        ))
        .unwrap();
    let rb = dev
        .create_buffer(&BufferDesc {
            label: Some("test.readback"),
            size: u64::from(SIZE.0 * SIZE.1 * 4),
            usage: BufferUsage::TRANSFER_DST,
            memory: Memory::Readback,
        })
        .unwrap();
    // Two frames: the second reuses a frame slot's buffer and a retired font texture path.
    let mut state = TextureState::Undefined;
    for _ in 0..2 {
        let frame = dev.begin_frame().unwrap();
        let mut cmd = dev.begin_commands(&frame).unwrap();
        cmd.texture_barrier(target, state, TextureState::ColorAttachment);
        cmd.begin_rendering(&RenderingDesc {
            color: &[ColorAttachment {
                texture: target,
                load: LoadOp::Clear,
                store: StoreOp::Store,
                clear: [0.0, 0.0, 0.0, 1.0],
            }],
            depth: None,
            width: SIZE.0,
            height: SIZE.1,
        });
        cmd.end_rendering();
        gpu.render(
            &dev,
            &mut cmd,
            &frame,
            &EguiTarget {
                texture: target,
                extent: SIZE,
                pixels_per_point: out.pixels_per_point,
            },
            &prims,
        )
        .unwrap();
        cmd.texture_barrier(
            target,
            TextureState::ColorAttachment,
            TextureState::TransferSrc,
        );
        cmd.copy_texture_to_buffer(target, rb, 0);
        state = TextureState::TransferSrc;
        let fin = cmd.finish().unwrap();
        dev.submit_unwindowed(fin).unwrap();
        dev.wait_idle().unwrap();
    }
    let mut pixels = vec![0u8; (SIZE.0 * SIZE.1 * 4) as usize];
    dev.read_buffer(rb, 0, &mut pixels).unwrap();
    dev.destroy_buffer(rb);
    dev.destroy_texture(target);
    gpu.destroy(&dev);
    rhi_vulkan::testing::assert_no_leaks(&mut dev);
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0), "validation messages: {c:?}");
    pixels
}

fn px(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * SIZE.0 + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2]]
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 3)
}

#[test]
fn egui_frame_renders_with_correct_srgb() {
    if std::env::var("ANVIL_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
        return;
    }
    // RGBA byte order (Rgba8Srgb): an sRGB target gets linear values from the shader.
    let p = run(Format::Rgba8Srgb);
    assert!(
        close(px(&p, 60, 40), [200, 40, 40]),
        "opaque rect {:?}",
        px(&p, 60, 40)
    );
    // 50% alpha over black: premultiplied (100, 20, 20), blended and re-encoded.
    assert!(
        close(px(&p, 160, 40), [100, 20, 20]),
        "alpha rect {:?}",
        px(&p, 160, 40)
    );
    assert_eq!(px(&p, 5, 5), [0, 0, 0], "background untouched");
    assert_eq!(px(&p, 110, 40), [0, 0, 0], "gap between the rects");
    assert_eq!(px(&p, 240, 120), [0, 0, 0], "outside every widget");
    let text_lit = (80..110)
        .flat_map(|y| (20..130).map(move |x| (x, y)))
        .any(|(x, y)| px(&p, x, y)[0] > 128);
    assert!(text_lit, "text drew white pixels");

    // A non-sRGB target gets gamma values directly: same bytes.
    let q = run(Format::Rgba8Unorm);
    assert!(
        close(px(&q, 60, 40), [200, 40, 40]),
        "unorm rect {:?}",
        px(&q, 60, 40)
    );
}
