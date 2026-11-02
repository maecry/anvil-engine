# LAYER-006: Render Graph

| Field | Value |
|---|---|
| Status | Accepted (ADR-0012); M4 complete |
| Milestone | M4 (complete: M4a graph and M3 port, M4b prepass/clustered/deferred/alpha/cascades, M4c SSAO/TAA/bloom) |
| Crates | render-graph |
| Traces to | MASTER-000 §8.6, §9.3, §10.3, §14 M4 |
| Last updated | 2026-11-02 |

## 1. Purpose

Describe a frame as passes plus declared resource reads/writes; compile it into an ordered schedule with resource transitions and transient allocation. A rendering execution system, not a list of callbacks (MASTER-000 §9.3).

## 2. Scope

| In | Out |
|---|---|
| Pass registration, resource declarations (imported and transient) | Concrete passes (LAYER-005) |
| Dependency analysis, read/write validation, culling unused passes | Pipeline/shader creation (LAYER-008) |
| Execution order, barrier/transition planning against RHI `TextureState` | Vulkan barriers (backend maps states) |
| Transient texture pool with lifetime-based reuse (memory aliasing M5) | Async compute scheduling (M5) |
| Graph dump for debug visualisation (plain data) | The visualiser UI (layer 5) |

## 3. Crates

render-graph (Layer 3), introduced at M4. Depends on foundation and rhi (plus `tracing`, `thiserror`). Nothing else.

## 4. Responsibilities / 5. Non-responsibilities

- Responsible: declare -> compile -> execute phases; validation errors (read-before-write, read and write in one pass, imports without state, extent mismatch, unwritten exports); producing a `CompiledGraph` with barriers.
- Not responsible: knowing what a mesh or material is; GPU object creation policy beyond transient resources; backend-specific sync.

## 6. Dependencies

Allowed: foundation, rhi. Forbidden: rhi-vulkan, renderer (renderer depends on the graph), layers 4-5.

## 7. Key concepts and data flow

```
 GraphBuilder<'a, D>: transient(name, desc) / import(name, tex, extent, initial) / export(tex, state)
                      pass(Pass::new(name).read(..).color(..).depth(..), |ctx| body)
   -> RenderGraph::compile(builder): validate, cull, order, plan barriers and transient slots
   -> RenderGraph::execute(compiled, dev, cmd, frame): per live pass: barriers, label, begin_rendering,
      viewport/scissor, body, end_rendering; then export barriers; pool/state bookkeeping
```

Decisions (ADR-0012 §1-10), all implemented in M4a:

- Whole-texture state only; buffers are not tracked (M5, with compute). Imported textures keep a persistent state map in the `RenderGraph`; `import(.., None)` uses the remembered state, a fresh texture is registered with `RenderGraph::track(tex, Undefined)`.
- The graph is rebuilt and compiled every frame (no cache). Order is declaration order with a dependency check (a read of a texture that nothing earlier wrote is `ReadBeforeWrite`, naming a later writer if there is one). Roots are `side_effect()` passes and `export`ed textures; other passes are culled and reported.
- Transients come from a `TransientDesc`-keyed pool. Within a frame a texture is shared after its last user when descriptors match; across frames a pooled texture is reused once the frame slot that last used it has retired (`last_used + frames_in_flight <= frame`); unused entries are destroyed after 8 frames. No memory aliasing (`// shortcut:`, M5).
- Pass bodies are `Box<dyn FnOnce(&mut PassContext<'_, D>) -> Result<(), RhiError> + 'a>`, one allocation per pass per frame (bounded exception to ADR-0005).
- Raster passes are declarative: `Pass::color(tex, ColorTarget)` / `Pass::depth(tex, DepthTarget)` carry load, store and clear; the graph does `begin_rendering`/`end_rendering`, viewport and scissor (attachment extent). A pass without attachments gets no rendering scope.
- Barriers: one per use whose state differs from the tracked state, plus one for every attachment use (consecutive writes in the same state still need an execution dependency). A pooled texture's first barrier starts from the state its last frame left it in (the backend tracks it and warns on mismatch).
- Compile is a pure function (`plan.rs`, no device) unit-tested without a GPU; execution adds the device, pool and state map.

## 8. Public interface (as built, M4a)

