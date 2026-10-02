# Proposal 0002: The Web Browser window and agent control of it

Status: Accepted, 2026-10-02 (PLAN.md v0.5 carries its section 10 changes; ADR-0008 accepted)
Plan reference: PLAN.md sections 1 (non-goals), 2 (principles 1 to 3, 5), 3 (D2), 4.9, 5.1 to 5.4, 7 (web row), 9, 10 (Phases 2 and 4), 11
Related: ADR-0002, ADR-0008 (proposed with this document), proposal 0001 (multi-session debugging, js-debug)
New paths: `crates/browser`, `browsers/chromium` (`eludite-browser`), `tools/cef/`, `protocol/schemas/browser-*.json`, `protocol/cdp/`

## 1. Goal

A Web Browser tool window (View > Other Windows > Web Browser, the Visual Studio name) renders the user's web application with a real browser engine inside Eludite. F5 on a web project opens the app there instead of in the system browser. An agent drives the same tabs through commands modeled on Claude in Chrome: navigate, screenshot, read the page as an accessibility tree with stable references, click, type, fill forms, evaluate JavaScript, read the console and the network log, wait for conditions, resize, record. The person sees every action live, can take over at any moment, and the page's JavaScript debugs in the same debugger windows as the server's C# through a second DAP session.

The window exists for testing and for agents. It is not a general-purpose browser: no bookmarks, sync, extensions or profile sharing with the user's own browser.

## 2. The engine

ADR-0008 records the decision. The summary:

**Chromium, through the Chromium Embedded Framework (CEF), out of process.** CEF is BSD-3-Clause, supports windowless (offscreen) rendering on Linux, Windows and macOS, exposes the Chrome DevTools Protocol (CDP), and has maintained Rust bindings (`cef` crate, Apache-2.0 OR MIT, tracking CEF 154 as of 2026-01; `wef`, Apache-2.0, is an offscreen-rendering reference). Chromium is what most users' users run, so it is the engine to test against.

| Option | Verdict | Why |
|---|---|---|
| Chromium via CEF | Chosen | Fidelity, CDP for every automation need, offscreen rendering on all three OSes, permissive license, Rust bindings maintained. Cost: about 100 MB per platform fetched on first use, a sandbox helper on Linux, a helper-bundle layout on macOS. |
| Servo | Not now; second engine later | Rust, MPL-2.0 (GPL-compatible), an embedding API that matured in 2025 and 2026 (delegate-based WebView API), DevTools gaining breakpoints in 0.0.6, WebDriver and accessibility work in progress. Web compatibility is not yet at the level a tester needs, there is no CDP so every automation command would be written against Servo internals, and the build is ten minutes or more. The command layer below is engine-neutral so Servo can slot in once it passes a bar ADR-0008 states. |
| Firefox (Gecko) | Not possible | No desktop embedding API; GeckoView is Android-only. Firefox remains reachable as an external browser over WebDriver BiDi in a later brief (section 9). |
| Platform web views (WebView2, WKWebView, WebKitGTK) | Rejected | Three engines with three behaviors, no CDP on WebKit, and invariant 11. |
| No embedding: drive the user's installed Chrome over CDP, show screenshots | Rejected as the product, kept as the first spike step | Weak for the person (no live page in the IDE), but the automation commands are the same code, so brief A proves them against an external Chrome before the embedding lands. |

Invariant 11 ("no Electron, Tauri, WebViews or a VS Code extension runtime") and PLAN.md section 1 ("not a WebView in a native frame") are about the IDE's own user interface. ADR-0008 states the carve-out exactly: the engine renders user content inside a tool window and never draws any part of Eludite. No IDE panel, dialog or editor is HTML.

## 3. Process model

```
eludite (shell, GPUI) ──JSON-RPC over stdio──▶ eludite-browser (CEF browser process)
         ▲                                            │ spawns CEF renderer, GPU and utility
         │ shared-memory frame ring                   │ subprocesses (same executable, --type=)
         │ CDP messages, both ways                    ▼
     Web Browser tool window                     per-workspace profile dir
```

