#!/usr/bin/env bash
# Brief 0013 manual run in a nested virtual KWin (works while the session is locked), on dotnet/Eludite.slnx with
# the real eludite-host and Roslyn:
#   1. X11 backend on the nested Xwayland, real XTest input (tools/intellisense.py --mode edit): HostRpcTarget.cs,
#      `_sdkDiscoverer.` on a new line (completion.png), then `string.Join(",", ` (signature-help.png)
#   2. the same with HostServer.cs (--mode hover): the pointer rests on `JsonRpc` (quick-info.png)
#   3. Wayland backend: RUNS runs of --bench-complete TRIGGERS (host and UI latency, keystroke frame cost with the
#      list open)                                                        -> OUT_DIR/complete.jsonl
#      one run of --bench-complete CYCLES (memory)                        -> OUT_DIR/complete-memory.jsonl
#      RUNS runs of --bench-type 500 in LspProxy.cs (brief 0012's typing benchmark, now with IntelliSense)
#                                                                         -> OUT_DIR/type.jsonl
#   The 1-minute load average before each run goes to OUT_DIR/loadavg.txt.
# Usage: tools/intellisense-linux.sh OUT_DIR    (RUNS=3 TRIGGERS=200 CYCLES=500 by default)
# The host is ELUDITE_HOST (default: the Debug build under dotnet/src/Eludite.Host/bin). Files are never saved.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/release/eludite}
sln=$repo/dotnet/Eludite.slnx
target=$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
server=$repo/dotnet/src/Eludite.Host/Rpc/HostServer.cs
proxy=$repo/dotnet/src/Eludite.Host/Lsp/LspProxy.cs
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
runs=${RUNS:-3}; triggers=${TRIGGERS:-200}; cycles=${CYCLES:-500}
title="Eludite - Eludite"
for f in "$target" "$server" "$proxy"; do cp "$f" "$out/$(basename "$f").orig"; done
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1
wl=\$WAYLAND_DISPLAY
load() { echo "\$1 \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt"); }
drive() { # mode file
  load "x11-\$1"
  env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --solution $(q "$sln") --open-file "\$2" \\
    --bounds-out $(q "$out")/bounds-\$1.json >$(q "$out")/\$1.out 2>$(q "$out")/\$1.err &
  local pid=\$!
  SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/intellisense.py") --mode "\$1" --title $(q "$title") \\
    --log $(q "$out")/\$1.err --bounds $(q "$out")/bounds-\$1.json --shots $(q "$out") \\
    >$(q "$out")/\$1.json 2>$(q "$out")/\$1-driver.err || true
  kill \$pid; wait \$pid
}
drive edit $(q "$target")
drive hover $(q "$server")
for run in \$(seq 1 $runs); do
  load "complete-\$run"
  $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$target") --bench-complete $triggers \\
    >>$(q "$out/complete.jsonl") 2>$(q "$out")/complete-\$run.err
done
load memory
$(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$target") --bench-complete $cycles \\
  >>$(q "$out/complete-memory.jsonl") 2>$(q "$out/complete-memory.err")
for run in \$(seq 1 $runs); do
  load "type-\$run"
  $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$proxy") --bench-type 500 \\
    >>$(q "$out/type.jsonl") 2>$(q "$out")/type-\$run.err
done
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 3600 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
for f in "$target" "$server" "$proxy"; do
  cmp -s "$f" "$out/$(basename "$f").orig" || { echo "restoring $f"; cp "$out/$(basename "$f").orig" "$f"; }
done
ls "$out"