```rust
pub struct TexId;                                   // per-frame texture id
pub struct TransientDesc { pub width: u32, pub height: u32, pub format: Format, pub usage: TextureUsage }
pub struct ColorTarget { pub load: LoadOp, pub store: StoreOp, pub clear: [f32; 4] }   // ::clear(c), ::dont_care()
pub struct DepthTarget { pub load: LoadOp, pub store: StoreOp, pub clear: f32 }
pub struct Pass;   // Pass::new(name).label_color(c).read(t).color(t, ColorTarget).depth(t, DepthTarget).side_effect()

pub struct GraphBuilder<'a, D: Device>;
impl GraphBuilder<'_, D> {
    pub fn new() -> Self;
    pub fn transient(&mut self, name, desc: TransientDesc) -> TexId;
    pub fn import(&mut self, name, tex: TextureHandle, extent: (u32, u32), initial: Option<TextureState>) -> TexId;
    pub fn export(&mut self, tex: TexId, final_state: TextureState);
    pub fn pass(&mut self, pass: Pass, body: impl FnOnce(&mut PassContext<'_, D>) -> Result<(), RhiError> + 'a);
}
pub struct PassContext<'c, D> { pub cmd: &'c mut D::Commands, pub device: &'c D }
impl PassContext { pub fn texture(&self, TexId) -> TextureHandle; pub fn texture_index(&self, TexId) -> Result<u32, RhiError>; }

pub struct RenderGraph;                             // persistent: state map + transient pool (not generic over D)
impl RenderGraph {
    pub fn new(frames_in_flight: u32) -> Self;
    pub fn track(&mut self, tex: TextureHandle, state: TextureState);
    pub fn forget(&mut self, tex: TextureHandle);
    pub fn compile<'a, D: Device>(&self, b: GraphBuilder<'a, D>) -> Result<CompiledGraph<'a, D>, GraphError>;
    pub fn execute<D: Device>(&mut self, c: CompiledGraph<'_, D>, dev: &D, cmd: &mut D::Commands, frame: &FrameToken) -> Result<GraphStats, GraphError>;
    pub fn destroy<D: Device>(self, dev: &D);       // pooled textures only
}
impl CompiledGraph { pub fn stats(&self) -> GraphStats; pub fn describe(&self) -> GraphDescription; }
pub struct GraphStats { passes, culled, barriers, transients, slots, compile_time, textures_created, pool_size }
pub struct GraphDescription { passes: Vec<PassDesc>, resources: Vec<ResourceDesc>, barriers: Vec<BarrierDesc> }  // .to_dot(), .to_json()
```

Deviations from the ADR-0012 text: `import` also takes the texture `extent` (the RHI cannot query it, and the graph needs it for the rendering scope and size validation); `RenderGraph` is not generic over `D` (only `GraphBuilder`, `CompiledGraph` and `PassContext` are); bodies return `Result<(), RhiError>` so `texture_index` can use `?`; `execute` takes the `FrameToken` (pool retirement) and returns `GraphStats`.

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: the builder and compiled graph live for one frame and may borrow frame data (`'a`); transient textures are created through the RHI by the pool and destroyed by `RenderGraph::destroy` or eviction (deferred by the device); imported textures stay owned by their creator, which calls `forget` before destroying one.
- Threading: build/compile are single-threaded pure CPU work and can run on a worker; parallel pass recording is M5.
- Errors: `GraphError { ReadBeforeWrite, ReadAndWrite, BoundTwice, NoInitialState, ExtentMismatch, ExportUnwritten, Rhi { pass, source } }`, every variant naming the pass and/or texture. There is no cycle error: order is declaration order and a later writer cannot feed an earlier reader (`ReadBeforeWrite`).
- Diagnostics: compile summary at `debug` (passes, culled, barriers, transients, slots, `compile_us`), one `trace` per executed pass and the whole `GraphDescription` at `trace`. `ANVIL_GRAPH_DUMP=path.dot|.json` writes the first compiled frame once (DOT: passes as boxes, culled dashed, imports as octagons, edges labeled with the planned barrier). Each pass gets a debug label region (name and color).

## 13. Testing strategy

Pure unit tests with no GPU on the compile step (`plan.rs`) and the pool (`pool.rs`): declaration order and exact barrier from/to states, transitive culling, same-state write barriers, remembered import state, each validation error message (pass and resource named), transient slot sharing. GPU integration test `tests/gpu.rs` (gated by `ANVIL_GPU_TESTS=1`, sync validation on): eight frames of a multi-pass graph with a persistent import, a culled pass, an export, pool reuse after the frames in flight, zero validation messages. The renderer port is checked by an identical (pixel-for-pixel) `pbr-scene` screenshot before and after. Property tests on random DAGs are not done.

## 14. Acceptance criteria (M4a)

- [x] Deterministic compile output for the same input (pure function of declarations and remembered states).
- [x] Invalid graphs rejected with a named pass/resource.
- [x] The M3 forward pipeline (shadow, IBL bakes, forward + skybox, tonemap) runs as a graph without behaviour change; no hand-written barriers remain in `crates/renderer`.
- [x] `describe()`, DOT and JSON dumps.
- [x] Deferred, prepass and post-processing graphs validation-clean: sync validation, 0 errors and 0 warnings for forward and deferred in all 8 SSAO/TAA/bloom combinations (and a run that cycles them every 3 frames, exercising the TAA reset path); GPU-AV 0 errors. pbr-scene builds 27 passes per frame with everything on (13 in M4b), compile 4 us in release.

## 15. Open questions (resolved by ADR-0012, 2026-11-02)

1. Resource state model: **resolved**. Textures only, whole-texture state. Buffers are not tracked (host-written or pre-uploaded); buffer tracking arrives with compute in M5. Features that need several levels use separate textures.
2. Cache compiled graphs across frames: **resolved**. No; rebuild and compile every frame. Measured on pbr-scene: 1-5 us in release (22 us in debug) for 3 passes (12 on the first frame), far below the 0.1 ms revisit threshold.
3. Transient aliasing: **resolved**. Pool keyed by `TransientDesc`, lifetime-based reuse within a frame, reuse across frames after the frame slot retired, eviction after 8 unused frames. No memory aliasing (`// shortcut:`; needs RHI aliasing support, M5).
4. Closure-based passes vs trait objects: **resolved**. Boxed `FnOnce` per pass per frame; explicit, bounded exception to ADR-0005 (the rule targets per-draw hot paths).
5. Multi-queue / async compute: **resolved** as deferred to M5 (one graphics queue).

## 16. Related modules

MODULE-009-render-graph-compiler.
