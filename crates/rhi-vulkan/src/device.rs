//! MODULE-002: physical device selection, logical device, `rhi::Device` implementation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use ash::vk;
use ash::{ext, khr};
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc};
use rhi::{
    Acquire, BufferDesc, BufferHandle, BufferTag, DEFAULT_FRAMES_IN_FLIGHT, DeviceDesc,
    DeviceDiagnostics, DeviceInfo, Format, FrameToken, GpuKind, GraphicsPipelineDesc, LiveObjects,
    MemoryDiagnostics, PipelineHandle, PipelineTag, PresentStatus, RhiError, SamplerDesc,
    SamplerHandle, SamplerTag, ShaderModuleDesc, ShaderModuleHandle, ShaderModuleTag,
    SwapchainDesc, SwapchainHandle, SwapchainImage, SwapchainTag, TextureDesc, TextureHandle,
    TextureTag,
};

use crate::bindings::{BindlessLimits, GlobalBindings};
use crate::buffer::{self, VkBuffer};
use crate::commands::{CommandManager, VkPipeline, VkShaderModule, VulkanCommands, VulkanFinished};
use crate::destroy::{DeferredQueue, Doomed, drain_all};
use crate::error::{invalid, map_vk};
use crate::instance::InstanceShared;
use crate::registry::Registry;
use crate::swapchain::{self, VkSwapchain, VulkanSurface};
use crate::texture::{self, VkSampler, VkTexture};
use crate::upload::PendingUpload;
use crate::{lock, read, write};

// ---------------------------------------------------------------- selection (pure)

pub(crate) struct QueueFamilyFacts {
    pub index: u32,
    pub flags: vk::QueueFlags,
    pub queue_count: u32,
    pub can_present: Option<bool>,
}

pub(crate) struct SurfaceFacts {
    pub format_count: u32,
    pub present_mode_count: u32,
}

pub(crate) struct PhysicalDeviceFacts {
    pub name: String,
    pub vendor_id: u32,
    pub device_type: vk::PhysicalDeviceType,
    pub api_version: u32,
    pub driver: String,
    pub extensions: Vec<String>,
    pub dynamic_rendering: bool,
    pub synchronization2: bool,
    pub timeline_semaphore: bool,
    pub buffer_device_address: bool,
    pub runtime_descriptor_array: bool,
    pub shader_sampled_image_array_non_uniform_indexing: bool,
    pub descriptor_binding_sampled_image_update_after_bind: bool,
    pub descriptor_binding_partially_bound: bool,
    /// `L_img` of MODULE-007 §6.3.
    pub bindless_sampled_images: u32,
    /// `L_smp` of MODULE-007 §6.3.
    pub bindless_samplers: u32,
    pub max_image_dimension_2d: u32,
    pub non_coherent_atom_size: u64,
    pub queue_families: Vec<QueueFamilyFacts>,
    pub max_device_local_heap: u64,
    pub surface: Option<SurfaceFacts>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    ApiVersion { found: u32 },
    MissingFeature(&'static str),
    MissingExtension(&'static str),
    BindlessLimits { images: u32, samplers: u32 },
    NoGraphicsQueue,
    NoPresentSupport,
    SurfaceUnusable,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiVersion { found } => write!(
                f,
                "Vulkan {}.{} < 1.3",
                vk::api_version_major(*found),
                vk::api_version_minor(*found)
            ),
            Self::MissingFeature(n) => write!(f, "missing feature {n}"),
            Self::MissingExtension(n) => write!(f, "missing extension {n}"),
            Self::BindlessLimits { images, samplers } => write!(
                f,
                "bindless limits too small (sampled images {images} < 1024 or samplers {samplers} < 64)"
            ),
            Self::NoGraphicsQueue => f.write_str("no graphics queue family"),
            Self::NoPresentSupport => f.write_str("no graphics queue family can present"),
            Self::SurfaceUnusable => f.write_str("surface has no formats or present modes"),
        }
    }
}

const MIN_BINDLESS_IMAGES: u32 = 1024;
const MIN_BINDLESS_SAMPLERS: u32 = 64;

