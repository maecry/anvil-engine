# MODULE-006: Texture Registry

| Field | Value |
|---|---|
| Status | Accepted |
| Crate | rhi-vulkan (API types and trait methods in rhi, LAYER-003 §17) |
| Layer | LAYER-007 |
| Milestone | M2 |
| Traces to | FR-RHI-003, FR-RHI-004, FR-RHI-005, FR-RES-001, FR-RES-002, FR-RES-003, FR-RES-005, FR-RES-006, FR-RES-009, FR-VK-008, NFR-REL-003; MASTER-000 §10.1-10.3; ADR-0006, ADR-0007, ADR-0009, ADR-0010 |
| Last updated | 2026-11-02 |

## 1. Purpose

Central registry of GPU images (with one default view) and samplers behind `TextureHandle`/`SamplerHandle`: creation, upload, advisory state tracking, bindless slot management and deferred destruction. It generalises the M1 texture pool (swapchain images only, MODULE-003). Guarantee: a bindless slot is never reused while a submission that may sample the old object can still run.

## 2. Responsibilities

- `create_texture`, `destroy_texture`, `texture_index`, `upload_texture`, `copy_texture_to_buffer` validation, `create_sampler`, `destroy_sampler`, `sampler_index` (LAYER-003 §17).
- Pool entries `VkTexture` for owned and swapchain textures (`TextureOwner`).
- Bindless descriptor writes into the global set (owned by MODULE-007) at creation; slot quarantine until retirement.
- Whole-texture advisory `TextureState` (ADR-0010 §9).
- Deferred destruction of image, view and allocation (`Doomed::Image`), samplers (`Doomed::Sampler`).
- `LiveObjects.textures/samplers`, `BindlessDiagnostics`.

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Buffers, staging mechanics, the upload command buffer | MODULE-005 (this module records into it) |
| Global descriptor set, set layout, pipeline layout, descriptor pool | MODULE-007 (`GlobalBindings`) |
| Image decoding, KTX2, streaming, mips | LAYER-009 / M5 |
| Automatic barrier placement, per-subresource state | render graph (M3/M4) |
| Swapchain image lifetime | MODULE-003 |
| Sampler dedupe cache | none in M2 (ADR-0010 §11) |
| Storage images, sampled depth, sRGB views, block-compressed formats | later |

## 4. Dependencies

Allowed: foundation, rhi, MODULE-002 (`DeviceShared`, allocator wrappers `alloc`/`free`, `set_name`), MODULE-004 (`Doomed`, deferred queue), MODULE-005 (`UploadState`, staging helpers), MODULE-007 (`GlobalBindings`), `ash`, `gpu-allocator`, `tracing`. Forbidden: layer 3+ crates.

## 5. Public API

Trait methods and `rhi` types: LAYER-003 §17. Backend-internal:

```rust
pub(crate) enum TextureOwner { Swapchain, Device }

pub(crate) struct VkTexture {                       // grows the M1 struct {image, view, format}
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub format: Format,
    pub width: u32, pub height: u32,                // swapchain images: the swapchain extent
    pub usage: TextureUsage,
    pub owner: TextureOwner,                        // Swapchain for M1 images (usage = COLOR_ATTACHMENT|TRANSFER_SRC as created)
    pub allocation: Option<Allocation>,             // None for Swapchain-owned
    pub state: TextureState,                        // advisory
    pub label: String,
}
pub(crate) struct VkSampler { pub raw: vk::Sampler, pub label: String }

// Pools gains:
pub samplers: Mutex<Registry<SamplerTag, VkSampler>>,   // textures stays RwLock<Registry<TextureTag, VkTexture>>

// registry.rs: slot quarantine (new methods on Registry<Tag, V>)
pub(crate) fn take(&mut self, h: Handle<Tag>) -> Option<V>;     // value out, id STILL allocated: get() is None (stale), index not reusable
pub(crate) fn release(&mut self, h: Handle<Tag>);               // ids.remove(h): index becomes reusable
pub(crate) fn len(&self) -> usize;                              // changed: counts live values only (excludes quarantined)

// destroy.rs
pub(crate) enum Doomed { /* ..., */
    Image   { image: vk::Image, view: vk::ImageView, allocation: Allocation, slot: TextureHandle },
    Sampler { sampler: vk::Sampler, slot: SamplerHandle },
}
// destroy_doomed changes to `unsafe fn destroy_doomed(s: &DeviceShared, d: Doomed)` so it can free allocations and release pool slots:
//   Image   -> destroy_image_view, destroy_image, s.free(allocation), pools.textures.write().release(slot)
//   Sampler -> destroy_sampler, pools.samplers.lock().release(slot)
//   Pipeline-> destroy_pipeline only (the layout is global now; MODULE-007)
```

