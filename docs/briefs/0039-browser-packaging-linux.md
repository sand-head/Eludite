# Brief 0039: Packaging the browser engine: the Linux layout, the sandbox rule and the opt-in

Status: in progress
Phase: 2 (proposal 0002, brief E, the Linux half)
Plan reference: PLAN.md sections 2 (principles 2, 3, 4), 4.9, 10 (Phase 2), 12 (`browsers/chromium`, `tools/cef`), 13 (installers are Phase 3; this brief lays the files out, it does not sign or install); proposal 0002 sections 3, 8 (E), 11 (download size and trust); brief 0031's report sections 1, 6 (finding 8), 10 (deviation 10) and 11 (packaging)
Related ADRs: ADR-0008
Depends on: brief 0032 (the window and the engine selection), brief 0031 (the sandbox rule, `tools/cef/fetch.sh`). Runs after 0032 merges.

## Goal

A built Eludite runs its browser engine from a predictable layout without a developer's environment variables: `eludite-chromium` and CEF's runtime files beside the `eludite` executable, found by the shell first, then by `ELUDITE_CHROMIUM` and `CEF_PATH`, then by the fetch script's cache. `tools/package/linux.sh` builds that layout (a tarball with the shell, the engine, CEF's runtime files, the sandbox helper, a `.desktop` file and an icon) and the engine starts from it. The Linux sandbox rule becomes Chromium's own: unprivileged user namespaces when the kernel allows them, else the setuid `chrome-sandbox` helper beside `libcef.so`, else a refusal that names both remedies; `--no-sandbox` only after an explicit, per-workspace opt-in in a Visual Studio-style dialog (or the setting `browser.allowNoSandbox`, or brief 0023's variable for tests), audited and shown in the window's strip while the engine runs that way. The Windows and macOS halves (the Windows bootstrap, the macOS nested bundle with its helper apps, signing) are written up and listed for the owner's machines, not run here.

## Files in scope

