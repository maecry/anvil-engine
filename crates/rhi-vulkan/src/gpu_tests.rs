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
