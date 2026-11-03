# ADR-0014: M6 runtime and tooling: assets, scene, runtime, egui debug UI, editor

- Status: Accepted
- Date: 2026-11-03

## Context
M6 covers FR-AST-001..010/012/013, FR-SCN-001..006 and FR-TOOL-001..006, plus the M5 deferrals: the on-screen overlays of FR-TOOL-007 (egui) and disk streaming. The exit criterion (MASTER-004) is: **a complete sample project can be created, saved, reopened and executed.** LAYER-009 §15 and LAYER-010 §15 list the open questions this ADR answers.

State at M5 exit:
- The renderer (`crates/renderer`) owns GPU resources behind `MeshId`/`TextureId`/`MaterialId`, with create and destroy for each and no update.
- Scenes are rebuilt every frame as a flat `RenderScene` (ADR-0011).
- `platform` runs the winit loop through an `App` trait. Examples own the device, swapchain and renderer directly.
- There is no UI and no serialization.

## Problem
Choose the ECS, serialization, asset identity, the import, cache and load model, the runtime composition root, the UI stack and the editor architecture. The goal is the exit criterion with the least new machinery, while keeping the layer rules: no GPU types in assets or scene (rule 5), tools only through runtime interfaces (rule 6), and the game runs without any layer 5 crate.

## Decision
1. **New crates (ADR-0002 layout, no prefix, ADR-0008).**
   - Layer 4:
     - `assets`: database, importers, loaders, cache, hot reload. GPU-free.
     - `scene`: world, hierarchy, components, serialization. GPU-free.
     - `runtime`: composition root and application API. It names the Vulkan backend.
   - Layer 5:
     - `debug`: the egui integration (winit input plus an egui renderer written on our RHI) and the reusable diagnostics panels (stats, GPU pass timings, memory, render graph, debug views and draws).
     - `editor`: a library plus the `anvil-editor` binary.
   - Binary: `player` (`anvil-player <project>`), which runs a project with no layer 5 crate. Overlays are behind its optional `overlay` feature, which pulls in `debug`.
   - **No separate `profiler` crate.** M5 already collects timings (RHI timestamps, `tracing`, `profile-tracy`). Displaying them is a `debug` panel. LAYER-010 is amended.
2. **ECS: `hecs`.**
   - Small, no scheduler, no proc-macro world. Systems are plain functions over `&mut World` called by the runtime in a fixed order.
   - The hierarchy is a `Parent(Entity)` component plus a derived `Children` list that the scene keeps consistent.
   - World transforms are computed each frame by a transform system (parents first).
   - Built-in components: `Name`, `Transform` (local TRS), `GlobalTransform`, `MeshRenderer { mesh: AssetId, material: AssetId }`, `Light`, `Camera`, `Parent`/`Children`.
   - Games may add their own components. Only the built-in set (plus types registered in a serialization registry) is saved.
3. **Serialization: serde + RON** for every engine-defined text format:
   - project (`project.anvil`)
   - scene (`*.scene`)
   - material (`*.material`)
   - metadata (`*.meta`)

   Every file starts with `version: u32` (FR-AST-012). Loaders reject unknown future versions with `AssetError::Version`, and older versions are migrated in code. Entities are saved with stable `EntityUuid`s (a component holding a u128), so parent links survive save and reload.

   The imported-artifact cache is binary, a `bytemuck` header plus raw payload, not serde.
4. **Asset identity (FR-AST-002/005).**
   - `AssetId` is a random UUID (u128) written to a `<file>.meta` sidecar the first time the file is discovered. Ids therefore survive renames and moves that carry the `.meta` along.
   - The `.meta` also stores the importer name, its settings, and the hash of the source content at last import.
   - Sub-assets (glTF meshes, materials and images) get deterministic ids derived from the parent id and their index or name, and are listed in the `.meta`.
   - Projects commit `.meta` files.
5. **Import, cache and load (FR-AST-001/003/006/007/009).**
   - **Discovery:** scan `<project>/assets/**` at startup.
   - **Import:** on a worker, `source -> importer -> CPU asset`.
   - **Importers:**
     - `gltf` crate: glTF 2.0, `.gltf` and `.glb`, meshes, PBR materials, images, node hierarchy as a scene prefab.
     - `image` crate: PNG, JPEG and EXR.
     - `ktx2` crate: uncompressed formats only. shortcut: BCn/ASTC in KTX2 are rejected with `AssetError::Format` until the RHI gains compressed formats.
   - **Cache:** processed artifacts in `<project>/.anvil/imported/<id>.bin`, keyed by importer version plus source hash. A mismatch reimports. The cache directory is never committed and is safe to delete.
   - **Loading:** `AssetServer::load::<T>(path or id) -> AssetHandle<T>`. Work runs on rayon, and results return over a channel drained by `AssetServer::update()` on the main thread.
     - `LoadState`: Loading, Loaded, Failed(reason).
     - A `Failed` asset never panics (FR-AST-004).
     - The renderer substitutes a default mesh, a default texture or a magenta material.
   - **Dependencies:** a graph (material to textures, scene to meshes and materials) is recorded at import. A dependency that fails propagates `Failed(Dependency)` to its dependents.
   - **GPU upload (FR-AST-008).**
     - The renderer pulls; assets never push.
     - The runtime owns the `AssetId -> MeshId/TextureId/MaterialId` bridge. It creates GPU resources when an asset turns `Loaded` and replaces them when it reloads.
     - The renderer gains `update_material`. Mesh and texture replacement is destroy plus create with a remap.
