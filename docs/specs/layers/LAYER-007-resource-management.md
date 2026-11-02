# LAYER-007: GPU Resource Management

| Field | Value |
|---|---|
| Status | Accepted |
| Milestone | M2 |
| Crates | rhi, rhi-vulkan (cross-cutting); material later |
| Traces to | MASTER-000 §4.2, §10, §22.6; ADR-0006; ADR-0007 |
| Last updated | 2026-11-02 |

## 1. Purpose

Specify how GPU buffers, images, samplers and their memory are created, tracked, uploaded and destroyed. M1 lays the groundwork (handles, pools, deferred destruction, allocator creation); M2 delivers the full resource API. This spec spans rhi (API) and rhi-vulkan (implementation), so it is a cross-cutting layer spec.

## 2. Scope

| In | Out |
|---|---|
| Buffer/texture/sampler creation and destruction via handles | Asset decoding, streaming policy (LAYER-009) |
| Memory via `gpu-allocator` (ADR-0007): device-local, host-visible, staging | Render-graph transient aliasing policy (LAYER-006) |
| Staging (per-call) and per-frame upload command buffer | Material parameter layout (material, LAYER-008) |
| Deferred destruction keyed by timeline value (defined in LAYER-003 §7.2) | |
| Memory statistics (`DeviceDiagnostics.memory`) and leak reporting | |
| Binding model: bindless (ADR-0010; global set owned by MODULE-007) | |

## 3. Crates

rhi (descriptors, handle tags, traits), rhi-vulkan (pools, allocator, uploads). No new crate before M2 shows a need.

## 4. Responsibilities / 5. Non-responsibilities

- Responsible: central ownership in backend pools; stale-handle detection; deferred destruction; suballocation policy (gpu-allocator); per-call staging and the per-frame upload command buffer (no ring in M2); advisory whole-texture state; bindless slot management; stats (`DeviceDiagnostics`).
- Not responsible: choosing which assets are resident; format conversion; the render graph's lifetime planning.

## 6. Dependencies

As LAYER-003 and LAYER-004. `gpu-allocator` stays inside rhi-vulkan.

## 7. Key concepts and data flow (ADR-0010)

```
 create_buffer(desc)  -> BufferHandle  --> pool slot { VkBuffer, Allocation, address, mapped? }
 write_buffer / read_buffer            --> direct copy into persistently mapped Upload/Readback memory
 upload_buffer / upload_texture        --> staging buffer -> per-frame upload cmd buffer -> same vkQueueSubmit2, before frame cmds
                                           (leading + trailing global barrier; textures end in ShaderRead)
 create_texture / create_sampler       --> pool slot + bindless descriptor written at creation (slot = handle index)
 destroy_*(h)                          --> handle stale now -> Doomed(tag = last_submitted + 1) -> freed when the timeline passes the tag
                                           (bindless slot index released only then)
 shaders: textures[texture_index], samplers[sampler_index], buffers via buffer_reference + 128 B push constants
```

