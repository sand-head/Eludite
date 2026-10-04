# Windows run checklist

What Phase 0 still owes on Windows, in the order to run it on a Windows 11 machine with a real GPU. Each item names the brief, the command and the exit criterion to record in that brief's report under a "Windows" heading.

## Setup (once)

1. Install: Git, Rust via rustup (the repo's `rust-toolchain.toml` picks 1.98.1), .NET SDK 10.0.302 (or let `global.json` roll forward), Visual Studio Build Tools 2022 (or a Visual Studio with the same workloads) with the ".NET Framework build tools", "Web development build tools" and "Desktop development with C++" workloads (Rust's MSVC target needs the C++ linker and the Windows SDK), Node (only to compare against the npx adapter), Claude Code (native installer) logged in.
   Then, from an elevated prompt, turn on long paths, which Roslyn's build requires: `New-ItemProperty HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem -Name LongPathsEnabled -Value 1 -PropertyType DWord -Force`.
2. Clone `https://github.com/sand-head/Eludite` and run `cargo build --workspace` and `dotnet build dotnet/Eludite.slnx` from a PowerShell prompt. Record anything that fails to build; that is finding number one.

## Brief 0001, GPUI shell and docking

- `cd spikes/0001-gpui-shell; cargo run --release` must open the window; exercise drag to dock, tab, float, auto-hide, restore.
- `python tools/bench_all.py --label windows --refresh-hz <monitor Hz>` and `python tools/inject_keys.py` (check its Windows notes first). Record cold start, frame cost p99, keystroke to pixel p99, scroll frame intervals, RSS. GPU, driver and refresh rate go in the report.
- Exit: GO or NO-GO for Windows in `docs/briefs/0001-report.md` section 8.

## Brief 0002, eludite-host with Roslyn

- `tools/roslyn-pin/build.ps1` (untested) must build the language server; fix the script if it does not.
- `bench/roslyn-200/run.ps1` (untested) with 3 cold and 3 warm runs. Record T0 to T3 and peak memory.

## Brief 0003, legacy projects

- `tools/legacy-load/run.ps1` (untested) against the corpus with the Build Tools column; `vswhere` discovery must find MSBuild.
- Compare Compile items against `MSBuild.exe -getItem:Compile` for at least 3 projects.
- The WebForms code-behind completion test must pass.

## Brief 0004, ICorDebug proof (not started; Windows only)

- Read `docs/briefs/0004-icordebug-dap-spike.md` and launch it as an agent on the Windows machine, or in a worktree here with the Windows machine reachable for the runs.

## Briefs 0005 and 0006, agents

- `cargo build --release -p eludite-claude-acp` under `agents/claude-acp`; run the 0005 panel against it; confirm `claude.exe` discovery and that no Node is needed.

## Briefs 0007 to 0009, Phase 1

- CI already builds and tests these on `windows-latest`. Run the manual parts of each report's "Windows" section if the brief has one.

## Brief 0039, packaging the browser engine (brief E's Windows half; not run)

The embedded engine is Linux-only so far (brief 0031): these are what the Windows half needs, in order. `tools/package/README.md` ("Windows") has the details.

