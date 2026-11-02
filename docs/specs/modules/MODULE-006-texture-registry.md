# MODULE-006: Texture Registry

| Field | Value |
|---|---|
| Status | Draft |
| Crate | rhi-vulkan (descriptors and trait methods in rhi) |
| Layer | LAYER-007 |
| Milestone | M2 |
| Traces to | FR-RHI-003, FR-RHI-004, FR-RHI-005, FR-RES-001, FR-RES-002, FR-RES-003, FR-RES-005, FR-RES-006, FR-RES-009, FR-VK-008, NFR-REL-003; MASTER-000 §10.1-10.3; ADR-0006, ADR-0007 |
| Last updated | 2026-11-02 |

## 1. Purpose

Central registry of GPU images, image views and samplers behind `TextureHandle`/`SamplerHandle`: creation, upload, per-resource state tracking, deferred destruction. It generalises the M1 texture pool (which holds swapchain images only, MODULE-003).

## 2. Responsibilities

- `create_texture` / `destroy_texture`, texture upload through staging, sampler creation.
- Pool entries `{ VkImage, VkImageView(s), Allocation?, extent, format, mips, layers, usage, state, owner }`; `TextureOwner::Swapchain` entries stay owned by the swapchain.
- Track `TextureState` per texture (FR-RES-005) so barrier calls can validate `from` against the tracked state (debug builds) and the render graph can import accurate initial states.
- Stale-handle and slot-reuse protection (FR-RES-002, FR-RES-003) via `Pool<T>` generations.
- Deferred destruction of image, views and allocation (extends `Doomed`).
- Sampler objects (dedupe cache keyed by descriptor, undecided).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Buffers | MODULE-005 |
| Image decoding, KTX2/Basis transcoding, streaming policy | assets (LAYER-009), M5 |
| Automatic barrier placement | render graph (MODULE-009) |
| Descriptor/binding of textures into shaders | binding model (undecided) |
| Swapchain image lifetime | MODULE-003 |

## 4. Dependencies

Allowed: foundation, rhi, MODULE-002 (allocator, pools), MODULE-004 (deferred queue, barrier table), MODULE-005 (staging ring), `ash`, `gpu-allocator`. Forbidden: layer 3+ crates.

## 5. Public API

Concept (LAYER-007 §8; extended in M2):

```rust
pub struct TextureDesc<'a> { pub extent: (u32, u32, u32), pub format: Format, pub mips: u32, pub layers: u32, pub usage: TextureUsage, pub label: Option<&'a str> }
bitflags! { pub struct TextureUsage: u32 { SAMPLED, COLOR_ATTACHMENT, DEPTH_ATTACHMENT, TRANSFER_SRC, TRANSFER_DST, STORAGE } }
pub struct SamplerDesc { /* filter, address, anisotropy, compare: undecided */ }
fn create_texture(&self, desc: &TextureDesc) -> Result<TextureHandle, RhiError>;
fn destroy_texture(&self, h: TextureHandle);
fn create_sampler(&self, desc: &SamplerDesc) -> Result<SamplerHandle, RhiError>;
fn destroy_sampler(&self, h: SamplerHandle);
```

`TextureState` (LAYER-003 §7.3) may grow (`DepthAttachment`, `DepthRead`, `General`) with this module.

## 6. Data structures

`VkTexture` (already declared in MODULE-003: `image, view, extent, format, owner`) gains `mips`, `layers`, `allocation`, `state`. Subresource ranges (per mip/layer states) are not modelled in M2 unless the render graph demands them (LAYER-006 Q1).

## 7. Ownership and lifetime

Pool owns entries; handles are keys. `destroy_texture`: remove (stale now), `Doomed::Image { image, views, allocation }` tagged `last_submitted + 1`. Destroying a `TextureOwner::Swapchain` handle through this API is `InvalidDescriptor`. Views are destroyed through the same deferred path as the image (ADR-0006).

## 8. Execution flow

`create_texture`: validate (non-zero extent, format supported for usage via `get_physical_device_format_properties`, mips <= log2) -> create image -> allocate (dedicated for render targets, ADR-0007) -> bind -> create default view -> insert -> initial state `Undefined`. Upload: staging ring -> `Undefined -> TransferDst` barrier -> `cmd_copy_buffer_to_image` -> `TransferDst -> ShaderRead` (all through MODULE-004 barrier translation).

## 9. Threading model

As MODULE-005: `&self` creation with locked pools; uploads recorded on the frame encoder in M2; transfer queue M5.

## 10. Error handling

`StaleHandle{texture}`, `InvalidDescriptor`, `Unsupported` (format/usage combination), `OutOfDeviceMemory`. A barrier whose `from` disagrees with the tracked state is a recorded error in debug builds (undecided: always vs debug only).

## 11. Performance requirements

Measure texture upload time and GPU memory by category (NFR-PERF-004, NFR-PERF-008); registry lookup cost shows up in the profiler before any redesign (ADR-0006 revisit condition). No numeric targets.

## 12. Testing requirements

Unit: generation reuse (FR-RES-003), stale lookup, state tracking transitions, format/usage validation tables, mip count math. Integration (lavapipe): create/upload/readback round trip; sampler creation; destroy while referenced by an in-flight submission; swapchain-owned handle rejected by `destroy_texture`; leak count zero.

## 13. Acceptance criteria (Draft, M2)

- [ ] Texture round trip (upload, sample or copy back) is exact for `Rgba8Unorm`.
- [ ] Stale and reused-slot handles are rejected (unit tests).
- [ ] No image or view destroyed while referenced by a submitted command buffer (validation clean under stress).
- [ ] Tracked state matches emitted barriers in the textured example.
- [ ] Zero leaks at shutdown.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RHI-003, FR-RHI-004, FR-RHI-005, FR-RES-009 | images, views, samplers |
| FR-RES-001, FR-RES-002, FR-RES-003 | `Handle<T>` / `Pool<T>` (LAYER-001) |
| FR-RES-005 | tracked `TextureState` |
| FR-RES-006 | deferred destruction |
| FR-VK-008 | barrier translation reuse (MODULE-004) |
| NFR-REL-003 | leak tests |

## 15. Open questions

1. Image views: one default view per texture, or an explicit `TextureViewHandle` (FR-RHI-004 lists views separately). Decide with the binding model.
2. Per-subresource state (mip/layer) vs whole-texture state (LAYER-006 Q1).
3. Sampler dedupe cache: needed in M2?
4. Which formats and usages M2 supports (depth formats, sRGB views, block-compressed for KTX2 in M5).
5. Whether tracked state is authoritative (backend validates `from`) or advisory (render graph owns truth, RESEARCH-001 open question 4).
6. Texture streaming interfaces (sparse/virtual) are M5 (LAYER-007 Q5).
