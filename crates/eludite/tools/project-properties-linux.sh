#!/usr/bin/env bash
# Brief 0049 manual run on an Xvfb screen (see xvfb-linux.sh): the project property pages, a property edit, a launch
# profile edit, the toolbar's configuration list and Configuration Manager, in the real eludite binary against the
# real eludite-host, on a copy of corpus/projects (OUT_DIR/projects), driven with real X input from xdotool against
# the element bounds `--bounds-out` reports, with screenshots.
#   1. Corpus.slnx open, Console/Program.cs active: the toolbar's lists                 -> OUT_DIR/solution.png
#   2. Project > Properties: Console's Application page                                  -> OUT_DIR/pages-application.png
#   3. The Build page, Debug in the Configuration list; Treat warnings as errors
#      checked (dirty)                                                                    -> OUT_DIR/pages-build-dirty.png
#   4. Save: the one conditioned element written (OUT_DIR/Console.csproj.diff)           -> OUT_DIR/pages-build-saved.png
#   5. The Debug page: Console's profile, " --trace" appended to its arguments, applied
#      (OUT_DIR/launchSettings.json.diff)                                                 -> OUT_DIR/pages-debug.png
#   6. The toolbar's Solution Configurations list open                                   -> OUT_DIR/configuration-list.png
#   7. Release chosen: the list shows it                                                  -> OUT_DIR/configuration-release.png
#   8. Configuration Manager... from the list: Lib not built in Release                  -> OUT_DIR/configuration-manager.png
# Usage: crates/eludite/tools/project-properties-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool jq git; the .NET SDK on PATH (DOTNET_ROOT set);
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
rm -rf "$out/config" "$out/projects" "$out/original" "$out/bounds.json" "$out"/*.png "$out"/*.diff
mkdir -p "$out/config"
cp -r "$repo/corpus/projects" "$out/projects"
cp -r "$repo/corpus/projects" "$out/original"
work="$out/projects"

pid=
stop() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  pid=
  return 0
}
trap stop EXIT
"$bin" --no-persist --reset-layout --solution "$work/Corpus.slnx" --open-file "$work/Console/Program.cs" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^Corpus - Eludite\$" | head -1)
xdotool windowsize --sync "$wid" 1580 980 2>/dev/null || true
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-25}"

keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }
# The screen position of probed element $1's center (waits for it to be drawn).
point() {
  local key=$1 b=""
  for _ in $(seq 1 100); do
    # Move the pointer (a frame follows) away from the menu bar: hovering a title switches menus.
    xdotool mousemove --window "$wid" $((900 + RANDOM % 40)) 700 >/dev/null 2>&1 || true
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
has() { jq -e --arg k "$1" '.[$k]' "$out/bounds.json" >/dev/null 2>&1; }
# Open menu $1 (again if its items did not draw) and click item $2.
menu() {
  local tries
  for tries in 1 2 3; do
    click "menu-$1"
    for _ in $(seq 1 10); do has "menu-item-$1-$2" && break 2; sleep 0.2; done
  done
  click "menu-item-$1-$2"
}
diffs() { (cd "$out" && diff -u "original/$1" "projects/$1" || true) >"$out/$2"; }

# 1. The solution loaded, the toolbar's lists drawn.
point build-configuration >/dev/null
shot solution
# 2. Project > Properties: the startup project's pages.
menu Project Properties
point pp-prop-AssemblyName >/dev/null
sleep 1
shot pages-application
# 3. The Build page in Debug, a per-configuration check box.
click pp-page-build
click pp-configuration-0
sleep 1
click pp-prop-TreatWarningsAsErrors
shot pages-build-dirty
# 4. Save: one setProperty, the solution reloaded.
click pp-save
sleep 4
shot pages-build-saved
diffs Console/Console.csproj Console.csproj.diff
# 5. The Debug page: the launch profile's arguments.
click pp-page-debug
click pp-profile-0
click pp-profile-field-command_line_args
xdotool type --delay 40 -- " --trace"
keys Return
sleep 2
shot pages-debug
diffs Console/Properties/launchSettings.json launchSettings.json.diff
# 6. The Solution Configurations list.
click build-configuration
point build-configuration-0 >/dev/null
shot configuration-list
# 7. Release.
click build-configuration-1
sleep 1
shot configuration-release
# 8. Configuration Manager... (the list's last entry, after Debug and Release).
click build-configuration
click build-configuration-2
point cm-close >/dev/null
sleep 1
shot configuration-manager
click cm-close
cp "$out/bounds.json" "$out/bounds-last.json" 2>/dev/null || true
echo "done: $out"
