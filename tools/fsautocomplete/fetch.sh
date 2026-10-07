#!/usr/bin/env bash
# Install FsAutoComplete, the F# language server, at the version pinned in tools/fsautocomplete/PIN (brief 0063) as a
# .NET tool into ~/.cache/eludite/fsautocomplete/<version>/ with `dotnet tool install --tool-path`, on the machine's
# .NET SDK (nothing is bundled). FsAutoComplete is MIT. Prints the executable's path, which Eludite searches by itself
# after the eludite folder and ELUDITE_FSAUTOCOMPLETE, and before ~/.dotnet/tools and PATH.
#
#   tools/fsautocomplete/fetch.sh                                        # install (once); print the executable's path
#   export ELUDITE_FSAUTOCOMPLETE="$(tools/fsautocomplete/fetch.sh)"     # what the real-server tests read
#   tools/fsautocomplete/fetch.sh /path/to/folder                        # another folder (Eludite then needs
#                                                                        # ELUDITE_FSAUTOCOMPLETE)
#
# Needs the .NET SDK (`dotnet` on PATH) and the network once (nuget.org); idempotent: an installed copy of the pinned
# version is reused, another version in that folder is replaced. Nothing installs this at startup: Eludite runs the
# server only when an F# document opens.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
version="$(awk '$1 == "version" { print $2; exit }' "$here/PIN")"
dest="${1:-$HOME/.cache/eludite/fsautocomplete/$version}"
exe="$dest/fsautocomplete"
command -v dotnet >/dev/null || { echo "fsautocomplete fetch: dotnet was not found on PATH (install the .NET SDK)" >&2; exit 1; }
# A .NET tool's apphost needs DOTNET_ROOT when the SDK is not at the default location (a user-local ~/.dotnet).
if [[ -z "${DOTNET_ROOT:-}" ]]; then
  dotnet_bin="$(command -v dotnet)"
  if command -v readlink >/dev/null; then
    dotnet_bin="$(readlink -f "$dotnet_bin" 2>/dev/null || echo "$dotnet_bin")"
  fi
  export DOTNET_ROOT="$(dirname "$dotnet_bin")"
fi
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
installed=""
if [[ -x "$exe" ]]; then
  installed="$("$exe" --version 2>/dev/null | head -n 1 || true)"
fi
if [[ "$installed" == "$version"* ]]; then
  echo "$exe"
  exit 0
fi
mkdir -p "$dest"
if [[ -x "$exe" ]]; then
  # Another version is there: `dotnet tool install` refuses a second install, `update` replaces it.
  echo "fsautocomplete fetch: replacing $installed with $version in $dest" >&2
  dotnet tool update fsautocomplete --version "$version" --tool-path "$dest" >&2
else
  echo "fsautocomplete fetch: dotnet tool install fsautocomplete $version into $dest" >&2
  dotnet tool install fsautocomplete --version "$version" --tool-path "$dest" >&2
fi
"$exe" --version >&2
if [[ -n "${1:-}" ]]; then
  echo "fsautocomplete fetch: Eludite searches ~/.cache/eludite/fsautocomplete/$version, ~/.dotnet/tools and PATH; for this folder export ELUDITE_FSAUTOCOMPLETE=$exe" >&2
fi
echo "$exe"
