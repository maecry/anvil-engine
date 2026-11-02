# Benchmarks

Specifications and results of the engine's performance measurements (MASTER-000 §4.3 and §22 p3, ADR-0013 §1).

| File | Content |
|---|---|
| `BENCH-001-baseline.md` | The benchmark spec: reference machine, protocol, workloads, the M4 baseline, M5a before/after. Answers every item of MASTER-004 §5. |
| `run.sh` | Runs the BENCH-001 workloads (3 runs each, release build) and writes the result files. |
| `results/<date>-<workload>-<tag>.json` | Committed results; `baseline` is the M4 renderer measured before M5a threading, `m5a` the same workloads after it. |

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

## Result files

One JSON object per file (schema 1): `workload`, `variant_env` (every `ANVIL_*` variable of the run), `machine` (GPU, driver, Vulkan API, CPU model, logical CPUs, worker threads, OS, kernel, rustc, build profile), `protocol`, `cpu_frame_ms`, `frame_interval_ms`, `gpu_frame_ms` (each `median`, `p99`, `mean`, `samples`), `cpu_stages_ms` (the renderer's preparation stages), `gpu_passes_ms` (median and p99 per graph pass), `draws`, `shadow_draws`, `triangles`, `lights`, `culled_instances`, `vram_bytes`. Files written by `run.sh` also carry `representative_run` (the run with the median frame interval) and `repetitions` (all three runs' CPU, interval and GPU medians and p99s).

Regression rule: a median more than 10% worse than the stored baseline on the reference machine needs a justification. CI does not run performance tests (it uses lavapipe).
