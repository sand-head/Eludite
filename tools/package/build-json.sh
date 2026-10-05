#!/usr/bin/env bash
# Write the build identity file a packaged Eludite carries beside its executable (tools/package/RELEASE.md).
#
#   tools/package/build-json.sh --channel unstable --build 20261005.142 [--commit SHA] [--os OS] [--arch ARCH] [--out FILE]
#
# The version is the workspace's (Cargo.toml); the os and arch default to this machine's as the packaging scripts
# spell them (linux, windows, macos; uname -m); the commit defaults to HEAD when git answers; `published` is now, UTC.
# Prints the JSON on stdout unless --out names a file. linux.sh and shell.sh call it when given --channel and --build.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
channel= build= commit= os= arch= out=
while [ $# -gt 0 ]; do
  case "$1" in
    --channel) channel=${2:?--channel needs a value}; shift 2 ;;
    --build) build=${2:?--build needs a value}; shift 2 ;;
    --commit) commit=${2:?--commit needs a value}; shift 2 ;;
    --os) os=${2:?--os needs a value}; shift 2 ;;
    --arch) arch=${2:?--arch needs a value}; shift 2 ;;
    --out) out=${2:?--out needs a file}; shift 2 ;;
    -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "build-json.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -n "$channel" ] || { echo "build-json.sh: --channel is required" >&2; exit 2; }
case "$channel" in unstable) ;; *) echo "build-json.sh: unknown channel $channel (unstable)" >&2; exit 2 ;; esac
[ -n "$build" ] || { echo "build-json.sh: --build is required" >&2; exit 2; }
printf '%s' "$build" | grep -Eq '^[A-Za-z0-9]+(\.[A-Za-z0-9]+)*$' || {
  echo "build-json.sh: a build id is dot-separated segments of letters and digits, not $build" >&2; exit 2; }

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n 1)
if [ -z "$os" ]; then
  case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    MINGW*|MSYS*|CYGWIN*) os=windows ;;
    *) echo "build-json.sh: unsupported system $(uname -s); pass --os" >&2; exit 2 ;;
  esac
fi
[ -n "$arch" ] || arch=$(uname -m)
[ -n "$commit" ] || commit=$(git -C "$repo" rev-parse HEAD 2>/dev/null || true)
published=$(date -u +%Y-%m-%dT%H:%M:%SZ)

json=$(printf '{\n  "version": "%s",\n  "channel": "%s",\n  "build": "%s",\n' "$version" "$channel" "$build")
[ -n "$commit" ] && json="$json$(printf '  "commit": "%s",\n' "$commit")"
json="$json$(printf '  "os": "%s",\n  "arch": "%s",\n  "published": "%s"\n}' "$os" "$arch" "$published")"
if [ -n "$out" ]; then
  printf '%s\n' "$json" >"$out"
else
  printf '%s\n' "$json"
fi
