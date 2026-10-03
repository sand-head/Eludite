# Brief 0037 report: Launch integration: F5 on a web project opens it in the Web Browser window

Status: done on Linux (Xvfb, software rendering). Windows and macOS: not run (out of scope). CI: not run (nothing
pushed). netcoredbg: not available here, so F5 under the debugger ran only against the fake adapter; Ctrl+F5 ran with
the real `dotnet` and the real embedded engine. Branch: `brief/0037-browser-launch-integration`, based on `main` at
`72916ba`. Date: 2026-10-03. Brief: [0037-browser-launch-integration.md](0037-browser-launch-integration.md).

## 1. Summary

- **F5 and Ctrl+F5 on an ASP.NET Core project open its page** as Visual Studio does: the selected launch profile's
  `launchBrowser` decides, its `launchUrl` is joined to the first application url (`ASPNETCORE_URLS` it sets, else
  `applicationUrl`; http before https unless the profile is named `https`), and the page opens once the server is up:
  Kestrel's `Now listening on:` line in the program's output, or the url answering any HTTP status, within 30 s.
  Otherwise the Output window's Debug source says the page could not be opened and the session goes on.
  ([page](../../crates/eludite/screenshots/linux-web-launch-page.png),
  [after Restart](../../crates/eludite/screenshots/linux-web-launch-restarted.png),
  [after Stop](../../crates/eludite/screenshots/linux-web-launch-stopped.png).)
- **Where:** in the Web Browser window (a tab of the embedded engine, opened through the bus with
  `eludite.browser.tab_open`, the caller being the session, audited with its arguments) when Debug > Open in Web
  Browser Window (the setting `browser.useBuiltIn`, on by default) is on and the engine is found; else in the system
  browser (`eludite.browser.open_external`). Debug > Start in External Browser starts with `browser: external`.
  `eludite.debug.start` takes `browser` (`built_in`, `external`, `none`); `debugger.launchBrowser` off stops F5 from
  following `launchBrowser`. Rust and console projects open nothing.
- **The tab is the session's:** `eludite.debug.state`'s `session.browser` and the stop summary's `browser` carry
  `tab`, `url`, `engine` (`embedded` or `system`) and `state` (`waiting`, `opened`, `failed`); a start with `wait_ms`
  answers once the page opened, so an agent gets the tab id for `eludite.browser.*` in the same answer.
  `eludite.browser.tabs` names a launched tab's `session` (id and project); the Web Browser window draws it with a
  green run glyph and the project in its tooltip. Restart navigates the same tab (a reload when the url is unchanged),
  Stop leaves it open, closing it leaves the session running.
- **The UI thread never waits:** the launch profile is read, the engine looked for, the development certificate
  checked (`dotnet dev-certs https --check`, only for an https page) and the server waited for on the launch thread
  (a `debug-browser` thread after a restart through the adapter); the UI thread only applies the step's messages.
