# MODULE-007: Pipeline Cache and Global Bindings

| Field | Value |
|---|---|
| Status | Accepted (M2 scope: global bindings + pipeline layout + depth + push constants; pipeline cache object deferred) |
| Crate | rhi-vulkan (pipeline objects, `VkPipelineCache`); key hashing helpers shared with shader (M5) |
| Layer | LAYER-008 (cache policy) / LAYER-007 (GPU object) |
| Milestone | M2 (global bindings, single layout, depth, push constants), M5 (dedupe, `VkPipelineCache` persistence, compute) |
| Traces to | FR-RHI-007, FR-RHI-008, FR-RHI-009, FR-VK-011, FR-SHD-004, FR-SHD-005, NFR-PERF-009, NFR-REL-004; MASTER-000 §8.4, §8.7, §14 M5; ADR-0005, ADR-0009, ADR-0010 |
| Last updated | 2026-11-02 |

## 1. Purpose

Own the pipeline objects and the **one global pipeline layout** (ADR-0010 §3) with its **global bindless descriptor set** (ADR-0010 §1). M1 builds a per-pipeline layout inside MODULE-004; M2 replaces it with a single device-wide layout and adds depth state. This module is the **owner of the global descriptor set, set layout, descriptor pool and pipeline layout** (chosen over MODULE-006 because the layout is a pipeline concern and MODULE-006 is only a writer of slots). A `VkPipelineCache` object, dedupe and compute pipelines remain M5 (below).

## 2. Responsibilities

M2:
- `GlobalBindings`: create/destroy the descriptor set layout, descriptor pool, the single descriptor set and the pipeline layout; expose `write_sampled_image` / `write_sampler` to MODULE-006; report capacities and high-water marks.
- Graphics pipeline creation against the global layout: no vertex input state, dynamic rendering formats, depth state per LAYER-003 §17.4.
- `bind_pipeline` binds set 0 on first use per command buffer; `push_constants` (encoder) uses the layout's range.
- Destroy pipelines through the deferred queue (`Doomed::Pipeline { pipeline }`; the layout is no longer per pipeline).

M5a (done): `VkPipelineCache` persistence with header validation (section 8a). M5 (not now): dedupe by descriptor hash, compute pipelines (M5b), parallel compile.

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Writing texture/sampler descriptors on create | MODULE-006 (calls `write_*` here) |
| Compiling GLSL/Slang | examples' `build.rs` in M2 (shaderc, ADR-0003); MODULE-008 later |
| Per-material layouts, bind groups, push descriptors | rejected (ADR-0010) |
| Hot reload | M6 |
| Pipeline cache file path policy | decided in M5a (section 8a): `DeviceDesc::pipeline_cache`, else `ANVIL_PIPELINE_CACHE`, else `target/anvil-pipeline-cache.bin` |
| `VK_EXT_graphics_pipeline_library` | out of scope (not in ADR-0004 baseline) |

**Pipeline cache object in M2:** the M1 empty `VkPipelineCache` in `DeviceShared.pipeline_cache` was passed to `create_graphics_pipelines`; nothing was loaded or saved. M5a makes it persistent (section 8a).

## 4. Dependencies

Allowed: foundation, rhi, MODULE-002 (`DeviceShared`, features, limits), MODULE-004 (`Doomed`), `ash`, `tracing`. Forbidden: layer 3+ crates. M5 additionally needs a stable hash (crate undecided).

## 5. Public API

No new `rhi` trait methods: `create_graphics_pipeline` / `destroy_pipeline` are unchanged (LAYER-003 §17.4 specifies the depth behavior; `CommandEncoder::push_constants` is in §17.3). Backend-internal:

