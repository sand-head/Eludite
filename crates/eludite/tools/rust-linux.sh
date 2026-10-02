#!/usr/bin/env bash
# Brief 0019 manual run in a nested virtual KWin (works while the session is locked): Eludite editing and building
# Eludite. The folder is this repository (a .NET solution and a Cargo workspace), the file crates/editor/src/buffer.rs,
# with the real eludite-host (and Roslyn) and the real rust-analyzer.
#   1. X11 backend on the nested Xwayland, real XTest input (tools/rust.py): the Workspace window, rust-analyzer's
#      completion and diagnostics, an error, Ctrl+S, Ctrl+Shift+B, cargo's Error List row and click-through
#                                                                                   -> OUT_DIR/*.png, drive.json
#   2. Wayland backend: --timings-out, cold (target/rust-analyzer removed first) then RUNS warm runs
#                                                                                   -> OUT_DIR/timings.jsonl
#   3. Wayland backend, RUNS runs each: --bench-type 500 in buffer.rs while rust-analyzer indexes (and the same with
#      no language server: ELUDITE_RUST_ANALYZER pointing nowhere), --bench-complete 100 after `self.`, and
#      --bench-build 5 (Ctrl+Shift+B to the first Output line with cargo)          -> OUT_DIR/bench.jsonl
#   The 1-minute load average before each step goes to OUT_DIR/loadavg.txt.
# Usage: tools/rust-linux.sh OUT_DIR    (RUNS=3; SKIP_DRIVE=1, SKIP_TIMINGS=1, SKIP_BENCH=1 skip steps)
# buffer.rs must be clean in git; the run restores it with `git checkout` at the end.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
file=$repo/crates/editor/src/buffer.rs
if [[ -n "$(git -C "$repo" status --porcelain -- "$file")" ]]; then
  echo "$file has changes; commit or stash them first (the run restores it from git)" >&2
  exit 1
fi
target_dir=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target_dir/release/eludite}
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
runs=${RUNS:-3}
title="$(basename "$repo") - Eludite"
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1
wl=\$WAYLAND_DISPLAY
load() { echo "\$1 \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt"); }
wait_file() { for i in \$(seq 1 9000); do [[ -s "\$1" ]] && return 0; sleep 0.1; done; return 1; }
if [[ -z "${SKIP_DRIVE:-}" ]]; then
  load drive
  env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --folder $(q "$repo") --open-file $(q "$file") \\
    --bounds-out $(q "$out")/bounds.json >$(q "$out")/drive.out 2>$(q "$out")/drive.err &
  pid=\$!
  SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/rust.py") --title $(q "$title") \\
    --log $(q "$out")/drive.err --bounds $(q "$out")/bounds.json --shots $(q "$out") \\
    >$(q "$out")/drive.json 2>$(q "$out")/driver.err || true
  sleep 1
  kill \$pid; wait \$pid
  git -C $(q "$repo") diff -- $(q "$file") >$(q "$out")/buffer-after.diff
  git -C $(q "$repo") checkout -q -- $(q "$file")
fi
if [[ -z "${SKIP_TIMINGS:-}" ]]; then
  for run in cold \$(seq 1 $runs); do
    [[ \$run == cold ]] && rm -rf $(q "$target_dir/rust-analyzer")
    load "timings-\$run"
    rm -f $(q "$out")/timings-\$run.json
    $(q "$bin") --reset-layout --no-persist --folder $(q "$repo") --open-file $(q "$file") \\
      --timings-out $(q "$out")/timings-\$run.json >$(q "$out")/timings-\$run.out 2>$(q "$out")/timings-\$run.err &
    pid=\$!; wait_file $(q "$out")/timings-\$run.json; kill \$pid; wait \$pid
    { printf '{"run":"%s","timings":' "\$run"; cat $(q "$out")/timings-\$run.json; echo '}'; } >>$(q "$out/timings.jsonl")
  done
fi
if [[ -z "${SKIP_BENCH:-}" ]]; then
  for run in \$(seq 1 $runs); do
    load "type-\$run"
    $(q "$bin") --reset-layout --no-persist --folder $(q "$repo") --open-file $(q "$file") --bench-type 500 \\
      >>$(q "$out/bench.jsonl") 2>$(q "$out")/type-\$run.err
    load "type-no-server-\$run"
    ELUDITE_RUST_ANALYZER=/nonexistent/rust-analyzer $(q "$bin") --reset-layout --no-persist --open-file $(q "$file") \\
      --bench-type 500 >>$(q "$out/bench.jsonl") 2>$(q "$out")/type-no-server-\$run.err
    load "complete-\$run"
    ELUDITE_BENCH_COMPLETE_AFTER=self $(q "$bin") --reset-layout --no-persist --folder $(q "$repo") \\
      --open-file $(q "$file") --bench-complete 100 >>$(q "$out/bench.jsonl") 2>$(q "$out")/complete-\$run.err
    load "build-\$run"
    $(q "$bin") --reset-layout --no-persist --folder $(q "$repo") --open-file $(q "$file") --bench-build 5 \\
      >>$(q "$out/bench.jsonl") 2>$(q "$out")/build-\$run.err
  done
fi
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 7200 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
git -C "$repo" checkout -q -- "$file"
git -C "$repo" status --porcelain -- "$file"
ls "$out"
