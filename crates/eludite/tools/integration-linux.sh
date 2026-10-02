#!/usr/bin/env bash
# Brief 0020 manual run in a nested virtual KWin (works while the session is locked), on dotnet/Eludite.slnx with the
# real eludite-host, Roslyn and netcoredbg. X11 backend on the nested Xwayland, real XTest input (tools/integration.py):
#   1. Tools > Options, the Projects and Solutions > Build and Run page (integration-options.png), a check box on and
#      off through eludite.settings.set.
#   2. The user settings file rewritten from outside the IDE ten times: write to "settings applied".
#   3. An error at the end of HostRpcTarget.cs, Ctrl+S, F5: the startup project's build fails, nothing launches
#      (integration-f5-build-failed.png); fixed, Ctrl+S, F5: built and launched under netcoredbg
#      (integration-f5-launched.png); Shift+F5.                                   -> OUT_DIR/*.png, OUT_DIR/drive.json
# The 1-minute load average before the drive goes to OUT_DIR/loadavg.txt. The host runs from a copy (OUT_DIR/host),
# because F5 builds Eludite.Host and rewrites its bin/ directory.
# Usage: tools/integration-linux.sh OUT_DIR
# Needs: `dotnet build dotnet/Eludite.slnx`, `cargo build --release -p eludite`, netcoredbg (ELUDITE_NETCOREDBG, see
# tools/netcoredbg/fetch.sh), python-xlib, spectacle. dotnet/ must be clean in git; the run restores it at the end.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
if [[ -n "$(git -C "$repo" status --porcelain -- dotnet/)" ]]; then
  echo "dotnet/ has changes; commit or stash them first (the run restores dotnet/ from git)" >&2
  exit 1
fi
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/release/eludite}
sln=$repo/dotnet/Eludite.slnx
file=$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
ncdbg=${ELUDITE_NETCOREDBG:-$("$repo/tools/netcoredbg/fetch.sh")}
rm -rf "$out/host" "$out/config"
cp -r "$repo/dotnet/src/Eludite.Host/bin/Debug/net10.0" "$out/host"
mkdir -p "$out/config"
echo '{}' >"$out/config/settings.json"
host=${ELUDITE_HOST:-$out/host/eludite-host}
title="Eludite - Eludite"
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1 ELUDITE_NETCOREDBG=$(q "$ncdbg")
wl=\$WAYLAND_DISPLAY
echo "drive \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt")
env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$file") \\
  --bounds-out $(q "$out")/bounds.json >$(q "$out")/drive.out 2>$(q "$out")/drive.err &
pid=\$!
SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/integration.py") --title $(q "$title") \\
  --log $(q "$out")/drive.err --bounds $(q "$out")/bounds.json --shots $(q "$out") \\
  --settings $(q "$out/config/settings.json") >$(q "$out")/drive.json 2>$(q "$out")/driver.err || true
sleep 1
kill \$pid; wait \$pid
git -C $(q "$repo") diff -- dotnet/ >$(q "$out")/dotnet-after.diff
git -C $(q "$repo") checkout -q -- dotnet/
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 1800 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
git -C "$repo" checkout -q -- dotnet/
git -C "$repo" status --porcelain -- dotnet/
ls "$out"