- Last-use tracking: conservative global value (the next submit's timeline value at destroy time). Per-resource tracking is an optimisation if measurements justify it (ADR-0010 §8).
- Memory categories (diagnostics): `allocated_bytes` and `reserved_bytes` from the allocator, plus per-kind live counts (ADR-0010 Consequences). Per-category bytes (device-local vs upload vs readback) are not reported in M2.
- State: whole-texture, advisory; no buffer state (ADR-0010 §9).

## 8. Public interface (M2)

The authoritative signatures are in LAYER-003 §17 (types `BufferDesc`, `TextureDesc`, `SamplerDesc`, `Memory`, `BufferUsage`, `TextureUsage`, `IndexFormat`; `Device::{create,destroy}_{buffer,texture,sampler}`, `buffer_address`, `texture_index`, `sampler_index`, `write_buffer`, `read_buffer`, `upload_buffer`, `upload_texture`; encoder `push_constants`, `bind_index_buffer`, `draw_indexed`, `copy_buffer_to_buffer`, `copy_texture_to_buffer`). Implementation: MODULE-005 (buffers, staging/upload), MODULE-006 (textures, samplers, bindless slots), MODULE-007 (global descriptor set and pipeline layout). `memory_snapshot()` is dropped from M2 in favor of `DeviceDiagnostics.memory`.

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: backend pools own; handles are keys (ADR-0006). Optional RAII wrappers belong to higher layers.
- Threading: creation/upload may be called from worker threads once pools have interior locking; transfer queue use is M5.
- Errors: `OutOfDeviceMemory` and `StaleHandle` as in LAYER-003; allocation failure never panics.
- Diagnostics: live counts and bytes per category, leak list at shutdown, labels as debug names.

## 13. Testing strategy

Handle/pool unit tests (LAYER-001), as itemized in MODULE-005/006/007 §12; stress test: repeated create/destroy of 100k buffers with validation on; deferred destruction with artificially lagging GPU; upload round-trip (write then readback) on a GPU runner.

## 14. Acceptance criteria (M2)

- [ ] No resource destroyed while a submitted command buffer references it (validation clean under stress).
- [ ] Stale handle use returns `StaleHandle`, never UB.
- [ ] Shutdown reports zero leaks for the `textured-quad` example (LAYER-003 §17.7).

## 15. Open questions

Resolved by ADR-0010:

1. Binding model: bindless (one global descriptor set with descriptor indexing, buffers through BDA, 128 B push constants). Owner of the set and layout: MODULE-007.
2. Suballocation: gpu-allocator only; no custom ring yet (ADR-0010 §6).
3. Last-use tracking: conservative global value (ADR-0010 §8).
4. Transfer queue: graphics queue in M2, transfer queue M5 (ADR-0010 §7).

Still open (M5): sparse/virtual textures and texture streaming interfaces.

## 16. Related modules

MODULE-005-buffer-allocator, MODULE-006-texture-registry.

## 17. M5b: the mesh arena (ADR-0013 §6)

All meshes of a renderer live in one shared vertex buffer and one shared index buffer (`crates/renderer/src/arena.rs`), suballocated, so one `bind_index_buffer` and one vertex buffer address (a push constant) serve every draw of every pass, and the indirect commands of GPU culling can address any mesh.

- **Layout.** `Arena` owns two `GpuOnly` buffers (`arena.vertices`: `STORAGE`, read through BDA; `arena.indices`: `INDEX`), capacity 256 Ki vertices (12 MiB) and 1 Mi 32-bit indices (4 MiB), and two `RangeAlloc`s (first fit over a sorted free list, neighbours coalesce on free; unit-tested for fragmentation). A mesh gets one vertex range and one index range holding all its levels of detail back to back; its indices are mesh-local, so a draw adds the mesh's first vertex as the base vertex (`vertex_offset` of the indirect command; `gl_VertexIndex` includes it).
- **Mesh record.** `Renderer::create_mesh` fills a `GpuMesh` (80 bytes: object-space bounding sphere, per-level `first_index` and `index_count`, per-level geometric error, `vertex_offset`, level count) and uploads vertices, indices and the record into a fixed `GpuOnly` mesh table (4096 records, indexed by the mesh handle index) with `Device::upload_buffer`, which orders them before the frame's commands. The CPU path and the transparent pass use level 0 of the same record. `destroy_mesh` returns the ranges to the arena and invalidates the scene cache (instances that still reference the mesh are skipped with a warning).
- **Limits (shortcuts).** Fixed capacities: `create_mesh` fails with `InvalidDescriptor` when the arena or the table is full; growing means allocating a bigger pair and re-uploading (M6 asset streaming). Meshes are never moved or compacted. Vertex format and index width are fixed (48-byte vertices, `u32`).
- **Levels of detail.** `MeshData::lods` (finest first, up to `MAX_LODS - 1` = 3 coarser levels, each with an object-space error) is used when given; otherwise `generate_lods` (`meshopt::simplify`, absolute error, halving the triangle count per level, stop when a level saves less than 10 %, none below 64 triangles) fills it. Test `generated_lods_shrink_with_growing_error`.
