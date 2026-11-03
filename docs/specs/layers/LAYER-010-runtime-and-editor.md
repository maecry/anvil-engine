# LAYER-010: Runtime and Tools

| Field | Value |
|---|---|
| Status | Draft |
| Milestone | M5 (profiler); M6 (scene, runtime, editor, debug) |
| Crates | scene, runtime, debug, profiler, editor |
| Traces to | MASTER-000 §5.1 (Layers 4-5), §5.2 rules 1, 4, 5, 6, §8.9-8.12, §14 M5-M6 |
| Last updated | 2026-11-03 |

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

## 6. Debug crate (M6b-1, ADR-0014 §8)

`crates/debug` (layer 5) depends on foundation, platform, rhi, renderer and render-graph, never on ash or a backend; it is generic over `rhi::Device`. Everything is plain egui (`egui` 0.33 and `egui-winit`).

- **`EguiInput`**: owns the `egui::Context` and the egui-winit state. Events arrive through the new `platform::App::on_raw_event` (every raw winit window event, before translation; `Window::winit()` exposes the window). `wants_pointer()` and `wants_keyboard()` tell an app to skip camera and key input.
- **`EguiRenderer`**: one graphics pipeline (`shaders/egui.vert|frag`, compiled by `build.rs`). Each frame slot owns one host-visible buffer holding the vertices (pulled through the buffer device address, like the renderer's debug lines) followed by the `u32` indices; it grows by doubling. One scissored `draw_indexed` per mesh, premultiplied-alpha blend (`One`, `OneMinusSrcAlpha`), `LoadOp::Load` into a target that is in `ColorAttachment` state and stays there. egui's gamma-space colors are multiplied in the fragment shader; for an sRGB target the result is converted to linear first, so the hardware encoding returns egui's intended bytes (tested). Font and image textures are `Rgba8Unorm` bindless textures; a `TexturesDelta` set or patch makes a fresh texture from a CPU copy (the old one is destroyed deferred), free destroys. `register_texture` turns an existing sampled rhi texture (for example the editor viewport's offscreen target, in `ShaderRead`, gamma-encoded `*Unorm`) into an `egui::TextureId`.
- **Panels** (`debug::panels`): functions over `&mut egui::Ui` and plain data: `frame_stats` (FPS, frame, renderer CPU and GPU ms, history graph), `gpu_timings`, `memory` (`DeviceDiagnostics`, texture streaming), `render_graph` (`GraphStats` and the pass order with GPU times), `renderer_settings` (render path, SSAO/TAA/bloom, debug view, debug draw, GPU culling, LOD pixels, through the renderer's setters).
- **`Overlay`**: bundles the above behind F1. Per frame: `run(device, window, renderer)` (builds the UI from last frame's stats, uploads textures), render the scene with `scene_final_state(final)` as its end state, then `render(device, cmd, frame, image, final)` which draws over the swapchain image and moves it to `final` (so screenshots include the overlay). Hidden, `render` does nothing and `scene_final_state` is the plain final state.
- **`ANVIL_OVERLAY=1`**: `Overlay::enabled_by_env()`. pbr-scene and many-draws create an `Overlay` only then; unset, no debug code runs. The apps copy the renderer's settings back after `run`, so panel edits persist.
- **Shortcuts**: one linear/clamp sampler for all textures; paint callbacks are skipped; no clipboard or IME output beyond what egui-winit does without its default features; the graph panel lists passes from `GpuTimings` (an earlier frame) rather than a `GraphDescription` (not exposed by the renderer).
- **Test**: `crates/debug/tests/gpu_egui.rs` (`ANVIL_GPU_TESTS=1`).

## 7b. Editor crate (M6b-2, ADR-0014 §9)

`crates/editor` (layer 5): the library `editor` and the binary `anvil-editor [project-dir]`. It depends on runtime, scene, assets, renderer, debug, platform and (for device creation, like the runtime) rhi and rhi-vulkan; nothing depends on it (FR-TOOL-006: only public runtime/scene/assets/renderer/debug APIs, no renderer internals).

- **Architecture.** `Editor` embeds a `runtime::RuntimeCore` (the player's world, asset bridge, systems and extract) with its own loop (`app.rs`). Per frame: acquire, `assets.update`, one script command, the egui pass (the panels read the editor and queue commands), `apply_pending`, transform systems + extract, then record: the `Renderer` renders the scene into an offscreen `Rgba8Unorm` **viewport texture** (sized to the viewport panel, recreated when it changes), a clear of the window image, egui over it (the viewport is a user texture from `EguiRenderer::register_texture`), optional readback. Windowed it presents; headless it renders to an offscreen `Rgba8Srgb` 1600x900 image and egui runs with a plain `egui::Context` (no window). `Editor::new` starts without a project; `OpenProject` builds a fresh `RuntimeCore` (old GPU copies go with the old one).
- **Panels.** Viewport (editor fly camera that is not an entity: RMB look, WASD/Space/Q while RMB is held, Shift fast, wheel dollies; click picks the nearest `MeshRenderer` world AABB, mesh bounds from the asset's vertices, the unit cube for NIL or unloaded meshes; selection AABB and a 3-axis translate gizmo are renderer debug lines; dragging a handle moves the entity along that axis in the parent's space). Hierarchy (tree by `Parent`/`Children`, select, context menu: add child, make the selected entity a child of this one, unparent, delete; Delete key; create empty). Inspector (name, position/rotation(euler degrees)/scale, `MeshRenderer` mesh and material pickers listing assets of the right kind, `Light`, `Camera`, add/remove components). Asset browser (folder tree of `assets/`, kind and `LoadState` per file with the failure reason, create material, create scene, instantiate model, edit material, open scene; it requests every source asset so states are visible). Material editor (factors, alpha mode, five texture slots, live preview, Save, Revert). Debug column: the `debug::panels` (frame, renderer settings, GPU passes, render graph, memory). Menu bar: File (New/Open project dialogs take a directory path, Save Scene Ctrl+S, Quit), Scene (create empty), Play/Stop.
- **Material preview and hot reload.** `RuntimeCore::preview_material` updates the GPU material in place from the editor's draft (`Renderer::update_material`). After every extract the editor re-applies the draft while it differs from the saved file or from the loaded asset, so neither the 200 ms hot reload of the file written by Save nor an old reload landing after newer edits ever reaches the screen.
- **Play/Stop.** Play stores `SceneFile::from_world` as RON text in memory and switches the viewport to the scene's camera (no gizmo, no editing interaction); Stop rebuilds the world from the text (uuids survive, so the selection does). Saving while playing is refused.
- **Project lifecycle.** `NewProject` writes `project.anvil`, `assets/{scenes,models,materials,textures}`, `.gitignore` (`.anvil/`) and `assets/scenes/main.scene` with a camera, a shadow-casting sun and a ground slab (the renderer's built-in cube, flattened; no third-party content). `OpenProject` loads the manifest, starts the asset server with hot reload and reads the startup scene directly from disk.
- **`EditorCommand`** (`serde`, RON; `command.rs`) is the only way state changes: `NewProject{path}`, `OpenProject{path}`, `ImportFile{src,dest}`, `Select{entity}`, `CreateEntity{name,parent}`, `DeleteEntity`, `Rename`, `SetTransform{entity,translation,rotation_deg,scale}`, `SetParent` (keeps the world transform), `AddComponent`/`RemoveComponent{entity,component}`, `SetLight`, `SetCamera`, `AssignMesh`/`AssignMaterial{entity, Option<AssetRef>}`, `InstantiateModel{model,name,translation}`, `CreateMaterial{path}`, `OpenMaterial{path}`, `EditMaterial{path,material}`, `SaveMaterial{path}`, `CreateScene{path}`, `OpenScene{path}`, `SaveScene`, `Play`, `Stop`, `Screenshot{path}`, `WaitFrames(n)` (script only), `AssertEntity{name,has_mesh,parent,material,translation}`, `AssertEntityCount{at_least}`, `Quit`. Entities are addressed by `Name("..")` (first match in spawn order) or `Uuid(..)`; assets by `Path("assets/..")` or `Id(..)`. `Editor::apply(device, cmd) -> Result<Outcome, EditorError>` applies one command; `Outcome::Wait` means "an asset is still loading, apply again next frame" (`InstantiateModel`, `Screenshot`, which waits until imports were idle for 3 frames so the image shows the finished scene).
- **Scripts.** `ANVIL_EDITOR_SCRIPT=file.ron` is a RON list of commands, one applied per frame (a waiting command blocks the script for at most 60 s). A failing command logs an error, stops the run and the process exits 1; so do validation errors. Headless, the editor quits when the script ends. Each applied command is logged (`script command applied`). Example:

```ron
[
    NewProject(path: "/tmp/demo"),
    ImportFile(src: "samples/sample-project/assets/models/crate.glb", dest: "models/crate.glb"),
    InstantiateModel(model: Path("assets/models/crate.glb"), name: "crate-instance", translation: (0.0, 0.5, 0.0)),
    CreateMaterial(path: "assets/materials/red.material"),
    EditMaterial(path: "assets/materials/red.material", material: (base_color: (0.9, 0.1, 0.1, 1.0), roughness: 0.35)),
    SaveMaterial(path: "assets/materials/red.material"),
    AssignMaterial(entity: Name("crate"), material: Some(Path("assets/materials/red.material"))),
    SetTransform(entity: Name("crate-instance"), translation: (1.5, 0.0, 0.0), rotation_deg: (0.0, 30.0, 0.0), scale: (1.0, 1.0, 1.0)),
    SaveScene,
    Screenshot(path: "/tmp/demo.png"),
    Quit,
]
```

- **Env.** `ANVIL_WINDOW=0` headless (1600x900) or `WxH`, `ANVIL_EDITOR_SCRIPT`, `ANVIL_EXIT_AFTER_FRAMES` (headless without a script defaults to 60), `ANVIL_VSYNC=0`, `ANVIL_SYNC_VALIDATION=1`. The `anvil-editor` exit code is 1 on a failed command or validation errors.
- **Shortcuts.** No undo/redo (`EditorCommand` is the hook: record inverses where commands are applied). Picking recomputes mesh bounds from the vertices each call and tests only `MeshRenderer` entities. The inspector derives euler angles from the quaternion every frame (a drag near gimbal lock jumps). No drag-and-drop in the hierarchy or from the asset browser to the viewport (menu actions and buttons instead). Project paths are typed, there is no file dialog. The gizmo is depth-tested debug lines (an occluding mesh hides it) and translate-only. The material preview runs every frame while a draft is unsaved. Selection is resolved by uuid with a linear search. One viewport, no multi-select, no scene tabs, no saved layout. Crash isolation stays deferred to M7.
- **Tests.** Unit tests (command script parse, picking and gizmo math, template); `crates/editor/tests/project_lifecycle.rs` (`ANVIL_GPU_TESTS=1`, the M6 exit test): a headless editor script creates a project, imports and instantiates a glTF, creates, edits, saves and assigns a material, moves it, plays, stops, saves and screenshots; a second editor process reopens the project and asserts the entity set; `anvil-player` runs it headless. All three processes run with sync validation: 0 errors, 0 warnings, and the screenshots show the red crate.

## 13. Testing strategy

Scene serialization round-trip; hierarchy and transform tests; headless runtime tests on a real device (`ANVIL_GPU_TESTS=1`, validation counts must be 0, screenshot not black); the editor is tested through its command script (the exit test), picking math in unit tests; interactive input (drag, camera, menus) is checked manually; M6 completion criterion (create, save, reopen, run a sample project).

## 14. Acceptance criteria (Draft)

- [x] `cargo tree` shows nothing depends on editor, debug or profiler except applications (`cargo tree -i editor` is the editor alone; `-i debug` lists the editor and the two examples).
- [x] A sample project runs with the editor disabled (`anvil-player samples/sample-project`, M6a).
- [x] The debug panels display GPU scopes without any dependency from lower layers on it (`debug::panels`, shown by the overlay and the editor; there is no separate profiler crate).
- [x] A sample project can be created, saved, reopened and executed (`crates/editor/tests/project_lifecycle.rs`, M6 exit test).
- [x] Every editor edit goes through `EditorCommand`; the editor uses only runtime, scene, assets, renderer and debug APIs (FR-TOOL-006).

## 15. Open questions (answered by ADR-0014)

1. ECS: `hecs` (ADR-0014 §2). Extraction is `RuntimeCore::extract`, a plain function over the world producing `RenderScene` (ADR-0011).
2. Composition root: `Runtime` for games and `anvil-player`; the editor embeds `RuntimeCore` with its own loop and offscreen target (§7).
3. Editor in-process, single binary; crash isolation deferred to M7 (§9).
4. Scene format: RON with `version`, rejected when newer, migrated in code when older (§3). Shared with LAYER-009.
5. Profiler hook: pull-style (existing RHI timestamps, `RendererStats`); no push hook needed.
6. debug vs profiler: one `debug` crate; no `profiler` crate (§1).

Remaining, deferred: saved game-defined components (a serde registry), several cameras, undo/redo. (The play-mode world copy is in the editor, section 7.)

## 16. Related modules

None defined yet.
