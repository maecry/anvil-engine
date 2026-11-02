# MODULE-010: Render World

| Field | Value |
|---|---|
| Status | Accepted (ADR-0011) |
| Crate | renderer |
| Layer | LAYER-005 |
| Milestone | M3 |
| Traces to | FR-REND-001, FR-REND-002, FR-REND-003, FR-REND-004, FR-REND-025, FR-RES-012, NFR-CONC-003, NFR-ARCH-004, NFR-ARCH-005; MASTER-000 §8.5, §9.1, §11; ADR-0005 |
| Last updated | 2026-11-02 |

## 1. Purpose

The renderer-owned copy of the scene: it receives immutable extracted data each frame, updates renderer-side state, and produces visibility results and draw lists that the passes consume. It is the boundary that keeps scene types and Vulkan types out of the renderer's hot data (MASTER-000 §5.2 rule 5, §11).

**M3 resolution (ADR-0011).** M3 has no persistent `RenderWorld`: the app rebuilds `RenderScene` (the former `ExtractedFrame`) every frame and `Renderer::render` consumes it directly. The responsibilities below that need persistent state (diff updates, culling, sorting) are deferred; M3a implements the extraction types, per-frame data upload and the fixed pass sequence (M3b adds the shadow pass, full PBR and the whole light array; see LAYER-005 §12a) described in LAYER-005 §8 and §12a.

## 2. Responsibilities

- Accept `RenderScene` (camera, mesh instances, lights; plain data, `Send`) (formerly `ExtractedFrame`).
- `RenderWorld::update(&mut self, frame: &ExtractedFrame)`: add/update/remove render-side instances keyed by renderer-neutral ids (`MeshId`, `MaterialId`).
- Visibility: frustum culling first (M3), later occlusion/LOD (M5).
- Sorting and batching into draw lists (by pipeline/material/depth).
- Per-frame uniform data (camera, lights) prepared as plain bytes for upload.
- Hand draw lists and data to pass implementations (forward in M3; the render graph from M4).
- Counters: instances, visible instances, draw calls, pipeline switches (as plain data).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Scene/ECS storage, transform hierarchy | scene (LAYER-010) |
| Asset loading and import | assets (LAYER-009) |
| GPU object creation policy | RHI via renderer owners; the render world stores handles only |
| Render graph compile | MODULE-009 |
| Shader/pipeline creation | MODULE-007/008 |
| Vulkan specifics | rhi-vulkan |
| Windowing and swapchain ownership (leaning: app owns, passes `SwapchainHandle`) | app |

## 4. Dependencies

Allowed: foundation, rhi (generic over `D: Device`), mesh, camera, material, shader (same-layer crates listed in LAYER-005 §3), render-graph from M4. Forbidden: rhi-vulkan, scene, assets, editor and any layer 4-5 crate (LAYER-005 §6). Math: `glam` until the math crate exists (LAYER-001).

## 5. Public API

Concept (LAYER-005 §8):

```rust
pub struct ExtractedFrame { pub camera: CameraData, pub meshes: Vec<MeshInstance>, pub lights: Vec<LightData> }
pub struct RenderWorld { /* instances, mesh/material tables, visibility scratch */ }
impl RenderWorld {
    pub fn new() -> Self;
    pub fn update(&mut self, frame: &ExtractedFrame);
    pub fn visible(&self) -> &VisibleSet;
    pub fn draw_lists(&self) -> &DrawLists;
    pub fn stats(&self) -> RenderWorldStats;       // plain data
}
```

As built in M3a (the concept above is superseded): `RenderScene { camera: Camera, meshes: Vec<MeshInstance>, lights: Vec<Light> }` is defined in the renderer crate; `Renderer::render(&mut self, device, cmd, slot, &RenderScene, &SwapchainImage)` takes it by reference. There is no `RenderWorld` type yet; see LAYER-005 §8 for the full API.

## 6. Data structures

As built: `MeshInstance { mesh: MeshId, material: MaterialId, transform: Mat4 }` (no flags yet); GPU side, `GpuInstance { model, normal_matrix, material }` (world matrices stored pre-multiplied, AoS, ADR-0011 §4), `GpuLight`, `GpuMaterial`, `GpuFrame` (M3b: plus the light view-projection, shadow texture/sampler slots and shadow light index) in `crates/renderer/src/gpu.rs`, std430, mirrored by `shaders/common.glsl` and checked by a size test. Originally:

