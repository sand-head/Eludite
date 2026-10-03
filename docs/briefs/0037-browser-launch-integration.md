# Brief 0037: Launch integration: F5 on a web project opens it in the Web Browser window

Status: in progress
Phase: 2 (proposal 0002, brief C)
Plan reference: PLAN.md sections 2 (principles 1, 3, 4), 4.5 (launch profiles), 4.9 (launch: Kestrel for modern projects, browser launch and attach), 5.8, 8 (Debug menu), 10 (Phase 2); proposal 0002 sections 4.4 (launch integration), 8 (C)
Related ADRs: ADR-0008
Depends on: brief 0032 (the Web Browser window and the engine selection), brief 0020 (settings, F5 builds first), brief 0028 (sessions). Runs after 0032 merges.

## Goal

F5 (and Ctrl+F5) on an ASP.NET Core project behaves as Visual Studio: the launch profile's `launchBrowser` and `launchUrl` (or `applicationUrl`) open the site in the Web Browser window once the server answers, in the embedded engine when it is present (`browser.useBuiltIn`, default on when the engine is found) and in the system browser otherwise; "Start in External Browser" stays in the Debug menu. The debuggee's output is watched for Kestrel's "Now listening on" line and the url is polled, so the page opens when the server is up, never on a connection refused; the tab is tied to the session (closing the session leaves the tab, restarting reuses it; the "Agent is driving" strip and the commands of briefs 0023 and 0024 work on it). `eludite.debug.start` gains `browser` (`built_in`, `external`, `none`) and the stop summary names the tab that was opened, so an agent that starts a web project gets the tab id for `eludite.browser.*` calls in the same answer.

## Files in scope

- `protocol/schemas/` first and alone: `debug-start.input.json` (`browser`), `debug-stop-summary.output.json` and `debug-state.output.json` (`session.browser`: `tab`, `url`, `engine`), `settings.json` (`browser.useBuiltIn`, `debugger.launchBrowser` override), `browser-tabs.output.json` (`session` on a tab opened by a launch), `browser-open-external.input.json` unchanged.
- `crates/dap/src/launch.rs` (`launchBrowser`, `launchUrl`, `applicationUrl` and `ASPNETCORE_URLS` into a `BrowserLaunch` the shell acts on; the `https` profile and the dev certificate rule: `dotnet dev-certs https --check` once per session on the launch thread, with the message Visual Studio shows when it is missing), `crates/eludite/src/shell/debug.rs` and `debug/state.rs` (the launch's browser step: watch the Debug output for Kestrel's listening line, poll the url with a 1-second cap per try for up to 30 s off the UI thread, then `eludite.browser.tab_open` through the bus on the session's behalf; Start in External Browser (Debug menu, `browser: external`) runs `eludite.browser.open_external`; the session's tab in the state), `crates/eludite/src/shell/browser.rs` and `browser_window.rs` (a tab opened by a session shows the project's name in its tooltip and a small debug glyph; restarting the session navigates the same tab; `tabs` carries `session`), `crates/ui/src/menu.rs` (Debug > Start in External Browser; the check item Debug > Open in Web Browser Window), `crates/eludite/src/shell/debug/tests.rs` and `browser_tests.rs`.
- `corpus/web/minimal-api/` (new, MIT): a minimal ASP.NET Core project (`Microsoft.NET.Sdk.Web`, one endpoint returning a page with a form and a `/api/time` JSON endpoint, a `launchSettings.json` with `http` and `https` profiles, `launchBrowser: true`, `launchUrl: ""`), used by the tests; `corpus/README.md`.
- `crates/eludite/tools/web-launch-linux.sh` (the Xvfb run: F5 on the corpus project, the page in the window, a screenshot) and screenshots; `docs/briefs/README.md`; `docs/briefs/0037-report.md` (new).

## Contract

- `launchSettings.json`'s selected profile decides: `launchBrowser` true opens `launchUrl` joined to the first `applicationUrl` (or `ASPNETCORE_URLS`), `http` before `https` unless the profile is `https`; `browser` on the start, the Debug menu's check item and `browser.useBuiltIn` choose the engine; `none` opens nothing. The server is "up" when the Debug output shows `Now listening on: <url>` or the url answers any HTTP status within 30 s; otherwise the Output window says the page could not be opened and the session continues.
- The tab is opened through the bus (`eludite.browser.tab_open` with the url, caller `session`), audited like an agent's call; the stop summary of a `start` with `wait_ms` and `state.session.browser` carry the tab id and url. Restart navigates the same tab (`navigate` with `reload` when the url is unchanged); Stop leaves the tab open (Visual Studio's behavior); the tab closing does not stop the session.
- Start in External Browser runs `eludite.browser.open_external` with the same url rule and no tab.
- Rust and console projects are unaffected (`browser: none` implied when no profile has `launchBrowser`).
- The UI thread never waits: the readiness poll runs on the launch thread; the dev-cert check too.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/dap`: `BrowserLaunch` from profiles (with and without `launchUrl`, `applicationUrl` lists, `https` first, `ASPNETCORE_URLS` set by the profile), the url join rule.
- `crates/eludite` headless tests: a fake host and fake adapter session whose Debug output prints a listening line: F5 opens the tab in a fake engine with the url, the state carries it, restart reloads it, Stop leaves it, `browser: none` opens nothing, `browser: external` runs the opener, the readiness timeout writes the Output line; the Debug menu items' enabling.
- Real: the corpus web project under netcoredbg (skips here) and under Ctrl+F5 with the real `dotnet` (runs here): the server starts, the page opens in the embedded engine under Xvfb and `read_page` sees the form; the Xvfb run's screenshot in the report.

## Budget

- Page opened within 500 ms of the listening line (fake session, measured); the readiness poll costs the UI nothing.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`, `cargo test --workspace --features eludite-chromium/cef` green with the Ctrl+F5 real run here; `dotnet build` and `dotnet test` unchanged; `corpus/web/minimal-api` builds.
2. The report records the numbers and what proposal 0002 brief D needs (the tab as a js-debug target, the compound entry).
3. The briefs index and `corpus/README.md` match the repository.

## Out of scope

- IIS Express (Windows), JavaScript debugging (brief D), packaging (brief E), Browser Link-style live reload.
- Windows and macOS runs.
