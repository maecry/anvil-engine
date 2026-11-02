# MASTER-003: Non-Functional Requirements
| Field | Value |
|---|---|
| Status | Accepted |
| Traces to | MASTER-000 §4.3, §5.2, §11, §15, §16, §17 |
| Last updated | 2026-11-02 |

Every entry states how it is measured. MASTER-000 §4.3 forbids arbitrary performance guarantees; no numeric target appears here. Where a metric needs a threshold the entry reads "Target: TBD after benchmark hardware/workload is defined (see MASTER-004)".

## PERF - Performance (measured, no targets yet)
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-PERF-001 | The engine shall measure and report CPU frame preparation time. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Profiler CPU zones per frame (profiler / Tracy); median and p99 over a fixed workload. |
| NFR-PERF-002 | The engine shall measure and report GPU frame execution time. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | GPU timestamp queries per frame/pass; median and p99. |
| NFR-PERF-003 | The engine shall measure and report frame pacing. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Frame-time distribution (variance, worst frames) over a fixed workload. |
| NFR-PERF-004 | The engine shall measure and report GPU memory consumption. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Allocator statistics (FR-RES-007) at end of workload. |
| NFR-PERF-005 | The engine shall measure and report CPU memory consumption. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Process/allocator tracking at end of workload. |
| NFR-PERF-006 | The engine shall measure and report draw-call count. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Per-frame renderer statistic. |
| NFR-PERF-007 | The engine shall measure and report pipeline switching count. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Per-frame renderer statistic. |
| NFR-PERF-008 | The engine shall measure and report resource upload time. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Timed staging/upload scheduling per workload. |
| NFR-PERF-009 | The engine shall measure and report shader compilation time. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Timed compile of the shader set, cold and cached. |
| NFR-PERF-010 | The engine shall measure and report asset loading time. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Timed load of a fixed asset set. |
| NFR-PERF-011 | The engine shall measure and report render graph execution time. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). | §4.3 | Timed graph build+compile and execution per frame (FR-RG). |
| NFR-PERF-012 | The project shall not assert any performance guarantee before benchmark hardware and workloads are defined. | §4.3, §22 p3 | Review: no performance number in any spec lacks a benchmark reference. |
| NFR-PERF-013 | Optimizations shall be justified by a before/after measurement. | §16 Performance, §22 p3 | Review: PR links benchmark results. |
| NFR-PERF-014 | Performance regressions shall be tracked across commits. | §16 Performance, §17 item 9 | Performance test suite in CI compared to stored baseline; threshold TBD after benchmark hardware/workload is defined. |
| NFR-PERF-015 | The engine shall avoid unnecessary CPU/GPU synchronization. | §16 Performance | Profiler review: no full-device wait in the frame loop (grep/review plus GPU trace). |

## CONC - Concurrency
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-CONC-001 | Hot paths shall avoid unnecessary locks. | §11 | Code review; lock contention visible in profiler traces. |
| NFR-CONC-002 | Cross-thread and CPU/GPU synchronization shall be explicit. | §11, §22 p7 | Code review; validation layers (synchronization validation) clean. |
| NFR-CONC-003 | Render workers shall not share mutable scene state; they shall use immutable extracted data. | §11 | Review plus type-level (Send/Sync) enforcement where possible. |
| NFR-CONC-004 | Task scheduling overhead shall be measured, and a custom scheduler introduced only if measurements justify it. | §11, §6.1 | Benchmark of task system vs. Rayon baseline. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). |

