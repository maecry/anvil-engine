//! GPU tests that need crate-private access (gated by `ANVIL_GPU_TESTS=1`).

use ash::vk;
use rhi::{
    Backend, CommandEncoder, Device, DeviceDesc, Instance, InstanceDesc, RhiError, ValidationMode,
};

use crate::Vulkan;

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
fn headless_frames_n1_n2_n3() {
    if !enabled() {
        return;
    }
    let _ = foundation::init_logging();
    for n in 1..=3u32 {
        let inst = Vulkan::create_instance(&desc()).unwrap();
        let mut dev = inst
            .create_device(
                &DeviceDesc {
                    frames_in_flight: n,
                    ..DeviceDesc::default()
                },
                None,
            )
            .unwrap();
        for i in 1..=100u64 {
            let t = dev.begin_frame().unwrap();
            let cmds = dev.begin_commands(&t).unwrap();
            let fin = cmds.finish().unwrap();
            let v = dev.frames.submit_unwindowed(fin).unwrap();
            assert_eq!(v, i);
        }
        dev.wait_idle().unwrap();
        let d = dev.diagnostics();
        assert_eq!(
            (d.frame, d.gpu_completed_frame, d.frames_in_flight),
            (100, 100, n)
        );
        assert_eq!(d.validation.errors, 0);
        assert_eq!(d.validation.warnings, 0);
        // begin_frame after idle does not block.
        dev.begin_frame().unwrap();
    }
}

#[test]
fn encoder_misuse_surfaces_at_finish() {
    if !enabled() {
        return;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let t = dev.begin_frame().unwrap();
    let mut c = dev.begin_commands(&t).unwrap();
    assert!(matches!(
        dev.begin_commands(&t),
        Err(RhiError::InvalidDescriptor(_))
    ));
    c.draw(3, 1, 0, 0);
    assert!(matches!(c.finish(), Err(RhiError::InvalidDescriptor(_))));
    // Encoder outliving its frame.
    let t = dev.begin_frame().unwrap();
    let c = dev.begin_commands(&t).unwrap();
    dev.begin_frame().unwrap();
    dev.begin_frame().unwrap(); // same slot again: epoch bumped
    assert!(matches!(c.finish(), Err(RhiError::InvalidDescriptor(_))));
    assert_eq!(dev.diagnostics().validation.errors, 0);
}

#[test]
fn validation_messages_are_counted() {
    if !enabled() {
        return;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    assert_eq!(inst.validation_counts().errors, 0);
    let du = inst.shared.debug_utils.as_ref().expect("debug utils");
    let msg = c"deliberate test error";
    let data = vk::DebugUtilsMessengerCallbackDataEXT::default()
        .message(msg)
        .message_id_name(c"anvil-test");
    // SAFETY: the instance is alive and `data` outlives the call.
    unsafe {
        du.submit_debug_utils_message(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
            vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
            &data,
        );
    }
    assert_eq!(inst.validation_counts().errors, 1);
}

fn lowered_device(inst: &crate::VulkanInstance, images: u32, samplers: u32) -> crate::VulkanDevice {
    crate::device::create_device_with(
        &inst.shared,
        &DeviceDesc::default(),
        None,
        Some(crate::bindings::BindlessLimits {
            sampled_images: images,
            samplers,
        }),
    )
    .unwrap()
}

#[test]
fn pending_uploads_drain() {
    if !enabled() {
        return;
    }
    use rhi::{BufferDesc, BufferUsage, Memory};
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let mut dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let b = dev
        .create_buffer(&BufferDesc {
            label: Some("drain"),
            size: 64,
            usage: BufferUsage::TRANSFER_DST,
            memory: Memory::GpuOnly,
        })
        .unwrap();
    // Queued outside any frame.
    dev.upload_buffer(b, 0, &[1; 16]).unwrap();
    dev.upload_buffer(b, 16, &[2; 16]).unwrap();
    assert_eq!(dev.shared.pending_uploads.lock().unwrap().len(), 2);
    let before = dev.diagnostics().pending_destroys;
    let t = dev.begin_frame().unwrap();
    let fin = dev.begin_commands(&t).unwrap().finish().unwrap();
    dev.submit_unwindowed(fin).unwrap();
    assert_eq!(dev.shared.pending_uploads.lock().unwrap().len(), 0);
    // Both staging buffers were handed to deferred destruction by that submit.
    assert_eq!(dev.diagnostics().pending_destroys, before + 2);
    // An empty queue adds nothing; the earlier staging buffers may already have retired.
    let t = dev.begin_frame().unwrap();
    let fin = dev.begin_commands(&t).unwrap().finish().unwrap();
    dev.submit_unwindowed(fin).unwrap();
    assert!(dev.diagnostics().pending_destroys <= before + 2);
    dev.wait_idle().unwrap();
    dev.destroy_buffer(b);
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0));
}

