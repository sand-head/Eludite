# Brief 0024: Browser automation over CDP: acting on the page, the browser policy and the fake-agent proof

Status: in progress
Phase: 2 (proposal 0002, brief A, second half)
Plan reference: PLAN.md sections 2 (principles 1, 3, 5), 4.9, 5.1, 5.3 (the `browser` policy object), 5.4, 5.6, 5.8, 9, 10 (Phase 2); proposal 0002 sections 4.3, 5, 7, 8 (A), 11
Related ADRs: ADR-0003, ADR-0008; a new ADR-0009 (below)
Depends on: brief 0023 (the engine, refs, rings, the fourteen reading and navigation commands), brief 0016 (the Agents window and the permission gate). Must not touch brief 0025's files (`crates/dap`, `crates/eludite/src/shell/debug*`, `crates/commands/src/debug.rs`).

## Goal

An agent acts on a page through Eludite with the same discipline as it reads it: `eludite.browser.input` (click, double click, right click, hover, type, key, scroll, drag, select, focus, by ref or by point), `form_input` (many fields in one call), `upload`, `storage`, `network_body` and `open_external` join brief 0023's commands; the per-solution policy gains the `browser` object of proposal 0002 section 5 (`origins`, `network_bodies`, `evaluate`), so navigating off the allowed origins and uploading from outside the workspace are `dangerous` and prompt; the Agents window shows the screenshots an agent took as thumbnails in its transcript rows; and the proving scenario of proposal 0002 brief A runs headless on CI: a scripted fake agent fills the form fixture through the MCP tools and reads the result. The structural change this needs, a permission class that can escalate per call from the command's input, is recorded in ADR-0009 and built into the command bus once, for every command that needs it later (`debug.attach` in proposal 0001 brief C is the next).

From brief 0023's report (section 10): `Input.dispatchMouseEvent`, `dispatchKeyEvent` and `insertText`, `DOM.scrollIntoViewIfNeeded` and `DOM.getContentQuads` are generated already; `TabState::console_errors_since` exists; the `Storage` and `DOMStorage` domains must be added to the generator's roots and regenerated; `Network.getResponseBody` needs the bodies kept; a ring entry has its `request_id`; `tests/fixtures/form.html` and the test server in `crates/browser/tests/chrome.rs` are reusable; the screenshot is MCP image content already.

## Files in scope

