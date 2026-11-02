//! Deferred destruction keyed on timeline value (LAYER-004 §7.5, MODULE-004 §6.2).

use std::collections::VecDeque;

use ash::vk;

pub(crate) enum Doomed {
    ShaderModule(vk::ShaderModule),
    Pipeline {
        pipeline: vk::Pipeline,
        layout: vk::PipelineLayout,
    },
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
pub(crate) unsafe fn destroy_doomed(device: &ash::Device, d: Doomed) {
    // SAFETY: forwarded from the caller's contract; entries are popped so destruction is
    // exactly-once.
    unsafe {
        match d {
            Doomed::ShaderModule(m) => device.destroy_shader_module(m, None),
            Doomed::Pipeline { pipeline, layout } => {
                device.destroy_pipeline(pipeline, None);
                device.destroy_pipeline_layout(layout, None);
            }
        }
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
