#!/usr/bin/env bash
# Lay out Eludite's own companion programs beside a built `eludite`, in the places the shell searches first, with no
# environment variables (crates/eludite/src/shell/session.rs, crates/dap/src/discovery.rs, crates/acp/src/lib.rs).
#
#   tools/package/companions.sh --into DIR
#
#   DIR/eludite-host/           the .NET host: `dotnet publish` of dotnet/src/Eludite.Host in Release, framework-dependent
#                               (the apphost `eludite-host`, `eludite-host.dll` and its dependencies). It runs on the
#                               .NET 10 runtime installed on the machine, which the SDK global.json names brings; the host
#                               builds projects with that SDK anyway.
#   DIR/eludite-dbg-mono/       the Mono soft-debugger DAP server (debuggers/mono, net472) and its dependencies; the shell
#                               runs it under the Mono it locates (.NET Framework debugging on Linux and macOS).
#   DIR/eludite-claude-acp      the Claude Code ACP adapter (agents/claude-acp, its own cargo workspace), release build,
#                               with its MIT license and notice in DIR/licenses/eludite-claude-acp/.
#   DIR/eludite-openai-acp      the ACP agent over OpenAI-compatible servers (agents/openai-acp, its own cargo workspace,
#                               brief 0060), release build, with its MIT license and notice in
#                               DIR/licenses/eludite-openai-acp/. The shell finds it beside itself for agents.json's
#                               `providers`.
#
# Pinned external tools stay located at run time, never packaged (CLAUDE.md, tools/): netcoredbg, rust-analyzer, lldb-dap,
# the Roslyn language server, vscode-js-debug, the web language servers and Chrome. Build output goes to stderr; nothing
# is printed on stdout. linux.sh --with-companions and shell.sh call this.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
into=
while [ $# -gt 0 ]; do
  case "$1" in
    --into) into=${2:?--into needs a folder}; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "companions.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$into" ] || { echo "companions.sh: --into DIR is required" >&2; exit 2; }
[ -d "$into" ] || { echo "companions.sh: $into is not a folder" >&2; exit 2; }

exe=
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) exe=.exe ;; esac

echo "companions.sh: dotnet publish Eludite.Host (Release) into eludite-host/" >&2
rm -rf "${into:?}/eludite-host"
dotnet publish "$repo/dotnet/src/Eludite.Host/Eludite.Host.csproj" --configuration Release --nologo \
  --output "$into/eludite-host" >&2
[ -f "$into/eludite-host/eludite-host.dll" ] || { echo "companions.sh: no eludite-host.dll was published" >&2; exit 1; }

echo "companions.sh: dotnet publish Eludite.Debugger.Mono (Release) into eludite-dbg-mono/" >&2
rm -rf "${into:?}/eludite-dbg-mono"
dotnet publish "$repo/debuggers/mono/Eludite.Debugger.Mono/Eludite.Debugger.Mono.csproj" --configuration Release \
  --nologo --output "$into/eludite-dbg-mono" >&2
[ -f "$into/eludite-dbg-mono/eludite-dbg-mono.exe" ] || { echo "companions.sh: no eludite-dbg-mono.exe was published" >&2; exit 1; }

echo "companions.sh: cargo build --release --bin eludite-claude-acp (agents/claude-acp)" >&2
(cd "$repo/agents/claude-acp" && cargo build --release --bin eludite-claude-acp >&2)
# agents/claude-acp is its own workspace: its target folder, unless CARGO_TARGET_DIR (absolute) says otherwise.
acp_target=${CARGO_TARGET_DIR:-$repo/agents/claude-acp/target}
acp="$acp_target/release/eludite-claude-acp$exe"
[ -f "$acp" ] || { echo "companions.sh: no $acp" >&2; exit 1; }
install -m 755 "$acp" "$into/eludite-claude-acp$exe"
mkdir -p "$into/licenses/eludite-claude-acp"
install -m 644 "$repo/agents/claude-acp/LICENSE" "$repo/agents/claude-acp/NOTICE" "$into/licenses/eludite-claude-acp/"

# The same rule for the OpenAI-compatible agent (brief 0060): its own workspace and target folder.
echo "companions.sh: cargo build --release --bin eludite-openai-acp (agents/openai-acp)" >&2
(cd "$repo/agents/openai-acp" && cargo build --release --bin eludite-openai-acp >&2)
openai_target=${CARGO_TARGET_DIR:-$repo/agents/openai-acp/target}
openai="$openai_target/release/eludite-openai-acp$exe"
[ -f "$openai" ] || { echo "companions.sh: no $openai" >&2; exit 1; }
install -m 755 "$openai" "$into/eludite-openai-acp$exe"
mkdir -p "$into/licenses/eludite-openai-acp"
install -m 644 "$repo/agents/openai-acp/LICENSE" "$repo/agents/openai-acp/NOTICE" "$into/licenses/eludite-openai-acp/"
