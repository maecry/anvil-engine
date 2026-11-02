# LAYER-007: GPU Resource Management

| Field | Value |
|---|---|
| Status | Draft |
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
| Staging and upload scheduling | Material parameter layout (material, LAYER-008) |
| Deferred destruction keyed by timeline value (defined in LAYER-003 §7.2) | |
| Memory statistics (`MemorySnapshot`) and leak reporting | |
| Binding model (descriptor sets vs bindless) | |

## 3. Crates

rhi (descriptors, handle tags, traits), rhi-vulkan (pools, allocator, uploads). No new crate before M2 shows a need.

## 4. Responsibilities / 5. Non-responsibilities

- Responsible: central ownership in backend pools; stale-handle detection; deferred destruction; suballocation policy; staging ring; per-resource state for barriers; stats.
- Not responsible: choosing which assets are resident; format conversion; the render graph's lifetime planning.

## 6. Dependencies

As LAYER-003 and LAYER-004. `gpu-allocator` stays inside rhi-vulkan.

## 7. Key concepts and data flow

```
 create_buffer(desc) -> BufferHandle --> pool slot { VkBuffer, Allocation, state, last_used_value }
 upload(buffer, bytes) -> staging ring (host-visible) -> transfer cmd -> barrier -> ready
 destroy_buffer(h)   -> generation bump -> deferred queue(value) -> freed when GPU passes value
```

- Last-use tracking: conservative initially (the most recent submitted timeline value at destroy time); per-resource tracking is an optimisation if measurements justify it.
- Memory categories for stats: device-local, host-visible upload, readback, transient.

## 8. Public interface sketch (concept, M2)

```rust
pub struct BufferDesc<'a> { pub size: u64, pub usage: BufferUsage, pub memory: MemoryLocation, pub label: Option<&'a str> }
pub struct TextureDesc<'a> { pub extent: (u32,u32,u32), pub format: Format, pub mips: u32, pub layers: u32, pub usage: TextureUsage, pub label: Option<&'a str> }
fn create_buffer(&self, desc: &BufferDesc) -> Result<BufferHandle, RhiError>;
fn create_texture(&self, desc: &TextureDesc) -> Result<TextureHandle, RhiError>;
fn write_buffer(&self, h: BufferHandle, offset: u64, data: &[u8]) -> Result<(), RhiError>;   // staging path chosen by backend
fn memory_snapshot(&self) -> MemorySnapshot;
```

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: backend pools own; handles are keys (ADR-0006). Optional RAII wrappers belong to higher layers.
- Threading: creation/upload may be called from worker threads once pools have interior locking; transfer queue use is M5.
- Errors: `OutOfDeviceMemory` and `StaleHandle` as in LAYER-003; allocation failure never panics.
- Diagnostics: live counts and bytes per category, leak list at shutdown, labels as debug names.

## 13. Testing strategy

Handle/pool unit tests (LAYER-001); stress test: repeated create/destroy of 100k buffers with validation on; deferred destruction with artificially lagging GPU; upload round-trip (write then readback) on a GPU runner.

## 14. Acceptance criteria (Draft, M2)

- [ ] No resource destroyed while a submitted command buffer references it (validation clean under stress).
- [ ] Stale handle use returns `StaleHandle`, never UB.
- [ ] Shutdown reports zero leaks for the textured-cube example.

## 15. Open questions

1. Binding model: classic descriptor sets, descriptor indexing/bindless, or push descriptors. Needs an ADR before M2.
2. Suballocation: rely on `gpu-allocator` only, or add a linear/ring allocator for per-frame data?
3. Per-resource last-use tracking vs conservative global value.
4. Transfer queue use for uploads (M2 or M5).
5. Sparse/virtual textures and texture streaming interfaces (M5).

## 16. Related modules

MODULE-005-buffer-allocator, MODULE-006-texture-registry.
