#!/usr/bin/env bash
# Lay out a built Eludite without the embedded browser engine and make its archive: for Windows and macOS, where
# eludite-chromium is still the stub (tools/package/README.md), or for Linux without CEF (linux.sh makes the Linux
# tarball with the engine).
#
#   tools/package/shell.sh [--profile release|debug] [--out DIR] [--no-build] [--channel CHANNEL --build ID]
#
# Builds `eludite` in the given cargo profile (release by default), lays out the companions (companions.sh: the .NET
# host, eludite-dbg-mono, eludite-claude-acp), then writes DIR/eludite-<version>-<os>-<arch>/ and its archive, a .zip
# on Windows and a .tar.gz elsewhere (DIR defaults to target/package):
#
#   eludite[.exe]              the shell
#   eludite-host/              the .NET host (framework-dependent; runs on the installed .NET 10 runtime)
#   eludite-dbg-mono/          the Mono soft-debugger DAP server (runs under Mono)
#   eludite-claude-acp[.exe]   the Claude Code ACP adapter
#   README, LICENSE, THIRD-PARTY-CRATES.txt, licenses/
#
# --no-build packages the `eludite` that target/<profile>/ already holds (the companions are always built; their builds
# are incremental). --channel and --build write build.json into the layout (build-json.sh, tools/package/RELEASE.md),
# which makes the packaged Eludite one that updates itself from that channel's releases; without them it is a
# development build to the updater. The last line on stdout is the archive's path; everything else goes to stderr.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
profile=release
out="$repo/target/package"
build=1
channel=
build_id=
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) profile=${2:?--profile needs release or debug}; shift 2 ;;
    --out) out=${2:?--out needs a folder}; shift 2 ;;
    --no-build) build=0; shift ;;
    --channel) channel=${2:?--channel needs a value}; shift 2 ;;
    --build) build_id=${2:?--build needs a value}; shift 2 ;;
    -h|--help) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "shell.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
case "$profile" in release|debug) ;; *) echo "shell.sh: --profile is release or debug" >&2; exit 2 ;; esac
if { [ -n "$channel" ] && [ -z "$build_id" ]; } || { [ -z "$channel" ] && [ -n "$build_id" ]; }; then
  echo "shell.sh: --channel and --build go together" >&2; exit 2
fi

exe=
case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  MINGW*|MSYS*|CYGWIN*) os=windows; exe=.exe ;;
  *) echo "shell.sh: unsupported system $(uname -s)" >&2; exit 2 ;;
esac
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n 1)
arch=$(uname -m)
name="eludite-$version-$os-$arch"
started=$(date +%s)

target_dir=${CARGO_TARGET_DIR:-$repo/target}
if [ "$build" = 1 ]; then
  flags=()
  [ "$profile" = release ] && flags+=(--release)
  echo "shell.sh: cargo build ${flags[*]} -p eludite" >&2
  (cd "$repo" && cargo build "${flags[@]}" -p eludite >&2)
fi
bin="$target_dir/$profile/eludite$exe"
[ -f "$bin" ] || { echo "shell.sh: no $bin (build it, or drop --no-build)" >&2; exit 1; }

mkdir -p "$out"
dest="$out/$name"
rm -rf "${dest:?}" "${out:?}/${name:?}.tar.gz" "${out:?}/${name:?}.zip"
mkdir -p "$dest"
install -m 755 "$bin" "$dest/eludite$exe"

here="$repo/tools/package"
"$here/companions.sh" --into "$dest"

install -m 644 "$repo/LICENSE" "$dest/LICENSE"
{
  sed -e "s/@VERSION@/$version/g" -e "s/@OS@/$os/g" -e "s/@ARCH@/$arch/g" -e "s/@EXE@/$exe/g" "$here/README-shell.in"
  echo
  sed -e "s/@EXE@/$exe/g" "$here/README-companions.in"
} >"$dest/README"
chmod 644 "$dest/README"
if [ -n "$channel" ]; then
  "$here/build-json.sh" --channel "$channel" --build "$build_id" --os "$os" --arch "$arch" --out "$dest/build.json"
  chmod 644 "$dest/build.json"
fi

# The Rust crates linked into the shell, with their licenses (from Cargo.lock, offline). BSD sed too: no \| alternation.
{
  echo "Rust crates linked into eludite $version (name version SPDX license), from cargo tree."
  echo "Eludite itself is GPL-3.0-or-later (LICENSE); eludite-claude-acp is MIT (licenses/eludite-claude-acp/)."
  echo
  (cd "$repo" && cargo tree --offline -p eludite -e normal --prefix none --format '{p} {l}' 2>/dev/null) |
    sed -e 's/ (\*)$//' -e 's/ (proc-macro)//' -e 's/ ([^)]*)$//' -e 's/ ([^)]*) / /' | sort -u
} >"$dest/THIRD-PARTY-CRATES.txt"

if [ "$os" = windows ]; then
  archive="$out/$name.zip"
  if command -v 7z >/dev/null 2>&1; then
    (cd "$out" && 7z a -tzip -bso0 -bsp0 "$name.zip" "$name" >&2)
  elif command -v zip >/dev/null 2>&1; then
    (cd "$out" && zip -qr "$name.zip" "$name")
  else
    echo "shell.sh: neither 7z nor zip is available to make $archive" >&2; exit 1
  fi
else
  archive="$out/$name.tar.gz"
  # COPYFILE_DISABLE keeps macOS's tar from adding ._ metadata entries.
  COPYFILE_DISABLE=1 tar -C "$out" -czf "$archive" "$name"
fi
layout_kb=$(du -sk "$dest" | cut -f1)
archive_bytes=$(wc -c <"$archive" | tr -d ' ')
echo "shell.sh: $dest: $((layout_kb / 1024)) MB laid out; $(basename "$archive"): $((archive_bytes / 1048576)) MB" \
  "($archive_bytes bytes) in $(($(date +%s) - started)) s" >&2
echo "$archive"