- `protocol/schemas/` first and alone: `settings.json` (`browser.allowNoSandbox`, default off, per workspace; `browser.enginePath`), `browser-rpc/engine-ready.json` (`sandbox`: `namespaces`, `helper`, `none`), `browser-tabs.output.json` or `browser-state` (`engine.sandbox`), `agents-policy.json` unchanged.
- `browsers/chromium/src/sandbox.rs` and `lib.rs` (the decision: user namespaces available (`/proc/sys/kernel/unprivileged_userns_clone` absent or 1 and `/proc/sys/user/max_user_namespaces` above 0, probed by `unshare(CLONE_NEWUSER)` in a forked child), else the helper owned by root with mode 4755 beside `libcef.so`, else refused with the message naming `sudo chown root:root chrome-sandbox && sudo chmod 4755 chrome-sandbox` and the opt-in; root keeps needing the opt-in as brief 0031 found; `--no-sandbox` only with `--allow-no-sandbox` on the engine's command line, which the shell passes only after the opt-in or the variable), `browsers/chromium/README.md` (the rule as shipped), `tools/cef/README.md` (the Linux section rewritten to the rule; the Windows and macOS sections name what brief E's other half does there).
- `crates/browser/src/embedded.rs` and `discovery.rs` (new: `EngineSearch`: `browser.enginePath`, `ELUDITE_CHROMIUM`, `eludite-chromium` beside the current executable, cargo's build layout from the repository root in a dev build; `CefSearch`: CEF's files beside the engine (`libcef.so` in the same folder), `CEF_PATH`, `tools/cef/fetch.sh`'s cache by the version in `tools/cef/PIN`; each probe says where it looked), `crates/eludite/src/shell/browser.rs` and `browser_window.rs` (the opt-in dialog: "Chromium's sandbox cannot start on this machine" with the two remedies and "Run without the sandbox for this workspace" as a check box, Visual Studio-style; refusing leaves the engine off and the window shows the message; the strip reads "Browser running without Chromium's sandbox" while it does; the engine-missing message names `tools/cef/fetch.sh` and the package layout), `crates/eludite/src/shell/settings.rs` (the two settings).
- `tools/package/linux.sh` (new): `cargo build --release` of `eludite` and `eludite-chromium` with the `cef` feature, the layout `eludite-<version>-linux-<arch>/` (`eludite`, `eludite-chromium`, `cef/` with `libcef.so`, `chrome-sandbox`, `*.pak`, `locales/`, `icudtl.dat`, `v8_context_snapshot.bin`, `libEGL.so`, `libGLESv2.so`, `libvk_swiftshader.so`, `vk_swiftshader_icd.json`, `libvulkan.so.1`, CEF's `LICENSE.txt` and Chromium's credits beside it, `eludite.desktop`, `icons/`), a `README` in the tarball naming the sandbox rule, and the tarball; `tools/package/README.md` (the layout, what Windows and macOS need and why they are not run here); `.github/workflows/ci.yml` (a Linux job step that runs `tools/package/linux.sh` and the smoke test below, only when CEF is cached in CI as brief 0031 arranged, else skipped with a notice).
- `browsers/chromium/tests/package.rs` or `crates/browser/tests/package.rs` (new): the smoke test on the layout; `browsers/chromium/src/sandbox.rs` tests (the decision table).
- `docs/briefs/windows-checklist.md` (brief E's Windows items: the bootstrap, `--no-sandbox` is never needed there, the layout under `tools/package/windows.ps1` to write; a new "macOS checklist" section or file for the nested bundle and the five helper apps), `README.md` (how to run a built Eludite with the browser), `CLAUDE.md` (the `tools/package/` row), `docs/briefs/README.md`, `docs/briefs/0039-report.md` (new).

## Contract

- **Discovery order.** The engine: the setting, the variable, beside the executable, the dev layout; CEF: beside the engine, `CEF_PATH`, the cache; the first found wins and `state` says which; nothing is probed at startup, only when the window or a browser command first needs the engine.
- **The sandbox rule.** Namespaces or the helper run the engine sandboxed with no question; neither, or root, refuses with the message, and the dialog offers the opt-in once per workspace (stored in the workspace's settings as `browser.allowNoSandbox: true`; the Options dialog shows it under Browser so it can be turned off); `ELUDITE_CHROME_NO_SANDBOX=1` keeps working for tests and CI and is the only other way; the engine's command line carries `--allow-no-sandbox` only then, and the engine refuses `--no-sandbox` from anywhere else. The audit log records the opt-in (`eludite.browser.engine_start` with `sandbox: none`), and the strip shows it while the engine runs.
- **The layout.** `tools/package/linux.sh` produces a tarball that runs on a clean checkout of the same distribution family without `CEF_PATH`, `ELUDITE_CHROMIUM` or the cache; the engine finds CEF beside itself; the shell finds the engine beside itself; the `.desktop` file and icon follow the freedesktop spec; the tarball lists its licenses (GPL-3.0-or-later for Eludite, CEF's BSD-3-Clause, Chromium's credits).
- The shell loads nothing of CEF; the engine's stdout is protocol only; no network at startup; the package script never downloads during a build when the cache is present (it calls `tools/cef/fetch.sh`, which is a no-op then).
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `browsers/chromium`: the decision table (namespaces yes, helper yes or no; namespaces no, helper setuid root; helper present but not setuid; neither; root; the opt-in flag) with the probes faked; `--no-sandbox` never appears without `--allow-no-sandbox`.
- `crates/browser`: `EngineSearch` and `CefSearch` orders with temporary folders; the message when nothing is found names the fetch script.
- `crates/eludite` headless tests (fake engine): the refusal shows the dialog, declining leaves the engine off with the message in the window, accepting stores the setting, restarts the engine with the flag, writes the audit entry and shows the strip; the setting off again removes the flag at the next start.
- The smoke test: `tools/package/linux.sh` into a temporary folder (CEF cached, Xvfb), then the built `eludite-chromium` from the layout opens `about:blank` and answers `engine/ready` with the layout's CEF (no `CEF_PATH` in its environment), and the built `eludite` with `--print-engine-discovery` (a hidden flag for this test) reports the engine beside it. Here it runs as root with the variable; the report says so.
- The Xvfb run (`crates/eludite/tools/browser-sandbox-linux.sh`): the dialog's screenshot and the strip's.

## Budget

- The layout adds nothing to cold start (discovery is on first use, measured: under 1 ms when found beside the executable).
- The tarball's size is reported (CEF's minimal distribution dominates; proposal 0002 section 11 names it).
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`, `cargo test --workspace --features eludite-chromium/cef` green with the smoke test running under Xvfb here; `dotnet build` and `dotnet test` unchanged.
2. The report gives the decision table as shipped, the tarball's contents and size, the discovery timings, and the Windows and macOS lists for the owner.
3. `CLAUDE.md`, `README.md`, `tools/cef/README.md`, `browsers/chromium/README.md`, the checklists and the briefs index match the repository.

## Out of scope

- Signing, installers, auto-update (Phase 3); Flatpak and Snap (user namespaces there are their own topic; named in the report).
- The Windows bootstrap and the macOS bundle runs (listed, not run); the accelerated offscreen path.
- Bumping CEF or GPUI.
