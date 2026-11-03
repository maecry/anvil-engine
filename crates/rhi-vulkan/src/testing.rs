//! Helpers for tests of code that owns Vulkan resources (ADR-0015 §2).

use rhi::Device as _;

use crate::VulkanDevice;

/// Asserts that `dev` holds nothing: after a short run of empty frames that retires every
/// deferred destroy, all live counts, `pending_destroys` and `allocated_bytes` are zero.
/// Call after the renderer, runtime or editor under test has been torn down. The device's own
/// internal objects (bindless set, frame slots, timeline) are not counted by diagnostics.
///
/// # Panics
/// On any non-zero count.
pub fn assert_no_leaks(dev: &mut VulkanDevice) {
    dev.retire_pending().expect("retire_pending");
    let d = dev.diagnostics();
    assert_eq!(
        d.live,
        rhi::LiveObjects::default(),
        "leaked objects: {d:#?}"
    );
    assert_eq!(d.pending_destroys, 0, "pending destroys: {d:#?}");
    assert_eq!(d.memory.allocated_bytes, 0, "leaked memory: {d:#?}");
}
