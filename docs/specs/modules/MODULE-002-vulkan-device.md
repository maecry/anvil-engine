# MODULE-002: Vulkan Device

| Field | Value |
|---|---|
| Status | Accepted |
| Crate | rhi-vulkan |
| Layer | LAYER-004 |
| Milestone | M1 |
| Traces to | FR-VK-002, FR-VK-003, FR-RHI-012, FR-VK-009 (allocator creation only), FR-VK-013, NFR-ARCH-003, NFR-QUAL-004, NFR-TEST-002; MASTER-000 §8.4, §14 M1; ADR-0004, ADR-0007 |
| Last updated | 2026-11-02 |

## 1. Purpose

Choose one physical GPU, create the logical `VkDevice` with the Vulkan 1.3 baseline features enabled (ADR-0004), fetch the single graphics+present queue, create the `gpu-allocator` allocator, and package these as `DeviceShared`, the object every other module (swapchain, command manager, encoders) holds an `Arc` to. Selection logic is pure over plain data so it is testable without a GPU.

## 2. Responsibilities

- `rhi::Instance::create_device(&DeviceDesc, Option<&Surface>)` for the Vulkan backend.
- Enumerate physical devices, read their facts, reject or score them, log every decision.
- Create the device with `dynamicRendering`, `synchronization2`, `timelineSemaphore` and `VK_KHR_swapchain`.
- Own `DeviceShared`: raw device, queue (behind a mutex), queue family, loaders, allocator, handle pools, `DeviceInfo`, a keep-alive `Arc` to the instance.
- Orchestrate creation of the frame machinery (MODULE-004) and teardown order (LAYER-004 §9).
- Debug naming helper (`VK_EXT_debug_utils`).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Instance, layers, messenger | MODULE-001 |
| Swapchain, surface, present | MODULE-003 |
| Command pools, timeline semaphore, submit, deferred queue | MODULE-004 |
| Buffer/image allocation API | MODULE-005, MODULE-006 (M2); the allocator object is only created here |
| Dedicated transfer/compute queues | M5 |

## 4. Dependencies

Allowed: MODULE-001 (`InstanceShared`), foundation (`Pool`, `Handle`), rhi, `ash`, `gpu-allocator` (feature `vulkan`), `tracing`, `thiserror`. Forbidden: platform, `winit`, layer 3+ crates. Expected versions (confirmed 2026-11-02; see LAYER-004 verification log): ash 0.38, gpu-allocator 0.28.

## 5. Public API

```rust
pub struct VulkanDevice { /* private: shared: Arc<DeviceShared>, frames: CommandManager, swapchains: SwapchainManager */ }
impl rhi::Device for VulkanDevice { type Surface = VulkanSurface; type Commands = VulkanCommands; /* LAYER-003 §8 */ }
impl VulkanDevice {
    /// Idempotent explicit teardown (LAYER-004 §9 steps 1-7); `Drop` calls it and logs errors.
    pub fn shutdown(&mut self) -> Result<(), RhiError>;
}
// rhi::Instance::create_device for VulkanInstance:
//   fn create_device(&self, desc: &DeviceDesc, compatible_surface: Option<&VulkanSurface>) -> Result<VulkanDevice, RhiError>
```

`rhi::DeviceDesc` (LAYER-003 §8, no `display`): `preferred_gpu: Option<String>`, `frames_in_flight: u32`. Env `ANVIL_GPU` overrides `preferred_gpu`.

Internal:

