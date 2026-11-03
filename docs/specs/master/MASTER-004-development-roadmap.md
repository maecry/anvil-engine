# MASTER-004: Development Roadmap
| Field | Value |
|---|---|
| Status | Accepted |
| Traces to | MASTER-000 §13, §14, §15, §17, §20 |
| Last updated | 2026-11-02 |

Current state: M0 in progress. Crates `foundation`, `platform`, `rhi`, `rhi-vulkan` and `examples/triangle` exist as empty stubs (ADR-0002). Per MASTER-000 §13, no milestone's implementation starts until its required specs and research are accepted. Deliverables and completion criteria are copied from MASTER-000 §14; "FR" ranges refer to MASTER-002.

## 1. Spec and module key
| Layer spec | Module spec |
|---|---|
| LAYER-001-foundation | MODULE-001-vulkan-instance |
| LAYER-002-platform | MODULE-002-vulkan-device |
| LAYER-003-rhi | MODULE-003-vulkan-swapchain |
| LAYER-004-vulkan-backend | MODULE-004-command-manager |
| LAYER-005-renderer | MODULE-005-buffer-allocator |
| LAYER-006-render-graph | MODULE-006-texture-registry |
| LAYER-007-resource-management | MODULE-007-pipeline-cache |
| LAYER-008-shader-system | MODULE-008-shader-compiler |
| LAYER-009-asset-system | MODULE-009-render-graph-compiler |
| LAYER-010-runtime-and-editor | MODULE-010-render-world |

## 2. Milestones
| M | Name | Deliverables (§14) | Entry criteria | Exit criteria (§14) | Specs required before implementation | Research required |
|---|---|---|---|---|---|---|
| M0 | Foundation | Master spec; initial architecture; repo structure; initial ADRs; Cargo workspace; CI configuration; development standards | Project start | Clean workspace builds; documentation structure established; dependency rules documented | MASTER-000..004; ADR-0001..0003 (0003 still pending decision); LAYER-001 | Vulkan architecture, GPU synchronization, resource lifetime (§20 W1) |
| M1 | Vulkan Foundation | Vulkan init; physical device selection; logical device; surface; swapchain; command buffers; synchronization; validation; triangle | M0 exit; ADR-0003 decided if shaders are compiled in M1 | Stable app lifecycle; window resizing; clean shutdown; no known validation errors in baseline test | LAYER-002, LAYER-003 (minimal), LAYER-004, LAYER-008 (minimal); MODULE-001, 002, 003, 004, 008 | Vulkan instance/device/swapchain, frames in flight, shaderc |
| M2 | RHI and Resources | Buffers; textures; samplers; pipelines; binding management; resource handles; memory allocation; deferred destruction | M1 exit | Reusable GPU resource infrastructure | LAYER-003 (full), LAYER-007; MODULE-005, 006 | GPU memory allocation (gpu-allocator vs VMA), descriptor/bindless models, handle design, deferred destruction |
| M3 | Core Renderer | Mesh rendering; cameras; transforms; textures; materials; basic lighting; PBR; shadow mapping; environment lighting | M2 exit | Complete PBR scene rendering | LAYER-005; MODULE-010 | PBR/BRDF, shadow mapping, IBL, scene extraction (Bevy, Filament) |
| M4 | Render Graph and Advanced Rendering | Render graph; dependency analysis; resource transitions; deferred; post-processing; SSAO; TAA; clustered lighting | M3 exit | Configurable pipeline with validated dependencies | LAYER-006; MODULE-009 | Render graph designs, barrier planning, deferred, SSAO, TAA, clustered lighting |
| M5 | Performance Engineering | Multithreaded preparation; GPU-driven rendering; indirect drawing; GPU culling; LOD; streaming; pipeline cache; profiling | M4 exit; benchmark baseline defined (section 5) | Reproducible benchmarks and measurable optimizations | LAYER-001 (threading update), LAYER-007 (streaming), LAYER-005 update; MODULE-007 | GPU-driven rendering, culling, streaming, task scheduling, Tracy/GPU timestamps |
| M6 | Engine Runtime and Tooling | Asset database; scene system; runtime; editor viewport; inspector; asset browser; material tools; debug visualization | M5 exit | A complete sample project can be created, saved, reopened and executed | LAYER-009, LAYER-010 | ECS, asset pipelines, serialization, egui integration (Godot, Fyrox) |
| M7 | Production Hardening | Regression testing; leak testing; device-loss handling; shader compatibility tests; asset corruption handling; stable public interfaces; packaging; user documentation | M6 exit | Reproducible release candidate with documented limitations | Updates to all LAYER-*; release/packaging spec (not yet named) | Device loss, fuzzing/corruption handling, packaging |

