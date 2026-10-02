#!/usr/bin/env bash
# Brief 0012 manual run in a nested virtual KWin (works while the session is locked):
#   1. Wayland backend: open the solution and a file, write the open timings  -> OUT_DIR/timings-wayland.json
#   2. X11 backend on the nested Xwayland: the same, then tools/edit_error.py types an error with XTest, waits
#      for its diagnostics, takes a screenshot, deletes it, waits for them to clear, takes another
#                                                                          -> OUT_DIR/edit.json, *.png
#   3. Wayland backend, 3 runs each: keystroke frame cost in LspProxy.cs while diagnostics arrive, and the same
#      without a solution or host (--bench-type 500)                       -> OUT_DIR/type.jsonl, type-no-host.jsonl
# Usage: tools/open-solution-linux.sh OUT_DIR [SOLUTION [FILE]]
#   defaults: dotnet/Eludite.slnx and dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
# The host is ELUDITE_HOST (default: the Debug build under dotnet/src/Eludite.Host/bin).
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/release/eludite}
sln=$(realpath "${2:-$repo/dotnet/Eludite.slnx}")
file=$(realpath "${3:-$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs}")
host=${ELUDITE_HOST:-$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host}
title="$(basename "${sln%.*}") - Eludite"
cp "$file" "$out/file.orig"
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(printf %q "$out/config") ELUDITE_HOST=$(printf %q "$host") ELUDITE_TRACE_LSP=1
wl=\$WAYLAND_DISPLAY
wait_file() { for i in \$(seq 1 1800); do [[ -s "\$1" ]] && return 0; sleep 0.1; done; return 1; }
$(printf %q "$bin") --reset-layout --solution $(printf %q "$sln") --open-file $(printf %q "$file") \\
  --timings-out $(printf %q "$out/timings-wayland.json") >$(printf %q "$out/run1.out") 2>$(printf %q "$out/run1.err") &
pid=\$!; wait_file $(printf %q "$out/timings-wayland.json"); sleep 1
spectacle -b -n -f -o $(printf %q "$out/opened-wayland.png") >/dev/null 2>&1
kill \$pid; wait \$pid
env -u WAYLAND_DISPLAY $(printf %q "$bin") --reset-layout --solution $(printf %q "$sln") --open-file $(printf %q "$file") \\
  --timings-out $(printf %q "$out/timings-x11.json") >$(printf %q "$out/run2.out") 2>$(printf %q "$out/run2.err") &
pid=\$!
SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(printf %q "$here/edit_error.py") --title $(printf %q "$title") \\
  --log $(printf %q "$out/run2.err") --shots $(printf %q "$out") >$(printf %q "$out/edit.json") 2>$(printf %q "$out/edit.err") || true
kill \$pid; wait \$pid
for run in 1 2 3; do
  $(printf %q "$bin") --reset-layout --solution $(printf %q "$sln") --open-file $(printf %q "$repo/dotnet/src/Eludite.Host/Lsp/LspProxy.cs") \
    --bench-type 500 >>$(printf %q "$out/type.jsonl") 2>$(printf %q "$out/type-\$run.err")
  $(printf %q "$bin") --reset-layout --open-file $(printf %q "$repo/dotnet/src/Eludite.Host/Lsp/LspProxy.cs") \
    --bench-type 500 >>$(printf %q "$out/type-no-host.jsonl") 2>$(printf %q "$out/type-no-host-\$run.err")
done
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 1200 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
# The edit is undone by the run; make sure the file on disk is unchanged.
cmp -s "$file" "$out/file.orig" || { echo "restoring $file"; cp "$out/file.orig" "$file"; }
ls "$out"