- `eludite-browser` (`browsers/chromium`, GPL) is CEF's browser process. It is launched the first time the Web Browser window opens, never at startup, so the cold-start budget is untouched and libcef (hundreds of megabytes mapped) stays out of the shell. A crash loses the tabs, not the IDE (invariant 2). CEF's own subprocesses use the same executable with `browser_subprocess_path`.
- **Control** is JSON-RPC over stdio with the same Content-Length framing as the host, schemas in `protocol/schemas/browser-rpc/`: create and close tabs, resize, input events, navigation, and a `cdp` pass-through method carrying CDP messages and events for a tab.
- **Pixels.** Version 1 uses CEF's software offscreen path: `OnPaint` delivers BGRA with dirty rectangles; the browser process copies into a double-buffered shared memory region (`memfd` on Linux, a file mapping on Windows, `shm_open` on macOS) and signals the shell with a frame sequence number and the dirty rectangles. The shell wraps the buffer as a `RenderImage` and draws it with GPUI's `img`. GPUI's `surface` element is macOS-only at the pinned revision, so there is no zero-copy path off macOS yet. The spike (section 8) measures the upload cost; the budget is the shell's frame cost under 8 ms p99 while a 1600 by 1000 tab plays a 60 fps animation. If software upload misses it, the fallbacks in order are: partial atlas updates from dirty rectangles; CEF's accelerated offscreen path (`OnAcceleratedPaint`: D3D11 shared handles, IOSurface, and DMA-BUF on Wayland, which landed in CEF during 2025) imported through a GPUI external-texture element carried as a patch on our pinned GPUI with an ADR note.
- **Input.** GPUI mouse, wheel, keyboard and IME events are forwarded as CEF events with the tab's coordinates; focus follows the tool window. The context menu is ours, in the Visual Studio style, not Chromium's. Clipboard goes through the engine's own clipboard integration.
- **Profile.** Each workspace gets its own profile directory under `.eludite/browser/` (gitignored by the workspace-open code), so cookies, storage and logins are isolated from the user's personal browser. Agents never act with the user's real sessions. "Clear browsing data" is a command.
- **DevTools.** Chromium's DevTools front end opens as a second tab for a page (`ShowDevTools`); it costs nothing and is what web developers expect. It is user content for this purpose, not IDE UI.
- **Binary acquisition.** `tools/cef/fetch.sh` (and `.ps1`) downloads the pinned CEF minimal distribution for the platform from the Spotify-hosted builds, checks its SHA-256, and unpacks to `~/.cache/eludite/cef/<version>/`. Discovery: beside the executable, `ELUDITE_CEF`, the cache. Without it, the window shows what to run, as the debugger does for netcoredbg. No network call happens unless the person opens the window and accepts the download. CEF builds without proprietary codecs, so H.264 and AAC media do not play; the window says so when a page tries.

## 4. The command surface

All ids are `eludite.browser.*`, schemas in `protocol/schemas/browser-<name>.{input,output}.json`, every command `agent_visible: true` and reachable from the window's toolbar, context menu or address bar. `tab` is optional everywhere and defaults to the active tab. Modeled on Claude in Chrome's tools, with the differences noted in section 5.

### 4.1 Tabs and navigation

| Command | Class | Input | Output |
|---|---|---|---|
| `tabs` | read | | Tabs: id, url, title, active, loading, `page_generation` |
| `tab_open` | execute | `url?` | The tab |
| `tab_close` | execute | `tab` | |
| `tab_select` | execute | `tab` | |
| `navigate` | execute; dangerous outside the origin policy | `url` or `action` (`back`, `forward`, `reload`), `wait_until` (`load` default, `domcontentloaded`, `network_idle`, `none`), `wait_ms` | Final url, title, status, `page_generation`, console error count during the load |
| `resize` | execute | `width`, `height`, `device_scale_factor?`, `mobile?`, `user_agent?` | The viewport in effect |

### 4.2 Reading the page

| Command | Class | Input | Output |
|---|---|---|---|
| `screenshot` | read | `full_page?`, `clip?` (ref or rectangle), `max_width` (default 1280), `format` (`png` default, `jpeg`) | The image (MCP image content) plus width, height, scale, `page_generation` |
| `read_page` | read | `mode` (`accessibility` default, `dom`), `filter` (`interactive` default, `all`), `root?` (ref), `max_nodes` (default 500) | A compact tree: role, name, value, state, bounding box, `ref`; `truncated`, `page_generation` |
| `find` | read | `css?`, `text?`, `role?` + `name?`, `max` | Matching refs with role, name, box |
| `page_text` | read | `root?`, `max_chars` (default 20,000), `cursor?` | Text, `next` cursor |
| `console` | read | `since` (cursor), `level?`, `pattern?`, `max` | Messages with level, text, source location, stack for errors; `next`, `dropped` |
| `network` | read | `since`, `url_pattern?`, `status?`, `resource_type?`, `max` | Requests with method, url, status, type, timing, sizes, initiator; `next` |
| `network_body` | read; policy `browser.network_bodies` | `request_id`, `max_bytes` | Response headers and body (text or base64) |
| `wait` | read | `for` (`selector`, `text`, `navigation`, `network_idle`, `console` with pattern, `function` with an expression), `wait_ms` | What was satisfied, `page_generation`, or `timeout` |

