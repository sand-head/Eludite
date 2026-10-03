# Brief 0024 report: Browser automation over CDP: acting on the page, the browser policy and the fake-agent proof

Status: done on Linux. Windows and macOS: not run (no machines; `open_external`'s `open` and `cmd /c start` are
written, not run). CI: not run (nothing pushed). Branch: `brief/0024-browser-automation-act`, based on `main` at
`10c947d`. Date: 2026-10-03. Brief: [0024-browser-automation-act.md](0024-browser-automation-act.md).
ADR: [ADR-0009](../adr/0009-per-call-permission-escalation.md).

## 1. Summary

- **Six commands act on the page.** `eludite.browser.input` (click, double click, right click, hover, type, key,
  scroll, drag, select, focus, by ref or by point), `form_input`, `upload`, `storage`, `network_body` and
  `open_external` join brief 0023's fourteen, with schemas in `protocol/schemas/browser-*.json`, agent-visible,
  audited, over MCP, and tested against headless Playwright Chromium 141.0.7390.37 and Chrome for Testing
  154.0.8037.92 (both pass). No display here: Chrome runs with `--headless=new`, and with `--no-sandbox` because the
  container runs as root (`ELUDITE_CHROME_NO_SANDBOX=1`).
- **Per-call permission escalation (ADR-0009)** is on the command bus: a command registers an escalation hook that
  reads the call's input and the policy and raises the call's class (never lowers it) or refuses it for an agent.
  The MCP boundary classifies each call once, the gate, the prompt and the audit entry use that class and its
  reason, and `tools/list` sends `_meta` `eludite/escalates`.
- **The `browser` policy object** (`origins`, `network_bodies`, `evaluate`) is in `agents-policy.json`: navigating
  (or opening a tab, or the system browser) off the allowed origins is dangerous and prompts; Always Allow adds the
  origin to the file, sorted; files outside the workspace make `upload` and `form_input` dangerous;
  `evaluate: prompt` makes evaluate dangerous and `deny` refuses it; `network_bodies: deny` refuses bodies; `storage`
  with `clear` is execute.
- **The Agents window shows images** a tool call returned (Eludite's screenshot, image content the agent forwards) as
  thumbnails of at most 160 pixels in its transcript row, decoded and scaled off the UI thread; a click saves the
  image and opens it with the system viewer (section 9, item 1).
- **The proof runs:** the scripted fake agent opens the order form in a real headless Chrome through the MCP
  endpoint, screenshots it, reads it, fills the name and the size, clicks Submit, waits for the result, reads it, and
  the test checks the text, one transcript row per tool call and the screenshot's thumbnail.
- **Budgets** (debug test build, 20 calls each, fixture server in the test process; another agent built in the
  other worktree during some runs):

  | Budget | Chromium 141 | Chrome for Testing 154 | |
  |---|---|---|---|
  | `input` click round trip, `wait_ms` 100, < 150 ms p95 | p95 **112.0 to 124.7 ms** (p50 105 to 106) | p95 **111.3 ms** (p50 106.9) | Pass |
  | `form_input` with 10 fields < 200 ms p95 | p95 **31.8 to 34.0 ms** (p50 23 to 25) | p95 **37.1 ms** (p50 25.6) | Pass |
  | `network_body` of a 1 MB body < 200 ms p95 | p95 **12.9 to 21.4 ms** (p50 11 to 15) | p95 **29.8 ms** (p50 24.4) | Pass |
  | Fake-agent proof < 10 s, Chrome's launch included | **844 to 931 ms** | **967 ms** | Pass |
  | No new dependency beyond PNG decoding | `image` 0.25.10, already in the build through GPUI (section 6) | | Pass |

  The click round trip is the `wait_ms` of 100 ms plus about 5 to 25 ms of dispatching and collecting; the
  proposal's "50 ms to the next painted frame in the window" waits for brief B's window.
- **Tests:** `cargo test --workspace` (with `ELUDITE_CHROME` and `ELUDITE_CHROME_NO_SANDBOX=1`): 542 passed,
  0 failed, 1 ignored (a doc example); 28 tests are new on this branch (section 7), and the Chrome tests and the
  proof ran, none skipped. fmt and clippy (`-D warnings`) clean. `dotnet build dotnet/Eludite.slnx`: 0 warnings,
  0 errors. `dotnet test dotnet/Eludite.slnx`: 171 tests, 163
  passed, 7 skipped, 1 failed: `HostProcessTests.Stdout_IsProtocolOnly_AndRenamedLifecycleExitsCleanly`, the
  load-sensitive test that runs the real `eludite-host` ("eludite/solution/status" not yet seen), failed in each of
  three full runs under another agent's builds (load average 9 to 34) and passed in three of four runs alone. No .NET
  code or file changed on this branch (`git diff 10c947d -- dotnet debuggers` is empty); the Mono adapter's tests,
  which failed once in a full run under that load, pass alone (18 of 18). Section 9, item 13.

## 2. What was built

Commits, in order:

1. `775a0dc` ADR-0009 (indexed) and the brief's Status line: in progress.
2. `5078291` `protocol/schemas/` alone: the input and output schemas of the six commands; `agents-policy.json`'s
   `browser` object; `command-spec.json`'s `escalates`; `x-eludite-escalates` and the origin rule in the
   descriptions of `navigate`, `tab_open` and `evaluate`.
3. `9cf883a` `protocol/cdp/`: the generator's roots gain `Storage` and `DOMStorage`; regenerated (15 domains, 1,044
   structs, 142 enums, 35 aliases, 370 commands, 132 events; 16 files, 25,605 lines).
