# Brief 0032: The Web Browser window

Status: done on Linux (Xvfb, software rendering); [report](0032-report.md)
Phase: 2 (proposal 0002, brief B)
Plan reference: PLAN.md sections 1 (the browser carve-out), 2 (principles 1, 2, 3, 4, 5), 4.9 (the Web Browser window), 5.8, 8 (Visual Studio names and layout), 9, 10 (Phase 2), 12; proposal 0002 sections 3, 4, 5, 7, 8 (B), 11; ADR-0008
Related ADRs: ADR-0002, ADR-0008, ADR-0009
Depends on: brief 0031 (the engine process `eludite-chromium`, `EmbeddedChromium`, `BrowserSurface`, the frame ring), briefs 0023 and 0024 (the commands, the policy, thumbnails). Runs after 0031 merges. The frame-budget verdict of brief 0031 is still open (it needs the owner's GPU run); this brief proceeds on the software path and keeps the partial-upload fallback as its last step, gated on that verdict.

## Goal

View > Other Windows > Web Browser opens the Visual Studio-named tool window with a real Chromium tab inside Eludite: tabs, an address bar, Back, Forward, Reload and Stop, a Visual Studio-style context menu, DevTools as a second tab, the per-workspace profile, and the "Agent is driving" strip with a Stop button while an agent's `eludite.browser.*` call is in flight. The person types, clicks, scrolls, uses the clipboard and IME in the page; the agent drives the same tabs through the commands of briefs 0023 and 0024, which now run against `EmbeddedChromium` when the window is open (`browser.engine` setting: `embedded` default when the engine is present, `external`). `record` joins the command set (a GIF of the tab). Chromium's background requests to Google are silenced, proven with a net log, so the engine makes no network call the user did not ask for. The engine process grows what the spike's report lists under "what brief B needs": popups as tabs, `<select>` popups, cursor changes, clipboard, DevTools, downloads, JavaScript dialogs and permission prompts in the Visual Studio style.

## Files in scope

- `protocol/schemas/` first and alone: `browser-rpc/` additions (`tab/cursor`, `tab/popup`, `tab/dialog` and its answer, `tab/download`, `tab/permission` and its answer, `tab/devtools`, `tab/title`, `tab/favicon`, `tab/loading`, `tab/select-popup` frames flagged in `tab/frame`), `browser-record.{input,output}.json`, `browser-devtools.{input,output}.json` (the proposal's `devtools` command), `settings.json` (`browser.engine`, `browser.homePage`, `browser.showDevToolsTab`), `view-show.input.json` if the window id needs listing (`web_browser`), the `browser-tabs` output (`favicon`, `loading` exist? add what is missing).
- `browsers/chromium/**`: the handlers above (`CefLifeSpanHandler` popups, `CefRenderHandler` popup rects and cursor, `CefDisplayHandler` title, favicon, loading state, status text, `CefDialogHandler`, `CefJSDialogHandler`, `CefDownloadHandler`, `CefPermissionHandler`, `CefContextMenuHandler` returning our own menu model), `ShowDevTools` as a windowless tab, the background-request switches and preferences (`--disable-background-networking`, `--disable-component-update`, `--disable-sync`, `--disable-features=...`, `kAccountConsistency` off, Safe Browsing off, the `net-log` test), IME (`ImeSetComposition`, `ImeCommitText`), the full key table; the engine's tests.
- `crates/browser/src/embedded.rs` (the new notifications and methods; `record`), `crates/browser/src/browser/*.rs` (`record` from the ring: frames at `fps` into a GIF with a small GIF encoder written here, no new dependency, or `image`'s GIF encoder which is already in the build: say which), the engine selection, screenshots from the ring when visible.
- `crates/eludite/src/shell/browser_window.rs` (new: the tool window on `BrowserSurface`: tab strip with favicons and loading spinners, address bar with Go and the history dropdown, Back, Forward, Reload, Stop, Home, DevTools button, the context menu (Back, Forward, Reload, Copy, Paste, Select All, Save Image As..., Copy Link, Open in External Browser, Inspect), the "Agent is driving" strip with Stop (which cancels the agent's in-flight call and interrupts its `wait` with `interrupted_by: "user"`), JavaScript dialogs and permission prompts as Visual Studio-style dialogs, downloads to the workspace's `.eludite/browser/downloads` with an Output line, focus and input forwarding including IME and the clipboard), `shell/browser.rs` (engine factory by setting, the window's lifecycle, closing with the workspace), `shell/browser_view.rs` (cursor, popup overlays, scale factor), `shell/agents/transcript.rs` (nothing new unless `record` output needs a row), `shell.rs` and `crates/docking` (the window id `web_browser`, View > Other Windows > Web Browser, default layout: a document-area tab, as Visual Studio's), `crates/ui/**` (the strip, the address bar control, favicon images).
- `crates/eludite/tools/browser-window-linux.sh` and `browser_window.py` (the Xvfb run: open the window, navigate to a local fixture, type into a field, open DevTools, let an agent drive and screenshot the strip), screenshots under `crates/eludite/screenshots/`.
- `.github/workflows/ci.yml` only if a test needs a new step; `CLAUDE.md`, `README.md` if the layout changes; `docs/briefs/README.md`; `docs/briefs/0032-report.md` (new).

## Contract

- **The window** is a tool window named Web Browser (id `web_browser`), opened from View > Other Windows > Web Browser, docked as a document tab by default, one per shell, holding the tabs of the workspace's browser. Closing the window keeps the engine running for 60 s (so a reopen is instant) and then closes it; closing the workspace closes it at once. Keys: Ctrl+L focuses the address bar, F5 reloads, Alt+Left and Alt+Right navigate, Ctrl+T opens a tab, Ctrl+W closes it, F12 opens DevTools, Escape stops loading, all only while the window has focus.
- **Both drivers.** `tabs`, `tab_open`, `tab_close`, `tab_select`, `navigate`, `resize` and the rest act on the window's tabs; the window's toolbar runs the same commands (so they are audited). While an agent's call is in flight the strip reads "Agent <name> is driving" with Stop; the person's input in the page or on the toolbar ends the agent's `wait` with `interrupted_by: "user"` (as brief 0027 does for the debugger), and the next action command from that agent is refused until it reads `tabs` again (a `page_generation` check).
- **Engine selection.** `browser.engine` is `embedded` when `eludite-chromium` and CEF are found (the setting's description names `tools/cef/fetch.sh`), else `external`; the Agents window and the commands use whichever is active; the window shows a message with the fetch command when embedded is unavailable and the window was opened.
- **Privacy.** The engine, launched with the profile, makes no network request other than the pages the person or the agent navigate to: proven by a test that runs the engine with `--log-net-log` for 10 s on `about:blank` and asserts no request leaves (the spike's report lists `accounts.google.com`, `update.googleapis.com`, `www.google.com` and `www.google.com/async/folae`; each is turned off by a switch, a preference or a feature flag, listed in `browsers/chromium/README.md`). The `tools/cef/README.md` names the switches and why.
- **Dialogs.** JavaScript `alert`, `confirm` and `prompt`, `beforeunload`, permission requests (geolocation, notifications, camera, clipboard read), file choosers and authentication prompts are Visual Studio-style dialogs in the shell (never Chromium's), answered through the control protocol; an agent's `input` that triggers a dialog gets it in the output (`dialog: { kind, message }`) and `eludite.browser.dialog` (new, execute) answers it (`accept`, `dismiss`, `text`). Downloads go to `<workspace>/.eludite/browser/downloads/` with an Output line and no prompt; files larger than 100 MB are refused with a message.
- **DevTools.** `devtools` (execute) and the toolbar button open Chromium's DevTools front end as a second tab bound to the page (`ShowDevTools` windowless), closed with its tab.
- **`record`** (execute): `action` (`start`, `stop`), `path?` (under the workspace or the cache), `fps` (default 10, max 30), `max_seconds` (default 60, max 600). Frames come from the ring at the rate; `stop` answers the GIF path, frame count and duration; a thumbnail of the first frame goes to the transcript row.
- **Frame cost.** If the owner's GPU run of brief 0031 (section 8 of its report) reports p99 over 8 ms, the last step of this brief implements partial uploads: the shell keeps a tiled image (256 by 256 tiles) and re-creates only the tiles the dirty rectangles touch, measured with the spike's bench. If the run reports under 8 ms, the step is skipped and the report says so. The decision is read from a line the owner adds to `crates/eludite/results/README.md` or communicated in the brief's Status line; absent any, the brief implements the tiles (they help the software path too).
- The UI thread never waits on the engine; the engine's stdout carries the protocol only; nothing of CEF is loaded in the shell; no network at startup; no `--no-sandbox` without the explicit variable.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Engine tests (CEF present, Xvfb or headless Ozone): popups become tabs, a `<select>` popup frame is flagged, a cursor change arrives, DevTools opens as a tab, a JavaScript dialog round-trips, a download lands, a permission request round-trips, the net-log test shows no outbound request on `about:blank`, IME composition commits text, the key table maps every key the shell sends.
- `crates/browser`: `record` produces a GIF with the expected frame count from a fake engine's frames; the engine selection by setting; `dialog`.
- Headless shell tests (fake engine): the window opens from the menu with a tab, the address bar navigates through the bus, Back and Forward enable and disable, the tab strip follows `tabs`, Ctrl+T and Ctrl+W, the strip appears while a fake agent's call is in flight and Stop interrupts its `wait`, the person's click ends the agent's wait with `interrupted_by`, a dialog from the engine shows as a shell dialog and its answer reaches the engine, downloads write the Output line, closing the workspace closes the engine, the engine-missing message.
- The Xvfb run (`browser-window-linux.sh`): screenshots of the window with a page, the DevTools tab, a dialog, and the "Agent is driving" strip while the fake agent (brief 0024's scenario) fills the form; recorded in the report.
- The frame bench of brief 0031 rerun after the tiles step (if done) with the numbers in the report.

## Budget

- Window open to the first page pixel, engine running: under 300 ms; engine cold: under 2 s (brief 0031 measured 267 ms to the first frame).
- `input` click to the next painted frame in the window: under 50 ms p95 (measured by the ring's sequence in the Xvfb run; the software caveat applies).
- Shell memory with the window open: plus 60 MB at most on a GPU machine (reported here with the lavapipe caveat).
- No new dependency; `image`'s GIF encoder counts as existing.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`, `cargo test --workspace --features eludite-chromium/cef` green with the engine tests running under Xvfb here; `dotnet build` and `dotnet test` unchanged.
2. The report records the budget numbers, the privacy switches and the net-log result, the dialogs and prompts matrix, and what briefs C (launch integration) and E (packaging) need.
3. `CLAUDE.md`, `README.md`, the briefs index and `browsers/chromium/README.md` match the repository.

## Out of scope

- Launch integration with F5 (brief C), JavaScript debugging (brief D), packaging and the sandbox helper installation (brief E).
- The accelerated offscreen path and any GPUI patch; bumping GPUI.
- Bookmarks, history sync, extensions, the user's own profile (never).
- Windows and macOS runs.
