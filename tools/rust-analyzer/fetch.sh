#!/usr/bin/env bash
# Fetch the rust-analyzer release pinned in tools/rust-analyzer/PIN (brief 0019), check its SHA-256 and install it.
# rust-analyzer is MIT OR Apache-2.0. Eludite finds it, in order: beside the eludite executable, at
# ELUDITE_RUST_ANALYZER, on PATH, then as the rustup component (`rustup component add rust-analyzer`).
#
#   tools/rust-analyzer/fetch.sh                    # into $HOME/.cache/eludite/rust-analyzer/<tag>/
#   tools/rust-analyzer/fetch.sh target/debug       # beside a built eludite
#
# Prints the installed path; export ELUDITE_RUST_ANALYZER=<path> when it is not beside eludite.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tag="$(awk '$1 == "tag" { print $2 }' "$here/PIN")"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) triple=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) triple=aarch64-unknown-linux-gnu ;;
  Darwin-x86_64) triple=x86_64-apple-darwin ;;
  Darwin-arm64) triple=aarch64-apple-darwin ;;
  *) echo "rust-analyzer fetch: no pinned build for $(uname -s)-$(uname -m); use tools/rust-analyzer/fetch.ps1 on Windows" >&2; exit 1 ;;
esac
sha="$(awk -v t="$triple" '$1 == "sha256" && $2 == t { print $3 }' "$here/PIN")"
dest="${1:-$HOME/.cache/eludite/rust-analyzer/$tag}"
mkdir -p "$dest"
url="https://github.com/rust-lang/rust-analyzer/releases/download/$tag/rust-analyzer-$triple.gz"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
echo "rust-analyzer fetch: $url" >&2
curl --fail --location --silent --show-error --output "$tmp" "$url"
actual="$( (sha256sum "$tmp" 2>/dev/null || shasum -a 256 "$tmp") | awk '{ print $1 }')"
if [[ "$actual" != "$sha" ]]; then
  echo "rust-analyzer fetch: SHA-256 mismatch: expected $sha, got $actual" >&2
  exit 1
fi
gunzip --stdout "$tmp" > "$dest/rust-analyzer"
chmod +x "$dest/rust-analyzer"
"$dest/rust-analyzer" --version >&2
echo "$dest/rust-analyzer"