4. `527afc6` `crates/commands`: `register_with_escalation` and `replace_with_escalation`, `Escalation`,
   `EscalationHook`, `CallClass`, `classify`, `invoke_as`, the policy source; `AuditEntry.escalation` and
   `record_call_class`; `CommandSpec::escalates()` (serialized as `escalates`); the `browser` policy with url
   parsing, origin matching, `launch_urls`, `PolicyView`, `AlwaysAllow`, `decide_call` and `remember`; the six
   requests, outputs, parsing, and the browser commands' hooks.
5. `00b47e9` `protocol/schemas/mcp-tool.json` alone: `_meta` `eludite/escalates` (found missing when the MCP code
   met the schema test; committed alone, before the code).
6. `3405b6b` `crates/mcp`: the call's class computed before the gate (`CallContext.class`), refusals that never
   reach the gate, `invoke_as` with that class, `ToolCallRecord.permission` the effective class, `eludite/escalates`
   in `tools/list` (the key removed from `inputSchema`), `readOnlyHint` false for a read command that may escalate.
7. `d14c962` `crates/browser`: `browser/act.rs` (`input`, `form_input`, `upload`), `browser/data.rs` (`storage`,
   `network_body`, `open_external`, base64), `keys.rs` (the key table), response headers kept in the network ring,
   `Network.enable` with the body buffers, three fixtures (`act.html`, `storage.html`, `fetch.html` with
   `data.json` and `pixel.png`), the acting Chrome test and `tests/opener.rs`.
8. `583c7d4` `crates/acp/src/fake_agent.rs`: `McpClient` (one connection, many calls), the `script` scenario and
   the `browser-form` scenario of the proof.