## 6. Data structures

### 6.1 Format and usage mapping

| `Format` | `vk::Format` | Aspect |
|---|---|---|
| `Bgra8Srgb` / `Bgra8Unorm` / `Rgba8Srgb` / `Rgba8Unorm` | `B8G8R8A8_SRGB` / `_UNORM` / `R8G8B8A8_SRGB` / `_UNORM` | COLOR |
| `R8Unorm` | `R8_UNORM` | COLOR |
| `Rg8Unorm` | `R8G8_UNORM` | COLOR |
| `Rgba16Float` | `R16G16B16A16_SFLOAT` | COLOR |
| `D32Sfloat` | `D32_SFLOAT` | DEPTH |

| `TextureUsage` | `vk::ImageUsageFlags` | Required format feature (optimal tiling) |
|---|---|---|
| `SAMPLED` | `SAMPLED` | `SAMPLED_IMAGE` (+ `SAMPLED_IMAGE_FILTER_LINEAR` is not required; linear sampler on a non-filterable format is the caller's error, verify validation message) |
| `COLOR_ATTACHMENT` | `COLOR_ATTACHMENT` | `COLOR_ATTACHMENT` |
| `DEPTH_ATTACHMENT` | `DEPTH_STENCIL_ATTACHMENT` | `DEPTH_STENCIL_ATTACHMENT` |
| `TRANSFER_SRC` / `TRANSFER_DST` | `TRANSFER_SRC` / `TRANSFER_DST` | `TRANSFER_SRC` / `TRANSFER_DST` |

Features come from `instance.get_physical_device_format_properties(physical, vk_format).optimal_tiling_features`. The image is `TYPE_2D`, `mip_levels 1`, `array_layers 1`, `SAMPLE_COUNT_1`, `OPTIMAL`, exclusive, initial layout `UNDEFINED`. Extent must be `<= limits.max_image_dimension2d` (from the physical-device properties).

### 6.2 State to barrier mapping (extends MODULE-004 image barriers)

| Layout | Stage / access (sync2) |
|---|---|
| `Undefined` | `NONE` / `NONE` |
| `TransferDst` -> `TRANSFER_DST_OPTIMAL` | `COPY` / `TRANSFER_WRITE` |
| `TransferSrc` -> `TRANSFER_SRC_OPTIMAL` | `COPY` / `TRANSFER_READ` |
| `ShaderRead` -> `SHADER_READ_ONLY_OPTIMAL` | `VERTEX_SHADER\|FRAGMENT_SHADER` / `SHADER_SAMPLED_READ` |
| `ColorAttachment` | unchanged from M1 |
| `DepthAttachment` -> `DEPTH_ATTACHMENT_OPTIMAL` | `EARLY_FRAGMENT_TESTS\|LATE_FRAGMENT_TESTS` / `DEPTH_STENCIL_ATTACHMENT_READ\|WRITE` |

`image_barrier` takes the aspect from the texture's format (previously fixed to COLOR). Any pair involving `Present` on a non-swapchain texture, or `DepthAttachment <-> ShaderRead`, is `Unsupported` (recorded). Pairs not above keep their M1 behavior.

### 6.3 Bindless slots

Slot = `handle.index()` (ADR-0010 §4). Capacity is `GlobalBindings.sampled_image_capacity` / `sampler_capacity` (MODULE-007). The texture registry holds swapchain and non-sampled textures too, so the index space is shared: `index >= capacity` for a **SAMPLED** texture is `Unsupported{what: "bindless sampled image capacity (N) exceeded"}`; non-sampled textures with a larger index are fine. Sampler registry indexes map 1:1 to the sampler array. The descriptor is written at creation (section 8); the slot becomes reusable only when `Doomed::Image/Sampler` retires and calls `release`. A retired slot keeps its stale descriptor (legal: `PARTIALLY_BOUND`, never dynamically used because no live handle maps to it); the next owner overwrites it.

`bindless_high_water.{images,samplers}: AtomicU32 = max(slot + 1)` at write time, reported in `BindlessDiagnostics`.

## 7. Ownership and lifetime

Pool owns `VkTexture`/`VkSampler`. `destroy_texture(h)`:
1. `write(pools.textures)`; `get(h)` -> `None` => `warn!(handle, "destroy_texture on stale handle ignored")`, return.
2. `owner == Swapchain` => `warn!(handle, "destroy_texture on swapchain-owned texture ignored")`, return (the infallible signature cannot return `InvalidDescriptor`; MODULE-003 destroys those).
3. `take(h)` (stale now, index quarantined), then `defer(Doomed::Image{.., slot: h})` with tag `doom_tag()`.
4. `allocated_bytes` drops at retirement (MODULE-005 `free`).

`destroy_sampler` is identical with `Doomed::Sampler`. Shutdown: after draining, leftover owned textures/samplers are removed, warned, destroyed; quarantined entries do not exist after the drain (their `Doomed` ran `release`). Swapchain textures are removed by MODULE-003 as in M1 (plain `remove`; their indices are reusable at once, which is safe: they were never written to the descriptor array).

Create order for a texture: image -> `alloc` (scheme `DedicatedImage(image)` for attachments, `GpuAllocatorManaged` otherwise; ADR-0007) -> `bind_image_memory` -> default view -> debug names (image and view) -> pool insert -> if `SAMPLED`: descriptor write. Failure unwinds in reverse; if the SAMPLED capacity check fails after insert, the entry is `remove`d and destroyed immediately (never submitted).

`unsafe` invariants:

| Block | Invariant |
|---|---|
| `create_image`/`bind_image_memory`/`create_image_view` | create infos outlive calls; image bound once; view format/aspect match the image |
| `update_descriptor_sets` | `GlobalBindings.update` mutex held (external sync of the set); `dst_array_element < capacity`; image view/sampler alive (just created); the set was created `UPDATE_AFTER_BIND` and the binding has `UPDATE_AFTER_BIND` flag |
| recording into `upload_cb` | `UploadState` mutex held; command buffer in recording state; image resolved under pool lock and alive until the submission retires (destroy is deferred past it) |
| `destroy_doomed` Image/Sampler | tag retired or device idle; exactly-once |

## 8. Execution flow

`create_texture(desc)`:
1. Validate (all `InvalidDescriptor` unless stated): width/height > 0 and within the device limit; usage non-empty; `COLOR_ATTACHMENT` only with a color format; `DEPTH_ATTACHMENT` only with a depth format; depth format with `SAMPLED` -> `Unsupported("sampled depth textures (M2)")`; each requested usage supported by 6.1 features -> else `Unsupported{what: "<format> with <usage>"}`.
2. Create/allocate/bind/view as in section 7. Initial `state = Undefined`.
3. Insert in the pool, apply names (`label` or `texture#<i>v<g>`).
4. If `SAMPLED`: capacity check, then `GlobalBindings::write_sampled_image(slot, view)` (MODULE-007: `vkUpdateDescriptorSets` with `DescriptorImageInfo{ sampler: null, image_view, image_layout: SHADER_READ_ONLY_OPTIMAL }`, type `SAMPLED_IMAGE`, binding 0, `dst_array_element = slot`); bump high-water.
5. `debug!(handle, label, width, height, format = ?, usage = ?, slot)`.

`texture_index(h)`: stale -> `StaleHandle{kind:"texture"}`; `owner == Swapchain` or no `SAMPLED` -> `InvalidDescriptor`; else `Ok(h.index())`.

`upload_texture(h, data)`:
- T0 validate: stale; owner `Device`; `TRANSFER_DST`; not depth; `data.len() == width*height*format.bytes_per_texel()` else `InvalidDescriptor` (message gives expected/actual); tracked `state` is `Undefined` or `ShaderRead` else `InvalidDescriptor("upload_texture from state X")`. Callable any time; queued like buffer uploads (MODULE-005 §6.3 lead amendment).
- T1 staging buffer (`TRANSFER_SRC`, `CpuToGpu`), copy `data` in; `defer(Doomed::Buffer{staging})`.
- T2 `ensure_upload_cb()` (MODULE-005 U2, includes the leading global barrier).
- T3 barrier `old_layout = (Undefined -> UNDEFINED | ShaderRead -> SHADER_READ_ONLY_OPTIMAL)` to `TRANSFER_DST_OPTIMAL` (src per old state: `NONE`, or `VERTEX_SHADER|FRAGMENT_SHADER`/`SHADER_SAMPLED_READ`; dst `COPY`/`TRANSFER_WRITE`).
- T4 `cmd_copy_buffer_to_image(upload_cb, staging, image, TRANSFER_DST_OPTIMAL, [BufferImageCopy{ buffer_offset 0, buffer_row_length 0, buffer_image_height 0, subresource{COLOR, mip 0, layer 0, count 1}, offset 0, extent {w, h, 1} }])` (tightly packed).
- T5 barrier `TRANSFER_DST_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL` (src `COPY`/`TRANSFER_WRITE`, dst `VERTEX_SHADER|FRAGMENT_SHADER`/`SHADER_SAMPLED_READ`; the trailing global barrier of MODULE-005 U4 additionally covers buffers).
- T6 `state = ShaderRead`; `trace!(handle, bytes)`.
Order T3-T5 is inside `upload_cb`, which is submitted before the frame command buffer in the same `vkQueueSubmit2`, so a draw in that frame sees the final layout.

`copy_texture_to_buffer(t, buffer, off)` (encoder, outside rendering): texture owner `Device`, usage `TRANSFER_SRC`, buffer `TRANSFER_DST` and `off + w*h*bpt <= size`, color format; `cmd_copy_image_to_buffer` with layout `TRANSFER_SRC_OPTIMAL` (the caller put it in `TransferSrc` with `texture_barrier`); if the buffer is `Readback`, emit the `COPY -> HOST/HOST_READ` memory barrier (MODULE-005).

`texture_barrier(t, from, to)` (M1 method): additionally compares `from` with `tex.state`; mismatch => `warn!(handle, tracked, from, "texture_barrier from-state mismatch")` and the barrier uses the caller's `from` (advisory, ADR-0010 §9); after recording `tex.state = to`. Swapchain textures are exempt from tracking.

`create_sampler(desc)`:
1. Create the `vk::Sampler` first with `mag/min filter`, `mipmap_mode NEAREST`, `address_mode_u/v/w`, `anisotropy_enable false`, `compare_enable false`, `min_lod 0.0`, `max_lod 0.0`, `border_color FLOAT_OPAQUE_BLACK` (unused: no `CLAMP_TO_BORDER` mode exposed), `unnormalized_coordinates false`.
2. Insert in the pool, names, capacity check (`index >= sampler_capacity` => `Unsupported`, destroy immediately), `GlobalBindings::write_sampler(slot, sampler)` (type `SAMPLER`, binding 1, `DescriptorImageInfo{ sampler, image_view: null, layout: UNDEFINED }`), high-water, `debug!`.

`sampler_index(h)`: `Ok(h.index())` or `StaleHandle{kind:"sampler"}`.

## 9. Threading model

Creation/upload/destroy take `&self`. Pool locks: `textures` is `RwLock` (reads on encoder paths: `texture_barrier`, `begin_rendering`, `copy_*`), `samplers` is `Mutex`. The descriptor set update takes `GlobalBindings.update` (Vulkan requires external synchronization of the set object; `UPDATE_AFTER_BIND` allows updates while bound by pending command buffers, provided the written slot is not dynamically used by them). Lock order: `pools` -> `allocator` -> `GlobalBindings.update` -> `upload`; `deferred` never nested. Retirement runs in `begin_frame` (render thread) and takes pool write locks briefly. Frame ops stay `&mut self`; encoders hold only `Arc<DeviceShared>`.

## 10. Error handling

| Condition | Error |
|---|---|
| zero/oversized extent, empty usage, usage/format class mismatch, wrong data length, wrong tracked state, `texture_index` on non-SAMPLED or swapchain texture, `upload_texture` without `TRANSFER_DST`, upload outside a frame | `InvalidDescriptor` |
| unsupported format/usage combination, sampled depth, bindless capacity exceeded, unsupported state pair | `Unsupported{what}` |
| dead handle | `StaleHandle{kind:"texture"/"sampler"}` |
| memory exhaustion | `OutOfDeviceMemory` / `OutOfHostMemory`; allocator variants as MODULE-005 |
| `destroy_*` on stale or swapchain-owned handle | `warn!` and ignored (infallible signature) |
| `texture_barrier` from-state mismatch | `warn!`, not an error (advisory tracking) |

## 11. Performance requirements

Measure (NFR-PERF-004, NFR-PERF-008): texture upload bytes and time (submit trace), `bindless.*_high_water`, allocator bytes. Descriptor writes happen once per create, never per frame or draw. Registry lookup cost is profiled before any redesign (ADR-0006). No numeric targets (NFR-PERF-012).

## 12. Testing requirements

Unit (no GPU):

| Test | Checks |
|---|---|
| `texture_usage_ops` | `|`, `contains`, `is_empty` |
| `format_bytes_per_texel` | R8 1, Rg8 2, RGBA/BGRA 4, Rgba16Float 8, D32 4 |
| `format_to_vk_and_aspect` | table 6.1 |
| `usage_to_vk_image_usage` | table 6.1 |
| `create_validation_table` | zero extent, empty usage, color usage on depth format, depth usage on color format, sampled depth -> expected error kinds (pure helper over a `FormatFacts` struct) |
| `upload_len_check` | `w*h*bpt` equality, overflow-safe |
| `state_pair_mapping` | table 6.2 incl. `DepthAttachment`, unsupported pairs |
| `registry_take_quarantines_index` | after `take`, `get` is None, next `insert` gets a different index; after `release` the index is reused with a new generation |
| `registry_len_excludes_quarantined` | `len` counts live values |
| `slot_equals_handle_index` | `texture_index`/`sampler_index` helpers |
| `high_water_monotonic` | max(slot+1) |
| `sampler_info_mapping` | `Filter`/`AddressMode` -> vk enums; lod 0 |
| `tracked_state_mismatch_is_warning` | helper returns `Mismatch` not `Err` |

Integration (`ANVIL_GPU_TESTS=1`, lavapipe, validation `On`, sync validation on; each ends with `validation.errors == 0`, warnings 0):

| Test | Checks |
|---|---|
| `texture_upload_readback_exact` | 16x16 `Rgba8Unorm` pattern, `upload_texture` (frame open), barrier `ShaderRead->TransferSrc`, `copy_texture_to_buffer` into `Readback`, submit_unwindowed, `wait_idle`, `read_buffer` byte-exact |
| `texture_upload_twice` | second `upload_texture` from `ShaderRead` works, latest data wins |
| `texture_sample_in_draw` | offscreen 8x8 target; textured full-screen quad (test shaders, BDA vertex pull) with a solid-color texture sampled through `texture_index`/`sampler_index` push constants; readback pixels equal the texel color |
| `texture_checkerboard_quad_pixels` | the `textured-quad` geometry on an offscreen target: sampled pixels at cell centers match the CPU checkerboard |
| `depth_pipeline_draw` | `D32Sfloat` target + `depth_format` pipeline, two overlapping quads at different depth, nearer wins in readback via a color target; validation clean |
| `bindless_slot_not_reused_before_retire` | create+destroy texture A; create B before the GPU retires A: `texture_index(B) != old index(A)`; after frames advance past the tag, a new texture gets the quarantined index (new generation) |
| `bindless_stale_index_rejected` | `texture_index(A)` after destroy -> `StaleHandle`; likewise `sampler_index` |
| `bindless_capacity_exceeded` | with a test hook lowering capacity to 4: fifth SAMPLED texture -> `Unsupported`, nothing leaked |
| `texture_swapchain_owned_rejected` | needs a display: `#[ignore]` without one; `texture_index` of a swapchain image -> `InvalidDescriptor`, `destroy_texture` ignored with warn |
| `texture_unsupported_combo` | e.g. `SAMPLED` on `D32Sfloat` -> `Unsupported` |
| `sampler_create_destroy` | create, index, destroy, slot quarantined, diagnostics counts back to 0 |
| `texture_destroy_while_in_flight` | destroy right after a submit that sampled it: validation clean, retired only after the tag |
| `diagnostics_bindless_and_memory` | `live.textures/samplers`, `bindless.*_high_water`, `memory.allocated_bytes` follow creates/destroys (allocated drops after retirement) |
| `texture_leak_at_shutdown` | undestroyed texture and sampler -> one leak `warn` each, no validation error |

## 13. Acceptance criteria

- [ ] `texture_upload_readback_exact` and `texture_sample_in_draw` pass (`Rgba8Unorm` exact).
- [ ] Stale and reused-slot handles are rejected (unit + integration).
- [ ] A bindless slot is not reused before its `Doomed` retires (`bindless_slot_not_reused_before_retire`).
- [ ] No image, view or sampler is destroyed while a submitted command buffer may reference it (validation clean under `texture_destroy_while_in_flight`).
- [ ] Layout transitions Undefined -> TransferDst -> ShaderRead are emitted by `upload_texture` and the tracked state ends `ShaderRead`.
- [ ] Zero validation errors and warnings in all integration tests; zero leaks in the `textured-quad` example.
- [ ] Every create/destroy traced per ADR-0009; every object named.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RHI-003, FR-RHI-004, FR-RHI-005, FR-RES-009 | images, one default view, samplers |
| FR-RES-001, FR-RES-002, FR-RES-003 | `Handle<T>` / `Registry`, quarantine keeps indices unique until retirement |
| FR-RES-005 | advisory `TextureState` |
| FR-RES-006 | `Doomed::Image/Sampler` |
| FR-VK-008 | barrier translation extended (6.2) |
| NFR-REL-003 | leak tests |
| ADR-0009 | below |

### Observability (ADR-0009)

| Operation | Level | Fields |
|---|---|---|
| `create_texture` / `create_sampler` | `debug` (`rhi_vulkan::texture`) | `handle, label, width, height, format, usage, slot` / `handle, label, filters, address, slot` |
| `destroy_texture` / `destroy_sampler` | `debug`; `warn` for stale or swapchain-owned | `handle` |
| retirement | `debug` | `kind = "image"/"sampler"`, `slot` released |
| bindless descriptor write | `trace` | `binding, slot, high_water` |
| `upload_texture` | `trace` (in `frame` span) | `handle, bytes, from_state` |
| barrier from-state mismatch | `warn` | `handle, tracked, from` |
| capacity exceeded, unsupported combo | `warn` | `slot/capacity` / `format, usage` before returning the error |
| leaked at shutdown | `warn` | `handle, label` |

Names: `set_name` on image, view (`<label>.view`) and sampler; unlabeled `texture#<i>v<g>`, `sampler#<i>v<g>`. Diagnostics: `live.textures` (includes swapchain images), `live.samplers`, `bindless.*`, `memory.*`.

## 15. Open questions

None blocking. Resolved by ADR-0010:

1. Image views: one default view per texture; no `TextureViewHandle` until needed (§10).
2. Per-subresource state: whole-texture only (§9).
3. Sampler dedupe cache: none (§11).
4. M2 formats: M1 set + `R8Unorm`, `Rg8Unorm`, `Rgba16Float`, `D32Sfloat`; sRGB views and block-compressed formats later (§12).
5. Tracked state: advisory; the backend warns on mismatch, the render graph owns truth from M3 (§9).
6. Streaming/sparse textures: M5.

Remaining `verify:`
- verify: whether `SAMPLED` on `Rgba16Float`/`R8Unorm` with a `Linear` sampler requires `SAMPLED_IMAGE_FILTER_LINEAR` (validation message on lavapipe/RADV); M2 does not check it.
- verify: sampler arrays and `descriptorBindingSampledImageUpdateAfterBind` cover `VK_DESCRIPTOR_TYPE_SAMPLER` (Vulkan spec lists SAMPLER under `descriptorBindingSampledImageUpdateAfterBind`), confirm in the validation layer on first run.
