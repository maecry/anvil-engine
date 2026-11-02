# MASTER-002: Functional Requirements
| Field | Value |
|---|---|
| Status | Accepted |
| Traces to | MASTER-000 §4.1, §4.2, §5, §8, §9, §10, §12, §14 |
| Last updated | 2026-11-02 |

Each requirement is testable and traces to MASTER-000, which prevails on conflict. Verification kinds follow MASTER-000 §15: unit, integration, rendering, manual.

Milestone marked `*` means MASTER-000 does not name the item in §14 explicitly; the milestone is derived from the nearest dependent deliverable and must be confirmed in the layer spec. Requirements describe the end state; "progressively" in MASTER-000 §4.1 means the milestone is when the capability first lands.

## PLAT - Platform and core
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-PLAT-001 | The engine shall provide an application lifecycle (init, run loop, shutdown) that exits cleanly without leaking GPU or OS resources. | §5.1 L1, §14 M1 | M1 | integration |
| FR-PLAT-002 | The platform layer shall create and manage an OS window and deliver window events (close, resize, focus). | §8.2, §20 W2 | M1 | integration |
| FR-PLAT-003 | The platform layer shall handle window resizing without crashing or producing validation errors. | §14 M1 | M1 | integration |
| FR-PLAT-004 | The platform layer shall expose window surface creation to the backend without exposing Vulkan types to non-backend crates. | §8.2, §5.2 r5 | M1 | integration |
| FR-PLAT-005 | The platform layer shall provide an input abstraction (keyboard, mouse). | §5.1 L1, §8.2 | M3* | manual |
| FR-PLAT-006 | The platform layer shall provide filesystem access used by asset and shader loading. | §5.1 L1 | M1* | unit |
| FR-PLAT-007 | foundation shall integrate logging through tracing; engine crates shall not print directly. | §6.1, §8.1, §20 W2 | M1 | unit |
| FR-PLAT-008 | foundation shall define common error types and report errors so that logs and errors are accessible to the user. | §8.1, §20 Month 1 | M1 | unit |
| FR-PLAT-009 | foundation shall provide time abstractions (frame time, delta time). | §5.1 L1, §8.1 | M1* | unit |
| FR-PLAT-010 | foundation shall provide engine configuration loadable from TOML or RON. | §8.1, §6.3 | M1* | unit |
| FR-PLAT-011 | foundation shall provide identifiers and diagnostics primitives. | §8.1 | M1* | unit |
| FR-PLAT-012 | The platform layer shall report platform capabilities. | §8.2 | M1* | unit |
| FR-PLAT-013 | The platform layer shall provide thread management for worker threads. | §5.1 L1, §11 | M5 | integration |
| FR-PLAT-014 | The engine shall build and run on Linux desktop. | §Header (Initial Platform), §20 | M1 | integration |
| FR-PLAT-015 | The engine shall support Windows and macOS, subject to backend availability. | §Header (Target Platforms) | M7* | integration |

## RHI - Rendering Hardware Interface
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-RHI-001 | The RHI shall provide a device interface and typed resource handles independent of any graphics API. | §8.3, §10.2 | M2 | unit |
| FR-RHI-002 | The RHI shall provide buffer management (create, map/upload, destroy). | §4.2 | M2 | integration |
| FR-RHI-003 | The RHI shall provide texture and image management. | §4.2 | M2 | integration |
| FR-RHI-004 | The RHI shall provide image views. | §4.2 | M2 | integration |
| FR-RHI-005 | The RHI shall provide samplers. | §4.2 | M2 | integration |
| FR-RHI-006 | The RHI shall provide shader modules. | §4.2 | M1* | integration |
| FR-RHI-007 | The RHI shall provide graphics pipelines. | §4.2, §20 W4 | M1* | integration |
| FR-RHI-008 | The RHI shall provide compute pipelines. | §4.2 | M2 | integration |
| FR-RHI-009 | The RHI shall provide pipeline layouts. | §4.2 | M2 | integration |
| FR-RHI-010 | The RHI shall provide descriptor or resource binding management. | §4.2, §14 M2 | M2 | integration |
| FR-RHI-011 | The RHI shall provide command recording. | §4.2, §8.3 | M1 | integration |
| FR-RHI-012 | The RHI shall provide queue management (graphics, compute, transfer). | §4.2, §11 | M1 | integration |
| FR-RHI-013 | The RHI shall provide a synchronization interface (fences, semaphores, barriers). | §4.2, §8.3 | M1 | integration |
| FR-RHI-014 | The RHI shall provide memory allocation for GPU resources. | §4.2, §14 M2 | M2 | integration |
| FR-RHI-015 | The RHI shall provide resource lifetime management. | §4.2, §5.2 r7 | M2 | integration |
| FR-RHI-016 | The RHI shall provide swapchain management. | §4.2 | M1 | integration |
| FR-RHI-017 | The RHI shall provide debug labels and GPU diagnostics. | §4.2 | M1* | manual |
| FR-RHI-018 | The RHI shall not depend on gameplay or scene systems. | §5.2 r2 | M1 | manual |
| FR-RHI-019 | RHI public types shall not expose backend-specific types unless unavoidable. | §22 p4, §5.2 r5 | M2 | manual |

