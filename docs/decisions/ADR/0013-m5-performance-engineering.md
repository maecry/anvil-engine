# ADR-0013: M5 performance engineering scope and techniques

- Status: Accepted
- Date: 2026-11-02

## Context
M5 covers:
- FR-REND-019..024: compute, frustum and occlusion culling, GPU-driven indirect rendering, LOD, multithreaded preparation and texture streaming.
- FR-VK-011 (pipeline cache), FR-AST-011 (asset streaming), FR-TOOL-007/008 (debug and profiler) and FR-PLAT-013 (worker threads).
- The items M4 deferred: point-light shadows, transient aliasing, buffer tracking and async compute.

MASTER-000 §4.3/§22 p3 forbid performance targets until a benchmark baseline exists (MASTER-004 §5). There is no asset system (M6) and no UI (egui, M6).

## Problem
Define the benchmark baseline, then pick the techniques and the minimum RHI growth for GPU-driven rendering, without depending on M6.

## Decision
1. **Benchmark first.**
   - The new spec `docs/benchmarks/BENCH-001-baseline.md` answers every item of MASTER-004 §5.
   - Reference machine: the dev box. i7-12700KF, 32 GB, RX 9070 XT on RADV/Mesa, CachyOS, Wayland, release profile with `lto = "thin"`, validation off.
   - Workloads are deterministic and driven by the frame number:
     - `pbr-scene`: default and stress-lights variants, forward and deferred.
     - New `examples/many-draws`: about 50 000 instances of 3 meshes with LODs in a large field, with a moving camera path.
   - Protocol:
     - 1920×1080, vsync off (`ANVIL_VSYNC=0`).
     - 120 warm-up frames, then 1000 measured, 3 runs.
     - Median and p99 of CPU frame time, GPU frame time (timestamps) and per-pass GPU time.
   - `ANVIL_BENCH=path.json` writes the results.
   - Baselines are stored in `docs/benchmarks/results/<date>-<workload>.json`.
   - Regression rule: more than 10% worse median on the reference machine needs justification. CI is not used for performance, because it runs on lavapipe.
   - Every M5 optimization is reported as a before/after against this baseline.
2. **Profiling (FR-TOOL-008).**
   - GPU timestamps for every graph pass, through a per-frame-slot query pool read back with a frames-in-flight delay. They are exposed in `GraphStats`/`RendererStats` and the bench JSON.
   - CPU timing comes from `tracing` spans. An optional cargo feature `profile-tracy` adds `tracing-tracy`.
   - Memory tracking already exists in `DeviceDiagnostics` (ADR-0010).
3. **Threads (FR-PLAT-013, FR-REND-023).**
   - `rayon` (a workspace dependency) is the worker pool, configured once at startup. `ANVIL_THREADS` overrides the thread count.
   - Parallelized: frustum culling, cluster assignment, sorting and instance-data packing.
   - Command recording stays single-threaded. Parallel recording needs secondary command buffers or one encoder per thread, and is deferred until a benchmark shows recording time matters.
4. **RHI growth (LAYER-003 §20).**
   - Compute pipelines and `dispatch`/`dispatch_indirect`.
   - Storage images: bindless set 0 binding 2, `STORAGE_IMAGE[]`.
   - Textures gain `mip_levels` and `array_layers` / cube, with per-mip and per-layer views for storage and attachment use.
   - `draw_indexed_indirect_count`.
   - Buffer barriers: `BufferState` (Indirect, ShaderRead, ShaderWrite, TransferDst/Src).
   - Timestamp queries.
   - A pipeline cache persisted to disk (FR-VK-011) at `ANVIL_PIPELINE_CACHE` (default `target/anvil-pipeline-cache.bin`), validated by the header's vendor/device/UUID.
   - Texture state stays whole-texture, except where a pass explicitly uses per-mip views (depth pyramid, mip streaming). Those cases are tracked by the graph as subresource ranges for *those textures only*.
5. **Render graph.**
   - Graph-managed buffers (transient and imported) with read/write declarations and buffer barriers.
   - Compute passes.
   - Transient memory aliasing becomes optional, behind an RHI capability. It is done only if BENCH-001 VRAM numbers justify it; otherwise the shortcut stays recorded.
   - Async compute is evaluated and adopted only with a measured gain.
6. **GPU-driven rendering (FR-REND-020/021/022).**
   - All instances live in one persistent GPU instance buffer, updated incrementally. Per-instance changes are uploaded through the ring.
   - A compute pass does frustum culling plus **two-phase Hi-Z occlusion culling**: the previous frame's visible set is drawn first, then the depth pyramid is built, then the remaining instances are tested.
   - Per-instance LOD selection happens in the same pass, by projected sphere size.
   - It writes compacted `DrawIndexedIndirectCommand`s per material batch, consumed by `draw_indexed_indirect_count`.
   - The same path serves the prepass, G-buffer and forward opaque passes. Shadow views use GPU frustum culling only.
   - The CPU path from M4 stays as a fallback (`ANVIL_GPU_CULLING=0`) for comparison and debugging.
   - All meshes share one big vertex buffer and one big index buffer (a suballocated mesh arena), so a single bind serves every draw.
7. **LOD.** `MeshData` can carry LOD levels. The `meshopt` crate generates them (`simplify`) when they are not given. Selection uses screen-space error with a hysteresis-free threshold.
8. **Texture streaming (FR-REND-024, FR-AST-011 subset).**
   - Textures are created with a full mip chain, and the CPU keeps the source data.
   - Residency is decided per texture from GPU feedback: the lowest mip requested by visible draws, written by the shading passes into a feedback buffer, or a CPU estimate from the projected size.
   - Uploads and evictions are bounded by a VRAM budget and a per-frame upload budget.
   - The shader clamps sampling with `minLod` from a per-texture resident-mip value.
   - Disk/asset streaming is M6 with the asset system. This milestone streams from CPU memory.
9. **Point-light shadows.** Cube shadow maps (6 layered views), up to 4, with distance-based selection.
10. **Debug (FR-TOOL-007).**
    - A debug-geometry API (lines, boxes, spheres, frusta) drawn as a graph pass.
    - The existing graph DOT/JSON dump.
    - Debug views for culling results (occluded instances tinted), the Hi-Z level, LOD level and mip residency.
    - Text overlays wait for egui (M6). Stats go to logs and the bench JSON instead.

## Advantages
- Optimizations are measured against a written baseline, as the master spec requires.
- GPU-driven culling and LOD scale the draw count while keeping the CPU path as an oracle.

## Disadvantages
- A large RHI expansion: compute, storage images, mips, layers, indirect and queries.
- Subresource state tracking for a few textures adds graph complexity.
- Streaming without an asset system is CPU-to-GPU only.

## Consequences
- LAYER-001 (threading), LAYER-003 §20, LAYER-004, LAYER-005, LAYER-006, LAYER-007 (streaming, arena) and MODULE-007 (pipeline cache) are updated.
- New spec BENCH-001 with committed baseline results.
- New example `many-draws`.

## Revisit conditions
- Profiling shows command recording to be a bottleneck: add parallel recording.
- BENCH-001 shows VRAM pressure: add aliasing.
- Async compute shows a gain: adopt it.
