//! Graph execution on a real device, gated by `ANVIL_GPU_TESTS=1` (validation and sync
//! validation on): barriers and pooling must produce zero validation messages.

use render_graph::{ColorTarget, DepthTarget, GraphBuilder, Pass, RenderGraph, TransientDesc};
use rhi::{
    Backend, CommandEncoder, Device, DeviceDesc, Format, Instance, InstanceDesc, LoadOp, StoreOp,
    TextureDesc, TextureState, TextureUsage, ValidationMode,
};
use rhi_vulkan::Vulkan;

#[test]
fn passes_barriers_and_pool_reuse_are_validation_clean() {
    if std::env::var("ANVIL_GPU_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
        return;
    }
    let _ = foundation::init_logging();
    let inst = Vulkan::create_instance(&InstanceDesc {
        validation: ValidationMode::On,
        sync_validation: true,
        ..InstanceDesc::default()
    })
    .unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let fif = dev.diagnostics().frames_in_flight;
    let mut graph = RenderGraph::new(fif);
    // A persistent "history" texture: written every frame, sampled the next one.
    let history = dev
        .create_texture(&TextureDesc {
            label: Some("test.history"),
            width: 32,
            height: 32,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
            mip_levels: 1,
            array_layers: 1,
            cube: false,
        })
        .unwrap();
    graph.track(history, TextureState::Undefined);
    let color = |w| TransientDesc {
        width: w,
        height: w,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
    };

    let mut created = Vec::new();
    for i in 0..8u32 {
        let frame = dev.begin_frame().unwrap();
        // The slot's previous frame retired: its per-pass GPU times are readable (ADR-0013 §2).
        if dev.timestamp_info().is_some() && i >= fif {
            let t = graph
                .gpu_timings(&dev, frame.slot)
                .unwrap()
                .expect("timings");
            let names: Vec<_> = t.passes.iter().map(|(n, _)| n.as_ref()).collect();
            assert_eq!(names, ["a", "b", "c", "export barriers"]);
            assert!(t.passes.iter().all(|(_, ms)| *ms >= 0.0) && t.total_ms >= 0.0);
            assert_eq!(t.frame, frame.frame - u64::from(fif));
        }
        let mut cmd = dev.begin_commands(&frame).unwrap();
        let mut b = GraphBuilder::<rhi_vulkan::VulkanDevice>::new();
        let hist = b.import("history", history, (32, 32), None);
        let (a, depth, scratch) = (
            b.transient("a", color(32)),
            b.transient(
                "depth",
                TransientDesc {
                    format: Format::D32Sfloat,
                    usage: TextureUsage::DEPTH_ATTACHMENT,
                    ..color(32)
                },
            ),
            b.transient("scratch", color(32)),
        );
        let dead = b.transient("dead", color(32));
        let clear = ColorTarget::clear([0.1, 0.2, 0.3, 1.0]);
        let drawn = std::cell::Cell::new(0);
        b.pass(
            Pass::new("a").color(a, clear).depth(
                depth,
                DepthTarget {
                    load: LoadOp::Clear,
                    store: StoreOp::DontCare,
                    clear: 0.0,
                },
            ),
            |_| {
                drawn.set(drawn.get() + 1);
                Ok(())
            },
        );
        // Reads last frame's history (skipped on frame 0, where it is Undefined).
        let mut p = Pass::new("b").read(a).color(scratch, clear);
        if i > 0 {
            p = p.read(hist);
        }
        b.pass(p, |ctx| {
            assert!(ctx.texture_index(a).is_ok());
            drawn.set(drawn.get() + 1);
            Ok(())
        });
        b.pass(Pass::new("c").read(scratch).color(hist, clear), |_| {
            drawn.set(drawn.get() + 1);
            Ok(())
        });
        b.pass(Pass::new("culled").read(a).color(dead, clear), |_| {
            panic!("culled pass must not run")
        });
        b.export(hist, TextureState::ShaderRead);
        let compiled = graph.compile(b).unwrap();
        assert_eq!(compiled.stats().culled, 1);
        let desc = compiled.describe();
        assert!(desc.passes[3].culled && !desc.passes[0].culled);
        assert!(desc.to_dot().contains("culled") && desc.to_json().contains("\"barriers\""));
        let stats = graph.execute(compiled, &dev, &mut cmd, &frame).unwrap();
        assert_eq!(drawn.get(), 3);
        created.push(stats.textures_created);
        dev.submit_unwindowed(cmd.finish().unwrap()).unwrap();
    }
    // Two frames in flight need two sets of transients; after that the pool serves everything.
    assert!(created[..fif as usize].iter().sum::<u32>() > 0);
    assert_eq!(
        created[fif as usize..].iter().sum::<u32>(),
        0,
        "{created:?}"
    );

    dev.wait_idle().unwrap();
    graph.destroy(&dev);
    dev.destroy_texture(history);
    rhi_vulkan::testing::assert_no_leaks(&mut dev);
    drop(dev);
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0), "validation messages: {c:?}");
}
