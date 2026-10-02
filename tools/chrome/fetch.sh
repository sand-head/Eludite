#!/usr/bin/env bash
# Fetch Chrome for Testing, pinned in tools/chrome/PIN (brief 0023), check its SHA-256 and unpack it where Eludite's
# browser tools look for it: ~/.cache/eludite/chrome/<version>/chrome-<platform>/. Prints the executable's path.
#
#   tools/chrome/fetch.sh                     # download (about 150 to 200 MB), verify, unpack; print the executable
#   CHROME_DIR=/path tools/chrome/fetch.sh    # another cache folder (Eludite then needs ELUDITE_CHROME)
#
# Eludite searches, in order: the setting browser.chromePath (ELUDITE_CHROME), this cache, google-chrome,
# google-chrome-stable, chromium, chromium-browser and chrome on PATH, then the platform's usual install locations
# (crates/browser/src/discovery.rs). On Linux, Chrome needs the usual desktop libraries (libnss3, libatk1.0-0,
# libgbm1, libasound2, ...); running as root needs ELUDITE_CHROME_NO_SANDBOX=1.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
version="$(awk '$1 == "version" { print $2 }' "$here/PIN")"
base="$(awk '$1 == "base" { print $2 }' "$here/PIN")"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux64; exe="chrome-linux64/chrome" ;;
  Darwin-arm64) platform=mac-arm64; exe="chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing" ;;
  Darwin-x86_64) platform=mac-x64; exe="chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing" ;;
  MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) platform=win64; exe="chrome-win64/chrome.exe" ;;
  *) echo "chrome fetch: Chrome for Testing has no build for $(uname -s) $(uname -m); set browser.chromePath to a Chrome or Chromium" >&2; exit 1 ;;
esac
sha="$(awk -v p="$platform" '$1 == "sha256" && $2 == p { print $3 }' "$here/PIN")"
dest="${CHROME_DIR:-$HOME/.cache/eludite/chrome}/$version"
out="$dest/$exe"
if [[ -x "$out" ]]; then
  echo "$out"
  exit 0
fi

mkdir -p "$dest"
url="$base/$version/$platform/chrome-$platform.zip"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "chrome fetch: $url" >&2
curl --fail --location --silent --show-error --output "$tmp/chrome.zip" "$url"
actual="$( (sha256sum "$tmp/chrome.zip" 2>/dev/null || shasum -a 256 "$tmp/chrome.zip") | awk '{ print $1 }')"
if [[ "$actual" != "$sha" ]]; then
  echo "chrome fetch: SHA-256 mismatch for chrome-$platform.zip: expected $sha, got $actual" >&2
  exit 1
fi
unzip -q -o "$tmp/chrome.zip" -d "$dest"
if [[ ! -f "$out" ]]; then
  echo "chrome fetch: expected $out after unpacking" >&2
  exit 1
fi
echo "chrome fetch: Chrome for Testing $version in $dest" >&2
echo "$out"
