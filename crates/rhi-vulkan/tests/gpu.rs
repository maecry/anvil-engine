//! Headless integration tests, gated by `ANVIL_GPU_TESTS=1` (validation On, sync validation on).

use rhi::{
    Backend, BlendState, CullMode, Device, DeviceDesc, Format, FrontFace, GpuKind,
    GraphicsPipelineDesc, Instance, InstanceDesc, PrimitiveTopology, RhiError, ShaderModuleDesc,
    ShaderStageDesc, SwapchainHandle, ValidationMode,
};
use rhi_vulkan::Vulkan;

// Minimal valid shaders (assembled with spirv-as, validated with spirv-val for Vulkan 1.3).
const VERT: [u32; 29] = [
    0x07230203, 0x00010600, 0x00070000, 0x00000005, 0x00000000, 0x00020011, 0x00000001, 0x0003000e,
    0x00000000, 0x00000001, 0x0005000f, 0x00000000, 0x00000001, 0x6e69616d, 0x00000000, 0x00020013,
    0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00050036, 0x00000002, 0x00000001, 0x00000000,
    0x00000003, 0x000200f8, 0x00000004, 0x000100fd, 0x00010038,
];
const FRAG: [u32; 32] = [
    0x07230203, 0x00010600, 0x00070000, 0x00000005, 0x00000000, 0x00020011, 0x00000001, 0x0003000e,
    0x00000000, 0x00000001, 0x0005000f, 0x00000004, 0x00000001, 0x6e69616d, 0x00000000, 0x00030010,
    0x00000001, 0x00000007, 0x00020013, 0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00050036,
    0x00000002, 0x00000001, 0x00000000, 0x00000003, 0x000200f8, 0x00000004, 0x000100fd, 0x00010038,
];

fn enabled() -> bool {
    let on = std::env::var("ANVIL_GPU_TESTS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set ANVIL_GPU_TESTS=1 to run GPU tests");
    }
    if on {
        let _ = foundation::init_logging();
    }
    on
}

fn desc() -> InstanceDesc {
    InstanceDesc {
        validation: ValidationMode::On,
        sync_validation: true,
        ..InstanceDesc::default()
    }
}

#[test]
fn create_instance_and_device_headless() {
    if !enabled() {
        return;
    }
    let _ = foundation::init_logging();
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    assert!(!dev.info().name.is_empty());
    assert!(dev.info().api_version >= (1, 3, 0));
    assert_ne!(dev.info().kind, GpuKind::Other);
    drop(dev);
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0));
}

#[test]
fn instance_twice_and_drop_orders() {
    if !enabled() {
        return;
    }
    let a = Vulkan::create_instance(&desc()).unwrap();
    let b = Vulkan::create_instance(&desc()).unwrap();
    let da = a.create_device(&DeviceDesc::default(), None).unwrap();
    let db = b.create_device(&DeviceDesc::default(), None).unwrap();
    drop(a); // instance first, device keeps it alive
    drop(da);
    drop(db);
    drop(b);
}

#[test]
fn preferred_gpu_fallback_and_bad_frames_in_flight() {
    if !enabled() {
        return;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    inst.create_device(
        &DeviceDesc {
            preferred_gpu: Some("zzz-no-match".into()),
            ..DeviceDesc::default()
        },
        None,
    )
    .unwrap();
    for n in [0, 4] {
        let r = inst.create_device(
            &DeviceDesc {
                frames_in_flight: n,
                ..DeviceDesc::default()
            },
            None,
        );
        assert!(matches!(r, Err(RhiError::InvalidDescriptor(_))));
    }
}

#[test]
fn shutdown_idempotent() {
    if !enabled() {
        return;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    dev.shutdown().unwrap();
    dev.shutdown().unwrap();
}

#[test]
fn shader_module_and_pipeline_lifecycle() {
    if !enabled() {
        return;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let v = dev
        .create_shader_module(&ShaderModuleDesc {
            spirv: &VERT,
            label: Some("test.vert"),
        })
        .unwrap();
    let f = dev
        .create_shader_module(&ShaderModuleDesc {
            spirv: &FRAG,
            label: None,
        })
        .unwrap();
    assert!(matches!(
        dev.create_shader_module(&ShaderModuleDesc {
            spirv: &[1, 2, 3],
            label: None
        }),
        Err(RhiError::InvalidDescriptor(_))
    ));
    let formats = [Format::Bgra8Srgb];
    let p = dev
        .create_graphics_pipeline(&GraphicsPipelineDesc {
            label: Some("test.pipeline"),
            vertex: ShaderStageDesc {
                module: v,
                entry: "main",
            },
            fragment: ShaderStageDesc {
                module: f,
                entry: "main",
            },
            color_formats: &formats,
            depth_format: None,
            topology: PrimitiveTopology::TriangleList,
            cull: CullMode::None,
            front_face: FrontFace::CounterClockwise,
            blend: &[BlendState::default()],
        })
        .unwrap();
    let d = dev.diagnostics();
    assert_eq!(
        (d.live.shader_modules, d.live.pipelines, d.pending_destroys),
        (2, 1, 0)
    );

    dev.destroy_pipeline(p);
    dev.destroy_shader_module(v);
    dev.destroy_shader_module(f);
    dev.destroy_pipeline(p); // stale: warn + no-op
    let d = dev.diagnostics();
    assert_eq!(
        (d.live.shader_modules, d.live.pipelines, d.pending_destroys),
        (0, 0, 3)
    );

    // Stale module cannot be used for a new pipeline.
    let r = dev.create_graphics_pipeline(&GraphicsPipelineDesc {
        label: None,
        vertex: ShaderStageDesc {
            module: v,
            entry: "main",
        },
        fragment: ShaderStageDesc {
            module: f,
            entry: "main",
        },
        color_formats: &formats,
        depth_format: None,
        topology: PrimitiveTopology::TriangleList,
        cull: CullMode::None,
        front_face: FrontFace::CounterClockwise,
        blend: &[],
    });
    assert!(matches!(
        r,
        Err(RhiError::StaleHandle {
            kind: "shader_module",
            ..
        })
    ));

    // Deferred items retire a few frames later, not at destroy time.
    for _ in 0..4 {
        dev.begin_frame().unwrap();
    }
    // No submits happened, so the tag (last_submitted + 1) is not reached yet.
    assert_eq!(dev.diagnostics().pending_destroys, 3);
    let t = dev.begin_frame().unwrap();
    let c = dev.begin_commands(&t).unwrap();
    use rhi::CommandEncoder;
    let fin = c.finish().unwrap();
    drop(fin);
    assert_eq!(dev.diagnostics().validation.errors, 0);
}

#[test]
fn stale_swapchain_handle_reports_context() {
    if !enabled() {
        return;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let r = dev.swapchain_format(SwapchainHandle::NULL);
    assert!(matches!(
        r,
        Err(RhiError::StaleHandle {
            kind: "swapchain",
            index: 0,
            generation: 0
        })
    ));
    dev.destroy_swapchain(SwapchainHandle::NULL); // no-op
}
