# CEF (the Chromium Embedded Framework)

`eludite-chromium` (`browsers/chromium`, brief 0031, ADR-0008) is CEF's browser process. CEF is fetched by the
scripts here, pinned and checked by SHA-256 in [`PIN`](PIN), never vendored, never committed and never downloaded at
startup.

```
export CEF_PATH="$(tools/cef/fetch.sh)"                    # Linux, macOS: download, verify, unpack; print the folder
$env:CEF_PATH = (tools\cef\fetch.ps1)                       # Windows
cargo build -p eludite-chromium --features eludite-chromium/cef
```

- **Version:** CEF 154.0.32+g682c378+chromium-154.0.8037.58, the one the `cef` crate 154.3.0+154.0.32 (Apache-2.0
  OR MIT, tauri-apps/cef-rs) is generated against. Update the crate and the pin together.
- **Where:** `~/.cache/eludite/cef/<version>/` (`CEF_CACHE` replaces `~/.cache/eludite/cef`), laid out as the `cef`
  crate's build script expects (download-cef's layout: `Release/` and `Resources/` flattened into one folder with
  `include/`, `cmake/`, `libcef_dll/` and an `archive.json` naming the archive; `archive.json` is written last, so its
  presence means the folder is complete).
- **Sizes** (minimal distributions): Linux 326 MB to download, 558 MB unpacked; Windows 173 MB; macOS arm64 132 MB,
  x64 139 MB. The Linux `libcef.so` ships with DWARF debug information (1.45 GB); `fetch.sh` strips it with
  `strip --strip-debug` (465 MB, the symbol table kept for backtraces) unless `CEF_KEEP_DEBUG=1`. The checksum is the
  download's.
- **Building:** `eludite-chromium` builds the real engine only with its `cef` feature, and then needs `CEF_PATH`: the
  `cef-dll-sys` build script reads the folder, links `libcef`, and copies the runtime files into `target/<profile>/`
  beside the executable (about 530 MB on Linux). Without `CEF_PATH` that build script downloads CEF by itself into
  `target/`, unchecked against `PIN`; set it. Without the feature (the default, so `cargo build --workspace` never
  downloads anything) the executable only explains how to get CEF, and the engine tests skip.
- **Finding it at run time** (brief 0039, `crates/browser/src/discovery.rs`), the first time the Web Browser window or
  a browser command needs the engine, never at startup: the engine at the setting `browser.enginePath`, then
  `ELUDITE_CHROMIUM`, then beside the `eludite` executable (a packaged Eludite), then cargo's build folder in a
  development build; CEF beside the engine (`libcef.so` in the engine's folder, as cargo copies it, or in `cef/` beside
  it, as `tools/package/linux.sh` lays it out), then `ELUDITE_CEF` and `CEF_PATH`, then this cache. The first found
  wins and `eludite.browser.tabs` says which (`engine.found_by`). The shell starts the engine with `--cef-dir` and puts
  that folder on the library path; the engine's `$ORIGIN:$ORIGIN/cef` run path finds it without either.
- **Packaging:** `tools/package/linux.sh` builds the release layout and tarball from this cache (see
  [`tools/package/README.md`](../package/README.md)).

## What the minimal distribution holds

| Part | Linux | Use |
|---|---|---|
| CEF core | `libcef.so` | Chromium and CEF's C API (`cef_*`), which the `cef` crate calls directly on Linux and Windows (no C++ wrapper is built there) |
| Sandbox helper | `chrome-sandbox` | The setuid helper for Chromium's Linux sandbox on systems without unprivileged user namespaces (below) |
| Data | `icudtl.dat`, `v8_context_snapshot.bin` | Required: Unicode data and V8's startup snapshot, beside `libcef.so` |
| Resources | `resources.pak`, `chrome_100_percent.pak`, `chrome_200_percent.pak`, `locales/*.pak` (220 files) | Strings, images, the DevTools front end; only the configured locales are needed |
| SwiftShader | `libvk_swiftshader.so`, `libvulkan.so.1`, `vk_swiftshader_icd.json` | Software Vulkan for ANGLE when the GPU is disabled or missing (canvas, WebGL, 3D CSS) |
| SDK | `include/`, `libcef_dll/`, `cmake/`, `CMakeLists.txt` | Headers and the C++ wrapper (built by `cef-dll-sys` on macOS only) |
| Licenses | `LICENSE.txt`, `CREDITS.html` | CEF is BSD-3-Clause (The Chromium Embedded Framework Authors); Chromium is BSD-3-Clause with the third-party licenses listed in `CREDITS.html` (also `about:credits`) |

No proprietary codecs: H.264 and AAC media do not play.

## The Linux sandbox

Chromium's renderer sandbox on Linux uses unprivileged user namespaces when the kernel allows them, and falls back to
the setuid `chrome-sandbox` helper (owned by root, mode 4755) otherwise; distributions that restrict user namespaces
(Ubuntu 23.10 and later through AppArmor, Debian kernels with `kernel.unprivileged_userns_clone=0`, some hardened
kernels) need the helper. Chromium never sandboxes root.

