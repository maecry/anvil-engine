# MODULE-005: Buffer Allocator

| Field | Value |
|---|---|
| Status | Draft |
| Crate | rhi-vulkan (descriptors and trait methods in rhi) |
| Layer | LAYER-007 |
| Milestone | M2 |
| Traces to | FR-RHI-002, FR-RHI-014, FR-RHI-015, FR-VK-009, FR-RES-004, FR-RES-006, FR-RES-007, FR-RES-008, FR-RES-010, NFR-REL-003, NFR-ARCH-007; MASTER-000 §10.1-10.3; ADR-0006, ADR-0007; RESEARCH-002 |
| Last updated | 2026-11-02 |

## 1. Purpose

Create, upload to, track and destroy GPU buffers through typed `BufferHandle`s, with memory obtained from `gpu-allocator` (created in M1, MODULE-002) and destruction deferred on the timeline value (MODULE-004). Concept level: the binding model and last-use tracking are undecided.

## 2. Responsibilities

- `create_buffer` / `destroy_buffer` / `write_buffer` and `memory_snapshot` (LAYER-007 §8).
- Pool of buffer entries `{ VkBuffer, Allocation, size, usage, location, label }` with generation checks.
- Memory categories for statistics: device-local, host-visible upload, readback, transient (LAYER-007 §7).
- Staging path chosen by the backend: host-visible memory is written directly; device-local buffers are filled through a staging ring and a transfer command, followed by the required barrier.
- Per-frame upload ring: a linear allocator on persistently mapped host-visible memory (ADR-0007), reset per frame slot.
- Deferred destruction of the buffer and its allocation (extends `Doomed`).
- Leak reporting at shutdown.

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Images, views, samplers | MODULE-006 |
| Descriptor sets / bindless tables | binding model (open, LAYER-007 Q1) |
| Residency and streaming policy | LAYER-009 / M5 |
| Render-graph transient aliasing placement | MODULE-009 (M4), may need allocator support |
| Format conversion, asset decoding | assets |

## 4. Dependencies

Allowed: foundation (`Pool`, `Handle`, `MemorySnapshot`), rhi, MODULE-002 (`DeviceShared`, allocator), MODULE-004 (`SyncShared.deferred`, submit for transfers), `ash`, `gpu-allocator`. Forbidden: any layer 3+ crate; `gpu-allocator` types in public signatures.

## 5. Public API

Concept sketch (exact signatures fixed when the module is promoted to Review; LAYER-007 §8):

```rust
// rhi
pub struct BufferDesc<'a> { pub size: u64, pub usage: BufferUsage, pub memory: MemoryLocation, pub label: Option<&'a str> }
bitflags! { pub struct BufferUsage: u32 { VERTEX, INDEX, UNIFORM, STORAGE, TRANSFER_SRC, TRANSFER_DST, INDIRECT } }   // INDIRECT arrives with M5
pub enum MemoryLocation { DeviceLocal, HostUpload, HostReadback }
fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferHandle, RhiError>;
fn write_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError>;
fn destroy_buffer(&self, h: BufferHandle);
fn memory_snapshot(&self) -> MemorySnapshot;
```

## 6. Data structures

`VkBuffer { raw, allocation: Option<Allocation>, size, usage, location, last_use: u64 /* conservative: see Open questions */ }`; `UploadRing { buffer, mapped_ptr, capacity, head, per-slot high-water marks }`; `MemoryStats { bytes and count per category }`. Dedicated allocations for large buffers; suballocation left to `gpu-allocator`.

## 7. Ownership and lifetime

Backend pool owns every buffer; handles are keys (ADR-0006). `destroy_buffer`: remove from pool (stale now), push `Doomed::Buffer { buffer, allocation }` tagged `last_submitted + 1`; freed when the timeline passes the tag. Allocator memory is released only on retirement (ADR-0006). Shutdown drains after wait idle and logs leaks. `unsafe`: mapped-pointer writes (bounds checked against `size`, no overlap with GPU-in-flight ring regions), `bind_buffer_memory` ownership by the allocator.

## 8. Execution flow

`create_buffer`: validate (size > 0, usage not empty) -> `create_buffer` -> query requirements -> allocate via `gpu-allocator` with `MemoryLocation` mapping -> bind -> insert in pool -> stats update.
`write_buffer` (host-visible): bounds check -> `memcpy` into mapped memory (flush if not coherent).
`write_buffer` (device-local): copy into the upload ring (stall or grow if full) -> record `cmd_copy_buffer` in the transfer scope -> buffer barrier to the first use. Where this command is recorded (frame encoder vs dedicated transfer queue) is undecided.
`destroy_buffer`: see section 7.

## 9. Threading model

Creation, writes and destroy take `&self` (LAYER-003 §10): pools and the allocator sit behind locks so worker-thread uploads can be added without a trait change. The upload ring is per frame slot and single-writer in M2. A dedicated transfer queue is M5.

## 10. Error handling

`OutOfDeviceMemory` / `OutOfHostMemory` from allocation; `InvalidDescriptor` (zero size, write out of range, usage mismatch); `StaleHandle{buffer}`. Allocation failure never panics (LAYER-007 §9-12). Allocator errors map as in MODULE-002 section 10.

## 11. Performance requirements

Measure, do not promise (NFR-PERF-012): upload time per frame (NFR-PERF-008), allocator overhead, bytes per category (NFR-PERF-004). Steady-state uploads allocate no host memory. Stress target: repeated create/destroy of 100k buffers stays validation-clean and flat in memory (LAYER-007 §13).

## 12. Testing requirements

Unit (no GPU): pool generation behaviour with buffer entries; ring allocator wraparound and per-slot high-water logic; stats arithmetic; deferred tag selection. Integration (lavapipe): write then read back through a readback buffer; 100k create/destroy stress with validation and sync validation; deferred destroy while a submission still references the buffer; leak count zero after shutdown.

## 13. Acceptance criteria (Draft, M2)

- [ ] Stale handle use returns `StaleHandle`, never UB.
- [ ] No buffer is destroyed while a submitted command buffer references it (validation clean under stress).
- [ ] Write/readback round trip is byte-exact.
- [ ] `memory_snapshot` totals match the allocator's own report within the documented categories.
- [ ] Zero leaks reported at shutdown for the textured-cube example.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RHI-002, FR-RES-008, FR-RES-010 | create/write, suballocation, staging ring |
| FR-RHI-014, FR-VK-009 | `gpu-allocator` (ADR-0007) |
| FR-RHI-015, FR-RES-006 | deferred destruction |
| FR-RES-004, FR-RES-007 | central pool, `memory_snapshot` |
| NFR-REL-003 | leak report and tests |

## 15. Open questions

1. Last-use tracking: conservative global value vs per-buffer (LAYER-007 Q3).
2. Suballocation: rely on `gpu-allocator` only, or add a linear/ring allocator beyond the upload ring (LAYER-007 Q2).
3. Transfer queue for uploads in M2 or M5 (LAYER-007 Q4; RESEARCH-001).
4. Whether `buffer_device_address` is enabled with the first bindless design (LAYER-004 Q2).
5. Buffer barriers: expressed through a `BufferState` mirroring `TextureState`, or implicit per upload (ties to the render graph state model, MODULE-009).
6. `gpu-allocator` 0.28 exact feature flags and statistics API (verify at cargo add).