```rust
pub(crate) struct DeviceShared {
    pub instance: Arc<InstanceShared>,                 // keeps the instance alive; declared LAST so it drops last
    pub physical: vk::PhysicalDevice,
    pub raw: ash::Device,
    pub queue_family: u32,
    pub queue: Mutex<vk::Queue>,                       // external synchronization for submit, present, wait_idle
    pub swapchain_fn: ash::khr::swapchain::Device,
    pub debug_fn: Option<ash::ext::debug_utils::Device>,
    pub allocator: Mutex<Option<gpu_allocator::vulkan::Allocator>>,   // Option: dropped before the device
    pub info: rhi::DeviceInfo,
    pub pools: Pools,
    pub pipeline_cache: vk::PipelineCache,            // empty in M1 (LAYER-004 Q1); persisted in M5 (MODULE-007)
    pub sync: SyncShared,                              // MODULE-004: timeline semaphore, last_submitted, deferred queue (shared so `&self` create/destroy can enqueue)
    pub present_capable: bool,                         // queue family can present to the creation surface (false when headless)
}
pub(crate) struct Pools {
    pub textures: RwLock<Pool<VkTexture>>,             // swapchain images in M1; MODULE-006 grows it
    pub shader_modules: RwLock<Pool<VkShaderModule>>,  // MODULE-004
    pub pipelines: RwLock<Pool<VkPipeline>>,           // MODULE-004
    pub swapchains: Mutex<Pool<VkSwapchain>>,          // MODULE-003
}
impl DeviceShared {
    pub(crate) fn set_name<H: vk::Handle>(&self, handle: H, name: &str);   // no-op without debug_utils
    pub(crate) fn lock_queue(&self) -> MutexGuard<'_, vk::Queue>;
}
impl Drop for DeviceShared { fn drop(&mut self); }     // wait_idle (best effort), allocator.take(), destroy_device

// ---- selection (pure, no Vulkan calls) ----
pub(crate) struct PhysicalDeviceFacts {
    pub name: String, pub vendor_id: u32, pub device_type: vk::PhysicalDeviceType,
    pub api_version: u32, pub driver: String,
    pub extensions: Vec<String>,
    pub dynamic_rendering: bool, pub synchronization2: bool, pub timeline_semaphore: bool,
    pub queue_families: Vec<QueueFamilyFacts>,
    pub max_device_local_heap: u64,                    // bytes; largest heap with DEVICE_LOCAL
    pub surface: Option<SurfaceFacts>,                 // Some iff a compatible_surface was given
}
pub(crate) struct QueueFamilyFacts { pub index: u32, pub flags: vk::QueueFlags, pub queue_count: u32, pub can_present: Option<bool> }
pub(crate) struct SurfaceFacts { pub format_count: u32, pub present_mode_count: u32 }

pub(crate) enum Rejection {
    ApiVersion { found: u32 },
    MissingFeature(&'static str),                      // "dynamicRendering" | "synchronization2" | "timelineSemaphore"
    MissingExtension(&'static str),                    // "VK_KHR_swapchain"
    NoGraphicsQueue,
    NoPresentSupport,                                  // graphics queue exists, none can present to the surface
    SurfaceUnusable,                                   // zero formats or zero present modes
}
impl std::fmt::Display for Rejection { /* human readable, goes into NoSuitableDevice.reason */ }

pub(crate) fn evaluate(f: &PhysicalDeviceFacts) -> Result<u32 /* queue family index */, Rejection>;
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Score { pub preferred_name: bool, pub kind_rank: u8, pub device_local_mib: u64 }
pub(crate) fn score(f: &PhysicalDeviceFacts, preferred: Option<&str>) -> Score;
pub(crate) struct Selection { pub index: usize, pub queue_family: u32 }
pub(crate) fn select(facts: &[PhysicalDeviceFacts], preferred: Option<&str>) -> Result<Selection, RhiError>;
pub(crate) fn gpu_kind(t: vk::PhysicalDeviceType) -> rhi::GpuKind;
```

## 6. Data structures

### 6.1 Required baseline (ADR-0004, LAYER-004 §7.1)

| Requirement | Check | Rejection |
|---|---|---|
| `properties.api_version >= vk::API_VERSION_1_3` | `get_physical_device_properties` | `ApiVersion` |
| `dynamicRendering` | `PhysicalDeviceVulkan13Features` | `MissingFeature("dynamicRendering")` |
| `synchronization2` | `PhysicalDeviceVulkan13Features` | `MissingFeature("synchronization2")` |
| `timelineSemaphore` | `PhysicalDeviceVulkan12Features` | `MissingFeature("timelineSemaphore")` |
| device extension `VK_KHR_swapchain` | `enumerate_device_extension_properties` | `MissingExtension` |
| a queue family with `GRAPHICS` and `queue_count >= 1` | `get_physical_device_queue_family_properties` | `NoGraphicsQueue` |
| that family can present to `compatible_surface` (skipped when `None`) | `surface_fn.get_physical_device_surface_support(pd, family, surface)` | `NoPresentSupport` |
| `compatible_surface` has >= 1 format and >= 1 present mode (skipped when `None`) | `get_physical_device_surface_formats` / `_present_modes` | `SurfaceUnusable` |

`VK_KHR_swapchain` is required even for headless devices, so any device can later create a swapchain for a surface it can present to.

### 6.2 Queue family choice (inside `evaluate`)

Candidates: families with `GRAPHICS`, `queue_count >= 1`, and (if a surface was given) `can_present == Some(true)`. Pick the candidate that also has `COMPUTE`; ties and the no-compute case: lowest index. Transfer is implied by graphics/compute (verified 2026-11-02). One queue, priority 1.0. If no candidate: `NoPresentSupport` when a graphics family exists but none can present, else `NoGraphicsQueue`.

