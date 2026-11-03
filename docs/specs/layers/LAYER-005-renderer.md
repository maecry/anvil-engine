# LAYER-005: Renderer

| Field | Value |
|---|---|
| Status | Accepted (ADR-0011, ADR-0012); M4 complete |
| Milestone | M3 (forward, PBR, shadows, IBL: complete); M4a (render graph port: complete); M4b (prepass, clustered lights, deferred, alpha, cascades: complete); M4c (SSAO, TAA, bloom: complete) |
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

M4a (ADR-0012): each pass is a function that adds a graph node from an inputs struct (`shadow_pass(&mut GraphBuilder, ShadowInputs)`, `forward_pass`, `tonemap_pass`, `bake_pass`); the closure only binds and draws, and the render graph owns barriers, `begin_rendering`, viewport and scissor. There are no hand-written barriers in the crate. The HDR target and depth are graph transients (pooled, recreated automatically on resize; `Renderer::resize` is gone); the shadow map and IBL textures are imported persistents whose states the graph remembers. The swapchain image is imported `Undefined` and exported in the state the caller passes to `Renderer::render(device, cmd, &FrameToken, scene, &SwapchainImage, final_state)`: `Present`, or `TransferSrc` when the app reads it back (the app then transitions it to `Present` itself after the copy). The steady-state frame was `shadow`, `forward`, `tonemap` in M4a (M4b: see below); `bake.*` passes are added only on frames where the environment changed (all of them, including the BRDF LUT, on the first frame).

Per frame the renderer fills one persistently mapped `Upload` ring per frame in flight (16 MiB each since M5a, bump allocated, reset when the slot is reused) with frame data (view, proj, view_proj, camera position, light count, sampler index, exposure), the light array and the dense material array. Shaders read them through buffer device addresses in the push constants (5 addresses = 40 of 128 bytes: frame, instances, lights, materials, vertices); `first_instance` of each indexed draw is the instance index. Meshes are GPU-only vertex (BDA) and index buffers.

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

### M4b: prepass, clustered lights, deferred, alpha, cascades (ADR-0012 §11, implemented)

Frame passes: `bake.*` (only when the environment changed), `shadow.cascade0..3`, `shadow.spot0..3` (always added; empty ones only clear so every shadow map is in a known state), `prepass`, then forward (`forward`) or deferred (`gbuffer`, `lighting`), `skybox`, `transparent` (only with blended draws), `tonemap`. Steady state is 13 passes; compile takes about 2 us.