#[test]
fn bindless_capacity_exceeded() {
    if !enabled() {
        return;
    }
    use rhi::{AddressMode, Filter, Format, SamplerDesc, TextureDesc, TextureUsage};
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let dev = lowered_device(&inst, 4, 2);
    let d = dev.diagnostics();
    assert_eq!(
        (d.bindless.image_capacity, d.bindless.sampler_capacity),
        (4, 2)
    );
    let tex = TextureDesc {
        label: Some("cap"),
        width: 4,
        height: 4,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::SAMPLED,
    };
    let held: Vec<_> = (0..4).map(|_| dev.create_texture(&tex).unwrap()).collect();
    let allocated = dev.diagnostics().memory.allocated_bytes;
    assert!(matches!(
        dev.create_texture(&tex),
        Err(RhiError::Unsupported { .. })
    ));
    // Non-sampled textures may use larger indices; the failed one leaked nothing.
    let attach = dev
        .create_texture(&TextureDesc {
            usage: TextureUsage::COLOR_ATTACHMENT,
            ..tex
        })
        .unwrap();
    assert!(attach.index() >= 4);
    dev.destroy_texture(attach);
    let s = SamplerDesc {
        label: None,
        mag_filter: Filter::Linear,
        min_filter: Filter::Linear,
        address_u: AddressMode::Repeat,
        address_v: AddressMode::Repeat,
        address_w: AddressMode::Repeat,
    };
    let s1 = dev.create_sampler(&s).unwrap();
    let s2 = dev.create_sampler(&s).unwrap();
    assert!(matches!(
        dev.create_sampler(&s),
        Err(RhiError::Unsupported { .. })
    ));
    let d = dev.diagnostics();
    assert_eq!((d.live.textures, d.live.samplers), (4, 2));
    assert!(d.memory.allocated_bytes >= allocated);
    for h in held {
        dev.destroy_texture(h);
    }
    dev.destroy_sampler(s1);
    dev.destroy_sampler(s2);
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0));
}

#[test]
fn global_bindings_created_and_reported() {
    if !enabled() {
        return;
    }
    let inst = Vulkan::create_instance(&desc()).unwrap();
    let dev = inst.create_device(&DeviceDesc::default(), None).unwrap();
    let mut p12 = vk::PhysicalDeviceVulkan12Properties::default();
    let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut p12);
    // SAFETY: the physical device belongs to this live instance.
    unsafe {
        inst.shared
            .raw
            .get_physical_device_properties2(dev.shared.physical, &mut p2)
    };
    let l_img = p12
        .max_per_stage_descriptor_update_after_bind_sampled_images
        .min(p12.max_descriptor_set_update_after_bind_sampled_images);
    let l_smp = p12
        .max_per_stage_descriptor_update_after_bind_samplers
        .min(p12.max_descriptor_set_update_after_bind_samplers);
    let b = dev.diagnostics().bindless;
    assert_eq!(b.image_capacity, l_img.min(65536));
    assert_eq!(b.sampler_capacity, l_smp.min(1024));
    assert!(b.image_capacity > 0 && b.sampler_capacity > 0);
    assert_eq!((b.image_high_water, b.sampler_high_water), (0, 0));
    eprintln!(
        "bindless capacity: images {} samplers {}",
        b.image_capacity, b.sampler_capacity
    );
}
