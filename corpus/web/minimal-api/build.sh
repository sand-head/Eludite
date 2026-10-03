#!/usr/bin/env bash
# Build the web corpus's minimal ASP.NET Core project (brief 0037) in the Debug configuration Eludite's F5 runs. Needs
# the .NET SDK pinned in global.json (its ASP.NET Core shared framework; no NuGet package). Extra arguments go to
# `dotnet build`.
#   corpus/web/minimal-api/build.sh [dotnet build args...]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
"${DOTNET:-dotnet}" build "$here/MinimalApi.csproj" --configuration Debug --nologo "$@"
