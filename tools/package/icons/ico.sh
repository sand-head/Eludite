#!/usr/bin/env bash
# Renders the app icon from eludite.svg: eludite-48.png and eludite-256.png (Linux's hicolor icons) and eludite.ico
# (Windows: embedded in eludite.exe by crates/eludite/build.rs, used by the MSI), the .ico with every size Windows asks
# for drawn from the vector. Needs rsvg-convert (librsvg) and ImageMagick. Run it after changing eludite.svg.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
magick="$(command -v magick || command -v convert)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
sizes=(256 128 64 48 40 32 24 20 16)
for s in "${sizes[@]}"; do
  rsvg-convert -w "$s" -h "$s" "$here/eludite.svg" -o "$tmp/$s.png"
done
cp "$tmp/48.png" "$here/eludite-48.png"
cp "$tmp/256.png" "$here/eludite-256.png"
"$magick" $(printf "$tmp/%s.png " "${sizes[@]}") "$here/eludite.ico"