pub(crate) fn evaluate(f: &PhysicalDeviceFacts) -> Result<u32, Rejection> {
    if f.api_version < vk::API_VERSION_1_3 {
        return Err(Rejection::ApiVersion {
            found: f.api_version,
        });
    }
    if !f.dynamic_rendering {
        return Err(Rejection::MissingFeature("dynamicRendering"));
    }
    if !f.synchronization2 {
        return Err(Rejection::MissingFeature("synchronization2"));
    }
    if !f.timeline_semaphore {
        return Err(Rejection::MissingFeature("timelineSemaphore"));
    }
    for (ok, name) in [
        (f.buffer_device_address, "bufferDeviceAddress"),
        (f.runtime_descriptor_array, "runtimeDescriptorArray"),
        (
            f.shader_sampled_image_array_non_uniform_indexing,
            "shaderSampledImageArrayNonUniformIndexing",
        ),
        (
            f.descriptor_binding_sampled_image_update_after_bind,
            "descriptorBindingSampledImageUpdateAfterBind",
        ),
        (
            f.descriptor_binding_partially_bound,
            "descriptorBindingPartiallyBound",
        ),
    ] {
        if !ok {
            return Err(Rejection::MissingFeature(name));
        }
    }
    if f.bindless_sampled_images < MIN_BINDLESS_IMAGES
        || f.bindless_samplers < MIN_BINDLESS_SAMPLERS
    {
        return Err(Rejection::BindlessLimits {
            images: f.bindless_sampled_images,
            samplers: f.bindless_samplers,
        });
    }
    if !f.extensions.iter().any(|e| e == "VK_KHR_swapchain") {
        return Err(Rejection::MissingExtension("VK_KHR_swapchain"));
    }
    let graphics: Vec<_> = f
        .queue_families
        .iter()
        .filter(|q| q.flags.contains(vk::QueueFlags::GRAPHICS) && q.queue_count >= 1)
        .collect();
    if graphics.is_empty() {
        return Err(Rejection::NoGraphicsQueue);
    }
    if let Some(s) = &f.surface
        && (s.format_count == 0 || s.present_mode_count == 0)
    {
        return Err(Rejection::SurfaceUnusable);
    }
    let usable = |q: &&QueueFamilyFacts| f.surface.is_none() || q.can_present == Some(true);
    let mut cands = graphics.into_iter().filter(usable);
    // Prefer a family that also has COMPUTE; ties go to the lowest index (families are in order).
    let all: Vec<_> = cands.by_ref().collect();
    all.iter()
        .find(|q| q.flags.contains(vk::QueueFlags::COMPUTE))
        .or(all.first())
        .map(|q| q.index)
        .ok_or(Rejection::NoPresentSupport)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Score {
    pub preferred_name: bool,
    pub kind_rank: u8,
    pub device_local_mib: u64,
}

pub(crate) fn score(f: &PhysicalDeviceFacts, preferred: Option<&str>) -> Score {
    Score {
        preferred_name: preferred
            .is_some_and(|p| f.name.to_lowercase().contains(&p.to_lowercase())),
        kind_rank: match f.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 4,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 3,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
            vk::PhysicalDeviceType::CPU => 1,
            _ => 0,
        },
        device_local_mib: f.max_device_local_heap >> 20,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub index: usize,
    pub queue_family: u32,
}

pub(crate) fn select(
    facts: &[PhysicalDeviceFacts],
    preferred: Option<&str>,
) -> Result<Selection, RhiError> {
    if facts.is_empty() {
        return Err(RhiError::NoSuitableDevice {
            reason: "no Vulkan physical devices".to_owned(),
        });
    }
    let mut best: Option<(Score, Selection)> = None;
    let mut reasons = Vec::new();
    let mut any_preferred = false;
    for (index, f) in facts.iter().enumerate() {
        match evaluate(f) {
            Ok(queue_family) => {
                let s = score(f, preferred);
                any_preferred |= s.preferred_name;
                tracing::info!(
                    gpu = %f.name, accepted = true, preferred_name = s.preferred_name,
                    kind_rank = s.kind_rank, device_local_mib = s.device_local_mib,
                    queue_family, "gpu candidate"
                );
                // Strictly greater: ties keep the lowest enumeration index.
                if best.as_ref().is_none_or(|(b, _)| s > *b) {
                    best = Some((
                        s,
                        Selection {
                            index,
                            queue_family,
                        },
                    ));
                }
            }
            Err(r) => {
                tracing::info!(gpu = %f.name, accepted = false, reason = %r, "gpu candidate rejected");
                reasons.push(format!("{}: {r}", f.name));
            }
        }
    }
    if let Some(p) = preferred
        && !any_preferred
        && best.is_some()
    {
        tracing::warn!(
            preferred = p,
            "no suitable GPU matches the preferred name; using default order"
        );
    }
    best.map(|(_, s)| s).ok_or(RhiError::NoSuitableDevice {
        reason: reasons.join("; "),
    })
}

pub(crate) fn gpu_kind(t: vk::PhysicalDeviceType) -> GpuKind {
    match t {
        vk::PhysicalDeviceType::DISCRETE_GPU => GpuKind::Discrete,
        vk::PhysicalDeviceType::INTEGRATED_GPU => GpuKind::Integrated,
        vk::PhysicalDeviceType::VIRTUAL_GPU => GpuKind::Virtual,
        vk::PhysicalDeviceType::CPU => GpuKind::Cpu,
        _ => GpuKind::Other,
    }
}

pub(crate) fn check_frames_in_flight(n: u32) -> Result<(), RhiError> {
    if (1..=3).contains(&n) {
        Ok(())
    } else {
        Err(invalid(format!("frames_in_flight must be 1..=3, got {n}")))
    }
}

// ---------------------------------------------------------------- shared state

pub(crate) struct Pools {
    pub textures: RwLock<Registry<TextureTag, VkTexture>>,
    pub shader_modules: RwLock<Registry<ShaderModuleTag, VkShaderModule>>,
    pub pipelines: RwLock<Registry<PipelineTag, VkPipeline>>,
    pub swapchains: Mutex<Registry<SwapchainTag, VkSwapchain>>,
    pub buffers: RwLock<Registry<BufferTag, VkBuffer>>,
    pub samplers: Mutex<Registry<SamplerTag, VkSampler>>,
}

/// Device limits the resource code needs.
pub(crate) struct DeviceLimits {
    pub max_image_dimension_2d: u32,
    pub non_coherent_atom: u64,
}

pub(crate) struct SyncShared {
    pub timeline: vk::Semaphore,
    pub last_submitted: AtomicU64,
    pub deferred: Mutex<DeferredQueue>,
}

impl SyncShared {
    /// Value the next submit will signal; >= any submission that may reference a live object.
    pub(crate) fn doom_tag(&self) -> u64 {
        self.last_submitted.load(Ordering::Acquire) + 1
    }

    pub(crate) fn defer(&self, d: Doomed) {
        lock(&self.deferred).push(self.doom_tag(), d);
    }
}

pub(crate) struct DeviceShared {
    pub physical: vk::PhysicalDevice,
    pub raw: ash::Device,
    pub queue_family: u32,
    pub queue: Mutex<vk::Queue>,
    pub swapchain_fn: khr::swapchain::Device,
    pub debug_fn: Option<ext::debug_utils::Device>,
    pub allocator: Mutex<Option<gpu_allocator::vulkan::Allocator>>,
    pub info: DeviceInfo,
    pub pools: Pools,
    pub pipeline_cache: vk::PipelineCache,
    pub sync: SyncShared,
    pub bindings: GlobalBindings,
    pub limits: DeviceLimits,
    /// Bytes of live (not yet retired) allocations.
    pub allocated_bytes: AtomicU64,
    /// Uploads waiting for the next submit (MODULE-005 lead amendment).
    pub pending_uploads: Mutex<Vec<PendingUpload>>,
    // Declared last so it drops after `Drop::drop` destroyed the device.
    pub instance: Arc<InstanceShared>,
}

impl DeviceShared {
    /// Applies a debug name (ADR-0009 §3); silently skipped without `VK_EXT_debug_utils`.
    pub(crate) fn set_name<H: vk::Handle>(&self, handle: H, name: &str) {
        let Some(du) = &self.debug_fn else { return };
        let Ok(c) = std::ffi::CString::new(name) else {
            return;
        };
        let info = vk::DebugUtilsObjectNameInfoEXT::default()
            .object_handle(handle)
            .object_name(&c);
        // SAFETY: `handle` was created from `self.raw`, which is alive; `info` outlives the call.
        let _ = unsafe { du.set_debug_utils_object_name(&info) };
    }

    /// Allocates memory and accounts for it. Lock order: `pools` -> `allocator`.
    pub(crate) fn alloc(&self, desc: &AllocationCreateDesc) -> Result<Allocation, RhiError> {
        let mut g = lock(&self.allocator);
        let Some(a) = g.as_mut() else {
            return Err(invalid("allocation after device shutdown"));
        };
        match a.allocate(desc) {
            Ok(alloc) => {
                self.allocated_bytes
                    .fetch_add(alloc.size(), Ordering::Relaxed);
                tracing::trace!(target: "rhi_vulkan::buffer", name = desc.name, bytes = alloc.size(), "memory allocated");
                Ok(alloc)
            }
            Err(e) => {
                tracing::warn!(target: "rhi_vulkan::buffer", variant = ?e, name = desc.name, size = desc.requirements.size, location = ?desc.location, "allocation failed");
                Err(match e {
                    gpu_allocator::AllocationError::OutOfMemory => RhiError::OutOfDeviceMemory,
                    _ => RhiError::Backend {
                        op: "gpu_allocator",
                        code: -1,
                    },
                })
            }
        }
    }

    /// Returns memory to the allocator.
    pub(crate) fn free(&self, a: Allocation) {
        let size = a.size();
        match lock(&self.allocator).as_mut() {
            Some(al) => {
                if let Err(e) = al.free(a) {
                    tracing::warn!(target: "rhi_vulkan::buffer", error = ?e, "allocation free failed");
                }
            }
            None => {
                tracing::warn!(target: "rhi_vulkan::buffer", "allocation freed after allocator shutdown")
            }
        }
        self.allocated_bytes.fetch_sub(size, Ordering::Relaxed);
    }

    /// Destroys staging buffers of uploads that never reached a submit.
    pub(crate) fn drop_pending_uploads(&self) {
        let pending = std::mem::take(&mut *lock(&self.pending_uploads));
        for u in pending {
            // SAFETY: the buffer was never submitted; destroyed exactly once.
            unsafe { crate::destroy::destroy_doomed(self, u.into_doomed()) };
        }
    }

    pub(crate) fn wait_idle(&self) -> Result<(), RhiError> {
        let q = lock(&self.queue);
        // SAFETY: queue mutex held (vkDeviceWaitIdle needs all queues externally synchronized).
        let r = unsafe { self.raw.device_wait_idle() };
        drop(q);
        r.map_err(|r| map_vk("vkDeviceWaitIdle", r))
    }
}

impl Drop for DeviceShared {
    fn drop(&mut self) {
        let _ = self.wait_idle();
        self.drop_pending_uploads();
        drain_all(self);
        // The allocator must drop before the device (ADR-0007).
        lock(&self.allocator).take();
        // SAFETY: all children are destroyed (shutdown order) and the device is idle; the
        // global bindings, timeline and cache are destroyed exactly once, immediately before
        // the device.
        unsafe {
            self.bindings.destroy(&self.raw);
            self.raw.destroy_semaphore(self.sync.timeline, None);
            self.raw.destroy_pipeline_cache(self.pipeline_cache, None);
            self.raw.destroy_device(None);
        }
        tracing::debug!(device = %self.info.name, "device destroyed");
    }
}

// ---------------------------------------------------------------- creation

fn gather_facts(
    inst: &InstanceShared,
    pd: vk::PhysicalDevice,
    surface: Option<&VulkanSurface>,
) -> Result<PhysicalDeviceFacts, RhiError> {
    let raw = &inst.raw;
    let mut p12 = vk::PhysicalDeviceVulkan12Properties::default();
    let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut p12);
    // SAFETY: `pd` comes from this live instance's enumeration.
    unsafe { raw.get_physical_device_properties2(pd, &mut p2) };
    let props = p2.properties;
    let driver = format!(
        "{} {}",
        p12.driver_name_as_c_str()
            .map(|c| c.to_string_lossy())
            .unwrap_or_default(),
        p12.driver_info_as_c_str()
            .map(|c| c.to_string_lossy())
            .unwrap_or_default()
    );
    let mut f12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut f13 = vk::PhysicalDeviceVulkan13Features::default();
    let mut f2 = vk::PhysicalDeviceFeatures2::default()
        .push_next(&mut f12)
        .push_next(&mut f13);
    // SAFETY: as above.
    unsafe { raw.get_physical_device_features2(pd, &mut f2) };
    // SAFETY: as above.
    let exts = unsafe { raw.enumerate_device_extension_properties(pd) }
        .map_err(|r| map_vk("vkEnumerateDeviceExtensionProperties", r))?;
    // SAFETY: as above.
    let qprops = unsafe { raw.get_physical_device_queue_family_properties(pd) };
    // SAFETY: as above.
    let mem = unsafe { raw.get_physical_device_memory_properties(pd) };
    let max_heap = mem.memory_heaps[..mem.memory_heap_count as usize]
        .iter()
        .filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
        .map(|h| h.size)
        .max()
        .unwrap_or(0);

    let mut queue_families = Vec::new();
    for (i, q) in qprops.iter().enumerate() {
        let index = i as u32;
        let can_present = match (surface, &inst.surface_fn) {
            (Some(s), Some(sf)) => {
                // SAFETY: surface and physical device belong to this instance (checked by caller).
                let r = unsafe { sf.get_physical_device_surface_support(pd, index, s.inner.raw) };
                Some(r.map_err(|r| map_vk("vkGetPhysicalDeviceSurfaceSupportKHR", r))?)
            }
            _ => None,
        };
        queue_families.push(QueueFamilyFacts {
            index,
            flags: q.queue_flags,
            queue_count: q.queue_count,
            can_present,
        });
    }
    let surface_facts = match (surface, &inst.surface_fn) {
        (Some(s), Some(sf)) => {
            // SAFETY: as above.
            let formats = unsafe { sf.get_physical_device_surface_formats(pd, s.inner.raw) }
                .map_err(|r| map_vk("vkGetPhysicalDeviceSurfaceFormatsKHR", r))?;
            // SAFETY: as above.
            let modes = unsafe { sf.get_physical_device_surface_present_modes(pd, s.inner.raw) }
                .map_err(|r| map_vk("vkGetPhysicalDeviceSurfacePresentModesKHR", r))?;
            Some(SurfaceFacts {
                format_count: formats.len() as u32,
                present_mode_count: modes.len() as u32,
            })
        }
        _ => None,
    };
    Ok(PhysicalDeviceFacts {
        name: props
            .device_name_as_c_str()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default(),
        vendor_id: props.vendor_id,
        device_type: props.device_type,
        api_version: props.api_version,
        driver,
        extensions: exts
            .iter()
            .filter_map(|e| e.extension_name_as_c_str().ok())
            .map(|c| c.to_string_lossy().into_owned())
            .collect(),
        dynamic_rendering: f13.dynamic_rendering == vk::TRUE,
        synchronization2: f13.synchronization2 == vk::TRUE,
        timeline_semaphore: f12.timeline_semaphore == vk::TRUE,
        buffer_device_address: f12.buffer_device_address == vk::TRUE,
        runtime_descriptor_array: f12.runtime_descriptor_array == vk::TRUE,
        shader_sampled_image_array_non_uniform_indexing: f12
            .shader_sampled_image_array_non_uniform_indexing
            == vk::TRUE,
        descriptor_binding_sampled_image_update_after_bind: f12
            .descriptor_binding_sampled_image_update_after_bind
            == vk::TRUE,
        descriptor_binding_partially_bound: f12.descriptor_binding_partially_bound == vk::TRUE,
        bindless_sampled_images: p12
            .max_per_stage_descriptor_update_after_bind_sampled_images
            .min(p12.max_descriptor_set_update_after_bind_sampled_images),
        bindless_samplers: p12
            .max_per_stage_descriptor_update_after_bind_samplers
            .min(p12.max_descriptor_set_update_after_bind_samplers),
        max_image_dimension_2d: props.limits.max_image_dimension2_d,
        non_coherent_atom_size: props.limits.non_coherent_atom_size,
        queue_families,
        max_device_local_heap: max_heap,
        surface: surface_facts,
    })
}