9. `69d31cc` `Browser::set_opener` (the program `open_external` runs, for the shell's test).
10. `1cdfd89` `crates/eludite`: the gate on `ctx.class` with `decide_call`, the prompt's reason, Always Allow through
    `remember`, the policy source, `invoke_as` in the invoker, thumbnails (`transcript.rs`, `window.rs`), opening an
    image, the opener override on the browser bus; the headless policy and thumbnail tests and the fake-agent proof.
11. `439d055` The shell tests' transcript folder is a scoped temporary folder; the opener script runs on Unix only.
12. This report, the brief's Status line and the briefs index.

## 3. The escalation design (ADR-0009)

- **Declared and effective class.** `CommandSpec.permission` is unchanged: what schemas, `tools/list` and the
  policy's class defaults are built on, and the floor of every call. `register_with_escalation(spec, hook,
  handler)` adds `Fn(&Value, &PolicyView) -> Option<Escalation>`. `Escalation::Raise { class, reason, always_allow }`
  raises the call (a class at or below the spec's is ignored: never lowered); `Escalation::Refuse(reason)` refuses
  it for an agent (the policy's `deny`; refusing is the top of the order, above dangerous). The brief's signature
  returned `Option<PermissionClass>`; the reason, the refusal and what Always Allow remembers made it an enum.
- **One evaluation per call.** `CommandRegistry::classify(id, input)` gives a `CallClass` (class, reason,
  `always_allow`, refusal). The MCP server classifies before the gate, puts it in `CallContext.class`, refuses
  refused calls itself (audited, never prompting), gates on the effective class, and invokes with
  `invoke_as(id, input, &class)`, so a policy change between the prompt and the run (Always Allow adding the
  origin) cannot change what is audited. `invoke_audited` classifies for every other caller; the audit entry has
  `permission` (effective) and `escalation` (the reason or the refusal).
- **`PolicyView`** is what a hook may read: the policy, the workspace (solution) folder and its launch urls, through
  a source the shell sets when an agent starts. It is read on first use per call (`OnceLock`), so the `storage` hook,
  which reads only the input, costs nothing; the launch urls are scanned once per solution
  (`Properties/launchSettings.json` up to three folders deep, skipping hidden folders, `bin`, `obj`,
  `node_modules`, `target`, `packages`, `dist`).
- **Rules and Always Allow.** A tool rule was written for a tool's ordinary calls, so allow rules do not apply to an
  escalated call unless its hook says Always Allow writes rules (`AlwaysAllow::Rule`: `evaluate: prompt`,
  `storage` clear); deny rules always apply. Always Allow remembers what the hook names: an origin added to
  `browser.origins` (navigation), a rule, or nothing (`AlwaysAllow::Never`: a file outside the workspace, or a url
  without an origin such as `data:`); the prompt hides Always Allow then.
- **Origins.** `origins` absent means `localhost`, `127.0.0.1`, `[::1]`, `$workspace` and `$launch_urls`. Present, it
  replaces them; entries are a host (`*.example.com` for a domain and its subdomains) or `host:port` for http and
  https, an origin (`https://example.com`: that scheme and host, any port; with a port, only that port), a `file:///`
  path, `$workspace` or `$launch_urls`. `about:` urls are always allowed. Always Allow on a file without `origins`
  writes the defaults first, so allowing one site never revokes localhost. `file:` urls are percent-decoded and
  `..` resolved before the comparison.
- **Hooks of the browser commands.** `navigate` and `tab_open` with a `url`, `open_external` with a `url`: off the
  origins is dangerous. `upload`'s `paths` and `form_input`'s `files` values: a path outside the workspace is
  dangerous (with no workspace open, every path is outside). `evaluate`: `browser.evaluate`. `network_body`:
  `browser.network_bodies`. `storage`: `clear` is execute (declared read).

## 4. CDP, Chrome and harness findings, and how each was handled

1. **Events and answers race through the pump.** CDP sends an event before the answer of the command that caused
   it, but a tab's events are applied by its pump thread, so right after an answer the ring may not have them yet.
   `input` waits `wait_ms` anyway; `form_input` waits until the tab's events have been quiet for 15 ms (at most
   150 ms) before reading the console errors.
