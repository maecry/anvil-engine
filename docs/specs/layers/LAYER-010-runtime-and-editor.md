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
| scene | 4 | scene representation, transform hierarchy, camera/mesh/light components, serialization | rendering, GPU objects |
| runtime | 4 | engine lifecycle, wires platform + backend + renderer + assets + scene, application-facing API | editor UI |
| debug | 5 | overlays, render graph and resource visualisation, GPU diagnostics display, debug geometry | collecting raw data (done by hooks) |
| profiler | 5 | CPU profiling integration (Tracy via `tracing` layer), GPU timestamp display/export, frame timing stats | emitting timestamps (RHI hooks) |
| editor | 5 | editor app, viewport, hierarchy, inspector, asset browser, material editor (egui) | engine logic that the game also needs |

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

## 8. Public interface sketch (concept)

```rust
pub trait Application { fn update(&mut self, ctx: &mut UpdateContext); fn render_config(&self) -> RendererConfig; }
pub struct Runtime<D: Device> { /* platform loop, device, renderer, scene, assets */ }
pub struct DiagnosticsView<'a> { pub gpu_scopes: &'a [GpuScopeSample], pub memory: MemorySnapshot, pub graph: Option<&'a GraphDescription> }
```

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

## 13. Testing strategy

Scene serialization round-trip; extraction determinism; headless runtime smoke test with `NullBackend`; editor UI tests are manual at first; M6 completion criterion (create, save, reopen, run a sample project).

## 14. Acceptance criteria (Draft)

- [ ] `cargo tree` shows nothing depends on editor, debug or profiler except applications.
- [ ] A sample project runs with the editor disabled.
- [ ] Profiler displays GPU scopes without any dependency from lower layers on it.

## 15. Open questions

1. ECS: custom or existing crate; shapes scene and extraction.
2. Where the runtime composition root lives for user games vs the editor.
3. Editor in-process with the game vs a separate process (hot reload, crash isolation).
4. Scene file format and versioning (shared with LAYER-009 Q2).
5. Whether the profiler needs a push-style hook later (hot-path cost of pull).
6. debug vs profiler split: both may need egui; confirm the boundary at M5.

## 16. Related modules

None defined yet.