pub(crate) fn create_device(
    inst: &Arc<InstanceShared>,
    desc: &DeviceDesc,
    surface: Option<&VulkanSurface>,
) -> Result<VulkanDevice, RhiError> {
    create_device_with(inst, desc, surface, None)
}

/// `bindless_override` lowers the bindless capacities (tests only).
pub(crate) fn create_device_with(
    inst: &Arc<InstanceShared>,
    desc: &DeviceDesc,
    surface: Option<&VulkanSurface>,
    bindless_override: Option<BindlessLimits>,
) -> Result<VulkanDevice, RhiError> {
    let started = Instant::now();
    check_frames_in_flight(desc.frames_in_flight)?;
    if let Some(s) = surface
        && !Arc::ptr_eq(&s.inner.instance, inst)
    {
        return Err(invalid("surface belongs to another instance"));
    }
    let env_gpu = std::env::var("ANVIL_GPU").ok().filter(|s| !s.is_empty());
    let preferred = env_gpu.or_else(|| desc.preferred_gpu.clone());

    // SAFETY: the instance is alive (Arc held).
    let pds = unsafe { inst.raw.enumerate_physical_devices() }
        .map_err(|r| map_vk("vkEnumeratePhysicalDevices", r))?;
    let facts = pds
        .iter()
        .map(|&pd| gather_facts(inst, pd, surface))
        .collect::<Result<Vec<_>, _>>()?;
    let sel = select(&facts, preferred.as_deref())?;
    let (pd, f) = (pds[sel.index], &facts[sel.index]);
    let v = f.api_version;
    let info = DeviceInfo {
        name: f.name.clone(),
        vendor_id: f.vendor_id,
        kind: gpu_kind(f.device_type),
        api_version: (
            vk::api_version_major(v),
            vk::api_version_minor(v),
            vk::api_version_patch(v),
        ),
        driver: f.driver.trim().to_owned(),
    };
    tracing::info!(
        gpu = %info.name, kind = ?info.kind, api_version = ?info.api_version,
        driver = %info.driver, queue_family = sel.queue_family, "gpu selected"
    );

    let priorities = [1.0f32];
    let queue_ci = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(sel.queue_family)
        .queue_priorities(&priorities)];
    let ext_names = [khr::swapchain::NAME.as_ptr()];
    let mut f12 = vk::PhysicalDeviceVulkan12Features::default()
        .timeline_semaphore(true)
        .buffer_device_address(true)
        .runtime_descriptor_array(true)
        .shader_sampled_image_array_non_uniform_indexing(true)
        .descriptor_binding_sampled_image_update_after_bind(true)
        .descriptor_binding_partially_bound(true);
    let mut f13 = vk::PhysicalDeviceVulkan13Features::default()
        .dynamic_rendering(true)
        .synchronization2(true)
        // `discard` in fragment shaders (alpha test) compiles to OpDemoteToHelperInvocation;
        // the feature is mandatory in Vulkan 1.3.
        .shader_demote_to_helper_invocation(true);
    let ci = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_ci)
        .enabled_extension_names(&ext_names)
        .push_next(&mut f12)
        .push_next(&mut f13);
    // SAFETY: all pointers in `ci` (priorities, names, feature chain) outlive the call.
    let raw = unsafe { inst.raw.create_device(pd, &ci, None) }
        .map_err(|r| map_vk("vkCreateDevice", r))?;

    let limits = bindless_override.unwrap_or_else(|| {
        BindlessLimits::from_device(f.bindless_sampled_images, f.bindless_samplers)
    });
    let bindings = match GlobalBindings::new(&raw, limits) {
        Ok(b) => b,
        Err(e) => {
            // SAFETY: no children exist yet.
            unsafe { raw.destroy_device(None) };
            return Err(e);
        }
    };
    let mut tl_type = vk::SemaphoreTypeCreateInfo::default()
        .semaphore_type(vk::SemaphoreType::TIMELINE)
        .initial_value(0);
    let tl_ci = vk::SemaphoreCreateInfo::default().push_next(&mut tl_type);
    // SAFETY: `raw` is a live device; on failure it is destroyed immediately (nothing else exists).
    let timeline = match unsafe { raw.create_semaphore(&tl_ci, None) } {
        Ok(s) => s,
        Err(r) => {
            // SAFETY: only the global bindings exist besides the device.
            unsafe {
                bindings.destroy(&raw);
                raw.destroy_device(None);
            }
            return Err(map_vk("vkCreateSemaphore", r));
        }
    };
    // SAFETY: as above.
    let cache =
        match unsafe { raw.create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None) } {
            Ok(c) => c,
            Err(r) => {
                // SAFETY: only the timeline and bindings exist besides the device.
                unsafe {
                    bindings.destroy(&raw);
                    raw.destroy_semaphore(timeline, None);
                    raw.destroy_device(None);
                }
                return Err(map_vk("vkCreatePipelineCache", r));
            }
        };
    // SAFETY: family/index 0 were requested in `queue_ci`.
    let queue = unsafe { raw.get_device_queue(sel.queue_family, 0) };
    let shared = Arc::new(DeviceShared {
        physical: pd,
        swapchain_fn: khr::swapchain::Device::new(&inst.raw, &raw),
        debug_fn: inst
            .debug_utils
            .as_ref()
            .map(|_| ext::debug_utils::Device::new(&inst.raw, &raw)),
        raw,
        queue_family: sel.queue_family,
        queue: Mutex::new(queue),
        allocator: Mutex::new(None),
        info,
        pools: Pools {
            textures: RwLock::new(Registry::new(|| TextureTag)),
            shader_modules: RwLock::new(Registry::new(|| ShaderModuleTag)),
            pipelines: RwLock::new(Registry::new(|| PipelineTag)),
            swapchains: Mutex::new(Registry::new(|| SwapchainTag)),
            buffers: RwLock::new(Registry::new(|| BufferTag)),
            samplers: Mutex::new(Registry::new(|| SamplerTag)),
        },
        pipeline_cache: cache,
        bindings,
        limits: DeviceLimits {
            max_image_dimension_2d: f.max_image_dimension_2d,
            non_coherent_atom: f.non_coherent_atom_size,
        },
        allocated_bytes: AtomicU64::new(0),
        pending_uploads: Mutex::new(Vec::new()),
        sync: SyncShared {
            timeline,
            last_submitted: AtomicU64::new(0),
            deferred: Mutex::new(DeferredQueue::default()),
        },
        instance: inst.clone(),
    });
    // From here failures unwind through `DeviceShared::drop`.
    shared.set_name(shared.raw.handle(), &format!("device:{}", shared.info.name));
    shared.set_name(queue, "graphics queue");
    shared.set_name(timeline, "graphics timeline");
    shared.set_name(cache, "pipeline cache");
    shared.set_name(shared.bindings.set_layout, "global-bindings.set_layout");
    shared.set_name(
        shared.bindings.pipeline_layout,
        "global-bindings.pipeline_layout",
    );
    shared.set_name(shared.bindings.set, "global-bindings.set");

    let allocator =
        gpu_allocator::vulkan::Allocator::new(&gpu_allocator::vulkan::AllocatorCreateDesc {
            instance: inst.raw.clone(),
            device: shared.raw.clone(),
            physical_device: pd,
            debug_settings: Default::default(),
            buffer_device_address: true,
            allocation_sizes: Default::default(),
        })
        .map_err(|e| match e {
            gpu_allocator::AllocationError::OutOfMemory => RhiError::OutOfDeviceMemory,
            other => {
                tracing::error!(error = ?other, "gpu-allocator failure");
                RhiError::Backend {
                    op: "gpu_allocator",
                    code: -1,
                }
            }
        })?;
    *lock(&shared.allocator) = Some(allocator);

    let frames = CommandManager::new(shared.clone(), desc.frames_in_flight)?;
    tracing::info!(
        frames_in_flight = desc.frames_in_flight,
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "device created"
    );
    Ok(VulkanDevice {
        shared,
        frames,
        shut: false,
    })
}