## VK - Vulkan backend
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-VK-001 | The Vulkan backend shall create a Vulkan instance and enable validation layers in debug builds. | §8.4, §20 W3, ADR-0001 | M1 | integration |
| FR-VK-002 | The Vulkan backend shall enumerate and select a physical device. | §8.4, §14 M1 | M1 | integration |
| FR-VK-003 | The Vulkan backend shall select queue families and create a logical device. | §8.4, §20 W3 | M1 | integration |
| FR-VK-004 | The Vulkan backend shall create a window surface and a swapchain. | §14 M1 | M1 | integration |
| FR-VK-005 | The Vulkan backend shall recreate the swapchain on resize or out-of-date and verify it in tests. | §14 M1, §15.2 | M1 | integration |
| FR-VK-006 | The Vulkan backend shall manage command pools and command buffers. | §8.4, §20 W4 | M1 | integration |
| FR-VK-007 | The Vulkan backend shall implement frame synchronization with a defined number of frames in flight. | §14 M1, §20 W4 | M1 | integration |
| FR-VK-008 | The Vulkan backend shall implement resource barriers. | §8.4 | M2 | integration |
| FR-VK-009 | The Vulkan backend shall allocate GPU memory via gpu-allocator or VMA. | §6.2, §8.4 | M2 | integration |
| FR-VK-010 | The Vulkan backend shall manage descriptors. | §8.4 | M2 | integration |
| FR-VK-011 | The Vulkan backend shall create Vulkan pipelines, and a pipeline cache. | §8.4, §14 M5 | M5 | integration |
| FR-VK-012 | The Vulkan backend shall render a triangle with no known validation errors in the baseline test. | §14 M1, §20 Month 1 | M1 | rendering |
| FR-VK-013 | The Vulkan backend shall implement the RHI rather than define the engine API; only it shall depend on ash. | §5.2 r3, ADR-0001 | M1 | manual |
| FR-VK-014 | The Vulkan backend shall handle device loss where supported. | §14 M7, §15.5 | M7 | integration |

## REND - Renderer
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-REND-001 | The renderer shall implement the frame lifecycle of §9.1 (update, extraction, render world update, visibility, graph construction and compilation, recording, submission, presentation), progressively. | §9.1 | M3 | integration |
| FR-REND-002 | The renderer shall maintain a render world fed by scene extraction, and camera extraction. | §8.5, §9.1 | M3 | integration |
| FR-REND-003 | The renderer shall consume immutable extracted render data and shall not share mutable scene state across workers. | §11 | M3 | manual |
| FR-REND-004 | The renderer shall render meshes with cameras and transforms. | §14 M3 | M3 | rendering |
| FR-REND-005 | The renderer shall support textures and materials. | §14 M3 | M3 | rendering |
| FR-REND-006 | The renderer shall support PBR metallic-roughness materials. | §4.1, §14 M3 | M3 | rendering |
| FR-REND-007 | The renderer shall support directional, point and spot lights. | §4.1 | M3 | rendering |
| FR-REND-008 | The renderer shall support forward rendering with depth testing. | §4.1, §9.2 | M3 | rendering |
| FR-REND-009 | The renderer shall support shadow mapping. | §4.1, §14 M3 | M3 | rendering |
| FR-REND-010 | The renderer shall support image-based (environment) lighting. | §4.1, §14 M3 | M3 | rendering |
| FR-REND-011 | The renderer shall support HDR rendering and tone mapping. | §4.1 | M4* | rendering |
| FR-REND-012 | The renderer shall support bloom. | §4.1 | M4* | rendering |
| FR-REND-013 | The renderer shall support a depth prepass. | §4.1 | M4* | rendering |
| FR-REND-014 | The renderer shall support deferred rendering. | §4.1, §14 M4 | M4 | rendering |
| FR-REND-015 | The renderer shall support clustered lighting. | §4.1, §14 M4 | M4 | rendering |
| FR-REND-016 | The renderer shall support screen-space ambient occlusion. | §4.1, §14 M4 | M4 | rendering |
| FR-REND-017 | The renderer shall support temporal anti-aliasing. | §4.1, §14 M4 | M4 | rendering |
| FR-REND-018 | The renderer shall support post-processing as a graph of passes. | §14 M4 | M4 | rendering |
| FR-REND-019 | The renderer shall support compute workloads. | §4.1 | M5* | integration |
| FR-REND-020 | The renderer shall support frustum culling and occlusion culling. | §4.1, §14 M5 | M5 | rendering |
| FR-REND-021 | The renderer shall support GPU-driven rendering with indirect drawing and GPU culling. | §4.1, §14 M5 | M5 | rendering |
| FR-REND-022 | The renderer shall support level of detail. | §4.1, §14 M5 | M5 | rendering |
| FR-REND-023 | The renderer shall support multithreaded render preparation (visibility, sorting, batching, graph construction). | §2.1 o8, §11, §14 M5 | M5 | integration |
| FR-REND-024 | The renderer shall support texture streaming. | §4.1, §14 M5 | M5 | integration |
| FR-REND-025 | The renderer shall not depend on editor implementation details and shall not be coupled to a game genre. | §5.2 r4, §3 | M3 | manual |

