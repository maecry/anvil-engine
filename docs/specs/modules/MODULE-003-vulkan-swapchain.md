# MODULE-003: Vulkan Swapchain and Surface

| Field | Value |
|---|---|
| Status | Accepted |
| Crate | rhi-vulkan |
| Layer | LAYER-004 |
| Milestone | M1 |
| Traces to | FR-VK-004, FR-VK-005, FR-RHI-016, FR-PLAT-003, FR-PLAT-004, FR-VK-013, NFR-REL-002, NFR-REL-004, NFR-TEST-002; MASTER-000 §8.4, §14 M1, §15.2; RESEARCH-001; ADR-0006 |
| Last updated | 2026-11-02 |

## 1. Purpose

Create the `VkSurfaceKHR` from a window the surface owns, create and recreate `VkSwapchainKHR` objects with deterministic format / present-mode / extent / image-count rules, hand out swapchain images as ordinary `TextureHandle`s, and perform acquire and present with correct binary-semaphore usage (RESEARCH-001: acquire semaphore per frame slot, render-done semaphore per swapchain image).

## 2. Responsibilities

- `Instance::create_surface<W>`: surface creation from raw handles; the `Surface` owns `Arc<W>`.
- `Device::create_swapchain / resize_swapchain / swapchain_format / destroy_swapchain`, `acquire`, `present` for the Vulkan backend.
- Pure selection functions: format, present mode, extent, image count, composite alpha.
- Image views per swapchain image; registration of images in the texture pool.
- Per-image render-done semaphores; recreation with `old_swapchain`.
- Minimized handling and out-of-date / suboptimal classification.
- Fatal `SurfaceLost` mapping (D6).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Acquire semaphore objects and the frame-slot index | MODULE-004 (`CommandManager`) lends the semaphore to `acquire` |
| Waiting on the acquire semaphore, signalling render-done | MODULE-004 `submit` |
| Image layout transitions (Undefined -> ColorAttachment -> Present) | recorded by the app via `texture_barrier`, translated by MODULE-004 |
| Window events, deciding when to resize | platform / app |
| Surface recreation after `SurfaceLost` | deferred (D6) |
| `VK_KHR_swapchain_maintenance1` present fences | not used in M1 |

## 4. Dependencies

Allowed: MODULE-001 (`InstanceShared`, `surface_fn`), MODULE-002 (`DeviceShared`, `Pools`), MODULE-004 (acquire semaphore access), foundation, rhi, `ash`, `ash-window`, `raw-window-handle` 0.6, `tracing`. Forbidden: platform, `winit`. Expected versions (confirmed 2026-11-02; see LAYER-004 verification log): ash 0.38, ash-window 0.13, raw-window-handle 0.6.

## 5. Public API

```rust
// ---- crate public ----
#[derive(Clone)]
pub struct VulkanSurface { pub(crate) inner: Arc<SurfaceInner> }   // opaque; Send + Sync

// rhi::Instance::create_surface (impl on VulkanInstance, body in this module)
fn create_surface<W>(&self, window: Arc<W>) -> Result<VulkanSurface, RhiError>
    where W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static;

// rhi::Device methods implemented here (LAYER-003 §8)
fn create_swapchain(&self, surface: &VulkanSurface, desc: &SwapchainDesc) -> Result<SwapchainHandle, RhiError>;
fn resize_swapchain(&self, sc: SwapchainHandle, width: u32, height: u32) -> Result<(), RhiError>;
fn swapchain_format(&self, sc: SwapchainHandle) -> Result<Format, RhiError>;
fn destroy_swapchain(&self, sc: SwapchainHandle);
fn acquire(&mut self, sc: SwapchainHandle) -> Result<Acquire, RhiError>;
fn present(&mut self, sc: SwapchainHandle, image: SwapchainImage) -> Result<PresentStatus, RhiError>;
```

