#!/usr/bin/env bash
# Brief 0039: lay out a built Eludite with its browser engine and make the Linux tarball.
#
#   tools/package/linux.sh [--profile release|debug] [--out DIR] [--no-build] [--with-companions] [--channel CHANNEL --build ID]
#
# Builds `eludite` and `eludite-chromium` (with the `cef` feature) in the given cargo profile (release by default),
# then writes DIR/eludite-<version>-linux-<arch>/ and DIR/eludite-<version>-linux-<arch>.tar.gz (DIR defaults to
# target/package). The layout (tools/package/README.md):
#
#   eludite                    the shell (loads nothing of CEF)
#   eludite-chromium           the browser engine; finds CEF in cef/ beside itself ($ORIGIN/cef run path)
#   cef/                       CEF's runtime files: libcef.so, chrome-sandbox, the paks, locales/, icudtl.dat,
#                              v8_context_snapshot.bin, SwiftShader, CEF's LICENSE.txt and Chromium's CREDITS.html
#   eludite.desktop, icons/    the freedesktop launcher and icons (hicolor theme layout)
#   README, LICENSE, THIRD-PARTY-CRATES.txt
#
# CEF comes from tools/cef/fetch.sh, which downloads nothing when its cache holds the pinned version. --no-build
# packages what target/<profile>/ already holds. --profile debug packages a development build (the smoke test in
# browsers/chromium/tests/package.rs uses it, so `cargo test` never makes a release build). --with-companions adds the
# .NET host, eludite-dbg-mono, eludite-claude-acp and eludite-openai-acp through companions.sh (CI does; the smoke
# test does not).
# --channel and --build write build.json into the layout (build-json.sh, tools/package/RELEASE.md), which makes the
# packaged Eludite one that updates itself from that channel's releases; without them it is a development build to the
# updater. The last line on stdout is the tarball's path; everything else goes to stderr.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
profile=release
out="$repo/target/package"
build=1
companions=0
channel=
build_id=
while [ $# -gt 0 ]; do
  case "$1" in
    --profile) profile=${2:?--profile needs release or debug}; shift 2 ;;
    --out) out=${2:?--out needs a folder}; shift 2 ;;
    --no-build) build=0; shift ;;
    --with-companions) companions=1; shift ;;
    --channel) channel=${2:?--channel needs a value}; shift 2 ;;
    --build) build_id=${2:?--build needs a value}; shift 2 ;;
    -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "linux.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
case "$profile" in release|debug) ;; *) echo "linux.sh: --profile is release or debug" >&2; exit 2 ;; esac
if { [ -n "$channel" ] && [ -z "$build_id" ]; } || { [ -z "$channel" ] && [ -n "$build_id" ]; }; then
  echo "linux.sh: --channel and --build go together" >&2; exit 2
fi
[ "$(uname -s)" = Linux ] || { echo "linux.sh: Linux only (see tools/package/README.md for Windows and macOS)" >&2; exit 2; }

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -n 1)
arch=$(uname -m)
name="eludite-$version-linux-$arch"
started=$(date +%s)

# CEF: the pinned minimal distribution, from the cache (a no-op then) or downloaded and verified once.
cef=$("$repo/tools/cef/fetch.sh")
[ -f "$cef/libcef.so" ] || { echo "linux.sh: no libcef.so in $cef" >&2; exit 1; }
export CEF_PATH="$cef"

target_dir=${CARGO_TARGET_DIR:-$repo/target}
if [ "$build" = 1 ]; then
  flags=()
  [ "$profile" = release ] && flags+=(--release)
  echo "linux.sh: cargo build ${flags[*]} -p eludite -p eludite-chromium --features eludite-chromium/cef" >&2
  (cd "$repo" && cargo build "${flags[@]}" -p eludite -p eludite-chromium --features eludite-chromium/cef >&2)
fi
bin="$target_dir/$profile"
for f in eludite eludite-chromium; do
  [ -x "$bin/$f" ] || { echo "linux.sh: no $bin/$f (build it, or drop --no-build)" >&2; exit 1; }
done
# An engine built without CEF is the stub that only explains how to build it; never package that.
if grep -aq "was built without CEF" "$bin/eludite-chromium"; then
  echo "linux.sh: $bin/eludite-chromium was built without the cef feature" >&2
  exit 1
fi

