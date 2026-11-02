# MASTER-001: Project Vision
| Field | Value |
|---|---|
| Status | Accepted |
| Traces to | MASTER-000 §1, §2, §3, §23 |
| Last updated | 2026-11-02 |

## 1. Vision (MASTER-000 §1.1, §1.2)
Anvil Engine is a modular 3D game engine in Rust, renderer-first, using Vulkan on Linux first. The goal is a production-quality rendering foundation that can grow into a full engine without fundamental rewrites. The first objective is the foundation, not a complete game engine (§23).

## 2. Philosophy (MASTER-000 §1.3, §22)
| Principle | Meaning for contributors |
|---|---|
| Rendering first | Other subsystems follow renderer requirements. |
| Performance by design | Memory, synchronization and scalability are architecture inputs; optimization still requires measurement (§22 p3). |
| Modular architecture | Clear responsibilities, explicit interfaces, enforced dependency rules (MASTER-003 NFR-ARCH). |
| Rust-native | Use ownership and types; `unsafe` isolated, mainly in `rhi-vulkan`. |
| Explicit GPU management | Lifetime, synchronization, allocation and command execution are designed, not implicit. |
| Documentation first | Research, ADR, layer spec, module spec, then code (§13). |
| Observable systems | CPU, GPU, memory, rendering and resource lifetime are diagnosable. |

## 3. Objectives (MASTER-000 §2.1, §2.2)
- Primary: Vulkan backend, RHI, scalable renderer, render graph, GPU resource and memory management, PBR, forward and deferred, multithreaded preparation, GPU-driven rendering, debugging and profiling.
- Secondary: asset management, scene and material systems, common 3D formats, extensible runtime, editor, additional backends where justified, application API.
- Requirements derived from these: MASTER-002 (functional), MASTER-003 (non-functional).

## 4. Target applications (MASTER-000 §3)
Real-time 3D games (first/third person, RPG, action), procedural environments, large-scale scenes, voxel environments, technical visualization, interactive graphics. The renderer must not be coupled to a genre.

## 5. Scope
| In scope | Source |
|---|---|
| Vulkan RHI and backend, resource management | §4.2, §8.3-8.4, §10 |
| Renderer, render graph, shader system | §4.1, §8.5-8.7, §9 |
| Asset pipeline, scene, runtime, editor, profiling | §8.8-8.12, §12 |
| Linux first; Windows and macOS subject to backend availability | Header |
| Milestones M0-M7 | §14 (see MASTER-004) |

## 6. Non-goals (MASTER-000 §2.3)
Not prioritized initially: visual scripting, multiplayer networking, custom physics engine, complete animation authoring, mobile rendering, every graphics API, universal gameplay framework, replicating the full feature set of Unreal or Godot. These may be reconsidered once the rendering foundation is stable. Experimental items (ray tracing, global illumination, virtual texturing, §9.2) have no milestone yet (MASTER-002 P7).

## 7. Glossary
| Term | Definition |
|---|---|
| RHI | Rendering Hardware Interface: the graphics-API-neutral device, resource, pipeline, command and sync interface (`rhi`, §8.3). |
| Backend | An RHI implementation; initially Vulkan (`rhi-vulkan`). |
| Render graph | Declared passes and their resource reads/writes, compiled into execution order, barriers and transient allocations (§9.3). |
| Render world | Renderer-owned copy of scene data used for rendering, updated from extraction (§9.1). |
| Extraction | Copying the scene state needed for rendering into immutable render data, decoupling scene and renderer (§9.1, §11). |
| Visibility | Determining which objects are rendered for a view (culling, sorting). |
| Frame in flight | A frame whose CPU recording is finished but whose GPU execution may be incomplete; resources it uses must stay alive. |
| Deferred destruction | Delaying destruction of a resource until GPU work that may reference it has completed (§10.3). |
| Transient resource | A render-graph resource whose lifetime is limited to a frame (or part of it) and may share memory with others. |
| Typed handle | Index plus generation identifying a resource; stale handles are detectable (§10.2). |
| Staging buffer | CPU-visible buffer used to upload data to GPU-local memory. |
| Suballocation | Carving many small buffers from one large GPU allocation. |
| Pipeline cache | Reuse of compiled pipeline state across runs. |
| Barrier | Vulkan synchronization/layout-transition command between GPU operations. |
| Swapchain | Set of presentable images tied to a window surface. |
| SPIR-V | Binary shader format consumed by Vulkan; produced from GLSL (§6.2). |
| PBR | Physically based rendering, metallic-roughness model. |
| IBL | Image-based lighting. |
| Forward / Deferred | Shade during geometry pass / shade from a G-buffer in a later pass. |
| Clustered lighting | Light assignment to 3D view-frustum cells. |
| SSAO / TAA | Screen-space ambient occlusion / temporal anti-aliasing. |
| GPU-driven rendering | GPU performs culling and generates draw commands (indirect drawing). |
| LOD | Level of detail. |
| ADR | Architecture Decision Record (§13.3). |
| MASTER / LAYER / MODULE | Specification hierarchy levels (§13.1). |
