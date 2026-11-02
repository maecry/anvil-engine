# LAYER-005: Renderer

| Field | Value |
|---|---|
| Status | Draft |
| Milestone | M3 (forward, PBR); M4 (deferred, post) |
| Crates | renderer, mesh, camera |
| Traces to | MASTER-000 §4.1, §5.2 rules 4-5, §8.5, §9, §11 |
| Last updated | 2026-11-02 |

## 1. Purpose

Turn extracted scene data into GPU work: render world, visibility, mesh submission, materials binding, lighting, post-processing. Generic over `D: rhi::Device` (ADR-0005).

## 2. Scope

| In | Out |
|---|---|
| Render world (renderer-owned copy of extracted scene data) | Scene/ECS storage (LAYER-010) |
| Extraction input types, visibility, sorting, batching | Asset loading/import (LAYER-009) |
| Mesh, camera types (mesh, camera) | Vulkan specifics (LAYER-004) |
| Pass implementations (forward, depth prepass, shadows, later deferred/post) | Editor UI |
| Frame orchestration per MASTER-000 §9.1 | Render graph compiler (LAYER-006) |

## 3. Crates

| Crate | Introduced | Notes |
|---|---|---|
| renderer | M3 | depends on rhi, render-graph (from M4; before that direct pass calls), shader, mesh, material, camera |
| mesh | M3 | CPU mesh data + vertex layout descriptions; GPU upload through RHI |
| camera | M3 | projection/view data; depends on foundation (+ math when it exists) |
| material | M3 | parameter sets and shader binding (see LAYER-007/008) |

## 4. Responsibilities / 5. Non-responsibilities

- Responsible: render world updates from immutable extracted data; visibility; draw list building; per-frame uniform data; pass execution; presenting through the RHI.
- Not responsible: owning scene entities, importing assets, device creation (injected), windowing, profiling UI (emits labels/scopes only, see README §2.4).

## 6. Dependencies

Allowed: layers 1-2 via `rhi` only, same-layer crates listed above. Forbidden: rhi-vulkan, scene, assets, editor and any layer 4-5 crate (rules 4, 5).

## 7. Key concepts and data flow

```
 Scene (layer 4) --extract--> ExtractedFrame (immutable, plain data, Send)
                                  |
                          RenderWorld::update
                                  |
                 visibility -> draw lists -> render graph build/compile -> record -> submit -> present
```

Extraction boundary: scene crate produces `ExtractedFrame` defined in the renderer crate (the scene depends on the renderer's input types, not the reverse). Open question 1.

## 8. Public interface sketch (concept)

```rust
pub struct Renderer<D: Device> { /* device handle, render world, pass set */ }
impl<D: Device> Renderer<D> {
    pub fn new(device: D, config: RendererConfig) -> Result<Self, RenderError>;
    pub fn render(&mut self, frame: &ExtractedFrame, target: SwapchainHandle) -> Result<(), RenderError>;
}
pub struct ExtractedFrame { pub camera: CameraData, pub meshes: Vec<MeshInstance>, pub lights: Vec<LightData> }
```

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: renderer owns GPU resources through RHI handles and destroys them via the RHI; scene holds only renderer-neutral ids (`MeshId`, `MaterialId`).
- Threading: extraction on the main thread; visibility/sorting parallelisable on workers (task, M5); recording single-threaded until M5. Prefer immutable extracted data (MASTER-000 §11).
- Errors: `RenderError` (thiserror) wrapping `RhiError`; `DeviceLost` passes through unchanged.
- Diagnostics: debug labels per pass, GPU scopes via RHI hooks, draw-call/pipeline-switch counters as plain data.

## 13. Testing strategy

Unit tests for visibility/sorting with the RHI `NullBackend`; golden-image rendering tests (MASTER-000 §15.3) from M3; performance counters from M5.

## 14. Acceptance criteria (Draft, M3)

- [ ] Mesh with texture and basic lighting renders via `Renderer<VulkanDevice>` with no `ash` in the dependency tree of renderer.
- [ ] Renderer builds against `NullBackend`.
- [ ] No scene crate dependency.

## 15. Open questions

1. Who defines extraction types: renderer (scene depends on it) or a neutral crate? Needs ADR before M3.
2. ECS choice (own, bevy_ecs, hecs) affects extraction; deferred to LAYER-010.
3. Does the renderer own the swapchain or does the app pass `SwapchainHandle`? (Leaning: app owns, passes it.)
4. Bindless vs descriptor-set-per-material: decide at M2 (LAYER-007).
5. Forward+ vs clustered timing; math conventions (LAYER-001 Q1).

## 16. Related modules

MODULE-010-render-world.
