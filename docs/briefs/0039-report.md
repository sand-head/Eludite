# Brief 0039 report: Packaging the browser engine on Linux: the layout, discovery, the sandbox rule and the opt-in

Status: done on Linux (Xvfb, software rendering, run as root in a container). Windows and macOS: written up, not run
(no machines; section 9). CI: not run (nothing pushed).
Branch: `brief/0039-browser-packaging-linux`, based on `main` at `de04b34`; not rebased (the coordinator merges).
Date: 2026-10-04.
Brief: [0039-browser-packaging-linux.md](0039-browser-packaging-linux.md).

## 1. Summary

- **A built Eludite runs its engine with no variables.** `tools/package/linux.sh` makes
  `eludite-<version>-linux-<arch>/` and its tarball: `eludite`, `eludite-chromium`, CEF's runtime files in `cef/`, the
  setuid helper as CEF ships it, `eludite.desktop`, icons, `README`, `LICENSE`, `THIRD-PARTY-CRATES.txt`. The release
  tarball is 198 MB (627 MB unpacked; section 4). From it, `eludite --print-engine-discovery` finds the
  engine beside itself and CEF in `cef/`, and the engine loads `cef/libcef.so` through its `$ORIGIN/cef` run path,
  answers `engine/ready` with the layout's CEF and opens `about:blank`, with no `CEF_PATH`, `ELUDITE_CEF`,
  `ELUDITE_CHROMIUM`, `LD_LIBRARY_PATH` or CEF cache (the smoke test, section 6).
- **Discovery** (`crates/browser/src/discovery.rs`: `EngineSearch`, `CefSearch`): the engine at
  `browser.enginePath`, `ELUDITE_CHROMIUM`, beside the executable, cargo's build layout (debug builds only); CEF beside
  the engine (its folder or `cef/`), `ELUDITE_CEF`/`CEF_PATH`, the fetch script's cache. Each miss names where it
  looked, the fetch script and the package. Searched on first use only (a test proves nothing is searched at startup):
  3.5 µs p50 beside the executable in the unit test, 36 to 61 µs (five runs) and 79 µs from the unpacked tarball for the packaged release binary's whole first call
  (section 5).
- **The sandbox rule is Chromium's** (`browsers/chromium/src/sandbox.rs`): user namespaces, else the setuid helper,
  else a refusal naming both remedies; root always refused. `--no-sandbox` only with `--allow-no-sandbox` on the
  engine's command line, which the shell passes only for the workspace's opt-in (`browser.allowNoSandbox`) or
  `ELUDITE_CHROME_NO_SANDBOX=1`. All three modes ran for real on this machine (section 3).
- **The opt-in**: the refusal opens a Visual Studio-style dialog, "Chromium's sandbox cannot start on this machine",
  with the remedies and "Run without the sandbox for this workspace" (once per workspace); OK with the box checked
  stores the setting in the workspace's `.eludite/settings.json`, opens the tab again with the flag, audits
  `eludite.browser.engine_start` (`sandbox: none`) and shows the strip "Browser running without Chromium's sandbox".
  The Xvfb run did it end to end against the real engine as root
  ([dialog](../../crates/eludite/screenshots/linux-browser-sandbox-dialog.png),
  [strip](../../crates/eludite/screenshots/linux-browser-sandbox-strip.png)).
- **This machine:** root in a Firecracker VM, kernel 6.18.44, Ubuntu 24.04 userland;
  `/proc/sys/kernel/unprivileged_userns_clone` absent, `/proc/sys/user/max_user_namespaces` 64301, unprivileged user
  namespaces work (`unshare -U` as the `ubuntu` user succeeds; no AppArmor restriction). As root the engine needs the
  opt-in (or `ELUDITE_CHROME_NO_SANDBOX=1`, which the full test run sets).

## 2. What was built (commits)

