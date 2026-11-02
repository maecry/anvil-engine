# LAYER-006: Render Graph

| Field | Value |
|---|---|
| Status | Draft |
| Milestone | M4 |
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
| Transient resource aliasing/allocation (M4+, via RHI) | Async compute scheduling (M5+, open) |
| Graph dump for debug visualisation (plain data) | The visualiser UI (layer 5) |

## 3. Crates

render-graph (Layer 3), introduced at M4. Depends on foundation, rhi. Nothing else.

## 4. Responsibilities / 5. Non-responsibilities

- Responsible: build -> compile -> execute phases; validation errors for undeclared access, cycles, read-before-write; producing a `CompiledGraph` with barriers.
- Not responsible: knowing what a mesh or material is; GPU object creation policy beyond transient resources; backend-specific sync.

## 6. Dependencies

Allowed: foundation, rhi. Forbidden: rhi-vulkan, renderer (renderer depends on the graph), layers 4-5.

## 7. Key concepts and data flow

```
 GraphBuilder: add_pass(name, |PassBuilder| { read(res, usage); write(res, usage); }) 
      -> compile(): topological order, lifetime ranges, barrier list, culled passes
      -> execute(device, &mut commands): per pass, emit barriers then call the pass's record closure
```

Recompilation per frame vs caching by structural hash: open (Open question 2).

## 8. Public interface sketch (concept)

```rust
pub struct GraphBuilder; pub struct CompiledGraph; pub struct ResourceId; pub struct PassId;
impl GraphBuilder {
    pub fn import_texture(&mut self, name: &str, h: TextureHandle, initial: TextureState) -> ResourceId;
    pub fn create_texture(&mut self, name: &str, desc: TransientTextureDesc) -> ResourceId;
    pub fn add_pass<F>(&mut self, name: &str, setup: F) -> PassId;   // setup declares usage, returns record closure
    pub fn compile(self) -> Result<CompiledGraph, GraphError>;
}
impl CompiledGraph { pub fn execute<D: Device>(&self, dev: &mut D, enc: &mut D::Commands) -> Result<(), GraphError>; pub fn describe(&self) -> GraphDescription; }
```

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: the graph borrows nothing across frames; transient resources are created via RHI and destroyed through deferred destruction; imported resources stay owned by their creator.
- Threading: build/compile are single-threaded pure CPU work and can run on a worker; parallel pass recording is M5.
- Errors: `GraphError { Cycle, UndeclaredAccess, ReadBeforeWrite, InvalidResource, Rhi(RhiError) }`.
- Diagnostics: `GraphDescription` (passes, resources, edges, barriers) as plain data for debug; per-pass labels/GPU scopes via RHI hooks.

## 13. Testing strategy

Pure unit tests with no GPU: ordering, cycle detection, culling, barrier minimisation, transient lifetime ranges; property tests on random DAGs; recorded `NullBackend` command streams compared to expected barriers.

## 14. Acceptance criteria (Draft)

- [ ] Deterministic compile output for the same input.
- [ ] Invalid graphs rejected with a named pass/resource.
- [ ] The M3 forward pipeline expressed as a graph without behaviour change.

## 15. Open questions

1. Resource state model: only texture states now; buffers and sub-resource (mip/layer) ranges needed by M4.
2. Cache compiled graphs across frames?
3. Transient aliasing strategy (needs RHI memory aliasing, ties to ADR-0007 `gpu-allocator` support; verify).
4. Closure-based passes vs trait objects (ADR-0005 forbids `dyn` in hot paths; pass records are per-frame hot): leaning generic closures stored as boxed `FnOnce` per pass (one allocation per pass), to be measured.
5. Multi-queue / async compute: deferred to M5.

## 16. Related modules

MODULE-009-render-graph-compiler.
