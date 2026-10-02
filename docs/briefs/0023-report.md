# Brief 0023 report: Browser automation over CDP against an external Chrome

Status: done on Linux. Windows and macOS: not run (no machines; the fetch scripts and discovery are written for them).
CI: not run (nothing pushed). Branch: `brief/0023-browser-automation-read`, based on `c88eadd`; `main` has since
moved to `5143b89` (brief 0022 merged) and the branch was not rebased (section 9). Date: 2026-10-03.
Brief: [0023-browser-automation-read.md](0023-browser-automation-read.md).

## 1. Summary

- **The fourteen commands work against a real browser.** `eludite.browser.tabs`, `tab_open`, `tab_close`,
  `tab_select`, `navigate`, `resize`, `screenshot`, `read_page`, `find`, `page_text`, `console`, `network`, `wait`
  and `evaluate` are on the command bus with schemas in `protocol/schemas/browser-*.json`, agent-visible, audited,
  exposed as MCP tools (a screenshot arrives as MCP image content), and drive Chrome over the Chrome DevTools Protocol
  on a websocket. `crates/browser/tests/chrome.rs` exercises every one of them against headless Playwright Chromium
  141.0.7390.37 and against Chrome for Testing 154.0.8037.92 installed by `tools/chrome/fetch.sh`; both pass.
  Chrome runs with `--headless=new` here (no display) and with `--no-sandbox` (this container runs as root), which
  only `ELUDITE_CHROME_NO_SANDBOX=1` adds.
