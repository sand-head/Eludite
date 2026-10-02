#!/usr/bin/env bash
# Fetch dotnet/roslyn at the commit pinned in tools/roslyn-pin/COMMIT and build
# Microsoft.CodeAnalysis.LanguageServer from source (brief 0002).
#
#   tools/roslyn-pin/build.sh            # clone/fetch + build (Release)
#   ROSLYN_SRC_DIR=/path tools/roslyn-pin/build.sh
#   ROSLYN_CONFIGURATION=Debug tools/roslyn-pin/build.sh
#
# The clone lives outside this repository (default $HOME/.cache/eludite/roslyn).
# Roslyn's own eng/ scripts download the SDK pinned in its global.json into
# $ROSLYN_SRC_DIR/.dotnet; we let them. Output:
#   $ROSLYN_SRC_DIR/artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/<Config>/net10.0/Microsoft.CodeAnalysis.LanguageServer.dll
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
commit="$(tr -d '[:space:]' < "$here/COMMIT")"
src="${ROSLYN_SRC_DIR:-$HOME/.cache/eludite/roslyn}"
config="${ROSLYN_CONFIGURATION:-Release}"
project="src/LanguageServer/Microsoft.CodeAnalysis.LanguageServer/Microsoft.CodeAnalysis.LanguageServer.csproj"

if [[ ! -d "$src/.git" ]]; then
  mkdir -p "$(dirname "$src")"
  git clone --filter=blob:none --no-checkout https://github.com/dotnet/roslyn.git "$src"
fi
if ! git -C "$src" cat-file -e "$commit^{commit}" 2>/dev/null; then
  git -C "$src" fetch origin "$commit"
fi
git -C "$src" checkout -q --detach "$commit"
echo "roslyn-pin: $src at $(git -C "$src" log -1 --format='%H %cI')" >&2

start=$(date +%s)
cd "$src"
./build.sh --restore --build --configuration "$config" --solution "$project" --nodeReuse false
end=$(date +%s)
out="$src/artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/$config/net10.0/Microsoft.CodeAnalysis.LanguageServer.dll"
echo "roslyn-pin: built in $((end - start)) s" >&2
if [[ ! -f "$out" ]]; then
  echo "roslyn-pin: expected output not found: $out" >&2
  exit 1
fi
echo "$out"