// ---------------------------------------------------------------- VulkanDevice

/// Vulkan logical device plus frame machinery. `Send`, not `Sync`.
pub struct VulkanDevice {
    pub(crate) shared: Arc<DeviceShared>,
    pub(crate) frames: CommandManager,
    shut: bool,
}

impl VulkanDevice {
    /// Idempotent teardown (LAYER-004 §9 steps 1-6); `Drop` calls it and logs errors.
    ///
    /// # Errors
    /// Fails if the initial `vkDeviceWaitIdle` fails; teardown continues regardless.
    pub fn shutdown(&mut self) -> Result<(), RhiError> {
        if self.shut {
            return Ok(());
        }
        self.shut = true;
        let s = &self.shared;
        let idle = s.wait_idle();
        s.drop_pending_uploads();
        drain_all(s);
        let pipelines = {
            let mut p = write(&s.pools.pipelines);
            let hs = p.handles();
            hs.into_iter()
                .filter_map(|h| p.remove(h).map(|v| (h, v)))
                .collect::<Vec<_>>()
        };
        for (h, p) in pipelines {
            tracing::warn!(handle = ?h, "leaked pipeline destroyed at shutdown");
            // SAFETY: device idle; removed from the pool so destroyed once.
            unsafe {
                crate::destroy::destroy_doomed(
                    s,
                    Doomed::Pipeline {
                        pipeline: p.pipeline,
                    },
                )
            };
        }
        let modules = {
            let mut p = write(&s.pools.shader_modules);
            let hs = p.handles();
            hs.into_iter()
                .filter_map(|h| p.remove(h).map(|v| (h, v)))
                .collect::<Vec<_>>()
        };
        for (h, m) in modules {
            tracing::warn!(handle = ?h, "leaked shader module destroyed at shutdown");
            // SAFETY: as above.
            unsafe { crate::destroy::destroy_doomed(s, Doomed::ShaderModule(m.raw)) };
        }
        buffer::destroy_leaked(s);
        texture::destroy_leaked(s);
        let scs = lock(&s.pools.swapchains).handles();
        for h in scs {
            tracing::warn!(handle = ?h, "leaked swapchain destroyed at shutdown");
            swapchain::destroy(s, h, false);
        }
        self.frames.shutdown();
        lock(&s.allocator).take();
        tracing::debug!(device = %s.info.name, "device shutdown");
        idle
    }