Specs and modules above are the plan; this document does not create them. Spec ownership of M1 items that sit in a later layer (e.g. shader compilation in M1, FR-SHD-001) is an ambiguity noted in the report; LAYER-008 and MODULE-008 are needed by M1.

## 3. Requirements per milestone
| M | FR / NFR areas landing (see MASTER-002 milestone column) |
|---|---|
| M0 | NFR-QUAL, NFR-ARCH, NFR-CI-001..004, ADRs, docs |
| M1 | FR-PLAT (most), FR-VK-001..007, 012, 013, FR-RHI (device, command, queues, sync, swapchain, shader modules, graphics pipeline), FR-SHD-001 |
| M2 | FR-RHI (resources), FR-VK-008..010, FR-RES (registry, deferred destruction, allocation) |
| M3 | FR-REND-001..010, 025, FR-PLAT-005, FR-RES-012 |
| M4 | FR-RG, FR-REND-011..018, FR-RES-011 |
| M5 | FR-REND-019..024, FR-VK-011, FR-AST-011, FR-TOOL-007..008, FR-PLAT-013 |
| M6 | FR-AST, FR-SCN, FR-TOOL-001..006 |
| M7 | FR-VK-014, FR-SHD-009, FR-SCN-007, FR-PLAT-015, FR-TOOL-009, NFR-REL, release NFR-CI |

## 4. 30-day plan (MASTER-000 §20) mapped to milestones
| Week | §20 tasks | Milestone | Specs/modules involved |
|---|---|---|---|
| 1 Research and design | Master spec; research Vulkan, GPU sync, resource lifetime; crate boundaries; ADRs | M0 | MASTER-*, ADRs, LAYER-001 |
| 2 Workspace and platform | Cargo workspace; core and platform crates; winit; app lifecycle; logging and error handling | M0 (workspace, core) then M1 (lifecycle, windowing) | LAYER-001, LAYER-002 |
| 3 Vulkan initialization | Instance; physical devices; queues; logical device; surface; swapchain; validation | M1 | LAYER-004, MODULE-001..003 |
| 4 First rendering | Command pools/buffers; sync; graphics pipeline; shader compilation; triangle; initial integration tests | M1 | MODULE-004, MODULE-008, LAYER-003/008 minimal |

Month 1 acceptance (§20) = M1 exit plus: builds, runs on Linux, window, swapchain, triangle, validation enabled, logs accessible, docs match implementation.