## QUAL - Code quality
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-QUAL-001 | All code shall be formatted by rustfmt. | §16, coding-standards | CI: cargo fmt --check passes. |
| NFR-QUAL-002 | All code shall pass clippy with warnings denied. | §16, coding-standards | CI: cargo clippy --workspace --all-targets -D warnings passes. |
| NFR-QUAL-003 | Public APIs shall have rustdoc. | §16, coding-standards | missing_docs lint; cargo doc without warnings. |
| NFR-QUAL-004 | unsafe code shall be minimal and isolated, with a SAFETY comment on each block. | §16, ADR-0001, coding-standards | Review; grep for unsafe outside rhi-vulkan; clippy undocumented_unsafe_blocks if enabled. |
| NFR-QUAL-005 | Libraries shall use thiserror and shall not unwrap fallible GPU or IO paths outside tests. | §6.1, coding-standards | Clippy unwrap_used on non-test code or review. |
| NFR-QUAL-006 | Ownership shall be explicit and abstractions shall exist only for a real requirement. | §16, §22 p5 | Review: each new abstraction cites a spec or ADR. |
| NFR-QUAL-007 | Modules shall be independently testable. | §16 Architecture | Each crate has tests runnable with cargo test -p <crate> without other crates' GPU state where feasible. |
| NFR-QUAL-008 | Every important architectural decision shall have an ADR with all fields in §13.3. | §13.3, §16 | Review: ADR matches docs/decisions/ADR/0000-template.md. |
| NFR-QUAL-009 | Specifications and code shall stay consistent; discrepancies follow §13.5. | §13.5, §16, §22 p2 | Review at each milestone exit (MASTER-004). |
| NFR-QUAL-010 | Every module specification shall contain the sections listed in §13.4. | §13.4 | Doc review checklist per MODULE-*. |

## REL - Reliability
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-REL-001 | Errors shall be handled explicitly with no silent failures. | §16 Reliability | Review; tests for each error path of public APIs. |
| NFR-REL-002 | GPU operations shall be validated (validation layers on in debug builds). | §16, ADR-0001 | Integration/rendering tests run with validation; zero errors is a pass criterion. |
| NFR-REL-003 | Resource lifetime shall be tracked and no resource shall leak across a run. | §16, §14 M7 | Leak test: allocator/registry live-count is zero after shutdown. |
| NFR-REL-004 | Shutdown and recovery paths shall be tested. | §16, §15.5 | Integration test: repeated start/stop; device-loss test where supported. |
| NFR-REL-005 | Corrupt assets shall not crash the engine. | §14 M7 | Fuzz/corruption test inputs return errors. |
| NFR-REL-006 | The engine shall survive long-running sessions without unbounded memory growth. | §15.5 | Soak test; memory tracking trend. Duration TBD. |

## PORT - Portability
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-PORT-001 | The initial platform shall be Linux desktop; Windows and macOS are targets subject to backend availability. | Header, §2.3 | CI build/run matrix; macOS requires MoltenVK (ADR-0001). |
| NFR-PORT-002 | Platform-specific code shall be confined to platform and the backend. | §5.2 r1, §8.2 | Dependency check (NFR-ARCH); review. |
| NFR-PORT-003 | The RHI shall allow additional backends without changes to renderer code. | §2.2 o6, §22 p4 | Review: renderer crates build with no ash/Vulkan dependency. |
| NFR-PORT-004 | Mobile and every-API support are non-goals. | §2.3 | Scope review. |

## OBS - Observability
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-OBS-001 | The engine shall expose diagnostics for CPU execution, GPU execution, memory usage, rendering operations and resource lifetime. | §1.3 | Manual check at M5 exit: each of the five categories has a visible output. |
| NFR-OBS-002 | Logging shall use tracing and logs and errors shall be accessible to the user. | §6.1, §20 Month 1 | Run triangle example; log output visible. |
| NFR-OBS-003 | GPU objects and passes shall carry debug labels for RenderDoc capture. | §4.2, §6.2 | Manual: capture in RenderDoc shows named objects/passes. |
| NFR-OBS-004 | GPU timestamps and memory statistics shall be exportable. | §8.11 | Integration test of export output. |

