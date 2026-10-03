#!/usr/bin/env bash
# Brief 0032: the Web Browser window's Xvfb run. Starts Eludite on a temporary workspace folder with the scripted fake
# agent (brief 0024's `browser-form` scenario) as the agent "Form Filler", and drives it with real X input
# (tools/browser_window.py, xdotool): View > Other Windows > Web Browser, a local page typed into the address bar, a
# field typed into by hand, F12 (DevTools as a tab), a JavaScript confirm() answered in the shell's dialog, then the
# agent filling the order form while the "Agent is driving" strip shows; then the window closed and reopened with the
# engine running. Screenshots and the measurements (window open to the first page pixel, cold and with the engine
# running; the shell's resident memory before and with the window open) go to OUT_DIR.
#
#   crates/eludite/tools/browser-window-linux.sh OUT_DIR
#     writes OUT_DIR/browser-window.json (what the driver saw and measured), OUT_DIR/*.png and OUT_DIR/eludite.log
#
# Needs: `cargo build -p eludite -p eludite-acp` and the engine built with CEF (CEF_PATH="$(tools/cef/fetch.sh)" cargo
# build -p eludite-chromium --features eludite-chromium/cef) beside eludite or at ELUDITE_CHROMIUM; on Debian and
# Ubuntu xvfb, mesa-vulkan-drivers, imagemagick, xdotool, python3. Running as root (a container) needs
# ELUDITE_CHROME_NO_SANDBOX=1, which this script never sets for you. PORT (default 4327) is the fixture server's.
# Here (no GPU) Mesa's software Vulkan (lavapipe) draws the window: timings are an upper bound, not the reference
# machine's.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
target_dir=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target_dir/debug/eludite}
fake=${ELUDITE_FAKE_AGENT:-$(dirname "$bin")/eludite-fake-acp-agent}
port=${PORT:-4327}
for f in "$bin" "$fake"; do
  [ -x "$f" ] || { echo "no $f (cargo build -p eludite -p eludite-acp)" >&2; exit 1; }
done
engine=${ELUDITE_CHROMIUM:-$(dirname "$bin")/eludite-chromium}
[ -x "$engine" ] || { echo "no eludite-chromium at $engine (see the header of this script)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99}
unset WAYLAND_DISPLAY
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
ws="$out/workspace"; config="$out/config"
rm -rf "$ws" "$config"; mkdir -p "$ws" "$config"
echo "A workspace for the Web Browser window's run (brief 0032)." >"$ws/README.txt"
cat >"$config/settings.json" <<JSON
{
  "agents.custom": [
    {"name": "Form Filler", "command": "$fake",
     "args": ["--scenario", "browser-form", "--url", "http://127.0.0.1:$port/form.html"]}
  ],
  "agents.default": "Form Filler"
}
JSON
cut -d' ' -f1-3 /proc/loadavg >"$out/loadavg-before.txt"
ELUDITE_CONFIG_DIR="$config" "$bin" --no-persist --reset-layout --folder "$ws" --agent "Form Filler" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.log" &
pid=$!
python3 "$here/browser_window.py" --pid "$pid" --log "$out/eludite.log" --bounds "$out/bounds.json" \
  --shots "$out" --port "$port" --fixtures "$repo/crates/browser/tests/fixtures" >"$out/browser-window.json" \
  2>"$out/driver.err" || true
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
cut -d' ' -f1-3 /proc/loadavg >"$out/loadavg-after.txt"
cat "$out/browser-window.json"