`ref`s come from CDP backend node ids captured by `read_page` and `find`; they are valid until the `page_generation` changes (navigation or a DOM replacement the shell detects through `DOM.documentUpdated`), and a stale ref is refused with the current generation.

### 4.3 Acting on the page

| Command | Class | Input | Output |
|---|---|---|---|
| `input` | execute | `action` (`click`, `double_click`, `right_click`, `hover`, `type`, `key`, `scroll`, `drag`, `select`, `focus`), `ref?` or `x`,`y`, `text?`, `keys?`, `delta?`, `to?`, `values?`, `modifiers?` | What was done and the element it landed on (role, name), `page_generation`, console errors emitted during the action |
| `form_input` | execute | `fields`: list of `ref` with `value` (text, checked, selected values, file paths) | Per-field result |
| `evaluate` | execute | `expression`, `await?`, `return_by_value?` (default true), `max_chars` | JSON result or the exception with its stack |
| `upload` | execute within the workspace; dangerous outside it | `ref`, `paths` | |
| `storage` | read for `get`; execute for `clear` | `kind` (`cookies`, `local`, `session`, `all`), `action`, `origin?` | |
| `record` | execute | `action` (`start`, `stop`), `path?` (under the workspace or the cache), `fps` (default 10), `max_seconds` | On stop, the GIF path and frame count |
| `devtools` | execute | `tab` | Opens DevTools for the tab |
| `open_external` | execute | `url?` | Opens the url in the system browser (the existing behavior, kept as a command) |

### 4.4 Launch integration

- `eludite.debug.start` on a web project honors the launch profile's `launchBrowser` and `launchUrl` by opening the url in the Web Browser window when `browser.useBuiltIn` is on (default: on when the engine is present). "Start in external browser" remains in the Debug menu.
- Attach: a tab's CDP endpoint is a debug target. With proposal 0001's multi-session work, a compound launch starts the server under netcoredbg and attaches vscode-js-debug to the tab, so the Call Stack shows both processes and a breakpoint in TypeScript and one in C# are the same experience.
- The agent's `eludite.browser.*` calls and its `eludite.debug.*` calls refer to the same tab, so "set a breakpoint in the click handler, click the button, read the locals" is three commands.

## 5. Differences from Claude in Chrome, on purpose

