#!/usr/bin/env bash
# Build brief 0030's seeded-bug corpus: each program for net10.0 (netcoredbg) and net472 (eludite-dbg-mono under Mono),
# in the Debug configuration Eludite's F5 runs. Needs the .NET SDK pinned in global.json; the net472 build takes the
# .NET Framework reference assemblies from NuGet's implicit Microsoft.NETFramework.ReferenceAssemblies package off
# Windows (cached after the first build). Extra arguments go to every `dotnet build`.
#   corpus/debugging/build.sh [dotnet build args...]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dotnet=${DOTNET:-dotnet}
for program in OffByOne MissingCase NullField; do
  "$dotnet" build "$here/$program/$program.csproj" --configuration Debug --nologo "$@"
done