```rust
// ---- internal ----
pub(crate) struct SurfaceInner {
    pub raw: vk::SurfaceKHR,
    _window: Arc<dyn Send + Sync>,          // declared before `instance`; dropped after the surface is destroyed
    pub instance: Arc<InstanceShared>,      // declared last
}
impl Drop for SurfaceInner { fn drop(&mut self) { /* instance.surface_fn.destroy_surface(raw, None) */ } }

pub(crate) struct SwapchainConfig {
    pub surface_format: vk::SurfaceFormatKHR, pub format: rhi::Format,
    pub present_mode: vk::PresentModeKHR, pub extent: vk::Extent2D, pub image_count: u32,
    pub pre_transform: vk::SurfaceTransformFlagsKHR, pub composite_alpha: vk::CompositeAlphaFlagsKHR,
}
// pure selection (no Vulkan calls; vk structs are plain data and constructible in tests)
pub(crate) fn choose_surface_format(available: &[vk::SurfaceFormatKHR], preferred: Format) -> Option<(vk::SurfaceFormatKHR, Format)>;
pub(crate) fn choose_present_mode(available: &[vk::PresentModeKHR], vsync: bool) -> vk::PresentModeKHR;
pub(crate) fn choose_extent(caps: &vk::SurfaceCapabilitiesKHR, requested: (u32, u32)) -> vk::Extent2D;
pub(crate) fn choose_image_count(caps: &vk::SurfaceCapabilitiesKHR) -> u32;
pub(crate) fn choose_composite_alpha(caps: &vk::SurfaceCapabilitiesKHR) -> Option<vk::CompositeAlphaFlagsKHR>;
pub(crate) fn choose_config(caps: &vk::SurfaceCapabilitiesKHR, formats: &[vk::SurfaceFormatKHR],
    modes: &[vk::PresentModeKHR], requested: (u32, u32), vsync: bool, preferred: Format) -> Result<SwapchainConfig, RhiError>;
pub(crate) fn vk_to_rhi_format(f: vk::Format) -> Option<Format>;
pub(crate) fn rhi_to_vk_format(f: Format) -> vk::Format;

pub(crate) enum AcquireOutcome { Image { index: u32, suboptimal: bool }, OutOfDate }
pub(crate) fn classify_acquire(r: VkResult<(u32, bool)>) -> Result<AcquireOutcome, RhiError>;     // pure
pub(crate) fn classify_present(r: VkResult<bool>) -> Result<PresentStatus, RhiError>;               // pure

pub(crate) struct VkSwapchain {
    pub raw: vk::SwapchainKHR,
    pub surface: VulkanSurface,                    // keeps the VkSurfaceKHR and window alive
    pub config: SwapchainConfig,
    pub images: Vec<vk::Image>,                    // owned by the swapchain, never destroyed individually
    pub views: Vec<vk::ImageView>,                 // one per image
    pub textures: Vec<TextureHandle>,              // entries in pools.textures, index-aligned with images
    pub render_done: Vec<vk::Semaphore>,           // binary, one per image
    pub vsync: bool,
    pub desired: (u32, u32),                       // last requested size
    pub minimized: bool,
    pub broken: bool,                              // set if recreation failed after the old swapchain was retired
}
pub(crate) struct VkTexture { pub image: vk::Image, pub view: vk::ImageView, pub extent: (u32, u32), pub format: Format, pub owner: TextureOwner }
pub(crate) enum TextureOwner { Swapchain /* M1 */, Allocation /* M2, MODULE-006 */ }
```

## 6. Data structures

### 6.1 Selection rules

| Item | Rule (in order; first match wins) |
|---|---|
| Surface format | Consider only formats with `color_space == SRGB_NONLINEAR_KHR` that map to an `rhi::Format` (`B8G8R8A8_SRGB`, `B8G8R8A8_UNORM`, `R8G8B8A8_SRGB`, `R8G8B8A8_UNORM`). Pick: (1) exact `SwapchainDesc.preferred_format`; (2) `B8G8R8A8_SRGB`; (3) `R8G8B8A8_SRGB`; (4) first remaining candidate in driver order (UNORM; `info!` that the format is not sRGB). No candidate -> `Unsupported{what:"surface format"}` (LAYER-004 says "else first available"; an unmappable format cannot be represented in `rhi::Format`) |
| Present mode | `vsync = true` -> `FIFO` (guaranteed available). `vsync = false` -> `MAILBOX` if listed, else `IMMEDIATE` if listed, else `FIFO` |
| Image count | `min_image_count + 1`; if `max_image_count != 0` then `min(count, max_image_count)` |
| Extent | If `current_extent.width != u32::MAX` use `current_extent`; else clamp requested width to `[min_image_extent.width, max_image_extent.width]` and height likewise. A requested size with a zero dimension never reaches this function (minimized, below) |
| Zero extent | Requested 0 in either dimension, or `current_extent` with a zero dimension: swapchain is marked `minimized`, no (re)creation, `acquire` returns `Acquire::Minimized` |
| Usage | `COLOR_ATTACHMENT`; if not in `supported_usage_flags` -> `Unsupported` |
| Pre-transform | `caps.current_transform` |
| Composite alpha | First supported of `OPAQUE`, `INHERIT`, `PRE_MULTIPLIED`, `POST_MULTIPLIED`; none -> `Unsupported` |
| Other | `image_array_layers = 1`, `image_sharing_mode = EXCLUSIVE`, `clipped = true` |

