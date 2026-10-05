#!/usr/bin/env bash
# Brief 0061 manual run on an Xvfb screen (see xvfb-linux.sh): the Workspace window's Visual Studio look, in the real
# eludite binary, driven with real X input from xdotool against the element bounds `--bounds-out` reports.
#   For each theme (dark, light, blue) eludite opens dotnet/ (the folder of dotnet/Eludite.slnx) as the workspace,
#   waits for the tree, double-clicks the rows `debuggers`, `src` and `tests` that the tree has (plain folders of the
#   listing without eludite-host; with the host the solution's projects are flat under the solution, so it expands
#   Eludite.Host and Eludite.Host.Tests instead) and takes      -> OUT_DIR/linux-workspace-vs-look-<theme>.png
#   In the dark theme it then presses Ctrl+; and types `host rpc` -> OUT_DIR/linux-workspace-search.png
# Usage: crates/eludite/tools/workspace-look-linux.sh OUT_DIR [THEMES...]   (THEMES default: dark light blue)
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool jq; `cargo build -p eludite` (or ELUDITE_BIN). The .NET host
# (ELUDITE_HOST, or `dotnet build dotnet/Eludite.slnx`) is optional: without it the solution node reads "(load failed)"
# and the folder listing shows src/ and tests/ with their files.
set -euo pipefail
out=$(realpath -m "$1"); shift; mkdir -p "$out"
themes=("$@"); [ ${#themes[@]} -eq 0 ] && themes=(dark light blue)
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config"
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
folder="$repo/dotnet"

pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }
# The probed key of the Workspace row whose id ends with $1 (empty when the tree has none).
row_key() {
  jq -r --arg s "$1" 'keys[] | select(startswith("se-") and endswith($s))' "$out/bounds.json" 2>/dev/null | head -1
}
# The screen position of probed element $1's center.
point() {
  local b
  b=$(jq -r --arg k "$1" '.[$k] // empty | @tsv' "$out/bounds.json")
  read -r x y w h <<<"$b"
  eval "$(xdotool getwindowgeometry --shell "$wid")"
  echo "$(( X + ${x%.*} + ${w%.*} / 2 )) $(( Y + ${y%.*} + ${h%.*} / 2 ))"
}
dclick() { read -r px py <<<"$(point "$1")"; xdotool mousemove "$px" "$py" click --repeat 2 --delay 80 1; sleep 1; }
# Expand the row whose id ends with $1, if the tree shows it.
expand() {
  local k=""
  for _ in $(seq 1 25); do
    xdotool mousemove --window "$wid" 5 5 >/dev/null 2>&1 || true
    k=$(row_key "$1"); [ -n "$k" ] && break
    sleep 0.2
  done
  if [ -n "$k" ]; then dclick "$k"; else echo "no row ending with $1 (skipped)"; fi
}

for theme in "${themes[@]}"; do
  rm -f "$out/bounds.json"
  "$bin" --no-persist --reset-layout --theme "$theme" --folder "$folder" \
    --bounds-out "$out/bounds.json" >"$out/eludite-$theme.out" 2>"$out/eludite-$theme.err" &
  pid=$!
  wid=$(timeout 60 xdotool search --sync --name "^dotnet - Eludite\$" | head -1)
  xdotool windowfocus --sync "$wid" 2>/dev/null || true
  # The listing, the solution (or its failure without the host) and the first frame of the tree.
  sleep "${SETTLE:-15}"
  for name in "|debuggers/" "|src/" "|tests/" "Eludite.Host/Eludite.Host.csproj" \
    "Eludite.Host.Tests/Eludite.Host.Tests.csproj"; do
    expand "$name"
  done
  xdotool mousemove --window "$wid" 5 5
  sleep 1
  shot "linux-workspace-vs-look-$theme"
  if [ "$theme" = dark ]; then
    keys ctrl+semicolon
    xdotool type --delay 60 "host rpc"
    sleep 1.5
    shot linux-workspace-search
  fi
  stop
done
ls "$out"