- **Culling and sorting** (`cull.rs`): every `Mesh` has an object-space bounding sphere computed in `create_mesh` (AABB center, farthest vertex); a world sphere is the transformed center with the radius scaled by the largest axis. Spheres are tested against a `Frustum` extracted from a view-projection matrix (works for the reverse-Z infinite camera, orthographic cascades and spot perspectives). Views: the camera, each cascade and each spot shadow. Opaque and masked draws are sorted by pipeline variant then front to back; blended draws back to front. `RendererStats` gains `culled` (camera frustum), `shadow_draws` and `cluster_time` (`draws` counts camera draws).
- **Depth prepass, always on**: writes `D32Sfloat` depth (reverse-Z, clear 0) and the view-space normal, octahedral in `Rg16Float` (`view_normal`, for the M4c SSAO). Forward, G-buffer and transparent passes load that depth and test `GreaterOrEqual` with no depth write. The prepass, forward, G-buffer and transparent pipelines share one vertex shader (`forward.vert`, `invariant gl_Position`) so depth matches bit for bit; shadow passes use `shadow.vert` with their own matrices.
- **Clustered lights** (`cluster.rs`): 16x9x24 froxels, exponential depth slices from the camera near plane to `CLUSTER_FAR` = 100 m (the last slice extends to infinity), view-space AABBs cached per camera shape. Directional lights are looped by every fragment (they come first in the light array); point lights and spot cones (bounded by `spot_bounds`) are assigned by sphere-versus-AABB tests over the slices their depth range touches. Output is `(offset, count)` per cluster plus a light index list, uploaded through the ring (addresses in `GpuFrame`). `MAX_LIGHTS` is 1024; the ring is 8 MiB per frame in flight. Assignment of 259 lights takes about 0.15 ms in release, 3 lights about 10 us. Range 0 means unlimited and lands in every cluster.
- **Shared lighting code** (`lighting.glsl`, `surface.glsl`, `bindings.glsl`): BRDF, IBL, cascade and spot shadows and the cluster loop live in common GLSL included by `forward.frag` and `lighting.frag`; material evaluation is shared with `gbuffer.frag`. `RenderPath::{Forward, Deferred}` (`Renderer::set_render_path`) selects the path at runtime. The G-buffer is albedo + metallic `Rgba8Srgb`, world normal + roughness `Rgba16Float`, emissive + AO `Rgba16Float`, plus the prepass depth; the lighting pass reconstructs the world position from depth and leaves sky pixels to the skybox pass. The G-buffer slots reach the shader through push constants (the transient textures have no stable bindless slot when the frame data is uploaded). On pbr-scene the two paths differ by at most 47/255 in a few edge pixels and 0.14/255 on average (8-bit albedo contour lines and slat edges).
- **Alpha**: `MaterialDesc.alpha_mode: AlphaMode::{Opaque, Mask(cutoff), Blend}` and `double_sided` select one of four pipeline variants per stage (`mask * 2 + double_sided`); the mask variants add `discard` (prepass, G-buffer, forward and shadows). Double-sided faces flip the normal on back faces. Blended draws go to the `transparent` pass after the lighting and the sky: premultiplied blend (`One`, `OneMinusSrcAlpha`), the shader scales only the diffuse term by alpha so reflections survive, clustered lights, IBL and shadows apply. `// shortcut:` blended objects do not cast shadows and are sorted per object (no intra-mesh or intersecting-object ordering); order-independent transparency is not planned.
- **Shadows** (`shadow.rs`): the first shadow-casting directional light gets 4 cascades over `SHADOW_DISTANCE` = 80 m with the practical split (`lambda` = 0.75, `cascade_splits`), each a separate imported 2048 `D32Sfloat` texture, fit to the bounding sphere of its frustum slice and snapped to texels (`fit_directional(.., near, far, ..)`). The shader picks the cascade by view depth and blends into the next over the last 10 percent of the range; lookups use the cascade's texel size for the normal offset. Up to `MAX_SPOT_SHADOWS` = 4 spot lights with `cast_shadows` get a 1024 perspective map (`fit_spot`, depth 0..1, `LessOrEqual`). `// shortcut:` point-light shadows need cube or layered textures and wait for the M5 RHI work. The orthographic matrix is built locally: glam 0.34.2 `camera::rh::proj::vulkan::orthographic` flips Y without flipping the Y translation, which mis-centered the M3 shadow volume (it was consistent between render and lookup, so M3 shadows still worked inside the shifted volume).
- **Debug views** (`Renderer::set_debug_view`, `DebugView::{None, Clusters, Cascades}`): the cluster light-count heatmap (blue 1, red 32+) and a per-cascade tint. pbr-scene keys: `G` forward/deferred, `L` 256 animated stress lights, `C` clusters, `V` cascades; env `ANVIL_RENDER_PATH=forward|deferred`, `ANVIL_STRESS_LIGHTS=1`, `ANVIL_DEBUG_VIEW=clusters|cascades`.
- RHI: the only change is that the device enables the Vulkan 1.3 feature `shaderDemoteToHelperInvocation`, which `discard` compiles to (LAYER-003 §19).
- Tests: `cull.rs` (frustum planes for perspective reverse-Z and orthographic, bounds), `cluster.rs` (slice math, assignment against a brute-force sphere/AABB loop, spot bounds), `shadow.rs` (cascade splits, per-cascade coverage and texel size ordering, ortho centering, spot matrix), and the GPU layout size check (`GpuFrame` is 1008 bytes).

Superseded M3 statements: the single 50 m shadow map, `MAX_LIGHTS` = 64, "the whole light array is looped" and the forward pass writing depth.

### M4c: SSAO, TAA, bloom (ADR-0012 §11, implemented)

Frame order: shadows, `prepass` (depth, view normal, velocity), `ssao` + `ssao.blur`, forward or G-buffer + `lighting`, `skybox`, `transparent`, `taa` (+ `taa.reset`), `bloom.down0..5` / `bloom.up4..0`, `tonemap`. With everything on, pbr-scene builds 27 passes. Each effect is a pass function (`post.rs`) and `PostSettings` simply omits the passes of a disabled one; the graph's culling and validation keep every combination valid (all 8, on both render paths, are run under sync validation).