    /// Submits finished commands without swapchain semaphores (headless rendering, tests).
    /// Returns the timeline value the submission signals.
    ///
    /// # Errors
    /// As [`rhi::Device::submit`], except no acquired image is required.
    pub fn submit_unwindowed(&mut self, cmds: VulkanFinished) -> Result<u64, RhiError> {
        self.shared.instance.check_validation_panic();
        self.frames.submit_unwindowed(cmds)
    }

    fn frame_span(&self) -> tracing::Span {
        self.frames.span.clone()
    }
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        if let Err(e) = self.shutdown() {
            tracing::error!(error = %e, "device shutdown failed");
        }
    }
}

impl rhi::Device for VulkanDevice {
    type Surface = VulkanSurface;
    type Commands = VulkanCommands;

    fn info(&self) -> &DeviceInfo {
        &self.shared.info
    }

    fn diagnostics(&self) -> DeviceDiagnostics {
        let s = &self.shared;
        // SAFETY: timeline semaphore is alive for the life of `shared`.
        let completed = unsafe { s.raw.get_semaphore_counter_value(s.sync.timeline) }.unwrap_or(0);
        // Guards are taken in separate statements: temporaries in a struct literal would live to
        // the end of the statement and `swapchain::diagnostics` locks the same mutex.
        let live = LiveObjects {
            swapchains: lock(&s.pools.swapchains).len(),
            textures: read(&s.pools.textures).len(),
            shader_modules: read(&s.pools.shader_modules).len(),
            pipelines: read(&s.pools.pipelines).len(),
            buffers: read(&s.pools.buffers).len(),
            samplers: lock(&s.pools.samplers).len(),
        };
        let reserved_bytes = lock(&s.allocator).as_ref().map_or(0, |a| a.capacity());
        let pending_destroys = lock(&s.sync.deferred).len();
        DeviceDiagnostics {
            info: s.info.clone(),
            frame: self.frames.frame_counter(),
            gpu_completed_frame: completed,
            frames_in_flight: self.frames.frames_in_flight(),
            live,
            pending_destroys,
            swapchains: swapchain::diagnostics(s),
            validation: s.instance.counts(),
            memory: MemoryDiagnostics {
                allocated_bytes: s.allocated_bytes.load(Ordering::Relaxed),
                reserved_bytes,
            },
            bindless: s.bindings.diagnostics(),
        }
    }

    fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferHandle, RhiError> {
        buffer::create_buffer(&self.shared, desc)
    }

    fn destroy_buffer(&self, h: BufferHandle) {
        buffer::destroy_buffer(&self.shared, h);
    }

    fn buffer_address(&self, h: BufferHandle) -> Result<u64, RhiError> {
        buffer::buffer_address(&self.shared, h)
    }

    fn write_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError> {
        buffer::write_buffer(&self.shared, h, offset, data)
    }

    fn read_buffer(&self, h: BufferHandle, offset: u64, out: &mut [u8]) -> Result<(), RhiError> {
        buffer::read_buffer(&self.shared, h, offset, out)
    }

    fn upload_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError> {
        buffer::upload_buffer(&self.shared, h, offset, data)
    }

    fn create_texture(&self, desc: &TextureDesc) -> Result<TextureHandle, RhiError> {
        texture::create_texture(&self.shared, desc)
    }

    fn destroy_texture(&self, h: TextureHandle) {
        texture::destroy_texture(&self.shared, h);
    }

    fn texture_index(&self, h: TextureHandle) -> Result<u32, RhiError> {
        texture::texture_index(&self.shared, h)
    }

    fn upload_texture(&self, h: TextureHandle, data: &[u8]) -> Result<(), RhiError> {
        texture::upload_texture(&self.shared, h, data)
    }

    fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerHandle, RhiError> {
        texture::create_sampler(&self.shared, desc)
    }

    fn destroy_sampler(&self, h: SamplerHandle) {
        texture::destroy_sampler(&self.shared, h);
    }

    fn sampler_index(&self, h: SamplerHandle) -> Result<u32, RhiError> {
        texture::sampler_index(&self.shared, h)
    }

    fn create_swapchain(
        &self,
        surface: &VulkanSurface,
        desc: &SwapchainDesc,
    ) -> Result<SwapchainHandle, RhiError> {
        swapchain::create(&self.shared, surface, desc)
    }

