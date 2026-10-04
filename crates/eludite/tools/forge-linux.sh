#!/usr/bin/env bash
# Brief 0046 manual run on an Xvfb screen (see xvfb-linux.sh): forges in the real eludite binary against the fixture
# server of `eludite-forge` (its GitHub fixtures, on loopback; nothing reaches a real forge), driven with real X input
# from xdotool against the element bounds `--bounds-out` reports, with screenshots.
#   The fixture (OUT_DIR/repo) is a repository the harness makes with git: main, origin/main, and feature/login one
#   commit ahead with src/login.rs; its remote is https://github.test/octo-org/hello-world.git, which the settings'
#   `forge.hosts` maps to the fixture server. Credentials go to OUT_DIR/xdg (XDG_CONFIG_HOME), never the machine's.
#   1. View > Other Windows > Pull Requests: the list (signed out: public data)        -> OUT_DIR/pull-requests.png
#   2. Sign in to GitHub: the dialog, a pasted token; with no credential store on the
#      screen's session, the consent to keep it in a file only the person can read     -> OUT_DIR/sign-in.png
#   3. A double-click on #12: the pull request document, its Threads tab               -> OUT_DIR/pull-request.png
#   4. src/login.rs: the review thread's glyph at line 14, clicked open                -> OUT_DIR/thread.png
#   5. Git > Create Pull Request: the form prefilled from the branch                   -> OUT_DIR/create-pull-request.png
#   6. View > Other Windows > Issues, the first issue selected                          -> OUT_DIR/issues.png
# Usage: crates/eludite/tools/forge-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool jq git; `cargo build -p eludite` and
# `cargo build -p eludite-forge --example fixture-server` (or ELUDITE_BIN and FIXTURE_SERVER).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
target=${CARGO_TARGET_DIR:-$repo/target}
bin=${ELUDITE_BIN:-$target/debug/eludite}
server_bin=${FIXTURE_SERVER:-$target/debug/examples/fixture-server}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
[ -x "$server_bin" ] || { echo "no fixture server at $server_bin (cargo build -p eludite-forge --example fixture-server)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config" XDG_CONFIG_HOME="$out/xdg"
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
rm -rf "$out/config" "$out/xdg" "$out/bounds.json" "$out"/*.png
mkdir -p "$out/config" "$out/xdg"

# The fixture repository, with its identity in its own config.
work="$out/repo"
rm -rf "$work"
mkdir -p "$work/src"
g() { git -C "$work" -c user.name="Eludite Run" -c user.email=run@example.com "$@"; }
git init -q -b main "$work"
g config user.name "Eludite Run"
g config user.email run@example.com
for i in $(seq 1 30); do echo "// line $i"; done >"$work/src/login.rs"
printf '.eludite/\n' >"$work/.gitignore"
g add -A && g commit -qm "Initial commit"
g remote add origin https://github.test/octo-org/hello-world.git
g update-ref refs/remotes/origin/main HEAD
g checkout -qb feature/login
sed -i 's|^// line 14$|    post("/session", form);|' "$work/src/login.rs"
g commit -qam "Add the login form" -m "It posts to /session."

pid= spid=
stop() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  if [ -n "$spid" ]; then kill "$spid" 2>/dev/null || true; wait "$spid" 2>/dev/null || true; fi
  pid= spid=
  return 0
}
trap stop EXIT
"$server_bin" "$repo/crates/forge/testdata/github" >"$out/server.out" 2>"$out/server.log" &
spid=$!
for _ in $(seq 1 50); do [ -s "$out/server.out" ] && break; sleep 0.1; done
base=$(head -1 "$out/server.out")
[ -n "$base" ] || { echo "the fixture server did not start" >&2; exit 1; }
jq -n --arg api "$base" '{"forge.hosts": [{"host": "github.test", "family": "github", "api": $api}]}' \
  >"$out/config/settings.json"

"$bin" --no-persist --reset-layout --folder "$work" --open-file "$work/src/login.rs" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^repo - Eludite\$" | head -1)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-8}"
before=$(wc -l <"$out/server.log")

keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }
# The screen position of probed element $1's center (waits for it to be drawn).
point() {
  local key=$1 b=""
  for _ in $(seq 1 50); do
    # Move the pointer (a frame follows) over the editor, away from the menu bar: hovering a title switches menus.
    xdotool mousemove --window "$wid" $((600 + RANDOM % 40)) 600 >/dev/null 2>&1 || true
    b=$(jq -r --arg k "$key" '.[$k] // empty | @tsv' "$out/bounds.json" 2>/dev/null || true)
    [ -n "$b" ] && break
    sleep 0.2
  done
  [ -n "$b" ] || { echo "no element $key on screen" >&2; return 1; }
  read -r x y w h <<<"$b"
  eval "$(xdotool getwindowgeometry --shell "$wid")"
  echo "$(( X + ${x%.*} + ${w%.*} / 2 )) $(( Y + ${y%.*} + ${h%.*} / 2 ))"
}
click() { local p; p=$(point "$1") || exit 1; read -r px py <<<"$p"; xdotool mousemove "$px" "$py" click 1; sleep 0.8; }
dclick() { local p; p=$(point "$1") || exit 1; read -r px py <<<"$p"; xdotool mousemove "$px" "$py" click --repeat 2 --delay 80 1; sleep 1.5; }
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

# Nothing reached the fixture server at startup.
echo "requests before a window opened: $before" | tee "$out/startup-requests.txt"
[ "$before" -eq 0 ] || { echo "the forge was reached at startup" >&2; exit 1; }
# 1. The Pull Requests window.
menu View "Other Windows > Pull Requests"
sleep 2
shot pull-requests
# 2. Sign in with a token.
click forge-pulls-sign-in
click forge-sign-in-method-token
xdotool type --delay 30 "ghp_run_token_not_real"
click forge-sign-in-ok
sleep 1.5
if has forge-sign-in-file; then
  click forge-sign-in-file
  shot sign-in
  click forge-sign-in-ok
else
  shot sign-in
fi
sleep 2
# 3. #12, its Threads tab.
dclick forge-pull-0
click forge-pr-tab-threads
sleep 1
shot pull-request
# 4. The editor's margin on the checked-out branch, the thread open.
click "doc-tab-$work/src/login.rs"
sleep 1
click forge-thread-mark-0
sleep 1
shot thread
keys Escape
# 5. Create Pull Request.
menu Git "Create Pull Request"
sleep 2
shot create-pull-request
# 6. Issues.
menu View "Other Windows > Issues"
sleep 2
click forge-issue-0
sleep 1.5
shot issues
stop
cp "$out/server.log" "$out/requests.txt"
if [ -f "$out/xdg/eludite/forge-credentials.json" ]; then
  stat -c '%a %n' "$out/xdg/eludite/forge-credentials.json" | tee "$out/credentials-mode.txt"
fi
if grep -rq "ghp_run_token_not_real" "$work/.eludite" 2>/dev/null; then
  echo "the token reached the cache" >&2
  exit 1
fi
echo "The token is not in the cache." | tee "$out/token.txt"
ls "$out"