- **Commands, not a browser extension.** Everything is a bus command with a schema, audited, policy-gated, and usable by the person from the window. Nothing runs inside the page except what `evaluate` is asked to run.
- **`find` is deterministic.** Claude in Chrome's `find` asks a model to locate "the login button". Eludite does not call a model inside the IDE; `find` takes CSS, text or role and name, and agents reason over `read_page`.
- **Refs over coordinates.** `read_page` returns refs and `input` prefers them; coordinates remain for canvas and vision-driven cases, taken in the same CSS pixel space the screenshot reports.
- **Isolated profile.** Claude in Chrome acts in the user's logged-in browser. Eludite's browser never has the user's sessions unless the user logs in inside it.
- **Origin policy.** Navigation off the allowed origins is `dangerous` (prompt by default). `agents-policy.json` gains `browser`: `origins` (default: `localhost`, `127.0.0.1`, `::1`, the workspace's launch urls, and `file://` under the workspace), `network_bodies` (`allow` default, `deny`), `evaluate` (`allow` default, `prompt`, `deny`).
- **Both drivers, visibly.** While an agent's call is in flight the tab shows an "Agent is driving" strip with a Stop button; the person's input ends the agent's `wait` with `interrupted_by: "user"` as in proposal 0001. Screenshots the agent took appear as thumbnails in its transcript row, so the person sees what it saw. Every action is an audit entry with the tab, the ref or point, and the resulting `page_generation`.

## 6. Protocol artifacts

- `protocol/cdp/`: the pinned Chromium `browser_protocol.json` and `js_protocol.json` (BSD-3-Clause) for the CEF version, and the generator that produces the Rust domain types used by `crates/browser`. Generated code is never hand-edited (invariant 4). Only the domains the commands use are generated at first: Page, DOM, Accessibility, Runtime, Log, Network, Input, Emulation, Target.
- `protocol/schemas/browser-rpc/`: the shell-to-engine control methods.
- `protocol/schemas/browser-*.json`: the commands above.
- `agents-policy.json`: the `browser` object.

## 7. Budgets

| Metric | Budget |
|---|---|
| Window open to the first page pixel, engine already running | < 300 ms |
| Window open, engine cold (binary present, profile warm) | < 2 s |
| Shell frame cost while a 1600 by 1000 tab plays a 60 fps animation | < 8 ms p99 (the existing keystroke budget; this is the spike's pass or fail) |
| Shell resident memory with the window open | + 60 MB at most over the section 9 budget; the engine's processes are separate |
| `screenshot` round trip at 1280 wide | < 150 ms p95 |
| `read_page` on a 5,000-node page | < 300 ms p95 |
| `input` click to the next painted frame in the window | < 50 ms p95 |
| Cold start of Eludite | unchanged (nothing of the engine loads) |

## 8. Briefs

| Brief | Content | Size (agent-weeks) | Depends on |
|---|---|---|---|
| S. Spike: CEF offscreen into GPUI, out of process | `eludite-browser` with one tab, software `OnPaint` into shared memory, the shell drawing it with `img`, input forwarding, the frame-cost benchmark on Linux and (later) Windows and macOS; the fetch script; the macOS bundle layout and the Linux sandbox findings; a report with GO or the fallback to try | 1 | none |
| A. Automation commands against an external Chrome | `crates/browser` with the CDP client over a websocket, `protocol/cdp/` and its generator, the commands of 4.1 to 4.3 except `record`, policy and audit, the MCP tools, headless tests against a pinned Chrome for Testing in CI; the fake-agent test "fill the form and read the result" | 2 | none; runs in parallel with S |
| B. The Web Browser window | The production engine process on the spike's result, tabs, address bar, toolbar, the VS-style context menu, DevTools tab, the per-workspace profile, "Agent is driving" strip, transcript thumbnails, `record` | 2 | S, A |
| C. Launch integration | `launchBrowser` into the window, `open_external`, the setting, the Debug menu items, the `wait_until` paths tied to the server's readiness | 0.5 | B, brief 0020 |
| D. JavaScript debugging | vscode-js-debug fetch and discovery, attach to a tab, compound launch with netcoredbg, source maps verified on a Vite and an ASP.NET `wwwroot` project | 1 | proposal 0001 D and E3 |
| E. Packaging | Windows and macOS runs, installer layout for the helper, the sandbox on Linux distributions that restrict user namespaces (the SUID helper from CEF's distribution; a clear message and an explicit opt-in before any `--no-sandbox`) | 1 | B |

Total: about 7.5 agent-weeks after the spike, of which A can start now.

## 9. Later, named so they are not forgotten

- **External browsers over WebDriver BiDi** (Firefox, Safari, the user's Chrome) behind the same commands for cross-browser checks, Playwright-style, without embedding.
- **Servo as a second engine** behind `crates/browser`'s engine trait, when ADR-0008's bar is met.
- **A Rust DAP adapter over CDP** replacing vscode-js-debug if the Node dependency proves unacceptable (proposal 0001, risks).
- **Accelerated offscreen rendering** (shared textures) if the software path is only just inside budget.
- **Browser Link-style live reload** for WebForms and Razor markup edits.

## 10. Changes to PLAN.md on acceptance

- 1: after "not a WebView in a native frame", add that a browser engine may render the user's web application inside a tool window and never draws the IDE.
- 4.9: add the Web Browser window and the launch integration; "Launch: ... browser launch and attach" points here.
- 5: add 5.8 "Agent control of the browser" with a pointer to section 4 here; 5.3 gains the `browser` policy object.
- 7, web row: the debugging column names the embedded browser as the js-debug target.
- 10: Phase 2 gains the window (briefs S, A, B, C); Phase 4 gains the agent-driven testing scenario (full-stack breakpoints, D).
- 12: `crates/browser`, `browsers/chromium`, `tools/cef`, `protocol/cdp`.
- 14: decision 13, the engine, pointing to ADR-0008.

## 11. Risks

- **Frame upload cost in GPUI.** The whole design rests on the spike's number. The fallbacks are listed in section 3 and are real, but the accelerated path means a GPUI patch.
- **Download size and trust.** 100 MB per platform fetched from a third-party CDN by checksum. The fetch is explicit, pinned and verified; a proxy or offline path (point `ELUDITE_CEF` at a copy) is documented.
- **macOS packaging.** CEF requires helper app bundles inside the application bundle; development runs from `cargo run` need a bundling script. Brief S finds out how painful.
- **Linux sandbox.** Distributions that restrict unprivileged user namespaces need the SUID helper. Never disable the sandbox silently.
- **Agent misuse of a live browser.** The isolated profile, the origin policy and the audit trail are the mitigations; the person can watch and stop at any time. Nothing in the window has the user's own credentials unless they typed them there.
- **CEF version churn.** Chromium releases every four weeks; CEF follows. Pin, update deliberately through `tools/cef/`, and keep the generated CDP types in step.
