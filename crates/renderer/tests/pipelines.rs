//! Pipeline-variant smoke test (ADR-0015 §5, FR-SHD-009), gated by `ANVIL_GPU_TESTS=1`:
//! `Renderer::new` builds every pipeline variant (opaque/masked x one/two sided for the prepass,
//! forward, G-buffer, transparent and shadow stages, the compute passes, tonemap, debug lines)
//! eagerly, so creating one per target format on the current device proves they all compile
//! there (RADV, lavapipe). Validation stays silent and teardown leaks nothing.

use renderer::Renderer;
use rhi::{Backend, Device, DeviceDesc, Format, Instance, InstanceDesc, ValidationMode};
use rhi_vulkan::{Vulkan, testing::assert_no_leaks};

#[test]
fn every_pipeline_variant_builds_on_this_device() {
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
    for format in [Format::Rgba8Srgb, Format::Bgra8Srgb, Format::Rgba8Unorm] {
        let r = Renderer::new(&dev, format, (64, 64)).unwrap();
        let pipelines = dev.diagnostics().live.pipelines;
        // 5 staged variant sets of 4, plus compute, post, tonemap and debug pipelines.
        assert!(pipelines >= 24, "{format:?}: only {pipelines} pipelines");
        eprintln!("{format:?}: {pipelines} pipelines");
        r.destroy(&dev);
        assert_no_leaks(&mut dev);
    }
    let c = inst.validation_counts();
    assert_eq!((c.errors, c.warnings), (0, 0), "validation messages: {c:?}");
}
