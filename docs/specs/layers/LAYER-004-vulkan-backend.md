# LAYER-004: Vulkan Backend

| Field | Value |
|---|---|
| Status | Accepted |
| Milestone | M1 |
| Crates | rhi-vulkan |
| Traces to | MASTER-000 §4.2, §5.2 rules 3 and 5, §8.4, §10, §14 M1, §20; ADR-0001; ADR-0004, ADR-0005, ADR-0007; ADR-0006 |
| Last updated | 2026-11-02 |

## 1. Purpose

Implement `rhi` on Vulkan 1.3 through `ash`. All `unsafe` Vulkan code lives here (ADR-0001). It is the only crate that depends on `ash`, `ash-window` and `gpu-allocator`.

## 2. Scope

| In (M1) | Later | Out |
|---|---|---|
| Instance, validation, debug messenger | Descriptor management (M2) | Frame/pass scheduling (LAYER-006) |
| Physical device selection, logical device, one graphics+present queue | Dedicated transfer / compute queues (M5) | Shader compilation (LAYER-008) |
| Surface (from raw handles), swapchain, recreation | Buffer/image allocation via gpu-allocator (M2; allocator created in M1) | Windowing |
| Per-frame command pools/buffers, timeline-semaphore frame sync | Pipeline cache persistence (MODULE-007) | Scene, assets |
| Shader module and graphics pipeline (dynamic rendering) | Timestamp queries (M5) | |
| Pools of handles, deferred destruction | Device-loss recovery (M7) | |

## 3. Crates

rhi-vulkan (Layer 2). Depends on foundation, rhi, `ash` (1.3 bindings), `ash-window`, `gpu-allocator` (feature `vulkan`), `raw-window-handle` 0.6, `thiserror`, `tracing`. Versions are pinned at implementation time with `cargo add`; expected set (confirmed 2026-11-02; see LAYER-004 verification log): ash 0.38, ash-window 0.13, raw-window-handle 0.6, winit 0.30 (platform only), gpu-allocator 0.28. (verified 2026-11-02)

## 4. Responsibilities

- Create the instance (API 1.3), enabling `VK_LAYER_KHRONOS_validation` and `VK_EXT_debug_utils` in debug builds; route messages to `tracing` (error -> `error!`, warning -> `warn!`, info -> `debug!`, verbose -> `trace!`).
- Select and create the device with required features (ADR-0004).
- Create `VkSurfaceKHR` from `RawDisplayHandle` + `RawWindowHandle` (via `ash_window::create_surface`) in `Instance::create_surface`; the `Surface` owns the `Arc<W>` window. Signature verified 2026-11-02.
- Own all objects in internal pools; expose only handles.
- Implement frame synchronization and the deferred destruction queue.
- Translate `vk::Result` into `RhiError`.

## 5. Non-responsibilities

- Defining engine-level API (rule 3): everything public is an `rhi` trait impl plus a `VulkanBackend` type.
- Choosing what to render, pass ordering, shader compile.
- Public `vk::*` types. Escape hatches for tests are `#[cfg(test)]` or `pub(crate)`.

## 6. Dependencies

| Allowed | Forbidden |
|---|---|
| foundation, rhi, ash, ash-window, gpu-allocator, raw-window-handle, thiserror, tracing | platform, winit (window arrives as generic `HasWindowHandle`), any layer 3+ crate |

## 7. Key concepts and data flow

### 7.1 Required Vulkan baseline (ADR-0004)

| Requirement | Source | Notes |
|---|---|---|
| Instance API version >= 1.3 | core | Device `apiVersion` >= 1.3 else `NoSuitableDevice` |
| `dynamicRendering` | `VkPhysicalDeviceVulkan13Features` | `vkCmdBeginRendering`/`vkCmdEndRendering`; no `VkRenderPass`/`VkFramebuffer` |
| `synchronization2` | `VkPhysicalDeviceVulkan13Features` | `vkCmdPipelineBarrier2`, `vkQueueSubmit2` |
| `timelineSemaphore` | `VkPhysicalDeviceVulkan12Features` | frame completion tracking |
| `VK_KHR_swapchain` | device extension | |
| Surface extensions | from `ash_window::enumerate_required_extensions(display)` | verified 2026-11-02 |
| Validation | `VK_LAYER_KHRONOS_validation`, `VK_EXT_debug_utils` | `ValidationMode::Auto` (debug builds): enable if present, `warn!` if missing; `On`: `Unsupported` error if missing (tests); `Off`: none. Synchronization validation is on in tests and opt-in via `ANVIL_SYNC_VALIDATION=1` |