### 6.3 Scoring (`Score`, derived lexicographic `Ord`, higher wins)

| Order | Component | Values |
|---|---|---|
| 1 | `preferred_name` | true iff `preferred` (arg, from `ANVIL_GPU` else `DeviceDesc.preferred_gpu`) is a case-insensitive substring of the device name |
| 2 | `kind_rank` | DISCRETE 4, INTEGRATED 3, VIRTUAL 2, CPU 1 (lavapipe), OTHER 0 |
| 3 | `device_local_mib` | `max_device_local_heap / 2^20` |

Ties: lowest enumeration index wins (deterministic). CPU devices are accepted (CI uses lavapipe) but never beat a real GPU. A preferred name only reorders suitable devices; it never rescues a rejected one. If a preferred name was given and no suitable device matches, `warn!` and fall back to the normal order.

### 6.4 Features enabled

```rust
let mut f12 = vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
let mut f13 = vk::PhysicalDeviceVulkan13Features::default().dynamic_rendering(true).synchronization2(true);
let info = vk::DeviceCreateInfo::default()
    .queue_create_infos(&[queue_info]).enabled_extension_names(&[khr::swapchain::NAME.as_ptr()])
    .push_next(&mut f12).push_next(&mut f13);     // no pEnabledFeatures: do not mix with Features2
```
ash 0.38 builder/`push_next` API and `ash::khr::swapchain::NAME` (verified 2026-11-02). No other features in M1; bindless and buffer device address arrive with the milestone that needs them.

### 6.5 `DeviceInfo`

`name` from `device_name` (NUL-terminated array), `vendor_id`, `kind = gpu_kind(type)`, `api_version = (major, minor, patch)` via `vk::api_version_major/minor/patch`, `driver = "<driver_name> <driver_info>"` from `PhysicalDeviceVulkan12Properties` (verify: chained through `get_physical_device_properties2`).

## 7. Ownership and lifetime

Create order inside `create_device`:

| # | Object | Owner | Destroyed by |
|---|---|---|---|
| 1 | facts (plain data) | local | dropped |
| 2 | `VkDevice` + `DeviceShared` shell (queue, loaders, `pools`, `sync` incl. the timeline semaphore, empty `VkPipelineCache`, `instance` Arc) | `Arc<DeviceShared>` | `Drop for DeviceShared`: wait idle, drain `sync.deferred`, destroy timeline and pipeline cache, `allocator.take()`, `destroy_device(None)` |
| 3 | `gpu_allocator::vulkan::Allocator` | `DeviceShared.allocator` | `allocator.take()` before `destroy_device` |
| 4 | frame machinery (command pools, timeline, per-slot semaphores) | `VulkanDevice.frames` (MODULE-004) | `VulkanDevice::shutdown` step 5 |
| 5 | `SwapchainManager` (empty) | `VulkanDevice.swapchains` | `shutdown` step 4 |

`DeviceShared` is constructed at step 2, immediately after `vkCreateDevice` succeeds, so any later failure in `create_device` unwinds through its `Drop` (no leaked `VkDevice`). Fields are ordered so `instance` (declared last) drops after the `Drop::drop` body has destroyed the device: the Device can never outlive the Instance.

`VulkanDevice::shutdown` follows LAYER-004 §9 exactly: (1) `device_wait_idle`, (2) drain deferred queue, (3) destroy pipelines and shader modules, warning once per leaked entry, (4) destroy swapchains, (5) destroy frame slots (the timeline semaphore is destroyed by `DeviceShared::drop` just before `destroy_device`, because encoders may keep the `Arc` alive), (6) `allocator.take()` (the `Drop` of `DeviceShared` repeats this if encoders still hold the `Arc`), (7) drop the device's `Arc<DeviceShared>`; `destroy_device` runs when the last `Arc` (including any leaked encoder) drops.

Unsafe invariants:

| Block | Invariant to justify |
|---|---|
| property/feature/queue queries | `PhysicalDevice` handles come from `enumerate_physical_devices` of this instance, and the instance is alive (`Arc<InstanceShared>` held) |
| `create_device` | All pointers (queue priorities, extension name array, feature chain structs) outlive the call; feature structs are not shared across threads |
| `get_device_queue(family, 0)` | `family` was in `DeviceQueueCreateInfo` with count >= 1, index 0 < count |
| loaders `khr::swapchain::Device::new`, `debug_utils::Device::new` | Built from the same instance/device pair that created them; stored in `DeviceShared`, never outliving `raw` |
| `destroy_device` | All child objects were destroyed first (shutdown order above), the queue is idle (`device_wait_idle`), called exactly once from `Drop` |
| `Allocator::new` | `instance`/`device` clones are valid for the allocator's lifetime; the allocator is dropped before `destroy_device` |

