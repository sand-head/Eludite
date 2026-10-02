#!/usr/bin/env bash
# Clones every repo in manifest.json at its pinned commit into corpus/legacy/.checkout/<id>.
# Shallow (depth 1); repos with "sparse" use a blobless partial clone plus sparse checkout.
# Idempotent: a checkout already at the pinned commit is left alone. Needs git and python3.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="$here/.checkout"
mkdir -p "$out"
python3 - "$here/manifest.json" <<'PY' | while IFS=$'\t' read -r id url commit sparse; do
import json, sys
m = json.load(open(sys.argv[1]))
for r in m["repos"]:
    print("\t".join([r["id"], r["url"], r["commit"], " ".join(r.get("sparse", []))]))
PY
  dir="$out/$id"
  if [ -d "$dir/.git" ] && [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" = "$commit" ]; then
    echo "[fetch] $id already at $commit"
    continue
  fi
  echo "[fetch] $id <- $url @ $commit"
  rm -rf "$dir"
  git init -q "$dir"
  git -C "$dir" remote add origin "$url"
  if [ -n "$sparse" ]; then
    git -C "$dir" config remote.origin.promisor true
    git -C "$dir" config remote.origin.partialclonefilter blob:none
    git -C "$dir" sparse-checkout set --no-cone $sparse /LICENSE* /*.md
    git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$commit"
  else
    git -C "$dir" fetch -q --depth 1 origin "$commit"
  fi
  git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
  echo "[fetch] $id done ($(du -sh "$dir" | cut -f1))"
done