## RG - Render graph
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-RG-001 | The render graph shall support pass registration. | §9.3 | M4 | unit |
| FR-RG-002 | The render graph shall support resource declarations by passes. | §9.3 | M4 | unit |
| FR-RG-003 | The render graph shall track dependencies between passes. | §9.3 | M4 | unit |
| FR-RG-004 | The render graph shall validate read/write usage and reject invalid graphs with a diagnostic. | §9.3 | M4 | unit |
| FR-RG-005 | The render graph shall compute an execution order (scheduling). | §9.3, §14 M4 | M4 | unit |
| FR-RG-006 | The render graph shall plan resource transitions between passes. | §9.3, §14 M4 | M4 | integration |
| FR-RG-007 | The render graph shall manage transient resources. | §9.3, §10.3 | M4 | integration |
| FR-RG-008 | The render graph shall provide debug visualization and diagnostics. | §9.3, §8.6 | M4 | manual |
| FR-RG-009 | The render graph shall be an execution system and not only a collection of pass callbacks. | §9.3 | M4 | manual |

## RES - GPU resource management
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-RES-001 | Resources shall be referenced by typed handles containing index and generation. | §10.2 | M2 | unit |
| FR-RES-002 | The registry shall detect stale handles. | §10.2 | M2 | unit |
| FR-RES-003 | The registry shall prevent accidental reuse of a resource slot by a stale handle. | §10.2 | M2 | unit |
| FR-RES-004 | Resource ownership shall be centralized. | §10.2, §5.2 r7 | M2 | manual |
| FR-RES-005 | The registry shall track resource state. | §10.2 | M2 | unit |
| FR-RES-006 | Resources shall be destroyed through deferred destruction so that in-flight GPU work never references a destroyed resource. | §10.2, §10.3 | M2 | integration |
| FR-RES-007 | The engine shall expose memory diagnostics and statistics. | §10.2, §10.3 | M2 | integration |
| FR-RES-008 | The engine shall provide buffer suballocation. | §10.3 | M2 | integration |
| FR-RES-009 | The engine shall provide image allocation. | §10.3 | M2 | integration |
| FR-RES-010 | The engine shall provide staging buffers and upload scheduling. | §10.3 | M2 | integration |
| FR-RES-011 | The engine shall provide transient render resources. | §10.3 | M4 | integration |
| FR-RES-012 | Scene data shall not directly expose Vulkan resource objects. | §5.2 r5 | M3 | manual |

## SHD - Shader system
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-SHD-001 | The shader system shall compile GLSL to SPIR-V (shaderc, subject to ADR-0003). | §6.2, §8.7, §20 W4 | M1 | integration |
| FR-SHD-002 | The shader system shall manage shader sources. | §8.7 | M1* | unit |
| FR-SHD-003 | The shader system shall provide reflection data. | §8.7 | M2* | unit |
| FR-SHD-004 | The shader system shall check shader and pipeline compatibility. | §8.7 | M2* | unit |
| FR-SHD-005 | The shader system shall provide a shader cache. | §8.7 | M5* | integration |
| FR-SHD-006 | The shader system shall support hot reload. | §8.7, §15.5 | M6* | integration |
| FR-SHD-007 | The shader system shall report compilation errors with source location. | §8.7 | M1* | unit |
| FR-SHD-008 | CI shall include a shader compilation step. | §17 item 5 | M1* | integration |
| FR-SHD-009 | Shader compatibility shall be tested. | §14 M7 | M7 | integration |

