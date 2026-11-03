# Anvil Engine user guide (1.0.0-rc.1)

Anvil is a 3D engine in Rust on Vulkan 1.3 (Linux x86_64). It ships a player (`anvil-player`) that
runs a project, and an editor (`anvil-editor`) that creates and edits one. Known gaps are in
[LIMITATIONS.md](LIMITATIONS.md); what is stable across versions is in
`docs/specs/master/API-STABILITY.md` in the source tree.

## Requirements

- Linux x86_64 with a Vulkan 1.3 driver (Mesa RADV/ANV, NVIDIA, or lavapipe for software rendering).
- Optional: the Khronos validation layer (`vulkan-validationlayers`) for validation runs.
- Building from source additionally needs the Rust toolchain pinned in `rust-toolchain.toml`
  and `libshaderc` (for example `libshaderc-dev`).

## Run a release archive

```sh
tar -xzf anvil-<version>-linux-x86_64.tar.gz && cd anvil-<version>-linux-x86_64
sha256sum -c SHA256SUMS            # optional integrity check
bin/anvil-player samples/sample-project
bin/anvil-editor samples/sample-project
```

`VERSION` records the version, git commit, rustc version and build date.

## Build from source

```sh
cargo build --release --locked -p player -p editor   # target/release/anvil-player, anvil-editor
cargo test --workspace                               # CPU tests
ANVIL_GPU_TESTS=1 cargo test --workspace             # plus GPU tests (needs a Vulkan device)
scripts/package.sh                                   # dist/anvil-<version>-linux-x86_64.tar.gz
scripts/package.sh --verify                          # builds twice and checks the archives match
```

Examples (`triangle`, `textured-quad`, `pbr-scene`, `many-draws`) run with `cargo run -p <name>`.

## The player

```sh
anvil-player <project-dir>
```

Opens a window and runs the project's startup scene as authored (there is no game logic in the
player). The run ends when the window closes. Exit codes: 0 normal, 1 failure or validation
errors reported, 3 device lost.

Headless run with a screenshot:

```sh
ANVIL_WINDOW=0 ANVIL_EXIT_AFTER_FRAMES=60 ANVIL_SCREENSHOT=out.png anvil-player samples/sample-project
```

## The editor

```sh
anvil-editor [project-dir]
```

- **Viewport:** hold the right mouse button and use WASD (Space/Q for up/down, Shift for fast) to
  fly; the wheel dollies; click selects; dragging a gizmo axis moves the selected entity.
- **Hierarchy, inspector:** create, rename, delete and re-parent entities; edit transform, light,
  camera, mesh and material assignment.
- **Asset browser:** lists the project's assets and their load state; import files from disk.
- **Material editor:** edit a material and see it live in the viewport; Save writes the file.
- **Debug panels:** frame statistics, GPU pass timings, memory and device counters, the render graph,
  renderer settings.
- **File > New Project** creates a project from a template (camera, sun, ground).
- Play/Stop runs the scene in the viewport and restores it afterwards. Saving while playing is refused.
- There is no undo/redo.

### Editor scripts

`ANVIL_EDITOR_SCRIPT=file.ron` runs a RON list of `EditorCommand`s, one per frame. A failing
command stops the run with exit code 1. Headless (`ANVIL_WINDOW=0`) the editor quits when the
script ends.

```ron
[
    NewProject(path: "/tmp/demo"),
    ImportFile(src: "samples/sample-project/assets/models/crate.glb", dest: "models/crate.glb"),
    InstantiateModel(model: Path("assets/models/crate.glb"), name: "crate-instance", translation: (0.0, 0.5, 0.0)),
    CreateMaterial(path: "assets/materials/red.material"),
    EditMaterial(path: "assets/materials/red.material", material: (base_color: (0.9, 0.1, 0.1, 1.0), roughness: 0.35)),
    SaveMaterial(path: "assets/materials/red.material"),
    SaveScene,
    Screenshot(path: "/tmp/demo.png"),
    Quit,
]
```

