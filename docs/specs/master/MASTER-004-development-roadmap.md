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
MASTER-000 §4.3 and §22 p3 forbid performance targets before this section is completed in a benchmark spec (docs/benchmarks/). All items below are currently undefined.
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
Target: TBD after benchmark hardware/workload is defined.

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
