#!/usr/bin/env bash
# Brief 0064 manual run on an Xvfb screen (see xvfb-linux.sh): the .resx editor in the real eludite binary against
# the real eludite-host, on a copy of corpus/resx (OUT_DIR/resx), driven with real X input from xdotool against the
# element bounds `--bounds-out` reports, with screenshots.
#   1. Corpus.slnx open, Resources.de.resx opened: the set's grid (neutral, de, fr-FR), the
#      missing French Save, the warning on the French Hello, the invariant Brand       -> OUT_DIR/screenshots/grid.png
#   2. The French Save selected and typed over (dirty tab)                                      -> OUT_DIR/screenshots/edited.png
#   3. Ctrl+S: only that element written (OUT_DIR/Resources.fr-FR.resx.diff)          -> OUT_DIR/screenshots/saved.png
#   4. Add Key "Welcome", saved: the host regenerated the designer
#      (OUT_DIR/Resources.Designer.cs.diff)                                           -> OUT_DIR/screenshots/added.png
#   5. The Missing filter                                                              -> OUT_DIR/screenshots/missing.png
#   6. Access Modifier: Public, the project edited and the designer regenerated
#      (OUT_DIR/Strings.csproj.diff)                                                   -> OUT_DIR/screenshots/public.png
# Usage: crates/eludite/tools/resx-linux.sh OUT_DIR   (the committed run keeps screenshots/ and the diffs; the copies,
# the config, bounds.json and the logs under OUT_DIR are deleted by hand)
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool jq; the .NET SDK on PATH (DOTNET_ROOT set);
# `cargo build -p eludite` and `dotnet build dotnet/Eludite.slnx` (or ELUDITE_BIN and ELUDITE_HOST).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
target=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target/debug/eludite}
export ELUDITE_HOST=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
[ -x "$ELUDITE_HOST" ] || { echo "no eludite-host at $ELUDITE_HOST (dotnet build dotnet/Eludite.slnx)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config"
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
rm -rf "$out/config" "$out/resx" "$out/original" "$out/bounds.json" "$out/screenshots" "$out"/*.diff
mkdir -p "$out/config" "$out/screenshots"
cp -r "$repo/corpus/resx" "$out/resx"
cp -r "$repo/corpus/resx" "$out/original"
work="$out/resx"

pid=
stop() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  pid=
  return 0
}
trap stop EXIT
"$bin" --no-persist --reset-layout --solution "$work/Corpus.slnx" \
  --open-file "$work/Strings/Properties/Resources.de.resx" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^Corpus - Eludite\$" | head -1)
xdotool windowsize --sync "$wid" 1580 980 2>/dev/null || true
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-25}"

keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
# A pointer move first: a frame follows it, so the screenshot shows the latest state.
shot() { xdotool mousemove --window "$wid" $((900 + RANDOM % 40)) 900 >/dev/null 2>&1 || true; sleep 0.6; import -window root "$out/screenshots/$1.png"; echo "$out/screenshots/$1.png"; }
# The screen position of probed element $1's center (waits for it to be drawn).
point() {
  local key=$1 b=""
  for _ in $(seq 1 100); do
    xdotool mousemove --window "$wid" $((900 + RANDOM % 40)) 900 >/dev/null 2>&1 || true
    b=$(jq -r --arg k "$key" '.[$k] // empty | @tsv' "$out/bounds.json" 2>/dev/null || true)
    [ -n "$b" ] && break
    sleep 0.2
  done
  [ -n "$b" ] || { echo "no element $key on screen" >&2; return 1; }
  read -r x y w h <<<"$b"
  eval "$(xdotool getwindowgeometry --shell "$wid")"
  echo "$(( X + ${x%.*} + ${w%.*} / 2 )) $(( Y + ${y%.*} + ${h%.*} / 2 ))"
}
click() { local p; p=$(point "$1") || exit 1; read -r px py <<<"$p"; xdotool mousemove "$px" "$py" click 1; sleep 1; }
dblclick() { local p; p=$(point "$1") || exit 1; read -r px py <<<"$p"; xdotool mousemove "$px" "$py" click --repeat 2 --delay 80 1; sleep 1; }
diffs() { (cd "$out" && diff -u "original/$1" "resx/$1" || true) >"$out/$2"; }

# 1. The set's grid.
point resx-add-key >/dev/null
point resx-cell-0-neutral >/dev/null
sleep 2
shot grid
# 2. The French Save: a click selects the cell, typing replaces it.
click resx-cell-2-fr-FR
xdotool type --delay 40 -- "Enregistrer ?"
sleep 1
shot edited
keys Return
sleep 1
# 3. Save: one element written.
keys ctrl+s
sleep 4
shot saved
diffs Strings/Properties/Resources.fr-FR.resx Resources.fr-FR.resx.diff
# 4. Add Key, saved: the designer regenerated by the host.
click resx-add-key
xdotool type --delay 40 -- "Welcome"
keys Return
sleep 1
keys ctrl+s
sleep 6
shot added
diffs Strings/Properties/Resources.resx Resources.resx.diff
diffs Strings/Properties/Resources.Designer.cs Resources.Designer.cs.diff
# 5. The Missing filter.
click resx-filter-missing
sleep 1
shot missing
click resx-filter-missing
# 6. Access Modifier: Public.
click resx-modifier-public
sleep 8
shot public
diffs Strings/Strings.csproj Strings.csproj.diff
diffs Strings/Properties/Resources.Designer.cs Resources.Designer.public.diff
echo done