## 5. Benchmark baseline (to define before any target)
MASTER-000 §4.3 and §22 p3 forbid performance targets before this section is completed in a benchmark spec (docs/benchmarks/). **Every item below is defined in [`docs/benchmarks/BENCH-001-baseline.md`](../../benchmarks/BENCH-001-baseline.md)** (M5a, 2026-11-02), which also records the M4 baseline and the first before/after. The Notes column is the original question; BENCH-001 sections 1-9 answer it (hardware 1, Vulkan environment 2, build 3, workloads 4, resolution and present mode 5, protocol 6, metrics 7, variance 8, storage and comparison 9). Items still open: the CI runner, the asset-load and streaming workloads, CPU memory, pipeline-switch count, shader compile and asset-load timing.
| Item to define | Notes |
|---|---|
| Reference hardware | GPU model(s), driver version, CPU, RAM, OS/kernel; at least one dedicated CI runner (§17 item 7). |
| Vulkan environment | SDK/loader version, validation layers on/off during benchmarks. |
| Build configuration | Profile (release), LTO/flags, toolchain version. |
| Workloads | Named, deterministic scenes: triangle, textured cube, PBR scene, shadows, deferred (examples in §7), large-count draw scene, asset-load set, streaming set. |
| Resolution and present mode | Window size, vsync/pacing mode. |
| Run protocol | Warm-up, duration, repetitions, statistic (median, p99). |
| Metrics | The 11 items of §4.3 (NFR-PERF-001..011). |
| Storage and comparison | Where baselines live, regression threshold method (threshold TBD). |
| Variance control | Clocks, thermal state, background load. |
Target: still none. The hardware and workloads are now defined and a baseline exists (BENCH-001 section 10); a target needs the remaining workloads and a second machine, and is set by a later benchmark revision, not by M5a.

## 6. Change control
Roadmap changes follow MASTER-000 §13.5. Milestone exit requires: exit criteria met, tests for the milestone's deliverables (NFR-TEST-007), specs matching code, and ADRs recorded.

## Status log

### M1 Vulkan Foundation (2026-11-02)
| Exit criterion | Status | Evidence |
|---|---|---|
| Stable app lifecycle | Done | `triangle` start → frames → `Control::Exit` → `on_exit` teardown |
| Window resizing | Code done; manual resize/minimise check pending | swapchain recreation on `Resized` / `OutOfDate` / `Recreate` |
| Clean shutdown | Done | reverse-order destroy, `pending_destroys: 0` at exit |
| No validation errors | Done | `ANVIL_SYNC_VALIDATION=1 ANVIL_EXIT_AFTER_FRAMES=120 cargo run -p triangle`: errors 0, warnings 0 (RADV, RX 9070 XT); FIFO run smooth (manual) |

Open: CI never run (no remote); headless swapchain test; wait_idle shortcuts on swapchain recreate/destroy (M2).

### M2 RHI and Resources (2026-11-02)
| Exit criterion | Status | Evidence |
|---|---|---|
| Reusable GPU resource infrastructure | Done | buffers/textures/samplers, bindless (ADR-0010), queued uploads, deferred destruction; 126 tests incl. GPU readback roundtrips |
| No validation errors | Done | `textured-quad` + `triangle`, 120 frames, sync validation: 0 errors / 0 warnings; GPU-AV run: 0 errors |
| Visual check | Pending (manual) | `cargo run -p textured-quad` should show a checkerboard quad |

Open: lavapipe/CI unverified; no staging ring (per-call staging); swapchain recreate still wait_idle.

### M3 Core Renderer (2026-11-02)
| Exit criterion | Status | Evidence |
|---|---|---|
| Complete PBR scene rendering | Done | `examples/pbr-scene`: glTF metallic-roughness materials (Cook-Torrance GGX), directional/point/spot lights, 2048 directional shadow map with PCF, procedural-sky IBL (irradiance, 6 prefiltered roughness levels, BRDF LUT), skybox, ACES tonemap, fly camera; renderer crate with extraction types (ADR-0011) |
| No validation errors | Done | `pbr-scene`, `textured-quad`, `triangle`, 120 frames, sync validation: 0 errors / 0 warnings; `ANVIL_GPU_AV=1` run: 0 errors (4 layer-settings notices) |
| Visual check | Done | `ANVIL_SCREENSHOT=path.png ANVIL_EXIT_AFTER_FRAMES=60 cargo run -p pbr-scene` writes a PNG: sky with horizon, soft shadows, coloured point/spot pools, metals reflecting the sky, blurrier reflections as roughness rises, no seam at the equirect wrap |

