#!/usr/bin/env bash
# Brief 0009 benchmarks for the example viewer. Generates the input files,
# runs each benchmark RUNS times inside a nested KWin (the method from
# spikes/0005-acp-panel/tools/nested.sh, needed while the session is locked;
# pass --direct to run on the current display instead), and appends one JSON
# line per run to $OUT/results.jsonl.
#
# Usage: crates/editor/tools/bench.sh [--direct] [OUT_DIR]
set -euo pipefail
direct=0
if [[ "${1:-}" == "--direct" ]]; then direct=1; shift; fi
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
out=$(mkdir -p "${1:-$root/target/bench-0009}" && cd "${1:-$root/target/bench-0009}" && pwd)
runs=${RUNS:-3}
cd "$root"
cargo build --release -p niello-editor --example viewer
viewer=$root/target/release/examples/viewer
cs=$out/Generated100k.cs rs=$out/generated20k.rs big=$out/Generated10MB.cs
[[ -f $cs ]] || "$viewer" --generate csharp 100000 "$cs"
[[ -f $rs ]] || "$viewer" --generate rust 20000 "$rs"
[[ -f $big ]] || "$viewer" --generate csharp 325000 "$big"

run() { # label args...
  local label=$1; shift
  for i in $(seq "$runs"); do
    if [[ $direct == 1 ]]; then
      "$viewer" "$@" >"$out/nested.out" 2>"$out/nested.err" || true
    else
      OUT_DIR=$out timeout 600 "$root/spikes/0005-acp-panel/tools/nested.sh" -- "$viewer" "$@"
    fi
    local line
    line=$(tail -n1 "$out/nested.out")
    echo "{\"label\":\"$label\",\"run\":$i,\"result\":${line:-null}}" | tee -a "$out/results.jsonl"
  done
}

run open-100k-cs "$cs" --bench-open
run open-20k-rs "$rs" --bench-open
run open-10mb-cs "$big" --bench-open
run scroll40-100k-cs "$cs" --bench-scroll --lines-per-frame 40
run scroll8-100k-cs "$cs" --bench-scroll --lines-per-frame 8
run scroll40-20k-rs "$rs" --bench-scroll --lines-per-frame 40
run type-100k-cs "$cs" --bench-type 500
run type-20k-rs "$rs" --bench-type 500