    fn resize_swapchain(
        &self,
        sc: SwapchainHandle,
        width: u32,
        height: u32,
    ) -> Result<(), RhiError> {
        swapchain::resize(&self.shared, sc, width, height)
    }

    fn swapchain_format(&self, sc: SwapchainHandle) -> Result<Format, RhiError> {
        swapchain::format(&self.shared, sc)
    }

    fn destroy_swapchain(&self, sc: SwapchainHandle) {
        swapchain::destroy(&self.shared, sc, true);
    }

    fn create_shader_module(
        &self,
        desc: &ShaderModuleDesc,
    ) -> Result<ShaderModuleHandle, RhiError> {
        crate::commands::create_shader_module(&self.shared, desc)
    }

    fn create_graphics_pipeline(
        &self,
        desc: &GraphicsPipelineDesc,
    ) -> Result<PipelineHandle, RhiError> {
        crate::commands::create_graphics_pipeline(&self.shared, desc)
    }

    fn destroy_shader_module(&self, h: ShaderModuleHandle) {
        crate::commands::destroy_shader_module(&self.shared, h);
    }

    fn destroy_pipeline(&self, h: PipelineHandle) {
        crate::commands::destroy_pipeline(&self.shared, h);
    }

    fn begin_frame(&mut self) -> Result<FrameToken, RhiError> {
        self.shared.instance.check_validation_panic();
        self.frames.begin_frame()
    }

    fn acquire(&mut self, sc: SwapchainHandle) -> Result<Acquire, RhiError> {
        let span = self.frame_span();
        let _g = span.enter();
        let sem = self.frames.acquire_semaphore()?;
        let r = swapchain::acquire(&self.shared, sc, sem)?;
        if matches!(r, Acquire::Image(_)) {
            self.frames.mark_acquired();
        }
        tracing::trace!(swapchain = ?sc, outcome = ?r.kind(), "acquire");
        Ok(r)
    }

    fn begin_commands(&mut self, frame: &FrameToken) -> Result<VulkanCommands, RhiError> {
        self.frames.begin_commands(frame)
    }

    fn submit(&mut self, cmds: VulkanFinished, image: &SwapchainImage) -> Result<(), RhiError> {
        self.shared.instance.check_validation_panic();
        let render_done = swapchain::render_done(&self.shared, image)?;
        self.frames
            .submit(cmds, true, Some(render_done))
            .map(|_| ())
    }

    fn present(
        &mut self,
        sc: SwapchainHandle,
        image: SwapchainImage,
    ) -> Result<PresentStatus, RhiError> {
        let span = self.frame_span();
        let _g = span.enter();
        let r = swapchain::present(&self.shared, sc, &image)?;
        tracing::trace!(swapchain = ?sc, index = image.index, status = ?r, "present");
        Ok(r)
    }

    fn wait_idle(&self) -> Result<(), RhiError> {
        self.shared.wait_idle()
    }
}

trait AcquireKind {
    fn kind(&self) -> &'static str;
}
impl AcquireKind for Acquire {
    fn kind(&self) -> &'static str {
        match self {
            Acquire::Image(_) => "image",
            Acquire::OutOfDate => "out_of_date",
            Acquire::Minimized => "minimized",
        }
    }
}

