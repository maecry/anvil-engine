# ADR-0011: M3 core renderer architecture and conventions

- Status: Accepted
- Date: 2026-11-02

## Context
M3 delivers "complete PBR scene rendering":
- FR-REND-001..010 and 025, FR-PLAT-005, FR-RES-012.
- Meshes, cameras, transforms, textures, materials, PBR metallic-roughness.
- Directional, point and spot lights, forward rendering with depth, shadow mapping and IBL.

The render graph is M4, the asset system and glTF are LAYER-009, and the ECS is LAYER-010. Open questions from LAYER-005 and MODULE-010 block the camera and renderer types, and the RHI from M2 cannot sample depth or render HDR.

## Problem
Fix the math conventions, the renderer/app boundary, the frame structure and the minimum RHI additions, so M3 can be built without the render graph, assets or an ECS.

## Decision
1. **Math (LAYER-001 Q1, MODULE-010 Q5).**
   - Library: `glam` (workspace dependency, re-exported as `foundation::math`).
   - World space: right-handed, Y-up, forward is -Z (the glTF convention).
   - Depth: reverse-Z. `D32Sfloat` is cleared to 0 and tested with `GREATER_OR_EQUAL`, using an infinite-far perspective (`Mat4::perspective_infinite_reverse_rh`).
   - Clip space: the Vulkan Y flip is folded into the projection matrix (`proj.y_axis.y *= -1`). The viewport stays positive, so every pass, including shadows, behaves the same.
2. **Renderer/app boundary (LAYER-005 Q1, Q3; MODULE-010 Q1, Q6, Q7).**
   - A new crate, `renderer`, defines plain-data extraction types: `RenderScene { camera, meshes: Vec<MeshInstance>, lights: Vec<Light>, environment }`.
   - The app rebuilds the `RenderScene` every frame (a full rebuild, not a diff). A future scene/ECS layer becomes just another producer of it, so the renderer never depends on scene types.
   - The app owns `Device` and `SwapchainHandle`.
   - The `Renderer` owns its GPU resources: pipelines, depth target, HDR target, shadow map, environment textures and the per-frame ring. Meshes, textures and materials are renderer-side handles created through `Renderer` (`MeshId`, `TextureId`, `MaterialId`). No Vulkan objects are exposed (FR-RES-012).
3. **Frame structure without the graph (graph is M4).** A fixed sequence of passes with hand-written barriers:
   1. shadow (directional light, depth only)
   2. forward opaque (PBR + IBL + shadows) into an HDR `Rgba16Float` target with reverse-Z depth
   3. skybox (in the same rendering scope, drawn after the opaques)
   4. tonemap (ACES fit, plus exposure) into the swapchain

   The pass functions are written so that they can become render-graph nodes in M4.
4. **GPU data.**
   - A per-frame-in-flight `Upload` ring buffer (persistently mapped, bump-allocated, reset per slot) carries the frame, instance, light and material data. Shaders read it through BDA, with addresses in push constants (ADR-0010).
   - Layout is AoS, with world matrices stored pre-multiplied (MODULE-010 Q7).
   - Meshes use one vertex layout: position, normal, uv, tangent(xyz, sign). Vertex and index data sit in GPU-only buffers.
5. **Materials.**
   - glTF metallic-roughness: base color factor and texture, metallic/roughness factors and texture (G = roughness, B = metallic), normal texture, occlusion, emissive factor and texture.
   - Textures are bindless indices. A missing texture uses built-in 1x1 defaults.
   - Only opaque materials in M3; alpha blending/masking is M4.
6. **Lights.**
   - Forward shading loops over a light array: directional, point and spot, with glTF `KHR_lights_punctual` semantics. Point and spot use inverse-square falloff with a range window.
   - There is no clustering; Forward+ and clustered shading are M4.
7. **Shadows.**
   - A single 2048² directional shadow map, fitted to the camera frustum bounds.
   - Hardware PCF uses a comparison sampler; slope-scaled depth bias is set in the pipeline.
   - Cascades and point/spot shadows are M4.
8. **IBL without compute or mip/cube support.**
   - A procedural HDR sky is rendered into an equirectangular `Rgba16Float` texture.
   - Fragment passes prefilter it into **N separate equirect textures**, one per roughness level (default 6). The shader picks two bindless indices and lerps, so no mip or per-mip state is needed.
   - Fragment passes also bake a diffuse irradiance equirect and a split-sum BRDF LUT.
   - All of this is baked once, when the environment changes.
   - HDR file loading arrives with the asset system.
9. **RHI additions (minimum).**
   - `GraphicsPipelineDesc.depth: Option<DepthState { format, compare: CompareOp, write: bool, bias: Option<DepthBias> }>`. This replaces the M2 "depth means LESS" rule; `depth_format` is replaced.
   - `SamplerDesc.compare: Option<CompareOp>` and sampled `D32Sfloat` (depth view in bindless slot). Transitions between `DepthAttachment` and `ShaderRead` are allowed.
   - `Format::Rg16Float` for the BRDF LUT.
   - `SwapchainDesc` always adds `TRANSFER_SRC` usage, for screenshots.
10. **Input (FR-PLAT-005).** `platform::KeyCode` gains the letters, digits, arrows, Space, Shift, Ctrl and Tab, and `Event::MouseMotion { dx, dy }` (raw device motion). Examples get a fly camera: WASD, mouse-look while the right button is held.
11. **AI observability (ADR-0009 §7 extension).**
    - `ANVIL_SCREENSHOT=path.png` combined with `ANVIL_EXIT_AFTER_FRAMES=n` copies the last frame's swapchain image to a readback buffer and writes a PNG (`png` crate, examples only). An agent can then look at the rendered result.
    - `RendererStats` (draws, triangles, lights, ring bytes used, pass timings later) is logged per second at `debug` and exposed via `Renderer::stats()`.

## Advantages
- No dependency on the unbuilt layers: graph, assets and ECS.
- IBL needs no new RHI surface beyond depth sampling.
- Screenshots let agents verify rendering visually.

## Disadvantages
- Hand-written pass barriers will be rewritten as graph nodes in M4. This is accepted, because the passes are structured as nodes from the start.
- Per-roughness textures cost more memory than a mip chain, which is small at 256×128 to 32×16.
- A full scene rebuild every frame costs CPU on large scenes. Revisit when the ECS arrives.

## Consequences
- New crate `renderer` (LAYER-005 and MODULE-010 are updated to this ADR).
- LAYER-003 gains §18, "M3 additions"; LAYER-002 gains the input additions.
- New example `examples/pbr-scene`: a grid of spheres with varying metallic and roughness, a textured cube, a ground plane, one directional light with shadows, a point light and a spot light, the sky, and a fly camera.

## Revisit conditions
- Profiling shows the per-roughness textures or the full rebuild are a bottleneck.
- M4's graph design needs a different pass interface.