```rust
pub(crate) struct GlobalBindings {
    pub set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,       // set 0 = set_layout, push range {ALL_GRAPHICS, 0, 128}
    pool: vk::DescriptorPool,                      // flags UPDATE_AFTER_BIND, one set, two descriptor types
    pub set: vk::DescriptorSet,
    pub sampled_image_capacity: u32,               // min(65536, device limit), binding 0
    pub sampler_capacity: u32,                     // min(1024, device limit), binding 1
    update: Mutex<()>,                             // external synchronization for vkUpdateDescriptorSets on `set`
    pub image_high_water: AtomicU32,
    pub sampler_high_water: AtomicU32,
}
impl GlobalBindings {
    /// Called by create_device right after vkCreateDevice, before the DeviceShared literal (MODULE-002 step 8a).
    /// Debug names ("global-bindings.*") are applied afterwards through DeviceShared::set_name.
    pub(crate) fn new(device: &ash::Device, limits: BindlessLimits) -> Result<Self, RhiError>;
    pub(crate) fn write_sampled_image(&self, device: &ash::Device, slot: u32, view: vk::ImageView);
    pub(crate) fn write_sampler(&self, device: &ash::Device, slot: u32, sampler: vk::Sampler);
    /// # Safety: device idle or no pending use; called once from DeviceShared::drop after the deferred drain.
    pub(crate) unsafe fn destroy(&self, device: &ash::Device);
}
pub(crate) struct BindlessLimits { pub sampled_images: u32, pub samplers: u32 }   // already clamped; from PhysicalDeviceFacts (MODULE-002)
pub(crate) fn bindless_capacity(device_limit: u32, wanted: u32) -> u32 { device_limit.min(wanted) }

// DeviceShared gains: pub bindings: GlobalBindings
// Doomed::Pipeline { pipeline: vk::Pipeline }   // layout field removed
// vk_pipeline in the pool: VkPipeline { pipeline, label, depth_format: Option<Format>, color_formats: Vec<Format> }  // for the draw-time format check
```

## 6. Data structures

### 6.1 Descriptor set layout (set 0)

| Binding | Type | Count | Stages | Binding flags |
|---|---|---|---|---|
| 0 | `SAMPLED_IMAGE` | `sampled_image_capacity` | `ALL_GRAPHICS` | `PARTIALLY_BOUND \| UPDATE_AFTER_BIND` |
| 1 | `SAMPLER` | `sampler_capacity` | `ALL_GRAPHICS` | `PARTIALLY_BOUND \| UPDATE_AFTER_BIND` |

`DescriptorSetLayoutCreateInfo { flags: UPDATE_AFTER_BIND_POOL, bindings, push_next(DescriptorSetLayoutBindingFlagsCreateInfo) }`. Fixed counts, **no** `VARIABLE_DESCRIPTOR_COUNT` (decision: capacity is constant per device, so the extra feature `descriptorBindingVariableDescriptorCount` is not needed and not enabled). `DescriptorPoolCreateInfo { flags: UPDATE_AFTER_BIND, max_sets 1, pool_sizes [{SAMPLED_IMAGE, cap_i}, {SAMPLER, cap_s}] }`; one `allocate_descriptor_sets`. The pool is never reset; the set lives for the device's life (`PARTIALLY_BOUND` means unwritten slots are legal as long as shaders never access them).

### 6.2 Pipeline layout

`PipelineLayoutCreateInfo { set_layouts: [set_layout], push_constant_ranges: [PushConstantRange { stage_flags: ALL_GRAPHICS, offset: 0, size: 128 }] }`. 128 B is the Vulkan-guaranteed minimum `maxPushConstantsSize`, so no device check is needed. All graphics pipelines use this layout; shader push-constant blocks may declare any prefix of the 128 bytes, in all stages that use them with identical layout.

### 6.3 Capacities and limits

Per ADR-0010 §1: `sampled_image_capacity = min(65536, L_img)`, `sampler_capacity = min(1024, L_smp)` where
`L_img = min(maxPerStageDescriptorUpdateAfterBindSampledImages, maxDescriptorSetUpdateAfterBindSampledImages)` and
`L_smp = min(maxPerStageDescriptorUpdateAfterBindSamplers, maxDescriptorSetUpdateAfterBindSamplers)`
(`PhysicalDeviceVulkan12Properties` fields `max_per_stage_descriptor_update_after_bind_sampled_images`, `max_descriptor_set_update_after_bind_sampled_images`, `..._samplers`: names confirmed in ash 0.38). Decision (not in ADR-0010): a device with `L_img < 1024` or `L_smp < 64` is rejected (`Rejection::BindlessLimits`), because the quarantine scheme needs headroom beyond a handful of live textures; the per-stage limit also bounds the total across stages when only the fragment/vertex stages use it, which is the case here.