On recreation the previous `Format` is passed as `preferred`, so the format does not change unless it disappeared (then `warn!`: the app's pipelines target the old format and must be rebuilt; `swapchain_format()` reports the new one).

### 6.2 Semaphore ownership

| Semaphore | Count | Owner | Rationale (RESEARCH-001) |
|---|---|---|---|
| acquire (binary) | one per frame slot | `CommandManager` (MODULE-004) | image index unknown until acquire returns |
| render-done (binary) | one per swapchain image | `VkSwapchain.render_done` | reuse is safe only when the image is re-acquired; recreated with the swapchain |

## 7. Ownership and lifetime

| Object | Owner | Created | Destroyed |
|---|---|---|---|
| `VkSurfaceKHR` | `SurfaceInner` (shared by `VulkanSurface` clones and by every swapchain on it) | `create_surface` | last `Arc<SurfaceInner>` drop: `destroy_surface`, then `_window` drops, then the instance `Arc` |
| `Arc<W>` window | `SurfaceInner._window` | `create_surface` | after the surface (declaration order) |
| `VkSwapchainKHR` | `VkSwapchain` in `pools.swapchains` | `create_swapchain` / recreation | `destroy_swapchain`, recreation, `VulkanDevice::shutdown` step 4 |
| swapchain `VkImage`s | the swapchain | implicit | implicit with the swapchain |
| `VkImageView`s, render-done semaphores | `VkSwapchain` | after swapchain creation | before `destroy_swapchain_khr` |
| `TextureHandle`s of images | `pools.textures` entries tagged `TextureOwner::Swapchain` | after views | removed (generation bumped, old handles stale) before views are destroyed |

Per-swapchain create order: swapchain, `get_swapchain_images`, image views, render-done semaphores, texture-pool entries. Destroy order: texture-pool entries, render-done semaphores, image views, swapchain; then the swapchain's `Surface` reference drops. `destroy_swapchain` in M1 calls `device_wait_idle` first (`// shortcut:` presentation completion is unobservable in core Vulkan; replace with deferred destruction keyed on timeline value plus `VK_KHR_swapchain_maintenance1` or the M2 plan).

Unsafe invariants:

| Block | Invariant to justify |
|---|---|
| `ash_window::create_surface` | `window_handle`/`display_handle` obtained from `window.window_handle()` / `display_handle()` on the live `Arc<W>` and used only within this call; the returned surface is stored together with that `Arc` and an `Arc<InstanceShared>` created with the matching display extension (verified 2026-11-02) |
| `destroy_surface` | all swapchains created from the surface are destroyed (each holds an `Arc<SurfaceInner>`, so `Drop` cannot run earlier); the instance is alive (Arc); called once |
| `create_swapchain_khr` | `surface` belongs to the same instance as `DeviceShared` (`Arc::ptr_eq`), the device's queue family can present to it (checked via `get_physical_device_surface_support`), `old_swapchain` is null or a swapchain of this device and surface; create-info pointers (queue family indices unused for EXCLUSIVE) live through the call |
| `get_swapchain_images` / views | image handles are only used while their swapchain lives; views are destroyed before the swapchain |
| `acquire_next_image` | the semaphore passed is unsignaled with no pending wait (guaranteed by the slot's timeline wait in `begin_frame`); externally synchronized: `&mut self` plus the swapchain pool lock |
| `queue_present` | called with the queue mutex held; the wait semaphore was signalled by a submit that is already queued; `image_index` came from the acquire of this swapchain in this frame |
| `device_wait_idle` before destroying/recreating | queue mutex held (all queues externally synchronized) |

## 8. Execution flow

### 8.1 `create_surface(window)`

1. `instance.surface_fn` is `None` (instance created headless) -> `Unsupported{what:"instance created without a display handle"}`.
2. `window.display_handle()` / `window.window_handle()`; `HandleError` -> `InvalidDescriptor`/`Unsupported`.
3. `ash_window::create_surface(&entry, &raw_instance, display.as_raw(), window.as_raw(), None)`; map `vk::Result`.
4. Wrap: `SurfaceInner { raw, _window: window as Arc<dyn Send + Sync>, instance: shared.clone() }`.
5. Debug-name the surface if debug_utils is enabled.

### 8.2 `create_swapchain(surface, desc)`

1. `desc.width == 0 || desc.height == 0` -> `InvalidDescriptor` (create swapchains with a real size; zero only arises through later resize).
2. `Arc::ptr_eq(&surface.inner.instance, &device.instance)` else `InvalidDescriptor`.
3. `get_physical_device_surface_support(pd, queue_family, surface)` false -> `Unsupported{what:"queue family cannot present to this surface"}`.
4. Query `get_physical_device_surface_capabilities`, `_formats`, `_present_modes` (each `SURFACE_LOST` -> `SurfaceLost`).
5. `choose_config`.
6. `create_swapchain_khr` (`min_image_count = config.image_count`, `image_format/color_space` from config, `image_extent`, `image_array_layers 1`, `image_usage COLOR_ATTACHMENT`, `EXCLUSIVE`, `pre_transform`, `composite_alpha`, `present_mode`, `clipped true`, `old_swapchain null`).
7. `get_swapchain_images`; for each image create a 2D view (`format`, identity swizzle, `COLOR` aspect, 1 mip, 1 layer) and a binary render-done semaphore; insert a `VkTexture` into `pools.textures`; debug-name all.
8. Insert `VkSwapchain` into `pools.swapchains`; return the handle. `info!` format, mode, count, extent.

### 8.3 `resize_swapchain(sc, w, h)` and recreation

1. Lookup (`StaleHandle` on mismatch). Store `desired = (w, h)`.
2. `w == 0 || h == 0` -> `minimized = true`; return `Ok(())` (no Vulkan calls).
3. Query caps/formats/modes; `choose_config(..., preferred = current format)`; `config.extent` with a zero dimension -> `minimized = true`; return.
4. Lock queue; `device_wait_idle` (`// shortcut:` M1 simplicity; the frame loop itself never waits idle, NFR-PERF-015).
5. `create_swapchain_khr` with `old_swapchain = old.raw`. On error return the mapped error; the old swapchain is untouched and still valid (creation failed before retirement).
6. Remove the old textures from `pools.textures` (stale), destroy old render-done semaphores and views, `destroy_swapchain_khr(old)`.
7. Build images, views, semaphores, textures for the new swapchain exactly as 8.2 step 7. A failure here marks the entry `broken` (the old swapchain is already retired): return the error; later calls on it return `Backend{op:"swapchain broken"}`; the app treats this as fatal in M1.
8. `minimized = false`; update `config`; `info!`.

Triggers: the app calls `resize_swapchain` after `Event::Resized`, after `Acquire::OutOfDate`, and after `PresentStatus::Recreate`. The module itself never recreates implicitly.

### 8.4 `acquire(sc)`

1. Frame must be open (`begin_frame` done) and no acquire yet in this frame, else `InvalidDescriptor`.
2. Lookup; `broken` -> error; `minimized` -> return `Acquire::Minimized` (no semaphore used).
3. `swapchain_fn.acquire_next_image(sc.raw, u64::MAX, acquire_sem[slot], vk::Fence::null())` through `classify_acquire`:
   - `Ok((i, false))` -> `Acquire::Image`.
   - `Ok((i, true))` (SUBOPTIMAL) -> still `Acquire::Image`: the semaphore is signalled and the image must be used; the following present reports `Recreate`.
   - `Err(ERROR_OUT_OF_DATE_KHR)` -> `Acquire::OutOfDate`; the acquire semaphore was not signalled (verify: semaphore state after a failed acquire) and must not be waited on; the frame is abandoned without submit.
   - `Err(ERROR_SURFACE_LOST_KHR)` -> `Err(SurfaceLost)`; `ERROR_DEVICE_LOST` -> `DeviceLost`; others -> `Backend`.
4. Build `SwapchainImage { swapchain, texture: textures[i], index: i, extent, format }`; tell `CommandManager` that this slot's acquire semaphore is pending (submit must wait on it).

### 8.5 `present(sc, image)`

1. `image.swapchain == sc` else `InvalidDescriptor`; frame must have been submitted with this image.
2. Lock queue; `swapchain_fn.queue_present(queue, &PresentInfoKHR { wait_semaphores: [render_done[image.index]], swapchains: [raw], image_indices: [index] })` through `classify_present`:
   - `Ok(false)` -> `PresentStatus::Ok`; `Ok(true)` -> `Recreate`; `Err(OUT_OF_DATE)` -> `Recreate` (the render-done wait may not have been consumed; recreation replaces the semaphores after `device_wait_idle`, which resets this problem); `Err(SURFACE_LOST)` -> `SurfaceLost`; `Err(DEVICE_LOST)` -> `DeviceLost`.

### 8.6 Shutdown

`VulkanDevice::shutdown` step 4: for each swapchain still in the pool: `warn!` leak, then destroy as in section 7 (the device is already idle).

## 9. Threading model

M1: all calls from the window-owning (main) thread. `create_surface` is `&self` on `Send + Sync` types. `create_swapchain`, `resize_swapchain`, `swapchain_format`, `destroy_swapchain` take `&self` and lock `pools.swapchains` (Mutex) and, for presentation-affecting steps, the queue mutex. `acquire` and `present` are `&mut self` (single owner on the render thread). Vulkan rules: a swapchain is externally synchronized for acquire/present/destroy/recreate: the pool Mutex plus single-owner `&mut self` guarantee this; present uses the queue mutex. If rendering moves to its own thread (M5), resize events travel over a channel to that thread; `resize_swapchain` must then run on it.

## 10. Error handling

| Source | Result |
|---|---|
| `ERROR_OUT_OF_DATE_KHR` at acquire | `Acquire::OutOfDate` (status) |
| `SUBOPTIMAL_KHR` at acquire | proceed; present reports `Recreate` |
| `ERROR_OUT_OF_DATE_KHR` / `SUBOPTIMAL_KHR` at present | `PresentStatus::Recreate` (status) |
| `ERROR_SURFACE_LOST_KHR` anywhere | `RhiError::SurfaceLost` (fatal in M1: the app stops the loop and shuts down cleanly; no recreation) |
| `ERROR_DEVICE_LOST` | `DeviceLost` |
| `ERROR_OUT_OF_HOST_MEMORY` / `ERROR_OUT_OF_DEVICE_MEMORY` | `OutOfHostMemory` / `OutOfDeviceMemory` |
| `ERROR_NATIVE_WINDOW_IN_USE_KHR` (verified 2026-11-02) at creation | `Backend{op:"vkCreateSwapchainKHR", code}` |
| zero-size create, foreign surface, mismatched image/swapchain | `InvalidDescriptor` |
| queue family cannot present, no usable format/alpha/usage | `Unsupported` |
| stale `SwapchainHandle` | `StaleHandle{kind:"swapchain"}` |
| `broken` swapchain | `Backend{op:"swapchain broken", code:0}` |

## 11. Performance requirements

- `acquire` and `present` do no heap allocation in steady state (fixed-size arrays for semaphores/swapchains; `ash` builders borrow stack slices) and hold the queue mutex only around `vkQueuePresentKHR`.
- The frame loop never calls `device_wait_idle`; only resize and destroy do (NFR-PERF-015).
- Recreation count and time are logged at `info` (input for later frame-pacing work; no numeric target, NFR-PERF-012).

## 12. Testing requirements

Unit tests (no GPU; `vk::SurfaceCapabilitiesKHR` etc. built by hand):

| Test | Checks |
|---|---|
| `format_prefers_exact_preferred` | preferred `Rgba8Unorm` honoured when listed |
| `format_default_bgra_srgb` | BGRA sRGB beats RGBA sRGB beats UNORM |
| `format_ignores_other_color_spaces` | HDR10 / extended color spaces skipped |
| `format_unmappable_only_is_unsupported` | only `A2B10G10R10` -> `None`/`Unsupported` |
| `present_mode_vsync_is_fifo` | even when MAILBOX listed |
| `present_mode_novsync_order` | MAILBOX, then IMMEDIATE, then FIFO |
| `image_count_min_plus_one` | 2 -> 3 |
| `image_count_clamped_to_max` | min 3, max 3 -> 3; max 0 means unlimited |
| `extent_uses_current_when_defined` | requested size ignored |
| `extent_clamps_when_special_value` | `u32::MAX` current extent; below min and above max |
| `composite_alpha_order` | OPAQUE then INHERIT ...; none -> `None` |
| `usage_without_color_attachment_unsupported` | `choose_config` error |
| `classify_acquire_table` | OK / SUBOPTIMAL / OUT_OF_DATE / SURFACE_LOST / DEVICE_LOST / other |
| `classify_present_table` | same for present |
| `format_mapping_roundtrip` | `rhi_to_vk_format(vk_to_rhi_format(f)) == f` for all four |
| `surface_inner_drop_order` | `cfg(test)` surface with a window type that records its drop; the window drops after the native surface destroy hook ran and before the instance `Arc` is released |
| `render_done_count_matches_images` | helper that sizes the semaphore vector |

Integration tests (in-crate `#[cfg(test)] mod gpu_tests`, so they can use a test-only headless surface; lavapipe on CI, validation `On`, sync validation on):

| Test | Checks |
|---|---|
| `headless_surface_swapchain_create` | `VK_EXT_headless_surface` is exposed by the Mesa loader stack on the dev machine (RADV); verify on the CI lavapipe; if not, the test falls back to xvfb + X11 surface (skipped without a display). Swapchain created, `swapchain_format` returns a mapped format |
| `acquire_present_300_frames` | empty-render frames (barrier Undefined->ColorAttachment->Present, clear): zero validation errors or warnings (`validation_counts()`) |
| `resize_recreates_and_stays_valid` | `resize_swapchain` to a new size 10 times between frames; old `TextureHandle`s return `StaleHandle`; zero validation messages |
| `resize_to_zero_is_minimized` | `Acquire::Minimized`, no crash; resize back restores `Acquire::Image` |
| `destroy_swapchain_then_surface_then_device` and permutations | no validation "object not destroyed" messages |
| `surface_outlives_window_arc` | drop the app's window `Arc` first; surface still destroys cleanly (window kept by `SurfaceInner`) |
| `foreign_instance_surface_rejected` | surface from instance A with device from instance B -> `InvalidDescriptor` |
| `surface_lost_is_fatal` | verify: injecting `SURFACE_LOST` needs a mock; implemented as a `classify_*` unit test only |
| `frames_in_flight_1_and_3` | the 300-frame loop passes for N = 1, 2, 3 |

The triangle example (M1 exit) additionally runs on a real window: 300 frames, manual resize and minimize/restore on X11 and Wayland.

## 13. Acceptance criteria

- [ ] `create_surface` takes `Arc<W>`; the window outlives the `VkSurfaceKHR` regardless of drop order in the app.
- [ ] Format, present mode, image count and extent follow section 6.1 exactly (unit-tested).
- [ ] Swapchain images are ordinary `TextureHandle`s; handles from before a recreation are stale afterwards.
- [ ] Acquire semaphores are per frame slot, render-done semaphores per swapchain image and recreated with the swapchain; no semaphore is signalled while a previous use is pending (sync validation clean).
- [ ] Out-of-date at acquire abandons the frame without submitting and without waiting on the acquire semaphore.
- [ ] Suboptimal at acquire still renders the frame; present then returns `Recreate`.
- [ ] Resize, minimize (0x0) and restore work with zero validation messages.
- [ ] `ERROR_SURFACE_LOST_KHR` yields `RhiError::SurfaceLost`.
- [ ] Recreation uses `old_swapchain` and carries a `// shortcut:` comment for the `device_wait_idle`.
- [ ] No `ash` in the dependency tree of platform; every `unsafe` block has `// SAFETY:` covering the section 7 invariants; clippy clean.

## 14. Traceability (FR/NFR IDs)

| Requirement | Satisfied by |
|---|---|
| FR-VK-004 | sections 8.1, 8.2 |
| FR-VK-005 | section 8.3 and tests `resize_recreates_and_stays_valid`, `acquire_present_300_frames` |
| FR-RHI-016 | trait methods in section 5 |
| FR-PLAT-003 | resize and minimize handling, validation-clean tests |
| FR-PLAT-004 | `create_surface` takes generic raw handles; platform never sees `ash` |
| NFR-REL-004 | shutdown and drop-order tests |
| NFR-REL-002 | validation `On` in tests |
| RESEARCH-001 | per-slot acquire, per-image render-done semaphores |

## 15. Open questions

1. verify: `VK_EXT_headless_surface` in lavapipe; otherwise xvfb job for swapchain tests (CI cost).
2. verify: spec-exact semaphore state after `ERROR_OUT_OF_DATE_KHR` from acquire (assumed unsignalled).
3. Replace the `device_wait_idle` shortcut with deferred destruction (M2) and evaluate `VK_KHR_swapchain_maintenance1` present fences.
4. Wayland: `SUBOPTIMAL`/`OUT_OF_DATE` behavior on resize varies by compositor; a loop that recreates on every `Recreate` may spin if a driver reports suboptimal permanently. Add a guard (recreate at most once per frame) and test on X11 and Wayland.
5. HDR / wide-gamut formats are excluded from M1 selection; revisit with the renderer (M4 tone mapping).
