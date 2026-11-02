#!/usr/bin/env bash
# Runs the BENCH-001 workloads 3 times each (release build, vsync off, 1920x1080) and writes
# docs/benchmarks/results/<date>-<workload>-<tag>.json: the run with the median frame interval,
# plus a "repetitions" array with all three. Usage: docs/benchmarks/run.sh <tag> (baseline, m5a, ...)
set -euo pipefail
cd "$(dirname "$0")/../.."
tag=${1:?usage: run.sh <tag>}
date=${BENCH_DATE:-$(date +%F)}
cargo build --release -p pbr-scene -p many-draws
tmp=$(mktemp -d)

workload() { # name binary [ENV=value ...]
  local name=$1 bin=$2; shift 2
  for i in 1 2 3; do
    env ANVIL_VSYNC=0 ANVIL_BENCH="$tmp/$name-$i.json" "$@" timeout 600 "target/release/$bin"
  done
  python3 - "$tmp/$name" "docs/benchmarks/results/$date-$name-$tag.json" <<'PY'
import json, sys
prefix, out = sys.argv[1:]
runs = [json.load(open(f"{prefix}-{i}.json")) for i in (1, 2, 3)]
order = sorted(range(3), key=lambda i: runs[i]["frame_interval_ms"]["median"])
best = runs[order[1]]
best["representative_run"] = order[1] + 1
best["repetitions"] = [
    {k: {"median": r[k]["median"], "p99": r[k]["p99"]}
     for k in ("cpu_frame_ms", "frame_interval_ms", "gpu_frame_ms")} for r in runs]
json.dump(best, open(out, "w"), indent=2)
open(out, "a").write("\n")
PY
}

workload pbr-scene-forward pbr-scene
workload pbr-scene-deferred-stress pbr-scene ANVIL_RENDER_PATH=deferred ANVIL_STRESS_LIGHTS=1
workload many-draws many-draws
