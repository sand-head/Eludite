#!/usr/bin/env bash
# Fetch the Chrome DevTools Protocol JSON pinned in protocol/cdp/PIN (brief 0023), check the tarball's SHA-256 and
# copy json/browser_protocol.json, json/js_protocol.json and the package's LICENSE (as LICENSE.chromium) into
# protocol/cdp/. The files are checked in; run this when the pin changes, then regenerate the Rust types with
# `cargo run -p eludite-cdp-generator`.
#
#   protocol/cdp/fetch.sh            # download, verify, copy
#   protocol/cdp/fetch.sh --check    # download, verify, and fail if the checked-in files differ
#
# devtools-protocol is BSD-3-Clause (The Chromium Authors).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
url="$(awk '$1 == "url" { print $2 }' "$here/PIN")"
sha="$(awk '$1 == "sha256" { print $2 }' "$here/PIN")"
check=0
[[ "${1:-}" == "--check" ]] && check=1

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "cdp fetch: $url" >&2
curl --fail --location --silent --show-error --output "$tmp/package.tgz" "$url"
actual="$( (sha256sum "$tmp/package.tgz" 2>/dev/null || shasum -a 256 "$tmp/package.tgz") | awk '{ print $1 }')"
if [[ "$actual" != "$sha" ]]; then
  echo "cdp fetch: SHA-256 mismatch: expected $sha, got $actual" >&2
  exit 1
fi
tar -xzf "$tmp/package.tgz" -C "$tmp"
status=0
for pair in "json/browser_protocol.json:browser_protocol.json" "json/js_protocol.json:js_protocol.json" "LICENSE:LICENSE.chromium"; do
  src="$tmp/package/${pair%%:*}"
  dst="$here/${pair##*:}"
  if [[ $check -eq 1 ]]; then
    if ! cmp -s "$src" "$dst"; then
      echo "cdp fetch: $dst differs from the pinned package" >&2
      status=1
    fi
  else
    cp "$src" "$dst"
  fi
done
[[ $check -eq 1 && $status -eq 0 ]] && echo "cdp fetch: the checked-in files match the pin" >&2
exit $status
