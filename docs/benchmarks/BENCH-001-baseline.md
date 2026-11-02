# BENCH-001: Benchmark baseline

- Status: Accepted (M5b, 2026-11-02)
- Implements: MASTER-004 §5, ADR-0013 §1; supports NFR-PERF-001..015
- Results: `docs/benchmarks/results/2026-11-02-<workload>-<tag>.json` (`baseline`, `m5a`, `m5b`; plus `many-draws-cpu-path-m5b`)

MASTER-000 §4.3 and §22 p3 forbid performance targets until this spec exists. It defines how the engine is measured and records the M4 renderer's numbers (the baseline) and the optimizations against them (M5a threading, M5b GPU-driven rendering). **No target is set**: targets need more workloads (asset loading, streaming) and a second machine.

## 1. Reference hardware (MASTER-004 §5 "Reference hardware")

| Item | Value |
|---|---|
| GPU | AMD Radeon RX 9070 XT (RADV GFX1201), discrete |
| Driver | `radv Mesa 26.2.5-arch3.1` (from `DeviceInfo`, `VK_KHR_driver_properties`) |
| Vulkan device API | 1.4.354 |
| CPU | 12th Gen Intel Core i7-12700KF, 20 logical CPUs (8 P-cores with SMT + 4 E-cores) |
| RAM | 32 GB |
| OS | CachyOS Linux, kernel 7.3.1-1-cachyos, Wayland (niri compositor) |
| Dedicated CI runner | none; CI (lavapipe) does not run performance tests |

The machine is a development box, not a quiet lab machine: see section 8.

## 2. Vulkan environment

- Vulkan instance 1.4.363 (`vulkan-icd-loader 1.4.363.0-1.1`); the Khronos validation layer (1.4.363.0-1.1) is installed but **off** in benchmarks (release builds default to `validation=false`; the instance log line of every run says so). Validation costs far more than the effect being measured.
- Frames in flight: 2 (the engine default). Present mode: `MAILBOX` with `ANVIL_VSYNC=0` (never blocks on vblank).
- GPU clocks: driver default (`power_dpm_force_performance_level = auto`).

## 3. Build configuration

`cargo build --release`: workspace `[profile.release] lto = "thin"` (added in M5a), opt-level 3, default codegen units (16), unwinding panics, no `target-cpu=native`. `[profile.bench]` inherits release. Toolchain: rustc 1.99.1 (58cf8f9c3 2026-10-13); MSRV 1.88. Recorded per run in the JSON (`machine.rustc`, `machine.build`).

## 4. Workloads

All workloads are deterministic: camera, animation and lights are functions of the frame number; the field in `many-draws` comes from an integer hash.

| Workload | Run | Status |
|---|---|---|
| `pbr-scene-forward` | `pbr-scene` with `ANVIL_BENCH` (forward path, camera orbit): 20 PBR spheres, textured cube, alpha-tested fence, 3 glass spheres, ground, cascaded sun, point and spot lights, SSAO, TAA, bloom (26 draws, 99 shadow draws, 53 k triangles) | Measured |
| `pbr-scene-deferred-stress` | as above with `ANVIL_RENDER_PATH=deferred ANVIL_STRESS_LIGHTS=1` (259 lights) | Measured |
| `many-draws` | `many-draws`: 50 001 instances (sphere 16x8, cube, torus) over a 1.0 x 0.8 km field, 16 materials, same post effects, sun with 4 cascades, 3 point lights, 1 spot; fly-path camera | Measured |
| triangle, textured quad | `triangle`, `textured-quad` (M1/M2 smoke tests) | Not measured: no workload content worth a baseline |
| Shadows | covered by the cascade and spot passes of the three workloads above (per-pass GPU times, section 7) | Measured as passes |
| Asset-load set | needs the asset system (M6) | Undefined |
| Streaming set | texture streaming (M5c) | Undefined |

## 5. Resolution and present mode