2. **Clicking a button focuses it first** (`focus` before `click` in the page's events); the test expects that order.
3. **`DOM.getContentQuads` fails for an element without layout** ("Could not compute content quads"); actions that
   need a point say so (hidden, empty or not rendered); `type`, `key`, `select` and `focus` on such an element still
   work through the node.
4. **`DOM.focus` refuses elements that cannot take focus**; `type` and `key` on such a ref click its point instead.
5. **`DOM.setFileInputFiles` fires `input` and `change` itself**; the page's handler reads the file (`f.text()`), so
   the test waits for the page's text.
6. **`Network.getCookies` with the origin's url** gives exactly the cookies the browser would send there, the
   HttpOnly one included; `Storage.getCookies` gives every cookie of the browser context. The `Storage` domain is
   generated as the brief says but the commands use `Network.getCookies` and `Network.deleteCookies` (by name, domain
   and path) for cookies and `DOMStorage` for `localStorage` and `sessionStorage`. Cookies are read field by field,
   as events are, because the pinned `Network.Cookie` marks members required that older Chromes leave out.
7. **A request that failed has no body in Chrome** ("No resource with given identifier found"); the test uses it as
   the request Chrome dropped. Forcing an eviction from the 200 MB buffer in a test was not practical.
8. **Native range inputs follow dispatched mouse drags** in headless Chrome: the slider reaches 90 or more.
9. **`Control+a` selects all in a text box on Linux** without `Input.dispatchKeyEvent`'s `commands` field (which
   macOS needs, per Puppeteer's notes; not run there).
10. **GPUI's test executor runs background tasks on the test thread**, and the shell's browser worker refuses the UI
    thread; opening an image therefore runs on a thread of its own (`agents-open-image`), which is right outside
    tests too, since writing the file may block.
11. **The same image arrives twice** for Eludite's screenshot: in the command's output (the MCP record) and in the
    agent's forwarded image content. Thumbnails are keyed by a hash of the data, so the row shows it once.

## 5. The commands

- `input`: a ref is scrolled into view (`DOM.scrollIntoViewIfNeeded`) and its point is the center of its first content
  quad; a point gets the element there (`DOM.getNodeForLocation`) and a ref for it. Role and name from
  `Accessibility.getPartialAXTree`. Mouse actions are `Input.dispatchMouseEvent` (moved, pressed and released per
  click count, wheel; drag in ten steps), keys `Input.dispatchKeyEvent` from a US-layout key table (`keys.rs`:
  names, characters, modifiers held down around the key, no text with Control, Alt or Meta), text
  `Input.insertText` or one key per character (`per_key`), `select` through the element (value or label, `input` and
  `change`), `focus` `DOM.focus`. After the action it waits `wait_ms` (default 500), ending early once a navigation
  the action caused has loaded; `navigated` is a new main-frame loader, so refs are stale; `console_errors` are the
  errors since the action began, with source, url, line, column and stack.
- `form_input`: refs resolved (`DOM.resolveNode`, pipelined), one `Runtime.callFunctionOn` per field (pipelined)
  running `SET_JS`: the prototype's `value` setter (React sees it), checkbox and radio state, select options, a
  contenteditable's text, then `input` and `change`; file fields are checked in the page and set with
  `DOM.setFileInputFiles` after checking each path is a file. A stale ref refuses the call; a field that is not a form
  field, disabled, read-only or names no option fails alone with a message.
- `upload`: the element must be a file input (`multiple` for several files); answers how many files it holds.
- `storage`: the origin given or the tab's; `get` cuts values at 1,000 characters with `truncated`; `clear` answers
  what it deleted by kind and logs a line to the Output window.
- `network_body`: the request from the tab's ring (its status, mime type and the response headers, now kept, at most
  64 per request); text bodies as `body`, binary as `body_base64`, cut at `max_bytes` on a character boundary.
- `open_external`: `xdg-open`, `open` or `cmd /c start "" url`, `ELUDITE_OPENER` when set, or the program
  `Browser::set_opener` gives; never waits (a thread reaps the opener); answers the url and the command line.

## 6. Dependencies

No new crate in `Cargo.lock`. `crates/eludite` depends directly on `image` 0.25.10 (`MIT OR Apache-2.0`; features
`png` and `jpeg`, both already enabled in the build by GPUI) to decode and scale thumbnails into GPUI's
`RenderImage`. Base64 is a few lines in `crates/browser` (`base64_encode`, `base64_decode`), not a crate.

