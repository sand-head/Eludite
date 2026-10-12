#!/usr/bin/env bash
# Lay out Eludite's own companion programs beside a built `eludite`, in the places the shell searches first, with no
# environment variables (crates/eludite/src/shell/session.rs, crates/dap/src/discovery.rs, crates/acp/src/lib.rs).
#
#   tools/package/companions.sh --into DIR
#
#   DIR/eludite-host/           the .NET host: `dotnet publish` of dotnet/src/Eludite.Host in Release, framework-dependent
#                               (the apphost `eludite-host`, `eludite-host.dll` and its dependencies). It runs on the
#                               .NET 10 runtime installed on the machine, which the SDK global.json names brings; the host
#                               builds projects with that SDK anyway. Published with -p:Version=<the workspace version>
#                               (brief 0080), so `eludite-host --version` and the status bar agree with the shell.
#   DIR/eludite-host/roslyn/    the Roslyn language server (Microsoft.CodeAnalysis.LanguageServer, MIT) built from source
#                               at tools/roslyn-pin's pin (ADR-0015, brief 0080), from ELUDITE_ROSLYN_DIR (the server's
#                               output folder; CI's roslyn job) else tools/roslyn-pin's default output; with Roslyn's
#                               License.txt and THIRD-PARTY-NOTICES.txt in DIR/licenses/roslyn/. Missing, it fails,
#                               unless ELUDITE_PACKAGE_NO_ROSLYN=1 (a development package without C# IntelliSense).
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
# vscode-js-debug, the web language servers and Chrome. The Roslyn language server is built from its pinned source and
# shipped (ADR-0015). Build output goes to stderr; nothing
# is printed on stdout. linux.sh --with-companions and shell.sh call this.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
into=
while [ $# -gt 0 ]; do
  case "$1" in
    --into) into=${2:?--into needs a folder}; shift 2 ;;
    -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "companions.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$into" ] || { echo "companions.sh: --into DIR is required" >&2; exit 2; }
[ -d "$into" ] || { echo "companions.sh: $into is not a folder" >&2; exit 2; }

exe=
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) exe=.exe ;; esac

# Brief 0080: the host carries the release version (Cargo.toml's, which CI's version.sh --set wrote).
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n 1)
echo "companions.sh: dotnet publish Eludite.Host $version (Release) into eludite-host/" >&2
rm -rf "${into:?}/eludite-host"
dotnet publish "$repo/dotnet/src/Eludite.Host/Eludite.Host.csproj" --configuration Release --nologo \
  "-p:Version=$version" --output "$into/eludite-host" >&2
[ -f "$into/eludite-host/eludite-host.dll" ] || { echo "companions.sh: no eludite-host.dll was published" >&2; exit 1; }

# Brief 0080, ADR-0015: the Roslyn language server beside the host, where RoslynProcessLauncher.Locate finds it.
roslyn_src=${ROSLYN_SRC_DIR:-$HOME/.cache/eludite/roslyn}
roslyn=${ELUDITE_ROSLYN_DIR:-$roslyn_src/artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/Release/net10.0}
roslyn_dll=Microsoft.CodeAnalysis.LanguageServer.dll
if [ -f "$roslyn/$roslyn_dll" ]; then
  echo "companions.sh: the Roslyn language server from $roslyn into eludite-host/roslyn/" >&2
  mkdir -p "$into/eludite-host/roslyn"
  cp -R "$roslyn/." "$into/eludite-host/roslyn/"
  # The licenses sit beside the server in CI's artifact, else in the clone's root.
  mkdir -p "$into/licenses/roslyn"
  for notice in License.txt THIRD-PARTY-NOTICES.txt; do
    for from in "$roslyn/$notice" "$roslyn_src/$notice"; do
      if [ -f "$from" ]; then install -m 644 "$from" "$into/licenses/roslyn/$notice"; break; fi
    done
    [ -f "$into/licenses/roslyn/$notice" ] || { echo "companions.sh: no Roslyn $notice beside $roslyn or in $roslyn_src" >&2; exit 1; }
  done
  rm -f "$into/eludite-host/roslyn/License.txt" "$into/eludite-host/roslyn/THIRD-PARTY-NOTICES.txt"
elif [ "${ELUDITE_PACKAGE_NO_ROSLYN:-}" = 1 ]; then
  echo "companions.sh: WARNING: no Roslyn language server at $roslyn; this package has no C# or Visual Basic IntelliSense" >&2
else
  echo "companions.sh: no $roslyn_dll in $roslyn: run tools/roslyn-pin/build.sh, set ELUDITE_ROSLYN_DIR to the server's" \
    "output folder, or set ELUDITE_PACKAGE_NO_ROSLYN=1 to package without it" >&2
  exit 1
fi

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
