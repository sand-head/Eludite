#!/usr/bin/env bash
# Brief 0052 manual run on an Xvfb screen (see xvfb-linux.sh): CodeLens from a real language server, driven with real X
# input from xdotool, with screenshots. The C# lenses need the Roslyn language server (tools/roslyn-pin/build.sh); the
# Rust ones need rust-analyzer (tools/rust-analyzer/fetch.sh, then ELUDITE_RUST_ANALYZER or beside eludite).
#   1. Rust: a generated crate OUT_DIR/lens-demo (a trait with two impls in two files, two tests), opened with
#      `--folder ... --open-file src/lib.rs`: rust-analyzer's lenses above the trait, the types and the tests
#                                                                                   -> OUT_DIR/codelens-rows.png
#      Ctrl+Home, Ctrl+K, Ctrl+Q opens the trait's "2 implementations" lens: the References popup, by file
#                                                                                   -> OUT_DIR/codelens-popup.png
#      Escape; Ctrl+F `fn squares`, Ctrl+K, Ctrl+Q runs the test from its Run Test lens through the Test Explorer
#      (discovery with `cargo test --no-run` first): the lens shows the outcome's glyph and duration
#                                                                                   -> OUT_DIR/codelens-run.png
#   2. .NET (only when the Roslyn language server is found): `--project corpus/tests/Corpus.XunitV3` with
#      Calculator.cs: the references lens above `Calculator` ("3 references") and its popup -> OUT_DIR/codelens-csharp.png
#   Each step waits fixed times (WAIT, default 30 s, for the server and a run); timings on software rendering are not
#   the reference machine's.
# Usage: crates/eludite/tools/codelens-linux.sh OUT_DIR
# Needs: xvfb mesa-vulkan-drivers imagemagick xdotool cargo; eludite built (cargo build -p eludite).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
wait_server=${WAIT:-30}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config" ELUDITE_HOST="$host" ELUDITE_TRACE_LSP=1
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
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
  demo=$out/lens-demo
  rm -rf "$demo"; mkdir -p "$demo/src"
  cat >"$demo/Cargo.toml" <<'TOML'
[package]
name = "lens-demo"
version = "0.1.0"
edition = "2024"

[workspace]
TOML
  cat >"$demo/src/lib.rs" <<'RS'
//! Brief 0052's CodeLens demo: a trait with two implementations in two files, and two tests.

mod circle;
pub use circle::Circle;

pub trait Shape {
    fn area(&self) -> u32;
}

pub struct Square(pub u32);

impl Shape for Square {
    fn area(&self) -> u32 {
        self.0 * self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squares() {
        assert_eq!(Square(3).area(), 9);
    }

    #[test]
    fn circles() {
        assert_eq!(Circle(1).area(), 3);
    }
}
RS
  cat >"$demo/src/circle.rs" <<'RS'
use crate::Shape;

pub struct Circle(pub u32);

impl Shape for Circle {
    fn area(&self) -> u32 {
        3 * self.0 * self.0
    }
}
RS
  start "lens-demo - Eludite" rust --folder "$demo" --open-file "$demo/src/lib.rs"
  sleep "$wait_server"
  shot codelens-rows
  keys ctrl+Home ctrl+f
  xdotool type --delay 40 "pub trait Shape"
  keys Return Escape
  keys ctrl+k ctrl+q
  sleep 3
  shot codelens-popup
  keys Escape
  # Find keeps its last query: clear it first.
  keys ctrl+Home ctrl+f
  xdotool key --repeat 20 --delay 20 BackSpace
  xdotool type --delay 40 "fn squares"
  keys Return Escape
  keys ctrl+k ctrl+q
  sleep "$wait_server"
  shot codelens-run
  stop
fi

if [ -z "${SKIP_DOTNET:-}" ]; then
  corpus=$repo/corpus/tests/Corpus.XunitV3
  start "Corpus.XunitV3 - Eludite" dotnet --solution "$corpus/Corpus.XunitV3.csproj" \
    --open-file "$corpus/Calculator.cs"
  sleep "$wait_server"
  if grep -q "codeLens reply" "$out/dotnet.err"; then
    keys ctrl+Home ctrl+f
    xdotool type --delay 40 "class Calculator"
    keys Return Escape
    keys ctrl+k ctrl+q
    sleep 5
    shot codelens-csharp
  else
    echo "no C# lenses: the Roslyn language server is not located (tools/roslyn-pin/build.sh)" >&2
    shot codelens-csharp-unavailable
  fi
  stop
fi
ls "$out"
