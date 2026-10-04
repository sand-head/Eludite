#!/usr/bin/env bash
# Build brief 0048's local NuGet feed in place (feed/): Eludite.Corpus.Logging 1.0.0, then Eludite.Corpus.Greeter
# 1.0.0, 1.1.0 and 2.0.0-beta.1 (each depending on Logging), with `dotnet pack`. Offline: the corpus's NuGet.config
# lists only feed/ and keeps its own global packages folder (.packages/). Needs the .NET SDK pinned in global.json.
# The dotnet/tests build the same packages in their own temporary feeds with NuGet.Packaging; this script is for
# crates/lsp/tests/real_host.rs and the Xvfb run (crates/eludite/tools/nuget-linux.sh).
#   corpus/nuget/build.sh [corpus folder, default this script's]
set -euo pipefail
here=$(cd "${1:-$(dirname "$0")}" && pwd)
dotnet=${DOTNET:-dotnet}
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
mkdir -p "$here/feed"
pack() {
  local project=$1 version=$2
  "$dotnet" pack "$here/packages/$project/$project.csproj" --configuration Release --nologo -v q \
    --output "$here/feed" -p:Version="$version" -p:PackageVersion="$version"
}
pack Eludite.Corpus.Logging 1.0.0
for v in 1.0.0 1.1.0 2.0.0-beta.1; do
  pack Eludite.Corpus.Greeter "$v"
done
ls "$here/feed"
