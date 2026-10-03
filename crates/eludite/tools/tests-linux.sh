#!/usr/bin/env bash
# Brief 0035 manual run on an Xvfb screen (see xvfb-linux.sh): the Test Explorer over the built test corpus
# (corpus/tests, `corpus/tests/build.sh` first), driven with real X input from xdotool, with screenshots.
#   1. Rust: `--folder corpus/tests/rust --open-file src/lib.rs`. Ctrl+F puts the caret on `let sum = add(2, 3);` in
#      `adds`, F9 sets a breakpoint there, Ctrl+E, T shows the Test Explorer (discovery through `cargo test -- --list`),
#      Ctrl+R, A runs all tests (one fails, one is ignored)                        -> OUT_DIR/rust-explorer.png
#      Ctrl+R, Ctrl+T debugs the test at the caret under lldb-dap; it stops at the breakpoint -> OUT_DIR/rust-debug.png
#   2. .NET: OUT_DIR/Corpus.slnx lists the four corpus projects; `--solution` opens it with the real eludite-host,
#      Ctrl+E, T and Ctrl+R, A run them all over Microsoft.Testing.Platform and VSTest (net472 under Mono off Windows)
#                                                                                   -> OUT_DIR/dotnet-explorer.png
#   Each step waits fixed times (WAIT, default 20 s, for a run; DEBUG_WAIT, default 20 s, for the stop); timings on
#   software rendering are not the reference machine's. SKIP_RUST=1 or SKIP_DOTNET=1 leaves a step out.
# Usage: crates/eludite/tools/tests-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool; lldb-dap for the Debug Test step; eludite-host built
# (`dotnet build dotnet/Eludite.slnx`, or ELUDITE_HOST) and Mono for net472 off Windows for step 2.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
corpus=$repo/corpus/tests
wait_run=${WAIT:-20}; wait_debug=${DEBUG_WAIT:-20}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config" ELUDITE_HOST="$host"
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
# Start eludite, wait for its window titled $1, focus it.
start() {
  local title=$1 log=$2; shift 2
  "$bin" --no-persist --reset-layout "$@" >"$out/$log.out" 2>"$out/$log.err" &
  pid=$!
  wid=$(timeout 60 xdotool search --sync --name "^$title\$" | head -1)
  xdotool windowfocus --sync "$wid" 2>/dev/null || true
  sleep "${SETTLE:-8}"
}
keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }

if [ -z "${SKIP_RUST:-}" ]; then
  start "rust - Eludite" rust --folder "$corpus/rust" --open-file "$corpus/rust/src/lib.rs"
  keys ctrl+Home ctrl+f
  xdotool type --delay 40 "let sum = add"
  keys Return Escape Left F9
  keys ctrl+e t
  sleep 5
  keys ctrl+r a
  sleep "$wait_run"
  shot rust-explorer
  keys ctrl+r ctrl+t
  sleep "$wait_debug"
  shot rust-debug
  keys shift+F5
  sleep 2
  stop
fi

if [ -z "${SKIP_DOTNET:-}" ]; then
  {
    echo "<Solution>"
    for p in Corpus.XunitV3 Corpus.MSTest Corpus.Xunit2 Corpus.NUnit; do
      echo "  <Project Path=\"$(realpath --relative-to="$out" "$corpus/$p/$p.csproj")\" />"
    done
    echo "</Solution>"
  } >"$out/Corpus.slnx"
  start "Corpus - Eludite" dotnet --solution "$out/Corpus.slnx" \
    --open-file "$corpus/Corpus.XunitV3/CalculatorTests.cs"
  keys ctrl+Home ctrl+f
  xdotool type --delay 40 "var sum = Calculator"
  keys Return Escape Left F9
  keys ctrl+e t
  sleep "$wait_run"
  keys ctrl+r a
  sleep "$wait_run"
  shot dotnet-explorer
  keys ctrl+r ctrl+t
  sleep "$wait_debug"
  shot dotnet-debug
  keys shift+F5
  sleep 2
  stop
fi
ls "$out"
