#!/usr/bin/env bash
# Fetch CEF's minimal distribution, pinned in tools/cef/PIN (brief 0031), check its SHA-256 and unpack it to
# ~/.cache/eludite/cef/<version>/ in the layout the `cef` crate's build script expects (download-cef's: Release and
# Resources flattened into one folder with include/, cmake/, libcef_dll/ and archive.json). Prints that folder.
#
#   tools/cef/fetch.sh                       # download (326 MB on Linux), verify, unpack; print the folder
#   export CEF_PATH="$(tools/cef/fetch.sh)"  # what the cef crate's build script reads (cargo build --features cef)
#   CEF_CACHE=/path tools/cef/fetch.sh       # another cache folder (Eludite then needs ELUDITE_CEF or CEF_PATH)
#
# eludite-chromium is built against it with `--features eludite-chromium/cef` and CEF_PATH set; the shell finds it,
# in order, beside the engine's executable (its folder, or cef/ as tools/package/linux.sh lays it out), through
# ELUDITE_CEF and CEF_PATH, then in this cache (crates/browser/src/discovery.rs, brief 0039). Always set CEF_PATH when
# building with the feature: without it the cef crate's build script downloads CEF by itself into target/, unchecked
# against PIN. Nothing downloads CEF at startup.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
version="$(awk '$1 == "version" { print $2 }' "$here/PIN")"
base="$(awk '$1 == "base" { print $2 }' "$here/PIN")"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux64; lib=libcef.so ;;
  Darwin-arm64) platform=macosarm64; lib="Chromium Embedded Framework.framework/Chromium Embedded Framework" ;;
  Darwin-x86_64) platform=macosx64; lib="Chromium Embedded Framework.framework/Chromium Embedded Framework" ;;
  MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64) platform=windows64; lib=libcef.dll ;;
  *) echo "cef fetch: no pinned CEF build for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
name="$(awk -v p="$platform" '$1 == "file" && $2 == p { print $3 }' "$here/PIN")"
size="$(awk -v p="$platform" '$1 == "file" && $2 == p { print $4 }' "$here/PIN")"
sha="$(awk -v p="$platform" '$1 == "sha256" && $2 == p { print $3 }' "$here/PIN")"
sha1="$(awk -v p="$platform" '$1 == "sha1" && $2 == p { print $3 }' "$here/PIN")"
cache="${CEF_CACHE:-$HOME/.cache/eludite/cef}"
dest="$cache/$version"
# archive.json is written last: its presence means the folder is complete.
if [[ -f "$dest/archive.json" && -e "$dest/$lib" ]]; then
  echo "$dest"
  exit 0
fi

mkdir -p "$cache"
stage="$(mktemp -d "$cache/.fetch.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
url="$base/${name//+/%2B}"
echo "cef fetch: $url ($size bytes)" >&2
curl --fail --location --silent --show-error --output "$stage/$name" "$url"
actual="$( (sha256sum "$stage/$name" 2>/dev/null || shasum -a 256 "$stage/$name") | awk '{ print $1 }')"
if [[ "$actual" != "$sha" ]]; then
  echo "cef fetch: SHA-256 mismatch for $name: expected $sha, got $actual" >&2
  exit 1
fi
echo "cef fetch: unpacking" >&2
tar --no-same-owner -xjf "$stage/$name" -C "$stage"
rm -f "$stage/$name"
src="$stage/${name%.tar.bz2}"
out="$stage/out"
mv "$src/Release" "$out"
if [[ "$platform" != macos* ]]; then
  # Resources (paks, icudtl.dat, locales/) beside libcef, as download-cef lays it out.
  for f in "$src/Resources"/*; do mv "$f" "$out/"; done
fi
# The Linux minimal distribution's libcef.so carries DWARF debug info (about 1 GB of its 1.45 GB); strip it to
# about 465 MB, keeping the symbol table for backtraces, so the cache stays well under 1.5 GB. CEF_KEEP_DEBUG=1 keeps
# it. (The SHA-256 above is of the download; the strip only changes the local copy.)
if [[ "$platform" == linux64 && "${CEF_KEEP_DEBUG:-}" != 1 ]] && command -v strip > /dev/null; then
  strip --strip-debug "$out/libcef.so"
fi
for f in CMakeLists.txt cmake include libcef_dll CREDITS.html LICENSE.txt README.txt; do
  if [[ -e "$src/$f" ]]; then mv "$src/$f" "$out/"; fi
done
printf '{\n  "type": "minimal",\n  "name": "%s",\n  "sha1": "%s"\n}' "$name" "$sha1" > "$out/archive.json.tmp"
rm -rf "$dest"
mv "$out" "$dest"
mv "$dest/archive.json.tmp" "$dest/archive.json"
echo "cef fetch: CEF $version in $dest ($(du -sh "$dest" | awk '{ print $1 }'))" >&2
echo "$dest"
