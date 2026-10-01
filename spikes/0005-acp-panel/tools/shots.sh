#!/usr/bin/env bash
# Run the panel in a nested virtual KWin (own D-Bus session, so spectacle can
# use that KWin's screenshot interface) and take screenshots:
#   RUN_DIR/prompt.png  ~1 s after the first permission prompt is shown (if any)
#   RUN_DIR/final.png   ~2 s after the turn ends
# Also writes RUN_DIR/transcript.json, app.out, app.err.
# Usage: tools/shots.sh RUN_DIR -- [spike-acp-panel args...]
set -euo pipefail
run=$(realpath -m "$1"); shift; [[ "${1:-}" == "--" ]] && shift
mkdir -p "$run"
here=$(dirname "$(realpath "$0")")
bin=${SPIKE_BIN:-${CARGO_TARGET_DIR:-$here/../target}/release/spike-acp-panel}
inner="$run/inner.sh"
{
  echo '#!/usr/bin/env bash'
  printf '%q' "$bin"; printf ' %q' "$@" --exit-when-done --linger-ms 6000 --transcript-out "$run/transcript.json"
  echo " >$(printf %q "$run/app.out") 2>$(printf %q "$run/app.err") &"
  echo 'pid=$!; shot_prompt=0'
  echo 'for i in $(seq 1 3000); do'
  echo "  if [[ \$shot_prompt == 0 ]] && grep -q '\\[ui\\] permission prompt shown' $(printf %q "$run/app.err") 2>/dev/null; then sleep 1; spectacle -b -n -f -o $(printf %q "$run/prompt.png") >/dev/null 2>&1; shot_prompt=1; fi"
  echo "  grep -q TURN-ENDED $(printf %q "$run/app.out") 2>/dev/null && break; sleep 0.1"
  echo 'done'
  echo "sleep 2; spectacle -b -n -f -o $(printf %q "$run/final.png") >/dev/null 2>&1"
  echo 'wait $pid'
} >"$inner"
chmod +x "$inner"
OUT_DIR="$run" timeout 600 dbus-run-session -- "$here/nested.sh" -- "$inner" 2>"$run/dbus.err" || true
ls "$run"
