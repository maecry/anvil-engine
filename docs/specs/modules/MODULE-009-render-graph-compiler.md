# MODULE-009: Render Graph Compiler

| Field | Value |
|---|---|
| Status | Draft |
| Crate | render-graph |
| Layer | LAYER-006 |
| Milestone | M4 |
| Traces to | FR-RG-001..FR-RG-009, FR-RES-011, FR-REND-018, NFR-TEST-001, NFR-PERF-011, NFR-OBS-001; MASTER-000 §8.6, §9.3, §10.3; ADR-0005; RESEARCH-001, RESEARCH-003 |
| Last updated | 2026-11-02 |

## 1. Purpose

Compile a declared set of passes and resource accesses into a deterministic, validated schedule with planned resource transitions and transient resource lifetimes. It is an execution system, not a list of callbacks (MASTER-000 §9.3; FR-RG-009). It is pure CPU logic over `rhi` types and never mentions Vulkan.

## 2. Responsibilities

- Pass registration and resource declarations (imported textures with an initial `TextureState`; transient textures with a descriptor) through `GraphBuilder` / `PassBuilder`.
- Validation: undeclared access, read-before-write, cycles, invalid resource ids, duplicate names; each error names the pass and resource (FR-RG-004).
- Dependency tracking and topological ordering with a deterministic tie-break (registration order) (FR-RG-003, FR-RG-005).
- Culling of passes whose outputs are unused (roots: imported resources that are marked as outputs, e.g. the swapchain image).
- Lifetime ranges per transient resource; allocation/aliasing plan for transient resources (FR-RG-007, FR-RES-011).
- Barrier planning: derive `texture_barrier(from, to)` calls from declared usages against RHI `TextureState` (FR-RG-006); minimise redundant barriers.
- `execute`: per pass, emit barriers, then run the pass record closure on the RHI encoder.
- `describe`: plain-data `GraphDescription` (passes, resources, edges, barriers) for debug visualisation (FR-RG-008).

## 3. Non-responsibilities

| Not here | Where |
|---|---|
| Concrete passes (forward, shadow, ...) | renderer (LAYER-005) |
| Vulkan barrier masks | rhi-vulkan maps `TextureState` pairs (MODULE-004) |
| Pipeline/shader creation | MODULE-007/008 |
| Graph visualiser UI | debug (layer 5) |
| Async compute and multi-queue scheduling | M5+ (open) |
| Persistent GPU resource ownership | imported resources stay with their creators |

## 4. Dependencies

Allowed: foundation, rhi. Forbidden: rhi-vulkan, `ash`, renderer, layers 4-5 (LAYER-006 §6).

## 5. Public API

Concept (LAYER-006 §8); exact closure and lifetime design is open:

```rust
pub struct GraphBuilder; pub struct CompiledGraph; pub struct ResourceId; pub struct PassId;
impl GraphBuilder {
    pub fn import_texture(&mut self, name: &str, h: TextureHandle, initial: TextureState) -> ResourceId;
    pub fn create_texture(&mut self, name: &str, desc: TransientTextureDesc) -> ResourceId;
    pub fn mark_output(&mut self, r: ResourceId, final_state: TextureState);
    pub fn add_pass<F>(&mut self, name: &str, setup: F) -> PassId;      // setup declares reads/writes and returns the record closure
    pub fn compile(self) -> Result<CompiledGraph, GraphError>;
}
impl CompiledGraph {
    pub fn execute<D: Device>(&self, dev: &mut D, enc: &mut D::Commands) -> Result<(), GraphError>;
    pub fn describe(&self) -> GraphDescription;
}
pub enum GraphError { Cycle, UndeclaredAccess, ReadBeforeWrite, InvalidResource, Rhi(RhiError) }   // each carries pass/resource names
```

## 6. Data structures

