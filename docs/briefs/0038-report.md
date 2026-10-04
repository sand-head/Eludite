# Brief 0038 report: JavaScript debugging in the Web Browser window with vscode-js-debug

Status: done on Linux (Xvfb, software rendering), against a fake js-debug everywhere and against the real
vscode-js-debug 1.140.0 under Node.js 22.22.0 here. Windows and macOS: not run (out of scope). CI: not run (nothing
pushed). netcoredbg: not available here, so the F5 compound (Kestrel under the debugger, then the page) ran against the
fake adapters only; with the real adapters the page was debugged after Ctrl+F5 and the Attach to Browser Tab... dialog.
Branch: `brief/0038-javascript-debugging`, rebased onto `main` at `a5cbf32` (briefs 0035, 0037 and 0040 merged). Date:
2026-10-04. Brief: [0038-javascript-debugging.md](0038-javascript-debugging.md).

**GitHub releases were reachable from this container** (the brief expected not): `tools/js-debug/fetch.sh` downloaded
`js-debug-dap-v1.140.0.tar.gz`, the SHA-256 in `tools/js-debug/PIN` is from that download, and every real-adapter test
ran here. Nothing waits on the owner's machine except netcoredbg's twin of the compound test and CI's re-recording.

## 1. Summary

- **A page is a debug target.** `eludite.debug.attach` with `tab` (a Web Browser tab id) or `url` (a page url, or an
  external Chrome's `ws://.../devtools/page/<id>`) starts vscode-js-debug's DAP server on the machine's Node.js and
  attaches it to the tab through the engine's remote debugging port on loopback, by the tab's DevTools target id. The
  browser session is `attached`, named after the tab's title (renamed with it), carries `tab` and `url`, Stop detaches
  and leaves the page running, and the tab closing ends it with `design` and `The tab tN closed.`. `adapter:
  javascript` with `pid` is refused. A tab Eludite opened is `execute`; a url is `dangerous` (the attach rule).
- **Child sessions.** js-debug's `startDebugging` reverse request is answered at once and opens a second connection
  to the same server: the page's session, with `parent`, listed under its parent in `eludite.debug.sessions` and the
  Call Stack's process selector. Breakpoints and exception filters go to the children (which own the scripts); a
  `wait` on the parent ends at the first stop of it or a child; Stop on the parent detaches the children first, then
  the parent once the last child ended (what the real adapter requires: it answers the parent's `disconnect` only then).
- **The compound.** F5 on a web project whose page opens in the Web Browser window (brief 0037) with
  `debugger.attachBrowser` on (the default) attaches js-debug to the opened tab once the page is up; the start's
  answer names both sessions. `compound` entries take `{ "browser": { project?, tab?, url?, web_root? } }` for any
  server entry. The attach never delays the server: without Node or js-debug the server keeps running, and the
  answer's `message` and the Output window say why, naming `tools/js-debug/fetch.sh` or Node's minimum version.
- **Breakpoints by language.** Each breakpoint goes only to the sessions whose adapter claims its file's kind (section
  7); a kind no adapter claims goes to all, as before. `breakpoints[].sessions` names only those sessions; js-debug's
  `verified: false` counts in `breakpoints_failed` unless its message says it is pending
  (`breakpoint.provisionalBreakpoint`, `Unbound breakpoint`).
- **Source maps.** Frames keep the mapped path js-debug gives (`app.ts`): the execution point, the Call Stack and the
  Breakpoints window work on it. When a map on disk maps the frame's line, the frame also carries `source: { original,
  generated, generated_line, generated_column }` (`app.js` line 18 for `app.ts` line 25). A dev server's in-memory
  map (Vite) leaves `generated` out; the frame still names `src/counter.ts`.
- **Exceptions.** Break When Thrown maps to js-debug's `all`, User-Unhandled to `uncaught`; .NET exception types are
  not sent. The Exception Settings window shows a JavaScript Exceptions row while a browser session is live;
  `exception_info` works.
- **The agent's three commands.** `toggle_breakpoint` on the handler's line, `eludite.browser.input` click (which
  answers `paused: true` instead of waiting on a page stopped in the debugger), then `wait` and `variables`.
- **The launch keeps the keyboard focus in the editor** (brief 0037's report, section 5 item 8): a page opened or
  navigated by a debugging session's launch shows the Web Browser window without focusing it, so the next F5 is the
  debugger's, not the window's Reload; the person's own browser commands focus it as before.
- **Debug > Attach to Browser Tab...** opens the Attach to Process dialog filtered to its Web Browser heading (the
  tabs with title and url), the first tab selected so Enter attaches.
- **Discovery.** vscode-js-debug and Node are located, never bundled, and looked for only when a browser session
  starts; `state.sessions[].adapter_version` reads `vscode-js-debug 1.140.0, node v22.22.0`.
- **Budgets:**

  | Budget | Measured | |
  |---|---|---|
  | Attach to the tab after the page is up, to `running`, with js-debug: under 1.5 s | **392 ms** in the shell (the embedded engine, `attach` by tab to the child `running`), **410 ms** in `crates/dap/tests/js_debug.rs` (Node's start included, the external Chrome); the Xvfb run's Enter in the dialog to the child session 349 and 737 ms | Pass |
  | The fake attach adds under 50 ms to brief 0037's launch | **10.5 ms** after the page opened alone, 17.9 ms in the full suite (load average 6 on 4 cores) | Pass |
  | A browser session's `snapshot` under 50 ms p95 (fake) | **5.3 ms** alone, 11.6 ms in the full suite | Pass |
  | Frame p99 unchanged with two sessions (the debugger's share under 8 ms) | the server and the page's child stopping alternately 10/s: frame p99 6.70 ms, the debugger's share p99 **2.75 ms** alone (14.56 and 6.50 ms in the full suite); brief 0028's one and two .NET sessions in the same suite: 8.18 and 8.81 ms, shares 1.88 and 3.24 ms | Pass |
  | No new dependency in the shell | `Cargo.lock` and every `Cargo.toml` unchanged | Pass |

- **Tests:** `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` with CEF, Xvfb, Chrome for Testing, `ELUDITE_CHROME_NO_SANDBOX=1`, `ELUDITE_DBG_MONO`, `ELUDITE_JS_DEBUG`, Node 22 and the Test Explorer corpus built: **911 passed, 1 failed, 1 ignored** (a doc example) on the last run, the failure an engine test from brief 0032 that passed alone and with its whole binary, twice each (section 10); the real js-debug tests ran, the netcoredbg tests (this brief's compound twin among them) return early. Section 6 lists what each new test proves; section 10 the checks.

## 2. What was built

- **Schemas first** (`ff233f3`, alone): `debug-attach.input.json` (`tab`, `url`, `adapter: javascript`, `web_root`),
  `debug-start.input.json` (the `browser` compound entry), `debug-state.output.json` and `debug-sessions.output.json`
  (`adapter: javascript`, `parent`, `tab`, `url`), `debug-stop-summary.output.json` and `debug-stack.output.json`
  (`source.original`, `generated`, `generated_line`, `generated_column`), `debug-processes.output.json` (`tabs`),
  `settings.json` (`debugger.attachBrowser`, `debugger.nodePath`, `debugger.jsDebugPath`),
  `browser-rpc/engine-ready.json` (`remote_debugging_port`), `browser-tabs.output.json` (`target_id`); then
  `browser-input.output.json`'s `paused` (`850d767`, alone) and `dap-js-debug.md`.
- **`tools/js-debug/`**: `fetch.sh` and `fetch.ps1` download the pinned release by checksum into
  `~/.cache/eludite/js-debug/<version>/` and print `src/dapDebugServer.js`; a `PIN` without `sha256` refuses and
  `JS_DEBUG_PIN_FROM_DOWNLOAD=1` prints the line to pin. `PIN` names 1.140.0, its SHA-256 and Node's minimum (18).
- **`crates/dap`**: `JsDebugSearch` and `NodeSearch` with the version check (`discovery.rs`); the TCP adapter server
  (`transport::start_tcp_server`: `node dapDebugServer.js 0 127.0.0.1`, the port read from its first line, one
  connection per session, the first owning the process); `attach::browser_attach` (the `pwa-chrome` arguments) and
  the exception filter mapping; the `startDebugging` reverse request (`client.rs`); `session::AdapterFamily` and the
  file-kind table; `sourcemap.rs` (new: VLQ decoding, the map beside the original found and cached by modification
  time, `adapt_stack`); a fake js-debug server (`fake::listen_js_debug`: child connections, mapped frames, the
  filters, provisional breakpoints); the recorder and replay for a server with several connections (`order`, marks,
  run tokens, `replay::ReplayGroup`).
- **`browsers/chromium`**: the remote debugging port on `127.0.0.1` (a free port chosen by the engine), announced in
  `engine/ready`; each tab's target id. **`crates/browser`**: `Engine::debug_endpoint()`, `tabs`' `target_id` for
  both engines, and `DebuggerPauses`: `input` stops waiting on a tab the shell marks stopped and answers `paused`.
- **`crates/commands`**: the new arguments and outputs, the attach's permission class by tab or url, the settings.
- **The shell** (`debug.rs`, `debug/state.rs`, `debug/windows.rs`, `browser.rs`, `browser_window.rs`): browser and
  child sessions, the compound's browser entry after brief 0037's readiness, breakpoints and exception plans per
  adapter family, the Attach dialog's Web Browser heading, the JavaScript Exceptions row, the tab title and close
  following, the quiet (focus-keeping) launch. **`crates/ui/src/menu.rs`**: Debug > Attach to Browser Tab....
- **Corpus**: `corpus/web/minimal-api/wwwroot/app.ts` with `app.js` and `app.js.map` (TypeScript 5.9.3, the command in
  its README) and an Add button whose `total` skips the first item; `corpus/web/vite-counter/` (Vite 8.3.2, committed
  `package-lock.json`); `corpus/dap/js-debug/`: two scenarios recorded from the real adapter here.
- **Docs**: `protocol/schemas/dap-js-debug.md`, a paragraph in `docs/agents/debugging.md` (1,975 words, under the
  2,000 the MCP test asserts), `crates/eludite/tools/js-debug-linux.sh` and its two screenshots, this report.

## 3. The design

- **One Node process per browser session, one TCP connection per DAP session.** js-debug's standalone server debugs
  each target in a session of its own and asks the client to start it (`startDebugging`); the shell answers on the
  reader thread and runs the child's handshake off the UI thread on a second connection to the same port. The child is
  an ordinary session with `parent`: every `eludite.debug.*` command works on it by `session`, and nothing in the
  command surface is js-debug specific beyond the attach.
- **The engine reports its port, nothing guesses it.** The embedded engine opens its remote debugging port on
  loopback (a free one) and says it in `engine/ready`; the external Chrome's port is the one Eludite started it with.
  The attach names the tab by `targetId` (with `urlFilter` as the fallback) and `targetSelection: automatic`.
- **Stop detaches in two steps**, children then parent (`b1da6f4`): the real adapter answers the parent's
  `disconnect` only after its last child is gone, so a parent-first detach waited out the timeout.
- **The mapped path is the frame's path.** js-debug already resolves frames to the original source under `webRoot`;
  Eludite keeps that path and adds the generated place from the map on disk (read on the client's reader thread), so
  no window learns about source maps and a breakpoint set in `app.ts` round-trips unchanged.
- **The focus.** `BrowserBus::quiet()` is set while a session's caller (`Caller::Session`) opened or navigated the
  page, and the window then skips `focus_page`; any other caller's command clears it.
- **A stopped page answers no input.** CDP's `Input.dispatchMouseEvent` waits for the renderer, which a breakpoint
  holds; `eludite.browser.input` would time out. The shell tells the browser bus which tabs are stopped, and `input`
  answers `paused: true` once the event is sent, so the agent's next call is `wait`.

## 4. The real runs

All on this container: 4 cores, Xvfb with Mesa's software Vulkan, Node.js 22.22.0, vscode-js-debug 1.140.0, CEF 154
(the embedded engine), Chrome for Testing 154.0.8037.92 (the external one), the real `dotnet` 10.0.302.

- **`crates/dap/tests/js_debug.rs`** (the external Chrome on the corpus page served by the test): attach by target id
  to the child running in 410 ms; the breakpoint at `app.ts` line 25 bound through the map, a click stopped
  there (`button#add.onAdd`), the frame's generated place `app.js` line 18, Locals `input`, `price`, `item`.