## TEST - Testing
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-TEST-001 | Unit tests shall cover handle generation, resource registry, render graph dependency resolution, shader metadata, asset identifiers and transform calculations. | §15.1 | cargo test; coverage reviewed per milestone. |
| NFR-TEST-002 | Integration tests shall cover device init, buffer creation, texture upload, pipeline creation, swapchain recreation and resource destruction. | §15.2 | Integration suite on GPU runner. |
| NFR-TEST-003 | Rendering tests shall render deterministic scenes and compare output images with tolerances. | §15.3 | Image diff vs. reference; tolerance TBD per test. |
| NFR-TEST-004 | Rendering tests shall cover geometry, materials, lighting, shadows, transparency, post-processing and edge cases. | §15.3 | Test list matches categories. |
| NFR-TEST-005 | Performance tests shall track CPU/GPU frame time, GPU memory, draw calls, upload performance, graph overhead and asset loading. | §15.4 | Benchmarks in tests/performance. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). |
| NFR-TEST-006 | Stress tests shall cover large resource counts, repeated alloc/destroy, resizing, shader reload, asset streaming, long sessions and device loss. | §15.5 | Stress suite run before milestone exit. |
| NFR-TEST-007 | Testing shall exist throughout development; each milestone adds tests for its deliverables. | §15 | MASTER-004 exit criteria. |
| NFR-TEST-008 | Rendering output shall be tested for every rendering feature. | §22 p9 | Each FR-REND with verification 'rendering' has a test. |

## CI - CI/CD
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-CI-001 | CI shall run formatting checks. | §17 (1) | Pipeline job. |
| NFR-CI-002 | CI shall run clippy. | §17 (2) | Pipeline job. |
| NFR-CI-003 | CI shall run unit tests. | §17 (3) | Pipeline job. |
| NFR-CI-004 | CI shall build the workspace. | §17 (4) | Pipeline job. |
| NFR-CI-005 | CI shall compile shaders. | §17 (5), ADR-0003 | Pipeline job; needs shaderc/Vulkan SDK. |
| NFR-CI-006 | CI shall run integration tests. | §17 (6) | Pipeline job. |
| NFR-CI-007 | CI shall run GPU tests on dedicated runners. | §17 (7) | Runner exists and job green. |
| NFR-CI-008 | CI shall run rendering regression tests. | §17 (8) | Pipeline job with image diffs. |
| NFR-CI-009 | CI shall run performance regression tests. | §17 (9) | Pipeline job on fixed hardware. Target: TBD after benchmark hardware/workload is defined (see MASTER-004). |
| NFR-CI-010 | CI stages shall be introduced progressively. | §17 | MASTER-004 maps stages to milestones. |
| NFR-CI-011 | Release artifacts shall contain runtime binaries, assets, shader artifacts or compiler support, configuration, version info and user documentation. | §17 | Release checklist at M7. |
| NFR-CI-012 | Development, experimental and stable channels shall exist once the project is mature. | §17 | Deferred; revisit at M7. |

## ARCH - Architecture and dependencies
| ID | Requirement | Source § | How measured |
|---|---|---|---|
| NFR-ARCH-001 | Platform code shall not depend on the editor. | §5.2 r1 | cargo metadata dependency check in CI. |
| NFR-ARCH-002 | The RHI shall not depend on gameplay or scene systems. | §5.2 r2 | Dependency check. |
| NFR-ARCH-003 | The Vulkan backend shall implement the RHI; only rhi-vulkan shall depend on ash. | §5.2 r3, ADR-0001 | Dependency check. |
| NFR-ARCH-004 | The renderer shall not depend on editor implementation details. | §5.2 r4 | Dependency check. |
| NFR-ARCH-005 | Scene data shall not expose Vulkan resource objects. | §5.2 r5 | Review; scene crates do not depend on ash/rhi-vulkan. |
| NFR-ARCH-006 | The editor shall interact only through runtime and editor-facing interfaces. | §5.2 r6 | Dependency check: editor depends on runtime, not backend. |
| NFR-ARCH-007 | Resource lifetime shall be explicitly managed. | §5.2 r7, §22 p6 | Leak tests (NFR-REL-003); review. |
| NFR-ARCH-008 | Circular crate dependencies shall be prohibited. | §5.2 r8 | Cargo rejects cycles; CI cargo build. |
| NFR-ARCH-009 | Crates shall be introduced only when their layer spec is accepted. | §7, ADR-0002 | Review: new crate PR links accepted spec. |
| NFR-ARCH-010 | Dependencies shall follow docs/specs/layers/README.md. | ADR-0002 | Dependency check against that file. |
| NFR-ARCH-011 | Technology choices shall be revisited via ADR. | §6.4 | Review. |