`Pass { name, reads: Vec<(ResourceId, Usage)>, writes: Vec<(ResourceId, Usage)>, record: Box<dyn FnOnce> }` (boxed closure vs generics undecided); `Resource { name, kind: Imported{handle, initial} | Transient{desc}, first_use, last_use }`; `Usage` maps to a `TextureState`; `CompiledGraph { order: Vec<PassId>, barriers: Vec<Vec<Barrier>>, transient_plan, culled: Vec<PassId> }`. Compilation output is a pure function of the builder input (determinism, LAYER-006 §14).

## 7. Ownership and lifetime

The graph borrows nothing across frames. Transient resources are created through the RHI at execute (or from a pool) and destroyed via deferred destruction; imported resources are never destroyed by the graph. Recompile per frame vs cached by structural hash is undecided.

## 8. Execution flow

Build: passes declare accesses. Compile: (1) validate declarations; (2) build edges (writer -> later reader/writer, reader -> later writer); (3) detect cycles; (4) cull from outputs; (5) topological sort with registration-order tie-break; (6) compute lifetime ranges; (7) plan transient allocation/aliasing; (8) per pass compute required transitions from tracked states, merge/skip no-ops; (9) emit `CompiledGraph`. Execute: for each pass in order: `begin_label(pass name)`, barriers via `texture_barrier`, record closure, `end_label`.

## 9. Threading model

Build and compile are single-threaded CPU work and may run on a worker thread; they take no GPU locks. Parallel pass recording is M5 and needs one encoder per thread (MODULE-004 open question 4).

## 10. Error handling

`GraphError` (thiserror) with names of the offending pass/resource; invalid graphs never reach the GPU. RHI failures during execute are wrapped in `GraphError::Rhi` (`DeviceLost` unchanged).

## 11. Performance requirements

Measure compile time and execute CPU time per frame (NFR-PERF-011); compare per-frame recompile with a cache keyed by a structural hash before adding the cache. No numeric target (NFR-PERF-012). Allocation-free steady state is a goal to verify, not a promise.

## 12. Testing requirements

Pure unit tests, no GPU (NFR-QUAL-007): ordering and tie-break determinism; cycle detection; culling of unused passes; undeclared-access and read-before-write rejection with names; barrier minimisation (no redundant transitions, merged compatible reads); transient lifetime ranges and aliasing legality; property tests on random DAGs (compile twice -> identical output; every consumer is scheduled after its producer); recorded `NullBackend` command streams compared to expected barrier lists; the M3 forward pipeline expressed as a graph yields the same command stream as direct calls.

## 13. Acceptance criteria (Draft, M4)

- [ ] Same input produces byte-identical compiled output.
- [ ] Invalid graphs are rejected with a named pass/resource.
- [ ] The M3 forward pipeline runs as a graph without behaviour change.
- [ ] Deferred rendering and post-processing graphs run validation-clean (sync validation on) on lavapipe and one real GPU.
- [ ] `describe()` output renders the graph in a debug dump.

## 14. Traceability (FR/NFR IDs)

| Requirement | How |
|---|---|
| FR-RG-001, FR-RG-002 | builder API |
| FR-RG-003, FR-RG-005 | edges, topological order |
| FR-RG-004 | validation errors |
| FR-RG-006 | barrier planning on `TextureState` |
| FR-RG-007, FR-RES-011 | transient resources |
| FR-RG-008 | `describe` |
| FR-RG-009 | compile/execute architecture |
| NFR-TEST-001 | dependency-resolution unit tests |

## 15. Open questions

1. Resource state model: textures only now; buffers and per-mip/layer ranges needed by M4 (LAYER-006 Q1).
2. Cache compiled graphs across frames (LAYER-006 Q2).
3. Transient aliasing strategy; needs RHI memory aliasing and `gpu-allocator` support (ADR-0007 consequence; verify).
4. Closure-based passes: boxed `FnOnce` per pass vs generic storage; ADR-0005 forbids `dyn` on hot paths (LAYER-006 Q4).
5. Multi-queue / async compute (M5).
6. Who decides final state of the swapchain image (`mark_output`): graph or renderer.
7. How the graph interacts with render-world draw lists (MODULE-010) without depending on renderer types.