- **Budgets:**

  | Budget | Measured | |
  |---|---|---|
  | Page opened within 500 ms of the listening line (fake session) | **0.7 to 0.8 ms** (F5, fake adapter and fake engine; Ctrl+F5 with a stand-in `dotnet`: 0.8 to 1.0 ms) | Pass |
  | The readiness poll costs the UI nothing | The wait runs on the launch thread; the UI applies one message per state change | Pass |
  | No new dependency | `Cargo.lock` unchanged | Pass |

  Real runs (section 4): Kestrel's line to the page in the embedded engine 448 to 702 ms in the shell test and 659 to
  897 ms in the Xvfb runs with the engine starting cold (brief 0032 measured its cold start at about 456 ms), **144 to
  344 ms** on a restart with the engine running (a reload that waits for the page's load event).
- **Tests:** `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` with CEF, Xvfb, Chrome for Testing,
  `ELUDITE_CHROME_NO_SANDBOX=1`, `ELUDITE_DBG_MONO` and the Test Explorer corpus built: **815 passed, 0 failed, 1
  ignored** (a doc example); the netcoredbg tests (this brief's one and the older ones) return early. Section 6 lists
  what each new test proves; section 8 the checks.

## 2. What was built

| Commit | What |
|---|---|
| `5f962ba` | The brief's Status line |
| `5d1c274` | Schemas first and alone: `debug-start.input.json`'s `browser`; `debug-state.output.json`'s `$defs/browser` and `session.browser`; `debug-stop-summary.output.json`'s `browser`; `settings.json`'s `browser.useBuiltIn` and `debugger.launchBrowser`; `browser-tabs.output.json`'s `session`; `command-invoke.json`'s caller kind `session` |
| `3154bb7` | `crates/dap/src/launch.rs`: `launchBrowser` and `launchUrl` on `LaunchProfile`; `BrowserLaunch`, `browser_launch_from` (the rules), `browser_launch` (a launch configuration's, reading the profile and the SDK), `join_url`, `browsable` (wildcard hosts as `localhost`), `listening_url`, `https_choice` with `DEV_CERT_MESSAGE` and `dev_cert_found`, `ServerWatch` (the output's listening line, `wait_up`, `rearm`, `cancel`) and `probe` |
| `49e1105` | `crates/ui/src/menu.rs`: Debug > Start in External Browser; the `SettingCheck` entry (a check item on a boolean setting, dispatching `eludite.settings.set`) for Debug > Open in Web Browser Window; per-item enabling (`set_item_enabled`) for items that share a command |
| `e084059` | `corpus/web/minimal-api` (MIT) and its row in `corpus/README.md` |
| `25f2a5a` | The shell: the launch's browser step (`shell/debug.rs`), the session's page in the model and the summary (`debug/state.rs`), the session's tabs in the browser worker and the window (`shell/browser.rs`, `browser_window.rs`), the settings and the menu wiring; `Caller::Session` (`crates/commands`); the headless tests and the real Ctrl+F5 test |
| `8fb9f8b` | The reload after a restart through the adapter, tested; the trace names how the server came up |
| `a19b22a` | The corpus entry's README names its users |
| `f30e774` | `crates/eludite/tools/web-launch-linux.sh`, the Xvfb run |
| `bf82ad4` | The trace names every page opened (also when the url answered before Kestrel's line); the script reads its timings from it |
| `fe53994` | The run's screenshots (`crates/eludite/screenshots/linux-web-launch-*.png`) |
| (last) | This report, the brief's Status line and the briefs index |

## 3. The design

- **The plan** (`launch::browser_launch`, on the launch thread): the selected profile (the start's `profile`, else the
  first with `commandName: Project`, as before) and whether the project is `Microsoft.NET.Sdk.Web`. Nothing to browse
  (no profile url, no `launchBrowser`, not a web project; a Cargo package) gives no plan, so Rust and console projects
  are untouched whatever the start says. The page is `launchUrl` when absolute, else joined to the chosen base with one
  `/` (`""` is the root, `http://localhost:5180/`). A web project without any url (Kestrel's default address) opens
  `launchUrl` joined to the address Kestrel names.
- **The choice** (`plan_browser`): the start's `browser` wins; without it the profile must ask (`launchBrowser`) and
  `debugger.launchBrowser` be on, and `browser.useBuiltIn` picks the window or the system browser. The window needs
  the embedded engine (`BrowserBus::status()`, as the window itself asks); without it the page opens in the system
  browser and the Output line says why.
- **The session's row** goes out with `Launched` (`state: waiting`), so an agent's start never answers between the
  program running and the page being known: `debug_settled(start)` waits while the page is `waiting` and the program
  runs (a break or the end answers at once, as before).
- **The wait** (`ServerWatch::wait_up`): the program's output feeds the watch from the Ctrl+F5 readers and from the DAP
  client's sink (`output` events of category stdout, stderr or console, before they reach the UI); a listening line
  wakes the wait at once; otherwise the url is probed (a GET with a 1-second cap, any status counts; for https an
  accepted connection counts, as there is no TLS client in the build) every 100 ms until the timeout. Stop cancels the
  wait (`ServerWatch::cancel`); the row then says the session ended first.
- **The page** (`run_browser_step`, `open_in_window`), with `with_caller(Caller::Session { session, name })`:
  `tab_open` (which starts the engine when needed and waits for the load), then `eludite.view.show web_browser`. On a
  restart the tab the session's page opened in (`browser_reuse`) is looked up in `tabs`: `navigate` with `reload` when
  it shows the url, else with the url, then `tab_select`; a closed tab gets a new one. The system browser is
  `open_external`. Each outcome is a `DebugMsg::Browser` with its Output line and, for the budget, the time since the
  listening line.
- **Restarts:** a stop-and-start restart keeps the session (brief 0028) and passes the tab to the next launch; a
  restart through the adapter's `restart` re-arms the watch and runs the step again on a `debug-browser` thread with the
  url the launch resolved (the https rule is not asked again).
- **The https rule:** only an https page asks `dotnet dev-certs https --check`, once per launch, on the launch thread;
  without the certificate the http page of the same profile opens (when it has one) and the Output window gets Visual
  Studio's message ("This project is configured to use SSL. To avoid SSL warnings in the browser you can choose to
  trust the self-signed certificate that ASP.NET Core has generated: run `dotnet dev-certs https --trust`." and which
  page opened). Tests set the answer (`LaunchBrowserSettings::dev_cert`), so they do not depend on this machine's
  certificate (it has one).
- **The browser worker** now receives each call's caller (`Job::Apply`), keeps which tabs sessions opened (a session's
  `tab_open` or `navigate` makes the tab its own; `tab_close` forgets it; a new engine forgets them all), fills
  `tabs`' `session`, and announces the tabs to the window before the caller's answer, so the window, opened right after
  a session's `tab_open`, never adds a blank tab (`BrowserBus::has_tabs`).
- **The Debug menu:** Start in External Browser is enabled while no session runs (as Start Without Debugging is in
  Visual Studio while debugging); Open in Web Browser Window is enabled while the engine is found (looked for at most
  once a second while the menu draws) and checked by `browser.useBuiltIn`, which a click sets through
  `eludite.settings.set`.

## 4. The real runs

`crates/eludite/tools/web-launch-linux.sh OUT_DIR` (Xvfb :99, lavapipe, debug build, the real `eludite-host` and
MSBuild, Ctrl+F5 because netcoredbg is not here; the other worktree's builds raised the load average in runs 1 and 2):

| Step | Run 1 | Run 2 | Run 3 (screenshots) |
|---|---|---|---|
| 1-minute load average before | 15.2 | 15.6 | 5.0 |
| Ctrl+F5 to the build's success (the host's MSBuild) | 4,068 ms | 4,607 ms | 3,143 ms |
| Ctrl+F5 to the server up | 4,456 ms | 4,999 ms | 3,386 ms |
| Ctrl+F5 to the page opened | 5,353 ms | 5,756 ms | 4,045 ms |
| Kestrel's line to the page opened (engine cold) | 897 ms | 757 ms | 659 ms |
| Window open to the first page pixel (the engine started by the tab, before the window) | 83 ms | 126 ms | 69 ms |
| Ctrl+Shift+F5 to the page reloaded (built again) | | 3,193 ms | 2,668 ms |
| Kestrel's line to the page reloaded (engine running) | | 190 ms | 144 ms |

Between runs 2 and 3, one run's restart found the url answering before Kestrel's line was read (the probe won the
race): the page reloaded (the Output line said so) but the trace then had no "page opened" line, so the script waited
in vain. The trace now names every opening (`bf82ad4`); the run after that fix reloaded 344 ms after the line.

The shell test `ctrl_f5_on_the_corpus_web_project_opens_the_page_in_the_embedded_engine` (no build: the test builds
the copy first) measured Ctrl+F5 to the page 695 to 966 ms, Kestrel's line to the page 448 to 702 ms, and
`read_page` saw the text box "Name" and the buttons "Greet" and "What time is it?".

## 5. Files outside the brief's list, and other deviations

1. **`crates/commands`**: `Caller::Session` (`caller.rs`; the brief's "caller `session`") with `keeps_arguments`, used
   by the audit (`registry.rs`) so the session's calls keep their arguments as an agent's do; `BrowserChoice`,
   `SessionBrowser`, `browser` on `start`, `SessionRow` and `StopSummary` (`debug.rs`); `TabSession` and `TabRow`'s
   `session` (`browser.rs`); the settings test's key list (`settings.rs`). The schemas say what these carry.
2. **`crates/browser/src/browser.rs`**: one line (`session: None` in `TabRow`; the shell fills it).
3. **`crates/eludite/src/shell.rs`** (the menu's per-item enabling hook, the debugger's bus and browser) and
   **`shell/settings.rs`** (applying the two settings).
4. **`command-invoke.json`** gained the caller kind `session` beside the brief's schemas (same schema-only commit).
5. **`browser.useBuiltIn` is its own setting**, although brief 0032's report suggested reusing `browser.engine`: the
   two answer different questions (where F5 opens a web project's page; which engine the browser tools drive), and
   the brief names `browser.useBuiltIn`. It takes effect only while the embedded engine is found.
6. **`debugger.launchBrowser` is a boolean** (on: follow the profile's `launchBrowser`; off: open nothing unless the
   start names `browser`).
7. **The tab-open answer has no `session`**: only `tabs` names it, so `browser-tab-open.output.json` is unchanged.
8. **F5 inside the Web Browser window is Reload** (brief 0032's key scope), and opening the page gives the window the
   keys, so after F5 the next F5 reloads the page rather than starting or continuing; Start Debugging from the menu,
   or F5 with the focus in an editor, behaves as before. Visual Studio's own launch moves the focus to the external
   browser, so this matches it, but the owner may prefer that the launch not take the keys (one line in
   `BrowserWindow::set_open`); the headless tests start through the command after the first page opened.
9. **No `results/` file**: the numbers are in this report and the script prints them (`OUT_DIR/web-launch.json`).
10. **https probes** count an accepted connection (no TLS client in the build; adding one would be a new dependency);
    Kestrel's line is the usual signal anyway.

## 6. Tests

| Where | Test | What it proves |
|---|---|---|
| `crates/dap/src/launch.rs` | `browser_launches_follow_the_profile_as_visual_studio_does` | Visual Studio's template profiles; with and without `launchUrl` (and with a leading slash); `applicationUrl` lists, http first, `https` first in the `https` profile, https-only lists; `ASPNETCORE_URLS` set by the profile winning, wildcard hosts browsable; an absolute `launchUrl`; `launchBrowser: false` not requested; a web project without a url; a console program has no plan |
| | `urls_join_and_kestrels_line_names_its_address` | The join rule; `browsable`; Kestrel's line with its log prefix and `[::]` |
| | `the_https_profile_needs_the_development_certificate` | The certificate is asked only for https; without it the http page and Visual Studio's message; https-only keeps https with the warning; a missing `dotnet` has no certificate |
| | `browser_launch_reads_the_projects_sdk_and_profile` | From real files: the `http` profile chosen first, the `https` profile's page and its http alternative; a console project and a Cargo package give none |
| | `the_server_is_up_on_its_listening_line_or_any_http_answer` | A 404 and a 500 count; https counts on connect; a refused url gives no answer within the cap; the line fed in pieces from another thread ends the wait within 500 ms; `rearm`; the timeout; `cancel` from another thread; a never-ending line keeps a bounded tail |
| `crates/commands` | `parses_and_validates_every_command`, `outputs_follow_their_schemas` (both crates' extended), `caller_is_scoped_and_restored`, the settings key list | `browser` parses, wrong values refused; `session.browser`, the summary's `browser` (opened, waiting, failed) and `tabs`' `session` against their schemas; `Caller::Session` serializes as `{"kind":"session",...}` and keeps arguments; the two settings default on |
| `crates/ui/src/menu.rs` | `the_debug_menu_starts_in_the_external_browser_and_checks_the_web_browser_window` | The items' order, Start in External Browser's arguments and no key, the check item dispatching the other value of `browser.useBuiltIn` |
| `crates/eludite/src/shell/debug/tests.rs` (fake adapter, fake engine) | `f5_on_a_web_project_opens_its_page_in_the_web_browser_window` | F5: the tab in the fake engine with the url, the state's `session.browser`, the window open with one tab, the glyph's session and the tooltip, `tabs`' `session`, the audit's session caller with arguments, the Output line, the 500 ms budget; Ctrl+Shift+F5 reloads the same tab; a restart with another `launchUrl` navigates it; Stop leaves it (still the session's); a new start opens its own tab; closing it leaves the session running |
| | `the_start_chooses_where_the_page_opens_and_says_when_it_cannot` | A console project opens nothing (F5 and `browser: built_in`); `browser: none`; an agent's start with `wait_ms` answers with the tab; Start in External Browser runs the opener with no tab; `browser.useBuiltIn` off does the same; `debugger.launchBrowser` off opens nothing; the readiness timeout's Output line with the session running; a url answering without the line; the `https` profile without the certificate opening the http page with the message |
| | `ctrl_f5_on_a_web_project_opens_its_page_when_the_program_listens` | Ctrl+F5's stdout feeds the watch (a stand-in `dotnet` printing Kestrel's line); Stop leaves the page |
| | `a_restart_through_the_adapter_reloads_the_same_tab` | DAP `restart`: the step again on its own thread, the same tab reloaded, the same session |
| | `the_debug_menus_browser_items_follow_the_session_and_the_engine` | Open in Web Browser Window disabled without the engine and enabled with it, checked by the setting and unchecked after a click's `settings.set`; Start in External Browser enabled in design mode, disabled while a session runs, enabled again after Stop |
| | `ctrl_f5_on_the_corpus_web_project_opens_the_page_in_the_embedded_engine` (real) | The corpus copied and built with the real `dotnet`, Ctrl+F5, Kestrel's line in the Debug source, the page in the real embedded engine, `read_page` seeing the form, `page_text` the title; Stop leaves the tab, still the session's |
| | `f5_on_the_corpus_web_project_under_netcoredbg_opens_the_page` (real) | The same under netcoredbg: **skipped here** (netcoredbg not found) |
| `crates/eludite/src/shell/browser_tests.rs` | (`PageEngine`, `install_page_engine`) | The fake engine the launch tests use: tabs with pages, `navigate`, `reload` with its load events |

Final run: section 8.

## 7. What brief D (0038, JavaScript debugging) needs

- **The tab as a js-debug target.** `session.browser.tab` names the command tab; the engine's target id is the
  `TabRow`'s `target` (not in the output). js-debug attaches to a CDP websocket: brief 0032's DevTools bridge (one per
  DevTools tab) generalized to an endpoint per tab on request is still the smallest step. The `debug-browser` step's
  "opened" message (`DebugMsg::Browser` with `state: opened`) is the moment to attach.
- **The compound entry.** "Start the server, then attach js-debug to the page" is a compound whose second entry waits
  for the first session's page: the attach can be started from the same step (after `open_in_window`) with the tab,
  as a child session whose `parent` is the server's (brief 0028's section 9). The tab's `session` in `tabs` already
  ties the page to the server's session; the child would be listed by `eludite.debug.sessions` and end with it, while
  the tab stays.
- **Restart** reloads the same tab; a js-debug child attached to it must survive the reload (CDP keeps the target) or
  be re-attached when the step reports `opened` again.
- **Breakpoints by language** (brief 0028's note) become necessary as soon as a `.ts` breakpoint reaches netcoredbg.
- **The guide** (`docs/agents/debugging.md`) does not mention `browser` yet; one paragraph (start with `wait_ms`, read
  `browser.tab`, then `eludite.browser.read_page`) fits there.

## 8. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings` | clean |
| `dotnet build dotnet/Eludite.slnx` | 0 warnings, 0 errors (no .NET file of the solution changed) |
| `corpus/web/minimal-api/build.sh` | builds, 0 warnings |
| `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` (CEF, Xvfb :99, Chrome for Testing 154, `ELUDITE_CHROME_NO_SANDBOX=1`, `ELUDITE_DBG_MONO`, the Test Explorer corpus built; load average 3 to 4) | **815 passed, 0 failed, 1 ignored** (a doc example). An earlier full run at load average 15 to 22 (the other worktree building) had 3 failures, each passing alone at once: `attach_and_restart_against_eludite_dbg_mono` (an empty process listing), `toggle_breakpoint_answers_compactly_and_a_null_reads_null` (a binding not yet answered) and `eludite-chromium`'s `select_popups_cursors_and_the_context_menu` (a cursor within 20 s); none tests this brief's code |
| `dotnet test dotnet/Eludite.slnx` | 201 tests: 194 passed, 0 failed, 7 skipped. One earlier run under load had `MonoAdapterTests.An_unqualified_enum_name_resolves_in_a_condition_a_tracepoint_and_evaluations` fail, and so did one run of that project alone at load average 10; no .NET file changed on this branch (`git diff 72916ba -- dotnet debuggers` is empty) |

## 9. How to reproduce

```
export CEF_PATH="$(tools/cef/fetch.sh)" ELUDITE_CHROME_NO_SANDBOX=1 DISPLAY=:99
cargo test -p eludite-dap --lib launch
cargo test -p eludite -- web_project the_start_chooses a_restart_through_the_adapter the_debug_menus_browser --nocapture
cargo build -p eludite -p eludite-chromium --features eludite-chromium/cef
cargo test -p eludite -- corpus_web --nocapture   # the real Ctrl+F5 (needs dotnet and the engine built above)
dotnet build dotnet/Eludite.slnx
crates/eludite/tools/web-launch-linux.sh /tmp/web     # screenshots and web-launch.json
```