- `$env:CEF_PATH = (tools\cef\fetch.ps1)` fetches CEF 154's Windows minimal distribution (173 MB) and checks its SHA-256.
- Write the engine's Windows main and the frame ring's Windows transport (named file mappings, specified in `protocol/schemas/browser-rpc/browser-rpc.md`), then build `eludite-chromium` with `--features eludite-chromium/cef`.
- The sandbox: CEF's Windows sandbox runs only inside CEF's `bootstrap.exe`/`bootstrapc.exe`. Build the engine as `eludite_chromium.dll` with the entry point the bootstrap calls (the `cef` crate's `cefsimple` example), ship `bootstrapc.exe` renamed to `eludite-chromium.exe` beside it, and check that pages run sandboxed as a standard user and as an administrator. `--no-sandbox` is never needed on Windows: make the engine refuse it there, and hide `browser.allowNoSandbox` (or leave it inert).
- Write `tools/package/windows.ps1`: a release build, then `eludite-<version>-windows-x86_64\` with `eludite.exe`, `eludite-chromium.exe`, `eludite_chromium.dll` and `cef\` (`libcef.dll`, `chrome_elf.dll`, `d3dcompiler_47.dll`, `libEGL.dll`, `libGLESv2.dll`, `vk_swiftshader.dll`, `vk_swiftshader_icd.json`, `vulkan-1.dll`, `icudtl.dat`, `v8_context_snapshot.bin`, the paks, `locales\`, `LICENSE.txt`, `CREDITS.html`), `LICENSE`, `README` and `THIRD-PARTY-CRATES.txt`, zipped. Check whether the bootstrap loads `libcef.dll` from `cef\` or needs it beside itself (the DLL search order); flatten the layout if so, and teach `CefSearch` the answer.
- Run `eludite.exe --print-engine-discovery` from the unzipped folder with no `CEF_PATH`, `ELUDITE_CEF` or `ELUDITE_CHROMIUM`: the engine beside `eludite.exe`, CEF beside the engine. Then port `browsers/chromium/tests/package.rs` (it is `cfg(target_os = "linux")` now) and add the CI step on `windows-latest`.
- Open the Web Browser window: a page renders, `eludite.browser.tabs` reports the engine's sandbox, and no sandbox dialog appears.

## Status (2026-10-03)

Run on a Windows 11 Pro laptop (Intel Core Ultra 9 185H, RTX 1000 Ada plus Intel Arc, 143 Hz panel, Visual Studio 2026 Enterprise) from the `windows-run` branch. Each brief's report has the details under "Windows".

| Item | Result |
|---|---|
| Setup: builds | `cargo build --workspace` and `dotnet build` pass after setup fixes: the repo now has a `nuget.config` (a second machine-wide feed fails restore with NU1507 under central package management), and the C++ workload and long paths above were missing from this list. |
| Setup: gates | `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `dotnet test` all pass on Windows. Getting there fixed Unix-only test code (paths, `chmod`, shell-script fakes, 8.3 temp paths, verbatim `\\?\` paths) and two product bugs (LLDB formatter commands got backslash paths; `MergedRoot` needed symlink rights). GitHub Actions never ran these jobs: every recent run on `main` was refused for an account billing problem. |
| 0001 | GO, provisional. Scroll holds 143 Hz, key to present p99 8.9 ms, render to present p99 2.6 ms, RSS 118 MB. Cold start 340 to 360 ms misses the 300 ms budget (about 200 ms in GPUI's platform init). Owed: a startup profile, a human pass of docking drags, and a `SendInput` keystroke tool (`inject_keys.py` is Linux-only). |
| 0002 | Done. `build.ps1` needed four fixes. Load and T2 take about 2x Linux (T2 about 20 s); completion and cancellation match or beat Linux. |
| 0003 | Done. vswhere finds MSBuild; 0 `-getItem` differences on 4 projects; completion test passes with the Build Tools MSBuild; 23 / 29 load cleanly. Owed: a re-run with the web development build tools installed (they were missing here; all 6 misses need `Microsoft.WebApplication.targets`). |
| 0004 | Done on this machine: the e2e test attaches to a .NET Framework 4.8 process, stops on the breakpoint and reads `counter` (attach 34 to 41 ms, hit to `stopped` about 1 ms, 12 MB). Owed: exit criterion 2, the DAP client on a second machine. Only Eludite on Linux or macOS needs that path (the adapter must sit beside the debuggee on Windows); on Windows the IDE and adapter share the machine. |
| 0005, 0006 | The adapter builds and its tests pass after two fixes; it finds `claude.exe` with no Node on `PATH` (session ready 0.8 to 1.0 s). The 0005 panel no longer compiles against `crates/mcp`, so the panel run was not repeated. |
| 0007 to 0029 | Their automated tests pass on Windows as part of the gates above. None of their reports has a manual Windows section. |

## macOS checklist (brief 0039, brief E's macOS half; not run)

No macOS machine has run the embedded engine. `tools/package/README.md` ("macOS") and `tools/cef/README.md` ("The macOS bundle") have the details.

- `export CEF_PATH="$(tools/cef/fetch.sh)"` fetches CEF 154's macOS minimal distribution (arm64 132 MB, x64 139 MB).
- Write the engine's macOS main and the frame ring's macOS transport (`shm_open` over the same socket, as `browser-rpc.md` specifies).
- Add the helper executable as a second `[[bin]]` of `browsers/chromium` (it loads the framework with `cef::library_loader` and runs the subprocess; with the sandbox it calls `cef_sandbox_initialize` from `libcef_sandbox.dylib` first).
- Write `tools/package/macos.sh`: `Eludite.app/Contents/MacOS/eludite` (the shell), `Contents/Frameworks/eludite-chromium.app` holding `MacOS/eludite-chromium`, `Frameworks/Chromium Embedded Framework.framework` and the five helper apps (`eludite-chromium Helper.app`, `Helper (GPU)`, `Helper (Renderer)`, `Helper (Plugin)`, `Helper (Alerts)`, each with its `Info.plist`: `LSUIElement`, the bundle id suffix), `Info.plist` and an icon for `Eludite.app`, the licenses; as the `cef` crate's `bundle-cef-app` does.
- Teach `EngineSearch` the bundle path (`../Frameworks/eludite-chromium.app/Contents/MacOS/eludite-chromium` from `eludite`) and `CefSearch` the framework inside the engine's bundle; run `eludite --print-engine-discovery` from the bundle with no variables.
- Check that pages run sandboxed (the macOS sandbox needs no setuid helper and no opt-in; `browser.allowNoSandbox` does not apply), and port the smoke test.
- Sign every helper, the framework and the app with the hardened runtime and the entitlements CEF documents for each helper, notarize, and check that Gatekeeper opens the app from a quarantined download (the `.dmg` itself is Phase 3).