Shortcuts / deferred (all documented in LAYER-005 §12a and the code as `shortcut:`): one shadow map, no cascades (M4); no point/spot shadows (M4); opaque materials only, no alpha blend/mask (M4); no clustered/Forward+ lighting, light count capped at 64 (M4); no visibility culling or sorting (M4, with the render graph); fixed 4 MiB upload ring; BRDF LUT baked on the first render rather than in `Renderer::new`; analytic sky only, HDR file loading and glTF import wait for the asset system (LAYER-009); sun disk clamped out of the IBL bakes (the sun is a directional light); material texture indices are not refcounted.

Open: golden-image test (NFR-TEST-003) not yet automated; renderer is not yet built against a mock backend; render graph replaces the hand-written barriers in M4.

### M4 Render Graph and Advanced Rendering (2026-11-02)
| Exit criterion | Status | Evidence |
|---|---|---|
| Configurable pipeline with validated dependencies | Done | `render-graph` crate (ADR-0012): declared reads/writes, culling, barrier planning, transient pool, `describe()` and `ANVIL_GRAPH_DUMP` (DOT/JSON), errors naming pass and resource; 10 unit tests plus a GPU test; the whole renderer is built from graph passes with no hand-written barriers (27 passes per frame with every effect on, compile ~4 us in release) |
| Deferred, clustered lighting, SSAO, TAA, post-processing | Done | depth prepass; 16x9x24 CPU-clustered lights (cap 1024; 259 lights assigned in ~0.15 ms release); forward and runtime-selectable deferred path sharing the lighting GLSL; alpha mask/blend; 4 cascades + 4 spot shadows; frustum culling and sorting; half-resolution SSAO with bilateral upsample; Halton-jittered TAA with velocity and YCoCg clamp; 6-level bloom; debug views for clusters, cascades and SSAO |
| No validation errors | Done | `pbr-scene`, `textured-quad`, `triangle`, 120 frames, sync validation: 0 errors / 0 warnings; pbr-scene in all 8 SSAO/TAA/bloom combinations x forward/deferred with 256 stress lights (`ANVIL_STRESS_LIGHTS=1`): 0 / 0; a run cycling the combinations every 3 frames: 0 / 0; `ANVIL_GPU_AV=1`: 0 errors |
| Visual check | Done | pbr-scene screenshots inspected: SSAO contact darkening under and between spheres and at the fence base; TAA smooths sphere and fence edges and shows no ghosting during `ANVIL_CAMERA_ORBIT=1` or on the turning cube; bloom halos around metal highlights and stress lights; forward vs deferred max 47/255 (edge pixels), mean 0.14/255 |

RHI changes: only the Vulkan 1.3 feature `shaderDemoteToHelperInvocation` is enabled (needed by `discard`; LAYER-003 §19). A glam 0.34.2 bug (`camera::rh::proj::vulkan::orthographic` does not flip the Y translation) is worked around by a local `orthographic` in the renderer.

Shortcuts / deferred (all documented in LAYER-005, LAYER-006, MODULE-009 and as `shortcut:` in the code): point-light shadows (need cube/layered textures, M5); no transient memory aliasing (pool and lifetime reuse only, M5); buffers are not tracked by the graph (M5 with compute); CPU light assignment and frustum culling only, no GPU culling (M5); one graphics queue, no async compute (M5); blended objects do not cast shadows and are sorted per object; TAA uses bilinear history sampling, no depth rejection for moving objects and no material mip bias or sharpening; SSAO noise is only blurred, not temporally filtered beyond TAA; bloom strength is a constant (0.025); fixed 8 MiB upload ring and 100 m cluster far plane; BRDF LUT still baked on the first render.

Open: golden-image test (NFR-TEST-003) not automated (screenshots are inspected by hand and pixel-diffed ad hoc); renderer not built against a mock backend; runtime window resize with TAA only exercised by the history-reset code path (the toggle test), not by a scripted resize; GPU frame cost not measured (CPU wall time ~0.7 ms per frame on RADV, vsync off).

