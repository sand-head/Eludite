#!/usr/bin/env bash
# Brief 0040 manual run on an Xvfb screen (see xvfb-linux.sh): Git in the real eludite binary, driven with real X input
# from xdotool against the element bounds `--bounds-out` reports, with screenshots.
#   The fixture (OUT_DIR/repo) is a repository the harness makes with git (Eludite starts no git process): a Rust file
#   committed on main, a feature branch merged back with --no-ff (so the graph has a merge), then a change to
#   src/lib.rs and an untracked notes.md. Eludite opens the folder with src/lib.rs.
#   1. A line typed at the end of src/lib.rs and Ctrl+S: the Workspace window's glyphs (modified, untracked), the
#      tab's glyph, the change margin and the status bar's branch and pending count      -> OUT_DIR/changes.png
#   2. Ctrl+0, Ctrl+G: the Git Changes window with the Changes group                     -> OUT_DIR/git-changes.png
#   3. A double-click on src/lib.rs there: Compare with Unmodified, F8 to the change     -> OUT_DIR/compare.png
#   4. A message typed in the box and Commit All: committed, the box and Changes empty  -> OUT_DIR/committed.png
#   5. Ctrl+0, Ctrl+R: the Git Repository window, its branches and the graph            -> OUT_DIR/repository.png
# Usage: crates/eludite/tools/git-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool jq git; `cargo build -p eludite` (or ELUDITE_BIN).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config"
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi

# The fixture, with its identity in its own config (never the machine's global one).
work="$out/repo"
rm -rf "$work"
mkdir -p "$work/src"
g() { git -C "$work" -c user.name="Eludite Run" -c user.email=run@example.com "$@"; }
git init -q -b main "$work"
g config user.name "Eludite Run"
g config user.email run@example.com
cat >"$work/src/lib.rs" <<'RS'
/// Adds two numbers.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// Subtracts one number from another.
pub fn sub(a: i32, b: i32) -> i32 {
    a - b
}
RS
printf '[package]\nname = "calc"\nversion = "0.1.0"\nedition = "2021"\n' >"$work/Cargo.toml"
printf 'target/\n' >"$work/.gitignore"
g add -A && g commit -qm "Add the calculator"
g checkout -qb feature
printf '\n/// Multiplies two numbers.\npub fn mul(a: i32, b: i32) -> i32 {\n    a * b\n}\n' >>"$work/src/lib.rs"
g commit -qam "Add mul"
g checkout -q main
printf '# calc\n' >"$work/README.md"
g add README.md && g commit -qm "Add a README"
g merge -q --no-ff feature -m "Merge branch 'feature'"
printf 'Things to do.\n' >"$work/notes.md"

pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
"$bin" --no-persist --reset-layout --folder "$work" --open-file "$work/src/lib.rs" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^repo - Eludite\$" | head -1)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-8}"

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

# 1. A change in the editor, saved.
keys ctrl+End
xdotool type --delay 30 "// Edited in Eludite."
keys Return ctrl+s
sleep 2
shot changes
# 2. Git Changes.
keys ctrl+0 ctrl+g
sleep 1
shot git-changes
# 3. Compare with Unmodified, F8 to the change.
dclick "git-file-changes-src/lib.rs"
sleep 1
keys F8
shot compare
# 4. Commit All with a message.
click git-changes-message
xdotool type --delay 30 "Note an edit"
sleep 0.3
click git-changes-commit
sleep 2
keys ctrl+0 ctrl+g
sleep 0.5
shot committed
# 5. The Git Repository window.
keys ctrl+0 ctrl+r
sleep 2
shot repository
stop
git -C "$work" log --oneline --graph --all >"$out/log.txt"
ls "$out"