Features are enabled by chaining `PhysicalDeviceVulkan12Features`/`Vulkan13Features` into `DeviceCreateInfo`. Further features (bindless indexing, buffer device address) are added with the milestone that needs them.

### 7.2 Physical device selection

1. Enumerate; reject devices failing 1.3 / features / `VK_KHR_swapchain` / having no queue family supporting graphics AND presenting to `compatible_surface` (`vkGetPhysicalDeviceSurfaceSupportKHR`). The surface is created first by `Instance::create_surface` (lead decision D1: no temporary surface); with `compatible_surface = None` (headless) the present check is skipped.
2. Score: `ANVIL_GPU`/`preferred_gpu` name match first, then discrete > integrated > virtual > other, then larger device-local heap.
3. Log chosen device and the rejection reason of every other device at `info`/`debug`.
4. Zero suitable devices -> `RhiError::NoSuitableDevice` with the aggregated reasons.

M1 uses a single queue family that supports graphics, compute and transfer, and presentation. Dedicated families come at M5.

### 7.3 Frame synchronization

```
 per frame slot i (0..frames_in_flight):
    command pool[i], one primary command buffer[i]
    acquire_sem[i]   (binary)        -- signalled by vkAcquireNextImageKHR
 per swapchain image k:
    render_done_sem[k] (binary)      -- signalled by submit, waited by present
 per device:
    graphics_timeline (timeline sem), next_value: u64

 begin_frame():  wait graphics_timeline >= slot[i].value   (vkWaitSemaphores)
                 reset pool[i]; run deferred destruction (items with value <= completed)
 acquire():      vkAcquireNextImageKHR(acquire_sem[i]) -> image k
 submit():       vkQueueSubmit2: wait acquire_sem[i] @ COLOR_ATTACHMENT_OUTPUT;
                 signal render_done_sem[k] and timeline = ++next_value;  slot[i].value = that
 present():      vkQueuePresentKHR wait render_done_sem[k]
```

- Binary semaphores are required for acquire/present (WSI); the timeline replaces per-frame fences. The Vulkan spec requires binary semaphores for both acquire and present (valid-usage rules), so this is not driver-dependent.
- `render_done_sem` is indexed per swapchain image (not per frame) to avoid reuse while presentation still holds it. Recreated with the swapchain.
- `slot[i]` index = `frame_counter % frames_in_flight`.

### 7.4 Swapchain lifecycle

- Format: prefer `B8G8R8A8_SRGB` + `SRGB_NONLINEAR_KHR`, else first with an sRGB format, else first available (log).
- Present mode: `vsync=true` -> FIFO (always available); `vsync=false` -> MAILBOX else IMMEDIATE else FIFO.
- Image count: `min_image_count + 1`, clamped to `max_image_count` when non-zero.
- Extent: `current_extent` unless it is `u32::MAX`, then clamp requested size to min/max. A zero extent => `Acquire::Minimized`, do not recreate.
- Usage `COLOR_ATTACHMENT`. Image views created per image; images registered in the texture pool so that `SwapchainImage.texture` is a normal `TextureHandle`.
- Recreate on: `ERROR_OUT_OF_DATE_KHR` from acquire or present, `SUBOPTIMAL_KHR` from present, explicit `resize_swapchain`. Pass the old swapchain as `old_swapchain`. M1 may `vkDeviceWaitIdle` before destroying the old one (simple and correct; acceptable shortcut, mark `// shortcut:`); later use the deferred queue.
- On out-of-date at acquire the frame is abandoned without submit (acquire semaphore was not signalled; do not wait on it). verify: semantics of semaphore state after a failed acquire.

### 7.5 Pools, allocator, destruction

```
 VulkanDevice
   Arc<InstanceShared> (instance, debug_messenger), physical, device, queue; surfaces are owned by the app's `Surface` handles and by swapchains
   allocator: Option<gpu_allocator::vulkan::Allocator>   (ADR-0007; Option so it can drop before the device)
   pools: Pool<VkSwapchain>, Pool<VkTexture>, Pool<VkShaderModule>, Pool<VkPipeline>
   deferred: VecDeque<(timeline_value, Doomed)>   Doomed = enum of native objects + allocations
   frames[], timeline, counters
```

