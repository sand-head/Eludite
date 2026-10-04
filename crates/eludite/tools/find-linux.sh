#!/usr/bin/env bash
# Brief 0042 manual run on an Xvfb screen (see xvfb-linux.sh): Find in Files and Replace in Files in the real eludite
# binary on this repository, driven with real X keys from xdotool, with screenshots.
#   1. Ctrl+Shift+F: the Find in Files dialog                                          -> OUT_DIR/find-dialog.png
#   2. `fn main`, Enter: Find Results 1 at the bottom, grouped by file, the count line  -> OUT_DIR/find-results.png
#   3. F8: the editor at the first match, the match selected                            -> OUT_DIR/find-f8.png
#   4. Ctrl+Shift+H: Replace `fn main` with `fn main_renamed`, Alt+A (Replace All) with preview: the pending changes
#      in the review view; nothing is accepted, nothing is written                      -> OUT_DIR/replace-preview.png
# The repository is left untouched (checked with `git status --porcelain` before and after).
# Usage: crates/eludite/tools/find-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool; `cargo build -p eludite` (or ELUDITE_BIN).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
[ -x "$bin" ] || { echo "no $bin (cargo build -p eludite)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99}
unset WAYLAND_DISPLAY
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
before=$(git -C "$repo" status --porcelain)
config="$out/config"; rm -rf "$config"; mkdir -p "$config"
echo '{}' >"$config/settings.json"

pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
ELUDITE_CONFIG_DIR="$config" "$bin" --no-persist --reset-layout --folder "$repo" \
  >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name " - Eludite\$" | head -1)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-6}"

keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }

# 1. The dialog.
keys ctrl+shift+f
sleep 1
shot find-dialog
# 2. Find All over the workspace.
xdotool type --delay 40 "fn main"
keys Return
sleep "${SEARCH_WAIT:-4}"
shot find-results
# 3. F8: the first match in the editor.
keys Escape
keys F8
sleep 2
shot find-f8
# 4. Replace in Files with preview: pending changes only.
keys ctrl+shift+h
sleep 1
keys ctrl+a
xdotool type --delay 40 "fn main"
keys Tab
xdotool type --delay 40 "fn main_renamed"
keys alt+a
sleep "${SEARCH_WAIT:-4}"
shot replace-preview
stop
after=$(git -C "$repo" status --porcelain)
if [ "$before" != "$after" ]; then
  echo "the repository changed during the run" >&2
  exit 1
fi
echo "The repository is unchanged (nothing was accepted)." | tee "$out/unchanged.txt"
ls "$out"
