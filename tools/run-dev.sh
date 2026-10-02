#!/usr/bin/env bash
# Launch a development build of Eludite on this repository with the host, the debuggers
# (netcoredbg, eludite-dbg-mono for .NET Framework under Mono, lldb-dap for Rust), rust-analyzer
# and the Claude Code adapter wired up. Builds what is missing.
# The browser tools (eludite.browser.*) use ELUDITE_CHROME when it is set, else they
# search for Chrome themselves (tools/chrome/fetch.sh installs Chrome for Testing).
# Usage: tools/run-dev.sh [--solution PATH | --folder PATH] [other eludite args]
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"
[ -x target/release/eludite ] || cargo build --release -p eludite
[ -x agents/claude-acp/target/release/eludite-claude-acp ] || (cd agents/claude-acp && cargo build --release)
host=dotnet/src/Eludite.Host/bin/Debug/net10.0/eludite-host.dll
[ -f "$host" ] || dotnet build dotnet/Eludite.slnx
export ELUDITE_HOST="${ELUDITE_HOST:-$repo/$host}"
export ELUDITE_CLAUDE_ACP="${ELUDITE_CLAUDE_ACP:-$repo/agents/claude-acp/target/release/eludite-claude-acp}"
ncdb=$(ls -d "$HOME"/.cache/eludite/netcoredbg/*/netcoredbg/netcoredbg 2>/dev/null | tail -1 || true)
[ -n "$ncdb" ] && export ELUDITE_NETCOREDBG="${ELUDITE_NETCOREDBG:-$ncdb}"
# Built by `dotnet build dotnet/Eludite.slnx` (brief 0022); it runs under the located Mono.
mono_dbg=debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
[ -f "$mono_dbg" ] && export ELUDITE_DBG_MONO="${ELUDITE_DBG_MONO:-$repo/$mono_dbg}"
# lldb-dap debugs Cargo packages (brief 0029): located on PATH or under /usr/lib/llvm-NN by Eludite itself;
# ELUDITE_LLDB_DAP (or CodeLLDB's ELUDITE_CODELLDB) chooses another (tools/lldb-dap/README.md).
[ -n "${ELUDITE_LLDB_DAP:-}" ] && export ELUDITE_LLDB_DAP
[ -n "${ELUDITE_CODELLDB:-}" ] && export ELUDITE_CODELLDB
[ -n "${ELUDITE_CHROME:-}" ] && export ELUDITE_CHROME
args=("$@"); [ $# -eq 0 ] && args=(--folder "$repo")
exec target/release/eludite "${args[@]}"
