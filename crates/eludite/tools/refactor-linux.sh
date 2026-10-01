#!/usr/bin/env bash
# Brief 0015 manual run in a nested virtual KWin (works while the session is locked), on dotnet/Eludite.slnx with
# the real eludite-host and Roslyn:
#   1. X11 backend on the nested Xwayland, real XTest input (tools/refactor.py):
#      rename `HostRpcTarget.Ping` to `PingHost` with its preview (rename-preview.png), apply "Use primary
#      constructor" to DotnetCliSdkDiscoverer's constructor from the light bulb menu (code-action-menu.png), and accept a
#      completion for StringBuilder, an unimported type, which adds `using System.Text;` (completion-using.png).
#   2. Wayland backend: RUNS runs of --bench-refactor N in DotnetCliSdkDiscoverer.cs (light bulb host and UI latency,
#      keystroke frame cost with the bulb active, rename preview latency, apply to 10 closed files)
#                                                                         -> OUT_DIR/refactor.jsonl
#   The 1-minute load average before each run goes to OUT_DIR/loadavg.txt.
# Usage: tools/refactor-linux.sh OUT_DIR    (RUNS=3 N=100 by default; SKIP_DRIVE=1 skips 1, SKIP_BENCH=1 skips 2)
# The rename writes the closed files it touches (that is the applier's job); the run needs dotnet/ clean in git and
# restores it with `git checkout -- dotnet/` at the end. The host is ELUDITE_HOST (default: the Debug build).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
if [[ -n "$(git -C "$repo" status --porcelain -- dotnet/)" ]]; then
  echo "dotnet/ has changes; commit or stash them first (the run restores dotnet/ from git)" >&2
  exit 1
fi
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/release/eludite}
sln=$repo/dotnet/Eludite.slnx
target=$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
sdk=$repo/dotnet/src/Eludite.Host/Sdk/DotnetCliSdkDiscoverer.cs
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
runs=${RUNS:-3}; n=${N:-100}
title="Eludite - Eludite"
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1
wl=\$WAYLAND_DISPLAY
load() { echo "\$1 \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt"); }
restore() { git -C $(q "$repo") checkout -q -- dotnet/; }
drive() { # mode file
  load "x11-\$1"
  env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --solution $(q "$sln") --open-file "\$2" \\
    --bounds-out $(q "$out")/bounds-\$1.json >$(q "$out")/\$1.out 2>$(q "$out")/\$1.err &
  local pid=\$!
  SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/refactor.py") --mode "\$1" --title $(q "$title") \\
    --log $(q "$out")/\$1.err --bounds $(q "$out")/bounds-\$1.json --shots $(q "$out") \\
    >$(q "$out")/\$1.json 2>$(q "$out")/\$1-driver.err || true
  kill \$pid; wait \$pid
  git -C $(q "$repo") status --porcelain -- dotnet/ >$(q "$out")/\$1-changed.txt
  restore
}
if [[ -z "${SKIP_DRIVE:-}" ]]; then
  drive rename $(q "$target")
  drive action $(q "$sdk")
  drive completion $(q "$sdk")
fi
if [[ -z "${SKIP_BENCH:-}" ]]; then
  for run in \$(seq 1 $runs); do
    load "refactor-\$run"
    $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$sdk") --bench-refactor $n \\
      >>$(q "$out/refactor.jsonl") 2>$(q "$out")/refactor-\$run.err
    restore
  done
fi
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 5400 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
git -C "$repo" checkout -q -- dotnet/
git -C "$repo" status --porcelain -- dotnet/
ls "$out"
