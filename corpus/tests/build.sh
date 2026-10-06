#!/usr/bin/env bash
# Build brief 0035's Test Explorer corpus in place, in the Debug configuration the Test Explorer discovers: the six
# .NET test projects (xunit.v3 for net10.0 and net472 on Microsoft.Testing.Platform, MSTest on Microsoft.Testing.Platform,
# xunit 2 and NUnit on VSTest; brief 0057's Visual Basic on MSTest and F# on NUnit) and the Cargo package's test
# executables (`cargo test --no-run`). Needs the .NET SDK
# pinned in global.json and the Rust toolchain; the net472 build takes the .NET Framework reference assemblies from
# NuGet off Windows. `--no-cargo` leaves the Cargo package out (a .NET-only machine). Extra arguments go to every
# `dotnet build`.
#   corpus/tests/build.sh [--no-cargo] [dotnet build args...]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
dotnet=${DOTNET:-dotnet}
cargo=${CARGO:-cargo}
with_cargo=1
if [ "${1:-}" = "--no-cargo" ]; then
  with_cargo=0
  shift
fi
# Each entry is the project file: C#, Visual Basic (.vbproj) and F# (.fsproj) alike.
for project in Corpus.XunitV3/Corpus.XunitV3.csproj Corpus.Xunit2/Corpus.Xunit2.csproj Corpus.MSTest/Corpus.MSTest.csproj \
  Corpus.NUnit/Corpus.NUnit.csproj Corpus.VisualBasic/Corpus.VisualBasic.vbproj Corpus.FSharp/Corpus.FSharp.fsproj; do
  "$dotnet" build "$here/$project" --configuration Debug --nologo "$@"
done
if [ "$with_cargo" = 1 ]; then
  "$cargo" test --no-run --manifest-path "$here/rust/Cargo.toml" --quiet
fi
