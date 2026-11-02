//! MODULE-007: the global bindless descriptor set and the single pipeline layout (ADR-0010).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use ash::vk;
use rhi::{BindlessDiagnostics, PUSH_CONSTANT_BYTES, RhiError};

use crate::error::map_vk;
use crate::lock;

/// Wanted capacities (ADR-0010 §1).
pub(crate) const WANTED_IMAGES: u32 = 65536;
pub(crate) const WANTED_SAMPLERS: u32 = 1024;
/// Storage images share the sampled images' slot space (a texture's slot is its pool index).
pub(crate) const WANTED_STORAGE_IMAGES: u32 = 65536;

/// Already clamped to the device limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BindlessLimits {
    pub sampled_images: u32,
    pub samplers: u32,
    pub storage_images: u32,
}

pub(crate) fn bindless_capacity(device_limit: u32, wanted: u32) -> u32 {
    device_limit.min(wanted)
}

impl BindlessLimits {
    pub(crate) fn from_device(l_img: u32, l_smp: u32, l_storage: u32) -> Self {
        Self {
            sampled_images: bindless_capacity(l_img, WANTED_IMAGES),
            samplers: bindless_capacity(l_smp, WANTED_SAMPLERS),
            storage_images: bindless_capacity(l_storage, WANTED_STORAGE_IMAGES),
        }
    }
}

/// Pure description of the set layout bindings: `(binding, type, count, binding flags)`.
pub(crate) fn layout_binding_descs(
    l: BindlessLimits,
) -> [(u32, vk::DescriptorType, u32, vk::DescriptorBindingFlags); 3] {
    let flags =
        vk::DescriptorBindingFlags::PARTIALLY_BOUND | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND;
    [
        (
            0,
            vk::DescriptorType::SAMPLED_IMAGE,
            l.sampled_images,
            flags,
        ),
        (1, vk::DescriptorType::SAMPLER, l.samplers, flags),
        (
            2,
            vk::DescriptorType::STORAGE_IMAGE,
            l.storage_images,
            flags,
        ),
    ]
}

/// Stages that see the global set and the push constants: every graphics stage and compute.
pub(crate) const ALL_STAGES: vk::ShaderStageFlags = vk::ShaderStageFlags::from_raw(
    vk::ShaderStageFlags::ALL_GRAPHICS.as_raw() | vk::ShaderStageFlags::COMPUTE.as_raw(),
);

pub(crate) const fn push_range() -> vk::PushConstantRange {
    vk::PushConstantRange {
        stage_flags: ALL_STAGES,
        offset: 0,
        size: PUSH_CONSTANT_BYTES,
    }
}

