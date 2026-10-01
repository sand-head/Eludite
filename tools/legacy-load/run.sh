#!/usr/bin/env bash
# Brief 0003: load every corpus project through eludite-host's legacy evaluators and the Roslyn language server,
# and write a JSON result per project plus results/<stamp>/matrix.md.
#
#   tools/legacy-load/run.sh                  # fetch corpus, restore, all phases
#   tools/legacy-load/run.sh --prepare-only   # fetch corpus, reference-assembly packages, build; no measurement
#   PHASES=eval,compile ENTRIES=webforms-changepk tools/legacy-load/run.sh
#   RESULTS=tools/legacy-load/results/<stamp> PHASES=getitem tools/legacy-load/run.sh   # re-run a phase on old evals
#   RESULTS=... PHASES=roslyn ROSLYN_MODES=mono-fixed tools/legacy-load/run.sh            # add a Roslyn mode to a run
#
# Needs: git, python3, the .NET SDK from global.json, network for the first fetch/restore.
# Optional: Mono with Mono's MSBuild (see README.md for a user-space install without root), and the Roslyn language
# server from tools/roslyn-pin/build.sh (the roslyn phase is skipped without it).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
src="${ROSLYN_SRC_DIR:-$HOME/.cache/eludite/roslyn}"
ls_dll="${ELUDITE_ROSLYN_LS:-$src/artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/Release/net10.0/Microsoft.CodeAnalysis.LanguageServer.dll}"

"$repo/corpus/legacy/fetch.sh"

# .NET Framework reference assemblies (MIT NuGet packages) for every TargetFrameworkVersion in the corpus.
refdl="$here/runner/obj/refdl"
mkdir -p "$refdl"
cat > "$refdl/refdl.csproj" <<'XML'
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><TargetFramework>net10.0</TargetFramework><ManagePackageVersionsCentrally>false</ManagePackageVersionsCentrally></PropertyGroup>
  <ItemGroup>
    <PackageDownload Include="Microsoft.NETFramework.ReferenceAssemblies.net40;Microsoft.NETFramework.ReferenceAssemblies.net45;Microsoft.NETFramework.ReferenceAssemblies.net451;Microsoft.NETFramework.ReferenceAssemblies.net452;Microsoft.NETFramework.ReferenceAssemblies.net46;Microsoft.NETFramework.ReferenceAssemblies.net461;Microsoft.NETFramework.ReferenceAssemblies.net462;Microsoft.NETFramework.ReferenceAssemblies.net47;Microsoft.NETFramework.ReferenceAssemblies.net471;Microsoft.NETFramework.ReferenceAssemblies.net472;Microsoft.NETFramework.ReferenceAssemblies.net48;Microsoft.NETFramework.ReferenceAssemblies.net481" Version="[1.0.3]" />
  </ItemGroup>
</Project>
XML
cp "$repo/global.json" "$refdl/global.json"
(cd "$refdl" && dotnet restore -v q)

dotnet build "$here/runner/LegacyLoad.Runner.csproj" -c Release -v q -nologo
[[ "${1:-}" == "--prepare-only" ]] && exit 0

bin="$here/runner/bin/Release/net10.0"
# RESULTS=<dir> reuses an earlier run (phases without eval read its eval/ JSON).
results="${RESULTS:-$here/results/$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$results"
args=(run --manifest "$repo/corpus/legacy/manifest.json" --checkout "$repo/corpus/legacy/.checkout"
      --results "$results" --host "$bin/eludite-host.dll")
[[ -f "$ls_dll" ]] && args+=(--roslyn-ls "$ls_dll") || echo "Roslyn LS not found at $ls_dll; skipping the roslyn phase" >&2
[[ -n "${PHASES:-}" ]] && args+=(--phases "$PHASES")
[[ -n "${ENTRIES:-}" ]] && args+=(--entries "$ENTRIES")
[[ -n "${EVALUATORS:-}" ]] && args+=(--evaluators "$EVALUATORS")
[[ -n "${ROSLYN_MODES:-}" ]] && args+=(--roslyn-modes "$ROSLYN_MODES")
"$bin/legacy-load" "${args[@]}"
echo "results: $results"
