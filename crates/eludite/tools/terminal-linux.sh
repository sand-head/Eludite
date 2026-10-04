#!/usr/bin/env bash
# Brief 0041 manual run on an Xvfb screen (see xvfb-linux.sh): the Terminal window in the real eludite binary, driven
# with real X input from xdotool against the element bounds `--bounds-out` reports, with screenshots.
#   Eludite opens a small Cargo package as its workspace, with the scripted fake agent "Terminal Runner" (its steps:
#   eludite.terminal.send `sleep 2 && cargo --version`, then eludite.terminal.wait for the prompt).
#   1. Ctrl+`: the Terminal window at the bottom with a shell in the workspace; `dotnet --version` typed by hand
#      prints the SDK Eludite builds with (its folder is first on the terminal's PATH)  -> OUT_DIR/terminal-dotnet.png
#   2. The agent prompted in the Agents window: its first terminal call asks, "Allow for this session"
#                                                                                       -> OUT_DIR/terminal-permission.png
#   3. Allowed: the agent types `cargo --version` into the same terminal; the tab says "Agent Terminal Runner is
#      typing" while its wait runs                                                     -> OUT_DIR/terminal-agent.png
#   4. The agent's turn ended: the output and the transcript's terminal rows          -> OUT_DIR/terminal-done.png
# Usage: crates/eludite/tools/terminal-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool jq; `cargo build -p eludite -p eludite-acp` (or ELUDITE_BIN and
# ELUDITE_FAKE_AGENT); a .NET SDK (DOTNET_ROOT, default ~/.dotnet when it exists) and Cargo for the commands typed.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
fake=${ELUDITE_FAKE_AGENT:-$(dirname "$bin")/eludite-fake-acp-agent}
for f in "$bin" "$fake"; do
  [ -x "$f" ] || { echo "no $f (cargo build -p eludite -p eludite-acp)" >&2; exit 1; }
done
if [ -z "${DOTNET_ROOT:-}" ] && [ -x "$HOME/.dotnet/dotnet" ]; then export DOTNET_ROOT="$HOME/.dotnet"; fi
export DISPLAY=${DISPLAY:-:99}
unset WAYLAND_DISPLAY
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi

ws="$out/calc"; config="$out/config"
rm -rf "$ws" "$config"; mkdir -p "$ws/src" "$config"
printf '[package]\nname = "calc"\nversion = "0.1.0"\nedition = "2021"\n' >"$ws/Cargo.toml"
printf 'pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n' >"$ws/src/lib.rs"
steps='[{"tool": "eludite-terminal-send", "arguments": {"text": "sleep 2 && cargo --version"}},
        {"tool": "eludite-terminal-wait", "arguments": {"prompt": true, "timeout_ms": 30000}}]'
jq -n --arg fake "$fake" --arg steps "$steps" '{
  "agents.custom": [{"name": "Terminal Runner", "command": $fake, "args": ["--scenario", "script", "--script", $steps]}],
  "agents.default": "Terminal Runner"
}' >"$config/settings.json"

pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
ELUDITE_CONFIG_DIR="$config" "$bin" --no-persist --reset-layout --folder "$ws" --agent "Terminal Runner" \
  --bounds-out "$out/bounds.json" >"$out/eludite.out" 2>"$out/eludite.err" &
pid=$!
wid=$(timeout 60 xdotool search --sync --name "^calc - Eludite\$" | head -1)
xdotool windowfocus --sync "$wid" 2>/dev/null || true
sleep "${SETTLE:-6}"

keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }
point() {
  local key=$1 b=""
  for _ in $(seq 1 75); do
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

# 1. Ctrl+`: the Terminal window with a shell; dotnet --version typed by hand.
keys ctrl+grave
sleep 3
xdotool type --delay 40 "dotnet --version"
keys Return
sleep 5
shot terminal-dotnet
# 2. The agent's first terminal call asks.
keys ctrl+backslash ctrl+c
sleep 1
click agents-prompt
xdotool type --delay 30 "Show the Cargo version in the terminal"
keys Return
sleep 4
shot terminal-permission
# 3. Allow for this session: the agent types into the terminal.
click agents-permission-always_allow
sleep 0.4
shot terminal-agent
# 4. Its turn ends.
sleep 5
shot terminal-done
stop
expected=$(cd "$repo" && "${DOTNET_ROOT:+$DOTNET_ROOT/}dotnet" --version 2>/dev/null || true)
echo "The SDK Eludite builds with (global.json in the repository): $expected" | tee "$out/expected-sdk.txt"
ls "$out"