- **The shell with the embedded engine** (`ctrl_f5_on_the_corpus_web_project_then_js_debug_stops_in_app_ts`): Ctrl+F5
  on the corpus web project with the real `dotnet`, the page in the embedded engine, the agent's `attach` by tab to
  running in 392 ms, the click through `eludite.browser.input` answering `paused: true`, `wait` answering
  the child's stop at `app.ts:25` with `app.js:18`, `variables` with the handler's locals, Stop leaving the page.
- **The Vite counter** (`the_vite_counter_stops_through_the_dev_servers_source_maps`): `npm ci` from the committed
  lock, `vite --port`, a breakpoint in `src/counter.ts` line 5 stopping on a click through the dev server's inline map
  (the test passes; it asserts the stop, not a time to running).
- **The two conformance scenarios** were recorded here with `tools/dap-corpus/record.sh js-debug` and replay through
  the headless shell on every platform (`js_debug_attach_break_step`, `js_debug_logpoints_and_exceptions`);
  re-recorded once more at the end with `DAP_CORPUS_CHECK=1` (what `record.sh --check js-debug` does): both passed with no difference from the checked-in recordings and golden files.
- **The Xvfb run** (`crates/eludite/tools/js-debug-linux.sh`, real X input): F9 in `app.ts`, Ctrl+F5, Debug > Attach to
  Browser Tab... clicked and Enter, a pointer click on the page's Add button: the handler stopped at `app.ts` line 25
  with the Call Stack's selector showing the server and the page's child session and the Locals the handler's
  variables ([stopped](../../crates/eludite/screenshots/linux-js-debug-stopped.png)), then F10 and Shift+F5: the
  sessions ended and the page kept running ([ended](../../crates/eludite/screenshots/linux-js-debug-ended.png)).
  Two runs on the rebased branch (load average 4 to 6, the other worktree building): Ctrl+F5 to the page opened 3.7
  and 5.1 s (the host's build included), Enter in the dialog to the child session 349 and 737 ms and to the breakpoint
  bound 465 and 954 ms, the click to the stop 250 and 443 ms and to the Locals 536 and 738 ms; the sessions ended.
  The screenshots are the second run's.

What the real adapter showed, now in `dap-js-debug.md`: the parent's `attach` is answered only after
`configurationDone` and the `startDebugging` request; the child answers breakpoints set before the script loads with
`verified: false` and `breakpoint.provisionalBreakpoint` (pending, not refused), then binds them with `breakpoint`
`changed` events; scripts with no file are named with modifier letter colons (`127.0.0.1꞉PORT`); the parent's
`disconnect` is answered only after its last child ended; telemetry arrives as `output` events (not shown); a stop
has `allThreadsStopped: false`. These shaped the fake, the replay's grouping across connections, and the Stop order.

## 5. Files outside the brief's list, and other deviations

1. **`crates/dap`**: `sourcemap.rs` (new), `client.rs` (the reverse request), `types.rs` (`generated` on a frame),
   `record.rs` and `replay.rs` (several connections in one order, marks, tokens), `lib.rs`; `tests/replay.rs` (the
   group and mark tests) and one line in `tests/mono.rs`.
2. **`crates/browser`**: `browser.rs`, `browser/act.rs`, `browser/window.rs`, `engine.rs`, `lib.rs` (the debug
   endpoint, `DebuggerPauses` and `input`'s `paused`), `tests/window.rs`.
3. **Protocol**: `debug-stack.output.json` (the frame's `source`, as the summary's), `browser-input.output.json`
   (`paused`), `browser-rpc/browser-rpc.md` (the port).
4. **`crates/commands`**: `browser.rs` (`target_id`, `paused`), `settings.rs` (the key list).
5. **The shell**: `browser.rs`, `browser_window.rs` (the debug target, the tab list, the titles, the focus),
   `browser_tests.rs`, `settings.rs`, `debug/windows.rs` (the JavaScript Exceptions row, the dialog's heading), and
   one-line fixture fields in `agents/tests.rs`, `debug/native_tests.rs`, `test_runs_tests.rs`, `tests.rs`.
6. **`tools/dap-corpus/record.ps1`** (the usage line) and **`corpus/dap/README.md`** (the js-debug rows and rules).
7. **The checksum is pinned** by this brief's agent, not left to the owner: the release was reachable.
8. **The Xvfb run attaches through the dialog after Ctrl+F5**, not F5, without netcoredbg (the script takes F5 when
   `ELUDITE_NETCOREDBG` is set).
9. **Restart of a browser session** is refused, as for any attach (`dap-js-debug.md`, known not to work).
10. **The Call Stack's process selector says modes in words** (`running without debugging`, not the state's
    `running_without_debugging`): after the rebase onto brief 0037's `main` the Ctrl+F5 program is listed beside the
    browser session and its child, and the Xvfb run showed the raw value. Children are indented under their parent.
11. **`DebugMsg::Launched` boxes its `SessionRow`**: the row's new `tab`, `url` and `parent` with brief 0037's
    `plan` crossed clippy's `large_enum_variant` limit after the rebase.

## 6. Tests

| Where | Test | What it proves |
|---|---|---|
| `crates/dap/src/discovery.rs` | `js_debug_search_order_is_setting_cache_beside_eludite` | The setting (file or folder), the cache by the pinned version then the newest, beside the executable; the error names the folders and `fetch.sh` |
| | `node_search_order_is_setting_path_volta_nvm_fnm` | The setting, `PATH`, Volta, nvm's default alias, fnm's two defaults |
| | `the_node_version_is_checked_against_the_release_minimum` | `v18`+ accepted, older and unreadable refused with the minimum |
| `crates/dap/src/transport.rs` | `the_listening_line_names_the_port`, `a_tcp_server_adapter_says_its_port_and_serves_several_connections` | The port from js-debug's first line (a fake `node` script printing it); several connections to one server; the first owns the process |
| `crates/dap/src/attach.rs` | `browser_attach_arguments_from_a_tab_and_a_url`, `exception_settings_map_to_js_debugs_filters` | The `pwa-chrome` arguments from a tab and from a `ws://` url; Thrown to `all`, User-Unhandled to `uncaught`, no types |
| `crates/dap/src/session.rs` | `breakpoints_go_to_the_adapters_that_claim_the_file` | The file-kind table; an unclaimed kind goes to all |
| `crates/dap/src/sourcemap.rs` | `vlq_numbers_decode`, `the_corpus_map_leads_from_app_ts_to_app_js`, `maps_are_found_by_their_sources_and_read_again_when_they_change` | The corpus map: `app.ts:25` to `app.js:18`; the map found by name or by `sources`; re-read after a change |
| `crates/dap/src/record.rs`, `tests/replay.rs` | `tokens_are_scrubbed_and_substituted_back`, `a_mark_holds_what_followed_it_until_the_test_passes_it`, `a_group_keeps_the_order_across_connections` | Run tokens; marks; the order across a server's connections |
| `crates/dap/tests/js_debug.rs` | `start_debugging_starts_a_child_on_a_second_connection` (fake) | The reverse request answered, the child's handshake on a second connection with the configuration unchanged |
| | `the_real_js_debug_stops_in_app_ts_on_a_click` (real) | Section 4; skips without `ELUDITE_JS_DEBUG`, Node or `ELUDITE_CHROME` |
| `browsers/chromium/tests/engine.rs` | `the_remote_debugging_port_is_reported_and_answers_on_loopback_only` | `engine/ready`'s port answers `/json/version` on 127.0.0.1 and not on another address; tabs name their target ids |
| `crates/browser/tests/window.rs` | `input_reports_a_stop_in_the_debugger_instead_of_waiting_on_the_paused_page` | `input` on a paused tab answers `paused: true` at once |
| `crates/commands/src/debug.rs` | `browser_attach_compound_entries_and_outputs` | Parsing of `tab`, `url`, `adapter: javascript` (alone: the dialog filtered to tabs), the browser entry; the refusals (`pid` with `javascript`: "by tab or url"; `tab` with `pid`, `url`, another adapter or a transport); `execute` for a tab, `dangerous` for a url; the outputs against their schemas |
| `crates/ui/src/menu.rs` | `the_debug_menu_attaches_to_a_browser_tab` | The item after Attach to Process..., its arguments, no key |
| `crates/eludite/src/shell/debug/state.rs` | `rows_name_only_the_adapters_that_take_the_file_and_js_gets_its_filters` | `breakpoints[].sessions` by family; js-debug's exception plan |
| `crates/eludite/src/shell/debug/tests.rs` (fake js-debug, fake engine) | `attach_to_a_tab_debugs_the_page_through_a_child_session` | Attach by tab: the session's name, tab, url, versions; the child under its parent; `app.ts` to the child only; the stop's mapped and generated places; Locals; the JavaScript Exceptions row and `uncaught` without .NET types; the Call Stack selector's labels (the child indented, modes in words); Stop on the parent ends both, the page stays; the timing |
| | `an_agent_debugs_the_click_handler_in_three_commands` | The scripted agent: `toggle_breakpoint`, `eludite.browser.input` click answering `paused`, `wait` then `variables`; the snapshot p95 budget |
| | `a_web_project_start_attaches_its_page_after_readiness` | F5 on a web project: the server's session, the page opened, then the attach (after readiness), the summary naming both; a `.cs` breakpoint to the .NET adapter only and `app.ts` to the child only; `webRoot` the project's `wwwroot`; the 50 ms budget; **the launch leaves the focus in the editor and F5 does not reload the page** |
| | `a_missing_js_debug_or_node_leaves_the_server_running_with_the_reason` | No js-debug: the server runs, the message and Output name `fetch.sh`; no Node: the minimum |
| | `the_browser_session_follows_its_tab_and_the_dialog_lists_tabs` | Rename with the title; the tab closing ends the session with the message; attach by `ws://` url; the dialog's Web Browser heading, filtering, the first tab selected, Enter attaching |
| | `a_server_and_its_page_stopping_by_turns_cost_the_frame_little` | Frame p99 with the server and the page's child stopping alternately (brief 0028's assertion) |
| | `ctrl_f5_on_the_corpus_web_project_then_js_debug_stops_in_app_ts`, `the_vite_counter_stops_through_the_dev_servers_source_maps` (real) | Section 4 |
| | `f5_on_the_corpus_web_project_under_netcoredbg_debugs_its_page_too` (real) | The compound with the real adapters; **skipped here (no netcoredbg)** |
| `crates/eludite/src/shell/debug/conformance_tests.rs` | `js_debug_attach_break_step`, `js_debug_logpoints_and_exceptions` | The recordings replayed through the headless shell against their golden files on every platform |

## 7. Breakpoints by file kind

| Adapter | Claims |
|---|---|
| netcoredbg, eludite-dbg-mono, eludite-dbg-netfx | `.cs`, `.vb`, `.fs`, `.fsx`, `.cshtml`, `.razor` |
| vscode-js-debug (the browser session's children) | `.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, `.cts`, `.tsx`, `.jsx`, `.vue`, `.svelte` |
| lldb-dap | `.rs`, `.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`, `.hpp`, `.hxx`, `.m`, `.mm`, `.swift` |

Any other kind goes to every session. A browser session that is a parent gets none: its children own the scripts.

## 8. The exception mapping

| Exception Settings | js-debug filter |
|---|---|
| Break When Thrown | `all` |
| User-Unhandled | `uncaught` |
| Exception types (.NET's) | not sent (js-debug's filter conditions are JavaScript expressions, not offered yet) |

## 9. What a Rust DAP adapter over CDP would replace

If the Node dependency proves unacceptable (proposal 0002 section 9), a Rust adapter would sit behind the same
`AdapterTransport` and session model in place of `dapDebugServer.js`, and would have to provide: CDP attach to a
target by id (`Target.attachToTarget`, `Debugger.enable`, `Runtime.enable`); source-map resolution of breakpoints
(original to generated with `Debugger.setBreakpointByUrl`, pending until `Debugger.scriptParsed` brings the map,
maps fetched from disk or over HTTP) and of frames (`Debugger.paused` call frames to original places and names);
per-target sessions (the `startDebugging` children, or one session with a thread per target); `all` and `uncaught`
as `Debugger.setPauseOnExceptions`; scopes and variables from `Runtime.getProperties` with paging; `evaluate` and
`setVariable` through `Debugger.evaluateOnCallFrame` and `Debugger.setVariableValue`; log points as conditional
breakpoints that log and continue; stepping; `exceptionInfo`; `pauseForSourceMap`. Eludite's shell side (sessions,
the file-kind filter, the frames' `source`, the Attach dialog, the compound) would not change.

## 10. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings` | clean (after `4cc773e`: the rebase had pushed `DebugMsg::Launched` over `large_enum_variant`) |
| `dotnet build dotnet/Eludite.slnx` | 0 warnings, 0 errors (no .NET file changed on this branch) |
| `corpus/tests/build.sh` | builds |
| `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` (CEF 154, Xvfb :99, Chrome for Testing 154, `ELUDITE_CHROME_NO_SANDBOX=1`, `ELUDITE_DBG_MONO`, `ELUDITE_JS_DEBUG` with Node 22.22.0, the corpus built; load average 4 to 7 with the other worktree building) | Last run: **911 passed, 1 failed, 1 ignored**. The failure, `eludite-chromium`'s `popups_become_tabs_and_keep_their_opener` (no popup within 20 s), passed alone twice and in two runs of the engine's whole test binary (10 of 10). The first run (before the last three commits) had 906 passed and 6 failed, each passing alone at once: `a_scripted_agent_fills_the_form_in_a_real_chrome_through_mcp`, `attach_and_restart_against_eludite_dbg_mono` (an empty process listing), `run_control_works_against_eludite_dbg_mono`, `ctrl_f5_on_the_corpus_web_project_then_js_debug_stops_in_app_ts` (the agent's command timed out; alone 392 ms to running), the browser crate's `acting_on_the_page_in_a_headless_chrome` (Chrome's DevTools endpoint within 10 s) and the engine's `javascript_dialogs_and_file_choosers_round_trip_through_the_shell` (a file chooser within 20 s) |
| js-debug re-record check (`DAP_CORPUS_CHECK=1`, both scenarios) | no difference from `corpus/dap/js-debug/` |
| `dotnet test dotnet/Eludite.slnx` | not rerun: no .NET file changed (`git diff origin/main -- dotnet debuggers` is empty) |

## 11. Pending

- **netcoredbg**: `f5_on_the_corpus_web_project_under_netcoredbg_debugs_its_page_too` and the Xvfb run with F5 need
  `ELUDITE_NETCOREDBG` (`tools/netcoredbg/fetch.sh`); CI's Linux job has it.
- **CI is not changed** (`.github/workflows/ci.yml` is outside the brief's files): its Linux job should run
  `tools/js-debug/fetch.sh` (cached by `tools/js-debug/PIN`'s hash, as netcoredbg's fetch is), export
  `ELUDITE_JS_DEBUG`, and add `js-debug` to its `tools/dap-corpus/record.sh --check` step (that step needs CEF and
  Xvfb, which the engine's tests already set up). The recording's `version` names Node's version, so that job must use
  Node 22.22.0 or the scenarios be re-recorded with its Node.
- **Later briefs**: Node.js programs (`pwa-node`), Edge, Firefox, WebDriver BiDi, js-debug's exception conditions,
  Windows and macOS (the embedded engine is Linux only so far).

## 12. How to reproduce

```
export CEF_PATH="$(tools/cef/fetch.sh)" ELUDITE_CHROME="$(tools/chrome/fetch.sh)" ELUDITE_CHROME_NO_SANDBOX=1 DISPLAY=:99
export ELUDITE_JS_DEBUG="$(tools/js-debug/fetch.sh)"   # Node.js 18 or later on PATH
cargo test -p eludite-dap -- js_debug node_ browser_attach exception_settings breakpoints_go sourcemap --nocapture
cargo test --workspace --features eludite-chromium/cef --bin eludite -- js_debug attach_to_a_tab a_web_project_start the_browser_session vite --nocapture
tools/dap-corpus/record.sh --check js-debug
cargo build -p eludite -p eludite-chromium --features eludite-chromium/cef && dotnet build dotnet/Eludite.slnx
crates/eludite/tools/js-debug-linux.sh /tmp/js   # screenshots and js-debug.json
```