- **Budgets** (debug test build, 20 calls each, against a fixture server in the test process; the machine was shared
  with another agent's builds, 1-minute load average 0.8 to 7.1 on 4 cores during these runs):

  | Budget | Chromium 141 | Chrome for Testing 154 | |
  |---|---|---|---|
  | `screenshot` at 1280 wide < 150 ms p95 | p95 **52.0 to 67.4 ms** (p50 49 to 50) | p95 **50.8 to 62.2 ms** (p50 34 to 46) | Pass |
  | `read_page` on the 5,000-item fixture < 300 ms p95 | p95 **240.8 to 298.5 ms** (p50 225 to 243) | p95 **241.6 to 287.6 ms** (p50 213 to 253) | Pass, narrowly; 327 to 381 ms in runs during heavier load (section 4) |
  | `read_page` answer < 64 KB with the default `max_nodes` | 55,890 bytes (500 of 5,000 nodes) | 55,890 bytes | Pass |
  | `navigate` to a local fixture with `load` < 500 ms p95 | p95 **24.1 to 47.0 ms** | p95 **30.7 to 57.6 ms** | Pass |
  | Cold start unchanged | No worker thread, engine or process until the first browser command (asserted); registering the fourteen commands (their schemas) costs 4.9 ms in the debug test build | | Pass |
  | Shell resident memory, browser running and idle: + 20 MB at most | **+0.5 to +0.7 MB** (`VmRSS` of the process holding the engine, before the launch and after it with a page loaded) | +0.7 MB | Pass |

  Other timings: launch plus loading the form page 303 to 609 ms; `wait` for an element the page adds 400 ms after
  load answers at 408 to 413 ms; while a fake engine blocks 200 ms in the `browser` worker, the UI thread ran 50 to
  55 updates with the slowest 1.9 to 4.5 ms.
- **Tests:** `cargo test --workspace` (with `ELUDITE_CHROME` and `ELUDITE_CHROME_NO_SANDBOX=1`) passes: 504 passed,
  0 failed, 1 ignored (a doc example). 46 tests are new on this branch (section 7). fmt and clippy (`-D warnings`)
  clean. `dotnet build` and `dotnet test dotnet/Eludite.slnx`: unchanged and green (153 tests: 146 passed, 7 skipped).
- **New dependencies:** `tungstenite` 0.30.0 (`MIT OR Apache-2.0`, no TLS features) and what it pulls in (section 6).
  The CDP JSON (`BSD-3-Clause`) is data under `protocol/cdp/`; Chrome is located at run time, never vendored.

## 2. What was built

Commits, in order:

1. `c64637c` `protocol/schemas/` alone: the input and output schema of each of the fourteen commands (each input's
   root `description` written for a model: when to use it, what it costs, what to call next), the settings
   `browser.chromePath` (`x-eludite-env` `ELUDITE_CHROME`), `browser.headless` and `browser.viewport` (section Web
   Browser), and the MCP image content rule in `mcp-tool.json`.
2. `3913e7d` `protocol/cdp/`: `devtools-protocol` 0.0.1709723 pinned (`PIN`, `fetch.sh`, `fetch.ps1`, the two JSON
   files byte-identical to the package's, `LICENSE.chromium`), `eludite-cdp-generator`, and its output in
   `protocol/rust/src/cdp/` behind `eludite-protocol`'s `cdp` feature (section 5).
3. `e417362` `tools/chrome/`: Chrome for Testing 154.0.8037.92 pinned for `linux64`, `win64`, `mac-arm64` and
   `mac-x64` by SHA-256 (computed from the downloads on 2026-10-02), and `fetch.sh` and `fetch.ps1`, which download,
   verify and unpack into `~/.cache/eludite/chrome/<version>/` and print the executable.
4. `17eaba9` `crates/commands/src/browser.rs`: request parsing and validation against the schemas, the typed outputs,
   `BrowserTarget` and `register`, as `debug.rs` does for the debugger; `OutputSource::Browser`.
5. `dbcae02` `crates/browser` (`eludite-browser`, GPL): the websocket CDP client, the `Engine` trait and
   `ExternalChrome`, tabs with page generations and refs, the console and network rings, discovery, the launcher, the
   command implementations, unit tests, the Chrome tests and their fixtures (section 3).
6. `b8512f1` `crates/mcp`: an output whose schema marks a top-level string with `"x-eludite-mcp-content": "image"`
   gets `{"type": "image", "data", "mimeType"}` after the text part, and the text and `structuredContent` carry
   `"(image content)"` in its place, so the base64 is sent once. `contentMediaType` `image/*` is read from the
   data's first bytes (PNG, JPEG, GIF, WebP).
7. `defec3e` `crates/eludite`: the shell's `browser` worker, the Output window's Browser source, the three settings,
   the workspace's profile and closing the browser with the workspace and on exit; `tools/run-dev.sh` passes
   `ELUDITE_CHROME` through (section 3.4).
8. `d928bb2` The Chrome test measures the process's resident memory with the browser running and idle.
9. `47539ab` `.github/workflows/ci.yml`: the Linux Rust job caches `~/.cache/eludite/chrome` keyed on
   `tools/chrome/PIN`, runs `tools/chrome/fetch.sh` and exports `ELUDITE_CHROME`; Windows and macOS skip the Chrome
   tests.
10. `10bc895` The brief's Status line: in progress.
11. `63357ba` The read_page schemas describe reading a large page element by element (section 4).
12. `c032867` `read_page` reads the boxes of those elements in one call, and the Chrome test checks that path's rows,
    depths, refs and boxes against `find`.
13. `f988f2a` The shell test times the commands' registration (the cold-start cost).
14. This report, the brief's Status line, the briefs index, `CLAUDE.md`'s crate map, `README.md`'s layout and
    `protocol/cdp/README.md`.

Commits 1 to 4 and part of 5 were made by an earlier agent interrupted by a container restart; it left `crates/browser`
uncommitted and its tests not compiling (a recorded fixture was missing). This agent recorded the fixture, fixed what
the Chrome tests found (section 4), met the `read_page` budget, and did items 6 to 14.

## 3. The design

### 3.1 The websocket client (`crates/browser/src/connection.rs`)

`tungstenite` over `std::net::TcpStream`, no TLS, no async runtime. One `cdp-reader` thread per connection owns the
read half; writes go through a second `tungstenite` socket over a clone of the stream, under a mutex, so a write never
waits for a read. Requests are correlated by id: `call` waits up to a timeout (10 s by default), `call_many` pipelines
several and waits for all, `send` only queues and returns a receiver, so nothing blocks a caller that did not ask to
wait. Events go to the subscribers of their `sessionId` through channels. A closed connection fails every pending
request with `Closed`, ends every subscription and runs a close hook once. The message envelope (`id`, `method`,
`params`, `sessionId`, `result`, `error`) is typed here, not generated.

### 3.2 The engine (`engine.rs`, `chrome.rs`, `discovery.rs`)

`Engine` is the trait the commands need: configure, launch, the page targets, open, close and activate a tab, attach
(a session per tab), send a CDP command on a session (and `send_many`), subscribe to a session's events, screenshot
pixels, shutdown. `ExternalChrome` implements it: `chrome --remote-debugging-port=0
--user-data-dir=<workspace>/.eludite/browser/profile --no-first-run --no-default-browser-check
--disable-background-networking --disable-sync --disable-default-apps --window-size=W,H [--headless=new]
[--no-sandbox] about:blank`, the endpoint read from `DevTools listening on ws://...` on stderr (10 s), then
`Target.setDiscoverTargets` and `Browser.getVersion`; tabs are `Target.createTarget` and `closeTarget`, sessions
`Target.attachToTarget` with `flatten: true`. When the connection closes without a shutdown, the engine is gone: the
log says so with the exit status, and the next launch starts a new browser. Shutdown asks `Browser.close` and kills
the process after 3 s. Discovery: the setting (`ELUDITE_CHROME`), the fetch script's cache, `google-chrome`,
`google-chrome-stable`, `chromium`, `chromium-browser`, `chrome` on `PATH` (each name through the whole `PATH`), then
the platform's install locations; the error names every place and `tools/chrome/fetch.sh`.

### 3.3 Tabs, generations, refs, rings (`browser.rs`, `tab.rs`, `page.rs`, `ring.rs`)

Tabs get ids `t1`, `t2`, ... never reused. Attaching enables `Page` (with lifecycle events), `Runtime`, `Log`,
`Network` and `DOM`, and a pump thread per tab applies its events to the tab's state: `page_generation` grows on the
main frame's `Page.frameNavigated` and on `DOM.documentUpdated`; refs `e1`, `e2`, ... map to backend node ids and
belong to the generation that issued them (a ref used later is refused with "stale ref: the page changed (generation
N, ref from M); call read_page again"); console messages (`Runtime.consoleAPICalled`, `Runtime.exceptionThrown`,
`Log.entryAdded`, 2,000 per tab) and requests (`Network.requestWillBeSent`, `responseReceived`, `loadingFinished`,
`loadingFailed`, 5,000 per tab) go into rings with cursors (`seq`, `next`, `dropped`). Console errors also go to the
log as `[t1] url:line: text`. Waits are event-driven where CDP has the event (lifecycle per loader, requests in
flight, console, navigations) and poll every 100 ms otherwise (selector, text, function).

### 3.4 The shell (`crates/eludite/src/shell/browser.rs`)

One `BrowserBus` per shell is the commands' target. The first browser command starts a thread named `browser` that
owns the `Browser` (engine and tabs); an agent's call (MCP thread) is handed to it and blocks until the answer, as
other commands block their caller. A browser command invoked on the UI thread fails at once with a message (there is
no Web Browser window yet, brief B), so the UI never waits on Chrome. The engine's lines (launch with executable,
version, endpoint and profile; tabs opened and closed; navigation failures; console errors; unexpected exit; close)
go through a channel to the Output window's Browser source, a batch per update. The settings apply to the next launch
through `apply_settings`; the profile is `<workspace root>/.eludite/browser/profile` (a folder in Eludite's cache when
no workspace is open); a workspace change or close closes the running browser; shell exit closes it and waits up to
5 s, off the UI thread, beside the language servers' shutdown.

