#!/usr/bin/env bash
# Run Eludite on a machine with no display: an Xvfb screen with Mesa's software Vulkan (lavapipe) draws the GPUI window,
# `import` (ImageMagick) takes screenshots and xdotool sends real X input. Verified on 2026-10-03 (Ubuntu 24.04 in a
# container): the shell started, drew the Welcome tab, the Workspace window and the status bar, and the screenshot
# below showed it. Timings on software rendering are not the reference machine's; use this for proving flows and
# screenshots, not for the frame-cost budgets.
#   crates/eludite/tools/xvfb-linux.sh OUT_DIR [eludite args...]
#     starts Xvfb on DISPLAY :99 (1600x1000) unless one runs, starts eludite with the args (default: --folder <repo>),
#     waits SETTLE seconds (default 12), writes OUT_DIR/shell.png and OUT_DIR/eludite.log, and stops eludite.
#   KEEP=1 leaves eludite running (print its pid) so a driver (xdotool, the tools/*.py XTest drivers with DISPLAY=:99)
#   can act on it; stop it yourself.
# Needs, on Debian and Ubuntu: xvfb mesa-vulkan-drivers imagemagick xdotool.
set -euo pipefail
out=$(realpath -m "$1"); shift || true; mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99}
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
args=("$@"); [ ${#args[@]} -eq 0 ] && args=(--folder "$repo")
"$bin" --no-persist "${args[@]}" >"$out/eludite.log" 2>&1 &
pid=$!
sleep "${SETTLE:-12}"
import -window root "$out/shell.png"
echo "$out/shell.png"
if [ "${KEEP:-0}" = "1" ]; then echo "eludite pid $pid"; else kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
