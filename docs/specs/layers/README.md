# Layer Specifications: Index and Dependency Rules

| Field | Value |
|---|---|
| Status | Review |
| Milestone | M0-M1 |
| Crates | all |
| Traces to | MASTER-000 §5, §7, §13.1, §22 |
| Last updated | 2026-11-02 |

Layer specs refine MASTER-000 and must not contradict it (§13.5 applies to any change). Dependency rules here are referenced by ADR-0002 and enforced in review and, later, by CI (`cargo metadata` check; see Open questions).

## 1. Index

Spec file numbers are a reading order and do not equal architecture layer numbers (MASTER-000 §5.1).

| Spec | Title | Arch. layer | Crates | Milestone | Status |
|---|---|---|---|---|---|
| [LAYER-001](LAYER-001-foundation.md) | Foundation | 1 | foundation (math, task deferred) | M1 | Accepted |
| [LAYER-002](LAYER-002-platform.md) | Platform | 1 | platform | M1 | Accepted |
| [LAYER-003](LAYER-003-rhi.md) | RHI | 2 | rhi | M1 (grows M2, M3) | Accepted |
| [LAYER-004](LAYER-004-vulkan-backend.md) | Vulkan backend | 2 | rhi-vulkan | M1 | Accepted |
| [LAYER-005](LAYER-005-renderer.md) | Renderer | 3 | renderer | M3 | Accepted |
| [LAYER-006](LAYER-006-render-graph.md) | Render graph | 3 | render-graph | M4 | Draft |
| [LAYER-007](LAYER-007-resource-management.md) | Resource management | 2-3 | cross-cutting (rhi, rhi-vulkan, material) | M2 | Draft |
| [LAYER-008](LAYER-008-shader-system.md) | Shader system | 3 | shader | M2-M5 | Draft |
| [LAYER-009](LAYER-009-asset-system.md) | Asset system | 4 | assets | M6 | Draft |
| [LAYER-010](LAYER-010-runtime-and-editor.md) | Runtime and tools | 4-5 | scene, runtime, debug, profiler, editor | M6 | Draft |

Module specs (to be written under `docs/specs/modules/`, each after its layer spec is accepted):

| Module | Owning layer spec |
|---|---|
| MODULE-001-vulkan-instance, 002-vulkan-device, 003-vulkan-swapchain, 004-command-manager | LAYER-004 |
| MODULE-005-buffer-allocator, 006-texture-registry | LAYER-007 |
| MODULE-007-pipeline-cache | LAYER-008 (cache) / LAYER-007 (GPU object) |
| MODULE-008-shader-compiler | LAYER-008 |
| MODULE-009-render-graph-compiler | LAYER-006 |
| MODULE-010-render-world | LAYER-005 |

Crates are introduced incrementally (ADR-0002). Only foundation, platform, rhi, rhi-vulkan and examples/triangle exist today.

ADRs cited by these specs (all Accepted, see `docs/decisions/ADR/README.md`): ADR-0003 shaderc (system lib), ADR-0004 Vulkan 1.3 baseline, ADR-0005 static-generic RHI dispatch, ADR-0006 typed generational handles, ADR-0007 `gpu-allocator`, ADR-0008 crate naming.

## 2. Dependency rules

### 2.1 MASTER-000 §5.2 (verbatim)

1. Platform code must not depend on the editor.
2. The RHI must not depend on gameplay or scene systems.
3. The Vulkan backend must implement the RHI rather than define the entire engine API.
4. The renderer must not depend on editor implementation details.
5. Scene data must not directly expose Vulkan resource objects.
6. The editor must interact through runtime and editor-facing interfaces.
7. Resource lifetime must be explicitly managed.
8. Circular crate dependencies are prohibited.

### 2.2 Allowed-dependency table

A crate may depend only on its own or a lower layer.

| Layer | Crates | May depend on |
|---|---|---|
| 1 Foundation | foundation, math, task, platform | lower crates in layer 1; foundation depends on no anvil crate |
| 2 RHI | rhi, rhi-vulkan | layer 1; rhi-vulkan -> rhi, never reverse; only rhi-vulkan may depend on `ash` |
| 3 Rendering | shader, render-graph, renderer, mesh, material, camera | layers 1-2, but only rhi (never rhi-vulkan) |
| 4 Runtime | assets, scene, runtime | layers 1-3 |
| 5 Tools | debug, profiler, editor | layers 1-4 |

Applications and examples choose the backend and inject it (they are the only places that name `rhi-vulkan` together with higher layers).

### 2.3 Clarifications (derived from the rules; not new policy)

- "Lower crates in layer 1" is an order: foundation < math, task < platform. platform may use foundation; it must not use `ash` (rule from the technical direction: the Vulkan backend creates the surface from raw window handles).
- Third-party restrictions: `ash`, `ash-window`, `gpu-allocator` only in rhi-vulkan; `winit` only in platform; `egui` only in layer 5; `shaderc` only in shader, plus build-dependencies of examples (ADR-0003).
- Within one layer, crates must not form cycles (rule 8). Same-layer dependencies are allowed only when listed in the owning layer spec.
- Rule 5 / MASTER-000 §22.4: no `ash::vk` type in the public API of any crate except rhi-vulkan, and not even there beyond what its own tests need.

### 2.4 Known tension: instrumentation lives in layer 5, renderer needs it in layer 3

The renderer (layer 3) must emit GPU timestamps and debug labels, but profiler and debug are layer 5, and layer 3 may not depend on layer 5.

Resolution: low-level instrumentation hooks live in rhi and foundation. Layer 5 crates only consume and display the data.

| Hook | Lives in | Consumed by |
|---|---|---|
| Debug labels (`begin_label`/`end_label` on the command encoder) | rhi (trait), rhi-vulkan (VK_EXT_debug_utils) | RenderDoc, debug |
| GPU timestamp scopes (query pool, resolved per frame into plain-data `GpuScopeSample`) | rhi (`Device` pull API), sample type in foundation | profiler |
| CPU spans and logs | `tracing` (foundation re-exports), subscribers installed by the app | profiler (Tracy layer), logging |
| Memory / resource statistics (`MemorySnapshot`) | rhi pull API, type in foundation | profiler, debug |

Rules: hooks are pull-based or `tracing`-based (no layer-5 callbacks injected downward); they are cheap when nothing consumes them; layer 5 never reaches into backends.

## 3. Common conventions for all specs

- Section order follows MASTER-000 §13.4 plus Scope, Diagnostics and Open questions.
- "verify:" marks an API detail not yet checked against upstream docs.
- Layer specs describe boundaries and interface sketches; exact signatures are fixed in module specs.

## 4. Open questions

- Automated dependency check (cargo-deny bans plus a small `cargo metadata` script) in CI: owner and milestone.
- Whether math/task are introduced at M2 or later (LAYER-001).