6. **Hot reload (FR-AST-010).**
   - The `notify` crate watches `assets/`, with a 200 ms debounce, and the changed source is reimported.
   - Dependents are re-resolved, and a `Reloaded(id)` event is emitted for the runtime bridge.
   - On by default in the editor. In the player it is enabled with `ANVIL_HOT_RELOAD=1`.
7. **Runtime (FR-SCN-004/005).**
   - `Runtime` owns: the platform window, the Vulkan device and swapchain, the `Renderer`, the `World` and the `AssetServer`.
   - `trait Application { fn start(&mut self, ctx); fn update(&mut self, ctx, dt); }`, where the context exposes the world, assets, input and time.
   - The fixed frame order is:
     1. input
     2. `assets.update`
     3. `app.update`
     4. built-in systems (transforms, camera)
     5. extract into `RenderScene`
     6. render
     7. present
   - Headless mode (`ANVIL_WINDOW=0` style offscreen target plus `ANVIL_EXIT_AFTER_FRAMES` and `ANVIL_SCREENSHOT`) lets tests run a project without a window.
   - The editor drives the same `Runtime` pieces through a `RuntimeCore` that it embeds: the same systems and extract, with a different loop and target.
8. **UI: egui** (`egui` plus `egui-winit` for input). The renderer is ours (`debug::EguiRenderer`):
   - One pipeline.
   - Vertex and index data through the per-frame upload ring.
   - Font and user textures as bindless sampled images.
   - Drawn into the swapchain image after the renderer, as its own graph pass or a separate encoder section.

   `egui-ash` and `egui-wgpu` are rejected because they bypass our RHI (rule 4). The editor viewport is an offscreen `Renderer` target shown as an egui image through its bindless index.
9. **Editor (FR-TOOL-001..006).**
   - In-process with the engine, single binary. Crash isolation is deferred to M7.
   - **Panels:**
     - viewport (fly camera; entity picking by a CPU ray against AABBs; translate gizmo)
     - hierarchy (create, delete, rename, reparent)
     - inspector (built-in components, edit transforms, assign mesh and material assets)
     - asset browser (folder tree, import status, create material or scene, drag to the viewport)
     - material editor (factors, texture slots, live preview through `update_material`, save to `.material`)
     - debug panels from `debug`
   - **Project lifecycle:**
     - New project (template with `assets/`, `project.anvil` and a default scene)
     - Open
     - Save scene
     - Play / Stop: play runs a copy of the world; stop restores it.
   - **All edits go through an `EditorCommand` enum.** This enables undo later and scripted runs now: `ANVIL_EDITOR_SCRIPT=file.ron` executes a command list (for example: new project, add entity, assign mesh, save, quit). That makes the exit criterion verifiable headlessly by tests and by AI agents.
   - Undo/redo itself is deferred: the command enum is the hook.
10. **Sample project (FR-SCN-006).**
    - `samples/sample-project/` is committed with a project file, a scene, a textured glTF model, a PNG and a material.
    - All content is generated by us; no third-party assets.
    - Exit test (`crates/editor/tests/project_lifecycle.rs`, GPU-gated):
      1. The editor script creates a project in a temp dir from the template, imports a glTF, places it, and saves.
      2. A second process reopens and verifies the entity set.
      3. `anvil-player` runs it headless for N frames with 0 validation errors and a non-empty screenshot.

## Rationale
- hecs plus RON is the smallest stack covering ECS, readable diffs and versioned files.
- Bevy-style schedulers, reflection and scene formats are more than M6 needs.
- UUID sidecars are the standard way to keep ids stable across moves (Godot, Unity).
- Pull-style upload keeps assets GPU-free.
- Our own egui renderer is about 300 lines and keeps the RHI the only GPU path.
- A command enum plus a script runner turns "manual" editor criteria into automated checks, in line with ADR-0009's AI observability.

## Alternatives
- **Own ECS:** more code, no benefit at this scale.
- **bevy_ecs:** heavy, and pulls in a scheduler and reflection.
- **Path-hash ids:** break on rename.
- **Content-hash ids:** break on edit.
- **bincode or MessagePack for scenes:** not diffable.
- **egui-ash:** bypasses the RHI.
- **A separate editor process:** IPC cost now; reconsider in M7.
- **Push-style asset upload:** puts GPU knowledge in assets.

## Consequences
- **New dependencies:** hecs, serde, ron, uuid, gltf, image (png/jpeg/exr features only), ktx2, notify, egui and egui-winit.
- The renderer gains `update_material` and default fallback resources.
- `examples/*` stay as they are (direct renderer use). New users go through `runtime`.
- LAYER-009 and LAYER-010 answer their open questions by referencing this ADR. MODULE specs are written if a component grows past one file.

## Phasing
- **M6a:** assets, scene, runtime, player, sample project. Headless create, save, reopen and run through code; corrupt asset handling; hot reload test.
- **M6b:** debug (egui on RHI, overlay in player and examples), then editor (all panels, commands and script runner), then the project-lifecycle exit test, docs and the M6 completion status.
