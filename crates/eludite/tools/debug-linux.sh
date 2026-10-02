#!/usr/bin/env bash
# Brief 0018 manual run in a nested virtual KWin (works while the session is locked): debug eludite-host itself from
# dotnet/Eludite.slnx under the real netcoredbg, with the real eludite-host and Roslyn as the shell's language server.
#   0. dotnet/ must be clean. For the run only, dotnet/src/Eludite.Host/Properties/launchSettings.json is written with
#      a Project profile (arguments `--stdio --no-roslyn`, environment ELUDITE_LEGACY=0), so the launch exercises the
#      profile; it is deleted afterwards and dotnet/ is checked clean again.
#   1. X11 backend on the nested Xwayland, real XTest input (tools/debug.py): F9 on Ping's first statement in
#      HostRpcTarget.cs, F5, a ping written into the debuggee's stdin from outside the IDE (as from a second terminal),
#      the break with Locals and the Call Stack (debug-break.png), F10 and a data tip on `timestamp`
#      (debug-step-datatip.png), the Breakpoints window (debug-breakpoints.png), F5 and the pong in the Output window's Debug source
#      (debug-continue-console.png), Shift+F5.
#   2. Wayland backend: --bench-debug SESSIONS, RUNS times. Before the first run the page cache of netcoredbg, the .NET
#      runtime and eludite-host is evicted (posix_fadvise DONTNEED; no root needed): that run's first session is the
#      cold one.                                                                            -> OUT_DIR/debug.jsonl
#   The 1-minute load average before each run goes to OUT_DIR/loadavg.txt.
# Usage: tools/debug-linux.sh OUT_DIR   (RUNS=3 SESSIONS=3 PINGS=20; SKIP_DRIVE=1 skips 1, SKIP_BENCH=1 skips 2)
# Needs: `dotnet build dotnet/Eludite.slnx` (the debuggee), netcoredbg (ELUDITE_NETCOREDBG, see tools/netcoredbg/fetch.sh),
# python-xlib, spectacle.
set -euo pipefail
out=$(realpath -m "$1"); mkdir -p "$out"
here=$(dirname "$(realpath "$0")")
repo=$(realpath "$here/../../..")
bin=${ELUDITE_BIN:-${CARGO_TARGET_DIR:-$repo/target}/release/eludite}
sln=$repo/dotnet/Eludite.slnx
target=$repo/dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs
hostdir=$repo/dotnet/src/Eludite.Host
host=${ELUDITE_HOST:-$hostdir/bin/Debug/net10.0/eludite-host}
ncdbg=${ELUDITE_NETCOREDBG:-$("$repo/tools/netcoredbg/fetch.sh")}
runs=${RUNS:-3}; sessions=${SESSIONS:-3}; pings=${PINGS:-20}
title="Eludite - Eludite"
if [[ -n "$(git -C "$repo" status --porcelain -- dotnet)" ]]; then
  echo "dotnet/ has changes; commit or stash them first" >&2; exit 1
fi
settings=$hostdir/Properties/launchSettings.json
mkdir -p "$hostdir/Properties"
cat >"$settings" <<'JSON'
{
  "profiles": {
    "Eludite.Host": {
      "commandName": "Project",
      "commandLineArgs": "--stdio --no-roslyn",
      "environmentVariables": { "ELUDITE_LEGACY": "0" }
    }
  }
}
JSON
cleanup() {
  rm -f "$settings"; rmdir "$hostdir/Properties" 2>/dev/null || true
  git -C "$repo" status --porcelain -- dotnet
}
trap cleanup EXIT
q() { printf %q "$1"; }
inner="$out/inner.sh"
cat >"$inner" <<INNER
#!/usr/bin/env bash
export ELUDITE_CONFIG_DIR=$(q "$out/config") ELUDITE_HOST=$(q "$host") ELUDITE_TRACE_LSP=1 ELUDITE_NETCOREDBG=$(q "$ncdbg")
wl=\$WAYLAND_DISPLAY
load() { echo "\$1 \$(cut -d' ' -f1-3 /proc/loadavg)" >>$(q "$out/loadavg.txt"); }
if [[ -z "${SKIP_DRIVE:-}" ]]; then
  load drive
  env -u WAYLAND_DISPLAY $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$target") \\
    --bounds-out $(q "$out")/bounds.json >$(q "$out")/drive.out 2>$(q "$out")/drive.err &
  pid=\$!
  SHOT_WAYLAND_DISPLAY=\$wl env -u WAYLAND_DISPLAY python3 $(q "$here/debug.py") --title $(q "$title") \\
    --log $(q "$out")/drive.err --bounds $(q "$out")/bounds.json --shots $(q "$out") \\
    >$(q "$out")/drive.json 2>$(q "$out")/drive-driver.err || true
  kill \$pid; wait \$pid
fi
if [[ -z "${SKIP_BENCH:-}" ]]; then
  python3 - $(q "$ncdbg") $(q "$hostdir/bin") /usr/share/dotnet/shared/Microsoft.NETCore.App <<'PY' >>$(q "$out/evicted.txt")
import os, sys
n = 0
for root in sys.argv[1:]:
    root = os.path.dirname(root) if os.path.isfile(root) else root
    for d, _, files in os.walk(root):
        for f in files:
            try:
                fd = os.open(os.path.join(d, f), os.O_RDONLY)
                os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
                os.close(fd)
                n += 1
            except OSError:
                pass
print(f"evicted {n} files")
PY
  for run in \$(seq 1 $runs); do
    load "debug-\$run"
    ELUDITE_BENCH_PINGS=$pings $(q "$bin") --reset-layout --solution $(q "$sln") --open-file $(q "$target") \\
      --bench-debug $sessions >>$(q "$out/debug.jsonl") 2>$(q "$out")/debug-\$run.err
  done
fi
INNER
chmod +x "$inner"
mkdir -p "$out/xdg-config"
printf '[Xwayland]\nXwaylandEisNoPrompt=true\n' >"$out/xdg-config/kwinrc"
XDG_CONFIG_HOME="$out/xdg-config" timeout 3600 dbus-run-session -- kwin_wayland --virtual --xwayland --no-lockscreen \
  --socket "wayland-eludite-$$" --width 1280 --height 960 --exit-with-session "$inner" \
  2>"$out/kwin.err" || true
ls "$out"
