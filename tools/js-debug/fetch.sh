#!/usr/bin/env bash
# Fetch vscode-js-debug's standalone DAP server, pinned in tools/js-debug/PIN (brief 0038), check its SHA-256 and
# unpack it to ~/.cache/eludite/js-debug/<version>/. Prints the path of js-debug/src/dapDebugServer.js, which Eludite
# runs with the Node.js found on the machine (crates/dap/src/discovery.rs searches this cache folder by itself).
#
#   tools/js-debug/fetch.sh                                # download, verify, unpack; print the server's path
#   export ELUDITE_JS_DEBUG="$(tools/js-debug/fetch.sh)"   # what the real-adapter tests read
#   JS_DEBUG_CACHE=/path tools/js-debug/fetch.sh           # another cache folder (Eludite then needs ELUDITE_JS_DEBUG)
#   JS_DEBUG_PIN_FROM_DOWNLOAD=1 tools/js-debug/fetch.sh   # PIN has no checksum yet: download once, print the sha256
#                                                          # line to add to PIN, and unpack what was downloaded
#
# Upstream: https://github.com/microsoft/vscode-js-debug  License: MIT (SPDX: MIT). Needs curl and tar; Node.js 18 or
# later runs the server (not needed to fetch it). Nothing downloads js-debug at startup.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pin="$here/PIN"
field() { awk -v k="$1" '$1 == k { print $2 }' "$pin"; }
version="$(field version)"
base="$(field base)"
file="$(field file)"
sha="$(field sha256)"
cache="${JS_DEBUG_CACHE:-$HOME/.cache/eludite/js-debug}"
dest="$cache/$version"
out="$dest/js-debug/src/dapDebugServer.js"
if [[ -f "$out" && -f "$dest/.complete" ]]; then
  echo "$out"
  exit 0
fi
from_download="${JS_DEBUG_PIN_FROM_DOWNLOAD:-}"
if [[ -z "$sha" && "$from_download" != 1 ]]; then
  cat >&2 <<MSG
js-debug fetch: tools/js-debug/PIN names version $version but no sha256 yet, so nothing is downloaded unchecked.
On a machine that reaches GitHub, download it once and print the line to add to PIN:

  JS_DEBUG_PIN_FROM_DOWNLOAD=1 tools/js-debug/fetch.sh

then add that \`sha256 ...\` line to tools/js-debug/PIN and commit it.
MSG
  exit 1
fi

mkdir -p "$cache"
stage="$(mktemp -d "$cache/.fetch.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
url="$base/$file"
echo "js-debug fetch: $url" >&2
curl --fail --location --silent --show-error --output "$stage/$file" "$url"
actual="$( (sha256sum "$stage/$file" 2>/dev/null || shasum -a 256 "$stage/$file") | awk '{ print $1 }')"
if [[ -z "$sha" ]]; then
  echo "js-debug fetch: downloaded $file, SHA-256 $actual. Add this line to tools/js-debug/PIN:" >&2
  echo "sha256 $actual" >&2
elif [[ "$actual" != "$sha" ]]; then
  echo "js-debug fetch: SHA-256 mismatch for $file: expected $sha, got $actual" >&2
  exit 1
fi
tar --no-same-owner -xzf "$stage/$file" -C "$stage"
if [[ ! -f "$stage/js-debug/src/dapDebugServer.js" ]]; then
  echo "js-debug fetch: $file has no js-debug/src/dapDebugServer.js" >&2
  exit 1
fi
rm -rf "$dest"
mkdir -p "$dest"
mv "$stage/js-debug" "$dest/js-debug"
touch "$dest/.complete"
echo "js-debug fetch: vscode-js-debug $version in $dest (MIT)" >&2
echo "$out"
