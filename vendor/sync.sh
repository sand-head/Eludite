#!/usr/bin/env bash
# Re-fetch the vendored Zed crates at the pinned commit and report drift.
#
# Usage: vendor/sync.sh [--rev REV] [--from ZED_CHECKOUT]
#   --rev REV           Compare against REV instead of the pin (to preview a bump).
#   --from DIR          Use an existing Zed checkout at that commit instead of
#                       fetching (for example cargo's checkout under
#                       ~/.cargo/git/checkouts/zed-*/<short-rev>/). Offline.
#
# The pin is read from the root Cargo.toml (the gpui git dependency). Each
# vendor/<crate>/ is compared with crates/<crate>/ upstream, following
# upstream's LICENSE symlinks and ignoring WHY.md. Exit 0 means no drift;
# exit 1 lists every differing file. Nothing is copied: a re-sync is a
# deliberate change reviewed with its WHY.md updates.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/.." && pwd)
crates=(sum_tree rope text clock fuzzy)
repo=https://github.com/zed-industries/zed

pin=$(grep -E '^gpui = \{ git = "https://github.com/zed-industries/zed"' "$root/Cargo.toml" |
  sed -E 's/.*rev = "([0-9a-f]{40})".*/\1/')
[[ ${#pin} == 40 ]] || { echo "sync.sh: could not read the gpui rev from Cargo.toml" >&2; exit 2; }

rev=$pin
from=
while [[ $# -gt 0 ]]; do
  case $1 in
    --rev) rev=$2; shift 2 ;;
    --from) from=$2; shift 2 ;;
    -h|--help) sed -n '2,15p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "sync.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done

for c in "${crates[@]}"; do
  grep -q "\`$pin\`" "$here/$c/WHY.md" ||
    echo "warning: vendor/$c/WHY.md does not name the pin $pin"
done

tmp=
cleanup() { if [[ -n $tmp ]]; then rm -rf "$tmp"; fi; }
trap cleanup EXIT

if [[ -n $from ]]; then
  src=$(cd "$from" && pwd)
  if have=$(git -C "$src" rev-parse HEAD 2>/dev/null) && [[ $have != "$rev" ]]; then
    echo "sync.sh: $src is at $have, not $rev" >&2; exit 2
  fi
else
  tmp=$(mktemp -d)
  src=$tmp/zed
  git init -q "$src"
  git -C "$src" remote add origin "$repo"
  git -C "$src" config core.sparseCheckout true
  {
    for c in "${crates[@]}"; do echo "/crates/$c/"; done
    echo "/LICENSE-GPL"; echo "/LICENSE-APACHE"
  } >"$src/.git/info/sparse-checkout"
  echo "Fetching $repo at $rev ..."
  git -C "$src" fetch -q --depth 1 --filter=blob:none origin "$rev"
  git -C "$src" checkout -q FETCH_HEAD
fi

echo "Comparing vendor/ with zed-industries/zed@$rev"
drift=0
for c in "${crates[@]}"; do
  if out=$(diff -r --exclude=WHY.md "$src/crates/$c" "$here/$c"); then
    echo "  $c: no drift"
  else
    echo "  $c: DRIFT"
    echo "$out" | sed 's/^/    /'
    drift=1
  fi
done
if [[ $drift == 0 ]]; then
  echo "No drift at $rev."
else
  echo "Drift found. Record every intended local change in the crate's WHY.md." >&2
fi
exit $drift