## 8. Execution flow

`create_device(desc, compatible_surface)`:

1. `desc.frames_in_flight` must be 1..=3 else `InvalidDescriptor`.
2. If `compatible_surface` is `Some`: `Arc::ptr_eq(&surface.inner.instance, &self.shared)` else `InvalidDescriptor("surface belongs to another instance")`.
3. `preferred = env ANVIL_GPU or desc.preferred_gpu`.
4. `instance.enumerate_physical_devices()`; empty -> `NoSuitableDevice{reason:"no Vulkan physical devices"}`.
5. For each device build `PhysicalDeviceFacts`: `get_physical_device_properties2` (chain `Vulkan12Properties`), `get_physical_device_features2` (chain `Vulkan12Features`, `Vulkan13Features`), `enumerate_device_extension_properties`, queue family properties, memory properties, and when a surface is given the per-family present support, formats and present modes.
6. `select(&facts, preferred)`: run `evaluate` on all (log each rejection at `info`/`debug`), score survivors, take the maximum. Zero survivors -> `NoSuitableDevice{reason}` listing `"<name>: <Rejection>"` for every device, joined by `"; "`.
7. `info!` the chosen device (name, kind, API, driver, queue family) and `debug!` the scores.
8. Build `DeviceCreateInfo` (section 6.4); `instance.create_device(pd, &info, None)`.
9. Build loaders, fetch the queue, wrap in `DeviceShared` (step 2 of the ownership table).
10. `Allocator::new(&AllocatorCreateDesc { instance, device, physical_device, debug_settings: default, buffer_device_address: false, allocation_sizes: default })` fields (verified 2026-11-02) against gpu-allocator 0.28.
11. `CommandManager::new(shared.clone(), desc.frames_in_flight)` (MODULE-004), `SwapchainManager::new(shared.clone())` (MODULE-003).
12. Return `VulkanDevice`.

## 9. Threading model

`DeviceShared` is `Send + Sync` (enforced by a compile-time assertion test). The queue sits behind a `Mutex<vk::Queue>`; submit, present and `vkDeviceWaitIdle` must hold the guard (Vulkan requires external synchronization of the queue, and of all queues for `vkDeviceWaitIdle`). The allocator is behind a `Mutex` (gpu-allocator requires `&mut`). Pools use `RwLock` (reads on the encoder path, writes on create/destroy). M1 uses one thread; locks are uncontended. `VulkanDevice: Send`, not `Sync`.

## 10. Error handling

| Condition | `RhiError` |
|---|---|
| No physical devices, or all rejected | `NoSuitableDevice{reason}` (aggregated) |
| `ERROR_DEVICE_LOST` | `DeviceLost` |
| `ERROR_OUT_OF_HOST_MEMORY` / `ERROR_OUT_OF_DEVICE_MEMORY` | `OutOfHostMemory` / `OutOfDeviceMemory` |
| `ERROR_FEATURE_NOT_PRESENT`, `ERROR_EXTENSION_NOT_PRESENT` at `vkCreateDevice` | `Unsupported` (should be unreachable after `evaluate`) |
| `ERROR_SURFACE_LOST_KHR` during surface queries | `SurfaceLost` |
| gpu-allocator error | `AllocationError::OutOfMemory` -> `OutOfDeviceMemory`; every other variant -> `Backend{op:"gpu_allocator", code:-1}`, with the variant logged (variant names verified against 0.28) |
| `frames_in_flight` outside 1..=3, foreign surface | `InvalidDescriptor` |

Errors in `Drop`/`shutdown` are logged at `error!`, never panic.

## 11. Performance requirements

- Device selection runs once at startup; no per-frame work in this module.
- Selection is O(devices x families); log its duration at `info`.
- `queue` lock hold time is limited to the Vulkan call itself (never held across a wait on the timeline semaphore).
- No numeric targets (NFR-PERF-012).

## 12. Testing requirements

Unit tests (no GPU; construct `PhysicalDeviceFacts` by hand):

