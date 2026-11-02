# ADR-0010: Bindless binding model and M2 resource decisions

- Status: Accepted
- Date: 2026-11-02

## Context
M2 introduces buffers, textures, samplers and shader resource binding. LAYER-007 Q1 requires a binding-model ADR before M2. MODULE-005 and MODULE-006 list open questions that block implementation. The baseline is Vulkan 1.3 (ADR-0004), where descriptor indexing and buffer device address are core features. RADV and lavapipe both support them.

## Problem
How do shaders reach resources, and which of the open M2 resource questions do we settle now?

## Considered alternatives
1. Classic descriptor sets with per-material layouts (bind groups in the RHI).
2. Push descriptors.
3. Bindless: one global descriptor set with descriptor indexing, buffers via buffer device address (BDA), and per-draw data in push constants.

## Decision
Option 3. It is the smallest RHI surface, with no bind-group or layout objects and a single pipeline layout. It is the modern Vulkan path, and the render graph can treat everything as plain indices.

1. **Global descriptor set (set 0)**, created once per device, with flags `UPDATE_AFTER_BIND | PARTIALLY_BOUND`:
   - binding 0: `SAMPLED_IMAGE[]`, capacity `min(65536, device limit)`
   - binding 1: `SAMPLER[]`, capacity `min(1024, device limit)`
   - Storage images arrive when compute needs them (M2 does not).
2. **Buffers are reached through BDA.** `Device::buffer_address(BufferHandle) -> Result<u64>`. Shaders use `GL_EXT_buffer_reference`. All buffers get `SHADER_DEVICE_ADDRESS` usage.
3. **One pipeline layout** for every pipeline: set 0 plus 128 bytes of push constants, visible to all graphics stages. `CommandEncoder::push_constants(offset, &[u8])` writes them.
4. **Bindless indices**:
   - `Device::texture_index(TextureHandle)` and `Device::sampler_index(SamplerHandle)` return a `u32`, the slot in the global array.
   - The slot is the handle's pool index.
   - The descriptor is written at creation.
   - A slot is reused only after deferred destruction retires the old object, so a stale index can never alias a live object while the GPU might still read it.
5. **No vertex input state.** Shaders pull vertices through BDA. Index buffers are bound classically: `bind_index_buffer(BufferHandle, offset, IndexFormat::{U16,U32})` and `draw_indexed`.
6. **Memory:**
   - `BufferDesc.memory: Memory::{GpuOnly, Upload, Readback}` maps onto gpu-allocator `MemoryLocation`.
   - `Upload` and `Readback` buffers are persistently mapped: `Device::write_buffer(h, offset, &[u8])` and `read_buffer(h, offset, &mut [u8])` copy directly.
   - Suballocation is gpu-allocator's only (Q2). There is no custom ring yet.
7. **Uploads to GPU-only memory** go through `Device::upload_buffer(h, offset, &[u8])` and `upload_texture(h, &[u8])` (mip 0, layer 0, tightly packed).
   - The backend copies into a staging buffer and records the copy into a per-frame upload command buffer.
   - That buffer is submitted ahead of the frame's commands in the same `vkQueueSubmit2`, followed by a global memory barrier to all shader reads.
   - The staging buffer is destroyed through deferred destruction.
   - Uploads run on the graphics queue in M2. The transfer queue moves to M5 (Q4).
   - Textures end in `ShaderRead` state.
8. **Lifetime:** every resource uses conservative global last-use tracking (Q3). Destruction is keyed on the timeline value of the latest submitted frame.
9. **State tracking:**
   - Texture state is whole-texture (no per-mip state) and advisory: the caller passes `from`/`to` as in M1. The render graph becomes the owner of state in M3.
   - There is no `BufferState` in M2; upload barriers are implicit.
10. **Views:** one default view per texture, and no `TextureViewHandle` until a feature needs one.
11. **Samplers** are created explicitly, with no dedupe cache.
12. **M2 formats:** the M1 set plus `R8Unorm`, `Rg8Unorm`, `Rgba16Float` and `D32Sfloat`. Depth attachments are supported in pipelines and rendering.
13. **Shortcuts that stay:** swapchain recreation still waits for idle. Retiring the old swapchain without a stall needs present-completion tracking (`VK_EXT_swapchain_maintenance1`) and is deferred until it shows up in a profile.

## Advantages
- One layout and no per-draw descriptor updates.
- A tiny RHI surface: indices and addresses are plain `u32`/`u64` values.
- The render graph (M3) only needs to pass indices.

## Disadvantages
- Every shader depends on `GL_EXT_buffer_reference` and `nonuniformEXT` for divergent texture indices.
- Bugs show up as wrong indices rather than API errors. Mitigations:
  - `diagnostics()` lists bindless slot usage.
  - Validation with GPU-assisted validation is opt-in through `ANVIL_GPU_AV=1`.

## Consequences
- LAYER-003 gains the M2 API listed above. LAYER-007, MODULE-005, MODULE-006 and MODULE-007 (pipeline layout) are updated to this ADR, and their open questions are marked resolved.
- `DeviceDiagnostics` gains buffer, texture and sampler counts, GPU memory usage (allocated and reserved bytes) and bindless slot high-water marks (ADR-0009).
- MODULE-008 (shader compiler crate) is not part of M2. Examples keep compiling GLSL in `build.rs` with shaderc.

## Revisit conditions
- A target device lacks descriptor indexing or BDA, which is not expected for Vulkan 1.3.
- Per-draw data outgrows 128 bytes of push constants. Then add a per-frame BDA uniform ring.
