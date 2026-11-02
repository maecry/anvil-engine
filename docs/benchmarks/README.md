# Benchmarks

Specifications and results of the engine's performance measurements (MASTER-000 §4.3 and §22 p3, ADR-0013 §1).

| File | Content |
|---|---|
| `BENCH-001-baseline.md` | The benchmark spec: reference machine, protocol, workloads, the M4 baseline, M5a before/after. Answers every item of MASTER-004 §5. |
| `run.sh` | Runs the BENCH-001 workloads (3 runs each, release build) and writes the result files. |
| `results/<date>-<workload>-<tag>.json` | Committed results; `baseline` is the M4 renderer measured before M5a threading, `m5a` the same workloads after it, `m5b` after GPU-driven rendering (plus `many-draws-cpu-path-m5b`: the same workload with `ANVIL_GPU_CULLING=0`). |

## Run

```sh
docs/benchmarks/run.sh <tag>        # e.g. m5a; writes results/<today>-<workload>-<tag>.json
```

A single run by hand:

```sh
ANVIL_VSYNC=0 ANVIL_BENCH=out.json cargo run --release -p many-draws
ANVIL_VSYNC=0 ANVIL_BENCH=out.json ANVIL_RENDER_PATH=deferred ANVIL_STRESS_LIGHTS=1 cargo run --release -p pbr-scene
```

| Env | Effect |
|---|---|
| `ANVIL_BENCH=path.json` | Enables the protocol: deterministic camera (orbit / fly path), vsync off, 1920x1080 fixed-size window, exits after warm-up plus measured frames, writes the JSON |
| `ANVIL_BENCH_WARMUP` / `ANVIL_BENCH_FRAMES` | Frames before measuring (default 120) and measured frames (default 1000) |
| `ANVIL_WINDOW=WxH` | Overrides the window size |
| `ANVIL_THREADS=n` | Worker pool size (default: logical CPUs) |
| `ANVIL_INSTANCES=n` | many-draws instance count (default 50000) |
| `ANVIL_GPU_CULLING=0` | M5b: the CPU culling and per-instance draw path (default: GPU-driven) |
| `ANVIL_LOD=0\|<px>` | M5b: LOD error threshold in pixels (default 0.5; 0 = always level 0) |
| `ANVIL_DEBUG_VIEW=lod\|occlusion` | M5b: tint instances by LOD level / by being drawn in occlusion phase 2 |

## Result files

One JSON object per file (schema 1): `workload`, `variant_env` (every `ANVIL_*` variable of the run), `machine` (GPU, driver, Vulkan API, CPU model, logical CPUs, worker threads, OS, kernel, rustc, build profile), `protocol`, `cpu_frame_ms`, `frame_interval_ms`, `gpu_frame_ms` (each `median`, `p99`, `mean`, `samples`), `cpu_stages_ms` (the renderer's preparation stages), `gpu_passes_ms` (median and p99 per graph pass), `draws`, `shadow_draws`, `triangles`, `lights`, `culled_instances`, `vram_bytes`. Files written by `run.sh` also carry `representative_run` (the run with the median frame interval) and `repetitions` (all three runs' CPU, interval and GPU medians and p99s).

Regression rule: a median more than 10% worse than the stored baseline on the reference machine needs a justification. CI does not run performance tests (it uses lavapipe).

## M5b visual equivalence

Screenshots (`ANVIL_VSYNC=0 ANVIL_EXIT_AFTER_FRAMES=120 ANVIL_SCREENSHOT=...`, 1242x1350) compared with an RGB per-pixel diff:

| Comparison | Identical pixels | Mean abs diff | Max | > 8/255 |
|---|---|---|---|---|
| many-draws, M5a build vs M5b `ANVIL_LOD=0` | 100 % | 0 | 0 | 0 % |
| many-draws, CPU path vs GPU path `ANVIL_LOD=0`, frames 2, 61, 120, 400, 900 | 100 % each | 0 | 0 | 0 % |
| pbr-scene (forward and deferred), M5a build vs M5b `ANVIL_LOD=0` / CPU path | 100 % | 0 | 0 | 0 % |
| pbr-scene, M5a build vs M5b default (LOD 0.5 px) | 99.78 % | 0.0022 | 69 | 0.003 % |
| many-draws, CPU path vs M5b default (LOD 0.5 px) | 93.33 % | 0.20 | 168 | 0.67 % |
