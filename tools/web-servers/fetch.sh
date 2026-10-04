#!/usr/bin/env bash
# Install the web language servers and formatters pinned in tools/web-servers/PIN (brief 0050) into
# ~/.cache/eludite/web-servers/<pin>/ with `npm ci` from the checked-in package.json and package-lock.json, and cache
# the pinned SchemaStore JSON schemas under <pin>/schemas/ (each checked against its SHA-256). Prints the folder, which
# Eludite searches by itself after the project's node_modules/.bin and the ELUDITE_<SERVER> variables.
#
#   tools/web-servers/fetch.sh                                     # install (once); print the folder
#   export ELUDITE_WEB_SERVERS="$(tools/web-servers/fetch.sh)"     # what the real-server tests read
#   WEB_SERVERS_CACHE=/path tools/web-servers/fetch.sh             # another cache folder (Eludite then needs
#                                                                  # ELUDITE_WEB_SERVERS)
#
# Needs Node.js (the `node` major version in PIN or later) with npm, and curl. Every package and its SPDX license is
# listed in PIN. Nothing installs these at startup: Eludite runs a server only when a matching document opens.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pin_file="$here/PIN"
field() { awk -v k="$1" '$1 == k { print $2; exit }' "$pin_file"; }
pin="$(field pin)"
node_min="$(field node)"
schemastore="$(field schemastore)"
cache="${WEB_SERVERS_CACHE:-$HOME/.cache/eludite/web-servers}"
dest="$cache/$pin"
if [[ -f "$dest/.complete" ]]; then
  echo "$dest"
  exit 0
fi

command -v node >/dev/null || { echo "web-servers fetch: Node.js was not found on PATH (needs $node_min or later)" >&2; exit 1; }
command -v npm >/dev/null || { echo "web-servers fetch: npm was not found on PATH" >&2; exit 1; }
major="$(node --version | sed -e 's/^v//' -e 's/\..*//')"
if (( major < node_min )); then
  echo "web-servers fetch: Node.js $(node --version) is older than $node_min, which the pinned servers need" >&2
  exit 1
fi

mkdir -p "$cache"
stage="$(mktemp -d "$cache/.fetch.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
cp "$here/package.json" "$here/package-lock.json" "$stage/"
echo "web-servers fetch: npm ci in $stage" >&2
# --ignore-scripts: no install script runs (Biome's only checks its platform package, which it resolves at run time).
(cd "$stage" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error >&2)

mkdir -p "$stage/schemas"
awk '$1 == "schema" { print $2, $3 }' "$pin_file" | while read -r file sha; do
  url="https://raw.githubusercontent.com/SchemaStore/schemastore/$schemastore/src/schemas/json/$file"
  curl --fail --location --silent --show-error --output "$stage/schemas/$file" "$url"
  actual="$( (sha256sum "$stage/schemas/$file" 2>/dev/null || shasum -a 256 "$stage/schemas/$file") | awk '{ print $1 }')"
  if [[ "$actual" != "$sha" ]]; then
    echo "web-servers fetch: SHA-256 mismatch for $file: expected $sha, got $actual" >&2
    exit 1
  fi
done
# Point `$ref`s to SchemaStore's own URLs at the cached copies, so the JSON server reads files only.
node "$here/rewrite-refs.mjs" "$dest/schemas" "$stage/schemas"

rm -rf "$dest"
mv "$stage" "$dest"
trap - EXIT
touch "$dest/.complete"
echo "web-servers fetch: installed $pin into $dest" >&2
echo "$dest"
