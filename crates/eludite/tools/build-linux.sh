#!/usr/bin/env bash
# Brief 0017 manual run in a nested virtual KWin (works while the session is locked), on dotnet/Eludite.slnx with the
# real eludite-host and Roslyn:
#   1. X11 backend on the nested Xwayland, real XTest input (tools/build.py): Ctrl+Shift+B builds the solution (Output
#      streaming, status bar, then the empty Error List), then an error typed at the end of HostRpcTarget.cs, Ctrl+S,
#      Ctrl+Shift+B: the build's Error List row and a double-click through to the error.     -> OUT_DIR/*.png, drive.json
#   2. Wayland backend, RUNS runs each: --bench-build 5 on the solution (Ctrl+Shift+B to the first Output line, finished
#      to Error List rows) and --bench-output 10 (100k lines in 10 s)                         -> OUT_DIR/build-bench.jsonl
#   The 1-minute load average before each step goes to OUT_DIR/loadavg.txt.
# The host runs from a copy (OUT_DIR/host), because building Eludite.slnx rewrites the host's own bin/ directory.
# Usage: tools/build-linux.sh OUT_DIR    (RUNS=3; SKIP_DRIVE=1 skips 1, SKIP_BENCH=1 skips 2)
# The run needs dotnet/ clean in git and restores it with `git checkout -- dotnet/` at the end.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
if [[ -n "$(git -C "$repo" status --porcelain -- dotnet/)" ]]; then
  echo "dotnet/ has changes; commit or stash them first (the run restores dotnet/ from git)" >&2
  exit 1
fi
target_dir=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target_dir/release/eludite}
sln=$repo/dotnet/Eludite.slnx
file=$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
rm -rf "$out/host"
cp -r "$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0" "$out/host"
host=${ELUDITE_HOST:-$out/host/eludite-host}
runs=${RUNS:-3}
title="Eludite - Eludite"
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1
wl=\$WAYLAND_DISPLAY
load() { echo "\$1 \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt"); }
if [[ -z "${SKIP_DRIVE:-}" ]]; then
  load drive
  env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$file") \\
    --bounds-out $(q "$out")/bounds.json >$(q "$out")/drive.out 2>$(q "$out")/drive.err &
  pid=\$!
  SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/build.py") --title $(q "$title") \\
    --log $(q "$out")/drive.err --bounds $(q "$out")/bounds.json --shots $(q "$out") \\
    >$(q "$out")/drive.json 2>$(q "$out")/driver.err || true
  sleep 1
  kill \$pid; wait \$pid
  git -C $(q "$repo") diff -- dotnet/ >$(q "$out")/dotnet-after.diff
  git -C $(q "$repo") checkout -q -- dotnet/
fi
if [[ -z "${SKIP_BENCH:-}" ]]; then
  for run in \$(seq 1 $runs); do
    load "build-\$run"
    $(q "$bin") --reset-layout --no-persist --solution $(q "$sln") --bench-build 5 \\
      >>$(q "$out/build-bench.jsonl") 2>$(q "$out")/build-\$run.err
    load "output-\$run"
    $(q "$bin") --reset-layout --no-persist --bench-output 10 \\
      >>$(q "$out/build-bench.jsonl") 2>$(q "$out")/output-\$run.err
  done
fi
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 3600 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
git -C "$repo" checkout -q -- dotnet/
git -C "$repo" status --porcelain -- dotnet/
ls "$out"