Commands: `NewProject`, `OpenProject`, `ImportFile`, `Select`, `CreateEntity`, `DeleteEntity`, `Rename`,
`SetTransform`, `SetParent`, `AddComponent`, `RemoveComponent`, `SetLight`, `SetCamera`, `AssignMesh`,
`AssignMaterial`, `InstantiateModel`, `CreateMaterial`, `OpenMaterial`, `EditMaterial`, `SaveMaterial`,
`CreateScene`, `OpenScene`, `SaveScene`, `Play`, `Stop`, `Screenshot`, `WaitFrames`, `AssertEntity`,
`AssertEntityCount`, `Quit`. The definitions are in `crates/editor/src/command.rs`.

## Project layout and formats

```
my-project/
  project.anvil              manifest (RON): version, name, startup_scene
  assets/
    scenes/main.scene        scene (RON)
    models/*.glb, *.gltf     glTF 2.0 models
    materials/*.material     material (RON)
    textures/*.png, *.jpg, *.exr, *.ktx2
    <file>.meta              id sidecar, created next to every asset; commit it
  .anvil/imported/           import cache; generated, do not commit
```

Every format carries a `version` and is rejected with an error if it is newer than the build
reads (project 1, scene 1, material 1, meta 1). A corrupt or unsupported file fails to load with
an error in the log and a `Failed` state; it never crashes the engine.

- **`project.anvil`:** `(version: 1, name: "Sample Project", startup_scene: "assets/scenes/main.scene")`.
- **Scene:** a list of entities, each with a uuid, optional name and parent, and optional
  `transform`, `mesh_renderer` (mesh and material asset ids), `light` (directional, point, spot)
  and `camera`. The first camera in the scene renders.
- **Material:** PBR metallic-roughness: `base_color`, `metallic`, `roughness`, `emissive`, normal,
  occlusion and emissive textures, `alpha_mode` (`Opaque`, `Mask(cutoff)`, `Blend`), `double_sided`.
- **`.meta`:** the asset's stable id, importer name and the ids of its sub-assets (glTF meshes,
  materials, images). Moving or renaming a file together with its `.meta` keeps references valid.
- **Imported formats:** glTF/GLB (meshes, materials, images, node hierarchy become a prefab),
  PNG/JPEG/EXR (as 8-bit RGBA), uncompressed RGBA8 KTX2.
- **Hot reload:** with `ANVIL_HOT_RELOAD=1` (player; always on in the editor) changed asset files
  are re-imported and dependents refresh while running.

## Environment variables

Booleans are `1` to enable unless noted. Unset variables use the default.