1920x1080 (a fixed-size window: `ANVIL_BENCH` sets `resizable = false` so the tiling compositor leaves it alone; `ANVIL_WINDOW=WxH` overrides), vsync off. `ANVIL_BENCH` forces vsync off; on Wayland a hidden FIFO window would be throttled to the refresh rate and the numbers would be meaningless. Every JSON records the resolution that was actually rendered.

## 6. Run protocol

- 120 warm-up frames (pipeline creation, IBL bake, clock ramp), then 1000 measured frames; `ANVIL_BENCH_WARMUP` / `ANVIL_BENCH_FRAMES` override.
- 3 runs per workload (`docs/benchmarks/run.sh <tag>`); the committed file is the run with the median frame interval, and lists all three in `repetitions`.
- Statistics: median and p99 (nearest rank) over the measured frames; the mean is also stored.

## 7. Metrics (the 11 items of MASTER-003 §4.3)

| NFR | Metric | How it is measured | Status |
|---|---|---|---|
| PERF-001 | CPU frame preparation | `cpu_frame_ms`: time of the application's frame from `begin_frame` and acquire returning (the wait for the GPU is excluded) to `present` returning, i.e. all CPU work including scene extraction. Stages in `cpu_stages_ms` (also `tracing` spans `renderer.<stage>`; Tracy with `--features profile-tracy`) | Measured |
| PERF-002 | GPU frame time | `gpu_frame_ms`: first to last timestamp of the graph; `gpu_passes_ms`: per pass. Timestamps read back with a 2 frame delay (ADR-0013 §2) | Measured |
| PERF-003 | Frame pacing | `frame_interval_ms` (time between frames; median, p99, mean). Worst-frame and variance are not stored yet | Partly |
| PERF-004 | GPU memory | `vram_bytes.allocated` and `.reserved` from the allocator (`DeviceDiagnostics`) at the last frame | Measured |
| PERF-005 | CPU memory | not measured | Not yet |
| PERF-006 | Draw calls | `draws` (camera) and `shadow_draws` (all shadow maps), plus `triangles` | Measured |
| PERF-007 | Pipeline switches | not counted | Not yet |
| PERF-008 | Resource upload time | the per-frame ring upload is the `upload` stage; texture/mesh upload time is not measured | Partly |
| PERF-009 | Shader compilation | shaders compile at build time (shaderc in `build.rs`); runtime pipeline creation is not timed (the pipeline cache is persisted, MODULE-007 §8a) | Not yet |
| PERF-010 | Asset loading | needs the asset system (M6) | Undefined |
| PERF-011 | Graph execution | `graph_build`, `compile` and `execute` stages | Measured |

## 8. Variance control

Not controlled, but recorded:
- CPU governor `powersave` under `intel_pstate` with turbo enabled; no pinning of cores.
- Desktop session running (load average about 2.6 before the runs), including the compositor that presents the frames.
- GPU clocks ramp up during the first run: the first repetition of `pbr-scene-forward` has an 8 % slower GPU median (0.908 vs 0.838 ms), which is why the median of three is reported.
- Observed repetition spread (medians of the three runs): `many-draws` CPU 9.21-9.60 ms before, 7.14-7.22 ms after; pbr-scene-forward GPU 0.838 ms apart from the slow first run.
- Treat differences below about 5 % as noise on this machine.

## 9. Storage and comparison

Results live in `docs/benchmarks/results/<date>-<workload>-<tag>.json` (README in the same directory). Comparisons are before/after of the same workload on the reference machine. **Regression rule (ADR-0013 §1):** a median more than 10 % worse than the stored baseline needs a justification in the PR. Every M5 optimization is reported as a before/after against the `baseline` files (NFR-PERF-013). CI runs on lavapipe and does not run performance tests.

## 10. Baseline: the M4 renderer (serial CPU preparation)

Measured before any M5a threading. `many-draws` is CPU-bound, the pbr-scene workloads are GPU-bound at about 1100 frames/s.

