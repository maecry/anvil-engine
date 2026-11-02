# MODULE-009: Render Graph Compiler

| Field | Value |
|---|---|
| Status | Accepted (ADR-0012); M4a implemented |
| Crate | render-graph |
| Layer | LAYER-006 |
| Milestone | M4 (M4a graph + M3 port; M4b-c features) |
| Traces to | FR-RG-001..FR-RG-009, FR-RES-011, FR-REND-018, NFR-TEST-001, NFR-PERF-011, NFR-OBS-001; MASTER-000 §8.6, §9.3, §10.3; ADR-0005, ADR-0012; RESEARCH-001, RESEARCH-003 |
| Last updated | 2026-11-02 |

## 1. Purpose

Compile a declared set of passes and resource accesses into a deterministic, validated schedule with planned resource transitions and transient resource lifetimes. It is an execution system, not a list of callbacks (MASTER-000 §9.3; FR-RG-009). It is pure CPU logic over `rhi` types and never mentions Vulkan.

## 2. Responsibilities

- Pass registration and texture declarations (imported textures with an optional initial `TextureState` and an extent; transient textures with a `TransientDesc`) through `GraphBuilder` and `Pass` (FR-RG-001, FR-RG-002).
- Validation with the pass and texture named (FR-RG-004): read before write, read and write of one texture in a pass, a texture bound twice, imported texture without a state, attachment extent mismatch, export of a never-written texture.
- Ordering by declaration with a dependency check (a read must follow its writer); roots are `export`s and `side_effect()` passes, everything else is culled and reported (FR-RG-003, FR-RG-005).
- Lifetime ranges per transient; slot sharing between disjoint lifetimes of identical descriptors; a persistent pool across frames (FR-RG-007, FR-RES-011).
- Barrier planning from declared usage against the tracked `TextureState` (FR-RG-006).
- Execution: per live pass barriers, debug label, `begin_rendering`/`end_rendering`, viewport and scissor, then the body; export barriers; state map and pool update.
- `describe` and `ANVIL_GRAPH_DUMP` (FR-RG-008).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Concrete passes (forward, shadow, ...) | renderer (LAYER-005) |
| Vulkan barrier masks | rhi-vulkan maps `TextureState` pairs (MODULE-004) |
| Pipeline/shader creation | MODULE-007/008 |
| Graph visualiser UI | debug (layer 5) |
| Buffer state tracking, async compute, multi-queue, memory aliasing | M5 |
| Persistent GPU resource ownership | imported resources stay with their creators |
| Meshes, materials, draw lists | passes capture renderer data in their closures (resolves Q7) |

## 4. Dependencies

Allowed: foundation, rhi (plus `tracing`, `thiserror`). Forbidden: rhi-vulkan (dev-dependency for the GPU test only), `ash`, renderer, layers 4-5 (LAYER-006 §6).

## 5. Public API (as built)

