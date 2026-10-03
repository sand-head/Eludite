#!/usr/bin/env bash
# Brief 0037: the launch integration's Xvfb run (see xvfb-linux.sh). OUT_DIR/web/Web.slnx holds a copy of the web
# corpus project (corpus/web/minimal-api); Eludite opens it with the real eludite-host and is driven with real X input
# from xdotool:
#   1. F5 (under netcoredbg, when ELUDITE_NETCOREDBG names it) or else Ctrl+F5 (the real `dotnet`): the host builds the
#      project first, the program starts, Kestrel says "Now listening on:", and the page opens in the Web Browser
#      window as the session's tab (the debug glyph before its title)                     -> OUT_DIR/web-launch-page.png
#   2. Ctrl+Shift+F5: the session restarts (stopped and started again, built first) and the same tab reloads once the
#      server is back, the engine already running                                  -> OUT_DIR/web-launch-restarted.png
#   3. Shift+F5: the session ends and the tab stays                                       -> OUT_DIR/web-launch-stopped.png
# The trace lines (ELUDITE_TRACE_LSP=1) give the timings: the key to the page opened, Kestrel's line to the page opened
# (the readiness watch on the launch thread plus the tab through the bus), and the window's first page pixel; they go
# to OUT_DIR/web-launch.json with the load average.
#   crates/eludite/tools/web-launch-linux.sh OUT_DIR
# Needs: `cargo build -p eludite` and the engine built with CEF (CEF_PATH="$(tools/cef/fetch.sh)" cargo build -p
# eludite-chromium --features eludite-chromium/cef) beside eludite or at ELUDITE_CHROMIUM; eludite-host built
# (`dotnet build dotnet/Eludite.slnx`, or ELUDITE_HOST); the .NET SDK on PATH; on Debian and Ubuntu xvfb,
# mesa-vulkan-drivers, imagemagick, xdotool. As root (a container) the engine needs ELUDITE_CHROME_NO_SANDBOX=1, which
# this script never sets for you. PORT (default 5180) is the site's http port. Timings on software rendering are an
# upper bound, not the reference machine's.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
port=${PORT:-5180}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
[ -x "$host" ] || { echo "no eludite-host at $host (dotnet build dotnet/Eludite.slnx)" >&2; exit 1; }
engine=${ELUDITE_CHROMIUM:-$(dirname "$bin")/eludite-chromium}
[ -x "$engine" ] || { echo "no eludite-chromium at $engine (see the header of this script)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config" ELUDITE_HOST="$host" ELUDITE_TRACE_LSP=1
unset WAYLAND_DISPLAY
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
# The solution: a copy of the corpus project, its http profile on PORT.
web="$out/web"; rm -rf "$web" "$out/config"; mkdir -p "$web/MinimalApi" "$out/config"
cp -r "$repo/corpus/web/minimal-api/." "$web/MinimalApi/"
rm -rf "$web/MinimalApi/bin" "$web/MinimalApi/obj"
sed -i "s/localhost:5180/localhost:$port/g" "$web/MinimalApi/Properties/launchSettings.json"
echo '<Solution><Project Path="MinimalApi/MinimalApi.csproj" /></Solution>' >"$web/Web.slnx"
if [ -n "${ELUDITE_NETCOREDBG:-}" ]; then key=F5; else key=ctrl+F5; fi
pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
ms() { date +%s%3N; }
# The first trace line matching $1 at or after $2 (ms), waiting up to $3 seconds: prints its time.
wait_line() {
  local pattern=$1 after=$2 deadline=$(( $(date +%s) + $3 ))
  while [ "$(date +%s)" -le "$deadline" ]; do
    local t
    t=$(grep -a '^\[lsp\] ' "$out/eludite.err" | awk -v a="$after" -v p="$pattern" '$2 >= a && index($0, p) { print $2; exit }')
    [ -n "$t" ] && { echo "$t"; return 0; }
    sleep 0.2
  done
  return 1
}
shot() { import -window root "$out/$1.png"; echo "$out/$1.png" >&2; }

cut -d' ' -f1-3 /proc/loadavg >"$out/loadavg-before.txt"
"$bin" --no-persist --reset-layout --solution "$web/Web.slnx" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^Web - Eludite\$" | head -1)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-12}"
pressed=$(ms)
xdotool key "$key"
answered=$(wait_line "debug start: http://" "$pressed" 180 || true)
opened=$(wait_line "debug start: page opened" "$pressed" 60 || true)
sleep "${PAINT:-4}"
shot web-launch-page
# The line-to-page time of the last opening (none when the url answered before Kestrel's line was read).
opened_after() { grep -a "debug start: page opened" "$out/eludite.err" | tail -1 | sed -nE 's/.*opened ([0-9.]+) ms.*/\1/p'; }
line=$(opened_after || true)
pixel=$(grep -a "web browser: first page pixel" "$out/eludite.err" | tail -1 | sed -E 's/.*pixel ([0-9.]+) ms.*/\1/' || true)
built=$(grep -a "debug start: build succeeded" "$out/eludite.err" | tail -1 | awk '{print $2}' || true)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
restart_pressed=$(ms)
xdotool key ctrl+shift+F5
reopened=$(wait_line "debug start: page opened" "$restart_pressed" 180 || true)
sleep "${PAINT:-4}"
shot web-launch-restarted
restart_line=$(opened_after || true)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
xdotool key shift+F5
sleep 3
shot web-launch-stopped
cut -d' ' -f1-3 /proc/loadavg >"$out/loadavg-after.txt"
stop
since() { if [ -n "$1" ]; then echo $(( $1 - pressed )); else echo null; fi; }
cat >"$out/web-launch.json" <<JSON
{
  "key": "$key",
  "port": $port,
  "key_to_build_succeeded_ms": $(since "$built"),
  "key_to_server_answered_ms": $(since "$answered"),
  "key_to_page_opened_ms": $(since "$opened"),
  "listening_line_to_page_opened_ms": ${line:-null},
  "window_open_to_first_page_pixel_ms": ${pixel:-null},
  "restart_key_to_page_reloaded_ms": $(if [ -n "$reopened" ]; then echo $((reopened - restart_pressed)); else echo null; fi),
  "restart_listening_line_to_page_reloaded_ms": ${restart_line:-null},
  "loadavg_before": "$(cat "$out/loadavg-before.txt")",
  "loadavg_after": "$(cat "$out/loadavg-after.txt")"
}
JSON
cat "$out/web-launch.json"
