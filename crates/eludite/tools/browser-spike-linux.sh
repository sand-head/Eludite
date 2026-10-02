#!/usr/bin/env bash
# Brief 0031: the CEF offscreen spike's benchmark. Runs `eludite --bench-browser SECONDS` (the animation page in a
# 1600x1000 tab of eludite-chromium, drawn by the shell with GPUI's img element) and takes a screenshot while the
# animation plays.
#
#   crates/eludite/tools/browser-spike-linux.sh OUT_DIR [SECONDS]      (SECONDS defaults to 20)
#     writes OUT_DIR/browser-spike.json (the bench's JSON line), OUT_DIR/browser-spike.png and OUT_DIR/eludite.log
#
# Here (no GPU, no display): an Xvfb screen on DISPLAY :98 (2400x1600, so the 2200x1500 window and the whole tab fit)
# with Mesa's software Vulkan (lavapipe) draws the window; frame costs are an upper bound, not the reference machine's.
# On a machine with a GPU: NATIVE=1 uses the session's display instead (X11 for the screenshot; on Wayland the
# screenshot is skipped), with a release build: `cargo build --release -p eludite` and
# `cargo build --release -p eludite-chromium --features eludite-chromium/cef`, then ELUDITE_BIN=target/release/eludite.
#
# Needs: the engine built with CEF (tools/cef/fetch.sh, then CEF_PATH="$(tools/cef/fetch.sh)" cargo build -p
# eludite-chromium --features eludite-chromium/cef) beside eludite or at ELUDITE_CHROMIUM; on Debian and Ubuntu xvfb,
# mesa-vulkan-drivers, imagemagick. Running as root (a container) needs ELUDITE_CHROME_NO_SANDBOX=1, which this script
# never sets for you. SHOT_AT (default 9) is when the screenshot is taken, in seconds after launch.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
secs=${2:-20}
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
engine=${ELUDITE_CHROMIUM:-$(dirname "$bin")/eludite-chromium}
[ -x "$engine" ] || { echo "no eludite-chromium at $engine (see the header of this script)" >&2; exit 1; }
if [ "${NATIVE:-0}" != "1" ]; then
  export DISPLAY=:98
  unset WAYLAND_DISPLAY
  if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
    Xvfb "$DISPLAY" -screen 0 2400x1600x24 >"$out/xvfb.log" 2>&1 &
    for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
  fi
fi
cut -d' ' -f1-3 /proc/loadavg >"$out/loadavg-before.txt"
"$bin" --no-persist --bench-browser "$secs" >"$out/browser-spike.json" 2>"$out/eludite.log" &
pid=$!
sleep "${SHOT_AT:-9}"
if [ -n "${DISPLAY:-}" ] && command -v import >/dev/null; then
  import -window root "$out/browser-spike.png" || true
fi
wait "$pid"
cat "$out/browser-spike.json"
