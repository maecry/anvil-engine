# MODULE-001: Vulkan Instance

| Field | Value |
|---|---|
| Status | Accepted |
| Crate | rhi-vulkan |
| Layer | LAYER-004 |
| Milestone | M1 |
| Traces to | FR-VK-001, FR-VK-013, FR-RHI-017, NFR-REL-002, NFR-OBS-002, NFR-QUAL-004, NFR-ARCH-003, NFR-TEST-002; MASTER-000 §8.4, §14 M1, §16; ADR-0001, ADR-0004, ADR-0008 |
| Last updated | 2026-11-02 |

## 1. Purpose

Load the Vulkan loader, create a Vulkan 1.3 `VkInstance` with the right layers and extensions, route validation messages into `tracing`, and expose the result as the RHI `Instance`. It is the root of the native object graph: the instance is destroyed last, and only after every `Device`, `Surface` and swapchain derived from it has been dropped (lead decision D1).

## 2. Responsibilities

- Load `libvulkan` (`ash::Entry`) and check the instance version is >= 1.3.
- Resolve `ValidationMode` and `sync_validation` into a concrete layer/extension plan.
- Create the instance, the optional `VK_EXT_debug_utils` messenger, and the surface-extension loaders.
- Route messages: ERROR -> `error!`, WARNING -> `warn!`, INFO -> `debug!`, VERBOSE -> `trace!`, target `rhi_vulkan::validation`; count messages per severity.
- Provide `Arc<InstanceShared>` to Device and Surface so they keep the instance alive.
- Implement `rhi::Instance::create_surface` (delegates to MODULE-003) and `create_device` (delegates to MODULE-002).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Physical device selection, logical device | MODULE-002 |
| Surface and swapchain objects | MODULE-003 |
| Debug names of device-level objects, command labels | MODULE-004 |
| Window creation | platform (LAYER-002) |

## 4. Dependencies

Allowed: foundation, rhi, `ash`, `ash-window`, `raw-window-handle`, `thiserror`, `tracing`. Forbidden: platform, `winit`, any layer 3+ crate. Expected versions (confirmed 2026-11-02; see LAYER-004 verification log): ash 0.38, ash-window 0.13, raw-window-handle 0.6 (D7).

## 5. Public API

```rust
// ---- crate public surface (everything else is pub(crate)) ----
pub struct VulkanBackend;
impl rhi::Backend for VulkanBackend {
    type Instance = VulkanInstance;
    type Surface  = VulkanSurface;   // MODULE-003
    type Device   = VulkanDevice;    // MODULE-002
    fn create_instance(desc: &rhi::InstanceDesc) -> Result<VulkanInstance, rhi::RhiError>;
}

#[derive(Clone)]
pub struct VulkanInstance { shared: Arc<InstanceShared> }
impl rhi::Instance for VulkanInstance {
    type Surface = VulkanSurface; type Device = VulkanDevice;
    fn create_surface<W>(&self, window: Arc<W>) -> Result<VulkanSurface, RhiError>
        where W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static;
    fn create_device(&self, desc: &DeviceDesc, compatible_surface: Option<&VulkanSurface>) -> Result<VulkanDevice, RhiError>;
}
impl VulkanInstance {
    /// Plain data, no vk types: used by the validation gate tests.
    pub fn validation_counts(&self) -> ValidationCounts;
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidationCounts { pub errors: u64, pub warnings: u64, pub infos: u64 }
```

`rhi::InstanceDesc` (LAYER-003 §8): `app_name: String`, `validation: ValidationMode`, `sync_validation: bool`, `display: Option<RawDisplayHandle>`. `InstanceDesc::default()` reads `ANVIL_SYNC_VALIDATION` (value `1` -> true).

