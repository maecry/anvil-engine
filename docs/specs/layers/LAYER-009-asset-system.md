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

assets (Layer 4), introduced at M6a (earlier simple loaders live in examples). Depends on layers 1-3 as needed (e.g. mesh data types), never on rhi-vulkan.

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

## 8. Public interface (M6a, as built)

Decisions: [ADR-0014](../../decisions/ADR/0014-m6-runtime-and-tooling.md) §3-6. The crate is GPU-free and depends only on `foundation` and plain data crates (`gltf`, `image`, `ktx2`, `ron`, `serde`, `uuid`, `notify`).

```rust
pub struct AssetId(pub u128);            // UUID in `<file>.meta`; AssetId::NIL = "use the built-in default"
impl AssetId { pub fn random() -> Self; pub fn derive(parent: AssetId, name: &str) -> Self /* UUID v5 */ }

pub enum Asset { Mesh(Arc<MeshAsset>), Texture(Arc<TextureAsset>), Material(Arc<MaterialAsset>), Prefab(Arc<PrefabAsset>), Scene(Arc<SceneAsset>) }
pub enum LoadState { Loading, Loaded, Failed(AssetError) }
pub enum AssetEvent { Loaded(AssetId), Reloaded(AssetId), Failed(AssetId) }

impl AssetServer {
    pub fn new(project_dir) -> Result<Self, AssetError>;      // scans assets/**, creates missing .meta
    pub fn load_path(&mut self, rel_path: &str) -> Result<AssetId, AssetError>;
    pub fn load(&mut self, id: AssetId) -> LoadState;         // starts the import on rayon if needed
    pub fn state(&self, id) -> Option<LoadState>;  pub fn get(&self, id) -> Option<Asset>;
    pub fn update(&mut self) -> Vec<AssetEvent>;              // main thread: drain results, resolve dependencies, hot reload
    pub fn enable_hot_reload(&mut self) -> Result<(), AssetError>;
    pub fn stats(&self) -> AssetStats;  pub fn list(&self) -> Vec<AssetInfo>;
}
```

- **Formats.** `.gltf`/`.glb` (meshes with generated normals and tangents when absent, PBR materials, images, node hierarchy as a `PrefabAsset`; extra primitives become child nodes), `.png`/`.jpg`/`.jpeg`/`.exr` (decoded to RGBA8), `.ktx2` (uncompressed RGBA8 only, anything else is `AssetError::Format`), `.material` and `.scene` (RON with `version`; a newer version is `AssetError::Version`).
- **Identity.** A source file gets a random id in `<file>.meta` (RON, `version`) on first discovery; moving the file with its `.meta` keeps the id; a copied `.meta` (duplicate id) gets a new id for the later file. glTF sub-assets are `AssetId::derive(parent, "mesh/<m>/<p>" | "material/<i>" | "image/<i>")` and are listed in the parent's `.meta` after the first import.
- **Cache.** `<project>/.anvil/imported/<id>.bin`: a `bytemuck` header (magic, importer version, FNV-1a hash of the source) plus the bundle's entries (raw arrays for meshes and images, RON for the small text assets). Any mismatch, damage or version change reimports. The directory is never committed.
- **Dependencies.** Material to textures and prefab to meshes/materials are hard (a failed dependency makes the dependent `Failed(Dependency)`; the dependent is `Loaded` only when all are). Scene to meshes/materials is soft: they are loaded, but one bad mesh does not fail the scene (the renderer shows fallbacks).
- **Hot reload.** `notify` on `assets/`, 200 ms debounce, access events ignored. A changed loaded asset is reimported and reported as `Reloaded`, followed by its hard dependents, dependencies first. The old contents are served until the new ones are ready.
- **Corrupt input.** Every importer returns `Result`; the worker also catches an importer panic and reports `AssetError::Corrupt`. Tests: truncated glb, garbage PNG/KTX2, broken and newer-version material and scene.
- **Diagnostics.** `AssetStats { pending, loaded, failed, cache_hits, cache_misses, total_import_ms, last_import_ms }` and a `tracing` info line per import.

CPU mesh and texture types are defined here (`MeshAsset` has separate attribute arrays); the runtime interleaves them into `renderer::MeshData`, so neither crate depends on the other.

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: the database owns CPU assets; handles are generational keys; consumers receive borrowed views or `Arc`.
- Threading: loading and importing on workers; results published to the main thread; no shared mutable scene state.
- Errors: `AssetError { Io, Format, Version, Dependency, Corrupt }`; a failed load yields `LoadState::Failed` with a reason, never a panic.
- Diagnostics: load times, cache hit rate, pending queue length, dependency dump.

## 13. Testing strategy

Importer golden tests on small glTF/PNG fixtures; corruption fuzzing (M7); hot-reload integration test; ID stability test.

## 14. Acceptance criteria (M6a)

- [x] glTF cube with texture loads asynchronously and renders through the renderer (`samples/sample-project`, `anvil-player` headless screenshot).
- [x] Corrupt file reports an error and the app continues (`assets/tests/assets.rs`, `runtime/tests/project.rs::broken_assets_fall_back_instead_of_failing`).
- [x] No GPU type appears in assets public API (`cargo tree -p assets` has no rhi, rhi-vulkan, ash or renderer).
- [x] `AssetServer::rescan` registers files created while running (the editor's create and import commands; `assets/tests/assets.rs::rescan_registers_only_new_files`).
- [x] The editor creates a material, edits it with a live preview and saves it without the hot reload fighting the save (`editor/tests/project_lifecycle.rs`).
- [x] Ids survive moves, cache hits on the second load and reimports on change, failed dependencies propagate, hot reload (unit and integration tests).

## 15. Open questions (answered by ADR-0014)

1. AssetId scheme: random UUID in the `.meta` sidecar; sub-assets derived with UUID v5 (ADR-0014 §4).
2. Formats and versioning: serde + RON with a `version: u32` field for project, scene, material and meta; the cache is binary (§3).
3. Async model: rayon workers and a channel drained by `AssetServer::update` on the main thread (§5). No executor.
4. Artifact storage: `.anvil/imported/<id>.bin`, keyed by importer version and source hash (§5).
5. GPU upload: the renderer side pulls; the runtime owns the `AssetId -> GPU id` bridge and creates resources when an asset is `Loaded` (§5, LAYER-010).

Remaining, deferred: BCn/ASTC KTX2 (needs compressed RHI formats), float (HDR) textures, importer settings in `.meta`, hashing external glTF buffers and images into the cache key, a `.meta` repair tool, asset streaming from disk (M7).

## 16. Related modules

None defined yet.
