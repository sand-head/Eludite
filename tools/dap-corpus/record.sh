#!/usr/bin/env bash
# Re-record brief 0033's DAP conformance corpus (corpus/dap/) from every real adapter on this machine, through the
# shell's conformance tests (crates/eludite/src/shell/debug/conformance_tests.rs): eludite-dbg-mono under Mono,
# lldb-dap, netcoredbg, vscode-js-debug (brief 0038). Prints which adapters it records and which it skips, and why.
#
#   tools/dap-corpus/record.sh [--check] [mono] [lldb] [netcoredbg] [js-debug]
#
# With no adapter named, every one found is recorded. Without --check the recordings and goldens are written into
# corpus/dap/ (review `git diff corpus/dap` and commit them). With --check they go to a temporary folder (or
# $RECORD_DAP) and each must reproduce the checked-in one, timing aside, or the run fails with the difference; the
# adapters named must be present (CI's Linux job runs `--check mono lldb`). Recordings are only ever made this way,
# never edited by hand.
#
# Needs: Mono and the .NET SDK (eludite-dbg-mono and the TestApp are built here with `dotnet build`), lldb-dap
# (ELUDITE_LLDB_DAP or on PATH) and cargo, netcoredbg (ELUDITE_NETCOREDBG, from tools/netcoredbg/fetch.sh) and the
# .NET SDK. lldb/attach-detach attaches lldb-dap to a process it did not start: on Linux with Yama, that needs
# `sysctl kernel.yama.ptrace_scope=0` (as CI sets it). js-debug (Linux): vscode-js-debug (ELUDITE_JS_DEBUG, from
# tools/js-debug/fetch.sh), Node.js 18 or later (ELUDITE_NODE or on PATH), CEF (CEF_PATH, from tools/cef/fetch.sh:
# eludite-chromium is built here with its `cef` feature) and a display (DISPLAY, e.g. Xvfb); as root, also
# ELUDITE_CHROME_NO_SANDBOX=1.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

check=0
wanted=()
for a in "$@"; do
  case "$a" in
    --check) check=1 ;;
    mono | lldb | netcoredbg | js-debug) wanted+=("$a") ;;
    *) echo "usage: $0 [--check] [mono] [lldb] [netcoredbg] [js-debug]" >&2; exit 2 ;;
  esac
done
required=0
if [ ${#wanted[@]} -eq 0 ]; then
  wanted=(mono lldb netcoredbg js-debug)
else
  required=1
fi

have() { command -v "$1" >/dev/null 2>&1; }

found=()
for a in "${wanted[@]}"; do
  why=""
  case "$a" in
    mono)
      if ! have mono; then why="mono is not on PATH"
      elif ! have dotnet; then why="the .NET SDK (dotnet) is not on PATH"
      fi ;;
    lldb)
      if [ -z "${ELUDITE_LLDB_DAP:-}" ] && ! compgen -c lldb-dap | grep -q .; then why="lldb-dap is not on PATH (or set ELUDITE_LLDB_DAP)"
      elif ! have cargo; then why="cargo is not on PATH"
      fi ;;
    netcoredbg)
      if [ -z "${ELUDITE_NETCOREDBG:-}" ] && ! have netcoredbg; then why="netcoredbg is not found (tools/netcoredbg/fetch.sh prints the path for ELUDITE_NETCOREDBG)"
      elif ! have dotnet; then why="the .NET SDK (dotnet) is not on PATH"
      fi ;;
    js-debug)
      if [ "$(uname -s)" != Linux ]; then why="the embedded engine runs on Linux only so far"
      elif [ -z "${ELUDITE_JS_DEBUG:-}" ]; then why="ELUDITE_JS_DEBUG is not set (tools/js-debug/fetch.sh prints it)"
      elif [ -z "${ELUDITE_NODE:-}" ] && ! have node; then why="Node.js is not on PATH (or set ELUDITE_NODE)"
      elif [ -z "${CEF_PATH:-}" ]; then why="CEF_PATH is not set (tools/cef/fetch.sh prints it)"
      elif [ -z "${DISPLAY:-}" ]; then why="no DISPLAY (run under Xvfb)"
      fi ;;
  esac
  if [ -n "$why" ]; then
    if [ $required -eq 1 ]; then echo "error: $a: $why" >&2; exit 1; fi
    echo "skipped $a: $why"
  else
    echo "recording $a"
    found+=("$a")
  fi
done
if [ ${#found[@]} -eq 0 ]; then
  echo "no adapter to record with"
  exit 0
fi

for a in "${found[@]}"; do
  if [ "$a" = mono ]; then
    dotnet build debuggers/mono/Eludite.Debugger.Mono/Eludite.Debugger.Mono.csproj --configuration Debug --nologo -v q
    dotnet build debuggers/mono/Eludite.Debugger.Mono.TestApp/Eludite.Debugger.Mono.TestApp.csproj --configuration Debug --nologo -v q
    export ELUDITE_DBG_MONO="$root/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe"
  fi
done

for a in "${found[@]}"; do
  if [ "$a" = js-debug ]; then
    cargo build -p eludite-chromium --features cef
  fi
done

filters=()
for a in "${found[@]}"; do filters+=("conformance_tests::${a//-/_}_"); done
if [ $check -eq 1 ]; then
  out=${RECORD_DAP:-$(mktemp -d)}
  export RECORD_DAP="$out" DAP_CORPUS_CHECK=1
  if [ $required -eq 1 ]; then
    DAP_CORPUS_REQUIRE=$(IFS=,; echo "${found[*]}")
    export DAP_CORPUS_REQUIRE
  fi
else
  export RECORD_DAP="$root/corpus/dap"
fi
echo "writing to $RECORD_DAP"
# One scenario at a time: the real adapters and the attach scenarios' fixed port are not shared.
cargo test -p eludite --bin eludite -- "${filters[@]}" --test-threads=1 --nocapture
