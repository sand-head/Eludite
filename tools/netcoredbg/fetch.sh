#!/usr/bin/env bash
# Fetch netcoredbg, the .NET debug adapter Eludite debugs .NET (Core) 5+ with (brief 0018, PLAN.md 4.5), pinned by
# version and checksum. netcoredbg is located on the machine, never vendored into git.
#
#   tools/netcoredbg/fetch.sh            # download, verify, unpack; prints the adapter's path
#   NETCOREDBG_DIR=/path tools/netcoredbg/fetch.sh
#
# Upstream: https://github.com/Samsung/netcoredbg  License: MIT (SPDX: MIT)
# Version:  3.2.0-1092 (release of 2026-06-25)
# Assets:   https://github.com/Samsung/netcoredbg/releases/download/3.2.0-1092/<asset>
#
# Output: $NETCOREDBG_DIR/3.2.0-1092/netcoredbg/netcoredbg (netcoredbg.exe on Windows), default
# NETCOREDBG_DIR=$HOME/.cache/eludite/netcoredbg. Point Eludite at it with ELUDITE_NETCOREDBG, put it on PATH, or
# copy the netcoredbg/ folder beside the eludite executable (the order Eludite searches, crates/dap/src/discovery.rs).
#
# If a release binary does not run on this machine, build from source with upstream's CMake instructions
# (https://github.com/Samsung/netcoredbg#building-from-source-code); that needs cmake, clang and the .NET SDK.
set -euo pipefail

version="3.2.0-1092"
base="https://github.com/Samsung/netcoredbg/releases/download/$version"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  asset="netcoredbg-linux-amd64.tar.gz"; sha256="080eb3b2d2152465f599d3b33d1ee6e747794e11cc0a3773ec689f5e5f2c5afa"; exe="netcoredbg" ;;
  Linux-aarch64) asset="netcoredbg-linux-arm64.tar.gz"; sha256="065ff49badec8a695dbea2de6ab6a330c774a191e426a217ab8cc05250627ccb"; exe="netcoredbg" ;;
  Darwin-arm64)  asset="netcoredbg-osx-arm64.zip";      sha256="f4fa33b3ff874910cc184b4bb3b9c56d0abdf5c6521cee0b144d7c6e4a6e59ea"; exe="netcoredbg" ;;
  MINGW*-x86_64|MSYS*-x86_64|CYGWIN*-x86_64)
                 asset="netcoredbg-win64.zip";          sha256="3c410a45fa502415203a94fcb88654af65bf8e3dac158a5527a722e7a6b9274a"; exe="netcoredbg.exe" ;;
  *) echo "netcoredbg: no $version release for $(uname -s) $(uname -m); build from source" >&2; exit 1 ;;
esac

root="${NETCOREDBG_DIR:-$HOME/.cache/eludite/netcoredbg}"
dest="$root/$version"
out="$dest/netcoredbg/$exe"
if [[ -x "$out" ]]; then
  echo "$out"
  exit 0
fi

mkdir -p "$dest"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "netcoredbg: downloading $base/$asset" >&2
curl -fsSL -o "$tmp/$asset" "$base/$asset"
actual="$( (sha256sum "$tmp/$asset" 2>/dev/null || shasum -a 256 "$tmp/$asset") | cut -d' ' -f1)"
if [[ "$actual" != "$sha256" ]]; then
  echo "netcoredbg: checksum mismatch for $asset: expected $sha256, got $actual" >&2
  exit 1
fi
case "$asset" in
  *.tar.gz) tar -xzf "$tmp/$asset" -C "$dest" ;;
  *.zip)    unzip -q -o "$tmp/$asset" -d "$dest" ;;
esac
if [[ ! -f "$out" ]]; then
  echo "netcoredbg: expected $out after unpacking" >&2
  exit 1
fi
echo "netcoredbg: $version in $dest (MIT)" >&2
echo "$out"
