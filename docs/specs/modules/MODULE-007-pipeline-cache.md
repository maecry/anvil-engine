# MODULE-007: Pipeline Cache

| Field | Value |
|---|---|
| Status | Draft |
| Crate | rhi-vulkan (pipeline objects, `VkPipelineCache`); key hashing helpers shared with shader |
| Layer | LAYER-008 (cache policy) / LAYER-007 (GPU object) |
| Milestone | M2 (pipeline creation), M5 (VkPipelineCache persistence) |
| Traces to | FR-RHI-007, FR-RHI-008, FR-RHI-009, FR-VK-011, FR-SHD-004, FR-SHD-005, NFR-PERF-009, NFR-REL-004; MASTER-000 §8.4, §8.7, §14 M5; ADR-0005 |
| Last updated | 2026-11-02 |

## 1. Purpose

Own pipeline objects (graphics and compute), pipeline layouts, and the Vulkan `VkPipelineCache`: deduplicate identical pipeline requests, create pipelines through the cache, and persist the cache blob between runs. M1 creates an empty `VkPipelineCache` and a minimal graphics pipeline inside MODULE-004 and MODULE-002; this module takes over those objects in M2 and adds persistence in M5.

## 2. Responsibilities

- `create_graphics_pipeline`, `create_compute_pipeline`, pipeline layouts derived from the chosen binding model, `destroy_*` with deferred destruction.
- Dedupe: a hash of the full pipeline descriptor (shader module identity, formats, state, layout) maps to an existing `PipelineHandle` or creates one. Whether handles are reference-counted or the cache returns shared handles is undecided.
- `VkPipelineCache` lifecycle: create (empty or from a validated blob), pass to every pipeline create call, serialize at shutdown or periodically.
- Blob validation on load: header version, `vendorID`, `deviceID`, `pipelineCacheUUID` must match the current device; mismatch or corruption means start empty (never an error).
- Pipeline compatibility checks between shader reflection and layout (FR-SHD-004) where reflection exists (MODULE-008).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Compiling shader source to SPIR-V | MODULE-008 |
| Choosing the shader permutation cache key | shader (LAYER-008), though both caches share a hashing helper |
| Binding model decisions | LAYER-007 Q1 |
| Hot-reload orchestration | renderer / shader (M6) |
| Where the cache file lives (path policy) | foundation/platform configuration (open) |

## 4. Dependencies

Allowed: foundation, rhi, MODULE-002, MODULE-004 (`Doomed`), `ash`, a hasher (std `DefaultHasher` is not stable across versions: a fixed, versioned hash is required, crate undecided). File IO for the blob goes through a path the application provides in `DeviceDesc`/config (platform filesystem access, FR-PLAT-006). Forbidden: layer 3+ crates.

## 5. Public API

Concept:

```rust
// rhi (grows from M1's GraphicsPipelineDesc)
fn create_graphics_pipeline(&self, desc: &GraphicsPipelineDesc) -> Result<PipelineHandle, RhiError>;
fn create_compute_pipeline(&self, desc: &ComputePipelineDesc) -> Result<PipelineHandle, RhiError>;
fn destroy_pipeline(&self, h: PipelineHandle);
// M5, backend-level
pub struct PipelineCacheConfig { pub path: Option<PathBuf> }       // None = in-memory only
fn save_pipeline_cache(&self) -> Result<(), RhiError>;              // also called from shutdown
```

## 6. Data structures

`PipelineKey` (stable 64/128-bit hash of the descriptor plus layout); `Pipelines { by_key: HashMap<PipelineKey, PipelineHandle>, pool: Pool<VkPipeline> }`; `CacheBlobHeader` (Vulkan-defined, 16 + `VK_UUID_SIZE` bytes; verify). The blob may be wrapped in an Anvil header with a format version and checksum (undecided).

## 7. Ownership and lifetime

Pipelines are owned by the pool; the `VkPipelineCache` by `DeviceShared` (created in `create_device`, destroyed in `DeviceShared::drop`, MODULE-002). Destroyed pipelines go through the deferred queue. Saving the blob happens before `destroy_pipeline_cache`; failure to write logs `warn!` and never blocks shutdown.

## 8. Execution flow

Startup (M5): read blob -> validate header against `PhysicalDeviceProperties` -> `create_pipeline_cache(initial_data)` (or empty). Creation: build key -> lookup -> on miss build Vulkan create info -> `create_graphics_pipelines(cache, ...)` -> insert. Shutdown: `get_pipeline_cache_data` -> write to a temp file -> atomic rename.

## 9. Threading model

`&self` creation with a locked map. `VkPipelineCache` is internally synchronized by the driver unless created with `EXTERNALLY_SYNCHRONIZED` (verify). Parallel pipeline compilation on worker threads is an M5 goal (NFR-PERF-009 measures compile time) and relies on this.

## 10. Error handling

Pipeline compile failure: `Backend{op, code}` or `InvalidDescriptor`; shader/layout incompatibility: `InvalidDescriptor` naming the binding. A bad or foreign cache blob is not an error: log and continue empty. Blob write failure: warn only.

## 11. Performance requirements

Measure pipeline creation time with a cold and a warm cache and the hit rate (NFR-PERF-009). No numeric targets until benchmark hardware exists (NFR-PERF-012). Creation must not occur on the per-frame path after warm-up.

## 12. Testing requirements

Unit: key stability (same descriptor, same key across runs; any field change alters it), blob header validation with crafted headers (wrong vendor/device/uuid/short), dedupe map behaviour. Integration (lavapipe): create the same pipeline twice yields one native pipeline; save then reload cache and create succeeds; corrupted blob falls back to empty; compute pipeline smoke test.

## 13. Acceptance criteria (Draft)

- [ ] M2: graphics and compute pipelines creatable and destroyable with zero validation messages.
- [ ] M2: identical descriptors resolve to one pipeline (if dedupe is adopted).
- [ ] M5: a second run with a valid blob measurably reduces pipeline creation time (reported, no fixed threshold).
- [ ] M5: a blob from another device or a corrupt file is ignored without error.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RHI-007, FR-RHI-008, FR-RHI-009 | pipeline and layout creation |
| FR-VK-011 | `VkPipelineCache` creation and persistence |
| FR-SHD-004 | compatibility check against reflection |
| FR-SHD-005 | shared hashing with the shader cache (M5) |
| NFR-PERF-009 | compile-time measurement |

## 15. Open questions

1. Binding model (descriptor sets, descriptor indexing, push descriptors) decides the pipeline layout derivation; ADR required before M2 (LAYER-007 Q1).
2. Pipeline dedupe: does the RHI expose shared handles with reference counts, or does the renderer own dedupe?
3. Stable hash function and crate (not `std::hash::DefaultHasher`).
4. Cache file location and invalidation on driver update (path policy and who passes it).
5. Is the in-memory shader-module cache (MODULE-008) and this cache keyed together?
6. Pipeline libraries / `VK_EXT_graphics_pipeline_library` are out of scope (not in the ADR-0004 baseline).
