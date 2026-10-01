#!/usr/bin/env bash
# Exercise the docking interactions on Linux with real input, in a nested
# virtual KWin (works while the user session is locked), and take the brief
# 0008 screenshots. Usage: tools/manual-linux.sh OUT_DIR
#   1. Wayland backend: default layout   -> OUT_DIR/default-layout.png
#   2. X11 backend on the nested Xwayland, driven by tools/drive.py with XTest
#      pointer and key events            -> OUT_DIR/drive.jsonl, customized-before-exit.png
#      then closed through WM_DELETE_WINDOW (saves the layout on exit)
#   3. Wayland backend, restarted        -> OUT_DIR/customized-after-restart.png
# Layouts go to OUT_DIR/config (NIELLO_CONFIG_DIR), solution path /work/Demo/Demo.sln.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
bin=${NIELLO_BIN:-${CARGO_TARGET_DIR:-$here/../../../target}/release/niello}
cfg="$out/config"; rm -rf "$cfg"; mkdir -p "$cfg"
sln=/work/Demo/Demo.sln
inner="$out/inner.sh"
cat >"$inner" <<EOF
#!/usr/bin/env bash
export NIELLO_CONFIG_DIR=$(printf %q "$cfg")
wl=\$WAYLAND_DISPLAY
$(printf %q "$bin") --solution $sln --exit-after-ms 4000 >$(printf %q "$out/run1.out") 2>$(printf %q "$out/run1.err") &
sleep 2.5; spectacle -b -n -f -o $(printf %q "$out/default-layout.png") >/dev/null 2>&1; wait
env -u WAYLAND_DISPLAY $(printf %q "$bin") --solution $sln --bounds-out $(printf %q "$out/bounds.json") >$(printf %q "$out/run2.out") 2>$(printf %q "$out/run2.err") &
pid=\$!
SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(printf %q "$here/drive.py") --bounds $(printf %q "$out/bounds.json") --title "Demo - Niello" --shots $(printf %q "$out") --close >$(printf %q "$out/drive.jsonl") 2>$(printf %q "$out/drive.err") || true
wait \$pid; echo "run2 exit \$?" >>$(printf %q "$out/run2.err")
cp -r $(printf %q "$cfg") $(printf %q "$out/config-after-run2")
$(printf %q "$bin") --solution $sln --exit-after-ms 5000 >$(printf %q "$out/run3.out") 2>$(printf %q "$out/run3.err") &
sleep 3; spectacle -b -n -f -o $(printf %q "$out/customized-after-restart.png") >/dev/null 2>&1; wait
EOF
chmod +x "$inner"
# A private config home for the nested KWin, so XTest from drive.py is not held
# behind KWin's "control input devices" prompt; the user's kwinrc is untouched.
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 300 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-niello-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
ls "$out"