M1 allocates no user memory except through the allocator for future buffers; swapchain images are owned by the swapchain. Shutdown order is §9.

### 7.6 Command encoder

`VulkanCommands` wraps the slot's primary command buffer plus a borrowed view of the pools needed to resolve handles (it resolves each handle at record time; a stale handle records the first error and the call becomes a no-op). Barriers: `texture_barrier(from,to)` maps via a table:

The full table, with separate source and destination columns, is in MODULE-004 §6.3. It is normative.
The swapchain `Undefined -> ColorAttachment` barrier MUST use COLOR_ATTACHMENT_OUTPUT as its source stage, not TOP_OF_PIPE. That is the stage the acquire semaphore wait is chained to. With TOP_OF_PIPE, the layout transition may run before the image is actually acquired (lead review 2026-11-02).

## 8. Public interface sketch

```rust
pub struct VulkanBackend;
impl Backend for VulkanBackend { type Device = VulkanDevice; fn create_device(desc: &DeviceDesc) -> Result<VulkanDevice, RhiError>; }

pub struct VulkanDevice { /* private */ }
impl Device for VulkanDevice { type Commands = VulkanCommands; /* see LAYER-003 */ }
pub struct VulkanCommands { /* private */ }
impl CommandEncoder for VulkanCommands { type Finished = VulkanFinished; /* ... */ }

// Internal module layout (each maps to a module spec)
mod instance;      // MODULE-001: instance, layers, debug messenger -> tracing
mod device;        // MODULE-002: physical selection, logical device, queue, features
mod swapchain;     // MODULE-003: surface, swapchain, image views, recreation
mod commands;      // MODULE-004: frame slots, pools, encoder, submit, timeline sync
mod error;         // vk::Result -> RhiError
mod destroy;       // deferred destruction queue
```

Nothing else is `pub`. No `vk::*` in public signatures.

## 9. Ownership and lifetime

- `VulkanDevice` owns every native object except the instance and surfaces; swapchains and views are owned by their pool entries; handles do not own. The Device holds an `Arc` to the shared instance internals (instance + debug messenger), so it cannot outlive the Instance; each `Surface` and each swapchain also holds one.
- Shutdown (`Drop`, also callable via `shutdown()` for error reporting): (1) `device_wait_idle`; (2) drain deferred queue; (3) destroy pipelines, shader modules; (4) destroy swapchains (views, semaphores, swapchain; then release their `Surface` reference); (5) destroy frame slots (pools, binary semaphores). The timeline semaphore is destroyed in `DeviceShared::drop`, immediately before `vkDestroyDevice`, because live encoders hold `Arc<DeviceShared>` (MODULE-004); (6) drop allocator; (7) destroy device and release the Device's instance `Arc`; (8) the `VkSurfaceKHR` is destroyed when the last `Surface`/swapchain reference drops; (9) debug messenger and instance are destroyed when the last instance `Arc` drops (after all surfaces and the device). Remaining pool entries at step 3 are logged as leaks at `warn`.
- A `Drop` guard in `VulkanDevice` ensures steps run even on panic unwinding paths where safe; no panics in `Drop`.
- Surface must be destroyed before the instance and while the `Arc<W>` window is alive; the `Surface` owns the `Arc<W>` and each swapchain keeps the `Surface` alive.

## 10. Threading model

M1: all calls from the thread that owns the window (main thread). `VulkanDevice: Send`. Internal pools sit behind `Mutex` only where `&self` creation methods require it (cheap, uncontended in M1). Vulkan external synchronization rules: one thread per command pool and per queue submit; only the main thread submits. Later: per-thread pools per frame slot, a submission mutex or dedicated submit thread (M5).

## 11. Error handling

Mapping table (`error.rs`):

