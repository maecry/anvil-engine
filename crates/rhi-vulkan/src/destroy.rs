//! Deferred destruction keyed on timeline value (LAYER-004 §7.5, MODULE-004 §6.2).

use std::collections::VecDeque;

use ash::vk;
use gpu_allocator::vulkan::Allocation;
use rhi::{SamplerHandle, TextureHandle};

use crate::device::DeviceShared;
use crate::lock;
use crate::write;

pub(crate) enum Doomed {
    ShaderModule(vk::ShaderModule),
    /// The pipeline layout is global (MODULE-007), so a pipeline owns nothing else.
    Pipeline {
        pipeline: vk::Pipeline,
    },
    /// A buffer and its memory; `staging` marks internal upload staging buffers (logging only).
    Buffer {
        buffer: vk::Buffer,
        allocation: Allocation,
        staging: bool,
    },
    /// Image, default view and memory; `slot` is released (bindless index reusable) afterwards.
    Image {
        image: vk::Image,
        view: vk::ImageView,
        allocation: Allocation,
        slot: TextureHandle,
    },
    Sampler {
        sampler: vk::Sampler,
        slot: SamplerHandle,
    },
}

impl Doomed {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Doomed::ShaderModule(_) => "shader_module",
            Doomed::Pipeline { .. } => "pipeline",
            Doomed::Buffer { staging: true, .. } => "staging",
            Doomed::Buffer { .. } => "buffer",
            Doomed::Image { .. } => "image",
            Doomed::Sampler { .. } => "sampler",
        }
    }
}

#[derive(Default)]
pub(crate) struct DeferredQueue {
    items: VecDeque<(u64, Doomed)>,
}

impl DeferredQueue {
    pub(crate) fn push(&mut self, tag: u64, d: Doomed) {
        debug_assert!(
            self.items.back().is_none_or(|(t, _)| *t <= tag),
            "deferred tags must be non-decreasing"
        );
        self.items.push_back((tag, d));
    }

    pub(crate) fn pop_ready(&mut self, completed: u64) -> Option<Doomed> {
        if self.items.front()?.0 <= completed {
            return self.items.pop_front().map(|(_, d)| d);
        }
        None
    }

    pub(crate) fn pop_any(&mut self) -> Option<Doomed> {
        self.items.pop_front().map(|(_, d)| d)
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}

/// # Safety
/// The GPU must no longer use the object (tag retired, or device idle); call once per object.
/// No pool lock or the `deferred` lock may be held by the caller.
pub(crate) unsafe fn destroy_doomed(s: &DeviceShared, d: Doomed) {
    // SAFETY: forwarded from the caller's contract; entries are popped so destruction is
    // exactly-once.
    unsafe {
        match d {
            Doomed::ShaderModule(m) => s.raw.destroy_shader_module(m, None),
            Doomed::Pipeline { pipeline } => s.raw.destroy_pipeline(pipeline, None),
            Doomed::Buffer {
                buffer, allocation, ..
            } => {
                s.raw.destroy_buffer(buffer, None);
                s.free(allocation);
            }
            Doomed::Image {
                image,
                view,
                allocation,
                slot,
            } => {
                s.raw.destroy_image_view(view, None);
                s.raw.destroy_image(image, None);
                s.free(allocation);
                write(&s.pools.textures).release(slot);
                tracing::debug!(target: "rhi_vulkan::texture", slot = slot.index(), "bindless image slot released");
            }
            Doomed::Sampler { sampler, slot } => {
                s.raw.destroy_sampler(sampler, None);
                lock(&s.pools.samplers).release(slot);
                tracing::debug!(target: "rhi_vulkan::texture", slot = slot.index(), "bindless sampler slot released");
            }
        }
    }
}

/// Destroys every queued object; the device must be idle.
pub(crate) fn drain_all(s: &DeviceShared) {
    loop {
        let d = lock(&s.sync.deferred).pop_any();
        let Some(d) = d else { break };
        // SAFETY: the caller made the device idle; entries are popped exactly once and no lock
        // is held here.
        unsafe { destroy_doomed(s, d) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ash::vk::Handle;

    fn m() -> Doomed {
        Doomed::ShaderModule(vk::ShaderModule::from_raw(1))
    }

    #[test]
    fn ordering_and_equal_tag() {
        let mut q = DeferredQueue::default();
        q.push(3, m());
        q.push(3, m());
        q.push(5, m());
        assert!(q.pop_ready(2).is_none());
        assert!(q.pop_ready(3).is_some());
        assert!(q.pop_ready(4).is_some());
        assert!(q.pop_ready(4).is_none());
        assert_eq!(q.len(), 1);
        assert!(q.pop_any().is_some());
        assert!(q.pop_any().is_none());
    }

    #[test]
    #[should_panic(expected = "non-decreasing")]
    #[cfg(debug_assertions)]
    fn decreasing_tag_asserts() {
        let mut q = DeferredQueue::default();
        q.push(5, m());
        q.push(4, m());
    }
}
