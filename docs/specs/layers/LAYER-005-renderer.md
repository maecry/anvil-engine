# LAYER-005: Renderer

| Field | Value |
|---|---|
| Status | Accepted (ADR-0011) |
| Milestone | M3 (forward, PBR, shadows, IBL: complete); M4a (render graph port: complete); M4b-c (post, deferred, ...) |
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
| renderer | M3 | depends on foundation (`foundation::math` = glam) and rhi; render-graph from M4a (passes are graph nodes) |

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
pub struct RenderScene { pub camera: Camera, pub meshes: Vec<MeshInstance>, pub lights: Vec<Light>, pub environment: Environment }
pub struct Environment { pub sun_direction, pub zenith, pub horizon, pub ground, pub sun_color, pub intensity } // PartialEq: re-baked only on change
pub struct Camera { pub position: Vec3, pub rotation: Quat, pub fov_y: f32, pub near: f32, pub exposure: f32 }
impl Camera { pub fn view(&self) -> Mat4; pub fn projection(&self, aspect: f32) -> Mat4 /* reverse-Z, infinite far, Y flip */ }
pub struct MeshInstance { pub mesh: MeshId, pub material: MaterialId, pub transform: Mat4 }
pub enum Light { Directional { direction, color, intensity, cast_shadows }, Point { position, color, intensity, range },
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

- Ownership: renderer owns GPU resources (pipelines, the shadow map and IBL textures, samplers, built-in 1x1 textures, per-frame rings, meshes, textures) through RHI handles and destroys them via the RHI; the app and scene hold only renderer-neutral ids (`MeshId`, `TextureId`, `MaterialId`, generational `foundation::Handle`s). No Vulkan objects are exposed (FR-RES-012).
- Threading: extraction on the main thread; visibility/sorting parallelisable on workers (task, M5); recording single-threaded until M5. Prefer immutable extracted data (MASTER-000 §11).
- Errors: construction and resource calls return `RhiError` (`DeviceLost` passes through unchanged); `Renderer::render` returns `RenderError { Rhi, Graph }` (M4a).
- Diagnostics: debug labels per pass (`shadow`, `bake.*`, `forward`, `tonemap`, from the graph), `tracing` `debug` on every mesh/texture/material create and destroy with its label, and `RendererStats { draws, triangles, lights, ring_bytes_used, graph }` (`graph` = `render_graph::GraphStats` incl. compile time) returned by `stats()` and logged once per second at `debug` (ADR-0011 §11). `ANVIL_SCREENSHOT` is implemented by the example, not the renderer.

## 12a. Frame structure and GPU data (M3a, ADR-0011 §3-5)

1. Shadow pass (M3b): depth-only, 2048x2048 `D32Sfloat`, from the first directional light with `cast_shadows`; always added to the graph (empty when no caster).
2. Forward opaque pass: HDR `Rgba16Float` color + reverse-Z `D32Sfloat` depth (clear 0, `GreaterOrEqual`, back-face culling, counter-clockwise front faces).
3. Tonemap pass: fullscreen triangle, ACES fit and exposure, HDR to the swapchain (manual gamma only if the swapchain format is not sRGB).

M4a (ADR-0012): each pass is a function that adds a graph node from an inputs struct (`shadow_pass(&mut GraphBuilder, ShadowInputs)`, `forward_pass`, `tonemap_pass`, `bake_pass`); the closure only binds and draws, and the render graph owns barriers, `begin_rendering`, viewport and scissor. There are no hand-written barriers in the crate. The HDR target and depth are graph transients (pooled, recreated automatically on resize; `Renderer::resize` is gone); the shadow map and IBL textures are imported persistents whose states the graph remembers. The swapchain image is imported `Undefined` and exported in the state the caller passes to `Renderer::render(device, cmd, &FrameToken, scene, &SwapchainImage, final_state)`: `Present`, or `TransferSrc` when the app reads it back (the app then transitions it to `Present` itself after the copy). The steady-state frame is `shadow`, `forward`, `tonemap`; `bake.*` passes are added only on frames where the environment changed (all of them, including the BRDF LUT, on the first frame).

Per frame the renderer fills one persistently mapped `Upload` ring per frame in flight (4 MiB each, bump allocated, reset when the slot is reused) with frame data (view, proj, view_proj, camera position, light count, sampler index, exposure), the instance array (model + normal matrix + material index), the light array and the dense material array. Shaders read them through buffer device addresses in the push constants (5 addresses = 40 of 128 bytes: frame, instances, lights, materials, vertices); `first_instance` of each indexed draw is the instance index. Meshes are GPU-only vertex (BDA) and index buffers.

### M3b: PBR, lights, shadows

- Shading (`forward.frag`): Cook-Torrance metallic-roughness: GGX NDF, Smith height-correlated visibility, Schlick Fresnel with `F0 = mix(0.04, baseColor, metallic)`, Lambert diffuse scaled by `(1-F)(1-metallic)`. Base color, metallic/roughness (G/B), normal, occlusion and emissive textures are honoured; roughness is clamped to 0.045.
- Lights: the whole light array is looped (directional, point, spot) with `KHR_lights_punctual` semantics: lux/candela, inverse-square falloff with the smooth range window `clamp(1-(d/range)^4)^2` (range 0 = unlimited), spot cone `smoothstep(cos_outer, cos_inner)`. At most `MAX_LIGHTS` = 64 lights are shaded (extra ones are dropped with a `warn`); `RendererStats.lights` is the number shaded.
- Ambient: a hemisphere gradient placeholder with a tinted specular term so metals are not black (`// shortcut:` in the shader), replaced by IBL in M3c.
- Shadows: one 2048 map, only for the first directional light with `cast_shadows`. The orthographic light volume is the bounding sphere of the camera frustum up to 50 m (`SHADOW_DISTANCE`), snapped to whole texels (`shadow.rs::fit_directional`, unit-tested). Lookup uses a comparison sampler (`LessOrEqual`, hardware bilinear PCF) in a 3x3 kernel plus a world normal offset scaled by the texel size and by `1 - N.L`. The shadow pipeline applies a slope-scaled `DepthBias` (constant 2, slope 2.5) and culls **front** faces: only back faces enter the map, so lit surfaces never self-shadow (no acne) without a large bias, and contact shadows stay attached (no peter-panning); the trade-off is light leaking through very thin closed-surface-free casters (a single plane casts nothing), acceptable for M3. Barriers: `Undefined/ShaderRead -> DepthAttachment` before the pass, `DepthAttachment -> ShaderRead` after it.
- The shadow pass needs a depth-only pipeline and rendering scope: the RHI now accepts zero color formats / attachments (LAYER-003 §18.5).
- shortcut: a single shadow map without cascades and fixed 2048 resolution; cascades and point/spot shadows are M4. Alpha-tested casters are M4.

### M3c: environment and IBL

- `Environment` is an analytic sky (zenith/horizon/ground gradient plus sun glow and disk). `sun_direction` is the direction the light travels and should equal the shadow-casting directional light. The environment is baked only when it differs (`==`) from the last baked one; the first render bakes everything.
- Bakes are fullscreen-triangle fragment passes (`bake_pass`, no compute), graph passes (`bake.brdf_lut`, `bake.sky`, `bake.irradiance`, `bake.prefilter` x6) added only when the environment changed, ordered before the shadow pass: sky 512x256 `Rgba16Float` equirect; diffuse irradiance 64x32 (cosine convolution on a 48x16 hemisphere grid, stored as E/pi); six GGX-prefiltered equirects for roughness 0, 0.2 ... 1 (256x128, 128x64, 64x32, 64x32, 32x16, 32x16; 192 importance samples each, no mips or cubemaps); split-sum BRDF LUT 256x256 `Rg16Float` (256 samples). The forward pass reads every IBL texture, so the graph leaves each in `ShaderRead` (`Undefined` the first time). Sample counts are small enough that recording is ~0.3 ms and the GPU work is far below 100 ms.
- Equirect mapping: u = atan2(z, x)/2pi + 0.5 wraps (sampler `Repeat` in U, `ClampToEdge` in V), v = acos(y)/pi; sampling uses `textureLod(.., 0)` so the atan discontinuity cannot select a wrong mip, and screenshots at the seam direction show none.
- Forward shading: diffuse = irradiance x albedo x (1-metallic); specular = lerp of the two nearest prefiltered levels (`level = roughness x 5`) x (F0 x LUT.x + LUT.y); both multiplied by occlusion and `Environment.intensity`. The hemisphere ambient placeholder is gone.
- Skybox: drawn after the opaques in the forward rendering scope with a fullscreen triangle at reverse-Z depth 0, compare `GreaterOrEqual`, depth write off; the ray comes from camera right/up/forward and the half-fov tangents in the frame data.
- Observability: each bake logs at `info` ("environment baked", command recording time in ms, `first`). shortcut: only recording time is measured (no wait); the BRDF LUT is baked on the first render, not in `Renderer::new`, because the RHI records only into a frame's encoder. shortcut: texels brighter than 4 (the sun disk) are clamped in the irradiance/prefilter bakes since the sun is a directional light; the prefilter has no filtered importance sampling, so very bright narrow sky features would be noisy.

### Deferred beyond M3
Cascaded shadows and point/spot shadows, alpha blend/mask materials, clustered or Forward+ lighting, visibility culling and sorting, the render graph (M4); HDR environment files and glTF import (asset system, LAYER-009).

## 13. Testing strategy

Unit tests for visibility/sorting with the RHI `NullBackend`; golden-image rendering tests (MASTER-000 §15.3) from M3; performance counters from M5.

## 14. Acceptance criteria (M3)

M3a (implemented):
- [x] Textured meshes with basic lighting render through `Renderer` driven by `examples/pbr-scene` on `VulkanDevice`, with no `ash` in the dependency tree of `renderer`.
- [x] Zero validation errors/warnings with sync validation (`ANVIL_SYNC_VALIDATION=1 ANVIL_EXIT_AFTER_FRAMES=120 cargo run -p pbr-scene`).
- [x] `ANVIL_SCREENSHOT` produces a PNG an agent can inspect.
- [x] No scene crate dependency.

M3b (implemented):
- [x] Full PBR metallic-roughness over directional, point and spot lights; 2048 directional shadow map with PCF; zero validation errors/warnings (also with `ANVIL_GPU_AV=1`, apart from the layer's own settings notices).

M3c (implemented):
- [x] IBL (procedural sky, per-roughness prefiltered equirect textures, irradiance, BRDF LUT), skybox, re-bake on change.

Open (not blocking M3 exit):
- [ ] Renderer builds against a mock backend (the `NullBackend` of earlier drafts is the mock in `rhi` tests).

## 15. Open questions

1. Resolved (ADR-0011 §2): the renderer defines `RenderScene`; the app rebuilds it each frame.
2. ECS choice (own, bevy_ecs, hecs) affects extraction; deferred to LAYER-010 (the ECS becomes another producer of `RenderScene`).
3. Resolved (ADR-0011 §2): the app owns `Device` and the swapchain; `render` takes the acquired `SwapchainImage`.
4. Resolved (ADR-0010): bindless; materials store bindless texture indices.
5. Math resolved (ADR-0011 §1): `glam`, right-handed, Y-up, -Z forward, reverse-Z, Y flip in the projection. Forward+ / clustered shading stays M4.

## 16. Related modules

MODULE-010-render-world.
