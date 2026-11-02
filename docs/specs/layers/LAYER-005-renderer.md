# LAYER-005: Renderer

| Field | Value |
|---|---|
| Status | Accepted (ADR-0011) |
| Milestone | M3 (forward, PBR; M3a implemented); M4 (graph, post) |
| Crates | renderer (M3: mesh, camera and material live inside it as modules) |
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
| renderer | M3 | depends on foundation (`foundation::math` = glam) and rhi; render-graph from M4 (M3 uses direct pass functions) |

M3 decision (ADR-0011): mesh, camera, material and the extraction types are modules of the single `renderer` crate (`mesh`, `scene`, `gpu`, `passes`, `renderer`), not separate crates. Split them when a second consumer appears.

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

Extraction boundary (ADR-0011 §2): the renderer crate defines the plain-data `RenderScene`; the app (later a scene/ECS layer) rebuilds it every frame (full rebuild, no diff). The renderer never depends on scene types. M3a has no visibility or sorting step: every instance is drawn in submission order.

## 8. Public interface sketch (concept)

Implemented in M3a (`crates/renderer`). `Renderer` is not generic itself; its methods are generic over `D: rhi::Device` (static dispatch, ADR-0005) and take the device explicitly, so the app keeps ownership of `Device` and `SwapchainHandle`.

```rust
pub struct RenderScene { pub camera: Camera, pub meshes: Vec<MeshInstance>, pub lights: Vec<Light> } // environment arrives in M3c
pub struct Camera { pub position: Vec3, pub rotation: Quat, pub fov_y: f32, pub near: f32, pub exposure: f32 }
impl Camera { pub fn view(&self) -> Mat4; pub fn projection(&self, aspect: f32) -> Mat4 /* reverse-Z, infinite far, Y flip */ }
pub struct MeshInstance { pub mesh: MeshId, pub material: MaterialId, pub transform: Mat4 }
pub enum Light { Directional { direction, color, intensity }, Point { position, color, intensity, range },
                 Spot { position, direction, color, intensity, range, inner_cone, outer_cone } }

pub struct Vertex { pub pos: [f32; 3], pub normal: [f32; 3], pub uv: [f32; 2], pub tangent: [f32; 4] } // 48 B
pub struct MeshData { pub vertices: Vec<Vertex>, pub indices: Vec<u32> }
pub mod mesh { pub fn cube() -> MeshData; pub fn uv_sphere(segments: u32, rings: u32) -> MeshData; pub fn plane(size: f32) -> MeshData; }

pub struct MaterialDesc<'a> { label, base_color, base_color_texture, metallic, roughness, metallic_roughness_texture,
                              normal_texture, normal_scale, occlusion_texture, occlusion_strength, emissive, emissive_texture }
impl Renderer {
    pub fn new<D: Device>(device: &D, swapchain_format: Format, extent: (u32, u32)) -> Result<Self, RhiError>;
    pub fn resize<D: Device>(&mut self, device: &D, extent: (u32, u32)) -> Result<(), RhiError>; // also automatic in render
    pub fn create_mesh<D: Device>(&mut self, device: &D, label: &str, data: &MeshData) -> Result<MeshId, RhiError>;
    pub fn create_texture<D: Device>(&mut self, device: &D, label: &str, rgba8: &[u8], w: u32, h: u32, srgb: bool) -> Result<TextureId, RhiError>;
    pub fn create_material(&mut self, desc: &MaterialDesc) -> MaterialId;
    pub fn destroy_mesh / destroy_texture / destroy_material(..);
    pub fn render<D: Device>(&mut self, device: &D, cmd: &mut D::Commands, slot: u32, scene: &RenderScene, target: &SwapchainImage) -> Result<(), RhiError>;
    pub fn stats(&self) -> RendererStats;   // draws, triangles, lights, ring_bytes_used
    pub fn destroy<D: Device>(self, device: &D);
}
```
`render` records into the app's encoder: it leaves `target` in `ColorAttachment`; the app transitions it to `Present` (or `TransferSrc` for `ANVIL_SCREENSHOT`). `RenderError` is not introduced in M3: `RhiError` is returned directly (`InvalidDescriptor` for an upload-ring overflow).

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: renderer owns GPU resources (pipelines, HDR and depth targets, samplers, built-in 1x1 textures, per-frame rings, meshes, textures) through RHI handles and destroys them via the RHI; the app and scene hold only renderer-neutral ids (`MeshId`, `TextureId`, `MaterialId`, generational `foundation::Handle`s). No Vulkan objects are exposed (FR-RES-012).
- Threading: extraction on the main thread; visibility/sorting parallelisable on workers (task, M5); recording single-threaded until M5. Prefer immutable extracted data (MASTER-000 §11).
- Errors: M3 returns `RhiError` directly (`DeviceLost` passes through unchanged); a `RenderError` wrapper is deferred until the renderer has errors of its own.
- Diagnostics: debug labels per pass (`forward`, `tonemap`), `tracing` `debug` on every mesh/texture/material create and destroy with its label, and `RendererStats { draws, triangles, lights, ring_bytes_used }` returned by `stats()` and logged once per second at `debug` (ADR-0011 §11). `ANVIL_SCREENSHOT` is implemented by the example, not the renderer.