| Workload | CPU frame med / p99 (ms) | Interval med / p99 (ms) | GPU frame med / p99 (ms) | Draws | Shadow draws | Triangles | Lights | VRAM alloc / reserved (MiB) |
|---|---|---|---|---|---|---|---|---|
| pbr-scene-forward | 0.103 / 0.208 | 0.862 / 1.008 | 0.838 / 0.899 | 26 | 99 | 53 008 | 3 | 260 / 548 |
| pbr-scene-deferred-stress | 0.381 / 0.441 | 0.914 / 1.047 | 0.896 / 0.947 | 26 | 99 | 53 008 | 259 | 341 / 629 |
| many-draws | 9.272 / 10.405 | 9.518 / 10.678 | 2.220 / 2.446 | 10 182 | 3 450 | 2 862 742 | 5 | 260 / 548 |

CPU stages, medians (ms):

| Stage | pbr-scene-forward | pbr-scene-deferred-stress | many-draws |
|---|---|---|---|
| pack (world bounds, instance data) | 0.001 | 0.001 | 2.130 |
| cull (camera frustum) | 0.000 | 0.000 | 0.266 |
| sort (camera lists) | 0.000 | 0.000 | 0.513 |
| shadow_lists | 0.002 | 0.002 | 0.429 |
| clusters | 0.010 | 0.264 | 0.027 |
| upload (ring) | 0.006 | 0.015 | 0.969 |
| graph_build | 0.003 | 0.003 | 0.009 |
| compile | 0.004 | 0.005 | 0.013 |
| execute (command recording) | 0.059 | 0.064 | 4.127 |

### Per-pass GPU time, pbr-scene-forward (median ms; frame 0.838)

shadow.cascade0 0.072, cascade1 0.046, cascade2 0.031, cascade3 0.026, shadow.spot0 0.018, spot1-3 0.004 each, prepass 0.025, ssao 0.044, ssao.blur 0.021, **forward 0.351**, skybox 0.012, transparent 0.014, taa 0.095, bloom.down0 0.019, down1 0.006, down2 0.004, down3-5 0.002 each, bloom.up4-2 0.002 each, up1 0.004, up0 0.010, tonemap 0.014, export barriers 0.002.

