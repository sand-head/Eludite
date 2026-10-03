#!/usr/bin/env bash
# Build brief 0035's Test Explorer corpus in place, in the Debug configuration the Test Explorer discovers: the four
# .NET test projects (xunit.v3 for net10.0 and net472 on Microsoft.Testing.Platform, MSTest on Microsoft.Testing.Platform,
# xunit 2 and NUnit on VSTest) and the Cargo package's test executables (`cargo test --no-run`). Needs the .NET SDK
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
for project in Corpus.XunitV3 Corpus.Xunit2 Corpus.MSTest Corpus.NUnit; do
  "$dotnet" build "$here/$project/$project.csproj" --configuration Debug --nologo "$@"
done
if [ "$with_cargo" = 1 ]; then
  "$cargo" test --no-run --manifest-path "$here/rust/Cargo.toml" --quiet
fi
