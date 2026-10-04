# Packaging Eludite with its browser engine

Brief 0039 (proposal 0002, brief E, the Linux half). A built Eludite runs its embedded browser engine from a
predictable layout, with no developer environment variables: `eludite-chromium` and CEF's runtime files beside the
`eludite` executable. Installers, signing and auto-update are Phase 3 (PLAN.md section 13); this folder lays the files
out and makes an archive.

## Linux: `linux.sh`

```
tools/package/linux.sh                           # release build, layout and tarball in target/package/
tools/package/linux.sh --out DIR                 # somewhere else
tools/package/linux.sh --profile debug --no-build  # package the debug build already in target/debug/ (the smoke test)
```

It runs `tools/cef/fetch.sh` (a no-op when CEF's cache holds the version in `tools/cef/PIN`; nothing downloads during
the build then), `cargo build --release -p eludite -p eludite-chromium --features eludite-chromium/cef` with
`CEF_PATH` set, and writes `eludite-<version>-linux-<arch>/` and `eludite-<version>-linux-<arch>.tar.gz`. It refuses
to package an engine built without the `cef` feature (the stub). The last line on stdout is the tarball's path.

```
eludite-0.1.0-linux-x86_64/
  eludite                      the shell; it never loads CEF
  eludite-chromium             the engine; run path $ORIGIN:$ORIGIN/cef, so it loads cef/libcef.so
  cef/
    libcef.so                  CEF 154 (Chromium 154), stripped of DWARF by fetch.sh (465 MB)
    chrome-sandbox             the setuid sandbox helper, as CEF ships it (mode 755, owned by whoever unpacks)
    icudtl.dat, v8_context_snapshot.bin
    resources.pak, chrome_100_percent.pak, chrome_200_percent.pak, locales/*.pak (220 locales)
    libvk_swiftshader.so, libvulkan.so.1, vk_swiftshader_icd.json   software Vulkan for ANGLE
    LICENSE.txt                CEF's license (BSD-3-Clause)
    CREDITS.html               Chromium's third-party licenses
  eludite.desktop              freedesktop launcher (Desktop Entry 1.5; Icon=eludite, StartupWMClass=eludite)
  icons/hicolor/{48x48,256x256}/apps/eludite.png, icons/hicolor/scalable/apps/eludite.svg
  README                       how to run it, add it to the menus, and the sandbox rule
  LICENSE                      GPL-3.0 (Eludite is GPL-3.0-or-later)
  THIRD-PARTY-CRATES.txt       the Rust crates linked into the two executables, with their SPDX licenses (cargo tree)
```

`libEGL.so` and `libGLESv2.so` are copied when CEF ships them; CEF 154's Linux minimal distribution does not (ANGLE is
inside `libcef.so`). The sizes of a release build are in brief 0039's report.