See LAYER-006 §8 for the full listing. Typical use (the renderer's frame):

```rust
let mut b = GraphBuilder::<D>::new();
let shadow = b.import("shadow", shadow_tex, (2048, 2048), None);          // remembered state
let sc = b.import("swapchain", image.texture, image.extent, Some(TextureState::Undefined));
let hdr = b.transient("hdr", TransientDesc { width, height, format, usage });
b.pass(Pass::new("forward").read(shadow).color(hdr, ColorTarget::clear(c)).depth(depth, d),
       move |ctx| { ctx.cmd.bind_pipeline(p); /* draws */ Ok(()) });
b.export(sc, final_state);                                                  // Present or TransferSrc
let compiled = graph.compile(b)?;
let stats = graph.execute(compiled, &device, &mut cmd, &frame)?;
```

Errors: `GraphError { ReadBeforeWrite, ReadAndWrite, BoundTwice, NoInitialState, ExtentMismatch, ExportUnwritten, Rhi }`.

## 6. Data structures

`plan.rs` (pure): `Decls { res: Vec<ResDecl>, passes: Vec<Pass>, exports }` in; `Plan { live, order, barriers: Vec<Barrier { before, tex, from, to }>, lifetime, slot_of, slots, slot_final, res_final }` out. `pool.rs`: `TransientPool { entries: Vec<Entry { desc, texture, state, last_used }> }`. `describe.rs`: `GraphDescription` (plain data, DOT and JSON writers). `lib.rs`: builder, `CompiledGraph`, `RenderGraph { states: HashMap<TextureHandle, TextureState>, pool }`, execution. Compile output is a pure function of the declarations and the remembered states.

## 7. Ownership and lifetime

Builder and compiled graph are per frame and borrow frame data for `'a`. `RenderGraph` persists: the state map (import states are updated after `execute`) and the pool. Imported textures are never destroyed by the graph; call `forget` first when the owner destroys one (only imports declared with `initial: None` have their final state remembered; swapchain images use `Some(Undefined)` every frame and never enter the map).

## 8. Execution flow

Compile (`plan::plan`): (1) validate structure; (2) cull from roots through last writers of each input; (3) lifetimes over live positions, exports live to the end; (4) assign slots (same descriptor, previous owner's last use earlier than the new first use); (5) simulate states in order: check data flow and imports' states, emit a barrier when the tracked state differs or the use is a write, then export barriers. Execute: acquire pool entries per slot; per live pass emit its barriers (a pooled texture's first barrier starts from the entry's remembered state), `begin_label`, rendering scope, body, `end_rendering`, `end_label`; export barriers; store final states; evict stale entries.

## 9. Threading model

Build and compile are single-threaded CPU work and may run on a worker thread; they take no GPU locks. Parallel pass recording is M5.

## 10. Error handling

`GraphError` (thiserror), names in every message; invalid graphs never reach the GPU. A failing pass body or texture creation is `GraphError::Rhi { pass, source }`; the partially recorded command buffer must be dropped by the caller.

## 11. Performance

Compile time is measured every frame (`GraphStats::compile_time`). pbr-scene: 1-5 us release, 22 us debug (3 passes steady state, 12 on the bake frame), below the 0.1 ms revisit threshold of ADR-0012 §5. Per frame allocations: one box per pass, small vectors in the declarations; not yet allocation-free (not a promise).

## 12. Testing requirements

As LAYER-006 §13. Unit tests: `plan::tests` (ordering and barrier states, culling, validation messages, slot sharing), `pool::tests` (retirement, eviction). GPU: `crates/render-graph/tests/gpu.rs`.

## 13. Acceptance criteria (M4a)

- [x] Same input produces identical compiled output.
- [x] Invalid graphs are rejected with a named pass/resource.
- [x] The M3 pipeline runs as a graph without behaviour change (pbr-scene screenshots pixel-identical, 0 validation errors and warnings with sync validation, 0 errors with GPU-AV).
- [x] `describe()` output renders the graph in a debug dump (`ANVIL_GRAPH_DUMP`).
- [ ] Deferred and post-processing graphs validation-clean (M4b, M4c).

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RG-001, FR-RG-002 | builder API |
| FR-RG-003, FR-RG-005 | dependency check, declaration order, culling |
| FR-RG-004 | validation errors |
| FR-RG-006 | barrier planning on `TextureState` |
| FR-RG-007, FR-RES-011 | transient pool and slots |
| FR-RG-008 | `describe`, `ANVIL_GRAPH_DUMP` |
| FR-RG-009 | compile/execute architecture |
| NFR-TEST-001 | dependency-resolution unit tests |

## 15. Open questions (resolved by ADR-0012)

1. Resource state model: textures only, whole-texture; buffers in M5.
2. Cache compiled graphs: no, recompile each frame (measured, see §11).
3. Transient aliasing: pool + lifetime reuse; no memory aliasing until M5.
4. Pass storage: boxed `FnOnce` per pass, bounded exception to ADR-0005.
5. Multi-queue: M5.
6. Final state of the swapchain image: the caller passes it to `Renderer::render`, which `export`s it (`Present` or `TransferSrc`).
7. Draw lists: passes capture renderer data in their closures; the graph never sees meshes.
