#!/usr/bin/env bash
# Brief 0050: the web language stack's Xvfb run (see xvfb-linux.sh). OUT_DIR/vite-counter is a copy of
# corpus/web/vite-counter with `npm ci` run (its own TypeScript 5.9.3, ESLint 9 and Prettier 3); Eludite opens the
# folder with the real servers from tools/web-servers/fetch.sh (ELUDITE_WEB_SERVERS) and is driven with real X input
# from xdotool, one start per file:
#   1. src/main.ts: a new line `document.qu` opens typescript-language-server's completion list (the status bar shows
#      `TypeScript: ready (TypeScript 5.9.3, project)` and `ESLint: ready`)              -> OUT_DIR/web-completion.png
#   2. src/lint.js: Ctrl+. on `total` lists ESLint's fixes with TypeScript's actions        -> OUT_DIR/web-eslint-menu.png
#      Enter runs "Fix this prefer-const problem" (`let` becomes `const`)                  -> OUT_DIR/web-eslint-fixed.png
#   3. src/unformatted.ts: Ctrl+K, Ctrl+D formats it with the project's Prettier          -> OUT_DIR/web-format-before.png,
#                                                                                         OUT_DIR/web-format-after.png
#   4. index.html: `ul>li.item$*3` then Tab expands the Emmet abbreviation                -> OUT_DIR/web-emmet.png
#   5. --bench-type 300 in a 10,000-line TypeScript file with TypeScript and ESLint running: the keystroke frame cost
#      (key handler + render to end of present; one JSON line)                             -> OUT_DIR/web-bench-type.json
# Each step waits fixed times (SETTLE, default 12 s, for the servers to start); timings on software rendering and a
# debug build are an upper bound, not the reference machine's.
#   crates/eludite/tools/web-linux.sh OUT_DIR
# Needs: `cargo build -p eludite`; Node.js 20 or later with npm on PATH; ELUDITE_WEB_SERVERS="$(tools/web-servers/fetch.sh)";
# on Debian and Ubuntu xvfb, mesa-vulkan-drivers, imagemagick, xdotool.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/debug/eludite}
[ -x "$bin" ] || { echo "no eludite at $bin (cargo build -p eludite)" >&2; exit 1; }
[ -n "${ELUDITE_WEB_SERVERS:-}" ] || { echo "set ELUDITE_WEB_SERVERS=\"\$(tools/web-servers/fetch.sh)\"" >&2; exit 1; }
settle=${SETTLE:-12}
export DISPLAY=${DISPLAY:-:99} ELUDITE_CONFIG_DIR="$out/config" ELUDITE_TRACE_LSP=1
unset WAYLAND_DISPLAY
if ! xdotool getdisplaygeometry >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1000x24 >"$out/xvfb.log" 2>&1 &
  for _ in $(seq 1 50); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.1; done
fi
vite="$out/vite-counter"
rm -rf "$vite" "$out/config"; mkdir -p "$vite" "$out/config"
(cd "$repo/corpus/web/vite-counter" && tar --exclude=node_modules -cf - .) | (cd "$vite" && tar -xf -)
(cd "$vite" && npm ci --no-audit --no-fund --ignore-scripts --loglevel=error >"$out/npm.log" 2>&1)
python3 - "$vite/src/big.ts" <<'PY'
import sys
lines = []
i = 0
while len(lines) < 10000:
    lines += [f"export function step{i}(count: number, label?: string): string {{",
              f"  const next = Math.min(count + {i}, 100);",
              "  return label ?? String(next);", "}", ""]
    i += 1
open(sys.argv[1], "w").write("\n".join(lines) + "\n")
PY
pid=
stop() { [ -n "$pid" ] && { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }; pid=; }
trap stop EXIT
start() {
  local file=$1 log=$2; shift 2
  "$bin" --no-persist --reset-layout --folder "$vite" --open-file "$vite/$file" "$@" >"$out/$log.out" 2>"$out/$log.err" &
  pid=$!
  wid=$(timeout 60 xdotool search --sync --name "vite-counter" | head -1)
  xdotool windowfocus --sync "$wid" 2>/dev/null || true
  sleep "$settle"
}
keys() { xdotool key --delay 120 "$@"; sleep 0.5; }
shot() { import -window root "$out/$1.png"; echo "$out/$1.png"; }

start src/main.ts completion
keys ctrl+End Return
xdotool type --delay 80 "document.qu"
sleep 4
shot web-completion
keys Escape
stop

start src/lint.js eslint
keys ctrl+Home ctrl+f
xdotool type --delay 40 "total = 1"
keys Return Escape Home
sleep 2
keys ctrl+period
sleep 3
shot web-eslint-menu
keys Return
sleep 3
shot web-eslint-fixed
stop

start src/unformatted.ts format
shot web-format-before
keys ctrl+k ctrl+d
sleep 4
shot web-format-after
stop

start index.html emmet
keys ctrl+Home ctrl+f
xdotool type --delay 40 "</h1>"
keys Return Escape End Return
xdotool type --delay 60 'ul>li.item$*3'
keys Tab
sleep 1
shot web-emmet
stop

"$bin" --no-persist --reset-layout --folder "$vite" --open-file "$vite/src/big.ts" --bench-type 300 \
  >"$out/web-bench-type.json" 2>"$out/bench.err" || true
cat "$out/web-bench-type.json"
ls "$out"
