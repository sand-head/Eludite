#!/usr/bin/env bash
# Brief 0014 manual run in a nested virtual KWin (works while the session is locked), on dotnet/Eludite.slnx with
# the real eludite-host and Roslyn:
#   1. X11 backend on the nested Xwayland, real XTest input (tools/navigation.py):
#      F12 on `JsonRpc` in HostServer.cs (definition-metadata.png), Shift+F12 on `HostRpcTarget` in HostRpcTarget.cs
#      (find-all-references.png), and the Error List filtered to warnings only (error-list-warnings.png) after
#      typing two unused locals and an unknown name into HostRpcTarget.cs. The solution builds with
#      TreatWarningsAsErrors (dotnet/Directory.Build.props), which makes every compiler warning an error; for this
#      run only, WarningsNotAsErrors=CS0168;CS0219 is set in the environment, which MSBuild reads as a property in
#      Roslyn's design-time build, so those two stay warnings. No file changes.
#   2. Wayland backend: RUNS runs of --bench-navigate N in HostRpcTarget.cs (Go To Definition and Find All References
#      host and UI latency, keystroke frame cost with the references window holding 1000 rows)
#                                                                         -> OUT_DIR/navigate.jsonl
#   The 1-minute load average before each run goes to OUT_DIR/loadavg.txt.
# Usage: tools/navigation-linux.sh OUT_DIR    (RUNS=3 N=100 by default; SKIP_DRIVE=1 skips 1, SKIP_BENCH=1 skips 2)
# The host is ELUDITE_HOST (default: the Debug build under dotnet/src/Eludite.Host/bin). Files are never saved.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/release/eludite}
sln=$repo/dotnet/Eludite.slnx
target=$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
server=$repo/dotnet/src/Eludite.Host/Rpc/HostServer.cs
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
runs=${RUNS:-3}; n=${N:-100}
title="Eludite - Eludite"
for f in "$target" "$server"; do cp "$f" "$out/$(basename "$f").orig"; done
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1
wl=\$WAYLAND_DISPLAY
load() { echo "\$1 \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt"); }
drive() { # mode file [NAME=VALUE...]
  load "x11-\$1"
  env -u WAYLAND_DISPLAY "\${@:3}" $(q "$bin") --reset-layout --solution $(q "$sln") --open-file "\$2" \\
    --bounds-out $(q "$out")/bounds-\$1.json >$(q "$out")/\$1.out 2>$(q "$out")/\$1.err &
  local pid=\$!
  SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/navigation.py") --mode "\$1" --title $(q "$title") \\
    --log $(q "$out")/\$1.err --bounds $(q "$out")/bounds-\$1.json --shots $(q "$out") \\
    >$(q "$out")/\$1.json 2>$(q "$out")/\$1-driver.err || true
  kill \$pid; wait \$pid
}
if [[ -z "${SKIP_DRIVE:-}" ]]; then
  drive definition $(q "$server")
  drive references $(q "$target")
  drive errors $(q "$target") WarningsNotAsErrors='CS0168;CS0219'
fi
if [[ -z "${SKIP_BENCH:-}" ]]; then
  for run in \$(seq 1 $runs); do
    load "navigate-\$run"
    $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$target") --bench-navigate $n \\
      >>$(q "$out/navigate.jsonl") 2>$(q "$out")/navigate-\$run.err
  done
fi
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 3600 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
for f in "$target" "$server"; do
  cmp -s "$f" "$out/$(basename "$f").orig" || { echo "restoring $f"; cp "$out/$(basename "$f").orig" "$f"; }
done
ls "$out"
