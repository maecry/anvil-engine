# LAYER-010: Runtime and Tools

| Field | Value |
|---|---|
| Status | Draft |
| Milestone | M5 (profiler); M6 (scene, runtime, editor, debug) |
| Crates | scene, runtime, debug, profiler, editor |
| Traces to | MASTER-000 §5.1 (Layers 4-5), §5.2 rules 1, 4, 5, 6, §8.9-8.12, §14 M5-M6 |
| Last updated | 2026-11-02 |

## 1. Purpose

Runtime layer (scene, application lifecycle) and tools layer (debug overlays, profiler, editor). Documented together because the tools consume runtime interfaces and renderer diagnostics. Boundaries only; detail is deferred to M5/M6.

## 2. Scope

| Crate | Layer | In | Out |
|---|---|---|---|
| scene | 4 (M6a) | scene representation, transform hierarchy, camera/mesh/light components, serialization | rendering, GPU objects |
| runtime | 4 (M6a) | engine lifecycle, wires platform + backend + renderer + assets + scene, application-facing API | editor UI |
| debug | 5 (M6b) | overlays, egui integration, the profiler panels (no separate `profiler` crate, ADR-0014 §1), render graph and resource visualisation, GPU diagnostics display, debug geometry | collecting raw data (done by hooks) |
| editor | 5 (M6b) | editor app, viewport, hierarchy, inspector, asset browser, material editor (egui) | engine logic that the game also needs |

## 3. Crates

All introduced at M5-M6 under ADR-0002. Layer rules in the README apply; layer 5 may depend on layers 1-4 but nothing may depend on layer 5.

## 4. Responsibilities / 5. Non-responsibilities

- Scene: owns entity/component data; produces `ExtractedFrame` for the renderer (LAYER-005); never exposes Vulkan objects (rule 5).
- Runtime: the composition root. It is the place that names the concrete backend (or the application does) and injects it as a generic parameter.
- Tools: consume data through the runtime and the hooks in README §2.4 (rule 6); must not reach into the backend or renderer internals (rule 4). The game must run without any layer 5 crate.

## 6. Dependencies

Per README table. Additional: egui only in layer 5; profiler may use the `tracy` client crate; none of these may depend on `ash`.

## 7. Key concepts and data flow

```
 platform events -> runtime loop -> scene update -> extract -> renderer<D> -> RHI
                                     ^                                         |
        editor / debug --+-- read: GraphDescription, MemorySnapshot, GpuScopeSample
```

GPU timing/labels: emitted via rhi (README §2.4); profiler pulls `GpuScopeSample`s from the device through a runtime-provided accessor and displays/exports them.

## 8. Public interface (M6a, as built)

Decisions: [ADR-0014](../../decisions/ADR/0014-m6-runtime-and-tooling.md) §2, §3, §7. Plus the `player` binary: `anvil-player <project-dir>`.

```rust
// scene (GPU-free)
pub struct World;                                   // Deref to hecs::World; spawn_entity, set_parent (cycle-checked), despawn_recursive, find_uuid
pub struct Name(String); pub struct EntityUuid(u128); pub struct Transform { translation, rotation, scale }
pub struct GlobalTransform { matrix, prev, valid }; pub struct Parent(Entity); pub struct Children(Vec<Entity>)
pub struct MeshRenderer { mesh: AssetId, material: AssetId }; pub struct Light { kind: LightKind, .. }; pub struct Camera { fov_y, near, exposure }
pub fn update_transforms(&mut World);               // parents first
pub struct SceneFile { version, entities: Vec<EntityData> }   // from_world / to_ron / from_ron / spawn_into
pub fn instantiate_prefab(&mut World, &PrefabAsset, name, Transform) -> Entity

// runtime
pub trait Application { fn start(&mut self, ctx: &mut Context); fn update(&mut self, ctx: &mut Context, dt: f32); }
pub struct Context<'a> { world, assets, input, time, quit }
impl<A: Application> Runtime<A> { pub fn run(opts: RunOptions, app: A) -> Result<RunReport, RuntimeError>; }
pub struct RuntimeCore { pub renderer, pub world, pub assets }   // no window, no loop: the editor embeds it
impl RuntimeCore { update_assets(&device); run_systems(); extract(&device) -> RenderScene; open_scene(path); wait_for_assets(..); destroy(..) }
pub struct Project { version, name, startup_scene }               // project.anvil (RON)
```

