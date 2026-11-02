//! `vk::Result` -> `RhiError` translation (LAYER-004 §11).

use ash::vk;
use foundation::Handle;
use rhi::RhiError;

/// `op` is the real Vulkan function name (ADR-0009 §5).
pub(crate) fn map_vk(op: &'static str, r: vk::Result) -> RhiError {
    match r {
        vk::Result::ERROR_DEVICE_LOST => RhiError::DeviceLost,
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => RhiError::OutOfDeviceMemory,
        vk::Result::ERROR_OUT_OF_HOST_MEMORY => RhiError::OutOfHostMemory,
        vk::Result::ERROR_SURFACE_LOST_KHR => RhiError::SurfaceLost,
        vk::Result::ERROR_LAYER_NOT_PRESENT
        | vk::Result::ERROR_EXTENSION_NOT_PRESENT
        | vk::Result::ERROR_FEATURE_NOT_PRESENT
        | vk::Result::ERROR_INCOMPATIBLE_DRIVER => RhiError::Unsupported {
            what: format!("{op} failed with {r:?}"),
        },
        other => RhiError::Backend {
            op,
            code: other.as_raw(),
        },
    }
}

pub(crate) fn stale<T>(kind: &'static str, h: Handle<T>) -> RhiError {
    RhiError::StaleHandle {
        kind,
        index: h.index(),
        generation: h.generation(),
    }
}

pub(crate) fn invalid(msg: impl Into<String>) -> RhiError {
    RhiError::InvalidDescriptor(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_table() {
        use vk::Result as R;
        assert!(matches!(
            map_vk("x", R::ERROR_DEVICE_LOST),
            RhiError::DeviceLost
        ));
        assert!(matches!(
            map_vk("x", R::ERROR_OUT_OF_DEVICE_MEMORY),
            RhiError::OutOfDeviceMemory
        ));
        assert!(matches!(
            map_vk("x", R::ERROR_OUT_OF_HOST_MEMORY),
            RhiError::OutOfHostMemory
        ));
        assert!(matches!(
            map_vk("x", R::ERROR_SURFACE_LOST_KHR),
            RhiError::SurfaceLost
        ));
        for r in [
            R::ERROR_LAYER_NOT_PRESENT,
            R::ERROR_EXTENSION_NOT_PRESENT,
            R::ERROR_FEATURE_NOT_PRESENT,
            R::ERROR_INCOMPATIBLE_DRIVER,
        ] {
            assert!(matches!(map_vk("x", r), RhiError::Unsupported { .. }));
        }
        assert!(matches!(
            map_vk("vkQueueSubmit2", R::ERROR_INITIALIZATION_FAILED),
            RhiError::Backend { op: "vkQueueSubmit2", code } if code == R::ERROR_INITIALIZATION_FAILED.as_raw()
        ));
    }
}