| `vk::Result` | `RhiError` / outcome |
|---|---|
| `ERROR_DEVICE_LOST` | `DeviceLost` |
| `ERROR_OUT_OF_DEVICE_MEMORY` | `OutOfDeviceMemory` |
| `ERROR_OUT_OF_HOST_MEMORY` | `OutOfHostMemory` |
| `ERROR_OUT_OF_DATE_KHR` (acquire/present) | `Acquire::OutOfDate` / `PresentStatus::Recreate` (status, not error) |
| `SUBOPTIMAL_KHR` (present) | `PresentStatus::Recreate` |
| `ERROR_SURFACE_LOST_KHR` | `RhiError::SurfaceLost` (fatal in M1; the app shuts down cleanly) |
| `ERROR_LAYER_NOT_PRESENT`, `ERROR_EXTENSION_NOT_PRESENT`, `ERROR_FEATURE_NOT_PRESENT`, `ERROR_INCOMPATIBLE_DRIVER` | `Unsupported` / `NoSuitableDevice` |
| any other | `Backend { op: "<vk call name>", code }` |

`gpu-allocator` errors map to `OutOfDeviceMemory`/`Backend`. Validation errors are logged, never turned into `Err` (the baseline test treats any `error!` from validation as test failure, see §13). No `unwrap()` on Vulkan results outside tests. Every `unsafe` block has `// SAFETY:`.

## 12. Diagnostics and observability

- Debug messenger -> `tracing` target `rhi_vulkan::validation`; message id name and objects included.
- Names: `set_debug_utils_object_name` for swapchain, images, pipelines, shader modules when `debug_utils` is enabled (from the `label` fields).
- Command labels: `vkCmdBeginDebugUtilsLabelEXT`/End when available.
- `info!` at startup: instance version, layers enabled, chosen GPU, driver, queue family, swapchain format/mode/count.
- Counters (frame number, pending deferred items) available for `MemorySnapshot` later.

## 13. Testing strategy

| Level | What | Where |
|---|---|---|
| Unit | error mapping, format/present-mode selection (pure functions), deferred queue ordering, barrier table | `#[cfg(test)]` |
| Integration (needs GPU/ICD) | create device headless (`display: None`, `compatible_surface: None`), create/destroy shader module and pipeline from fixed SPIR-V; swapchain tests need a window, so the triangle example runs N frames then exits | `tests/` marked `#[ignore]` unless `ANVIL_GPU_TESTS=1` |
| Validation gate | run the triangle for 300 frames with resize events; test passes only if no validation message at severity ERROR/WARNING (a test `tracing` layer counts them) | triangle smoke test |
| CI | CPU-only unit tests on all runners; headless integration tests on Mesa lavapipe (software Vulkan) on GitHub ubuntu runners; synchronization validation ON in tests | MASTER-000 §17 |

## 14. Acceptance criteria (M1)

- [ ] Instance created with validation in debug; messages appear in `tracing` output.
- [ ] Device selection logs all candidates; picks a Vulkan 1.3 GPU with dynamic rendering, sync2, timeline semaphores.
- [ ] Surface and swapchain created from `Arc<Window>`; no `ash` in platform.
- [ ] Triangle renders for >= 300 frames at `frames_in_flight = 2` with no validation errors or warnings (including synchronization validation run once manually).
- [ ] Window resize and minimize/restore: swapchain is recreated, no crash, no validation error.
- [ ] Clean shutdown: no validation "object not destroyed" messages; leak warnings zero.
- [ ] No `VkRenderPass`/`VkFramebuffer` created anywhere (`grep` check).
- [ ] All `unsafe` blocks have `// SAFETY:`; `clippy -D warnings` clean.
- [ ] Setting `frames_in_flight` to 1 and 3 works.

## 15. Open questions

1. Pipeline cache (`VkPipelineCache`): create empty in M1, persist to disk in M5 (MODULE-007).
2. `buffer_device_address` in the allocator in M1 (default: no).
3. `vkDeviceWaitIdle` on swapchain recreation: acceptable for M1; replace with deferred destruction in M2.

### Resolved (lead review 2026-11-02)