### 6.4 Pipeline state (graphics)

- Shader stages vertex + fragment, entry names from `ShaderStageDesc`.
- `PipelineVertexInputStateCreateInfo::default()` (empty); vertices come from BDA.
- Input assembly from `topology` (existing M1 mapping), viewport/scissor dynamic, rasterization from `cull`/`front_face`, one sample.
- Color blend from `blend` (M1).
- Depth/stencil: `depth_format: None` -> `depth_test_enable false`, `depth_write_enable false`. `Some(D32Sfloat)` -> `depth_test_enable true`, `depth_write_enable true`, `depth_compare_op LESS`, no bounds test, no stencil; `PipelineRenderingCreateInfo.depth_attachment_format = D32_SFLOAT`. Any other depth format -> `InvalidDescriptor`.
- Layout: `GlobalBindings.pipeline_layout`.

## 7. Ownership and lifetime

`GlobalBindings` is owned by `DeviceShared` (field `bindings`, created in `create_device`, destroyed in `DeviceShared::drop` after the deferred drain and before the allocator is taken and the device destroyed; M1's drop order for the timeline/cache is otherwise unchanged). Pipelines are pool entries; `destroy_pipeline` -> `Doomed::Pipeline{pipeline}` deferred as in M1. No pipeline owns a layout, so destroying a pipeline never touches `pipeline_layout`. Create order in `create_device`: `vkCreateDevice` -> `GlobalBindings::new` (if it fails, destroy the device) -> `DeviceShared` literal (owns `bindings`; later failures unwind through its `Drop`) -> allocator.

Unsafe invariants: `create_descriptor_set_layout`/`create_pipeline_layout`/`create_descriptor_pool`/`allocate_descriptor_sets` info structs and slices outlive the call; pool has capacity for exactly one set; `update_descriptor_sets` holds `update`, writes `slot < capacity`, and the written view/sampler is alive; `destroy` runs once with the device idle.

## 8. Execution flow

Startup (`create_device`, MODULE-002 step 10a): `BindlessLimits` from the chosen device's facts -> `GlobalBindings::new`: layout -> pipeline layout -> pool -> allocate set -> names (`set_name` on each) -> `info!(sampled_image_capacity, sampler_capacity, push_constant_bytes = 128, "global bindings created")`.

`create_graphics_pipeline` (extends M1): resolve shader modules, build state per 6.4 against `bindings.pipeline_layout`, `create_graphics_pipelines(pipeline_cache, ...)`, insert, name, `debug!(handle, label, color_formats, depth_format, topology)`. A failed create does not leak the shared layout (nothing per-pipeline to unwind).

Encoder: first `bind_pipeline` in a command buffer also records `cmd_bind_descriptor_sets(cb, GRAPHICS, pipeline_layout, 0, [set], [])` once (`EncoderState.global_bound`); later pipelines share the layout so the binding stays valid. `push_constants(offset, data)`: validate `offset % 4 == 0`, `data.len() % 4 == 0`, `offset + len <= 128` (else recorded `InvalidDescriptor`); `cmd_push_constants(cb, pipeline_layout, ALL_GRAPHICS, offset, data)`; allowed before any pipeline is bound. `draw`/`draw_indexed` require a bound pipeline and (for indexed) a bound index buffer, and compare `depth_format`/`color_formats` with the open rendering (recorded `InvalidDescriptor` on mismatch).

Bindless writes (called by MODULE-006): lock `update`; one `WriteDescriptorSet` (`dst_set = set`, `dst_binding` 0/1, `dst_array_element = slot`, `descriptor_count 1`); `update_descriptor_sets`; update high-water.

## 8a. Persistent pipeline cache (M5a, FR-VK-011, ADR-0013 §4)

`DeviceShared.pipeline_cache` is the one `VkPipelineCache` every `create_graphics_pipelines` call uses (and every compute pipeline in M5b). Implementation: `crates/rhi-vulkan/src/pipeline_cache.rs`.

- **Path:** `DeviceDesc::pipeline_cache` (`Option<PathBuf>`), else env `ANVIL_PIPELINE_CACHE`, else `target/anvil-pipeline-cache.bin` relative to the working directory.
- **Load (device creation):** read the file; validate the `VkPipelineCacheHeaderVersionOne` header (32 bytes: header size, version 1, vendor ID, device ID, 16-byte `pipelineCacheUUID`) against the selected physical device. A missing file starts empty silently. A short, unreadable, corrupt or mismatched file (other GPU, other driver version) is ignored with a `warn!` naming the reason, and the cache starts empty. The blob is passed as `pInitialData`. Logs `info!` `pipeline cache loaded path bytes`.
- **Save (device destruction, `DeviceShared::drop`):** `vkGetPipelineCacheData`, then write to `<path>.<pid>.<n>.tmp` and rename over the file, so concurrent processes or devices never leave a torn file. Nothing is written when the cache holds only the header (no pipeline was compiled). The directory is not created: a missing directory logs at `debug` and skips the save (tests running in a crate directory therefore never litter `crates/*/target`). Logs `info!` `pipeline cache saved path bytes`; failures are `warn!`, never errors.
- **Trust:** the header check guards against another device or driver; it does not authenticate the blob. The Vulkan driver still validates its own data. shortcut: no content hash or size limit on the file, add a checksum if the file ever comes from an untrusted place.
- **Tests:** unit `header_validation`, `save_then_load_and_corrupt` (pure, no GPU); GPU `pipeline_cache_saved_and_reloaded` (`tests/profiling.rs`: create device, compile a pipeline, drop, file exists; second device loads it; a corrupt file is ignored and replaced).
- Effect: the second run of pbr-scene logs `pipeline cache loaded` (173 888 bytes on RADV). Whole-process start (1 frame) was 53-56 ms with and without it (one 28 ms outlier), because Mesa keeps its own on-disk shader cache; the application-level cache is for drivers without one and for the larger M5b pipeline set. Not part of the BENCH-001 frame metrics.

## 9. Threading model

`bindings` is read-only after creation except the `update` mutex and atomics. `vkUpdateDescriptorSets` on the same set must be externally synchronized (hence `update`); `UPDATE_AFTER_BIND` permits updates while pending command buffers have the set bound, only for slots they do not dynamically use. Pipeline creation takes `&self` with the pool lock (M1). `VkPipelineCache` is internally synchronized by the driver (M2 does not set `EXTERNALLY_SYNCHRONIZED`; verify against the spec text).

## 10. Error handling

| Condition | Error |
|---|---|
| set layout/pool/set creation fails | `OutOfHostMemory`/`OutOfDeviceMemory` or `Backend{op, code}` through `map_vk`; `create_device` fails and unwinds |
| missing feature or limit | rejected in selection (MODULE-002 amendment), so `create_device` never sees it; if reached, `Unsupported{what}` |
| non-`D32Sfloat` depth format, depth format in a color-only call | `InvalidDescriptor` |
| shader/layout mismatch | validation layer only in M2 (reflection is MODULE-008) |
| pipeline compile failure | `Backend{op:"vkCreateGraphicsPipelines", code}` (M1) |
| push constant misuse, format mismatch at draw | recorded `InvalidDescriptor`, returned from `finish()` |

## 11. Performance requirements

Measurable only: pipeline create time (existing M1 trace), count of `update_descriptor_sets` calls (one per texture/sampler create). Nothing allocates per draw; set binding happens once per command buffer. No numeric targets (NFR-PERF-012).

## 12. Testing requirements

Unit (no GPU):

| Test | Checks |
|---|---|
| `bindless_capacity_clamps` | `min(65536, L)` and `min(1024, L)` for L below, equal, above |
| `limits_reject_small` | facts with `L_img = 512` or `L_smp = 32` -> `Rejection::BindlessLimits` (MODULE-002 tests) |
| `layout_binding_descs` | bindings 0/1 types, counts, stage flags, binding flags (pure builder returning the create-info data) |
| `push_range_is_128_all_graphics` | range offset/size/stages |
| `push_constants_validation` | alignment, overflow, boundary 128 |
| `depth_state_mapping` | `None` -> off; `Some(D32Sfloat)` -> test+write+LESS; other format -> `InvalidDescriptor` |
| `doomed_pipeline_has_no_layout` | variant shape (compile-level) |

Integration (`ANVIL_GPU_TESTS=1`, lavapipe, validation `On`, sync validation on, zero errors/warnings):

| Test | Checks |
|---|---|
| `global_bindings_created_and_reported` | `diagnostics().bindless.*_capacity` equal `min(65536, L)` / `min(1024, L)`; non-zero |
| `pipelines_share_one_layout` | two pipelines created and drawn with no validation error; destroying one does not invalidate the other |
| `push_constants_reach_shader` | draw writes a push-constant-derived color to an offscreen target; readback matches |
| `push_constants_misuse_surfaces_at_finish` | misaligned / oversize -> `InvalidDescriptor` at `finish()` |
| `depth_pipeline_draw` | (shared with MODULE-006) depth test orders two overlapping quads |
| `draw_format_mismatch_rejected` | pipeline depth_format vs rendering without depth -> `InvalidDescriptor` at `finish()` |
| `device_without_bindless_features_is_rejected` | selection unit test on facts; on hardware covered by manual run |
| `triangle_example_still_clean` | `examples/triangle` with `ANVIL_EXIT_AFTER_FRAMES=120`: errors 0, warnings 0 on the new layout |
| `pipeline_destroy_deferred` | destroy while in flight, validation clean (M1 test, re-run with global layout) |

## 13. Acceptance criteria

- [ ] Every graphics pipeline is created with the single global layout; destroying a pipeline never destroys the layout.
- [ ] Device selection rejects GPUs lacking the required descriptor-indexing and BDA features or limits (MODULE-002 tests).
- [ ] `push_constants` + `draw_indexed` + bindless sampling work in `examples/textured-quad` with zero validation errors.
- [ ] Depth test works (`depth_pipeline_draw`).
- [ ] `examples/triangle` unchanged and still clean.
- [ ] M5 (deferred): dedupe, persisted cache with blob validation, compute pipelines.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RHI-007, FR-RHI-008 | graphics pipeline + the global layout (M2); compute M5 |
| FR-RHI-009 | pipeline layout derived from the binding model (ADR-0010) |
| FR-VK-011 | empty `VkPipelineCache` passed in M2; persisted and validated in M5a (section 8a) |
| FR-SHD-004 | reflection/layout compatibility with MODULE-008 (later) |
| FR-SHD-005 | shared hashing with shader cache (M5) |
| NFR-PERF-009 | compile-time measurement (M5) |
| ADR-0009 | below |

### Observability (ADR-0009)

| Operation | Level | Fields |
|---|---|---|
| global bindings created | `info` | `sampled_image_capacity, sampler_capacity, push_constant_bytes` (target `rhi_vulkan::bindings`) |
| global bindings destroyed | `debug` | none |
| pipeline create / destroy | `debug` (M1) | adds `depth_format` |
| set bound / push constants | `trace` (inside `frame` span) | `offset, bytes` |
| descriptor write | `trace` | `binding, slot` (emitted by MODULE-006) |
| leaked pipelines at shutdown | `warn` (M1) | `handle` |

Names: `global-bindings.set_layout`, `global-bindings.pipeline_layout`, `global-bindings.pool`, `global-bindings.set` via `set_name`. Diagnostics: `bindless.*` (LAYER-003 §17.5).

## 15. Open questions

Resolved by ADR-0010: binding model (bindless, one layout), push constants (128 B, all graphics stages), no vertex input.

Open (M5, not blocking):
1. Pipeline dedupe: shared handles with refcounts vs renderer-owned.
2. Stable hash function and crate (not `DefaultHasher`).
3. Cache file location and invalidation on driver update.
4. Whether the in-memory shader cache (MODULE-008) and this cache are keyed together.
5. Compute pipelines use the same global layout (push range visibility would add `COMPUTE`); decide with the first compute feature.

Remaining `verify:`
- verify: `PipelineLayoutCreateInfo`/`DescriptorSetLayoutBindingFlagsCreateInfo` builder names in ash 0.38 (`DescriptorBindingFlags::{PARTIALLY_BOUND, UPDATE_AFTER_BIND}`, `DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL`, `DescriptorPoolCreateFlags::UPDATE_AFTER_BIND`).