## 4. CDP and Chrome quirks met, and how each was handled

1. **`Accessibility.getFullAXTree` costs about 60 microseconds per accessibility node.** The 5,000-item fixture has
   30,007 nodes (15 MB of JSON): 1.7 to 2.9 s per call in Chrome alone, ten times the budget; `queryAXTree` by role
   took 2.4 s. `getPartialAXTree` costs about 0.38 ms per call. So `read_page` with `filter: interactive` on a
   subtree of more than 1,500 elements collects the elements that can be interactive in the page
   (`a[href]`, `button`, `input`, `select`, `textarea`, `option`, `summary`, `iframe`, `[tabindex]`,
   `[contenteditable]`, `[role]` and a few more, in document order, through open shadow roots) with their nearest
   candidate ancestors and boxes, reads the accessibility node of each with pipelined `getPartialAXTree` calls until
   `max_nodes` rows are kept, and keeps them by the same rule as the full tree's filter; `total` then counts the
   candidates not examined as matches (an upper bound when truncated, said in the schema). Smaller pages and
   `filter: all` still read the full tree. The phases of one call on the fixture: collecting 10 ms, the 508
   accessibility nodes 190 to 230 ms (Chrome), decoding 11 ms, the boxes one call (`DOM.getBoxModel` for 500 nodes
   was 25 ms). That leaves little headroom: under the heaviest load of this machine (another agent compiling, load
   average 8 to 11) p95 reached 327 to 381 ms. This is a deviation from the brief's "`accessibility`:
   `Accessibility.getFullAXTree`", made to meet its budget.