- `docs/adr/0009-per-call-permission-escalation.md` (new, 40 to 90 lines) and `docs/adr/README.md`: the decision that a command's spec carries one class (unchanged, stable for schemas and `tools/list`) and that a command may register an escalation hook, evaluated per call from the input and the current policy, whose result is the class the gate and the audit record use; `dangerous` only ever escalates, never relaxes.
- `protocol/schemas/` first and alone: `browser-input.{input,output}.json`, `browser-form-input.{input,output}.json`, `browser-upload.{input,output}.json`, `browser-storage.{input,output}.json`, `browser-network-body.{input,output}.json`, `browser-open-external.{input,output}.json`; `agents-policy.json` (the `browser` object); `command-spec.json` (an optional `escalates` description string on a spec, documenting when a call is `dangerous`); `browser-navigate.input.json` (its description names the origin rule now in force); `settings.json` if a setting is needed (`browser.openExternalCommand` is not: the system opener is the platform's `xdg-open`, `open`, `start`).
- `protocol/cdp/generator/` (roots gain `Storage`, `DOMStorage`; regenerate `protocol/rust/src/cdp/`).
- `crates/commands/src/{registry,policy,browser,audit}.rs` and `lib.rs`: the escalation hook on registration (`register_with_escalation` or a builder), `AuditEntry.permission` recording the effective class, the `browser` policy object with its defaults and its decision (`origins` matched by scheme and host, with `localhost`, `127.0.0.1`, `::1`, `file://` under the workspace and the workspace's launch urls as defaults; `network_bodies` `allow` default or `deny`; `evaluate` `allow` default, `prompt`, `deny`), and the new requests and outputs.
- `crates/mcp/**`: the gate receives the effective class; `tools/list` keeps the spec's class and adds `_meta` `eludite/escalates` when the spec has it.
- `crates/browser/**`: the six commands, the response-body buffer, the storage reads and clears, the console errors emitted during an action, the point computation for refs; tests and fixtures (a second form fixture with a file input, a select and checkboxes; a page with cookies, `localStorage` and `sessionStorage`; a page whose fetch returns a JSON body).
- `crates/eludite/src/shell/browser.rs`, `browser_tests.rs`, `shell/agents/{transcript,window}.rs` and `agents.rs` (thumbnails from image content in tool results, clickable to open the full image in a document tab as a read-only image view, or, if the editor has no image view yet, saved beside the transcript and opened with the system viewer; choose the former if it fits in the brief, else record the latter as the deviation), `shell/agents/tests.rs` for the fake-agent proof; `crates/acp/src/fake_agent.rs` if the scripted fake needs a step that reads tool results.
- `tools/run-dev.sh` only if a variable is added; `CLAUDE.md` and `README.md` only if the layout changes; `docs/briefs/README.md`; `docs/briefs/0024-report.md` (new).

## Contract

### The escalation hook (ADR-0009)

- `CommandSpec.permission` stays the declared class. A command may be registered with an escalation function `Fn(&Value, &PolicyView) -> Option<PermissionClass>` that returns a higher class for this call or `None`. `CommandRegistry::invoke_audited` and the MCP gate call it before deciding; the audit entry records the effective class and the reason the hook gives (a short string, such as `navigate off the allowed origins: https://example.com`). A hook can never return a lower class than the spec's. `PolicyView` is what the hook may read: the current `agents-policy.json` object and the workspace folder.
- `tools/list` is unchanged in shape; a spec with a hook carries `_meta` `eludite/escalates` (the description string from the schema), so a model knows a call may prompt.

### The `browser` policy object

- `agents-policy.json` gains `browser`: `origins` (a list of origins or hosts; default when absent: `localhost`, `127.0.0.1`, `[::1]` on any port and scheme `http` or `https`, `file` under the workspace folder, and every `applicationUrl` of the workspace's `launchSettings.json` profiles), `network_bodies` (`allow` default, `deny`), `evaluate` (`allow` default, `prompt`, `deny`). Always Allow in the permission prompt for an off-origin navigation adds the origin to `origins`, as it adds rules today, keeping the file sorted.
- `navigate` with a url off the allowed origins is `dangerous` through the hook (`back`, `forward` and `reload` stay `execute`); `tab_open` with a url likewise. `upload` with a path outside the workspace is `dangerous`. `evaluate` follows `browser.evaluate` (`prompt` escalates to `dangerous`; `deny` refuses with the policy named). `network_body` is refused with the policy named when `network_bodies` is `deny`.

### The commands

All `eludite.browser.*`, `agent_visible: true`, `tab` optional, outputs carrying `page_generation`, descriptions written for a model.

- `input` (execute): `action` (`click`, `double_click`, `right_click`, `hover`, `type`, `key`, `scroll`, `drag`, `select`, `focus`), `ref?` or `x`, `y` (CSS pixels of the viewport, the space `screenshot` reports), `text?` (for `type`: inserted with `Input.insertText` after focusing, or per key when `per_key` is true), `keys?` (for `key`: names such as `Enter`, `Tab`, `Escape`, `ArrowDown`, `a`, `Control+a`, with modifiers), `delta?` (`scroll`: `{x, y}` in pixels, default `{0, 400}`), `to?` (`drag`: a ref or point), `values?` (`select`: option values or labels), `modifiers?` (`shift`, `control`, `alt`, `meta`), `wait_ms` (default 500: how long to collect console errors and a navigation after the action). A ref is scrolled into view (`DOM.scrollIntoViewIfNeeded`) and its point is the center of its first content quad; a stale ref is refused as in brief 0023. Output: `action`, `target` (`ref`, `role`, `name`, `point`), `page_generation` (after the action), `navigated` (true when the main frame navigated within `wait_ms`), `console_errors` (the errors emitted during the action, with text and location).
- `form_input` (execute): `fields` (1 to 100 of `{ ref, value }` where `value` is a string, a boolean (checkboxes and radios), a list of strings (multi-select) or `{ "files": [paths] }`), through `Runtime.callFunctionOn` on the resolved node (set the value and dispatch `input` and `change`), `DOM.setFileInputFiles` for files (the `upload` rule applies to each path). Output: per field `ref`, `ok`, `message?`, plus `page_generation` and `console_errors`.
- `upload` (execute; dangerous outside the workspace): `ref`, `paths` (1 to 50 existing files). Output: `count`, `page_generation`.
- `storage`: `kind` (`cookies`, `local`, `session`, `all`), `action` (`get` is `read`; `clear` is `execute`), `origin?` (default: the tab's origin). `get` output: `cookies` (name, value, domain, path, expires, http_only, secure, same_site) and `local`, `session` (key, value, with values cut at 1,000 characters and `truncated`), `total`. `clear` output: what was cleared.
- `network_body` (read; policy `browser.network_bodies`): `request_id`, `max_bytes` (default 65,536, max 10 MB). Output: `status`, `headers` (response), `mime_type`, `body` (text, or `body_base64` when binary), `size`, `truncated`. Bodies are kept by Chrome for requests since `Network.enable` with `maxResourceBufferSize` 50 MB and `maxTotalBufferSize` 200 MB; a body Chrome no longer has is a failed command naming it.
- `open_external` (execute): `url?` (default: the active tab's url; the origin rule applies). Opens the url in the system browser (`xdg-open`, `open`, `cmd /c start`), never waits for it. Output: `url`, `command`.
- The Agents window: a tool result with image content shows a thumbnail (at most 160 pixels wide, decoded off the UI thread) in the transcript row; clicking it opens the image.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands`: the escalation hook raises the class and never lowers it; the audit entry records the effective class and reason; the `browser` policy parses, defaults and decides (`origins` by host and port, `file` under the workspace, `evaluate` modes); `agents-policy.json` validation; parsing and validation of the six commands.
- `crates/mcp`: a spec with a hook lists `eludite/escalates`; the gate sees the effective class.
- `crates/browser/tests/chrome.rs` (skips without Chrome): on the form fixture, `input` click on a button by ref and by point, `type` into a text field (and `per_key`), `key` `Enter` submits, `scroll` moves the viewport, `hover` shows a title, `select` picks options, `drag` moves a slider, `focus`; `form_input` sets text, a checkbox, a radio, a multi-select and a file; a stale ref is refused; `console_errors` carry the error a handler throws; `navigated` is true after a submit; `upload` sets a file input and the page reads the name; `storage` `get` lists the cookie and the storage items the page set and `clear` empties them; `network_body` returns the fetch's JSON body and a binary body as base64, and refuses a request Chrome dropped; `open_external` runs a fake opener from an environment variable in the test. Timings for the budgets.
- `crates/eludite` headless tests: the policy gate prompts for `navigate` to `https://example.com` and allows `http://127.0.0.1:<port>`; Always Allow adds the origin to the policy file; `browser.evaluate: deny` refuses; `network_bodies: deny` refuses; a fake engine's tool result with image content produces a thumbnail row in the Agents window.
- The fake-agent proof (`shell/agents/tests.rs`, with the fake ACP agent from `crates/acp`): a scripted agent, through the MCP endpoint and a real headless Chrome serving the form fixture, runs `tab_open` with the fixture url, `read_page`, `form_input` for the name and the choice, `input` click on Submit, `wait` for the result selector, `page_text` of the result, and the test asserts the text and the transcript rows (one per tool call, the screenshot the agent took as a thumbnail); skipped without Chrome, run on Linux CI.
- No display: headless; the report says so.

## Budget

- `input` click round trip (dispatch to the answer with console errors collected, `wait_ms` 100): under 150 ms p95 over 20 clicks (the proposal's 50 ms "to the next painted frame in the window" waits for brief B's window; record the round trip here).
- `form_input` with 10 fields: under 200 ms p95.
- `network_body` of a 1 MB body: under 200 ms p95.
- The fake-agent proof: under 10 s end to end on this machine, with Chrome's launch included.
- No new dependency beyond what decoding a PNG thumbnail needs (`image` or `png` are already in the build through GPUI; use GPUI's image loading if it fits).

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the Chrome tests running on this machine (`ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`); `dotnet build` and `dotnet test` unchanged and green.
2. ADR-0009 is written and indexed; the report records the budget numbers, the dependencies with SPDX ids (if any), and what brief S and brief B (proposal 0002) need from the command layer.
3. The briefs index, `CLAUDE.md` and `README.md` match the repository.

## Out of scope

- `record`, `devtools`, the "Agent is driving" strip, the Web Browser window, CEF (briefs S and B); launch integration (brief C); JavaScript debugging (brief D).
- WebDriver BiDi and other browsers; the user's own Chrome profile (never).
- Windows and macOS runs (`open_external`'s platform commands are written for them, not run).