```rust
// ---- internal ----
pub(crate) struct InstanceShared {
    pub entry: ash::Entry,
    pub raw: ash::Instance,
    pub surface_fn: Option<ash::khr::surface::Instance>,        // Some iff display was given
    pub debug_utils: Option<ash::ext::debug_utils::Instance>,   // Some iff VK_EXT_debug_utils enabled
    messenger: Option<DebugMessenger>,
    pub caps: InstanceCaps,
}
pub(crate) struct InstanceCaps {
    pub api_version: u32, pub validation_active: bool, pub sync_validation_active: bool,
    pub debug_utils: bool, pub surface_extensions: Vec<&'static CStr>,
}
struct DebugMessenger { raw: vk::DebugUtilsMessengerEXT, counters: *const Counters /* Arc::into_raw */ }
pub(crate) struct Counters { errors: AtomicU64, warnings: AtomicU64, infos: AtomicU64 }

pub(crate) struct ValidationPlan { pub enable_layer: bool, pub warn_missing: bool, pub sync: bool }
pub(crate) fn resolve_validation(mode: ValidationMode, debug_build: bool, layer_present: bool, sync_requested: bool)
    -> Result<ValidationPlan, RhiError>;                         // pure, unit-tested
pub(crate) fn check_instance_version(v: Option<u32>) -> Result<u32, RhiError>;   // None = Vulkan 1.0 loader
pub(crate) fn plan_extensions(available: &[&CStr], required: &[&CStr], optional: &[&CStr])
    -> Result<Vec<*const c_char> /* names from the input slices */, RhiError>;     // pure
pub(crate) fn severity_to_level(s: vk::DebugUtilsMessageSeverityFlagsEXT) -> tracing::Level;  // pure
unsafe extern "system" fn debug_callback(
    sev: vk::DebugUtilsMessageSeverityFlagsEXT, ty: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>, user: *mut c_void) -> vk::Bool32;
impl Drop for InstanceShared { fn drop(&mut self) { /* see section 7 */ } }
```

## 6. Data structures

`resolve_validation` truth table (`debug_build = cfg!(debug_assertions)`):

| mode | debug_build | layer_present | result |
|---|---|---|---|
| Off | any | any | `enable_layer=false`, sync=false |
| On | any | true | `enable_layer=true`, sync=`sync_requested` |
| On | any | false | `Err(Unsupported{what:"VK_LAYER_KHRONOS_validation"})` |
| Auto | true | true | enable, sync=`sync_requested` |
| Auto | true | false | `enable_layer=false`, `warn_missing=true` (log `warn!`, continue) |
| Auto | false | any | `enable_layer=false` |

Extension plan:

| Extension | Class | Condition |
|---|---|---|
| names from `ash_window::enumerate_required_extensions(display)` | required | `display` is `Some` |
| `VK_EXT_debug_utils` (`ash::ext::debug_utils::NAME`) | optional | always requested; labels and object names need it even with validation Off |
| `VK_EXT_validation_features` | optional | `plan.sync` is true. Missing: `warn!`, sync validation is skipped (mode `On` does not make this fatal). verify: it is provided by the validation layer, so it is enumerated with `enumerate_instance_extension_properties(Some(layer_name))`; verify whether `VK_EXT_layer_settings` supersedes it |

Layers: `VK_LAYER_KHRONOS_validation` iff `plan.enable_layer`.

## 7. Ownership and lifetime

Objects and order.

| # | Create (in this order) | Owner | Destroy (reverse order) |
|---|---|---|---|
| 1 | `ash::Entry` (`Entry::load()`, dlopen) | `InstanceShared.entry` | dropped last, after `destroy_instance` |
| 2 | counters `Arc<Counters>` | leaked to raw pointer in `DebugMessenger` | `Arc::from_raw` after the messenger is destroyed |
| 3 | `VkInstance` (with messenger create-info chained for create/destroy-time messages) | `InstanceShared.raw` | `destroy_instance(None)` in `Drop`, last Vulkan call |
| 4 | `khr::surface::Instance` loader | `surface_fn` | plain drop (function table) |
| 5 | `ext::debug_utils::Instance` loader and `VkDebugUtilsMessengerEXT` | `messenger` | `destroy_debug_utils_messenger` before `destroy_instance` |

`Drop for InstanceShared`: (1) destroy the messenger if any, (2) reclaim the counters `Arc`, (3) `destroy_instance`. It runs when the last `Arc<InstanceShared>` drops. Holders: `VulkanInstance`, every `SurfaceInner` (MODULE-003), every `DeviceShared` (MODULE-002). This realises "the Device must not outlive the Instance" without lifetimes in public types.

Unsafe invariants (each `// SAFETY:` must state these):

