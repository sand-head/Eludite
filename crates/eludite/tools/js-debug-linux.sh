#!/usr/bin/env bash
# Brief 0038: JavaScript debugging's Xvfb run (see xvfb-linux.sh). OUT_DIR/web/Web.slnx holds a copy of the web corpus
# project (corpus/web/minimal-api, its wwwroot/app.ts compiled with its map); Eludite opens it with the real
# eludite-host and wwwroot/app.ts in the editor, and is driven with real X input from xdotool:
#   1. Ctrl+F on the Add handler's `const sum = total(cart);` and F9: a breakpoint in app.ts.
#   2. F5 (under netcoredbg, when ELUDITE_NETCOREDBG names it: the setting debugger.attachBrowser, on by default,
#      attaches vscode-js-debug to the page once it is up, one compound) or else Ctrl+F5 (the real `dotnet`) and then
#      Debug > Attach to Browser Tab... (clicked) and Enter on the tab it selects. The page opens in the Web Browser
#      window and the browser session's child session binds the breakpoint through app.js.map.
#   3. A click on the page's Add button, with the pointer (the button's place read from the engine's DevTools port on
#      loopback): the handler stops at app.ts line 25; the Call Stack shows the browser session and its page's child
#      session in its selector, the Locals the handler's variables                         -> OUT_DIR/js-debug-stopped.png
#   4. F10, then Shift+F5: the sessions end, the page keeps running                         -> OUT_DIR/js-debug-ended.png
# The trace lines (ELUDITE_TRACE_LSP=1) give the timings, written to OUT_DIR/js-debug.json with the load average: the
# start key to the page opened, the attach (Enter in the dialog, or the page opened under F5) to the child session,
# and the click to the stop with its Locals.
#   crates/eludite/tools/js-debug-linux.sh OUT_DIR
# Needs: `cargo build -p eludite` and the engine built with CEF (CEF_PATH="$(tools/cef/fetch.sh)" cargo build -p
# eludite-chromium --features eludite-chromium/cef) beside eludite or at ELUDITE_CHROMIUM; eludite-host built
# (`dotnet build dotnet/Eludite.slnx`, or ELUDITE_HOST); the .NET SDK on PATH; vscode-js-debug
# (ELUDITE_JS_DEBUG="$(tools/js-debug/fetch.sh)", or that script's cache) and Node.js 18 or later on PATH (node also
# reads the button's place: Node 22's WebSocket); on Debian and Ubuntu xvfb, mesa-vulkan-drivers, imagemagick,
# xdotool, lsof. As root (a container) the engine needs ELUDITE_CHROME_NO_SANDBOX=1, which this script never sets for
# you. PORT (default 5180) is the site's http port. Timings on software rendering are an upper bound, not the
# reference machine's.
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
command -v node >/dev/null || { echo "no node on PATH (Node.js 18 or later)" >&2; exit 1; }
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
    sleep 0.1
  done
  return 1
}
shot() { import -window root "$out/$1.png"; echo "$out/$1.png" >&2; }
# A bounds file entry (x y w h, window-relative) by key, waiting up to $2 seconds.
rect() {
  local deadline=$(( $(date +%s) + $2 ))
  while [ "$(date +%s)" -le "$deadline" ]; do
    local r
    r=$(node -e '
      const b = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"))[process.argv[2]];
      if (b) console.log(b.map(Math.round).join(" "));' "$out/bounds.json" "$1" 2>/dev/null || true)
    [ -n "$r" ] && { echo "$r"; return 0; }
    sleep 0.2
  done
  return 1
}
# Click the middle of window-relative rect "x y w h".
click_rect() {
  local x y w h ox oy
  read -r x y w h <<<"$1"
  eval "$(xdotool getwindowgeometry --shell "$wid")"
  ox=$X; oy=$Y
  xdotool mousemove $((ox + x + w / 2)) $((oy + y + h / 2)) click 1
}

cut -d' ' -f1-3 /proc/loadavg >"$out/loadavg-before.txt"
"$bin" --no-persist --reset-layout --solution "$web/Web.slnx" --open-file "$web/MinimalApi/wwwroot/app.ts" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^Web - Eludite\$" | head -1)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-12}"
# 1. The breakpoint on the handler's line.
xdotool key ctrl+Home ctrl+f
sleep 0.3
xdotool type --delay 30 "const sum = total(cart);"
xdotool key Return Escape Left
sleep 0.3
xdotool key F9
sleep 1
# 2. The start, and the page's debugging.
pressed=$(ms)
xdotool key "$key"
opened=$(wait_line "debug start: page opened" "$pressed" 180 || true)
attach_from=$opened
if [ "$key" != F5 ]; then
  sleep 2
  xdotool windowfocus --sync "$wid" 2>/dev/null || true
  click_rect "$(rect menu-Debug 30)"
  click_rect "$(rect "menu-item-Debug-Attach to Browser Tab..." 10)"
  sleep 1
  attach_from=$(ms)
  xdotool key Return
fi
child=$(wait_line "debug child session" "${attach_from:-$pressed}" 60 || true)
bound=$(wait_line "verified=true" "${attach_from:-$pressed}" 60 || true)
sleep 1
# 3. The click on Add: its middle in CSS pixels, from the engine's DevTools port (the browser process's loopback
# listener), plus the page's place in the window.
devtools=
for p in $(pgrep -f "eludite-chromium" || true); do
  grep -qa -- "--type=" "/proc/$p/cmdline" 2>/dev/null && continue
  for port_ in $(lsof -a -p "$p" -iTCP -sTCP:LISTEN -nP -Fn 2>/dev/null | sed -n 's/^n127\.0\.0\.1://p'); do
    devtools=$port_
  done
done
[ -n "$devtools" ] || { echo "no DevTools port of the engine" >&2; exit 1; }
button=$(node - "$devtools" ":$port/" <<'JS'
const [port, want] = process.argv.slice(2);
(async () => {
  const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const page = list.find((t) => t.type === "page" && t.url.includes(want));
  if (!page) throw new Error(`no page ${want} in ${JSON.stringify(list)}`);
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: {
    expression: "JSON.stringify(document.getElementById('add').getBoundingClientRect())", returnByValue: true } }));
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id !== 1) return;
    const r = JSON.parse(m.result.result.value);
    console.log(Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2));
    ws.close();
  };
})().catch((e) => { console.error(e.message); process.exit(1); });
JS
)
read -r bx by <<<"$button"
read -r px py _ _ <<<"$(rect web-browser-page 10)"
clicked=$(ms)
click_rect "$((px + bx)) $((py + by)) 0 0"
stopped=$(wait_line "debug break stop" "$clicked" 30 || true)
locals=$(wait_line "debug locals stop" "$clicked" 30 || true)
sleep "${PAINT:-3}"
shot js-debug-stopped
# 4. F10, then Shift+F5.
xdotool windowfocus --sync "$wid" 2>/dev/null || true
xdotool key F10
sleep 2
xdotool key shift+F5
ended=$(wait_line "debug ended" "$(ms)" 30 || true)
sleep 3
shot js-debug-ended
cut -d' ' -f1-3 /proc/loadavg >"$out/loadavg-after.txt"
stop
diff_ms() { if [ -n "$1" ] && [ -n "$2" ]; then echo $(( $2 - $1 )); else echo null; fi; }
cat >"$out/js-debug.json" <<JSON
{
  "key": "$key",
  "port": $port,
  "key_to_page_opened_ms": $(diff_ms "$pressed" "$opened"),
  "attach_to_child_session_ms": $(diff_ms "$attach_from" "$child"),
  "attach_to_breakpoint_bound_ms": $(diff_ms "$attach_from" "$bound"),
  "click_to_stop_ms": $(diff_ms "$clicked" "$stopped"),
  "click_to_locals_ms": $(diff_ms "$clicked" "$locals"),
  "sessions_ended": $([ -n "$ended" ] && echo true || echo false),
  "loadavg_before": "$(cat "$out/loadavg-before.txt")",
  "loadavg_after": "$(cat "$out/loadavg-after.txt")"
}
JSON
cat "$out/js-debug.json"