- **SSAO**: 16-sample hemisphere kernel (radius 0.8 m, per-pixel rotation from interleaved gradient noise) at half resolution from the prepass depth and the octahedral view-space normal, then `ssao.blur`: a 3x3 depth-aware (bilateral) blur fused with the upsample to full resolution (`R8Unorm`). It multiplies only the ambient/IBL term (`shade()` in `lighting.glsl`, both paths); direct lights are untouched, blended surfaces do not receive it. Key `O`, `ANVIL_SSAO=0|1`, `DebugView::Ssao` (shows the occlusion exaggerated 3x).
- **TAA**: Halton(2, 3) jitter over 8 frames applied to the projection of the main view only (prepass, G-buffer/forward, transparent; `GpuFrame` carries the jittered and the plain matrices); shadows, culling and clusters do not jitter. `MeshInstance.prev_transform` and the previous view-projection give a velocity target written by the prepass (`Rg16Float`, uv delta); the sky and other pixels with no geometry use camera-only reprojection through the inverse view-projection (homogeneous, valid at infinity). The resolve (`taa.frag`) works in HDR before bloom: 3x3 YCoCg neighborhood clamp of the history, velocity taken from the closest-depth neighbor, history weight 0.9 (blend 0.1), history rejected outside the screen, on the first frame, after a resize and after TAA was off, and on disocclusion (static pixels compare the linear depth stored in the history alpha with the depth the camera motion predicts). It ping-pongs two imported `Rgba16Float` history textures. Key `Y`, `ANVIL_TAA=0|1`. `// shortcut:` bilinear history sampling (no Catmull-Rom), no history depth rejection for moving objects (clamping only), no material mip bias or sharpening (optional in the ADR, skipped).
- **Bloom**: threshold-free. A 6-level downsample chain (13-tap filter, Karis average on the first level) in separate `Rgba16Float` transients starting at half resolution, and a tent upsample chain (`bloom.up4..0`, each adding the same-size down level) in separate textures; `tonemap.frag` adds the result scaled by 0.025 to the HDR image before the ACES curve. Key `B`, `ANVIL_BLOOM=0|1`.
- **Examples/tests**: pbr-scene also has `ANVIL_CAMERA_ORBIT=1` (slow orbit, a function of the frame number), a turning checker cube (exercises `prev_transform`), and `ANVIL_TOGGLE_POST=n` (steps through all 8 combinations every n frames). Unit tests: `post.rs` (Halton values, jitter bounds and cycle, projection jitter moves the image by the requested pixels at any depth), `GpuFrame` (1200 bytes) and `GpuInstance` (208 bytes) layout checks.

### M5a: profiling, benchmarking and parallel preparation (ADR-0013)
- **Stage timing:** `RendererStats::stages: StageTimes` holds the CPU time of `pack`, `cull`, `sort`, `shadow_lists`, `clusters`, `upload`, `graph_build`, `compile` and `execute`; each is also a `debug` span `renderer.<stage>` (visible in Tracy with the `profile-tracy` feature). `RendererStats::gpu` carries the per-pass GPU times of an earlier frame (LAYER-006 section 8a). `RendererStats` is no longer `Copy` (it owns the pass list); `Renderer::stats()` clones it.
- **Parallel preparation (`prep.rs`, `cluster.rs`):** instance packing (world bounds, normal matrix), camera frustum culling, camera and shadow draw-list sorting, the per-view shadow lists and the cluster light assignment run on the worker pool (LAYER-001 section 10). Results are identical to the serial loops: order-preserving collects, stable sorts, in-order chunk concatenation; unit tests `parallel_matches_serial`, `one_thread_equals_many` and `parallel_assignment_matches_serial` compare against plain serial code and a 1-thread pool, and a many-draws screenshot is byte-identical before and after. Inputs below `PAR_MIN` (2048 instances) or `PAR_LIGHTS` (16 lights) run on the calling thread, because waking the pool for a 25-draw scene costs more than the work (measured: pbr-scene CPU time doubled without the threshold). Packing reuses `prep::Scratch` buffers between frames: freshly allocated multi-megabyte vectors page-fault on first touch, which made the parallel pack slower than the serial one. Command recording and the ring upload stay single-threaded.
- **Upload ring:** raised from 8 to 16 MiB because 50 000 instances need 10.4 MB (208 bytes each); the original ring overflowed at about 40 000 instances (BENCH-001). Since M5b the instances live in a persistent buffer and only changed ones pass through the ring (see M5b below); the ring is still fixed.
- **Benchmarks:** `renderer::bench::Bench` implements `ANVIL_BENCH=path.json` (`ANVIL_BENCH_WARMUP` default 120, `ANVIL_BENCH_FRAMES` default 1000) for `pbr-scene` and `many-draws`: documented in `docs/benchmarks/` (BENCH-001). The new example `examples/many-draws` (about 50 000 instances of a sphere, a cube and a torus, 16 materials, cascaded sun, 3 point lights and a spot, fly-path camera) is the large-count workload.