mkdir -p "$out"
dest="$out/$name"
rm -rf "$dest" "$out/$name.tar.gz"
mkdir -p "$dest/cef/locales" "$dest/icons/hicolor/scalable/apps" "$dest/icons/hicolor/48x48/apps" \
  "$dest/icons/hicolor/256x256/apps"

install -m 755 "$bin/eludite" "$bin/eludite-chromium" "$dest/"

# CEF's runtime files (tools/cef/README.md, "What the minimal distribution holds"): required, then optional ones that a
# CEF version may not ship (libEGL.so and libGLESv2.so are not in CEF 154's Linux minimal distribution: ANGLE is in
# libcef.so).
for f in libcef.so icudtl.dat v8_context_snapshot.bin resources.pak chrome_100_percent.pak chrome_200_percent.pak \
  LICENSE.txt CREDITS.html; do
  [ -f "$cef/$f" ] || { echo "linux.sh: CEF has no $f ($cef)" >&2; exit 1; }
  install -m 644 "$cef/$f" "$dest/cef/$f"
done
chmod 755 "$dest/cef/libcef.so"
for f in libEGL.so libGLESv2.so libvk_swiftshader.so libvulkan.so.1 vk_swiftshader_icd.json; do
  if [ -f "$cef/$f" ]; then
    install -m 644 "$cef/$f" "$dest/cef/$f"
    case "$f" in *.so|*.so.1) chmod 755 "$dest/cef/$f" ;; esac
  fi
done
install -m 644 "$cef"/locales/*.pak "$dest/cef/locales/"
# The setuid sandbox helper, as CEF ships it: a tarball cannot carry root ownership, so it is installed by hand where
# user namespaces are not available (README, "The sandbox").
install -m 755 "$cef/chrome-sandbox" "$dest/cef/chrome-sandbox"

here="$repo/tools/package"
install -m 644 "$here/icons/eludite.svg" "$dest/icons/hicolor/scalable/apps/eludite.svg"
install -m 644 "$here/icons/eludite-48.png" "$dest/icons/hicolor/48x48/apps/eludite.png"
install -m 644 "$here/icons/eludite-256.png" "$dest/icons/hicolor/256x256/apps/eludite.png"
install -m 644 "$here/eludite.desktop" "$dest/eludite.desktop"
install -m 644 "$repo/LICENSE" "$dest/LICENSE"
sed -e "s/@VERSION@/$version/g" -e "s/@ARCH@/$arch/g" "$here/README.in" >"$dest/README"
chmod 644 "$dest/README"
if [ -n "$channel" ]; then
  "$here/build-json.sh" --channel "$channel" --build "$build_id" --os linux --arch "$arch" --out "$dest/build.json"
  chmod 644 "$dest/build.json"
fi

# The Rust crates linked into the two executables, with their licenses (from Cargo.lock, offline).
{
  echo "Rust crates linked into eludite and eludite-chromium $version (name version SPDX license), from cargo tree."
  echo "Eludite itself is GPL-3.0-or-later (LICENSE); CEF is BSD-3-Clause (cef/LICENSE.txt); Chromium's third-party"
  echo "licenses are in cef/CREDITS.html."
  echo
  (cd "$repo" && cargo tree --offline -p eludite -p eludite-chromium --features eludite-chromium/cef -e normal \
    --prefix none --format '{p} {l}' 2>/dev/null) | sed 's/ (\*)$//; s/ (proc-macro)//; s/ ([^)]*)\( \|$\)/\1/' |
    sort -u
} >"$dest/THIRD-PARTY-CRATES.txt"

if [ "$companions" = 1 ]; then
  "$here/companions.sh" --into "$dest"
  { echo; sed 's/@EXE@//g' "$here/README-companions.in"; } >>"$dest/README"
fi

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$dest/eludite.desktop" >&2
fi

tar -C "$out" -czf "$out/$name.tar.gz" "$name"
layout_bytes=$(du -sb "$dest" | cut -f1)
tar_bytes=$(stat -c %s "$out/$name.tar.gz")
echo "linux.sh: $dest: $((layout_bytes / 1048576)) MB laid out; $name.tar.gz: $((tar_bytes / 1048576)) MB" \
  "($tar_bytes bytes) in $(($(date +%s) - started)) s" >&2
echo "$out/$name.tar.gz"
