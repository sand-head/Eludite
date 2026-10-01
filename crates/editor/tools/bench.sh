#!/usr/bin/env bash
# Benchmarks for the example viewer (briefs 0009 and 0011). Generates the
# input files, runs each benchmark RUNS times inside a nested KWin (the method
# from spikes/0005-acp-panel/tools/nested.sh, needed while the session is
# locked; pass --direct to run on the current display instead), appends one
# JSON line per run to $OUT/results.jsonl, and prints a memory summary.
#
# Each run waits until the 1-minute load average is below LOAD_MAX (default
# 2.5), so other builds on the machine do not skew frame times; the load at
# start is recorded. ONLY=<regex> limits the runs to matching labels.
# VIEWER=<path> runs a prebuilt viewer instead of building one (for example a
# `--features system-allocator` build, to compare allocators). TAG=<text> is
# recorded with each result.
#
# Memory (brief 0011): the open runs record RSS at first paint, when
# highlighting completes and idle 2 s later; the type runs record RSS before,
# every 50 keystrokes, 300 ms after the last one and settled (highlighting
# caught up, then 2 s idle), plus peak RSS (VmHWM) for the process.
#
# Usage: crates/editor/tools/bench.sh [--direct] [OUT_DIR]
set -euo pipefail
direct=0
if [[ "${1:-}" == "--direct" ]]; then direct=1; shift; fi
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
out=$(mkdir -p "${1:-$root/target/bench-0011}" && cd "${1:-$root/target/bench-0011}" && pwd)
runs=${RUNS:-3}
cd "$root"
if [[ -z ${VIEWER:-} ]]; then
  cargo build --release -p eludite-editor --example viewer
  viewer=$root/target/release/examples/viewer
else
  viewer=$VIEWER
fi
cs=$out/Generated100k.cs rs=$out/generated20k.rs big=$out/Generated10MB.cs
[[ -f $cs ]] || "$viewer" --generate csharp 100000 "$cs"
[[ -f $rs ]] || "$viewer" --generate rust 20000 "$rs"
[[ -f $big ]] || "$viewer" --generate csharp 325000 "$big"

run() { # label args...
  local label=$1; shift
  [[ -n ${ONLY:-} && ! $label =~ $ONLY ]] && return 0
  for i in $(seq "$runs"); do
    while awk -v max="${LOAD_MAX:-2.5}" '{exit !($1 >= max)}' /proc/loadavg; do sleep 5; done
    local load
    load=$(cut -d' ' -f1 /proc/loadavg)
    : >"$out/nested.out"
    if [[ $direct == 1 ]]; then
      "$viewer" "$@" >"$out/nested.out" 2>"$out/nested.err" || true
    else
      OUT_DIR=$out timeout 600 "$root/spikes/0005-acp-panel/tools/nested.sh" -- "$viewer" "$@"
    fi
    local line load_end
    line=$(tail -n1 "$out/nested.out")
    load_end=$(cut -d' ' -f1 /proc/loadavg)
    echo "{\"label\":\"$label\",\"tag\":\"${TAG:-}\",\"run\":$i,\"load_at_start\":$load,\"load_at_end\":$load_end,\"result\":${line:-null}}" | tee -a "$out/results.jsonl"
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
run type-10mb-cs "$big" --bench-type 500

# Memory summary over this TAG's runs: MiB, min to max over the runs.
jq -rs --arg tag "${TAG:-}" '
  def span(f): [.[] | f | select(. != null)] | if length == 0 then "-" else "\(min) to \(max)" end;
  [.[] | select(.tag == $tag and .result != null)] as $all
  | ["file", "idle highlighted", "peak (open)", "after 500 keys (settled)", "peak (typing)"],
    (["100k-cs", "20k-rs", "10mb-cs"][] as $f
     | [$all[] | select(.label == "open-\($f)")] as $o
     | [$all[] | select(.label == "type-\($f)")] as $t
     | [$f, ($o | span(.result.rss_idle.rss_mib)), ($o | span(.result.rss_idle.peak_rss_mib)),
        ($t | span(.result.rss_settled.rss_mib)), ($t | span(.result.rss_settled.peak_rss_mib))])
  | @tsv' "$out/results.jsonl" | column -t -s $'\t'