## 7. Tests

| Where | New tests | What |
|---|---|---|
| `crates/commands/src/registry.rs` | 2 | A hook raises and never lowers, `escalates` in the spec, the audit records the effective class and reason, a refusal stops an agent and not the user, `invoke_as`; the policy is read only when a hook needs it |
| `crates/commands/src/policy.rs` | 6 | Url parsing and origins; default origins, host and port, scheme, subdomains, `file` under the workspace, launch urls; Always Allow writing the defaults and the origin sorted, `remember`; escalated calls and rules, refusals, `dangerous: deny`; the `browser` object against its schema and saved sorted; `launchSettings.json` scanning |
| `crates/commands/src/browser.rs` | 3 | The hooks against the policy; registered hooks through `classify`; parsing and validation of the six commands (and outputs against their schemas in the existing test) |
| `crates/mcp/src/tests.rs` | 2 | `eludite/escalates` in `tools/list` (and `readOnlyHint`); the gate sees the effective class, refusals never reach it, audit and denial text |
| `crates/browser` (unit) | 7 | Key table; quads and click events; base64, body cutting, cookies field by field, the opener; response headers and console error lists in `tab.rs` (extended) |
| `crates/browser/tests/chrome.rs` | 1 | `acting_on_the_page_in_a_headless_chrome` (below); the two Chrome tests now take turns |
| `crates/browser/tests/opener.rs` | 1 | `open_external` runs the opener named by `ELUDITE_OPENER` (a script recording its argument), never waits, defaults to the active tab's url, names a missing opener |
| `crates/acp/src/fake_agent.rs` | 1 | `--script`, `--url`, refs from `read_page`, MCP content to ACP content |
| `crates/eludite` | 5 | Thumbnails (decode, scale, BGRA, once per row); the gate prompts for `https://example.com` (dangerous, with the reason) and lets `http://127.0.0.1:4321` through, Always Allow writes the origin, the next turn does not prompt; `evaluate: deny` and `network_bodies: deny` refuse and a denied navigation fails; a screenshot from the fake engine becomes a thumbnail that a click saves and opens through `open_external`; the fake-agent proof in a real Chrome |

The acting Chrome test, on `act.html`: click by ref and by point (with Shift), double click, right click, type
(insertion and per key), `key` `Control+a` and `Backspace`, focus, hover showing the title, select by label and by
value (one and several) and a wrong option listing the options, a drag moving the slider to 90 or more, a click
whose handler throws (the error with its location), scroll down and back, `form_input` of ten fields (eight set, a
disabled and a non-field failing alone, the events fired), the page reading the uploaded file, `upload` (one file,
the `multiple` rule, a non-file input, a missing file), Submit and the result's text, the budgets, Enter submitting
the search form (`navigated`, the new url, ending at the load), a stale ref refused by `input` and `form_input`; on
`storage.html`, `get` (the page's cookie, the server's HttpOnly cookie, two local items, one cut, a session item),
`kind`, `clear` and an empty `get`; on `fetch.html`, the JSON body with its headers, the PNG as base64, the failed
request refused, an unknown id, the 1 MB body cut by default and whole with a larger `max_bytes`, timed.

## 8. How to reproduce

```
ELUDITE_CHROME_NO_SANDBOX=1 cargo test -p eludite-browser --test chrome -- --nocapture     # (as root) budgets printed
cargo test -p eludite-browser --test opener
ELUDITE_CHROME_NO_SANDBOX=1 cargo test -p eludite a_scripted_agent_fills -- --nocapture   # the proof, timed
cargo test -p eludite browser_tests
ELUDITE_CHROME=/opt/pw-browsers/chromium-1194/chrome-linux/chrome ELUDITE_CHROME_NO_SANDBOX=1 cargo test --workspace
cargo run -p eludite-cdp-generator && cargo test -p eludite-cdp-generator
```

## 9. Deviations, decisions and open points