/// Checks a push-constant write against the 128 byte block.
pub(crate) fn check_push_constants(offset: u32, len: usize) -> Result<(), RhiError> {
    let end = u64::from(offset) + len as u64;
    if !offset.is_multiple_of(4) || !len.is_multiple_of(4) {
        return Err(crate::error::invalid(format!(
            "push_constants offset ({offset}) and length ({len}) must be multiples of 4"
        )));
    }
    if end > u64::from(PUSH_CONSTANT_BYTES) {
        return Err(crate::error::invalid(format!(
            "push_constants range {offset}..{end} exceeds {PUSH_CONSTANT_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(crate) struct GlobalBindings {
    pub set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    pub sampled_image_capacity: u32,
    pub sampler_capacity: u32,
    pub storage_image_capacity: u32,
    /// External synchronization for `vkUpdateDescriptorSets` on `set`.
    update: Mutex<()>,
    pub image_high_water: AtomicU32,
    pub sampler_high_water: AtomicU32,
}

impl GlobalBindings {
    /// Called right after `vkCreateDevice`. On failure everything created here is destroyed.
    pub(crate) fn new(device: &ash::Device, limits: BindlessLimits) -> Result<Self, RhiError> {
        let descs = layout_binding_descs(limits);
        let bindings: Vec<_> = descs
            .iter()
            .map(|&(binding, ty, count, _)| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(ty)
                    .descriptor_count(count)
                    .stage_flags(ALL_STAGES)
            })
            .collect();
        let flags: Vec<_> = descs.iter().map(|d| d.3).collect();
        let mut flags_ci =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
        let layout_ci = vk::DescriptorSetLayoutCreateInfo::default()
            .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
            .bindings(&bindings)
            .push_next(&mut flags_ci);
        // SAFETY: `layout_ci` and the slices it points to outlive the call; the device is live.
        let set_layout = unsafe { device.create_descriptor_set_layout(&layout_ci, None) }
            .map_err(|r| map_vk("vkCreateDescriptorSetLayout", r))?;

        let layouts = [set_layout];
        let ranges = [push_range()];
        let pl_ci = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&ranges);
        // SAFETY: as above.
        let pipeline_layout = match unsafe { device.create_pipeline_layout(&pl_ci, None) } {
            Ok(l) => l,
            Err(r) => {
                // SAFETY: the set layout is unused so far.
                unsafe { device.destroy_descriptor_set_layout(set_layout, None) };
                return Err(map_vk("vkCreatePipelineLayout", r));
            }
        };

        let sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: limits.sampled_images,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: limits.samplers,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_IMAGE,
                descriptor_count: limits.storage_images,
            },
        ];
        let pool_ci = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
            .max_sets(1)
            .pool_sizes(&sizes);
        let undo = |pool: Option<vk::DescriptorPool>| {
            // SAFETY: objects created above are unused and destroyed exactly once.
            unsafe {
                if let Some(p) = pool {
                    device.destroy_descriptor_pool(p, None);
                }
                device.destroy_pipeline_layout(pipeline_layout, None);
                device.destroy_descriptor_set_layout(set_layout, None);
            }
        };
        // SAFETY: `pool_ci` and `sizes` outlive the call.
        let pool = match unsafe { device.create_descriptor_pool(&pool_ci, None) } {
            Ok(p) => p,
            Err(r) => {
                undo(None);
                return Err(map_vk("vkCreateDescriptorPool", r));
            }
        };
        let alloc = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: the pool has capacity for exactly this one set.
        let set = match unsafe { device.allocate_descriptor_sets(&alloc) } {
            Ok(v) => v[0],
            Err(r) => {
                undo(Some(pool));
                return Err(map_vk("vkAllocateDescriptorSets", r));
            }
        };
        tracing::info!(
            target: "rhi_vulkan::bindings",
            sampled_image_capacity = limits.sampled_images,
            sampler_capacity = limits.samplers,
            storage_image_capacity = limits.storage_images,
            push_constant_bytes = PUSH_CONSTANT_BYTES,
            "global bindings created"
        );
        Ok(Self {
            set_layout,
            pipeline_layout,
            pool,
            set,
            sampled_image_capacity: limits.sampled_images,
            sampler_capacity: limits.samplers,
            storage_image_capacity: limits.storage_images,
            update: Mutex::new(()),
            image_high_water: AtomicU32::new(0),
            sampler_high_water: AtomicU32::new(0),
        })
    }

    /// Writes a sampled image descriptor into binding 0. `slot < sampled_image_capacity`.
    pub(crate) fn write_sampled_image(&self, device: &ash::Device, slot: u32, view: vk::ImageView) {
        debug_assert!(slot < self.sampled_image_capacity);
        let info = [vk::DescriptorImageInfo {
            sampler: vk::Sampler::null(),
            image_view: view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        self.write(device, 0, vk::DescriptorType::SAMPLED_IMAGE, slot, &info);
        let hw = self
            .image_high_water
            .fetch_max(slot + 1, Ordering::Relaxed)
            .max(slot + 1);
        tracing::trace!(target: "rhi_vulkan::texture", binding = 0, slot, high_water = hw, "bindless descriptor written");
    }

    /// Writes a storage image descriptor into binding 2 (`GENERAL` layout).
    /// `slot < storage_image_capacity`.
    pub(crate) fn write_storage_image(&self, device: &ash::Device, slot: u32, view: vk::ImageView) {
        debug_assert!(slot < self.storage_image_capacity);
        let info = [vk::DescriptorImageInfo {
            sampler: vk::Sampler::null(),
            image_view: view,
            image_layout: vk::ImageLayout::GENERAL,
        }];
        self.write(device, 2, vk::DescriptorType::STORAGE_IMAGE, slot, &info);
        tracing::trace!(target: "rhi_vulkan::texture", binding = 2, slot, "bindless descriptor written");
    }

    /// Writes a sampler descriptor into binding 1. `slot < sampler_capacity`.
    pub(crate) fn write_sampler(&self, device: &ash::Device, slot: u32, sampler: vk::Sampler) {
        debug_assert!(slot < self.sampler_capacity);
        let info = [vk::DescriptorImageInfo {
            sampler,
            image_view: vk::ImageView::null(),
            image_layout: vk::ImageLayout::UNDEFINED,
        }];
        self.write(device, 1, vk::DescriptorType::SAMPLER, slot, &info);
        let hw = self
            .sampler_high_water
            .fetch_max(slot + 1, Ordering::Relaxed)
            .max(slot + 1);
        tracing::trace!(target: "rhi_vulkan::texture", binding = 1, slot, high_water = hw, "bindless descriptor written");
    }

    fn write(
        &self,
        device: &ash::Device,
        binding: u32,
        ty: vk::DescriptorType,
        slot: u32,
        info: &[vk::DescriptorImageInfo],
    ) {
        let w = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(binding)
            .dst_array_element(slot)
            .descriptor_type(ty)
            .image_info(info);
        let _g = lock(&self.update);
        // SAFETY: `update` is held (external sync of the set); slot < capacity (asserted by the
        // callers); the view/sampler was just created and is alive; the set was created with
        // UPDATE_AFTER_BIND and the bindings carry the flag.
        unsafe { device.update_descriptor_sets(&[w], &[]) };
    }

    pub(crate) fn diagnostics(&self) -> BindlessDiagnostics {
        BindlessDiagnostics {
            image_capacity: self.sampled_image_capacity,
            sampler_capacity: self.sampler_capacity,
            image_high_water: self.image_high_water.load(Ordering::Relaxed),
            sampler_high_water: self.sampler_high_water.load(Ordering::Relaxed),
        }
    }

    /// # Safety
    /// Device idle or no pending use; called once after the deferred drain.
    pub(crate) unsafe fn destroy(&self, device: &ash::Device) {
        // SAFETY: forwarded from the caller's contract; destroying the pool frees its set.
        unsafe {
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
        }
        tracing::debug!(target: "rhi_vulkan::bindings", "global bindings destroyed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindless_capacity_clamps() {
        assert_eq!(bindless_capacity(100, 65536), 100);
        assert_eq!(bindless_capacity(65536, 65536), 65536);
        assert_eq!(bindless_capacity(1_000_000, 65536), 65536);
        let l = BindlessLimits::from_device(1_000_000, 500, 90);
        assert_eq!(
            (l.sampled_images, l.samplers, l.storage_images),
            (65536, 500, 90)
        );
        let l = BindlessLimits::from_device(2000, 5000, 1_000_000);
        assert_eq!(
            (l.sampled_images, l.samplers, l.storage_images),
            (2000, 1024, 65536)
        );
    }

    #[test]
    fn layout_binding_descs_match_spec() {
        let d = layout_binding_descs(BindlessLimits {
            sampled_images: 7,
            samplers: 3,
            storage_images: 5,
        });
        let f = vk::DescriptorBindingFlags::PARTIALLY_BOUND
            | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND;
        assert_eq!(d[0], (0, vk::DescriptorType::SAMPLED_IMAGE, 7, f));
        assert_eq!(d[1], (1, vk::DescriptorType::SAMPLER, 3, f));
        assert_eq!(d[2], (2, vk::DescriptorType::STORAGE_IMAGE, 5, f));
        assert!(!f.contains(vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT));
    }

    #[test]
    fn push_range_is_128_graphics_and_compute() {
        let r = push_range();
        assert_eq!(r.offset, 0);
        assert_eq!(r.size, 128);
        assert_eq!(
            r.stage_flags,
            vk::ShaderStageFlags::ALL_GRAPHICS | vk::ShaderStageFlags::COMPUTE
        );
    }

    #[test]
    fn push_constants_validation() {
        assert!(check_push_constants(0, 128).is_ok());
        assert!(check_push_constants(124, 4).is_ok());
        assert!(check_push_constants(0, 0).is_ok());
        assert!(check_push_constants(4, 128).is_err());
        assert!(check_push_constants(128, 4).is_err());
        assert!(check_push_constants(2, 4).is_err());
        assert!(check_push_constants(0, 3).is_err());
        assert!(check_push_constants(u32::MAX - 3, 8).is_err());
    }
}