| Variable | Applies to | Meaning |
|---|---|---|
| `ANVIL_LOG` | all | Log filter directives (default: engine crates at `debug` in debug builds, `info` in release). |
| `ANVIL_LOG_FORMAT` | all | `pretty` (default) or `json` (one event per line). |
| `ANVIL_LOG_FILE` | all | Write the log to this file instead of stderr. |
| `ANVIL_THREADS` | all | Worker thread count (default: one per logical CPU). |
| `ANVIL_WINDOW` | player, editor, examples | `0` = headless (offscreen); `WxH` = window size. |
| `ANVIL_VSYNC` | player, editor, examples | `0` turns vsync off. |
| `ANVIL_EXIT_AFTER_FRAMES` | player, editor, examples | Exit after n frames. |
| `ANVIL_SCREENSHOT` | player, examples | PNG path written from the last frame; needs `ANVIL_EXIT_AFTER_FRAMES`. |
| `ANVIL_HOT_RELOAD` | player | `1` re-imports changed assets while running. |
| `ANVIL_EDITOR_SCRIPT` | editor | Path of an `EditorCommand` script (RON). |
| `ANVIL_GPU` | all | Substring of the preferred GPU's name. |
| `ANVIL_PIPELINE_CACHE` | all | Pipeline cache file. Default `$XDG_CACHE_HOME/anvil/pipeline-cache.bin`, else `~/.cache/anvil/pipeline-cache.bin`; the directory is created. |
| `ANVIL_GPU_CULLING` | renderer | `0` selects the CPU culling path. |
| `ANVIL_SYNC_VALIDATION` | all | `1` enables synchronization validation (needs the validation layer). |
| `ANVIL_GPU_AV` | all | `1` enables GPU-assisted validation (replaces synchronization validation). |
| `ANVIL_VALIDATION_PANIC` | all | `1` panics on the next call after a validation error. |
| `ANVIL_GRAPH_DUMP` | all | Writes the first compiled render graph to this path (`.json`, otherwise Graphviz DOT). |
| `ANVIL_SIMULATE_DEVICE_LOST` | player, editor | `<frame>`: the Vulkan backend reports device loss at that frame (fault injection; no real loss). The process exits with code 3. |
| `ANVIL_GPU_TESTS` | tests | `1` runs the GPU tests (they skip otherwise). |
| `ANVIL_BLESS` | tests | `1` rewrites the golden reference images (`crates/renderer/tests/golden/`). |
| `ANVIL_STRESS` | tests | `1` multiplies the stress-test counts by 10. |

Examples (`pbr-scene`, `many-draws`) read more: `ANVIL_RENDER_PATH=forward|deferred`,
`ANVIL_SSAO|TAA|BLOOM=0|1`, `ANVIL_OVERLAY=1` (diagnostics overlay, F1 toggles),
`ANVIL_DEBUG_VIEW=clusters|cascades|ssao|lod|occlusion|mips`, `ANVIL_DEBUG_DRAW=bounds|cascades|lights`,
`ANVIL_POINT_SHADOWS`, `ANVIL_CAMERA`, `ANVIL_CAMERA_ORBIT`, `ANVIL_TOGGLE_POST`, `ANVIL_STRESS_LIGHTS`,
`ANVIL_INSTANCES`, `ANVIL_LOD`, `ANVIL_TEXTURE_BUDGET_MB`, `ANVIL_TEXTURE_UPLOAD_MB`, and
`ANVIL_BENCH=path.json` with `ANVIL_BENCH_WARMUP` and `ANVIL_BENCH_FRAMES` for the benchmark
protocol (`docs/benchmarks/`).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Normal exit. |
| 1 | Failure: the run returned an error, validation reported errors, or an editor script command failed. |
| 3 | The GPU device was lost. |

## Validation and debug views

- Validation layers are on in debug builds when the layer is installed and off in release builds.
  `ANVIL_SYNC_VALIDATION=1` adds synchronization validation; a run with validation errors exits 1.
  The shutdown log reports validation counts and live GPU object counts.
- In the editor, the debug panels show frame time history, per-pass GPU times, memory use, device
  object counts and the render graph.
- In the examples, `ANVIL_DEBUG_VIEW` and `ANVIL_DEBUG_DRAW` show clusters, cascades, SSAO, LOD,
  occlusion, streamed mips, bounds and light volumes.

## Troubleshooting

- **"no suitable Vulkan device" or a missing-feature message:** the driver lacks Vulkan 1.3 or a
  required feature. `vulkaninfo --summary` lists devices; `ANVIL_GPU=<name>` picks one.
- **No window on a server:** use `ANVIL_WINDOW=0` for offscreen rendering.
- **A texture or model does not show:** check the log (`ANVIL_LOG=info`) for `asset load failed`
  with the reason; a missing `.meta` is created on start, a damaged one skips the asset.
- **Stale imports:** delete `<project>/.anvil/` to force a reimport.
- **Slow first start:** the pipeline cache is empty on the first run.
- **Exit code 3:** the GPU was lost (driver reset, hang, unplug). Restart; device recreation is not implemented.
