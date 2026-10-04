#!/usr/bin/env bash
# Brief 0048 manual run on an Xvfb screen (see xvfb-linux.sh): NuGet in the real eludite binary with the real
# eludite-host against the corpus's local feed (corpus/nuget), driven with real X input from xdotool against the
# element bounds `--bounds-out` reports, with screenshots. Offline: the corpus's NuGet.config lists only its feed.
#   The fixture (OUT_DIR/nuget) is a copy of corpus/nuget with its feed packed (corpus/nuget/build.sh) and restored.
#   1. Tools > NuGet Package Manager > Manage NuGet Packages for Solution..., "Corpus" typed in Browse's search box:
#      the results after the debounce                                                        -> OUT_DIR/browse.png
#   2. "Logging" searched, its row clicked: the detail pane with its versions and the projects -> OUT_DIR/details.png
#   3. Shared checked, Install: Logging into Shared, the project file edited and restored  -> OUT_DIR/installed.png
#   4. Ctrl+Alt+O: the Output window's Package Manager lines                                -> OUT_DIR/output.png
#   5. Ctrl+Alt+L, Shared, Dependencies and Packages expanded in the Workspace window, and App's Dependencies with
#      Packages, Projects and Frameworks                                                    -> OUT_DIR/dependencies.png
# Usage: crates/eludite/tools/nuget-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool jq; the .NET SDK; `cargo build -p eludite` (or ELUDITE_BIN);
# eludite-host built (`dotnet build dotnet/Eludite.slnx`, or ELUDITE_HOST).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config" ELUDITE_HOST="$host" ELUDITE_CACHE_DIR="$out/cache"
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi

# The fixture: the corpus without build output, its feed packed, restored.
work="$out/nuget"
rm -rf "$work"
mkdir -p "$work"
(cd "$repo/corpus/nuget" && tar --exclude=.packages --exclude=bin --exclude=obj --exclude=feed -cf - .) | tar -xf - -C "$work"
bash "$repo/corpus/nuget/build.sh" "$work" >"$out/pack.log"
dotnet restore "$work/Corpus.slnx" >"$out/restore.log"
cp "$work/Shared/Shared.csproj" "$out/Shared.csproj.before"

pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
"$bin" --no-persist --reset-layout --solution "$work/Corpus.slnx" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^Corpus - Eludite\$" | head -1)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-10}"

keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }
# The screen position of probed element $1's center (waits for it to be drawn).
point() {
  local key=$1 b=""
  for _ in $(seq 1 50); do
    xdotool mousemove --window "$wid" 5 5 >/dev/null 2>&1 || true
    b=$(jq -r --arg k "$key" '.[$k] // empty | @tsv' "$out/bounds.json" 2>/dev/null || true)
    [ -n "$b" ] && break
    sleep 0.2
  done
  [ -n "$b" ] || { echo "no element $key on screen" >&2; return 1; }
  read -r x y w h <<<"$b"
  eval "$(xdotool getwindowgeometry --shell "$wid")"
  echo "$(( X + ${x%.*} + ${w%.*} / 2 )) $(( Y + ${y%.*} + ${h%.*} / 2 ))"
}
click() { read -r px py <<<"$(point "$1")"; xdotool mousemove "$px" "$py" click 1; sleep 0.6; }
dclick() { read -r px py <<<"$(point "$1")"; xdotool mousemove "$px" "$py" click --repeat 2 --delay 80 1; sleep 1; }

# 1. The window for the solution, Browse, a search.
click menu-Tools
click "menu-item-Tools-NuGet Package Manager > Manage NuGet Packages for Solution..."
sleep 2
click nuget-search
xdotool type --delay 60 "Corpus"
sleep 3
shot browse
# 2. Logging's details: its own search, its row, the detail pane.
click nuget-search
for _ in 1 2 3 4 5 6; do xdotool key BackSpace; done
xdotool type --delay 60 "Logging"
sleep 3
click nuget-row-0
sleep 1
shot details
# 3. Shared checked (a package not installed starts with none), Install.
click nuget-project-Shared
sleep 0.5
click nuget-install
sleep "${INSTALL_WAIT:-20}"
shot installed
diff -u "$out/Shared.csproj.before" "$work/Shared/Shared.csproj" >"$out/Shared.csproj.diff" || true
# 4. The Output window's Package Manager pane.
keys ctrl+alt+o
sleep 1
shot output
# 5. The Dependencies node.
keys ctrl+alt+l
sleep 1
shared="$work/Shared/Shared.csproj"
app="$work/App/App.csproj"
dclick "se-$shared"
dclick "se-$shared|deps"
dclick "se-$shared|deps|packages"
dclick "se-$app"
dclick "se-$app|deps"
dclick "se-$app|deps|packages"
dclick "se-$app|deps|packages|Eludite.Corpus.Greeter"
dclick "se-$app|deps|projects"
dclick "se-$app|deps|frameworks"
sleep 1
shot dependencies
stop
ls "$out"
