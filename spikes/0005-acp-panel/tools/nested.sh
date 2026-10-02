#!/usr/bin/env bash
# Run a command inside a nested, virtual KWin (60 Hz virtual output, real GPU),
# as brief 0001 did, because a locked session sends no Wayland frame callbacks.
# Usage: tools/nested.sh [--x11] -- COMMAND [ARGS...]
#   --x11  unset WAYLAND_DISPLAY inside, so GPUI uses its X11 backend on the
#          nested Xwayland (needed for `import` screenshots).
# The command's stdout/stderr go to $OUT_DIR/nested.{out,err} (default /tmp).
set -euo pipefail
x11=0
if [[ "${1:-}" == "--x11" ]]; then x11=1; shift; fi
[[ "${1:-}" == "--" ]] && shift
out=${OUT_DIR:-/tmp}
inner=$(mktemp)
{
  echo '#!/usr/bin/env bash'
  [[ $x11 == 1 ]] && echo 'unset WAYLAND_DISPLAY'
  printf 'exec'; printf ' %q' "$@"; echo " >$(printf %q "$out/nested.out") 2>$(printf %q "$out/nested.err")"
} >"$inner"
chmod +x "$inner"
kwin_wayland --virtual --xwayland --no-lockscreen --socket "wayland-spike5-$$" \
  --width 1280 --height 960 --exit-with-session "$inner" 2>"$out/kwin.err" || true
rm -f "$inner"