| Block | Invariant to justify |
|---|---|
| `Entry::load()` | Loading a dynamic library runs foreign initialisers; allowed once per process-wide use, `Entry` outlives all objects created from it (stored in `InstanceShared`, dropped after the instance) |
| `entry.create_instance` | Every pointer in `InstanceCreateInfo` (app info, layer/extension name arrays, pNext chain) points to data alive for the call; `CString`s are held in locals until the call returns |
| `ash_window::enumerate_required_extensions` | `RawDisplayHandle` came from a live `HasDisplayHandle` borrowed for the duration of the call (verified 2026-11-02) |
| debug callback `user_data` | The pointer comes from `Arc::into_raw(Arc<Counters>)`; the messenger is destroyed before `Arc::from_raw` reclaims it, so the callback never sees a dangling pointer |
| `debug_callback` body | Receives driver-owned `*const CallbackData`, valid only during the call; null-check it; read strings with `CStr::from_ptr` only when non-null; never unwind across FFI (`catch_unwind`); always return `vk::FALSE` |
| `destroy_*` in `Drop` | Exactly-once destruction (fields are `Option` and `take()`n); no child object (surface, device) exists any more because they each hold an `Arc` to this struct |

## 8. Execution flow

`create_instance(desc)`:

1. Validate `desc.app_name`: build `CString`; interior NUL -> `InvalidDescriptor`.
2. `ash::Entry::load()`; failure -> `Unsupported{what:"Vulkan loader (libvulkan)"}` (verified 2026-11-02).
3. `entry.try_enumerate_instance_version()` returns `VkResult<Option<u32>>` (verified 2026-11-02) -> `check_instance_version`: `None` or `< 1.3` -> `Unsupported{what:"Vulkan instance >= 1.3"}`.
4. `entry.enumerate_instance_layer_properties()`; detect `VK_LAYER_KHRONOS_validation`.
5. `resolve_validation(desc.validation, cfg!(debug_assertions), present, desc.sync_validation)`; log `warn!` when `warn_missing`.
6. `entry.enumerate_instance_extension_properties(None)` (plus `Some(layer)` for validation features when `plan.sync`); `plan_extensions` with required = surface extensions (only if `desc.display` is `Some`, from `ash_window::enumerate_required_extensions`), optional = debug_utils, validation_features.
7. Build `ApplicationInfo` (application_name = app_name, engine_name = "Anvil", `api_version = vk::API_VERSION_1_3`).
8. Build `InstanceCreateInfo` with enabled layers/extensions. Chain `DebugUtilsMessengerCreateInfoEXT` via `push_next` when debug_utils is enabled and the layer is active (captures instance create/destroy messages). When `plan.sync`, chain `ValidationFeaturesEXT` with `ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION` (verify exact names; verify whether `BEST_PRACTICES` is wanted: not in M1).
9. `entry.create_instance(&info, None)`; map `vk::Result` via the table in section 10.
10. Create loaders: `surface_fn` if `desc.display` is `Some`; `debug_utils` if enabled.
11. If the layer is active and debug_utils is enabled: `create_debug_utils_messenger` with severity = ERROR | WARNING | INFO | VERBOSE filtered at runtime by the tracing subscriber, types = GENERAL | VALIDATION | PERFORMANCE, `user_data` = `Arc::into_raw(counters)`.
12. `info!`: instance version, layers enabled, extensions enabled, sync validation state.
13. Return `VulkanInstance`.

`debug_callback`: map severity to `tracing` level; emit `target: "rhi_vulkan::validation"` with `message_id_name`, `message_id_number`, message, and up to 4 object names/handles; increment the matching counter; return `vk::FALSE`.

## 9. Threading model

`VulkanInstance` is `Send + Sync` (`ash::Instance` and `Entry` are). The debug callback can run on any thread the driver chooses, so it only touches atomics and `tracing`. Vulkan instance-level calls used here (`vkCreateInstance`, `vkDestroyInstance`) need no external synchronization beyond not racing destruction, which the `Arc` guarantees.

## 10. Error handling

| Condition | `RhiError` |
|---|---|
| Loader missing / version < 1.3 | `Unsupported` |
| Validation layer missing with mode `On` | `Unsupported{what:"VK_LAYER_KHRONOS_validation"}` |
| Required surface extension missing | `Unsupported{what:<ext name>}` |
| `ERROR_LAYER_NOT_PRESENT`, `ERROR_EXTENSION_NOT_PRESENT`, `ERROR_INCOMPATIBLE_DRIVER` | `Unsupported` |
| `ERROR_OUT_OF_HOST_MEMORY` | `OutOfHostMemory` |
| `ERROR_INITIALIZATION_FAILED` and anything else | `Backend{op:"vkCreateInstance", code}` |
| `app_name` contains NUL | `InvalidDescriptor` |

Validation messages are logged, never converted to `Err`. No `unwrap()` on Vulkan results outside tests.

## 11. Performance requirements

