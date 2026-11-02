# MODULE-005: Buffer Allocator

| Field | Value |
|---|---|
| Status | Accepted |
| Crate | rhi-vulkan (API types and trait methods in rhi, LAYER-003 §17) |
| Layer | LAYER-007 |
| Milestone | M2 |
| Traces to | FR-RHI-002, FR-RHI-014, FR-RHI-015, FR-VK-009, FR-RES-004, FR-RES-006, FR-RES-007, FR-RES-008, FR-RES-010, NFR-REL-003, NFR-ARCH-007; MASTER-000 §10.1-10.3; ADR-0006, ADR-0007, ADR-0009, ADR-0010 |
| Last updated | 2026-11-02 |

## 1. Purpose

Create, write, upload to, address and destroy GPU buffers through `BufferHandle`s, with memory from `gpu-allocator` (created in MODULE-002) and destruction deferred on the timeline value (MODULE-004). Guarantee: no native buffer or allocation is freed while a submission that may reference it can still run, and a stale handle never reaches Vulkan.

## 2. Responsibilities

- `create_buffer`, `destroy_buffer`, `buffer_address`, `write_buffer`, `read_buffer`, `upload_buffer`, `copy_buffer_to_buffer`, `bind_index_buffer` validation (signatures: LAYER-003 §17.2-17.3).
- Buffer pool `Registry<BufferTag, VkBuffer>` (generation checks, ADR-0006).
- Persistent mapping for `Upload`/`Readback` memory.
- Staging for `upload_buffer` and the per-frame upload command buffer (shared with MODULE-006, owned here: section 6.3).
- Deferred destruction of buffer + allocation (`Doomed::Buffer`), including internal staging buffers.
- Memory accounting (`MemoryDiagnostics`) and `LiveObjects.buffers`.
- Leak reporting at shutdown.

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Images, views, samplers, bindless slots | MODULE-006 |
| Global descriptor set, pipeline layout, push-constant range | MODULE-007 |
| Residency/streaming, asset decoding | LAYER-009 / M5 |
| Transient aliasing | MODULE-009 (M4) |
| Dedicated transfer queue, linear/ring allocator | M5 (ADR-0010 §6-7) |
| Buffer state tracking / explicit buffer barriers | none in M2 (ADR-0010 §9); upload barriers are implicit (section 8) |

## 4. Dependencies

Allowed: foundation, rhi, MODULE-002 (`DeviceShared`, allocator, `set_name`), MODULE-004 (`SyncShared`, `FrameSlot`, `Doomed`), `ash`, `gpu-allocator`, `tracing`. Forbidden: layer 3+ crates; `gpu-allocator`/`ash` types in public signatures.

## 5. Public API

Trait methods and `rhi` types: LAYER-003 §17 (authoritative, not repeated). Backend-internal:

```rust
pub(crate) struct VkBuffer {
    pub raw: vk::Buffer,
    pub allocation: Option<gpu_allocator::vulkan::Allocation>, // None only after being moved into Doomed
    pub size: u64,
    pub usage: BufferUsage,
    pub memory: Memory,
    pub address: u64,                       // vkGetBufferDeviceAddress, queried at creation
    pub mapped: Option<MappedPtr>,          // Some for Upload/Readback
    pub coherent: bool,                     // memory type is HOST_COHERENT (`Allocation::memory_properties()` contains `HOST_COHERENT`, confirmed in 0.28)
    pub label: String,                      // user label or generated "buffer#3v1"
}
pub(crate) struct MappedPtr(NonNull<u8>);   // unsafe impl Send + Sync: points into memory owned by `allocation`

// Pools gains:
pub buffers: RwLock<Registry<BufferTag, VkBuffer>>,

// destroy.rs
pub(crate) enum Doomed { /* M1 variants, plus: */ Buffer { buffer: vk::Buffer, allocation: Allocation } }

// Registry: no change needed for buffers (plain `remove`; no slot semantics). 

// Memory accounting on DeviceShared
pub allocated_bytes: AtomicU64,             // += allocation.size() on allocate, -= on free
pub(crate) fn alloc(&self, desc: &AllocationCreateDesc) -> Result<Allocation, RhiError>;   // locks allocator, maps errors, updates counter, logs
pub(crate) fn free(&self, a: Allocation);                                                   // idem
```

## 6. Data structures

### 6.1 Usage and memory mapping