const _: () = {
    const fn assert_send<T: Send>() {}
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send::<VulkanDevice>();
    assert_send_sync::<DeviceShared>();
    let _ = DEFAULT_FRAMES_IN_FLIGHT;
};

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(name: &str, ty: vk::PhysicalDeviceType) -> PhysicalDeviceFacts {
        PhysicalDeviceFacts {
            name: name.into(),
            vendor_id: 0,
            device_type: ty,
            api_version: vk::API_VERSION_1_3,
            driver: String::new(),
            extensions: vec!["VK_KHR_swapchain".into()],
            dynamic_rendering: true,
            synchronization2: true,
            timeline_semaphore: true,
            buffer_device_address: true,
            runtime_descriptor_array: true,
            shader_sampled_image_array_non_uniform_indexing: true,
            descriptor_binding_sampled_image_update_after_bind: true,
            descriptor_binding_partially_bound: true,
            bindless_sampled_images: 65536,
            bindless_samplers: 1024,
            max_image_dimension_2d: 16384,
            non_coherent_atom_size: 64,
            queue_families: vec![QueueFamilyFacts {
                index: 0,
                flags: vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
                queue_count: 1,
                can_present: None,
            }],
            max_device_local_heap: 1 << 30,
            surface: None,
        }
    }
    const D: vk::PhysicalDeviceType = vk::PhysicalDeviceType::DISCRETE_GPU;

    #[test]
    fn rejections() {
        let mut f = facts("a", D);
        f.api_version = vk::API_VERSION_1_2;
        assert!(matches!(evaluate(&f), Err(Rejection::ApiVersion { .. })));
        for (i, n) in ["dynamicRendering", "synchronization2", "timelineSemaphore"]
            .iter()
            .enumerate()
        {
            let mut f = facts("a", D);
            match i {
                0 => f.dynamic_rendering = false,
                1 => f.synchronization2 = false,
                _ => f.timeline_semaphore = false,
            }
            assert_eq!(evaluate(&f), Err(Rejection::MissingFeature(n)));
        }
        let mut f = facts("a", D);
        f.extensions.clear();
        assert_eq!(
            evaluate(&f),
            Err(Rejection::MissingExtension("VK_KHR_swapchain"))
        );
        let mut f = facts("a", D);
        f.queue_families[0].flags = vk::QueueFlags::COMPUTE;
        assert_eq!(evaluate(&f), Err(Rejection::NoGraphicsQueue));
        let mut f = facts("a", D);
        f.surface = Some(SurfaceFacts {
            format_count: 1,
            present_mode_count: 1,
        });
        f.queue_families[0].can_present = Some(false);
        assert_eq!(evaluate(&f), Err(Rejection::NoPresentSupport));
        f.queue_families[0].can_present = Some(true);
        assert_eq!(evaluate(&f), Ok(0));
        f.surface = Some(SurfaceFacts {
            format_count: 0,
            present_mode_count: 1,
        });
        assert_eq!(evaluate(&f), Err(Rejection::SurfaceUnusable));
        f.surface = Some(SurfaceFacts {
            format_count: 1,
            present_mode_count: 0,
        });
        assert_eq!(evaluate(&f), Err(Rejection::SurfaceUnusable));
    }

    #[test]
    fn reject_missing_each_bindless_feature() {
        type Clear = fn(&mut PhysicalDeviceFacts);
        let cases: [(&str, Clear); 5] = [
            ("bufferDeviceAddress", |f| f.buffer_device_address = false),
            ("runtimeDescriptorArray", |f| {
                f.runtime_descriptor_array = false
            }),
            ("shaderSampledImageArrayNonUniformIndexing", |f| {
                f.shader_sampled_image_array_non_uniform_indexing = false
            }),
            ("descriptorBindingSampledImageUpdateAfterBind", |f| {
                f.descriptor_binding_sampled_image_update_after_bind = false
            }),
            ("descriptorBindingPartiallyBound", |f| {
                f.descriptor_binding_partially_bound = false
            }),
        ];
        for (name, clear) in cases {
            let mut f = facts("a", D);
            clear(&mut f);
            assert_eq!(evaluate(&f), Err(Rejection::MissingFeature(name)));
        }
    }

    #[test]
    fn reject_small_bindless_limits() {
        let mut f = facts("a", D);
        f.bindless_sampled_images = 512;
        assert_eq!(
            evaluate(&f),
            Err(Rejection::BindlessLimits {
                images: 512,
                samplers: 1024
            })
        );
        let mut f = facts("a", D);
        f.bindless_samplers = 32;
        assert!(matches!(
            evaluate(&f),
            Err(Rejection::BindlessLimits { .. })
        ));
        let mut f = facts("a", D);
        f.bindless_sampled_images = 1024;
        f.bindless_samplers = 64;
        assert_eq!(evaluate(&f), Ok(0));
        assert!(
            Rejection::BindlessLimits {
                images: 512,
                samplers: 32
            }
            .to_string()
            .contains("512")
        );
    }

    #[test]
    fn bindless_features_checked_after_baseline() {
        let mut f = facts("a", D);
        f.buffer_device_address = false;
        f.synchronization2 = false;
        assert_eq!(
            evaluate(&f),
            Err(Rejection::MissingFeature("synchronization2"))
        );
        let mut f = facts("a", D);
        f.buffer_device_address = false;
        f.extensions.clear();
        assert_eq!(
            evaluate(&f),
            Err(Rejection::MissingFeature("bufferDeviceAddress"))
        );
    }

    #[test]
    fn headless_skips_present_check() {
        assert_eq!(evaluate(&facts("a", D)), Ok(0));
    }

    #[test]
    fn queue_family_choice() {
        let mut f = facts("a", D);
        let q = |index, flags| QueueFamilyFacts {
            index,
            flags,
            queue_count: 1,
            can_present: None,
        };
        f.queue_families = vec![
            q(0, vk::QueueFlags::GRAPHICS),
            q(1, vk::QueueFlags::COMPUTE),
            q(2, vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE),
        ];
        assert_eq!(evaluate(&f), Ok(2));
        f.queue_families = vec![
            q(3, vk::QueueFlags::GRAPHICS),
            q(5, vk::QueueFlags::GRAPHICS),
        ];
        assert_eq!(evaluate(&f), Ok(3));
    }

    #[test]
    fn scoring_and_selection() {
        let integrated = facts("intel", vk::PhysicalDeviceType::INTEGRATED_GPU);
        let cpu = facts("llvmpipe", vk::PhysicalDeviceType::CPU);
        let disc = facts("AMD RADV NAVI", D);
        assert!(score(&disc, None) > score(&integrated, None));
        assert!(score(&integrated, None) > score(&cpu, None));
        let mut big = facts("big", D);
        big.max_device_local_heap = 2 << 30;
        assert!(score(&big, None) > score(&disc, None));
        let nv = facts("NVIDIA", D);
        let all = [nv, disc];
        assert_eq!(select(&all, Some("radv")).unwrap().index, 1);
        assert_eq!(select(&all, Some("zzz")).unwrap().index, 0);
        assert_eq!(select(&all, None).unwrap().index, 0);
    }

    #[test]
    fn preferred_cannot_rescue_rejected_and_messages() {
        let mut bad = facts("radv", D);
        bad.dynamic_rendering = false;
        let ok = facts("other", vk::PhysicalDeviceType::CPU);
        assert_eq!(select(&[bad, ok], Some("radv")).unwrap().index, 1);
        let mut a = facts("alpha", D);
        a.synchronization2 = false;
        let mut b = facts("beta", D);
        b.api_version = vk::API_VERSION_1_1;
        let RhiError::NoSuitableDevice { reason } = select(&[a, b], None).unwrap_err() else {
            panic!("wrong error")
        };
        assert!(reason.contains("alpha: missing feature synchronization2"));
        assert!(reason.contains("beta: Vulkan 1.1 < 1.3"));
        assert!(matches!(
            select(&[], None),
            Err(RhiError::NoSuitableDevice { .. })
        ));
    }

    #[test]
    fn kinds_and_frames() {
        use vk::PhysicalDeviceType as T;
        assert_eq!(gpu_kind(T::DISCRETE_GPU), GpuKind::Discrete);
        assert_eq!(gpu_kind(T::INTEGRATED_GPU), GpuKind::Integrated);
        assert_eq!(gpu_kind(T::VIRTUAL_GPU), GpuKind::Virtual);
        assert_eq!(gpu_kind(T::CPU), GpuKind::Cpu);
        assert_eq!(gpu_kind(T::OTHER), GpuKind::Other);
        assert!(check_frames_in_flight(0).is_err() && check_frames_in_flight(4).is_err());
        assert!((1..=3).all(|n| check_frames_in_flight(n).is_ok()));
    }
}