| Commit | What |
|---|---|
| `4172fd1` | The brief's Status line: in progress |
| `9d30e5d` | Schemas first and alone: `settings.json` `browser.enginePath` and `browser.allowNoSandbox` (with `x-eludite-scope: solution`, documented in the file's description), `browser-rpc/engine-ready.json` `sandbox` (`namespaces`, `helper`, `none`) and `cefDir`, `initialize.json`'s `sandbox` description, `browser-tabs.output.json` `engine.sandbox`, `engine.cef`, `engine.found_by`; `browser-rpc.md`'s command line and a section on the sandbox. `agents-policy.json` unchanged |
| `c3ed310` | The engine's decision (`sandbox.rs`), `--allow-no-sandbox`, exit code 5 for a refusal, `engine/ready`'s mode and CEF folder, the default CEF folder `cef/` beside the engine (`cef_dir_beside`), run path `$ORIGIN:$ORIGIN/cef`; engine tests pass the switch instead of the variable |
| `e492828` | `EngineSearch` and `CefSearch`; `ChromiumSearch` composed of them; `EngineConfig::allow_no_sandbox`; the refusal as `EngineEvent::SandboxRefused`; `LaunchInfo` and `tabs` with sandbox, CEF and `found_by`; the shell: lazy search with `browser.enginePath`, the opt-in, the dialog, the strip, the audit entry, the two settings, the Options dialog writing a `solution`-scoped setting in the workspace's file; the headless tests |
| `2222216` | The hidden `eludite --print-engine-discovery` |
| `6701d3e` | `tools/package/linux.sh`, `README.md`, the `.desktop` file, the icons (SVG, 48 and 256 PNG), the tarball's README; `browsers/chromium/tests/package.rs` |
| `8a32b60` | CI's package step (and the tests honoring `ELUDITE_CHROME_NO_SANDBOX=1` as the shell does) |
| `f2e417f` | `crates/eludite/tools/browser-sandbox-linux.sh` and `browser_sandbox.py`, the screenshots, [the run's JSON](0039-run/browser-sandbox.json) |
| `9c8a2be` | `browsers/chromium/README.md`, `tools/cef/README.md` (and `fetch.sh`'s header), `README.md`, `CLAUDE.md`, the Windows checklist and a macOS checklist |
| (last) | This report, the brief's Status, the index row |

## 3. The sandbox rule as shipped

Decided before CEF starts (`browsers/chromium/src/sandbox.rs`, `decide`, a pure function of the probes and the
command line):

| Probes | without `--allow-no-sandbox` | with `--allow-no-sandbox` |
|---|---|---|
| `--no-sandbox` on the command line | refused (exit 5, names `--allow-no-sandbox`) | `none` |
| running as root | refused: run as a normal user, or the opt-in | `none` |
| user namespaces available | `namespaces` | `namespaces` |
| no namespaces, helper setuid root where Chromium uses it | `helper` | `helper` |
| no namespaces, helper present but not setuid root | refused: both remedies, the `chown`/`chmod` command for it | `none` |
| no namespaces, helper missing | refused: both remedies | `none` |
| no namespaces, helper setuid root beside `libcef.so` only, engine owned by another user | refused: put the helper beside the engine | `none` |

- **Namespaces** = `kernel.unprivileged_userns_clone` absent or 1, `user.max_user_namespaces` above 0, and a forked
  child's `unshare(CLONE_NEWUSER | CLONE_NEWPID | CLONE_NEWNET)` succeeding. The brief asked for `CLONE_NEWUSER`
  alone; the PID and network namespaces are added because they are what Chromium's zygote creates, and Ubuntu 23.10's
  AppArmor restriction lets the user namespace be created but takes its capabilities, so `CLONE_NEWUSER` alone would
  say yes where Chromium then dies with "No usable sandbox!". Not verifiable here (no AppArmor); worth a run on Ubuntu
  24.04 as a normal user (CI's runners are such a machine).
- **The helper** is looked for beside the engine first and then beside `libcef.so`. Chromium itself looks beside its
  executable, and reads `CHROME_DEVEL_SANDBOX` (which the engine sets to the helper it chose) only when the executable
  belongs to the user running it. In the package the helper is in `cef/` as the brief says; that works when the user
  unpacked the tarball, and the README tells a root-owned install to put it beside `eludite-chromium` (the "unusable"
  row says the same).
- **Proven here** with the probes faked (`the_decision_table`, `no_sandbox_needs_the_allow_switch`, exhaustively:
  `none` never comes out without the switch) and for real: as root, refused without the switch and `none` with it; as
  the unprivileged `ubuntu` user, `namespaces` (a tab opened, so the renderer started inside the namespace sandbox); as
  `ubuntu` inside a user namespace whose `max_user_namespaces` is 0, refused with the plain helper cargo copies and
  `helper` once that helper was `chmod 4755` (a tab opened; the mode was restored after). The last two were manual runs
  with a small script, not tests.
- **The shell** passes `--allow-no-sandbox` when `ELUDITE_CHROME_NO_SANDBOX=1` is in its environment, when
  `browser.allowNoSandbox` is true (the effective value; the dialog and the Options dialog write it in the workspace's
  file), or after the dialog's OK until that setting applies; turning the setting off forgets the dialog's opt-in too.
  The engine reads no variable for the sandbox any more and the shell removes `ELUDITE_CHROME_NO_SANDBOX` from its
  environment.

## 4. The package

The release build (`tools/package/linux.sh`, `cargo build --release`, thin LTO): 12 min 26 s for the build on this
loaded machine (another agent was building), 12 min 53 s in all; `eludite-0.1.0-linux-x86_64.tar.gz` is 207,414,470
bytes (198 MB, gzip), 252 entries; unpacked 657,699,258 bytes (627 MB).

| File | Bytes | |
|---|---|---|
| `eludite` | 79,278,864 | the shell, release |
| `eludite-chromium` | 1,694,024 | the engine (links `libcef.so`) |
| `cef/libcef.so` | 465,000,784 | CEF 154.0.32 / Chromium 154.0.8037.58, DWARF stripped by `fetch.sh` |
| `cef/locales/*.pak` | about 49 MB | 220 locales |
| `cef/resources.pak`, `chrome_100_percent.pak`, `chrome_200_percent.pak` | 23,500,705; 582,159; 944,864 | |
| `cef/libvk_swiftshader.so`, `libvulkan.so.1`, `vk_swiftshader_icd.json` | 14,907,216; 1,533,216; 107 | software Vulkan for ANGLE |
| `cef/icudtl.dat`, `cef/v8_context_snapshot.bin` | 10,819,840; 757,576 | required |
| `cef/chrome-sandbox` | 31,680 | the setuid helper, mode 755 as shipped |
| `cef/CREDITS.html`, `cef/LICENSE.txt` | 8,679,939; 1,662 | Chromium's credits, CEF's BSD-3-Clause |
| `LICENSE`, `README`, `THIRD-PARTY-CRATES.txt` | 35,149; 2,827; 18,507 | |
| `eludite.desktop`, `icons/hicolor/{48x48,256x256,scalable}/apps/eludite.*` | 337; 853, 2,200, 264 | |

The smoke test passed against this tarball (`ELUDITE_PACKAGE_TARBALL`, unpacked into a temporary folder): discovery
`beside`/`beside` in 79 µs, the packaged engine answered `initialize` 199 ms after its start, `engine/ready` named the
layout's `cef/`, `about:blank` opened; as root, with `--allow-no-sandbox` (`sandbox: none`). The debug layout of the
same test (what `cargo test` packages) is 920 MB laid out and 253 MB as a tarball (the debug `eludite` is 374 MB) and
takes about 40 s to make. The layout and tarballs were deleted after the measurements.

`libEGL.so` and `libGLESv2.so`, which the brief lists, are not in CEF 154's Linux minimal distribution (ANGLE is
inside `libcef.so`); the script copies them when a CEF version ships them. The `.desktop` file follows the Desktop
Entry Specification 1.5 (`Type`, `Name`, `Exec=eludite %F`, `TryExec`, `Icon=eludite`, `Categories=Development;IDE;`,
`StartupWMClass=eludite`, GPUI's app id); `Exec` is relative, so the README gives the one `sed` that installs it with
the absolute path (a tarball cannot know where it will be unpacked; `desktop-file-validate` is not installed here). The
icons follow the hicolor theme layout (48x48 PNG, the minimum the Icon Theme Specification asks of an application,
256x256 PNG and a scalable SVG; the repository had no application icon, so this one is new and simple). Licenses:
`LICENSE` (GPL-3.0, Eludite is GPL-3.0-or-later), `cef/LICENSE.txt` (CEF, BSD-3-Clause), `cef/CREDITS.html`
(Chromium's third-party licenses), and `THIRD-PARTY-CRATES.txt`, the 547 Rust crates linked into the two
executables with their SPDX expressions (from `cargo tree --offline`); the crates' full license texts are not copied
(an installer's job, Phase 3, noted in section 10). The script calls `tools/cef/fetch.sh` (a no-op with the cache) and
never downloads during the build; it refuses an engine built without the `cef` feature.

## 5. Discovery timings

| What | Time |
|---|---|
| `EngineSearch` + `CefSearch` on a layout in a temporary folder, beside the executable (unit test, 200 runs) | 3.5 µs p50, 48 µs max |
| `eludite --print-engine-discovery` from `target/debug` (the whole search, `current_exe` included) | 173 µs |
| The same from the debug layout in the smoke test (the machine loaded by another agent's build) | 4.8 ms |
| The same from the release layout | 36 to 61 µs (five runs); 79 µs from the unpacked tarball in the smoke test |
| `the_first_engine_search` in the shell's headless test (`nothing_is_searched_for_at_startup`) | printed by the test |

Nothing is searched at startup: `BrowserBus` makes its `ChromiumSearch` on the first `status()` (the Web Browser window
opening, a browser command, or the Debug menu's Open in Web Browser Window being drawn), and the test asserts that
after the shell's start nothing was searched. So the layout adds nothing to cold start.

## 6. Tests

| Where | What it proves |
|---|---|
| `browsers/chromium/src/sandbox.rs` (5) | The decision table row by row with the probes faked; `--no-sandbox` never without `--allow-no-sandbox` (exhaustive over the probes); the sysctl rule; the helper probe on real files (missing, a plain copy, setuid root beside CEF and beside the engine, as root); what this machine says |
| `browsers/chromium/src/lib.rs` (2) | CEF beside the engine or in `cef/` (cargo's layout first); `--allow-no-sandbox` among CEF's switches |
| `browsers/chromium/tests/engine.rs` (+1, +assertions) | The real engine refuses `--no-sandbox` without the switch (exit 5, before CEF starts) and, as root, refuses without it naming both remedies; `engine/ready` carries the mode (`none` as root) and the CEF folder |
| `browsers/chromium/tests/package.rs` (1) | The smoke test: `linux.sh --profile debug --no-build` into a temporary folder; every file of the layout; the licenses, the README's sandbox rule, the launcher's keys; the tarball's listing under one folder; `eludite --print-engine-discovery` from the layout (engine and CEF `beside`); the engine from the layout with no CEF variable, cache or library path: initialize, `engine/ready` with the layout's `cef/`, `about:blank` through CDP, shutdown with exit 0. Runs here as root with `--allow-no-sandbox` and says so. `ELUDITE_PACKAGE_TARBALL` tests a release tarball instead (CI) |
| `crates/browser/src/discovery.rs` (2) | `EngineSearch` (setting, variable, beside, dev; a named but missing engine is an error, not the next place) and `CefSearch` (beside or `cef/`, the variables, the cache) with temporary folders; the messages name every place, the fetch script and the package; the timing |
| `crates/browser/src/embedded.rs` (+1, changed) | The package layout found through `ChromiumSearch` with `found_by`; `engine/ready` sets the sandbox `info` reports; the engine-missing message names the fetch script and the package |
| `crates/browser/tests/embedded.rs` (+assertions) | Against the real engine: `tabs` reports `engine.sandbox` (`none` as root), `engine.cef` and `engine.found_by` |
| `crates/commands` (settings, browser) | The two settings, their kinds, defaults and scopes (only `browser.allowNoSandbox` is `solution`); a `tabs` sample with the new members validates against the schema |
| `crates/eludite/src/shell/browser_window_tests.rs` (+3) | Nothing is searched at startup; the refusal opens the dialog with both remedies, unchecked; Escape declines: engine off, the message in the window, nothing stored, and a reopen is refused again without a second dialog; Space and Enter accept: the setting in the workspace's file (as the person's audited command), the engine started with the opt-in (the fake saw `allow_no_sandbox` false then true), the audit entry `eludite.browser.engine_start` `{"sandbox": "none", "allowed_by": "browser.allowNoSandbox"}`, the strip drawn; the setting off again: the next start has no opt-in and is refused |
| `crates/eludite/src/shell/settings_tests.rs` (+1) | The Options dialog writes `browser.allowNoSandbox` in the workspace's file, never the user's, and turns it off again |
| `crates/eludite/src/args.rs` | The hidden flag parses and is not in the usage text |
| `crates/eludite/tools/browser-sandbox-linux.sh` (Xvfb, by hand) | The real dialog and strip, as root against the real engine: refused 1.7 s after the menu click, Space and Enter, the engine started without the sandbox, `browser.allowNoSandbox: true` in the workspace's file |

**The full run** (`cargo test --workspace --no-fail-fast --features eludite-chromium/cef` under Xvfb, with
`ELUDITE_DBG_MONO`, the test corpus built, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`, Node 22, `ELUDITE_JS_DEBUG`
and `ELUDITE_TEST_SSHD`, after `dotnet build dotnet/Eludite.slnx` with 0 warnings): 1007 passed, 1 failed, 1 ignored.
The failure was the known load-sensitive `the_options_dialog_is_generated_from_the_schema_and_edits_through_the_bus`
("the dialog follows the file", a file-poll timing; another agent was building), which passes alone. The smoke test
ran in it (the debug layout: 925 MB, tarball 254 MB, 36 s). `cargo fmt --check` and `cargo clippy --workspace
--all-targets --features eludite-chromium/cef -- -D warnings` are clean.

## 7. Budget

- Cold start: unchanged; nothing of the engine or CEF is searched at startup (test above).
- Discovery: well under 1 ms beside the executable (section 5).
- The tarball: 198 MB; CEF dominates (`libcef.so` 465 MB of the 627 MB unpacked, after `fetch.sh`'s
  strip), as proposal 0002 section 11 expects.
- No new dependency.

## 8. Deviations and decisions

1. **`discovery.rs` existed** (brief 0023's `ChromeSearch`); `EngineSearch` and `CefSearch` are added to it.
   `ChromiumSearch` stays, as the pair, so its callers keep their shape; two tests outside the brief's files
   (`crates/eludite/src/shell/debug/tests.rs` and the window tests) built an empty one by fields and now use
   `ChromiumSearch::default()`.
2. **CEF's order changed** from brief 0031's (variables, cache, beside) to the brief's (beside, variables, cache), so a
   package never picks a developer's `CEF_PATH`. In a development build the engine in `target/debug` has CEF beside it
   (cargo copies it), so nothing changes there.
3. **The engine no longer reads `ELUDITE_CHROME_NO_SANDBOX`**; the shell and the tests turn it into
   `--allow-no-sandbox`. Files outside the brief's list changed for that: `browsers/chromium/tests/engine.rs`,
   `crates/browser/tests/{chrome,embedded}.rs` (one field in their configurations), `crates/browser/src/{browser,
   chrome,engine}.rs`, `crates/commands/src/browser.rs` (`EngineRow`), `crates/eludite/src/{args,main}.rs`,
   `crates/eludite/src/shell/{browser_tests,browser_view,settings_tests}.rs`.
4. **`--allow-no-sandbox` permits, it does not force**: the engine still sandboxes where it can (so a workspace's
   opt-in on a machine with namespaces changes nothing).
5. **`x-eludite-scope`** is a new schema keyword (`SettingSpec::scope`): the Options dialog wrote every setting in the
   user file, where `false` would never override a workspace's `true`.
6. **`engine/ready` carries the mode**, as the brief asks; it is sent only once the DevTools port answers, so the
   shell also treats `initialize`'s `sandbox: false` as `none` (the strip and the audit need no more).
7. **The dialog is offered once per workspace per session**; after Cancel the window shows the message, which names
   Tools > Options > Web Browser. With no workspace open, OK opts in for the session only (there is no workspace file).
8. **The smoke test packages the test's own debug build** (`--profile debug --no-build`) rather than making a release
   build inside `cargo test`; CI's step makes the release tarball and runs the same test against it. The test checks the
   tarball's listing and runs from the layout folder (unpacking a second copy would double the disk it needs).
9. **The probe adds `CLONE_NEWPID | CLONE_NEWNET`** to the brief's `CLONE_NEWUSER` (section 3).
10. **No generated bindings** exist for `settings.json`, `browser-rpc/` or the `tabs` output (they are hand-typed
    with schema checks in tests, as before); nothing was generated or hand-edited.

## 9. Windows and macOS, for the owner

The lists are in [windows-checklist.md](windows-checklist.md) ("Brief 0039" and "macOS checklist") and
[tools/package/README.md](../../tools/package/README.md). In short:

- **Windows:** the engine's Windows main and the ring's named file mappings first; then the sandbox through CEF's
  `bootstrapc.exe` (renamed `eludite-chromium.exe`) loading the engine as `eludite_chromium.dll`; `--no-sandbox` never
  needed and to be refused there (`browser.allowNoSandbox` inert); `tools/package/windows.ps1` with the layout and a
  zip; check where the bootstrap expects `libcef.dll`; port the smoke test and add a CI step.
- **macOS:** the `shm_open` transport; the helper as a second `[[bin]]`; `tools/package/macos.sh` for
  `Eludite.app/Contents/Frameworks/eludite-chromium.app` with the framework and five helper apps; `EngineSearch` and
  `CefSearch` taught the bundle; signing with the hardened runtime and CEF's entitlements, notarization.

## 10. Open points

1. **A committed `.eludite/settings.json` can opt a workspace out of the sandbox.** The brief stores the opt-in in the
   workspace's settings file, which a team may commit; a cloned repository with `browser.allowNoSandbox: true` then
   runs the engine unsandboxed on a machine where the sandbox cannot start, without the dialog (the strip and the
   audit entry still show it). Suggested follow-up: keep the opt-in in the user's own state keyed by the workspace's
   path, or honor the workspace file's value only after the person confirmed it once.
   **Resolved by [brief 0047](0047-report.md):** the opt-in now lives in the person's state for the workspace and a
   value in `.eludite/settings.json` is ignored with a warning.
2. **Ubuntu's AppArmor restriction** is handled by the probe in theory, untested here (section 3).
3. **The helper's location**: the brief puts it beside `libcef.so`; Chromium prefers it beside the executable. Shipped
   as the brief says, with the "unusable" case detected and explained; an installer (Phase 3) should put it beside the
   engine.
4. **Third-party license texts** of the Rust crates are listed, not copied.
5. **CI disk**: the release build in the package step adds several GB on the hosted runner after the debug build;
   watch the first run.
6. **Flatpak and Snap** (out of scope): their sandboxes restrict user namespaces and offer their own; Chromium inside
   them uses the portal's sandbox.

## 11. How to reproduce

```
export CEF_PATH="$(tools/cef/fetch.sh)"
tools/package/linux.sh --out /tmp/pkg                               # release layout and tarball; size on stderr
ELUDITE_PACKAGE_TARBALL=/tmp/pkg/eludite-0.1.0-linux-x86_64.tar.gz \
  cargo test -p eludite-chromium --features cef --test package -- --nocapture
cargo test -p eludite-chromium --features cef --test package -- --nocapture   # the debug layout
crates/eludite/tools/browser-sandbox-linux.sh /tmp/sandbox-run      # as root, or where the sandbox cannot start
/tmp/pkg/eludite-0.1.0-linux-x86_64/eludite --print-engine-discovery
```
