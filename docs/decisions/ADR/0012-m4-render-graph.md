# ADR-0012: M4 render graph design and advanced-rendering scope

- Status: Accepted
- Date: 2026-11-02

## Context
M4 delivers the render graph (FR-RG-001..009, FR-RES-011) and the advanced rendering features FR-REND-011..018: HDR, bloom, depth prepass, deferred, clustered lighting, SSAO, TAA, and post-processing as passes. It also absorbs what M3 deferred: cascades, spot shadows, alpha, culling and sorting. M3 has hand-written barriers across 4 fixed passes plus IBL bakes, and texture state is advisory (ADR-0010 §9). LAYER-006 and MODULE-009 leave seven questions open.

## Problem
Pick the graph's state model, compile strategy, transient strategy, pass storage, swapchain ownership and renderer boundary. Pick the techniques for the M4 features so that no compute shaders or new RHI surface are needed beyond what is listed.

## Decision
1. **Crate.** `render-graph` depends only on `rhi` and `foundation`. It knows nothing about meshes, materials or draw lists (MODULE-009 Q7): passes capture renderer data in their closures.
2. **State model (Q1).**
   - Textures only, whole-texture state (no per-mip/layer ranges). Features that need several levels use separate textures, as the IBL bakes already do.
   - Buffers are not tracked in M4. Every buffer M4 uses is either written by the host (the upload ring) or uploaded before the frame (ADR-0010 §7).
   - Buffer tracking and `buffer_barrier` arrive with compute in M5.
3. **Persistent state.** The `RenderGraph` object lives across frames and owns a state map `TextureHandle -> TextureState` for imported textures: IBL maps, TAA history, shadow maps.
   - `import(texture, initial: Option<TextureState>)` uses the remembered state when `initial` is `None`.
   - This replaces advisory states for everything the graph touches.
   - Swapchain images are imported with `Undefined`.
4. **Output (MODULE-009 Q6).** `graph.export(texture, final_state)` declares the state a texture must end in, for example `Present` or `TransferSrc` for a screenshot. Exported textures and passes marked `side_effect()` are the roots. Passes that do not reach a root are culled and reported.
5. **Compile (Q2).**
   - The graph is rebuilt and compiled every frame, with no caching.
   - Compile steps: validate, cull, order by declaration with a dependency check, plan barriers, allocate transients.
   - Compile CPU time is measured and exposed in `GraphStats`. Revisit if it exceeds 0.1 ms on pbr-scene.
6. **Transients (Q3).**
   - The pool is keyed by `TextureDesc`. A texture is reused only by a later frame after its last use has retired (the frame-slot fence), or within a frame after its last reader when the descriptors match (lifetime-based reuse).
   - There is no memory aliasing. `// shortcut:` until memory pressure shows up in M5, which would need RHI aliasing support.
   - Unused pool entries are destroyed after N frames.
7. **Pass storage (Q4).**
   - Each pass is `Box<dyn FnOnce(&mut PassContext<'_, D>) + 'a>`: one allocation per pass per frame, not per draw.
   - This is an explicit, bounded exception to ADR-0005's no-`dyn` rule (the rule targets per-draw hot paths).
   - The graph is generic over `D: rhi::Device`.
8. **Raster passes are declarative.**
   - A pass declares color and depth attachments with load/store/clear, plus sampled reads.
   - The graph emits barriers and `begin_rendering`/`end_rendering`, and sets the viewport and scissor to the attachment extent.
   - The closure only binds and draws.
   - Writing a texture read in the same pass, reading a texture nobody wrote, or writing an import with no initial state is rejected with an error that names the pass and the resource (FR-RG-004).
9. **Multi-queue (Q5).** Deferred to M5. One graphics queue.
10. **Diagnostics (FR-RG-008, ADR-0009).**
    - `CompiledGraph::describe() -> GraphDescription`, plain data: passes in order with culled flags, the resources with their desc, lifetime and imported/transient kind, and every planned barrier.
    - It is logged at `trace` per frame.
    - `ANVIL_GRAPH_DUMP=path` writes it once as Graphviz DOT (`.dot`) or JSON (`.json`), so an agent can inspect the frame structure.
    - Each pass gets a debug label region.
11. **Feature techniques (no compute in M4).**
    - **Depth prepass:** always on. It also writes view-space normals (Rg16Float octahedral) for SSAO.
    - **Clustered lighting:** a 16×9×24 froxel grid with exponential depth slices. Cluster assignment runs on the CPU and is uploaded through the ring as an offsets+indices list. The light cap rises to 1024. GPU culling is M5.
    - **Forward and deferred paths:** both use the clustered lights.
      - `RenderPath::{Forward, Deferred}` is a runtime toggle.
      - Deferred G-buffer: albedo+metallic `Rgba8Srgb`, normal+roughness `Rgba16Float`, emissive+AO `Rgba16Float`, and depth.
      - The lighting pass is fullscreen.
      - Alpha-blended materials always go through a forward transparent pass, sorted back to front.
      - Alpha mask (cutoff) is supported in both paths and in shadows.
    - **Shadows:**
      - The directional light gets 4 cascades with practical split scheme, each a separate 2048² texture, stable-fit and texel-snapped.
      - Up to 4 spot shadows (1024², separate textures).
      - Point-light shadows need cube/layer support. `// shortcut:` deferred to M5 with the RHI layer work.
    - **SSAO:** hemisphere SSAO at half resolution from depth and normals, followed by a bilateral blur. It multiplies only the ambient/IBL term.
    - **TAA:** Halton(2,3) projection jitter. Reprojection uses depth and camera motion, plus per-instance previous transforms (`MeshInstance.prev_transform`) for a velocity pass. History is clamped to the neighborhood in YCoCg and rejected on disocclusion. Disabled by key.
    - **Bloom:** threshold-free physically-based bloom. A 6-level down and up chain uses separate textures (13-tap down, tent up) and is mixed into HDR before tonemapping.
    - **Culling and sorting:** CPU frustum culling per view (camera, each cascade, each spot) using mesh bounding spheres. Opaques are sorted front to back by pipeline/material.
12. **RHI additions (minimum).**
    - Additive blend state presets for bloom upsampling, if not already expressible.
    - `Format::Rg16Float` already exists.
    - Nothing else is expected. Any new need must be listed in the phase report and LAYER-003 §19.

## Advantages
- Every M3 pass becomes a node with no RHI churn.
- Barriers and attachment setup are produced in one place and validated.
- The frame structure is dumpable for agents.

## Disadvantages
- No memory aliasing, so transients cost more VRAM. The resolution-sized set is about 100 MB at 4K.
- Per-frame compile and per-pass boxing allocations.
- Whole-texture state forces bloom and shadows into separate textures.

## Consequences
- LAYER-006 and MODULE-009 are updated and Accepted. LAYER-005 and MODULE-010 gain the M4 renderer features.
- The renderer drops all hand-written barriers.
- `pbr-scene` gains toggles: deferred/forward, SSAO, TAA, bloom. It also gains a stress mode with many point lights and alpha-tested and transparent objects.

## Revisit conditions
- Graph compile exceeds 0.1 ms.
- VRAM pressure from transients.
- M5 compute or async work needs buffer states.
