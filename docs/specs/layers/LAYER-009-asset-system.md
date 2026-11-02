# LAYER-009: Asset System

| Field | Value |
|---|---|
| Status | Draft |
| Milestone | M6 |
| Crates | assets |
| Traces to | MASTER-000 §8.8, §12, §5.2 rule 5 |
| Last updated | 2026-11-02 |

## 1. Purpose

Identify, import, load, cache, track and hot-reload assets (glTF 2.0, PNG/JPEG/EXR, KTX2, engine formats). The import pipeline stays separate from runtime GPU resource management (MASTER-000 §12).

## 2. Scope

| In | Out |
|---|---|
| Asset ids, metadata, database, dependency tracking | GPU resource creation policy (LAYER-007; renderer uploads) |
| Importers (offline/dev) and runtime loaders | Scene representation (LAYER-010) |
| Typed asset handles with load state | Editor asset browser UI |
| Cache, hot reload, versioning, streaming hooks | |

## 3. Crates

assets (Layer 4), introduced at M6 (earlier simple loaders may live in examples). Depends on layers 1-3 as needed (e.g. mesh data types), never on rhi-vulkan.

## 4. Responsibilities / 5. Non-responsibilities

- Responsible: stable `AssetId`, import -> processed artifact -> runtime load, dependency graph, asynchronous loading on workers, error reporting on corrupt assets (M7).
- Not responsible: holding `vk` objects (rule 5), deciding GPU residency, rendering.

## 6. Dependencies

Allowed: layers 1-3 (foundation, task, mesh, material data types, rhi types if needed for descriptors). Forbidden: rhi-vulkan, layer 5.

## 7. Key concepts and data flow

```
 source file -> importer -> processed artifact (+ .meta) -> AssetDatabase
 request(AssetId) -> worker load -> CPU asset (MeshData, TextureData) -> Handle<Asset<T>> state Loaded
 renderer observes Loaded -> creates GPU resources via RHI -> keeps its own AssetId -> GpuHandle map
```

## 8. Public interface sketch (concept)

```rust
pub struct AssetId(u128);                                  // stable, content- or path-derived (open)
pub type AssetHandle<T> = Handle<T>;                       // reuse foundation Handle (ADR-0006) 
pub trait Asset: Send + Sync + 'static { const KIND: &'static str; }
pub struct AssetServer { /* ... */ }
impl AssetServer { pub fn load<T: Asset>(&self, path: &str) -> AssetHandle<T>; pub fn state<T: Asset>(&self, h: AssetHandle<T>) -> LoadState; pub fn get<T: Asset>(&self, h: AssetHandle<T>) -> Option<&T>; }
```

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: the database owns CPU assets; handles are generational keys; consumers receive borrowed views or `Arc`.
- Threading: loading and importing on workers; results published to the main thread; no shared mutable scene state.
- Errors: `AssetError { Io, Format, Version, Dependency, Corrupt }`; a failed load yields `LoadState::Failed` with a reason, never a panic.
- Diagnostics: load times, cache hit rate, pending queue length, dependency dump.

## 13. Testing strategy

Importer golden tests on small glTF/PNG fixtures; corruption fuzzing (M7); hot-reload integration test; ID stability test.

## 14. Acceptance criteria (Draft, M6)

- [ ] glTF cube with texture loads asynchronously and renders through the renderer.
- [ ] Corrupt file reports an error and the app continues.
- [ ] No GPU type appears in assets public API.

## 15. Open questions

1. AssetId scheme (path hash, UUID in `.meta`, content hash).
2. Serialization formats and versioning (serde + RON/TOML vs binary).
3. Async model: threads+channels vs an executor; relation to task.
4. Processed artifact storage and cache invalidation.
5. Ownership of GPU upload scheduling: renderer pulls from assets vs assets pushes.

## 16. Related modules

None defined yet.