| Test | Checks |
|---|---|
| `reject_old_api` | 1.2 device -> `ApiVersion` |
| `reject_missing_each_feature` | three cases, each names its feature |
| `reject_missing_swapchain_extension` | `MissingExtension("VK_KHR_swapchain")` |
| `reject_no_graphics_queue` | compute-only family -> `NoGraphicsQueue` |
| `reject_no_present_support` | graphics family, `can_present=Some(false)` -> `NoPresentSupport` |
| `headless_skips_present_check` | `surface=None` accepts a family with `can_present=None` |
| `reject_unusable_surface` | zero formats or zero modes -> `SurfaceUnusable` |
| `queue_family_prefers_compute` | graphics-only family 0 vs graphics+compute family 2 -> 2 |
| `queue_family_tiebreak_lowest_index` | deterministic |
| `score_discrete_beats_integrated_beats_cpu` | `kind_rank` order |
| `score_heap_breaks_ties` | larger heap wins at equal kind |
| `preferred_name_wins_case_insensitive` | "radv" matches "AMD RADV NAVI" over a discrete NVIDIA |
| `preferred_name_cannot_rescue_rejected` | rejected device never chosen |
| `preferred_name_no_match_falls_back` | normal order, warn path |
| `all_rejected_message_lists_every_device` | `NoSuitableDevice.reason` contains each name and reason |
| `empty_device_list` | `NoSuitableDevice` |
| `gpu_kind_mapping` | all five types |
| `frames_in_flight_range` | 0 and 4 -> `InvalidDescriptor`; 1, 2, 3 ok |
| `device_shared_is_send_sync` | compile-time assertion |

Integration tests (lavapipe on CI, `ANVIL_GPU_TESTS=1`, validation `On`, sync validation on):

| Test | Checks |
|---|---|
| `create_device_headless` | `compatible_surface: None`; `info()` non-empty; `kind == Cpu` on CI |
| `device_enables_baseline_features` | record and submit an empty `begin_rendering`/`end_rendering` pair on a test image: no validation error (proves dynamicRendering/sync2/timeline are on) |
| `anvil_gpu_env_override` | `ANVIL_GPU=zzz-no-match` still creates a device (fallback) |
| `foreign_surface_rejected` | needs a surface: `#[ignore]` unless a display is available; otherwise covered by the unit test of the `ptr_eq` helper |
| `drop_order_device_then_instance` / `drop_order_instance_then_device` | zero validation messages either way |
| `shutdown_idempotent` | second `shutdown()` is a no-op `Ok` |
| `create_device_failure_does_not_leak` | inject failure after step 9 (cfg(test) hook); validation reports no live objects |

## 13. Acceptance criteria

- [ ] Selection logs every candidate with accept/reject reason and the chosen device.
- [ ] A GPU lacking 1.3, `dynamicRendering`, `synchronization2`, `timelineSemaphore`, `VK_KHR_swapchain`, or a present-capable graphics queue is rejected with the matching `Rejection`.
- [ ] `ANVIL_GPU` / `preferred_gpu` reorders only suitable devices.
- [ ] Lavapipe is accepted when it is the only device; a discrete GPU wins over it when both exist.
- [ ] The device is created with exactly the section 6.4 features and one queue.
- [ ] The allocator exists after `create_device` and is dropped before `vkDestroyDevice`.
- [ ] The Device never outlives the Instance: either drop order is validation-clean.
- [ ] `shutdown()` is idempotent and `Drop` never panics.
- [ ] `clippy -D warnings` clean; every `unsafe` block has `// SAFETY:` covering the section 7 invariants.

## 14. Traceability (FR/NFR IDs)

| Requirement | Satisfied by |
|---|---|
| FR-VK-002 | steps 4-7, section 6.1-6.3 |
| FR-VK-003, FR-RHI-012 | section 6.2, step 8 (single queue family; multi-queue deferred to M5) |
| FR-VK-009 | allocator created in step 10; allocation API is M2 |
| FR-VK-013, NFR-ARCH-003 | only this crate touches `ash`/`gpu-allocator` |
| NFR-QUAL-004 | section 7 unsafe invariants |
| NFR-TEST-002 | integration tests above |
| ADR-0004 | section 6.1 |
| ADR-0007 | allocator wrapped inside the crate, `Option` so it drops before the device |

## 15. Open questions

1. Should an integrated GPU outrank a discrete GPU that cannot present to the surface? Current rule: such a discrete GPU is rejected, so the integrated one wins. Confirm with a hybrid-graphics laptop test.
2. Do we want an explicit CPU-device opt-out (`ANVIL_ALLOW_CPU=0`) for release builds? Default: CPU devices accepted everywhere.
3. Resolved: the Vulkan spec guarantees that GRAPHICS or COMPUTE families support transfer even when TRANSFER_BIT is not reported. Do not require the bit.
4. `max_device_local_heap` on integrated GPUs reports shared memory; acceptable as a tiebreak only.