### Deferred beyond M4
Point-light shadows, GPU culling and shadows for blended objects (M5); HDR environment files and glTF import (asset system, LAYER-009).
Point-light shadows, GPU culling and shadows for blended objects (M5); HDR environment files and glTF import (asset system, LAYER-009).

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

M4 (implemented, ADR-0012):
- [x] M3 passes run as graph nodes with no hand-written barriers (M4a); pixel-identical screenshot.
- [x] Depth prepass, clustered lights (1024 cap), forward and deferred paths (screenshots differ by at most 47/255 at a few edge pixels, 0.14/255 on average), alpha mask and blend, 4 cascades, 4 spot shadows, CPU frustum culling and sorting (M4b).
- [x] SSAO, TAA and bloom as graph passes, each switchable at runtime; zero validation errors and warnings in all 8 combinations on both paths, and with `ANVIL_GPU_AV=1` zero errors (M4c).

Open (not blocking M3 exit):
- [ ] Renderer builds against a mock backend (the `NullBackend` of earlier drafts is the mock in `rhi` tests).

## 15. Open questions

1. Resolved (ADR-0011 §2): the renderer defines `RenderScene`; the app rebuilds it each frame.
2. ECS choice (own, bevy_ecs, hecs) affects extraction; deferred to LAYER-010 (the ECS becomes another producer of `RenderScene`).
3. Resolved (ADR-0011 §2): the app owns `Device` and the swapchain; `render` takes the acquired `SwapchainImage`.
4. Resolved (ADR-0010): bindless; materials store bindless texture indices.
5. Math resolved (ADR-0011 §1): `glam`, right-handed, Y-up, -Z forward, reverse-Z, Y flip in the projection. Clustered shading landed in M4b.

## 16. Related modules

MODULE-010-render-world.