1. **Opening a thumbnail's image** saves it beside the transcript (`<transcript>-images/` with `--transcript-out`,
   else `~/.cache/eludite/agents/images/`) and opens it with the system viewer through
   `eludite.browser.open_external` (a command, audited), as the brief allows: a read-only image document tab needs a
   view in the document area, drawn in `shell.rs`, which is outside the files in scope.
2. **`open_external` without a `url`** is judged by its declared class: the hook sees the input and the policy, not
   the active tab's url (ADR-0009's Negative). The tab got there under the origin rule.
3. **The hook's result is an enum**, not `Option<PermissionClass>` (section 3): the reason, refusals and what Always
   Allow remembers.
4. **`CommandSpec` has no `escalates` field**: adding one breaks every `CommandSpec { ... }` literal, including
   `crates/commands/src/debug.rs` (brief 0025's file). `escalates` is read from the input schema's root
   `x-eludite-escalates` by `CommandSpec::escalates()` and serialized as `escalates`, matching `command-spec.json`.
5. **Schemas beyond the brief's list:** `browser-tab-open.input.json` and `browser-evaluate.input.json` gained the
   rule in their descriptions and `x-eludite-escalates` (their commands got hooks), and `mcp-tool.json` gained
   `eludite/escalates`, in a second schema-only commit, before the MCP code.
6. **Outputs carry `tab`**, as every brief 0023 output does; `input` also answers `to`, `selected`, `url` and
   `elapsed_ms`; `storage` answers `origin` and `action`; `network_body` answers `request_id` and `url`.
7. **`keys`** takes one string or a list (the schema says so with `anyOf`).
8. **The proof uses `act.html`**, the second form fixture: `form.html` has no result to wait for and its recorded
   accessibility tree must not change. It takes a screenshot after `tab_open`, so the transcript has the thumbnail
   the brief asks for.
9. **`open_external`'s env test** is its own test binary (`tests/opener.rs`, no Chrome needed), so setting
   `ELUDITE_OPENER` races no other test thread; the shell test uses `BrowserBus::set_opener`.
10. **Always Allow on an escalated call without anything to remember** (an upload from outside the workspace)
    allows once; the prompt hides the button.
11. **`tools/run-dev.sh`** is unchanged: `ELUDITE_OPENER` is a test hook, inherited like any variable. `CLAUDE.md` and
    `README.md` are unchanged: the layout did not change.
12. **Brief 0023's `read_page` timing** (printed, not asserted) reached p95 460 ms in the full workspace run, with
    every test binary running at once beside another agent's build (load average above 30); alone it was 321 ms
    against Chrome for Testing. The two Chrome tests now take turns so this brief's test does not add to it.
13. **The .NET suite under load:** see section 1; the failing test is one known to fail
    under heavy machine load, and this branch does not touch what it runs.
14. **CI** was not run; the Linux job already exports `ELUDITE_CHROME` and `ELUDITE_CHROME_NO_SANDBOX`, so the new
    Chrome tests and the proof run there; on Windows and macOS they skip.

## 10. What briefs S and B need from the command layer

- **The "Agent is driving" strip (S):** a hook on calls in flight per tab: the MCP observer gives the end of a call;
  S needs its start (a `CallContext` before the invoker) and a cancel path that ends an agent's `wait` and `input`
  wait with `interrupted_by: "user"`, which `Browser` must check between its polls.
- **The person's own input (B):** the window's clicks and keys can call `input` itself on the browser worker (as
  commands, invariant 3); the UI-thread refusal stays, so the window must invoke from a background task and render
  the answer when it comes.
- **Thumbnails and images:** the transcript's `Thumb` (encoded bytes, scaled `RenderImage`) is what an image document
  view would show; B's document area can register an image view and replace item 1's system viewer.
- **Escalation:** `record`'s path rule (outside the workspace or the cache) and `devtools` fit as hooks, and
  `debug.attach` (proposal 0001 brief C) is the next non-browser user.
- **Screencast frames (B):** `Page.startScreencast` is generated; frames would bypass the command bus (they are not
  user actions) and go straight to the window.
