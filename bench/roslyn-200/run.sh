#!/usr/bin/env bash
# Brief 0002 bench: time-to-IntelliSense of niello-host + Roslyn LS on the generated 200-project solution.
#
#   bench/roslyn-200/run.sh                 # 10 cold + 10 warm runs, 1000 completions each (T3)
#   COLD=3 WARM=3 T3=1000 bench/roslyn-200/run.sh
#   bench/roslyn-200/run.sh --prepare-only  # generate + restore + build, no measurement (used by the integration test)
#
# Needs: tools/roslyn-pin/build.sh run first (ROSLYN_SRC_DIR, default ~/.cache/niello/roslyn), python3, the
# .NET SDK from global.json, and the NuGet packages listed in generate.py already in ~/.nuget/packages
# (restore uses only that folder as a source, so the run is offline).
#
# Cold run: Roslyn LS MEF composition cache and /tmp/roslyn-canonical-misc deleted, `dotnet build-server shutdown`.
# The OS page cache is NOT dropped (needs root); say so when quoting cold numbers.
# Warm run: same machine state as the previous run. The host is killed (whole process tree) after every run.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
src="${ROSLYN_SRC_DIR:-$HOME/.cache/niello/roslyn}"
ls_dir="$src/artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/Release/net10.0"
ls_dll="${NIELLO_ROSLYN_LS:-$ls_dir/Microsoft.CodeAnalysis.LanguageServer.dll}"
cold="${COLD:-10}"; warm="${WARM:-10}"; t3="${T3:-1000}"; cancel="${CANCEL_TRIALS:-50}"

if [[ ! -f "$ls_dll" ]]; then
  echo "Roslyn language server not found at $ls_dll; run tools/roslyn-pin/build.sh first" >&2
  exit 1
fi

if [[ ! -f "$here/out/probe.json" || "${REGENERATE:-0}" == "1" ]]; then
  python3 "$here/generate.py" --out "$here/out" > /dev/null
fi
dotnet restore "$here/out/Bench200.slnx" -v q
dotnet build "$repo/dotnet/src/Niello.Host/Niello.Host.csproj" -c Release -v q -nologo
dotnet build "$here/driver/Bench.Driver.csproj" -c Release -v q -nologo
[[ "${1:-}" == "--prepare-only" ]] && exit 0

stamp="$(date +%Y%m%d-%H%M%S)"
results="$here/results/$stamp"
mkdir -p "$results"
{
  echo "date: $(date -Iseconds)"
  echo "cpu: $(lscpu | sed -n 's/^Model name:[[:space:]]*//p')"
  echo "cores: $(nproc) logical; $(lscpu | sed -n 's/^Core(s) per socket:[[:space:]]*//p') per socket"
  echo "ram: $(free -h | awk '/^Mem:/{print $2}')"
  dev="$(df --output=source "$here" | tail -1)"
  echo "disk: $dev ($(lsblk -no TRAN,ROTA,MODEL "$(lsblk -no PKNAME "$dev" 2>/dev/null | head -1 | sed 's|^|/dev/|')" 2>/dev/null | head -1 | xargs))"
  echo "fs: $(df -T "$here" | tail -1 | awk '{print $2}')"
  echo "os: $(. /etc/os-release; echo "$PRETTY_NAME")"
  echo "kernel: $(uname -r)"
  echo "dotnet: $(dotnet --version) (runtime $(dotnet --list-runtimes | awk '/NETCore.App 10/{v=$2} END{print v}'))"
  echo "roslyn: $(cat "$repo/tools/roslyn-pin/COMMIT") ($(git -C "$src" log -1 --format=%cI 2>/dev/null || echo '?'))"
  echo "runs: cold=$cold warm=$warm t3=$t3 cancel-trials=$cancel"
} | tee "$results/machine.txt"

reset_paths="$ls_dir/cache:/tmp/roslyn-canonical-misc"
dotnet "$here/driver/bin/Release/net10.0/Bench.Driver.dll" \
  --host "$repo/dotnet/src/Niello.Host/bin/Release/net10.0/niello-host.dll" \
  --roslyn-ls "$ls_dll" \
  --probe "$here/out/probe.json" \
  --results "$results" \
  --cold "$cold" --warm "$warm" --t3 "$t3" --cancel-trials "$cancel" \
  --cold-reset-paths "$reset_paths"
echo "results: $results" >&2