### M5b: GPU-driven rendering (ADR-0013 §6, §7)
- **Persistent GPU scene (`prep.rs::SceneCache`, `renderer.rs::SceneBuffers`).** Instance data lives in one `GpuOnly` buffer indexed by the instance's position in `RenderScene::meshes` (224 bytes: model, normal matrix, previous model, world bounding sphere, material, mesh record index, flags, scale) next to a `visible` flag buffer (one u32 per instance). Each frame `SceneCache::update` compares every `MeshInstance` with the one at the same index last frame (`MeshInstance` is `PartialEq`: transform, previous transform, mesh and material ids), on the worker pool, and re-resolves and re-packs only the changed ones; an unchanged scene uploads nothing (`RendererStats::instances_uploaded`). Changed slots are merged into runs (gap up to 4 slots), written to the frame's ring and copied into the instance buffer by the graph pass `scene.upload` (`BufferState::TransferDst`); a first upload, a growth (the buffers double) or an update larger than half of the free ring goes through `Device::upload_buffer` instead. **Design choice:** compare against the previous frame's instance instead of stable instance ids or generations. It needs no change to `RenderScene` (the app keeps rebuilding it each frame, as ADR-0011 §2 promises), one parallel pass over about 150 bytes per instance costs 0.1 ms for 50 000 instances (the `pack` stage), and a stable id would have to be threaded through every producer (the ECS of LAYER-010 would have to mint and recycle them). shortcut: the instance identity is its index, so removing an instance from the middle re-uploads everything after it; add stable ids when scenes churn. `destroy_mesh` and `destroy_material` invalidate the cache so resolved data cannot go stale.
- **Culling passes (`gpu_cull.rs`, shaders `cull_camera.comp`, `cull_shadow.comp`, `hiz.comp`).** Per frame (GPU mode, the default): `scene.upload`, `cull.reset` (zero the counters), `cull.shadow` (frustum culling for the 4 cascades and 4 spot lights in one dispatch), `cull.camera.p1`, the 8 shadow passes, `prepass` (phase 1), `hiz.0..9`, `cull.camera.p2`, `prepass.p2`, then SSAO, forward or G-buffer, skybox, transparent, `cull.stats`, TAA, bloom, tonemap. **Phase 1** draws the instances that were visible last frame (`visible` flags) and still inside the frustum. The **depth pyramid** is built from that depth: an `R32Float` mip chain, mip 0 half the depth resolution, 2x2 reduction per mip with the odd row and column folded into the last texel; reverse-Z means the farthest depth is the minimum, so the reduction is `min`. **Phase 2** tests every instance against the frustum and the pyramid (conservative screen rectangle of the bounding sphere from its tangents, the mip whose texels cover the rectangle in at most 2x2, nearest sphere depth `near / (distance - radius)` against the minimum of the four texels with a 0.2 % margin for the TAA jitter), stores the new visible set, and draws the instances that pass and were not drawn in phase 1. Frame 0, a camera cut or a hidden object turning visible all fall into phase 2. The same shader selects the **level of detail**: the coarsest level whose object-space error, scaled by the instance and projected at the instance's distance, is below `lod_pixels` (default 0.5 px, `Renderer::set_lod_pixels`, `ANVIL_LOD=<px>`, `0` = always level 0; `MAX_LODS` = 4).
- **Indirect draws.** Each pass writes `VkDrawIndexedIndirectCommand`s into a transient command buffer (regions per culling view and pipeline variant, each as large as the number of opaque instances of that variant: `CullLayout`) and counts into a transient counts buffer (graph transients, addresses pushed to the compute pass). `first_instance` carries the instance slot, the level (bits 28-30) and the phase 2 flag (bit 31); the vertex shader masks it (`forward.vert`, `shadow.vert`). Prepass, G-buffer and forward opaque passes issue one `draw_indexed_indirect_count` per variant and phase (prepass phases: one each; forward and G-buffer: both); each shadow pass one per variant. Blended objects stay on the CPU path (culled, sorted back to front, drawn from the same arena and instance buffer) because they need sorting.
- **CPU path as oracle.** `ANVIL_GPU_CULLING=0` (or `Renderer::set_gpu_culling(false)`) keeps the M4 path on the same persistent buffers: CPU frustum culling, sorting and one `draw_indexed` per instance at level 0, now with one push constant write per pass instead of per draw. Verified: with `ANVIL_LOD=0` the GPU path's screenshots are byte-identical to the CPU path's for many-draws at frames 2, 61, 120, 400 and 900 and for pbr-scene, and the renderer's GPU test compares both on a scene with a wall hiding 400 spheres (`crates/renderer/tests/gpu_culling.rs`). A scene without opaque instances also takes the CPU path.
- **Statistics.** The counters live in the counts buffer (draw counters, then visible, frustum culled, occlusion culled, triangles after LOD, camera indirect draws, phase 2 draws, shadow draws), copied by `cull.stats` into a per-frame-slot readback buffer and read `frames_in_flight` frames later: `RendererStats::cull: CullStats` (with its `frame`), `culled`, `draws`, `triangles` and `shadow_draws` fold them in when GPU culling is on (`draws` and `triangles` = the delayed GPU numbers plus the blended draws), `gpu_culling`, `instances`, `instances_uploaded`. The bench JSON has `gpu_culling`, `instances`, `instances_uploaded` and a `cull` block.
- **Debug views.** `DebugView::Lod` (`ANVIL_DEBUG_VIEW=lod`): instances tinted by level, 0 green, 1 yellow, 2 orange, 3 red. `DebugView::Occlusion` (`ANVIL_DEBUG_VIEW=occlusion`): instances that phase 2 drew this frame tinted magenta (all of them on frame 0, a handful of newly visible ones in steady state). Both are applied in `forward.frag` and `gbuffer.frag` from the vertex shader's flag varying; `DebugView::from_name` parses the environment value in both examples.
- **Tests.** `prep.rs` (`parallel_matches_serial` over the cache, `only_changed_instances_are_repacked`: unchanged scene, moved instances, grow, shrink, invalidate; `one_thread_equals_many`), `arena.rs`, `gpu_cull.rs` (command layout regions never overlap, pyramid sizes, stats parsing), `mesh.rs` (LOD generation), `gpu.rs` (layout sizes: instance 224, mesh record 80, cull parameters 1184) and the GPU tests of `crates/renderer/tests/gpu_culling.rs` (identical image, occlusion culled and visible counts, static scene uploads nothing and draws no phase 2, LOD lowers the triangle count and keeps the picture within 2/255 on average), all under sync validation.
- **Shortcuts.** Fixed upload ring (16 MiB), mesh table (4096) and arena capacities; one command region per (view, variant) sized to the variant's instance count (a 50 000-instance scene needs a 10 MB command buffer); the occlusion test uses bounding spheres (loose for long thin meshes); shadow views use the camera's LOD choice; no per-cluster or per-meshlet culling; the LOD debug view and the G-buffer tint only exist when GPU culling is on; the hierarchical pyramid is built from the phase 1 depth, so an object that only phase 2 draws can never occlude within its own frame (it occludes from the next frame on, when it is in the phase 1 set).