| `BufferUsage` | `vk::BufferUsageFlags` |
|---|---|
| (always) | `SHADER_DEVICE_ADDRESS` (ADR-0010 §2) |
| `INDEX` | `INDEX_BUFFER` |
| `STORAGE` | `STORAGE_BUFFER` |
| `UNIFORM` | `UNIFORM_BUFFER` |
| `TRANSFER_SRC` / `TRANSFER_DST` | `TRANSFER_SRC` / `TRANSFER_DST` |

| `Memory` | `gpu_allocator::MemoryLocation` | Mapped |
|---|---|---|
| `GpuOnly` | `GpuOnly` | no |
| `Upload` | `CpuToGpu` | yes (gpu-allocator maps host-visible blocks persistently) |
| `Readback` | `GpuToCpu` | yes |

Allocation: `AllocationCreateDesc { name: &label, requirements: get_buffer_memory_requirements, location, linear: true, allocation_scheme: GpuAllocatorManaged }` (verify field names against gpu-allocator 0.28: confirmed `name, requirements, location, linear, allocation_scheme`; scheme enum is `AllocationScheme::{DedicatedBuffer, DedicatedImage, GpuAllocatorManaged}`). The allocator is created with `buffer_device_address: true` (MODULE-002 amendment) so it adds `VK_MEMORY_ALLOCATE_DEVICE_ADDRESS`.

### 6.2 Invariants

- `size > 0`, `usage` non-empty; `address != 0`.
- `mapped.is_some()` iff `memory != GpuOnly`.
- A destroyed buffer is absent from the pool (stale) from the moment `destroy_buffer` returns.

### 6.3 Per-frame upload state (owned here, used by MODULE-006)

```rust
pub(crate) struct UploadState {            // DeviceShared.upload: Mutex<UploadState>
    pub slot: Option<usize>,               // Some while a frame is open (set by CommandManager::begin_frame, cleared by submit)
    pub cb: vk::CommandBuffer,             // this slot's upload command buffer (from FrameSlot.upload_cb)
    pub recording: bool,                   // vkBeginCommandBuffer issued this frame
    pub uploads: u32, pub bytes: u64,      // for the submit trace
}
```
`FrameSlot` gains `upload_cb: vk::CommandBuffer` allocated from the slot's existing pool at slot creation; the pool reset in `begin_frame` resets both. `CommandManager::begin_frame` sets `upload = {slot: Some(i), cb: upload_cb, recording: false, ..}`; `submit` calls `UploadState::finish()` (section 8, step U4) and, if it returns a command buffer, submits `[upload_cb, frame_cb]` as two `CommandBufferSubmitInfo`s in the same `SubmitInfo2` (single `vkQueueSubmit2`). The mutex is leaf-level: never held while taking `pools`, `allocator` or `deferred` (lock order: `pools` -> `allocator` -> `upload`; `deferred` is never held with any of them).

## 7. Ownership and lifetime

Pool owns `VkBuffer`; the allocation lives inside it until `destroy_buffer` moves `{raw, allocation}` into `Doomed::Buffer`. `destroy_buffer(h)`: `pools.buffers.write().remove(h)` (stale now) -> `sync.defer(Doomed::Buffer{..})` with tag `doom_tag()` (= `last_submitted + 1`, ADR-0010 §8, conservative global last-use) -> `allocated_bytes` drops when the allocation is freed at retirement, not at destroy. Retirement (`begin_frame` and shutdown drain): `destroy_buffer` then `allocator.free`. Staging buffers use the same path. An unmapped destroy of a stale handle logs `warn` and does nothing.

Create order: `vkCreateBuffer` -> requirements -> `alloc` -> `bind_buffer_memory` -> `get_buffer_device_address` -> debug name -> pool insert. Failure unwinds in reverse (free allocation, destroy buffer).

Shutdown (MODULE-002 §7 step 3): after draining the deferred queue, remaining pool entries are removed, warned once each (`leaked buffer destroyed at shutdown`, handle + label), destroyed and freed.

`unsafe` invariants (each SAFETY comment must state):

| Block | Invariant |
|---|---|
| `create_buffer`/`bind_buffer_memory`/`get_buffer_device_address` | create info and slices outlive the call; buffer is bound exactly once; usage includes `SHADER_DEVICE_ADDRESS` |
| mapped copy (`write_buffer`, `read_buffer`, staging fill) | pointer comes from the buffer's own allocation, `offset + len <= size` was checked, source/dest slices do not alias the mapping; host-vs-GPU ordering is the caller's contract (write: GPU not reading that range; read: GPU finished) |
| flush/invalidate (non-coherent) | range is aligned to `nonCoherentAtomSize` and clamped to the allocation (verify exact ranges via `Allocation::offset()`/`size()`) |
| `destroy_doomed` Buffer | tag retired or device idle; exactly-once because entries are popped |