2. **`DOM.querySelectorAll` with 5,000 results costs 180 ms**, pushing every node and its ancestors to the client.
   The candidate collection runs in the page (`Runtime.callFunctionOn`) instead.
3. **`Target.getTargets` still lists a closed target for a moment after `Target.closeTarget` answers.** The next
   sync adopted it as a new tab. The browser remembers the targets it closed and skips them.
4. **The screenshot of the viewport excludes the vertical scrollbar.** `cssVisualViewport.clientWidth` is 375 CSS
   pixels of a 390-pixel `innerWidth` on the list page; the test compares with `clientWidth`.
5. **`--headless=new --window-size=1280,800` gives a 1280 by 661 viewport** (Chromium 141; 1280 by 657 in Chrome for
   Testing 154): the window size includes emulated browser chrome. `resize` sets the viewport exactly.
6. **A fetch whose body the page never reads stays loading forever** (no `loadingFinished`), so `network_idle` would
   never hold. A request leaves the in-flight set at its response headers; `Network.dataReceived` counts as activity.
7. **Requests cut off by a navigation (a favicon) may never finish or fail.** A main-frame navigation forgets the old
   loader's requests in flight.
8. **A page restored from the back/forward cache fires no lifecycle events.** Once the main frame has committed,
   `navigate` and `wait` also check `document.readyState`.
9. **`Page.navigate` to a fragment of the same document answers no `loaderId`** and fires no load: the navigation is
   done when it answers.
10. **A fresh browser opens one `about:blank` tab.** The first `tab_open` uses it rather than leave it beside the new
    one.
11. **`DOM.performSearch` matches text nodes.** `find` by text answers their parent elements (resolve, `parentElement`,
    describe), deduplicated.
12. **Mobile emulation lays a page without a meta viewport out 980 CSS pixels wide**, as a phone does; `resize`
    answers the viewport in effect, read from the page.
13. **Chrome prints its endpoint on stderr and blocks if stderr fills.** stderr is drained for the browser's life; its
    last 30 lines go into launch errors.
14. **Chrome refuses to run as root without `--no-sandbox`.** Only `ELUDITE_CHROME_NO_SANDBOX=1` adds it; the tests
    pass it when `id -u` is 0, the shell never sets it, and CI sets the variable on Linux (section 9).
15. **Events are read field by field, not through the generated event structs**: a Chrome older or newer than the pin
    may leave out a member the pin marks required, and an event that failed to decode would be lost from the tab's
    state. Commands whose answers are read in full use the generated `Params` and `Returns`.

## 5. The generator

- `protocol/cdp/generator` (`eludite-cdp-generator`, MIT): 838 lines in `lib.rs` and 40 in `main.rs`, 182 lines of
  tests, plain Rust with `std::fmt::Write`, `serde_json` its only dependency; it runs `rustfmt` on its output.
- Input: `browser_protocol.json` (1.4 MB) and `js_protocol.json` (180 KB) of `devtools-protocol` 0.0.1709723. Roots
  `Target`, `Page`, `DOM`, `Accessibility`, `Runtime`, `Log`, `Network`, `Input`, `Emulation`, `Browser`, `Security`,
  plus `Debugger` and `IO` reached through `$ref`s.
- Output: 13 domains, 960 structs, 140 enums (each with an `Other(String)` variant), 32 aliases, 337 commands, 121
  events; 14 files, 24,370 lines, 796,935 bytes, `rustfmt`- and `clippy`-clean.
- Tests: the checked-in output equals a fresh generation into a temporary directory; serde round trips of
  `Page.captureScreenshot`'s returns, a `Runtime.consoleAPICalled` event and an `Accessibility.AXNode` with an unknown
  role. `protocol/cdp/fetch.sh --check` re-fetched the tarball on 2026-10-03: the checked-in files match the pin.