- **Frame order** (`Runtime::frame`): input (collected by `on_event`), `assets.update` (via `RuntimeCore::update_assets`), `Application::update`, transform system, extract, render, present.
- **Bridge.** `RuntimeCore` keeps `AssetId -> MeshId / MaterialId` and `(AssetId, srgb) -> TextureId` maps. Resources are created lazily by `extract` when an asset is `Loaded`; while it is `Loading` the renderer's default material or fallback mesh is drawn, when `Failed` the magenta missing material, the fallback cube, and the entry is cached so nothing is retried. `Reloaded` replaces meshes and textures (old textures are destroyed after the materials were repointed) and updates materials in place with `Renderer::update_material`; `Failed` drops the GPU copies.
- **Renderer additions.** `Renderer::update_material`, `fallback_mesh()`, `white_texture()`, `missing_material()`, `default_material()`.
- **Headless.** `ANVIL_WINDOW=0` renders into an offscreen `Rgba8Srgb` target (1280x720), waits for assets, runs `ANVIL_EXIT_AFTER_FRAMES` (default 60) frames, writes `ANVIL_SCREENSHOT`, logs validation counts at exit and returns them in `RunReport`; the player exits with code 1 on validation errors. `ANVIL_VSYNC=0`, `ANVIL_HOT_RELOAD=1` as in the other binaries.
- **Project.** `project.anvil` is RON with `version`, `name`, `startup_scene`. `samples/sample-project` is generated by `cargo test -p runtime --test generate_sample -- --ignored` (no third-party content).

## 9-12. Ownership, threading, errors, diagnostics

- Ownership: the runtime owns device, renderer, scene, asset server; the editor owns UI state and borrows a `DiagnosticsView`.
- Threading: main thread runs the loop; workers via task; editor UI runs on the main thread.
- Errors: per-crate thiserror enums; runtime converts to a single application-level result at the binary boundary (`anyhow` allowed in binaries only).
- Diagnostics: this layer is the display side; it adds no instrumentation requirement on lower layers beyond README §2.4.

## 13. Testing strategy

Scene serialization round-trip; hierarchy and transform tests; headless runtime tests on a real device (`ANVIL_GPU_TESTS=1`, validation counts must be 0, screenshot not black); editor UI tests are manual at first; M6 completion criterion (create, save, reopen, run a sample project).

## 14. Acceptance criteria (Draft)

- [ ] `cargo tree` shows nothing depends on editor, debug or profiler except applications.
- [x] A sample project runs with the editor disabled (`anvil-player samples/sample-project`, M6a).
- [ ] The debug panels display GPU scopes without any dependency from lower layers on it (M6b; there is no separate profiler crate).

## 15. Open questions (answered by ADR-0014)

1. ECS: `hecs` (ADR-0014 §2). Extraction is `RuntimeCore::extract`, a plain function over the world producing `RenderScene` (ADR-0011).
2. Composition root: `Runtime` for games and `anvil-player`; the editor embeds `RuntimeCore` with its own loop and offscreen target (§7).
3. Editor in-process, single binary; crash isolation deferred to M7 (§9).
4. Scene format: RON with `version`, rejected when newer, migrated in code when older (§3). Shared with LAYER-009.
5. Profiler hook: pull-style (existing RHI timestamps, `RendererStats`); no push hook needed.
6. debug vs profiler: one `debug` crate; no `profiler` crate (§1).

Remaining, deferred: saved game-defined components (a serde registry), several cameras, undo/redo, play-mode world copy (M6b editor).

## 16. Related modules

None defined yet.
