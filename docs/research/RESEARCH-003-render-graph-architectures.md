# RESEARCH-003: Render Graph Architectures

| | |
|---|---|
| Status | Draft |
| Related | MASTER-000 §8.6, §9.3, §19; RESEARCH-001, RESEARCH-002 |
| Last updated | 2026-11-02 |

## Summary
A render graph describes a frame as passes plus the resources they read and write, then derives scheduling, barriers, transient memory and pass culling from that description. Frostbite's FrameGraph (2017) popularized the three-phase model (setup, compile, execute). Unreal's RDG, Themaister's Granite and Bevy's render graph differ in how much they automate. Anvil's spec asks for an execution system, not just pass callbacks (MASTER-000 §9.3).

## Concepts

### Three phases (Frostbite model)
1. **Setup**: each pass declares, without touching the GPU, which resources it creates, reads and writes (virtual resources). Execution lambdas are recorded but not run.
2. **Compile**: the graph is analyzed on the CPU: reference counting culls passes whose outputs are never consumed; resource lifetimes (first/last use) are computed; transient resources are assigned physical memory (aliasing); barriers and layout transitions are derived.
3. **Execute**: passes run in order, resources are materialized lazily, barriers are emitted, command buffers recorded.

### Barrier derivation
For each resource the compiler tracks last usage state (access, stage, layout, queue). When the next pass declares a different usage, a barrier is generated (or merged with others at the same point). With read/write declarations only, it can also detect hazards and invalid reads of uninitialized resources (MASTER-000 §9.3 "read/write validation").

### Transient aliasing
Virtual resources with non-overlapping lifetime intervals share memory. Needs lifetime ranges from compile and an aliasing-capable allocator (RESEARCH-002). Imported resources (swapchain, persistent history textures for TAA) are excluded.

### Pass culling
Passes with no path to an exported/imported output are dropped, together with resources only they touch. Needs explicit marking of side-effecting passes (readback, present).

### Async compute
Compiler assigns passes to queues, then inserts cross-queue semaphores and ownership transfers where needed (RESEARCH-001). Gains depend on workload overlap; the compiler must avoid creating more sync points than it hides.

## Analysis / Comparison

| Aspect | Frostbite FrameGraph | Unreal RDG | Granite (Themaister) | Bevy render graph |
|---|---|---|---|---|
| Source | O'Donnell & Barczak, GDC 2017 | Unreal Engine source / docs | Open source (Granite) and blog posts | Open source (`bevy_render`) |
| Declaration | Setup lambdas declare create/read/write | Pass parameter structs declare resource access (`FRDGBuilder::AddPass`) | Passes declare attachment/texture/buffer inputs and outputs; graph is baked | Nodes with named slots and edges; mostly manual |
| Compile | Cull, lifetimes, aliasing, barriers | Cull, barriers, transient allocation, pass merging for mobile, parallel recording | Bake: dependency sort, barrier and layout derivation, optional render-pass merging, aliasing of physical resources | None to speak of; node ordering from edges |
| Barriers | Derived | Derived | Derived | Left to wgpu (automatic tracking) |
| Aliasing | Yes | Yes (transient resource allocator) | Yes (physical resource aliasing) | wgpu-managed, no explicit aliasing |
| Async compute | Yes (designed for) | Yes | Supports queue assignment | No (wgpu has one queue) |
| Culling | Ref-count | Yes | Dead passes pruned (verify) | None in graph; scheduling is per node |
| Dynamic per frame | Rebuilt every frame | Rebuilt every frame | Built once, rebaked on change | Static graph plus sub-graphs; phases built per frame |

Unreal and Granite details are from memory of public sources; confirm before relying on them (verify).

### Rebuild per frame vs bake
| Approach | Pro | Con |
|---|---|---|
| Rebuild every frame | Fully dynamic; simple data flow | CPU cost scales with passes; allocations |
| Bake and cache until config changes | Cheap steady state | Invalidation logic; harder dynamic features |

Graphs of tens to a few hundred passes are normally cheap to rebuild; measure before optimizing (MASTER-000 §22.3).

### Automation levels
| Level | Description | Example |
|---|---|---|
| 0 | Ordered pass list, manual barriers | Hand-written frame |
| 1 | Pass list + automatic barriers | Minimal useful graph |
| 2 | + culling + transient aliasing | FrameGraph / RDG core |
| 3 | + async compute, pass merging, parallel recording | Mature engines |

## Implications for Anvil
- Target level 2 in M4 (MASTER-000 §14); design resource/handle types (ADR-0006) so virtual resources can be declared before materialization.
- Barrier derivation is the main benefit given dynamic rendering requires explicit transitions (ADR-0004).
- Keep setup declarative (typed read/write usage per pass) so validation and debug visualization come for free.
- Defer async compute; design the pass description so a queue hint can be added later without API change.
- Frames before M4 may use a hand-written pass list with hand-placed barriers wrapped in a small barrier helper.
- The graph lives above `rhi` and must use only RHI types.

## Open questions
- Rebuild each frame or bake? Decide after a prototype is measured.
- How are persistent/history resources (TAA) imported and versioned?
- Do passes record into one command buffer or one per pass (for later parallel recording)?
- Generic-over-backend graph (ADR-0005) and compile-time cost?

## References
- Yuriy O'Donnell, "FrameGraph: Extensible Rendering Architecture in Frostbite", GDC 2017 (slides on GDC Vault; verify direct URL): https://www.gdcvault.com/play/1024612/FrameGraph-Extensible-Rendering-Architecture-in
- Unreal Engine docs, Render Dependency Graph: https://dev.epicgames.com/documentation/en-us/unreal-engine/render-dependency-graph-in-unreal-engine
- Hans-Kristian Arntzen, "Render graphs and Vulkan - a deep dive": https://themaister.net/blog/2017/08/15/render-graphs-and-vulkan-a-deep-dive/
- Granite repository: https://github.com/Themaister/Granite
- Bevy render graph docs: https://docs.rs/bevy_render/latest/bevy_render/render_graph/index.html
