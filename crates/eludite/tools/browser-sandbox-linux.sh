#!/usr/bin/env bash
# Brief 0039: the sandbox opt-in's Xvfb run. Starts Eludite on a temporary workspace folder and drives it with real X
# input (tools/browser_sandbox.py, xdotool): View > Other Windows > Web Browser, the engine's refusal to start without
# Chromium's sandbox, the dialog "Chromium's sandbox cannot start on this machine" (browser-sandbox-dialog.png), "Run
# without the sandbox for this workspace" checked and OK, the engine started with --allow-no-sandbox and the strip
# "Browser running without Chromium's sandbox" (browser-sandbox-strip.png). Writes OUT_DIR/browser-sandbox.json, the
# screenshots and OUT_DIR/eludite.log.
#
#   crates/eludite/tools/browser-sandbox-linux.sh OUT_DIR
#
# The refusal happens only where the sandbox cannot start: as root (this container), or with neither unprivileged
# user namespaces nor the setuid chrome-sandbox. ELUDITE_CHROME_NO_SANDBOX is unset for the run (it would allow the
# engine to drop the sandbox without asking). Needs `cargo build -p eludite` and the engine built with CEF beside it
# (CEF_PATH="$(tools/cef/fetch.sh)" cargo build -p eludite-chromium --features eludite-chromium/cef) or at
# ELUDITE_CHROMIUM; on Debian and Ubuntu xvfb, mesa-vulkan-drivers, imagemagick, xdotool, python3.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
target_dir=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target_dir/debug/eludite}
[ -x "$bin" ] || { echo "no $bin (cargo build -p eludite)" >&2; exit 1; }
engine=${ELUDITE_CHROMIUM:-$(dirname "$bin")/eludite-chromium}
[ -x "$engine" ] || { echo "no eludite-chromium at $engine (see the header of this script)" >&2; exit 1; }
unset ELUDITE_CHROME_NO_SANDBOX
export DISPLAY=${DISPLAY:-:99}
unset WAYLAND_DISPLAY
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
ws="$out/workspace"; config="$out/config"
rm -rf "$ws" "$config"; mkdir -p "$ws" "$config"
echo "A workspace for the sandbox opt-in's run (brief 0039)." >"$ws/README.txt"
echo '{}' >"$config/settings.json"
ELUDITE_CONFIG_DIR="$config" "$bin" --no-persist --reset-layout --folder "$ws" --bounds-out "$out/bounds.json" \
  >"$out/eludite.out" 2>"$out/eludite.log" &
pid=$!
python3 "$here/browser_sandbox.py" --pid "$pid" --log "$out/eludite.log" --bounds "$out/bounds.json" \
  --shots "$out" --workspace "$ws" >"$out/browser-sandbox.json" 2>"$out/driver.err" || true
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
cat "$out/browser-sandbox.json"