`MeshInstance { mesh: MeshId, material: MaterialId, transform: Mat4, flags }`; `RenderMesh { vertex/index buffer handles, ranges, bounds }`; `RenderMaterial { pipeline handle(s), binding data }`; `VisibleSet { indices }`; `DrawList { sort_key: u64, instance, mesh, material }`. Struct-of-arrays versus array-of-structs is a data-layout decision to make with measurements. Stable ids are generational where entries can be removed (reuse of `Handle<T>`/`Pool<T>` is the default).

## 7. Ownership and lifetime

The render world owns its copy of instance data. GPU resources it refers to (buffers, textures, pipelines) are owned by the RHI pools and referenced by handle; the renderer destroys them via the RHI (deferred destruction applies). Extracted frames are borrowed for the duration of `update` and never stored by reference. The scene holds only `MeshId`/`MaterialId`, never RHI handles (FR-RES-012).

## 8. Execution flow

Per frame (MASTER-000 §9.1): app update -> scene update -> extract (`ExtractedFrame`) -> `RenderWorld::update` -> visibility -> sort/batch -> graph build/compile (M4; direct pass calls in M3) -> record -> submit -> present. Decided (ADR-0011 §2): a full rebuild every frame, not a diff; revisit when the ECS arrives or profiling shows a bottleneck.

## 9. Threading model

Extraction runs on the main thread; `update` and visibility are single-threaded in M3. Visibility, sorting and batching are parallelisable (M5, `task`/Rayon) because they read only immutable extracted data and write per-worker scratch (FR-REND-003, NFR-CONC-003). The render world must be `Send` so it can move to a render thread; no shared mutable scene state.

## 10. Error handling

`RenderError` (thiserror) wraps `RhiError`; `DeviceLost` passes through unchanged. Unknown `MeshId`/`MaterialId` in an extracted frame: skip the instance and `warn!` once per id (never panic). Missing GPU resources for an id are the renderer's loading concern, not the world's.

## 11. Performance requirements

Measure update time, visibility time, draw-call count, pipeline switches and instances per frame (NFR-PERF-001, NFR-PERF-006, NFR-PERF-007). No targets before benchmark workloads exist (NFR-PERF-012). Steady-state frames should reuse scratch allocations; verify with an allocation counter once the structure is fixed.

## 12. Testing requirements

Unit tests against `NullBackend` or no backend at all: add/update/remove semantics; id reuse safety; frustum culling against known boxes and edge cases (on-plane, behind, huge bounds); sort key ordering and batching; determinism (same input same draw list). Rendering tests (M3+): golden images for the first mesh/material scene with tolerance (NFR-TEST-003). Build check: `cargo tree -p renderer` shows no `ash`, no scene crate.

## 13. Acceptance criteria (M3)

- [x] A mesh with a texture and basic lighting renders via `Renderer` on `VulkanDevice` (`examples/pbr-scene`, M3a).
- [x] `renderer` has no `ash` or scene dependency (`cargo tree -p renderer`).
- [x] Extracted data is immutable during rendering (type-level: `&RenderScene`).
- [x] Unit tests: mesh winding/tangents/counts, camera reverse-Z and Y flip, GPU layout sizes.
- [ ] Culling and sorting unit tests, deterministic draw lists (deferred to M4, with visibility).
- [ ] Builds against a mock backend (pending).

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-REND-001 | frame lifecycle positions (section 8) |
| FR-REND-002 | render world fed by extraction, camera extraction |
| FR-REND-003, NFR-CONC-003 | immutable extracted data, no shared mutable scene state |
| FR-REND-004 | draw lists for mesh rendering with cameras and transforms |
| FR-REND-025, NFR-ARCH-004 | no editor or genre coupling |
| FR-RES-012, NFR-ARCH-005 | neutral ids, no Vulkan objects in scene data |

## 15. Open questions

1. Resolved (ADR-0011 §2): the renderer crate defines the extraction types.
2. ECS choice (own, `bevy_ecs`, `hecs`) affects extraction; deferred to LAYER-010.
3. Resolved (ADR-0011 §2): the app owns the swapchain and passes the acquired image.
4. Resolved (ADR-0010): bindless.
5. Resolved (ADR-0011 §1): `glam` re-exported as `foundation::math`; right-handed, Y-up, -Z forward, reverse-Z (clear 0, `GreaterOrEqual`, infinite far), Y flip folded into the projection.
6. Resolved (ADR-0011 §2): full rebuild per frame.
7. Resolved (ADR-0011 §4): AoS, world matrices pre-multiplied (plus a normal matrix).
8. How draw lists feed render-graph passes without the graph depending on renderer types (MODULE-009 Q7); the M3 pass functions take explicit input structs for this reason.