**Discovery** (`crates/browser/src/discovery.rs`), on first use only (the Web Browser window or a browser command;
never at startup): the engine at the setting `browser.enginePath`, then `ELUDITE_CHROMIUM`, then beside `eludite`
(this layout), then cargo's build folder in a development build; CEF beside the engine (`libcef.so` in its folder, as
cargo copies it, or in `cef/`, as here), then `ELUDITE_CEF` and `CEF_PATH`, then `tools/cef/fetch.sh`'s cache.
`eludite --print-engine-discovery` is a hidden flag that prints what it finds as JSON (the smoke test's).

**The sandbox** (`browsers/chromium/src/sandbox.rs`): unprivileged user namespaces when the kernel allows them, else
`cef/chrome-sandbox` owned by root with mode 4755 (`sudo chown root:root cef/chrome-sandbox && sudo chmod 4755
cef/chrome-sandbox`), else a refusal naming both remedies; the Web Browser window then offers "Run without the sandbox
for this workspace" (`browser.allowNoSandbox`), the only way besides `ELUDITE_CHROME_NO_SANDBOX=1` (tests, CI) that the
shell passes `--allow-no-sandbox`. Running as root always needs that opt-in. Chromium reads `CHROME_DEVEL_SANDBOX`
(the helper outside its own folder) only for an engine owned by the user running it, so a root-owned install puts the
helper beside `eludite-chromium` instead; the engine looks there first and says so when it refuses.

**The smoke test** is `browsers/chromium/tests/package.rs` (`cargo test -p eludite-chromium --features cef --test
package`): `linux.sh --profile debug --no-build` into a temporary folder, the tarball's listing, the launcher's keys,
then `eludite --print-engine-discovery` and the engine from the layout (initialize, `engine/ready` with the layout's
CEF, `about:blank`) with no `CEF_PATH`, `ELUDITE_CEF`, `ELUDITE_CHROMIUM`, `LD_LIBRARY_PATH` or CEF cache.
`ELUDITE_PACKAGE_TARBALL=PATH` tests a release tarball instead (CI's step does). It skips when CEF is not cached.

**CI** (`.github/workflows/ci.yml`, Linux job): when the CEF cache was restored, `linux.sh` builds the release tarball
and the smoke test runs against it; otherwise the step is skipped with a notice.

**Not done here:** Flatpak and Snap (their sandboxes restrict user namespaces in their own ways, and Chromium inside
them uses the portal's sandbox: a topic of its own), `.deb` and `.rpm` (installers are Phase 3; an installer would own
the helper's root ownership, which a tarball cannot carry).

## Windows (to write: `windows.ps1`; not run here)

There is no Windows machine in this environment, and `eludite-chromium` builds only as the stub off Linux (brief
0031): the frame ring's Windows transport (named file mappings) and the engine's Windows main are still to write. What
the Windows half of brief E needs, for the owner's machine (also in `docs/briefs/windows-checklist.md`):

- **The sandbox needs CEF's bootstrap.** CEF's Windows sandbox runs only when the process's executable is CEF's own
  `bootstrap.exe` (GUI) or `bootstrapc.exe` (console), which loads the client as a DLL and calls its entry point (the
  `cef` crate's `cefsimple` example shows it). So the engine becomes `eludite_chromium.dll` exporting the entry, and
  the package ships `bootstrapc.exe` renamed to `eludite-chromium.exe` beside it. `--no-sandbox` is never needed on
  Windows (the sandbox works for administrators and standard users alike) and the engine should refuse it there
  outright; `browser.allowNoSandbox` does not apply.
- **The layout:** `eludite.exe`, `eludite-chromium.exe` (the bootstrap) and `eludite_chromium.dll`, and `cef\` with
  `libcef.dll`, `chrome_elf.dll`, `d3dcompiler_47.dll`, `libEGL.dll`, `libGLESv2.dll`, `vk_swiftshader.dll`,
  `vk_swiftshader_icd.json`, `vulkan-1.dll`, `icudtl.dat`, `v8_context_snapshot.bin`, the paks and `locales\`, CEF's
  `LICENSE.txt` and `CREDITS.html`; a zip (an installer is Phase 3). Check whether the bootstrap finds `libcef.dll` in
  `cef\` or needs it beside itself (the DLL search order); if beside, the layout flattens.
- **Discovery** is the same code (`EngineSearch` and `CefSearch` look for `eludite-chromium.exe` and `libcef.dll`).
- `tools/cef/fetch.ps1` fetches the Windows minimal distribution (173 MB) already.

## macOS (to write: `macos.sh`; not run here)

There is no macOS machine either. CEF on macOS loads only from an application bundle (`tools/cef/README.md`, "The
macOS bundle"), so the layout is a nested bundle, built by a script doing what the `cef` crate's `bundle-cef-app`
does:

```
Eludite.app/Contents/
  Info.plist, Resources/eludite.icns
  MacOS/eludite                                         the shell (never loads CEF)
  Frameworks/eludite-chromium.app/Contents/
    MacOS/eludite-chromium                              the engine
    Frameworks/Chromium Embedded Framework.framework    loaded at run time (cef::library_loader), not linked
    Frameworks/eludite-chromium Helper.app              and Helper (GPU), Helper (Renderer), Helper (Plugin),
                                                        Helper (Alerts): five helper apps, each a small executable
                                                        that loads the framework and runs a subprocess
```

- The helpers are a second `[[bin]]` of `browsers/chromium`; each helper's `Info.plist` sets `LSUIElement` and the
  bundle id suffix macOS picks the helper by.
- With the sandbox, each helper calls `cef_sandbox_initialize` from `libcef_sandbox.dylib` first
  (`cef::sandbox::Sandbox`); the macOS sandbox needs no setuid helper and no opt-in.
- Discovery on macOS: the engine at `../Frameworks/eludite-chromium.app/Contents/MacOS/eludite-chromium` relative to
  `eludite` (to add to `EngineSearch`), and CEF inside the engine's bundle.
- A notarized release signs every helper and the framework with the hardened runtime and the entitlements CEF's
  documentation lists for each helper (JIT and unsigned executable memory for the renderer, among others; to check
  against CEF 154's notes on the owner's machine); a `.dmg` is Phase 3. The frame ring's macOS transport (`shm_open` over the same socket) is still to write.