- Instance creation is a startup cost; log its duration at `info` (NFR-PERF-009 style timing, no target).
- The debug callback performs no allocation beyond what `tracing` does and takes no locks.
- No per-frame work in this module.

## 12. Testing requirements

Unit tests (no GPU, plain `cargo test -p rhi-vulkan`):

| Test | Checks |
|---|---|
| `validation_plan_matrix` | every row of the section 6 truth table, including sync flag propagation |
| `validation_auto_release_is_off` | `debug_build=false` never enables the layer in Auto |
| `instance_version_check` | `None`, 1.0, 1.2 rejected; 1.3 and 1.4 accepted |
| `plan_extensions_required_missing` | missing required -> `Unsupported` naming the extension |
| `plan_extensions_optional_missing` | missing optional is skipped, rest kept |
| `severity_mapping` | ERROR/WARNING/INFO/VERBOSE -> error/warn/debug/trace |
| `app_name_with_nul_rejected` | `InvalidDescriptor` |
| `vk_result_mapping_instance` | table in section 10 |

Integration tests (lavapipe on CI, `ANVIL_GPU_TESTS=1`, validation `On`, `sync_validation=true`):

| Test | Checks |
|---|---|
| `create_instance_headless` | `display: None`, instance created, `validation_counts()` all zero after drop |
| `create_instance_validation_on_missing_layer` | skipped when the layer is present; otherwise `Unsupported` (documented skip) |
| `validation_messages_are_counted` | deliberately create an invalid object in a `#[cfg(test)]` helper using the raw instance; the error counter becomes > 0 and a `tracing` event with target `rhi_vulkan::validation` is seen |
| `instance_outlives_children` | create instance, surface-less device, drop the `VulkanInstance` first, then use the device, then drop it: no validation messages |
| `create_instance_twice` | two instances in one process both work |

## 13. Acceptance criteria

- [ ] Instance is created with API 1.3; a loader < 1.3 yields `Unsupported`.
- [ ] `ValidationMode::{Auto,On,Off}` behave exactly per the section 6 table.
- [ ] Validation messages appear in `tracing` output with target `rhi_vulkan::validation` at the mapped levels.
- [ ] `ANVIL_SYNC_VALIDATION=1` enables synchronization validation; tests enable it explicitly.
- [ ] `VK_EXT_debug_utils` is enabled whenever available, independent of validation.
- [ ] Dropping the last `Arc<InstanceShared>` destroys messenger then instance; no validation "object not destroyed" messages in `instance_outlives_children`.
- [ ] No `ash::vk` type in the crate's public signatures; every `unsafe` block has `// SAFETY:` covering the section 7 invariants.
- [ ] `clippy -D warnings` clean; public items documented.

## 14. Traceability (FR/NFR IDs)

| Requirement | Satisfied by |
|---|---|
| FR-VK-001 | steps 2-11; validation in debug builds via `Auto` |
| FR-VK-013, NFR-ARCH-003 | only this crate uses `ash`; no vk types in public API |
| FR-RHI-017 (labels) | `VK_EXT_debug_utils` enabled and exposed in `InstanceCaps` for MODULE-004 |
| NFR-REL-002 | validation `On` in tests, counters gate |
| NFR-OBS-002 | messages routed to `tracing` |
| NFR-QUAL-004 | section 7 unsafe invariants |
| NFR-TEST-002 | integration tests above |
| ADR-0004 | instance version >= 1.3 |

## 15. Open questions

1. Resolved 2026-11-02: prefer `VK_EXT_layer_settings`, which the loader exposes on the dev machine and ash 0.38 provides as `ash::ext::layer_settings`. It is the current mechanism; `VK_EXT_validation_features` is its deprecated predecessor and stays as a fallback only. The setting key is `validate_sync` (layer `VK_LAYER_KHRONOS_validation` 1.4.363), verified 2026-11-02 from the layer manifest.
2. Resolved 2026-11-02: `ash_window::enumerate_required_extensions(RawDisplayHandle) -> VkResult<&'static [*const c_char]>`; `unsafe create_surface(&Entry, &Instance, RawDisplayHandle, RawWindowHandle, Option<&AllocationCallbacks>) -> VkResult<SurfaceKHR>`.
3. macOS (MoltenVK) needs `VK_KHR_portability_enumeration` and `VK_KHR_portability_subset` handling; out of scope for M1 (ADR-0004 does not guarantee macOS).
4. Should the validation message counters also be exposed via `rhi` for the profiler, or stay test-only inherent methods? Default: inherent, test-only.