### M5a Baseline, Profiling, Threads, Pipeline Cache (2026-11-02)
| Deliverable | Status | Evidence |
|---|---|---|
| Benchmark baseline (§5, ADR-0013 §1) | Done | `docs/benchmarks/BENCH-001-baseline.md`; `ANVIL_BENCH=path.json` in `pbr-scene` and the new `many-draws` (about 50 000 instances); results `docs/benchmarks/results/2026-11-02-*-{baseline,m5a}.json`, 3 runs each (`docs/benchmarks/run.sh`) |
| Profiling (ADR-0013 §2) | Done | RHI timestamp queries (LAYER-003 §20), per-pass GPU times in `RendererStats::gpu` and the bench JSON, CPU stage spans `renderer.*`, optional `profile-tracy` feature |
| Threads (ADR-0013 §3) | Done | `rayon` pool in `foundation` (`ANVIL_THREADS`); parallel packing, culling, sorting, shadow lists and cluster assignment, identical to the serial path (3 unit tests); many-draws CPU frame 9.27 to 7.18 ms (-22.5 %), pbr-scene-deferred-stress 0.381 to 0.311 ms (-18.5 %) |
| Pipeline cache (FR-VK-011) | Done | persisted `VkPipelineCache` with header validation (MODULE-007 §8a); GPU test saves and reloads |
| No validation errors | Done | the 4 examples, 120 frames, debug build, sync validation: 0 errors / 0 warnings |

Shortcuts: one fixed parallel threshold (`PAR_MIN` 2048 instances, `PAR_LIGHTS` 16); command recording and the ring upload still single-threaded; only 126 passes per graph are timed; per-instance upload ring still fixed (now 16 MiB); the pipeline cache file has no checksum. Open: default thread count (20) is not the best on the reference CPU (4-8 is); CPU memory, pipeline switches and frame-pacing worst case are not measured.

