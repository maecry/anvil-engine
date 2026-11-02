# RESEARCH-004: Bevy Rendering Architecture

| | |
|---|---|
| Status | Draft |
| Related | MASTER-000 §19, §11; RESEARCH-003, RESEARCH-006 |
| Last updated | 2026-11-02 |

## Summary
Bevy renders in a separate **render sub-app** with its own ECS `World`. Each frame the main world's data are copied by an **Extract** step into the render world, then render schedules prepare GPU data, queue draw items into phases, sort them and execute a render graph on wgpu. This describes Bevy as of the 0.14 to 0.16 era from the author's knowledge; Bevy makes breaking changes every release (roughly every 3 to 4 months), so all names must be re-checked against the version in use (verify).

## Concepts

### Render sub-app and render world
- Main app and render app each own a `World`. Main world holds game state; render world holds render-side copies (components, resources, GPU resource caches).
- `ExtractSchedule` runs with access to both worlds, copying what render needs (transforms, meshes, lights, cameras). After extraction, main world can start the next frame while render world processes this one (pipelined rendering, on separate threads).
- Render-world entities are cleared and recreated each frame unless synced (the mapping between main and render entities has changed between versions; verify).

### Render schedule sets
Described in `RenderSet` (name and members change between versions; verify):
| Set | Purpose |
|---|---|
| ExtractCommands | Apply deferred extract commands |
| PrepareAssets | Convert CPU assets (meshes, images) into GPU resources |
| ManageViews | Create/prepare view targets, cameras |
| Queue | Build draw items (phase items) from visible entities |
| PhaseSort | Sort/batch phase items |
| Prepare | Create buffers, bind groups, uniforms |
| Render | Execute the render graph |
| Cleanup | Per-frame cleanup |

### Render phases
A `RenderPhase`/`PhaseItem` (renamed in newer versions: binned and sorted phases; verify) is a list of draw items for a view, such as Opaque3d, AlphaMask3d, Transparent3d. Opaque items are binned by pipeline/material to batch and sorted front-to-back or by key; transparent items are sorted back-to-front. Each item is drawn by a tuple of `RenderCommand`s (set pipeline, set bind group, draw mesh).

### Render graph
A directed graph of nodes with input/output slots and edges, executed by a `RenderGraphRunner` with a `RenderContext` over wgpu command encoders. Sub-graphs (e.g., per camera, Core2d/Core3d) are run from a view node. Barriers are not expressed in the graph; wgpu tracks resource usage and inserts them.

### wgpu usage
Bevy's renderer is built on wgpu (Vulkan, Metal, D3D12, GL, WebGPU backends). Resources are `wgpu::Buffer/Texture/BindGroup`; bind groups are created in prepare; wgpu handles memory allocation, barriers and validation.

### Pipeline specialization and caching
Materials and meshes produce a **specialization key** (mesh layout, MSAA, HDR, material flags); `SpecializedMeshPipelines` maps key -> `CachedRenderPipelineId` via a `PipelineCache`, which compiles pipelines asynchronously and is keyed on descriptor. Shader variants come from shader defs. This avoids creating pipelines per draw while allowing many permutations.

## Analysis / Comparison

| Aspect | Bevy approach | Benefit | Cost / risk |
|---|---|---|---|
| Separate render world | Extract copy | Parallelism with simulation; clear data contract | Copy cost; duplicated components; sync-point complexity |
| Phases + binning | Draw items sorted/binned | Batching, ordering | Phase abstraction is intertwined with ECS systems |
| Specialization cache | Key -> pipeline | Avoids permutation explosion at runtime | Hitches without pre-warm; key design is critical |
| Render graph | Nodes + slots | Easy to extend | No culling, aliasing or barrier derivation |
| wgpu | Safe, portable | No explicit sync/memory work | No control over sync/allocation (RESEARCH-006) |
| API churn | Every release | Rapid evolution | Hard to copy specifics |

## Implications for Anvil

### Adopt (concepts)
- Explicit extract step producing a render-side snapshot; decouples simulation from rendering and clarifies ownership (MASTER-000 §11). Start single-threaded as a plain copy into a render-frame struct; consider true pipelining later.
- Queue/sort/render phase split with sort keys for draw items.
- Pipeline specialization key + cache, with an explicit pre-warm path.
- Separation of prepare (GPU data upload) from record (command recording).

### Avoid or defer
- Bevy's untyped slot-based graph with no barrier derivation (use RESEARCH-003 approach instead).
- Coupling the render world to an ECS: Anvil's scene layer is not defined yet (MASTER-000 §8.9); keep the render input as plain data structures.
- Copying API names: they are version-specific.

## Open questions
- Is a separate render world needed or is a per-frame extracted struct enough?
- Will pipeline compilation be async from the start?
- How are draw items represented for later GPU-driven rendering (MASTER-000 §9.2 advanced)?

## References
- Bevy repository: https://github.com/bevyengine/bevy
- `bevy_render` docs: https://docs.rs/bevy_render
- Bevy rendering architecture overview in the repo's `docs/` and cheat book: https://bevy-cheatbook.github.io/gpu/intro.html (community, may be outdated; verify)
- Bevy 0.6 release notes ("New Renderer" and Extract/Prepare/Queue/Render design): https://bevyengine.org/news/bevy-0-6/ (verify)
- wgpu docs: https://docs.rs/wgpu