1. Surface before device (was Q1): the Instance/Device split of LAYER-003 applies; the surface is created by `Instance::create_surface` and passed to `create_device` as `compatible_surface`; no temporary surface.
2. Validation layer missing (was Q2): `ValidationMode` Auto warns and continues (debug builds), On fails with `Unsupported`, Off skips.
3. CI GPU tests (was Q3): Mesa lavapipe on GitHub ubuntu runners for headless integration tests (verify: lavapipe Vulkan 1.3 feature support in the runner's Mesa).
4. Synchronization validation (was Q4): ON in tests; opt-in via `ANVIL_SYNC_VALIDATION=1` for interactive runs. Enabled through `VK_EXT_layer_settings`, with `VK_EXT_validation_features` as fallback (MODULE-001 Q1).
5. `ERROR_SURFACE_LOST_KHR` (was Q5): fatal in M1 (`RhiError::SurfaceLost`); the app shuts down cleanly; recreation deferred.
6. Crate versions (was Q8, version part): pinned at implementation time with `cargo add`; expected ash 0.38, ash-window 0.13, raw-window-handle 0.6, winit 0.30, gpu-allocator 0.28 (confirmed 2026-11-02; see LAYER-004 verification log).

## 16. Related modules

| Module | Content | State |
|---|---|---|
| MODULE-001-vulkan-instance | instance, layers, debug messenger | Accepted |
| MODULE-002-vulkan-device | physical selection, logical device, queue, features | Accepted |
| MODULE-003-vulkan-swapchain | surface, swapchain, recreation, per-image semaphores | Accepted |
| MODULE-004-command-manager | frame slots, pools, encoder, submit, timeline, deferred destruction | Accepted |

## Verification log (2026-11-02)

Checked against crate sources, a compile check and `vulkaninfo` on the dev machine (RX 9070 XT, RADV, Mesa 26.2.5, Vulkan 1.4).

| Item | Result |
|---|---|
| Versions | ash 0.38.0+1.3.281, ash-window 0.13.0, raw-window-handle 0.6.2, winit 0.30.13, **gpu-allocator 0.28.0** (not 0.27), shaderc 0.10.1. One shared `ash`, no duplicates |
| ash 0.38 API | `Entry::load -> Result<_, LoadingError>`, `try_enumerate_instance_version`, `queue_submit2`, `cmd_pipeline_barrier2`, `cmd_begin_rendering`, `wait_semaphores`, `get_semaphore_counter_value`, `reset_command_pool`, `ash::{khr::swapchain, khr::surface, ext::debug_utils, ext::layer_settings, ext::validation_features}::NAME`, `REMAINING_*`, `QUEUE_FAMILY_IGNORED`, `ERROR_NATIVE_WINDOW_IN_USE_KHR` all present |
| gpu-allocator 0.28 | `AllocatorCreateDesc { instance, device, physical_device, debug_settings, buffer_device_address, allocation_sizes }`; `AllocationError::OutOfMemory` maps to `OutOfDeviceMemory`; `generate_report()` exists for statistics |
| winit `Window` | satisfies `HasWindowHandle + HasDisplayHandle + Send + Sync + 'static` (compile check) |
| shaderc | links the system `/usr/lib` libshaderc; a GLSL→SPIR-V test passes (magic `0x07230203`) |
| Instance extensions (dev machine) | `VK_EXT_debug_utils`, `VK_EXT_headless_surface`, `VK_EXT_layer_settings`, wayland/xcb/xlib surface |
| Validation layer | `VK_LAYER_KHRONOS_validation` 1.4.363 installed; sync-validation setting key `validate_sync` (from the layer manifest) |
| Still open | lavapipe 1.3 + headless_surface on the CI runner (first CI run answers it); acquire-semaphore state after `OUT_OF_DATE` (the spec reading is "unsignalled", confirm with validation once the layer is installed) |

## Verification log (2026-11-02, implementation run)

Dev machine: RX 9070 XT, RADV (Mesa 26.2.5), validation layer 1.4.363, Wayland.

| Item | Result |
|---|---|
| `VK_KHR_swapchain` on a headless instance | Validation error `VUID-vkCreateDevice-ppEnabledExtensionNames-01387`: the device extension needs the instance extension `VK_KHR_surface`. The backend now always requests `VK_KHR_surface` (optional) even without a display. |
| Sync validation | `VK_EXT_layer_settings` key `validate_sync` works; layer reports "Synchronization" enabled. 120 triangle frames with it: zero errors and zero warnings. |
| `ERROR_OUT_OF_DATE` acquire-semaphore state | Not exercised: the compositor never returned OUT_OF_DATE. Still open. |
| Wayland | Every acquire reports `SUBOPTIMAL` on this compositor; frames still render and present returns Ok. With FIFO and a hidden or occluded window the compositor throttles presents to about 1 per second, so the run uses `ANVIL_VSYNC=0` (MAILBOX). |
| Deadlock found by the tests | `diagnostics()` built a struct literal holding a pool guard while calling a function that locks the same mutex. Fixed by taking the guards in separate statements. |