### M5b GPU-driven rendering (2026-11-02)
| Deliverable | Status | Evidence |
|---|---|---|
| RHI growth (LAYER-003 §20.3) | Done | compute pipelines, `dispatch` / `dispatch_indirect`, bindless storage images (binding 2), mips / layers / cube and per-mip or per-layer view handles, `BufferState` barriers, `fill_buffer`, `draw_indexed_indirect` and `_count`; GPU tests `crates/rhi-vulkan/tests/compute.rs` (compute write read back, indirect count draw, per-mip storage writes, view creation), sync validation clean |
| Render graph (LAYER-006 §8b) | Done | compute passes, transient and imported buffers with read / write / modify declarations and buffer barriers, per-mip tracking through view textures (depth pyramid only); 5 new `plan.rs` unit tests |
| Mesh arena (LAYER-007 §17) | Done | one vertex and one index buffer, first-fit suballocator, one index bind per pass |
| Persistent GPU scene (LAYER-005 M5b) | Done | per-frame diff against the previous scene instead of stable ids; only changed instances are uploaded (0 per frame for a static scene) |
| GPU culling, LOD, indirect draws (ADR-0013 §6, §7) | Done | two-phase Hi-Z occlusion plus frustum culling and LOD selection in compute, compacted indirect commands per pipeline variant for prepass, G-buffer, forward and the 8 shadow views; blended objects stay on the CPU; `ANVIL_GPU_CULLING=0` keeps the CPU path; `meshopt` LODs (up to 4 levels) |
| Debug views | Done | `ANVIL_DEBUG_VIEW=lod` and `occlusion` |
| Stats and benchmark | Done | `RendererStats::cull` (visible, frustum culled, occlusion culled, triangles after LOD, indirect draws, read back with a 2 frame delay); BENCH-001 section 12; `docs/benchmarks/results/2026-11-02-*-m5b.json` |
| No validation errors | Done | the 4 examples and many-draws with `ANVIL_GPU_CULLING=0`, 120 frames, debug, sync validation: 0 errors / 0 warnings; `ANVIL_GPU_AV=1` on many-draws: 0 errors (4 warnings are the layer's own setting notices); `ANVIL_GPU_TESTS=1 cargo test --workspace` passes |

Results (reference machine, median of 3 runs, BENCH-001 section 12): `many-draws` CPU frame 7.18 (M5a) / 9.27 (baseline) to **0.61 ms**, GPU frame 2.21 to **1.25 ms**, 2.86 M to 0.50 M triangles, frame interval 7.26 to 1.28 ms (now GPU-bound); the CPU path on the new persistent buffers is 1.91 ms CPU / 1.56 ms GPU. `pbr-scene` GPU frame +0.9 % (forward) and +2.4 % (deferred, 259 lights): within the noise and the 10 % rule, but a small consistent cost of the culling passes on a 26-instance scene (reported honestly, BENCH-001 section 12). Pixel checks: with `ANVIL_LOD=0` the GPU path is byte-identical to the CPU path and to the M5a screenshots; at the default 0.5 px LOD pbr-scene differs in 0.22 % of pixels.

Shortcuts (all as `shortcut:` in code or in LAYER-005 / LAYER-007): instance identity is the scene index (removal from the middle re-uploads the tail); fixed arena (256 Ki vertices, 1 Mi indices), mesh table (4096) and upload ring (16 MiB); command buffer sized to the opaque instance count per variant and view; occlusion test on bounding spheres; shadow views use the camera's LOD; cube and array textures have no bindless slots yet (M5c); storage image and indirect-count features enabled without capability checks; transient aliasing and async compute not adopted (no measured need). Open: texture streaming, point-light shadows and the debug-geometry API (M5c); the occlusion win could only be measured on the synthetic wall scene (the many-draws field is sparse).

### M5c Point-light shadows, texture streaming, debug geometry; M5 complete (2026-11-03)
| Deliverable | Status | Evidence |
|---|---|---|
| Point-light shadows (ADR-0013 §9) | Done | up to 4 cube maps (D32, 512 per face, six layered views) chosen by distance and intensity; faces use the shadow-view path (GPU culling, indirect draws); comparison sampling on a cube view through the bindless binding 0 (LAYER-003 §20.4, LAYER-005 M5c); pbr-scene's warm light casts; screenshots inspected: shadows from the spheres, cube and fence, no seams at face edges, no acne; with shadows off the image is pixel-identical to M5b |
| Texture streaming (§8) | Done | CPU mip chains (sRGB-correct), CPU residency estimate, per-frame upload budget, VRAM budget with eviction, resident-mip-only GPU images (LAYER-007 §18), `ANVIL_DEBUG_VIEW=mips`, `TextureStats`; many-draws has 3 streamed textures (2048, 2048, 1024) |
| Debug geometry (§10) | Done | `Renderer::debug_lines()` and `ANVIL_DEBUG_DRAW=bounds|cascades|lights`, `debug.lines` pass after the transparent pass |
| Device capability checks | Done | the five M5b features are rejected with a clear message; `ANVIL_GPU_TESTS=1 cargo test --workspace` passes on RADV and on lavapipe |
| Benchmark | Done | BENCH-001 §13, `docs/benchmarks/results/2026-11-03-*-m5c.json` (plus `pbr-scene-forward-no-point-shadow` and `many-draws-point-shadows`) |
| No validation errors | Done | the 4 examples, 120 frames, debug, sync validation, also with `ANVIL_DEBUG_DRAW` bounds / cascades / lights, `ANVIL_DEBUG_VIEW=mips` with a 0.6 MiB budget, and many-draws with the CPU path: 0 errors / 0 warnings; `ANVIL_GPU_AV=1` on pbr-scene and many-draws: 0 errors (4 warnings are the layer's own setting notices) |

Results (BENCH-001 §13): pbr-scene GPU frame 0.84 (M5b) to 0.72 ms (noise of GPU clocks; the point light adds 0.027 ms, 0.013 ms of it in the six face passes); many-draws with 3 shadow-casting lights adds 0.073 ms of GPU time (0.014 ms per light in face passes), streaming costs 0.076 ms CPU per frame at 50 000 instances.

**M5 is complete.** Delivered: benchmark baseline and protocol (M5a), profiling, rayon threads, pipeline cache (M5a), compute and indirect RHI, GPU-driven culling with Hi-Z, LOD, mesh arena (M5b), point-light shadows, texture streaming, debug geometry (M5c). **Deferred, recorded:** transient memory aliasing and async compute (not adopted, no measured need: VRAM and GPU time show neither pressure nor an idle queue to exploit), parallel command recording (recording is 0.07-0.12 ms), disk and asset streaming (M6 assets), on-screen overlays (M6 egui), GPU texture-feedback streaming and array-texture slots. **Shortcuts (all as `shortcut:` in code or in the docs):** the streaming estimate ignores UV density, mip filtering is a plain box filter (odd sizes drop half a texel, normals not renormalized), texture reallocation re-uploads all resident levels, point shadows use the camera's LOD and a fixed 512 face resolution, point casters need range of at least 0.5, debug lines are capped at 131 072 vertices, cascade volumes can only be seen from inside.

### M6a Assets, scene, runtime, player, sample project (2026-11-03)

ADR-0014 phase M6a. A sample project can be created in code, saved, reopened and executed headless; the editor and the egui overlays are M6b.

| Deliverable | Status | Evidence |
|---|---|---|
| `assets` crate (ADR-0014 §3-6) | Done | UUID ids in `.meta` stable across moves, deterministic glTF sub-asset ids, glTF/PNG/JPEG/EXR/KTX2(uncompressed)/`.material`/`.scene` importers, binary cache keyed by importer version and source hash, dependency graph with failure propagation, rayon loading drained by `update()`, `notify` hot reload (200 ms), stats; 10 unit and integration tests incl. corrupt files (truncated glb, garbage PNG/KTX2, newer-version material and scene) -> `Failed`, no panic |
| `scene` crate (ADR-0014 §2) | Done | hecs wrapper, Name/Transform/GlobalTransform/MeshRenderer/Light/Camera/Parent/Children/EntityUuid, cycle-checked hierarchy, parents-first transform system, RON scenes with `version` (newer rejected), glTF prefab instantiation; 5 tests |
| Renderer additions | Done | `update_material`, fallback cube, white texture, magenta missing material; all existing tests and pixel outputs unchanged |
| `runtime` crate (ADR-0014 §7) | Done | `Runtime`, `Application`, `RuntimeCore` (bridge, systems, extract), headless mode with the `ANVIL_*` env conventions, `project.anvil`; GPU tests: create-save-reopen-run, fallbacks for broken assets, hot reload swapping a GPU texture, all with 0 validation errors/warnings under sync validation |
| `anvil-player` | Done | `anvil-player samples/sample-project` windowed and headless; screenshot inspected: textured crate, brick-material crate, ground, sun shadow |
| `samples/sample-project` (ADR-0014 §10) | Done | project file, scene, textured glb, PNG, material, `.meta` files, all generated by `crates/runtime/tests/generate_sample.rs` |

Shortcuts (as `shortcut:` in code): EXR is clamped to 8-bit; KTX2 only uncompressed RGBA8 level 0; the glTF cache key ignores external buffers/images; FNV-1a 64-bit source hash; no importer settings in `.meta`; damaged `.meta` skips the asset; first camera wins; no registry for game components in scenes; no typed `AssetHandle<T>` (ids plus `state()`/`get()`). Deviations from ADR-0014: the source hash is stored in the cache header, not in the `.meta` (keeps metas stable in version control); scene-to-asset dependencies are soft (a missing mesh must not fail the scene).