## 12a. Frame structure and GPU data (M3a, ADR-0011 §3-5)

1. Forward opaque pass: HDR `Rgba16Float` color + reverse-Z `D32Sfloat` depth (clear 0, `GreaterOrEqual`, back-face culling, counter-clockwise front faces).
2. Tonemap pass: fullscreen triangle, ACES fit and exposure, HDR to the swapchain (manual gamma only if the swapchain format is not sRGB).

Each pass is a free function over an explicit inputs struct (`forward_pass(cmd, &ForwardInputs)`, `tonemap_pass(cmd, &TonemapInputs)`), so M4 can turn them into graph nodes. Barriers are hand-written; HDR/depth start `Undefined` on the first frame and `ShaderRead`/`DepthAttachment` afterwards.

Per frame the renderer fills one persistently mapped `Upload` ring per frame in flight (4 MiB each, bump allocated, reset when the slot is reused) with frame data (view, proj, view_proj, camera position, light count, sampler index, exposure), the instance array (model + normal matrix + material index), the light array and the dense material array. Shaders read them through buffer device addresses in the push constants (5 addresses = 40 of 128 bytes: frame, instances, lights, materials, vertices); `first_instance` of each indexed draw is the instance index. Meshes are GPU-only vertex (BDA) and index buffers.

M3a shading is deliberately simple: base color texture x factor, Lambert plus Blinn-Phong from the first directional light, hemisphere ambient, normal map, occlusion and emissive. Point and spot lights are uploaded but unused until M3b (PBR over the full light array, directional shadow map). M3c adds IBL.

## 13. Testing strategy

Unit tests for visibility/sorting with the RHI `NullBackend`; golden-image rendering tests (MASTER-000 §15.3) from M3; performance counters from M5.

## 14. Acceptance criteria (M3)

M3a (implemented):
- [x] Textured meshes with basic lighting render through `Renderer` driven by `examples/pbr-scene` on `VulkanDevice`, with no `ash` in the dependency tree of `renderer`.
- [x] Zero validation errors/warnings with sync validation (`ANVIL_SYNC_VALIDATION=1 ANVIL_EXIT_AFTER_FRAMES=120 cargo run -p pbr-scene`).
- [x] `ANVIL_SCREENSHOT` produces a PNG an agent can inspect.
- [x] No scene crate dependency.

M3b/M3c (pending):
- [ ] Full PBR metallic-roughness over directional, point and spot lights; 2048 directional shadow map with PCF.
- [ ] IBL (procedural sky, per-roughness prefiltered equirect textures, irradiance, BRDF LUT).
- [ ] Renderer builds against a mock backend (the `NullBackend` of earlier drafts is the mock in `rhi` tests).

## 15. Open questions

1. Resolved (ADR-0011 §2): the renderer defines `RenderScene`; the app rebuilds it each frame.
2. ECS choice (own, bevy_ecs, hecs) affects extraction; deferred to LAYER-010 (the ECS becomes another producer of `RenderScene`).
3. Resolved (ADR-0011 §2): the app owns `Device` and the swapchain; `render` takes the acquired `SwapchainImage`.
4. Resolved (ADR-0010): bindless; materials store bindless texture indices.
5. Math resolved (ADR-0011 §1): `glam`, right-handed, Y-up, -Z forward, reverse-Z, Y flip in the projection. Forward+ / clustered shading stays M4.

## 16. Related modules

MODULE-010-render-world.