`eludite-chromium` follows the same rule (brief 0039; the table is in
[`browsers/chromium/README.md`](../../browsers/chromium/README.md#the-sandbox-brief-0039)): user namespaces when they
work, else the helper where Chromium will use it (beside the engine, or beside `libcef.so` for an engine owned by the
user running it), else it refuses to start with a message naming both remedies (allowing user namespaces, or `sudo
chown root:root chrome-sandbox && sudo chmod 4755 chrome-sandbox`) and the opt-in. Running as root is refused the same
way. `--no-sandbox` happens only with `--allow-no-sandbox` on the engine's command line, which the shell passes only
after the workspace's opt-in (the Web Browser window's dialog, stored as `browser.allowNoSandbox` in the workspace's
settings) or with `ELUDITE_CHROME_NO_SANDBOX=1` in its environment (brief 0023's variable, for tests and CI). The
package ships the helper in `cef/` as CEF does; installing it (root ownership and the setuid bit) is the user's or an
installer's step (Phase 3).

## Windows (brief E's other half; not built here)

The engine is Linux-only so far. On Windows CEF's sandbox runs only inside CEF's `bootstrap.exe`/`bootstrapc.exe`,
which loads the client as a DLL, so the engine becomes `eludite_chromium.dll` behind a renamed bootstrap, and
`--no-sandbox` is never needed there; the frame ring uses named file mappings. `tools/package/README.md` lists the
layout and `docs/briefs/windows-checklist.md` the steps for the owner's machine.

## The macOS bundle

CEF on macOS loads only from an application bundle laid out as CEF's `GeneralUsage` describes and the `cef` crate's
`bundle-cef-app` builds (`cef::build_util::mac`):

```
Eludite.app/Contents/
  MacOS/eludite                                   the shell (never loads CEF)
  Frameworks/eludite-chromium.app/Contents/       the engine: CEF's main app bundle
    MacOS/eludite-chromium
    Frameworks/Chromium Embedded Framework.framework
    Frameworks/eludite-chromium Helper.app            (and Helper (GPU), (Renderer), (Plugin), (Alerts))
      Contents/MacOS/eludite-chromium Helper           each a small executable: load the framework, run the subprocess
```

- The framework is loaded at run time (`cef::library_loader::LibraryLoader`: `../Frameworks/...` from the main
  executable, `../../..` from a helper), not linked; the helpers are separate executables (a second `[[bin]]`), since
  macOS picks a helper by its bundle name and `Info.plist` (`LSUIElement`, bundle id suffixes).
- With the sandbox, each helper calls `cef_sandbox_initialize` from
  `Chromium Embedded Framework.framework/Libraries/libcef_sandbox.dylib` first (`cef::sandbox::Sandbox`).
- A development run from `cargo run` needs a bundling step (`bundle-cef-app` or a script doing the same), and a
  notarized release needs every helper signed with the hardened runtime and CEF's entitlements. That is brief E's
  macOS half (`tools/package/README.md`, "macOS", and the macOS checklist in `docs/briefs/windows-checklist.md`); no
  macOS machine has run any of it.

## The switches that keep it off the network (brief 0032)

CEF 154 runs Chrome's browser process with Chrome's background services, which call Google with a fresh profile and
no page open. `eludite-chromium` turns each one off before CEF starts (`browsers/chromium/src/privacy.rs`; the table
of what each request was and what stops it is in [`browsers/chromium/README.md`](../../browsers/chromium/README.md)):

| Switch, feature or preference | Why |
|---|---|
| `--disable-features=NetworkTimeServiceQuerying` | network time (`clients2.google.com/time`) |
| `--disable-features=AimEnabled,AimServerEligibilityEnabled,AimServerRequestOnStartupEnabled` | AI Mode's eligibility check (`www.google.com/async/folae`) |
| `--disable-features=PreconnectToSearch`, `net.network_prediction_options: 2` | the preconnect to the search engine (`www.google.com`) |
| `--disable-features=DnsOverHttpsUpgrade`, `dns_over_https.mode: "off"` | the DNS-over-HTTPS probe (`dns.google`) |
| `--gaia-config-contents={"urls":{"list_accounts_url":{"url":"data:,"}}}` | the accounts check (`accounts.google.com/ListAccounts`), which no switch or policy stops in 154: account consistency is off (`signin.allowed_on_next_startup: false`, read at startup), yet the account service still lists the cookie jar's accounts, so its one url points off the network |
| `--disable-component-update`, `--component-updater=url-source=data:,` | the component updater (`update.googleapis.com`), which still checks on demand with the first switch alone |
| `--disable-background-networking`, `--disable-sync`, `--disable-default-apps`, `--no-pings`, `--disable-domain-reliability`, `--disable-client-side-phishing-detection`, `--disable-breakpad`, `--disable-field-trial-config`, `--metrics-recording-only` | defense in depth: Chrome's other background fetches, sync, reporting and field trials |
| `safebrowsing.enabled: false`, `signin.allowed: false`, managed policies (`BrowserSignin: 0`, `SyncDisabled`, `ComponentUpdatesEnabled: false`, `SafeBrowsingProtectionLevel: 0`, ...) | Safe Browsing's list updates, sign-in and sync, written to the profile and to `<profile>/Policies/managed/eludite.json` |

`--disable-chrome-login-prompt` keeps Chrome's own authentication dialog away (the shell asks instead). The proof is
`browsers/chromium/tests/engine.rs`'s `the_engine_makes_no_request_on_about_blank`: 10 s on `about:blank` with
`--log-net-log`, no request, host resolution or connection in the log.
