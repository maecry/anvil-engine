# Known limitations (1.0.0-rc.1)

This is the single list of known gaps. Every `shortcut:` comment in the code appears here; when
you add one, add it here too (`grep -rn "shortcut:" crates examples`).

## Platform

- **Linux x86_64 only.** No Windows, macOS, Android or console support. Wayland and X11 through winit.
- **Vulkan 1.3 only.** There is no other graphics backend. The device must support the features
  the RHI enables (dynamic rendering, synchronization2, descriptor indexing, indirect count,
  storage images, demote-to-helper-invocation); a device without them is rejected at startup with a message.
- **Building needs `libshaderc`** (system package, for example `libshaderc-dev`). Shaders are
  compiled at build time and embedded in the binaries; a released build needs no shader compiler.
- **Single graphics queue**, one window, one swapchain. No async compute, no multi-GPU.
- **No device recreation.** A lost device (`VK_ERROR_DEVICE_LOST`) ends the run: the runtime logs
  an `error` event with the diagnostics snapshot, and the player and editor exit with code 3.
  Recreating every GPU resource is not implemented.
- **Lavapipe test flakiness.** Two GPU tests, `push_constants_reach_shader` and
  `pipelines_share_one_layout`, read back all zeros intermittently on lavapipe (software Vulkan);
  suspected cause was `wait_idle` returning before the work was visible, so it now also waits on
  the timeline. The fix is unconfirmed on lavapipe; if they still flake there they get ignored on
  lavapipe only.
- **No license has been chosen.** The repository has no `LICENSE` file, so there is no permission
  to redistribute or reuse it yet. See [release-checklist](../development/release-checklist.md).
- **No release channels.** The only artifact is a tagged release candidate tarball
  (NFR-CI-012 is deferred). No distro package, Flatpak or AppImage.

## Features not implemented

- **No undo/redo in the editor.** Commands are applied directly (`EditorCommand` is the hook for it).
- **No game logic API.** The player runs the scene as authored; `Application` is the embedding hook.
- **No saved game-defined components** (no serde component registry).
- **One viewport, no multi-select, no drag-and-drop, no file dialog** in the editor.
- **No compressed textures.** KTX2 is read only as uncompressed RGBA8 level 0 (BCn, ASTC, Basis and
  supercompression are rejected).
- **No disk streaming of imported assets.** Texture mips are streamed from CPU memory only.
- **No transient memory aliasing** in the render graph; **no parallel command recording**.
- **No fuzzing infrastructure** (cargo-fuzz needs nightly). The seeded mutation tests in
  `crates/assets/tests/corruption.rs` and `crates/runtime/tests/corruption.rs` cover the same readers.

## Limits and shortcuts in the code

### Assets

- **EXR/HDR is clamped to 8-bit LDR RGBA** until the renderer takes float textures.
- **Only uncompressed RGBA8 level 0 of a KTX2 file is read.**
- **Image decode cap of 128 MiB** (4096x4096 RGBA8); larger textures fail to load. Width and height are capped at 16384.
- **The import cache key covers the `.gltf`/`.glb` file only.** Edits to external `.bin` or `.png` files
  are not noticed until the main file changes.
- **Source hash is FNV-1a 64-bit**, not cryptographic.
- **No per-asset importer settings** in `.meta` files, and no source hash there (it lives in the cache header).
- **A damaged `.meta` skips its asset** instead of being regenerated (a new id would break references); there is no repair tool.
- **A glTF with a dangling POSITION accessor** is rejected through a contained parser panic in
  `gltf-json` (the panic message is still printed to stderr); the load fails cleanly.

### Scene and runtime

- **The first camera in the world renders**; there is no active-camera flag, so no several cameras.
- **No registry for game-defined components** in scene files.

### Renderer

- **Fixed capacities:** mesh arena of 256 Ki vertices and 1 Mi indices (12 MiB and 4 MiB);
  mesh table of 4096 meshes; upload ring of 16 MiB (a frame whose uploads overflow it fails;
  changed instances above 70 000 per frame go through `Device::upload_buffer`); debug lines
  capped at 131 072 vertices (2 MiB). `create_mesh` fails when the arena or table is full.
- **Instance identity is its scene index.** Removing an instance from the middle re-uploads everything after it.
- **Texture indices are resolved at material creation.** Destroying a texture before its materials leaves stale bindless indices.
- **Texture streaming estimate ignores UV tiling and scale** (a texture repeated 8 times is under-resolved by 3 levels).
- **Mip chains use a box filter:** an odd dimension drops half a texel of weight at the last row or column, and normal maps are not renormalized.
- **The BRDF LUT is baked on the first render**, not in `Renderer::new`.
- **One fixed parallel threshold** (`PAR_MIN` = 2048 instances) measured on one machine.

### RHI and render graph

- **Array (non-cube) texture views get no bindless slot.**
- **Swapchain recreate and destroy wait for the device to be idle** (no retire-old-swapchain).
- **`TRANSFER_SRC` (screenshots) is assumed supported by the surface;** it is not checked.
- **Only the first 126 render-graph passes are GPU-timed** (the timestamp pool size).
- **The transient texture pool uses a linear scan and no memory aliasing,** fine for tens of transients.

### Editor and debug UI

- **Euler angles are recomputed from the quaternion every frame,** so a drag near gimbal lock jumps.
- **Mesh bounds are recomputed on every pick.**
- **Material previews update every frame while they differ** from the saved material.
- **One sampler for every egui texture;** egui's per-texture nearest option is ignored.