## AST - Asset system
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-AST-001 | The asset system shall discover assets. | §12 (1) | M6 | integration |
| FR-AST-002 | The asset system shall assign stable asset identifiers. | §12 (2), §15.1 | M6 | unit |
| FR-AST-003 | The asset system shall import assets through an import pipeline separate from runtime GPU resource management. | §12 (3), §12 | M6 | integration |
| FR-AST-004 | The asset system shall validate imported assets and handle corrupt assets without crashing. | §12 (4), §14 M7 | M6 | integration |
| FR-AST-005 | The asset system shall generate metadata. | §12 (5), §8.8 | M6 | unit |
| FR-AST-006 | The asset system shall track asset dependencies. | §12 (6) | M6 | unit |
| FR-AST-007 | The asset system shall load assets at runtime and provide asset handles. | §12 (7), §8.8 | M6 | integration |
| FR-AST-008 | The asset system shall upload loaded assets to the GPU. | §12 (8) | M6 | integration |
| FR-AST-009 | The asset system shall cache resources. | §12 (9), §8.8 | M6 | integration |
| FR-AST-010 | The asset system shall support hot reload. | §12 (10) | M6 | integration |
| FR-AST-011 | The asset system shall support streaming. | §12 (11), §14 M5 | M5 | integration |
| FR-AST-012 | The asset system shall version asset formats. | §12 (12) | M6 | unit |
| FR-AST-013 | The asset system shall support glTF 2.0, PNG/JPEG, EXR, KTX2, and engine-defined scene and material formats. | §12, §6.3 | M6 | integration |

## SCN - Scene and runtime
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-SCN-001 | The scene system shall represent scenes and a transform hierarchy. | §8.9 | M6 | unit |
| FR-SCN-002 | The scene system shall provide camera, mesh and light components. | §8.9 | M6 | unit |
| FR-SCN-003 | The scene system shall serialize and deserialize scenes. | §8.9, §14 M6 | M6 | unit |
| FR-SCN-004 | The runtime shall provide runtime lifecycle and application-facing interfaces. | §5.1 L4 | M6 | integration |
| FR-SCN-005 | The runtime shall provide entity and component systems and camera systems. | §5.1 L4 | M6 | integration |
| FR-SCN-006 | A sample project shall be creatable, saveable, reopenable and executable. | §14 M6 | M6 | manual |
| FR-SCN-007 | Public interfaces shall be stable at release. | §14 M7 | M7 | manual |

## TOOL - Tooling
| ID | Requirement | Source § | Milestone | Verification |
|---|---|---|---|---|
| FR-TOOL-001 | The editor shall provide a scene viewport. | §8.12, §14 M6 | M6 | manual |
| FR-TOOL-002 | The editor shall provide a scene hierarchy and an inspector. | §8.12 | M6 | manual |
| FR-TOOL-003 | The editor shall provide an asset browser. | §8.12 | M6 | manual |
| FR-TOOL-004 | The editor shall provide material editing tools. | §8.12, §14 M6 | M6 | manual |
| FR-TOOL-005 | The editor shall provide debug panels and debug visualization. | §8.12, §14 M6 | M6 | manual |
| FR-TOOL-006 | The editor shall interact with the engine only through runtime and editor-facing interfaces. | §5.2 r6 | M6 | manual |
| FR-TOOL-007 | The debug crate shall provide overlays, resource visualization, render graph visualization, GPU diagnostics, rendering statistics and debug geometry. | §8.10 | M5* | manual |
| FR-TOOL-008 | The profiler shall provide CPU profiling, GPU timestamps, memory tracking, frame timing, performance statistics and exports. | §8.11, §14 M5 | M5 | integration |
| FR-TOOL-009 | The release shall include runtime binaries, required assets, shader artifacts or compiler support, runtime configuration, version information and user documentation. | §17, §14 M7 | M7 | manual |

## Proposed additions (not yet accepted)
| # | Proposal | Reason |
|---|---|---|
| P1 | Define a minimum Vulkan version and required device features/extensions (e.g. synchronization2, dynamic rendering, timeline semaphores). | MASTER-000 §6.2 names Vulkan only; ADR-0001 mentions timeline semaphores/bindless without committing. |
| P2 | Define the number of frames in flight. | FR-VK-007 needs a value; MASTER-000 only says "frame in flight" implicitly (§10.3). |
| P3 | Define behavior when no suitable physical device exists (error, not panic). | §16 "Avoid silent failures". |
| P4 | Specify transparency/alpha handling in the renderer. | §15.3 tests transparency, but §4.1 does not list it. |
| P5 | Specify screenshot/readback API for rendering tests. | §15.3 needs image comparison; no capture requirement exists. |
| P6 | Specify headless (no window) rendering mode. | Needed for GPU CI runners (§17 item 7). |
| P7 | Assign milestones to ray tracing, global illumination, virtual texturing (§9.2 Experimental) or declare them out of M0-M7. | Not in any milestone. |
| P8 | Assign a milestone to the egui debug UI (§6.4). | Needed by FR-TOOL-007 before M6. |
| P9 | Add a milestone for input, filesystem, Windows/macOS support. | Milestones marked `*` above. |