## 6. Dependencies

| Crate | Version | SPDX | Why |
|---|---|---|---|
| `tungstenite` | 0.30.0 | `MIT OR Apache-2.0` | The websocket (feature `handshake` only, no TLS) |
| `httparse` | 1.10.1 | `MIT OR Apache-2.0` | tungstenite's handshake |
| `sha1` | 0.11.0 | `MIT OR Apache-2.0` | tungstenite's handshake |
| `data-encoding` | 2.11.1 | `MIT` | tungstenite's handshake |
| `rand` | 0.10.3 | `MIT OR Apache-2.0` | tungstenite's masking keys |
| `rand_core` | 0.10.1 | `MIT OR Apache-2.0` | rand |
| `chacha20` | 0.10.2 | `MIT OR Apache-2.0` | rand |

`regex` (`MIT OR Apache-2.0`, already in the build) is now a direct dependency of `eludite-browser`. Resolving the
new crates moved six existing `Cargo.lock` entries' `windows-sys` from 0.59.0 to 0.61.2 (both versions were already
in the lock). Data and tools: the CDP JSON (`BSD-3-Clause`, The Chromium Authors) is checked in under `protocol/cdp/`;
Chrome for Testing (Google's build of Chromium, whose code is `BSD-3-Clause`) is downloaded at run time by
`tools/chrome/fetch.sh`, never vendored.

## 7. Tests

| Where | Tests | What |
|---|---|---|
| `crates/browser/src/connection.rs` | 5 | Framing and correlation against an in-process fake websocket server (out-of-order answers, events by session, protocol errors), pipelining, the request timeout and a late answer dropped, a closed connection failing what is pending, message shapes |
| `crates/browser/src/tab.rs` | 4 | Refs and generations (a ref from generation 1 refused at 2 with the exact message), console events into the ring, network events and what is in flight, lifecycle per loader |
| `crates/browser/src/page.rs` | 6 | The `interactive` filter over `tests/fixtures/form-axtree.json` (recorded from Chromium 141 with `ELUDITE_RECORD_FIXTURES=1`), the partial-tree row, depths of candidates, DOM rows, boxes and image sizes |
| `crates/browser/src/ring.rs`, `discovery.rs`, `chrome.rs`, `engine.rs`, `browser.rs` | 2, 2, 4, 1, 3 | Cursors and `dropped`; discovery order with temporary directories; the launch command line, the endpoint line, the profile's `.gitignore`, a missing Chrome; viewports; truthiness, patterns, exception text |
| `crates/browser/tests/chrome.rs` | 2 | The proving test against a launched headless Chrome (below); a bad `browser.chromePath` |
| `crates/commands/src/browser.rs` | 5 | Schemas and classes of the fourteen commands, parsing and validation, outputs against their schemas, registration |
| `crates/mcp/src/tests.rs` | 2 | Image content and the replaced text and structured content; media type sniffing |
| `crates/eludite/src/shell/browser_tests.rs` | 4 | Registered and agent-visible with their schemas and nothing started at startup; `tabs` off the UI thread with a fake engine blocking 200 ms while frames run; the Output source's lifecycle lines; the settings reach the launch and the browser closes with the workspace and on exit |
| `protocol/cdp/generator/tests` | 5 | The checked-in output is current; serde round trips |

The Chrome test, against a std-only HTTP server serving `tests/fixtures/` (a form, 5,000 list items, a page that
logs an error and fetches a missing resource, a page whose button rewrites the document): `tabs` before launch says
not running; `tab_open` with a url launches and reuses the first blank tab; `navigate` to each fixture with `load`
and `network_idle`, back, forward, reload, and a `net::ERR_` failure; `screenshot` (PNG dimensions against the
viewport scaled to `max_width`, JPEG, full page, a ref clip); `read_page` with refs, states and boxes, `dom` mode;
`find` by css, text and role with name; `wait` for a selector, a function, text, network idle and a timeout;
`page_text` paging and a root; `console` with level and `/regex/` and the error at `errors.html:11`; `network` with
the 404 and the document's duration and bytes; `evaluate` of an object, a cut string, a promise, a node and an
exception as a failed command; the rewrite bumps `page_generation` and a stale ref is refused; the 5,000-item page's
budget, size and its element-by-element rows; `resize` with a scale factor, user agent and mobile; a second tab,
select, close; Chrome killed with `kill -9` under a pending request fails it, and the next `tab_open` relaunches it as
`t3`. It skips with a message when no Chrome is found, and on Windows and macOS unless `ELUDITE_CHROME` is set.

## 8. How to reproduce

```
tools/chrome/fetch.sh                                     # prints Chrome for Testing's path
ELUDITE_CHROME_NO_SANDBOX=1 cargo test -p eludite-browser --test chrome -- --nocapture   # (as root) budgets printed
ELUDITE_CHROME=/opt/pw-browsers/chromium-1194/chrome-linux/chrome ELUDITE_CHROME_NO_SANDBOX=1 cargo test --workspace
cargo test -p eludite browser_tests -- --nocapture
cargo run -p eludite-cdp-generator && cargo test -p eludite-cdp-generator
protocol/cdp/fetch.sh --check
ELUDITE_RECORD_FIXTURES=1 ELUDITE_CHROME_NO_SANDBOX=1 cargo test -p eludite-browser --test chrome   # re-record the tree
```

## 9. Deviations, decisions and open points

1. **`read_page` on large pages** reads element by element instead of `Accessibility.getFullAXTree` (section 4,
   item 1). The budget passes when the machine is not saturated, with little headroom.
2. **The profile's `.gitignore`.** The brief says to add the profile to "the `.gitignore` entry the agents' policy file
   uses, if one exists"; none exists (`.eludite/agents-policy.json` is meant to be committed). The launcher writes
   `.eludite/browser/.gitignore` containing `*`, so the profile is ignored whatever the workspace's own `.gitignore`.
3. **The cache layout** is `~/.cache/eludite/chrome/<version>/chrome-<platform>/chrome` (how the Chrome for Testing
   zip unpacks), where the brief wrote `<version>/<platform>/chrome`; discovery and the fetch scripts agree.
4. **Browser commands on the UI thread fail at once** rather than run: nothing in the shell invokes them there until
   brief B's window.
5. **CI** sets `ELUDITE_CHROME_NO_SANDBOX=1` for the Linux job, because Ubuntu's AppArmor policy on the hosted runners
   blocks the user namespaces Chrome's sandbox needs. Not verified: CI was not run.
6. **The kill-and-relaunch step** of the Chrome test runs on Unix only (it finds the browser's pid with `ps`).
7. **`main` moved** to `5143b89` (brief 0022) and the branch was not rebased, as instructed. A trial merge
   (`git merge-tree`) conflicts in three places where both briefs append to shared lists:
   `crates/eludite/src/shell/settings.rs` (the settings table and `Applied`), `docs/briefs/README.md` (the index) and
   `tools/run-dev.sh` (the environment it exports); keeping both sides resolves each.
8. **One unexplained failure** of the Chrome test against Chrome for Testing, in its first run (a command failed in
   the test's `Run::ok`; the message was lost), was not reproduced in more than 20 later runs against both browsers.

## 10. What brief 0024 needs

- **Acting on the page:** `input` over `Input.dispatchMouseEvent`, `dispatchKeyEvent` and `insertText` (generated),
  with refs turned into points through `DOM.scrollIntoViewIfNeeded` and `DOM.getContentQuads` (generated), and the
  console errors emitted during the action (`TabState::console_errors_since` exists); `form_input` through
  `Runtime.callFunctionOn` on resolved refs (and `DOM.setFileInputFiles` for files); `upload` likewise.
- **`storage` and `network_body`:** the `Storage` and `DOMStorage` domains are not generated yet (add them to the
  generator's roots and regenerate); `Network.getResponseBody` needs the bodies kept (`Network.enable`'s buffer sizes)
  and a ring entry's `request_id`, which is there.
- **The policy object:** `agents-policy.json` gains `browser` (`origins`, `network_bodies`, `evaluate`); `navigate`
  outside the allowed origins becomes `dangerous`. The command bus gives a command one class; a class that depends
  on the input (the url) needs a per-call hook in the permission gate, which is a structural change to decide first.
- **Transcript thumbnails:** the screenshot is already MCP image content; the Agents window must render image
  content from a tool call's result.
- **The fake-agent form proof:** `tests/fixtures/form.html` and the test server in `tests/chrome.rs` are reusable.
- Size: about one agent-week, most of it `input` and the policy gate.