## 8. Execution flow

`create_buffer(desc)`: (1) validate: `size > 0`, usage non-empty, else `InvalidDescriptor`; (2) build `vk::BufferCreateInfo` (exclusive sharing, usage per 6.1); (3) create, allocate, bind, query address; (4) name via `set_name(buffer, label)`; unlabeled -> `generated_name("buffer", h)` applied after pool insert; (5) insert; `debug!(handle, label, size, usage = ?, memory = ?, address)`.

`buffer_address(h)`: pool read -> `Ok(address)` or `StaleHandle{kind:"buffer"}`.

`write_buffer(h, offset, data)`: resolve (stale -> `StaleHandle`); require `memory == Upload` else `InvalidDescriptor("write_buffer needs Memory::Upload")`; `offset.checked_add(len) <= size` else `InvalidDescriptor`; empty `data` is `Ok` no-op; `ptr::copy_nonoverlapping` into `mapped + offset`; if `!coherent` flush the aligned range. `trace!(handle, offset, bytes)`.

`read_buffer(h, offset, out)`: same checks with `Readback`; if `!coherent` invalidate the aligned range first; copy out.

`upload_buffer(h, offset, data)` (any memory kind with `TRANSFER_DST`):
- U0 validate: stale, `TRANSFER_DST` in usage, range, empty data is a no-op. Callable at any time: the staging copy is pushed onto the device-level `pending_uploads` queue (Mutex) and recorded into the upload command buffer at the next submit.
- U1 create a staging buffer (`TRANSFER_SRC`, `CpuToGpu`, size `len`, name `staging(<label>)`), copy `data` into its mapping.
- U2 `ensure_upload_cb()`: on first use this frame, `vkBeginCommandBuffer(upload_cb, ONE_TIME_SUBMIT)` and record the **leading barrier** (`cmd_pipeline_barrier2`, one `MemoryBarrier2`: src `ALL_COMMANDS` / `MEMORY_READ|MEMORY_WRITE`, dst `COPY` / `TRANSFER_READ|TRANSFER_WRITE`). It protects uploads against reads/writes by earlier frames' submissions (WAR/WAW).
- U3 `cmd_copy_buffer(upload_cb, staging, dst, [BufferCopy{0, offset, len}])`; `defer(Doomed::Buffer{staging})` (tag = this frame's submit value); update `uploads`/`bytes`; `trace!(handle, offset, bytes)`.
- U4 `UploadState::finish()` at `submit` (only if `recording`): record the **trailing barrier** (one `MemoryBarrier2`: src `COPY` / `TRANSFER_WRITE`, dst `ALL_COMMANDS` / `SHADER_SAMPLED_READ|SHADER_STORAGE_READ|SHADER_STORAGE_WRITE|UNIFORM_READ|INDEX_READ|TRANSFER_READ|MEMORY_READ`; this is "a global memory barrier to all shader reads" of ADR-0010 §7), `vkEndCommandBuffer`, return the cb, reset the state. MODULE-006 image transitions are recorded in the same command buffer between U2 and U4.
- **Lead amendment (supersedes "open frame" wording anywhere in this doc):** uploads are queued in `DeviceShared.pending_uploads: Mutex<Vec<PendingUpload>>` (staging buffer + region, or texture + layout transitions). Every submit (windowed and `submit_unwindowed`) drains the queue: if non-empty, it records the current slot's `upload_cb` (leading barrier, copies, trailing barrier) and submits `[upload_cb, frame_cb]` in one `SubmitInfo2`; the staging buffers are deferred-destroyed at that submit's timeline value. An abandoned frame loses nothing: the queue persists to the next submit.
- Uploads are recorded in call order; a later upload to the same range wins (queue order inside one command buffer, with no barrier between copies: callers must not overlap ranges within one frame — documented limitation).

`copy_buffer_to_buffer(src, so, dst, do, size)` (encoder, outside rendering): resolve both under one pool read lock, check usages/ranges/non-overlap-if-same-buffer, `cmd_copy_buffer2`/`cmd_copy_buffer` (verify ash 0.38 name; sync2 variant `cmd_copy_buffer2` with `CopyBufferInfo2` preferred). No implicit barrier: the caller orders it after uploads (trailing barrier covers `TRANSFER_READ`) and before reads by `wait_idle`/barriers; for GPU->readback the test relies on `wait_idle` plus a host-read barrier recorded by the encoder (`MemoryBarrier2` src `COPY`/`TRANSFER_WRITE`, dst `HOST`/`HOST_READ`) which `copy_buffer_to_buffer` emits when `dst.memory == Readback`.

`bind_index_buffer(b, offset, fmt)`: resolve; `INDEX` usage; `offset % size_of(fmt) == 0` and `< size`; `cmd_bind_index_buffer(cb, raw, offset, UINT16|UINT32)`. Errors are recorded in the encoder (LAYER-003 §8).

`destroy_buffer`: section 7.

## 9. Threading model

All methods take `&self`; the pool is an `RwLock`, the allocator a `Mutex<Option<Allocator>>`, the upload state a `Mutex`. Mapped copies run outside every lock after the pointer is read. Upload recording is single-writer per frame in M2 (frame ops are `&mut self` on one thread); concurrent worker-thread `upload_*` is safe by the mutex but serializes. `VkBuffer` is `Send + Sync` via `MappedPtr` (compile-time assertion test). Vulkan external sync: recording into `upload_cb` happens under the `upload` mutex; submission under the queue mutex (MODULE-004).

## 10. Error handling

| Condition | Error |
|---|---|
| `size == 0`, empty usage, write to non-`Upload`, read from non-`Readback`, upload without `TRANSFER_DST`, range out of bounds / overflow, bad index alignment | `InvalidDescriptor` (message names field and values) |
| dead / reused handle | `StaleHandle{kind:"buffer", index, generation}` |
| `ERROR_OUT_OF_DEVICE_MEMORY` or `AllocationError::OutOfMemory` | `OutOfDeviceMemory` |
| `ERROR_OUT_OF_HOST_MEMORY` | `OutOfHostMemory` |
| `NoCompatibleMemoryTypeFound`, `FailedToMap`, other allocator variants | `Backend{op:"gpu_allocator", code:-1}` (variant logged) |
| other `vkCreateBuffer`/`vkBind*` failures | `Backend{op, code}` via `map_vk` |

No path panics. Recovery: callers may retry after freeing resources; a failed create leaves the pool unchanged.

## 11. Performance requirements

Measure, do not promise (NFR-PERF-012): upload bytes and count per frame (from the submit trace), allocator reserved vs allocated bytes (`MemoryDiagnostics`). `write_buffer`/`read_buffer` perform no allocation and no Vulkan call except flush/invalidate on non-coherent memory. `upload_buffer` allocates one staging buffer per call (known cost; a staging ring is deferred until profiling shows it matters, ADR-0010 §6). Stress: 100k create/destroy cycles stay validation-clean and `reserved_bytes` stays flat after warm-up (reported, no fixed number).

## 12. Testing requirements

Unit tests (no GPU, names are `#[test]` names):

| Test | Checks |
|---|---|
| `buffer_usage_ops` | `|`, `contains`, `is_empty`, `NONE` |
| `usage_to_vk_flags` | table 6.1 incl. always-on device address |
| `memory_to_location` | table 6.1 |
| `range_check_overflow` | `offset + len` overflow and `== size` boundary |
| `index_alignment_check` | u16 offset 2 ok, 1 bad; u32 offset 2 bad |
| `buffer_registry_stale_after_remove` | generation bump (Registry) |
| `alloc_accounting_arith` | counter add/sub on a fake allocation size |
| `pending_uploads_drain` | queue 2 uploads outside a frame -> next submit drains both into `upload_cb`; empty queue -> no `upload_cb` in the submit |
| `vk_buffer_is_send_sync` | compile-time assertion |
| `generated_names` | unlabeled buffer -> `buffer#iv g` |

Integration (`ANVIL_GPU_TESTS=1`, lavapipe, validation `On`, sync validation on; every test ends with `diagnostics().validation.errors == 0`, warnings 0):

| Test | Checks |
|---|---|
| `buffer_write_read_roundtrip_exact` | `Upload` write 4 KiB pattern -> `copy_buffer_to_buffer` into `Readback` -> submit_unwindowed -> `wait_idle` -> `read_buffer` byte-exact |
| `buffer_upload_gpu_roundtrip` | `upload_buffer` into `GpuOnly` (`TRANSFER_DST\|TRANSFER_SRC`) (before or during the frame) -> `copy_buffer_to_buffer` to `Readback` in the same frame -> submit -> `wait_idle` -> bytes equal (proves leading/trailing barriers and single submit) |
| `buffer_upload_offsets` | two uploads at disjoint offsets |
| `buffer_address_nonzero_stable` | address != 0, equal across calls, two buffers differ |
| `buffer_stale_handle` | after `destroy_buffer`: `buffer_address`, `write_buffer`, `upload_buffer` -> `StaleHandle`; reuse after retirement still rejects the old generation |
| `buffer_destroy_deferred` | destroy in frame N after a submit referencing it: `pending_destroys > 0` until `gpu_completed_frame` passes the tag, then 0 |
| `buffer_invalid_args` | zero size, empty usage, write to `GpuOnly`, range overflow, index misalignment -> `InvalidDescriptor` |
| `buffer_stress_create_destroy_100k` | 100k create/destroy with frames interleaved, validation clean; `diagnostics().live.buffers == 0` at end |
| `buffer_leak_report_at_shutdown` | undestroyed buffer -> exactly one leak `warn`, no validation error |
| `buffer_memory_diagnostics` | `allocated_bytes` rises on create, falls only after retirement; `reserved_bytes >= allocated_bytes` |
| `abandoned_frame_keeps_uploads` | upload, begin_frame twice without submit, then submit -> data arrives, no validation error |

## 13. Acceptance criteria

- [ ] `buffer_write_read_roundtrip_exact` and `buffer_upload_gpu_roundtrip` pass with zero validation errors and zero warnings.
- [ ] Stale handles return `StaleHandle`, never reach Vulkan.
- [ ] No buffer or staging allocation is freed before the timeline passes its tag (validation clean under the stress test).
- [ ] `diagnostics().memory` and `live.buffers` match creates/destroys (test `buffer_memory_diagnostics`).
- [ ] Every create/destroy/upload is traced per ADR-0009 and every buffer is named.
- [ ] Zero leaks at shutdown for the `textured-quad` example.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RHI-002, FR-RES-008, FR-RES-010 | create/write/upload, gpu-allocator suballocation, per-call staging (ring deferred) |
| FR-RHI-014, FR-VK-009 | `gpu-allocator` (ADR-0007) |
| FR-RHI-015, FR-RES-006 | `Doomed::Buffer` deferred destruction |
| FR-RES-004, FR-RES-007 | central pool, `MemoryDiagnostics` |
| NFR-REL-003 | leak report and tests |
| ADR-0009 | tracing/naming/diagnostics below |

### Observability (ADR-0009)

| Operation | Level | Fields / name |
|---|---|---|
| `create_buffer` | `debug` | `handle, label, size, usage, memory, address` (target `rhi_vulkan::buffer`) |
| `destroy_buffer` | `debug` (`warn` if stale) | `handle` |
| deferred retirement | `debug` (existing `deferred destroy retired`) | `kind = "buffer"` (`"staging"` for staging buffers), `completed` |
| `write_buffer`/`read_buffer`/`upload_buffer` | `trace` | `handle, offset, bytes` (inside the `frame` span for uploads) |
| submit with uploads | `trace` (existing submit event) | adds `uploads, upload_bytes` |
| leaked buffer at shutdown | `warn` | `handle, label` |
| allocator failures | `warn` | `variant, size, memory` before returning the error |

Names: `set_name(buffer, label)`; unlabeled `buffer#<i>v<g>`; staging `staging(<dst label>)`. Diagnostics: `live.buffers`, `memory.*` (LAYER-003 §17.5); `pending_destroys` includes staging.

## 15. Open questions

None blocking. Resolved by ADR-0010:

1. Last-use tracking: conservative global value (ADR-0010 §8).
2. Suballocation: gpu-allocator only; no ring (ADR-0010 §6; the M1 draft's `UploadRing` is dropped).
3. Transfer queue: graphics queue in M2, transfer queue M5 (ADR-0010 §7).
4. `bufferDeviceAddress` is enabled with the first bindless design and all buffers get device-address usage (ADR-0010 §2; MODULE-002 amendment).
5. Buffer barriers: implicit per upload, no `BufferState` (ADR-0010 §9).
6. gpu-allocator 0.28: feature `vulkan`; API names confirmed in the crate source (`Allocator::allocate/free/capacity/generate_report`, `AllocationCreateDesc`, `AllocatorCreateDesc.buffer_device_address`, `Allocation::mapped_ptr/mapped_slice_mut`). Remaining verify: `Allocator::capacity()` cost for `reserved_bytes` (called from `diagnostics()`).