Deferred with 259 lights (frame 0.896): same shadow, prepass, ssao, taa, bloom and tonemap passes (within 0.005 ms), **gbuffer 0.035, lighting 0.374**, skybox 0.011, transparent 0.025. The deferred and forward paths cost the same on this scene; the 256 extra lights are absorbed by the clustered lighting (the lighting pass is 0.374 against forward's 0.351).

`many-draws` (frame 2.220): shadow.cascade0-3 0.013 / 0.024 / 0.060 / 0.265, spot0-3 about 0.005, **prepass 0.485, forward 1.026**, ssao 0.069, ssao.blur 0.040, skybox 0.013, taa 0.089, bloom 0.027 + smaller levels, tonemap 0.020.

(The timestamps wait for all earlier work, so passes that could overlap are measured serialized; per-pass numbers are for attribution. `p99` per pass is in the JSON.)

### What `many-draws` shows: the CPU-side limits of the M4 renderer

1. **Per-frame instance upload and fixed ring.** Every instance is packed (208 bytes) and copied to the per-frame upload ring whether or not it is visible (39 819 of 50 001 are frustum-culled). The M4 ring of 8 MiB failed the first run with `renderer upload ring overflow (10 400 208 > 8 388 608 bytes)`, so the limit was about 40 000 instances; the ring was raised to 16 MiB (about 75 000 instances). The upload stage is 0.97 ms (10.4 MB memcpy).
2. **Command recording.** One `draw_indexed` per instance and per view: 10 182 camera draws (the prepass and the main pass both draw them) plus 3 450 shadow draws, about 24 000 draw calls per frame, recorded on one thread: `execute` is 4.1 ms of 9.3 ms, the largest single stage, and the reason the frame is CPU-bound (CPU 9.3 ms against GPU 2.2 ms: the GPU idles 76 % of the frame).
3. **Preparation.** Packing 50 001 instances (world sphere, a 4x4 inverse-transpose per instance) takes 2.1 ms serially; culling, sorting and the shadow lists take another 1.2 ms.
4. **No LOD and no occlusion culling.** The camera draws 2.86 M triangles (many tiny, far instances of the 256-triangle sphere and the 576-triangle torus); the GPU time is dominated by the prepass and main pass over those.
5. **Application scene extraction.** `RenderScene` is rebuilt (cloned) every frame; 0.6 ms of the CPU frame is outside the renderer stages (scene clone, light setup, submit, present).

These are what M5b (GPU-driven culling, indirect draws, LOD, persistent instance buffer) targets; no target number is set here (section 9).

## 11. M5a: threading before/after

M5a parallelizes instance packing, frustum culling, draw-list sorting, the shadow lists and the cluster assignment on a `rayon` pool (`ANVIL_THREADS`, default 20; LAYER-001 §10, LAYER-005 M5a). Command recording and the ring upload stay on one thread. Same machine and protocol, `m5a` files.

| Workload | Metric | Baseline med / p99 (ms) | M5a med / p99 (ms) | Median change |
|---|---|---|---|---|
| pbr-scene-forward | CPU frame | 0.103 / 0.208 | 0.104 / 0.155 | +0.8 % (noise; the work runs inline below the threshold) |
| pbr-scene-forward | Frame interval | 0.862 / 1.008 | 0.856 / 0.991 | -0.7 % |
| pbr-scene-forward | GPU frame | 0.838 / 0.899 | 0.836 / 0.897 | -0.3 % |
| pbr-scene-deferred-stress | CPU frame | 0.381 / 0.441 | 0.311 / 0.486 | **-18.5 %** |
| pbr-scene-deferred-stress | Frame interval | 0.914 / 1.047 | 0.911 / 1.124 | -0.3 % (GPU-bound) |
| pbr-scene-deferred-stress | GPU frame | 0.896 / 0.947 | 0.891 / 0.937 | -0.6 % |
| many-draws | CPU frame | 9.272 / 10.405 | 7.184 / 8.382 | **-22.5 %** |
| many-draws | Frame interval | 9.518 / 10.678 | 7.259 / 8.479 | **-23.7 %** |
| many-draws | GPU frame | 2.220 / 2.446 | 2.208 / 2.425 | -0.5 % |

Stage medians (ms), baseline to M5a:

| Stage | pbr-scene-deferred-stress | many-draws |
|---|---|---|
| pack | 0.001 to 0.001 | 2.130 to 0.807 |
| cull | 0.000 to 0.002 | 0.266 to 0.154 |
| sort | 0.000 to 0.001 | 0.513 to 0.195 |
| shadow_lists | 0.002 to 0.003 | 0.429 to 0.141 |
| clusters | 0.264 to 0.151 | 0.027 to 0.028 |
| upload | 0.015 to 0.016 | 0.969 to 0.967 |
| graph_build + compile + execute | 0.072 to 0.097 | 4.149 to 4.259 |

Notes:
- The parallel stages of `many-draws` sum to 3.3 ms before and 1.3 ms after: 2.0 ms saved, 2.1 ms of CPU frame time (the rest is the unchanged recording, upload and extraction). Amdahl: `execute` (4.2 ms, 59 % of the frame) and `upload` (1.0 ms) are now the CPU frame.
- Scaling is limited: on `many-draws` the parallel stages gain little past 4 threads (`ANVIL_THREADS=1/4/8/20`: CPU frame about 8.4 / 6.9 / 7.0 / 7.2 ms in a development run with the final code), so the default of one thread per logical CPU is not the best on this CPU (hyper-threads and E-cores bring no gain); `ANVIL_THREADS=4..8` is. The default stays at the ADR's logical-CPU count until a second machine shows the same.
- `pbr-scene-deferred-stress` graph recording stages read 0.072 ms before and 0.097 ms after; the +0.025 ms is in the 25 % noise band of such short stages (the cluster assignment now wakes the pool, and the main thread's caches and frequency are affected). The workload remains GPU-bound; its interval and GPU time are unchanged.
- Two failed first attempts are worth recording: a naive `par_iter().collect()` of the packing made the stage slower with more threads (3.5 ms at 20 threads against 2.7 ms serial) because 14 MB of freshly allocated vectors page-faulted on first touch every frame; reusing the buffers fixed it. And parallelizing tiny scenes doubled the CPU frame of `pbr-scene-forward` (0.103 to 0.26 ms) until inputs under 2048 instances or 16 lights ran on the calling thread.
- Results are identical to the serial path: unit tests compare against plain serial code and a 1-thread pool, and the `many-draws` screenshot at frame 240 is byte-identical before and after.

## 12. M5b: GPU-driven rendering before/after

M5b replaces the per-frame instance pack and upload, the CPU culling and the one-draw-per-instance recording with a persistent GPU scene, GPU frustum and two-phase Hi-Z occlusion culling, LOD selection and indirect draws (LAYER-005 M5b). Same machine and protocol, `m5b` files; the median of three runs is committed. Defaults: GPU culling on, LOD threshold 0.5 px. Median / p99 in ms; the last two columns are the median change against M5a and against the baseline.

| Workload | Metric | Baseline | M5a | M5b | vs M5a | vs baseline |
|---|---|---|---|---|---|---|
| pbr-scene-forward | CPU frame | 0.103 / 0.208 | 0.104 / 0.155 | 0.109 / 0.246 | +4.9 % | +5.7 % |
| pbr-scene-forward | Frame interval | 0.862 / 1.008 | 0.856 / 0.991 | 0.864 / 1.022 | +1.0 % | +0.3 % |
| pbr-scene-forward | GPU frame | 0.838 / 0.899 | 0.836 / 0.897 | 0.844 / 0.913 | +0.9 % | +0.7 % |
| pbr-scene-deferred-stress | CPU frame | 0.381 / 0.441 | 0.311 / 0.486 | 0.327 / 0.503 | +5.4 % | -14.2 % |
| pbr-scene-deferred-stress | Frame interval | 0.914 / 1.047 | 0.911 / 1.124 | 0.931 / 1.155 | +2.2 % | +1.9 % |
| pbr-scene-deferred-stress | GPU frame | 0.896 / 0.947 | 0.891 / 0.937 | 0.912 / 0.973 | +2.4 % | +1.8 % |
| many-draws | CPU frame | 9.272 / 10.405 | 7.184 / 8.382 | **0.606 / 0.796** | **-91.6 %** | **-93.5 %** |
| many-draws | Frame interval | 9.518 / 10.678 | 7.259 / 8.479 | **1.276 / 1.491** | **-82.4 %** | **-86.6 %** |
| many-draws | GPU frame | 2.220 / 2.446 | 2.208 / 2.425 | **1.248 / 1.365** | **-43.5 %** | **-43.8 %** |
| many-draws, `ANVIL_GPU_CULLING=0` | CPU frame | | | 1.908 / 2.693 | | |
| many-draws, `ANVIL_GPU_CULLING=0` | Frame interval | | | 1.954 / 2.745 | | |
| many-draws, `ANVIL_GPU_CULLING=0` | GPU frame | | | 1.556 / 1.730 | | |

Counters of the last frame (GPU path `many-draws`, read back with the 2 frame delay): 50 001 instances, 39 809 frustum-culled, 140 occlusion-culled, 10 052 visible, 10 150 camera indirect draws (45 from phase 2), 3 454 shadow draws, **502 576 triangles after LOD against 2 862 742** (-82 %), 0 instances uploaded per frame (the baseline packed and uploaded all 50 001, 10.4 MB, every frame). `pbr-scene` (26 instances): 23 visible, 0 culled, 32 272 camera triangles after LOD against 53 008 submitted before (the sphere LODs).

CPU stages, `many-draws` medians (ms), M5a to M5b: pack 0.807 to 0.102 (now the change detection: one compare per instance), cull 0.154 to 0.000, sort 0.195 to 0.000, shadow_lists 0.141 to 0.000, upload 0.967 to 0.007, execute 4.236 to 0.071 (about 50 indirect draw calls and 20 dispatches instead of about 24 000 `draw_indexed` calls). What remains is the cluster assignment (0.021), graph build and compile (0.028) and the application's own scene extraction. The GPU path scales with the GPU, not the draw count.

GPU, `many-draws` per pass medians (ms), M5a to M5b: prepass 0.485 to 0.205 (+ `prepass.p2` 0.007), forward 1.026 to 0.492, shadow.cascade3 0.258 to 0.106, ssao 0.069 to 0.037, culling passes `cull.shadow` 0.033, `cull.camera.p1` 0.012, `cull.camera.p2` 0.022, depth pyramid `hiz.0..9` 0.024 in total, `cull.stats` 0.003: the whole culling machinery costs about 0.10 ms of the 1.25 ms frame. The frame is now GPU-bound (interval 1.276 against GPU 1.248 ms) and CPU 0.6 ms: the CPU idles 52 %.

**The CPU path (`ANVIL_GPU_CULLING=0`) also improved** on the same persistent buffers: CPU frame 7.18 to 1.91 ms (no instance pack or upload; the draw loop writes the push constants and binds the index buffer once per pass instead of per draw, `execute` 4.24 to 0.79 ms) and GPU frame 2.21 to 1.56 ms (less state per draw), still drawing 2.86 M triangles at level 0. So of the 7.18 to 0.61 ms gain, about 5.3 ms comes from the arena and the persistent scene and 1.3 ms from GPU culling, LOD and indirect draws.

**GPU on `pbr-scene` (reported honestly):** forward +0.9 % (0.836 to 0.844 ms, p99 0.897 to 0.913), deferred with 259 lights +2.4 % (0.891 to 0.912 ms). Not a regression by the 10 % rule of section 9 and at the 5 % noise level of section 8, but it is consistent across the three runs: a 26-instance scene gains nothing from GPU culling and pays for the extra passes, the eleven pyramid dispatches (`hiz.*`, 0.024 ms), `prepass.p2` (0.006), `cull.stats` and the culling dispatches (0.010), about 0.04 ms, of which the LOD savings (sphere levels in the shadow and forward passes: forward 0.350 to 0.337 ms, cascade 0 0.072 to 0.064) take back about 0.03 ms. The pbr-scene CPU frame rose 0.104 to 0.109 ms (+4.9 %, about 20 more passes to declare and record). VRAM allocated: pbr-scene 260 to 280 MiB, many-draws 260 to 312 MiB (the 16 MiB arena, the 14.7 MiB instance buffer at 65 536 slots, the 10 MiB command buffer, the pyramid).

Visual equivalence (screenshots at frame 120, 1242x1350, pixel diffs in `docs/benchmarks/README.md`): with `ANVIL_LOD=0` the GPU path is **byte-identical** to the CPU path and to the M5a build for `many-draws` (also at frames 2, 61, 400 and 900) and `pbr-scene` (forward and deferred), so frustum and occlusion culling are exact on these scenes; with the default 0.5 px LOD `pbr-scene` differs in 0.22 % of the pixels (0.003 % by more than 8/255, maximum 69) and `many-draws` in 6.7 % (0.67 % by more than 8/255, mean absolute difference 0.2/255), at the silhouettes of far spheres and tori.

Caveats: the `many-draws` field is sparse (140 of 50 001 instances are occluded), so the occlusion win is small here; the two-phase scheme is exercised on denser occluders by the renderer's GPU test (a wall hides 400 spheres: 371 of the 407 instances are occlusion-culled, 29 frustum-culled, 7 drawn, image identical to the CPU path). LOD error is geometric only, normals are not part of the metric (hence the silhouette-only differences at 0.5 px). The `many-draws-cpu-path` file is the comparison run required by M5b; it is meaningless for earlier tags (the variable is ignored there).

## 12. Related

ADR-0013 (§1 protocol, §2 profiling, §3 threads), MASTER-004 §5, LAYER-005 (M5a), LAYER-006 §8a, LAYER-003 §20, MODULE-007 §8a.
