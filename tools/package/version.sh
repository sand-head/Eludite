#!/usr/bin/env bash
# Print the release version of HEAD, <YY>.<M>.<iteration>: the year and month of HEAD's commit (UTC) and how many
# commits on HEAD's first-parent history (main's pushes and merges) fall in that month up to and including HEAD, so
# `26.10.1` is the first build of October 2026. Numbers carry no leading zeros (Cargo's versions are SemVer), and each
# fits an MSI ProductVersion field. The same commit always gets the same version; it needs the month's history
# (CI checks out with fetch-depth 0).
#
#   tools/package/version.sh            print it
#   tools/package/version.sh --apply    also write it as Cargo.toml's [workspace.package] version
#   tools/package/version.sh --set V    write V there instead (CI computes the version once, in its build-id job)
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
apply= version=
case "${1:-}" in
  '') ;;
  --apply) apply=1 ;;
  --set) apply=1 version=${2:?--set needs a version} ;;
  -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
  *) echo "version.sh: unknown argument $1" >&2; exit 2 ;;
esac

if [ -z "$version" ]; then
  stamp=$(TZ=UTC0 git -C "$repo" log -1 --format=%cd --date=format-local:'%y %m' HEAD)
  read -r yy mm <<<"$stamp"
  yy=$((10#$yy)) mm=$((10#$mm))
  month_start=$(printf '20%02d-%02d-01T00:00:00Z' "$yy" "$mm")
  iteration=$(git -C "$repo" rev-list --first-parent --count --since="$month_start" HEAD)
  [ "$iteration" -ge 1 ] || iteration=1
  version="$yy.$mm.$iteration"
fi
printf '%s' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || {
  echo "version.sh: a version is three numbers without leading zeros, not $version" >&2; exit 2; }

if [ -n "$apply" ]; then
  sed -i.bak "/^\[workspace.package\]/,/^\[/s/^version = \".*\"/version = \"$version\"/" "$repo/Cargo.toml"
  rm -f "$repo/Cargo.toml.bak"
  grep -q "^version = \"$version\"" "$repo/Cargo.toml" || { echo "version.sh: could not set the version" >&2; exit 1; }
fi
echo "$version"