### M5c: point-light shadows, texture streaming, debug geometry (ADR-0013 §8-§10)
- **Point-light shadows.** `Light::Point` gained `cast_shadows`. Per frame `select_point_shadows` picks up to `MAX_POINT_SHADOWS` (4) casters by `luminance * intensity / distance^2` (camera distance, lights reaching within the 80 m shadow distance, range at least 0.5). Each owns a persistent D32 cube (512 x 512 faces, 6 layers) plus six one-layer views; a face is a normal shadow pass (`shadow.pointN.faceF`) using view indices 8..31 in `shadow.vert`, GPU frustum culling (cull views 10..33, 90 degree frustum planes of the face matrix) and indirect draws, so the shadow-view path is reused. The face matrix (`point_face_view_proj`) follows the Vulkan cube face table (left-handed frames, unit-tested against the table), depth 0..1 between 0.05 and the light range of the major-axis distance; the face pipelines cull the opposite winding (images are mirrored). Sampling (`point_shadow`, `lighting.glsl`): normal-offset lookup, reference depth recomputed per tap from the lookup direction with the same formula, four hardware-compared taps over a half-texel footprint (about 3 x 3 PCF); cube sampling is seamless so face edges show no seam. The command buffer only has regions for active views (`CullLayout` takes the view mask). GPU cost per light is in BENCH-001 §13. Shortcuts: casters use the camera's LOD; the cube resolution is fixed; blended objects do not cast.
- **Texture streaming.** See LAYER-007 §18. `Renderer::create_texture` generates the mip chain on the CPU; `update_streaming` runs once per frame (stage `streaming`, span `renderer.streaming`). Stats: `RendererStats::textures` (`TextureStats`: resident bytes, budget, pending uploads/bytes, uploads and bytes this frame, cumulative evictions). Debug view `ANVIL_DEBUG_VIEW=mips` tints each material by the resident base mip of its base color texture (0 green, 1 light green, 2 yellow, 3 orange, 4 red, 5+ magenta).
- **Debug geometry.** `Renderer::debug_lines()` returns the `DebugLines` buffer (`line`, `aabb`, `sphere`, `circle`, `frustum`); the `debug.lines` pass (line list, depth tested, no depth write, HDR target before TAA) follows the transparent pass. Up to 131 072 vertices per frame (2 MiB of the ring), the rest is dropped with a warning. `Renderer::set_debug_draw(DebugDraw)` / `ANVIL_DEBUG_DRAW=bounds|cascades|lights` makes the renderer add instance bounding spheres (within 60 m, radius up to 50 m), the cascade volumes, or point ranges and spot cones. Shortcut: cascade volumes enclose the camera, so they can only be inspected from inside (a detached debug camera is future work).
- **Examples.** pbr-scene's warm point light casts shadows (`ANVIL_POINT_SHADOWS=0` disables; with it off the frame is pixel-identical to M5b), `ANVIL_CAMERA=x,y,z,yaw,pitch`; many-draws has three streamed procedural textures (2048, 2048, 1024) and `ANVIL_POINT_SHADOWS=n`.
- **Deferred beyond M5** (recorded, no measured need or out of scope): transient memory aliasing, async compute, parallel command recording, disk and asset streaming (M6), on-screen overlays (M6 egui), texture streaming feedback from the GPU, array-texture bindless slots.
