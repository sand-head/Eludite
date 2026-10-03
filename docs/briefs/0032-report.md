# Brief 0032 report: The Web Browser window

Status: done on Linux (Xvfb, software rendering). Windows and macOS: not run (out of scope; the new code is
platform-neutral, the engine is Linux-only as in brief 0031). CI: not run (nothing pushed). Branch:
`brief/0032-web-browser-window`, based on `main` at `6a07897`. Date: 2026-10-03.
Brief: [0032-web-browser-window.md](0032-web-browser-window.md).

## 1. Summary

- **View > Other Windows > Web Browser** opens the Visual Studio-named window (id `web_browser`) as a document tab, one
  per shell, on a real Chromium tab of `eludite-chromium`: a tab strip with favicons and loading spinners, Back,
  Forward, Reload or Stop, Home, the address bar with Go and its history drop-down, a DevTools button, the shell's
  context menu, the shell's dialogs, a status line, and the yellow "Agent <name> is driving this browser" strip with
  Stop. The person types, clicks, scrolls, uses the clipboard and IME in the page.
  ([page](../../crates/eludite/screenshots/linux-browser-window-page.png),
  [DevTools](../../crates/eludite/screenshots/linux-browser-window-devtools.png),
  [dialog](../../crates/eludite/screenshots/linux-browser-window-dialog.png),
  [agent driving](../../crates/eludite/screenshots/linux-browser-window-agent-driving.png),
  [agent done](../../crates/eludite/screenshots/linux-browser-window-agent-done.png).)
- **Both drivers, one set of commands.** The toolbar, keys, tab strip and menu run `eludite.browser.*` as the person
  (audited); agents run the same commands on the same tabs. While an agent's call is in flight the strip shows; the
  person's click, key or wheel in the page, or a toolbar command, ends the agent's `wait` with
  `interrupted_by: "user"`, Stop fails its call, and that agent's next action command is refused until it reads
  `tabs` again.
- **`browser.engine`** (`embedded`, the default, or `external`) picks the engine; without `eludite-chromium` or CEF
  the commands use the external Chrome and the window says what to run (`tools/cef/fetch.sh` and the build command).
- **New commands:** `record` (a GIF of the tab from the frame ring, `image`'s GIF encoder, no new crate),
  `devtools` (Chromium's DevTools front end as a windowless tab bound to the page) and `dialog` (answer a JavaScript
  dialog an agent's `input` reported in its output); `navigate` gains `stop`, `wait` gains `interrupted_by`.
- **Privacy:** the engine makes no request on `about:blank` in 10 s of net log (`requests: []`, 61 KB of log); seven
  background destinations are each turned off by a switch, feature, preference or policy (section 6).
- **Partial uploads** (the frame-cost fallback; the owner's GPU run has not been reported, so the brief's default
  applies): the page is drawn as 256 by 256 tiles and only the tiles a frame's dirty rectangles touch are re-created,
  including across frames the ring overwrote before the shell drew them. On a still page with a moving 200 by 200 box
  the browser's CPU per drawn frame falls from 2.9 ms to 0.5 ms p50 (section 5). The frame-cost verdict itself still
  needs a GPU.
- **Budgets** (Xvfb, lavapipe, debug builds; section 4): window open to the first page pixel 456 ms with the engine
  cold (budget 2 s), **110 ms** with it running (300 ms); `input` click to the next painted frame in the ring
  **p95 19.4 ms** (50 ms); shell memory **+28 MB** with the window on the order form (60 MB; +76 to +85 MB with the
  1600 by 1000 animation of the bench, section 4.3).
- **Tests:** the full suite with the engine under Xvfb: 740 passed, 0 failed, 1 ignored (section 9).

## 2. What was built

| Commit | What |
|---|---|
| `83caa4d` | Schemas first and alone: `browser-rpc/` `tab/cursor`, `tab/popup`, `tab/dialog`, `tab/dialogAnswer`, `tab/dialogClosed`, `tab/permission`, `tab/permissionAnswer`, `tab/download`, `tab/devtools`, `tab/contextMenu`, `tab/action`, `tab/state` (title, url, favicon, loading, history, status), `popup` in `tab/frame`, `initialize`'s download folder and limit; `browser-record`, `browser-devtools`, `browser-dialog` input and output, `dialog` in `browser-input`'s output, `stop` in `browser-navigate`, `interrupted_by` in `browser-wait`, `favicon`/`loading`/history in `browser-tabs`; `settings.json` `browser.engine`, `browser.homePage`, `browser.showDevToolsTab`; `view-show` lists `web_browser` |
| `1577c10` | The background requests silenced (`browsers/chromium/src/privacy.rs`) and the net-log test |
| `6145b6f` | The engine's handlers: popups as tabs, `<select>` popups drawn into the frame and flagged, cursors, title, favicon, loading and status, JavaScript dialogs, file choosers, authentication, permissions, downloads, the shell's context menu, `tab/action`, DevTools as a windowless tab, IME |
| `618b1ba`, `a60badb` | The dialog schema takes absolute paths; the engine's `tungstenite` line in `Cargo.lock` |
| `8e88b91` | `crates/commands`: `record`, `devtools`, `dialog`, `navigate`'s `stop`, `input`'s `dialog`, `wait`'s `interrupted_by`, the settings |
| `b73262d` | `crates/browser/src/keys.rs`: the key table (GPUI key name to Windows key code, X11 keycode, DOM key and code) |
| `7b6a894` | `crates/browser`: `record` (`browser/record.rs`), `devtools`, `dialog`, the person's `Interrupt`, the engine selection (`select_engine`), `EmbeddedChromium`'s observer for the window and its new notifications and methods |
| `d2a9a20` | The window (`shell/browser_window.rs`), the engine factory by setting, the window's lifecycle and the 60 s linger (`shell/browser.rs`), cursors, popups, IME and tiles in `BrowserSurface` (`shell/browser_view.rs`), View > Other Windows > Web Browser, the `web_browser` document (`crates/docking`), the window's headless tests (saved as the previous agents left them; finished by the commits below) |
| `b34a8b0`, `976fde1` | Partial uploads across frames the ring overwrote: `TabFrames` keeps the last 64 announced frames' dirty rectangles (`FrameSource::dirty_between`), the drawn frame's own come from its header |
| `6697b7f`, `5ee52d0`, `b202fbe` | The Xvfb run (`crates/eludite/tools/browser-window-linux.sh`, `browser_window.py`), the page's bounds in `--bounds-out`, the window's log lines; the bench's still page with a moving box (`ELUDITE_BENCH_BROWSER_PAGE=box`) and its tile counts |
| `cd194ce`, `f096913` | Found by the Xvfb run: Ctrl+L and a click select the whole address (typing replaces it), Go gives the keys to the page, closing DevTools shows the page's address again, an answered dialog gives the keys back to the page |
| `1bdc716` | `tools/cef/README.md`: the switches and why |
| `be03d8c` | The run's screenshots and `crates/eludite/results/linux-browser-window.json` |
| (last) | This report, `browsers/chromium/README.md`, the index |

## 3. The design

- **Process model** (unchanged from brief 0031): the shell never loads CEF; the engine is a child process started by
  the first browser command that needs it; frames come through the memfd ring, control through JSON-RPC on stdio.
  The window and the commands share one `Browser` on the `browser` worker thread; the window gets the engine's
  notifications as `WindowEvent`s on the UI thread (`BrowserBus::window_events`) and a `PageDriver` (the engine's
  `TabControl`) whose calls never wait: input, resize, `tab/action` and the dialog answers are notifications, and
  every command the window runs goes to a thread of its own. The UI thread never waits on the engine.
- **The tabs are the commands' tabs.** The worker announces `(t1, engine tab)` and the active tab after every command;
  the strip draws them; selecting, opening and closing run `tab_select`, `tab_open` and `tab_close`. A popup the page
  opens (`tab/popup`) is adopted by `tabs` and selected. DevTools tabs are the window's alone (not listed by `tabs`),
  shown beside their page and closed with it or by their close button.
- **Who drives.** `BrowserBus` keeps the agents' calls in flight (from the command bus's caller); the person's hand
  (`person_acted`: a click, key, wheel or IME in the page, a toolbar command, a menu item, a dialog answer) marks the
  shared `Interrupt`, which ends a `wait` with `interrupted_by: "user"`; Stop (`stop`) fails the call in flight. Both
  mark the agent stale until it reads `tabs` (the `page_generation` check of the brief).
- **Engine selection.** `select_engine(browser.engine, search)` gives the embedded engine when it was chosen and the
  engine and CEF are found, else the external Chrome with the reason; the worker makes a new `Browser` when the
  choice changes. The window shows the reason with the fetch and build commands.
- **Lifetime.** Closing the window starts a 60 s linger (a reopen within it reuses the engine and its tabs; measured
  110 ms to the first pixel); closing the workspace closes the engine at once.
- **Dialogs** are the shell's (section 7); downloads go to `<workspace>/.eludite/browser/downloads/` without a prompt,
  the status line shows their progress and the Output window's Browser pane their end (`download_line`), and over
  100 MB they are refused.
- **`record`** reads the newest frame of the ring at `fps` (default 10, at most 30) on a thread of its own without
  consuming it (the window keeps drawing), scales to at most 1280 wide, and encodes with `image`'s GIF encoder (the
  `gif` feature of the `image` already in the build through GPUI); a tick with no new frame lengthens the previous
  frame. `stop` answers the path, frame count, duration, size and a PNG thumbnail of the first frame, which MCP sends
  as image content, so the Agents window shows it in the call's row (brief 0024's thumbnails; no transcript change).
  Without the embedded engine, `record` says it needs it.
- **Partial uploads:** section 5.

## 4. The measurements

Xvfb (DISPLAY :99), Mesa lavapipe, no GPU, debug builds with dependencies at opt-level 2, 4 cores shared with another
agent's builds (load average 1.1 to 4.5 during the runs). Results:
[`crates/eludite/results/linux-browser-window.json`](../../crates/eludite/results/linux-browser-window.json).
**Frame costs here are software rasterization and an upper bound.**

### 4.1 Budgets

| Budget | Measured | Where | |
|---|---|---|---|
| Window open to the first page pixel, engine running | **109.9 ms** | Xvfb run, step 6 (the window closed and reopened within the linger; the shell's log line, from `set_open` to the first paint of a page image) | Pass (< 300 ms) |
| Window open to the first page pixel, engine cold | **456.2 ms** (407.7 to 456.2 over 5 runs) | Xvfb run, step 1 (spawn, `initialize`, `tab/create` of `about:blank`, first frame, first paint) | Pass (< 2 s) |
| `input` click to the next painted frame | **p50 16.6 ms, p95 19.4 ms**, max 22.9 (20 clicks) | `crates/browser/tests/embedded.rs`, under Xvfb: `input` click on a page that changes color on mousedown, timed to the ring's first frame showing the change | Pass (< 50 ms p95) |
| Shell memory with the window open | **+28.3 MB** (189.7 to 218.0 MB RSS) on the order form, 954 by 452 | Xvfb run | Pass (< 60 MB) |
| | +76 to +85 MB with a 1600 by 1000 tab animating at 60 fps (the bench: 28 tiles, the atlas's copies) | bench | over (as brief 0031: +83.9 MB); lavapipe keeps textures in system memory, so this counts what a GPU holds in VRAM |
| No new dependency | `image`'s `gif` feature (the `image` 0.25 already in the build); `tungstenite` 0.30 in the engine (MIT OR Apache-2.0, already in the build through `crates/browser`, same features) | `Cargo.lock`: one line, the engine's dependency list | Pass |
| `screenshot` from the ring at 1280 wide | p50 90.2 ms, p95 118.8 ms (20 calls, test build) | `embedded.rs` test | brief 0031's < 150 ms p95 holds |
| `record` | 15 frames for 1.5 s at 10 fps, 800 by 600, 22,704 bytes | `embedded.rs` test | |

### 4.2 Frame cost and partial uploads

Section 5.

### 4.3 Memory

The Xvfb run's +28 MB is the window, the tab's tiles (954 by 452) and the surfaces; the bench's +76 to +85 MB is a
1600 by 1000 tab whose 28 tiles are replaced up to 60 times a second (GPUI's atlas grows before freed tiles are
reused). Brief 0031 measured +83.9 MB with one image per frame; tiles change it by less than the run-to-run spread.
The engine's own processes are unchanged from brief 0031 (PSS about 370 MB, out of the shell).

## 5. Partial uploads (the frame-cost fallback)

The owner's GPU run of brief 0031 (its section 8) has not been reported and `crates/eludite/results/README.md` has no
line about it, so per the contract the tiles are implemented. `BrowserSurface` keeps the frame as 256 by 256 tiles,
each its own `RenderImage`; a frame re-creates only the tiles its dirty rectangles touch and frees their old atlas
entries. When the shell draws fewer frames than the engine paints (here about 10 of 60 a second), the frames in
between were overwritten in the ring; their dirty rectangles are known from their `tab/frame` notifications, which
`TabFrames` keeps for the last 64 frames, so the shell still re-creates only the union. A frame whose predecessors are
unknown, or of a new size, re-creates every tile. `ELUDITE_BROWSER_TILES=0` draws one image per frame (the spike's
path) for comparison.

The spike's bench (`--bench-browser 12` through `browser-spike-linux.sh`), 1600 by 1000 tab, 60 fps engine paint:

| Page | Path | Tiles per upload p50 / p95 | Upload p50 / p95 (ms) | `img` paint p50 / p95 (ms) | Frame cost p50 / p99 (ms) | Baseline without uploads p50 / p99 (ms) |
|---|---|---|---|---|---|---|
| Box (200 by 200 canvas redrawn every frame, the rest still) | tiles, run a | **2 / 2** of 28 | **0.34 / 0.68** | **0.15 / 0.23** | 104.8 / 192.0 | 126.4 / 236.1 |
| | tiles, run b | **2 / 2** | **0.35 / 0.49** | **0.15 / 0.24** | 80.8 / 119.1 | 81.9 / 116.4 |
| | whole frames, run a | 1 image | 1.54 / 2.16 | 1.36 / 1.76 | 93.9 / 128.8 | 85.3 / 107.7 |
| | whole frames, run b | 1 image | 1.57 / 1.88 | 1.43 / 1.86 | 88.7 / 192.0 | 111.2 / 274.6 |
| Animation (canvas covering the view, every pixel every frame) | tiles | 28 / 28 | 2.79 / 4.82 | 1.68 / 2.38 | 95.0 / 142.6 | 79.8 / 107.0 |
| | whole frames | 1 image | 1.57 / 2.06 | 1.39 / 1.85 | 93.4 / 152.8 | 75.4 / 99.5 |

- **What the tiles change:** on the common page (a small area changing) the browser's CPU work per drawn frame falls
  from about 2.9 ms (copy 1.55 + paint 1.38) to about 0.5 ms (0.34 + 0.15) p50, 5.8 times less, and the texture data
  sent to the GPU from 6.4 MB to 0.5 MB per frame. On a page that changes everywhere it costs about 1.5 ms more (28
  images to make instead of one).
- **What it does not show here:** the frame cost is Mesa's software rasterizer drawing the 2200 by 1500 window
  (the baseline without uploads is 80 to 126 ms p50 and moves by 30 ms between identical runs on this shared
  machine), so the browser's 2.4 ms saving is inside the noise. Whether the shell's frame is under 8 ms p99 is still
  the owner's GPU run's question (brief 0031 section 8, now with `ELUDITE_BENCH_BROWSER_PAGE=box` and
  `ELUDITE_BROWSER_TILES=0|1` to compare). If whole-frame changes are the problem on a GPU, a full-frame change could
  fall back to one image (the 1.5 ms difference above); not done, since it only matters with a GPU's numbers.

## 6. Privacy: the switches and the net log

The engine, launched with a fresh profile, makes no network request other than the pages the person or an agent
navigates to. `browsers/chromium/tests/engine.rs`'s `the_engine_makes_no_request_on_about_blank` runs it with
`--log-net-log=FILE --net-log-capture-mode=Everything` on `about:blank` for 10 s and asserts the log has no request
to an `http`, `https`, `ws` or `ftp` url, no host resolution and no TCP, UDP, SSL or QUIC connection. **Result: 61,248
bytes of log, `requests: []`.**

| Destination (fresh profile, `about:blank`) | Turned off by |
|---|---|
| `accounts.google.com/ListAccounts` | Account consistency off (`signin.allowed`, `signin.allowed_on_next_startup` false, policy `BrowserSignin: 0`); the account service still lists the cookie jar's accounts, which nothing turns off in Chromium 154, so `--gaia-config-contents` points that one url at `data:,` (each attempt fails inside the engine; signing in to sites with Google is unaffected) |
| `update.googleapis.com/service/update2/json` | `--disable-component-update` and `--component-updater=url-source=data:,` (the on-demand check for the on-device model's manifest runs even with the first), policy `ComponentUpdatesEnabled: false` |
| `www.google.com` (preconnect to the search engine) | `--disable-features=PreconnectToSearch`, `net.network_prediction_options: 2`, policy `NetworkPredictionOptions: 2` |
| `www.google.com/async/folae` (AI Mode eligibility) | `--disable-features=AimEnabled,AimServerEligibilityEnabled,AimServerRequestOnStartupEnabled` |
| `clients2.google.com/time` (network time) | `--disable-features=NetworkTimeServiceQuerying` |
| `dns.google/dns-query` and the system resolver for it | `--disable-features=DnsOverHttpsUpgrade`, `dns_over_https.mode: "off"`, policy `DnsOverHttpsMode: "off"` |
| Safe Browsing list updates | `safebrowsing.enabled: false`, policy `SafeBrowsingProtectionLevel: 0` |

Defense in depth, and the reasons, are in [`browsers/chromium/README.md`](../../browsers/chromium/README.md) and
[`tools/cef/README.md`](../../tools/cef/README.md): `--disable-background-networking`, `--disable-sync`,
`--disable-default-apps`, `--no-pings`, `--disable-domain-reliability`, `--disable-client-side-phishing-detection`,
`--disable-breakpad`, `--disable-field-trial-config`, `--metrics-recording-only`, the features `OptimizationHints`,
`MediaRouter`, `DialMediaRouteProvider`, `Translate`, `CertificateTransparencyComponentUpdater`, `LensOverlay`,
`AutofillServerCommunication`, and managed policies in `<profile>/Policies/managed/eludite.json`. No `--no-sandbox`
without `ELUDITE_CHROME_NO_SANDBOX=1`; no network at Eludite's startup (the engine starts with the first command or the
window).

## 7. Dialogs and prompts

Every dialog is the shell's; Chromium's own never shows. Answers go back through `tab/dialogAnswer` and
`tab/permissionAnswer`; an agent's `input` that opens a JavaScript dialog gets `dialog: { kind, message }` in its
output at once (instead of waiting on the paused page) and `eludite.browser.dialog` answers it (`accept`, `dismiss`,
`text`), which closes the shell's dialog (`tab/dialogClosed`).

| Kind | CEF handler | Shell dialog (title: buttons) | Agent | Engine test (real CEF) | Shell test (fake engine) |
|---|---|---|---|---|---|
| `alert` | `CefJSDialogHandler::OnJSDialog` | Message from Webpage: OK | `input` reports it, `dialog` | yes | yes (closed by the agent's answer) |
| `confirm` | same | Message from Webpage: OK, Cancel | same | yes; also the agent's round trip in `crates/browser/tests/embedded.rs` | yes (OK; Xvfb run screenshot) |
| `prompt` | same | Message from Webpage: a text box, OK, Cancel | same, `text` | yes | yes (typed into) |
| `beforeunload` | `OnBeforeUnloadDialog` | Leave Page: Leave, Stay | same | no (Chromium shows it only after a user gesture on the page; the handler is the same path as `confirm`) | no |
| File chooser (`open`, `openMultiple`, `openFolder`, `save`) | `CefDialogHandler::OnFileDialog` | Choose File: Choose..., Cancel, then the system's file dialog | `upload` (brief 0024) sets files without a dialog | yes (a chosen file reaches the page) | no (the system dialog) |
| Authentication | `CefRequestHandler::GetAuthCredentials` (`--disable-chrome-login-prompt`) | Sign In: user name, password, Sign In, Cancel | not exposed to agents | yes (a 401 challenge answered) | no |
| Permissions: geolocation, notifications, camera, microphone, clipboard read, and the rest CEF names | `CefPermissionHandler` (`OnShowPermissionPrompt`, `OnRequestMediaAccessPermission`) | Permission Request: Allow, Block | not exposed to agents (the person decides) | yes (geolocation denied) | yes (blocked) |
| Downloads | `CefDownloadHandler` | no dialog: status line, Output line; over 100 MB refused | `Save Image As...` / `tab/action download` | yes (a download lands; the limit) | yes (the Output line) |
| Context menu | `CefContextMenuHandler::RunContextMenu` (Chromium's cancelled) | Back, Forward, Reload, Copy, Paste, Select All, Save Image As..., Copy Link, Open in External Browser, Inspect | n/a | yes (`tab/contextMenu`) | yes (Copy Link, Copy) |

## 8. How to reproduce

```
export CEF_PATH="$(tools/cef/fetch.sh)"
cargo build -p eludite -p eludite-acp -p eludite-chromium --features eludite-chromium/cef
# As root (a container) the engine needs ELUDITE_CHROME_NO_SANDBOX=1.
crates/eludite/tools/browser-window-linux.sh /tmp/bw          # the window's run: screenshots, budgets, browser-window.json
ELUDITE_BENCH_BROWSER_PAGE=box crates/eludite/tools/browser-spike-linux.sh /tmp/box 12        # tiles
ELUDITE_BENCH_BROWSER_PAGE=box ELUDITE_BROWSER_TILES=0 crates/eludite/tools/browser-spike-linux.sh /tmp/box0 12
cargo test -p eludite-chromium --features cef                  # the engine against the real CEF
cargo test -p eludite-browser --test embedded -- --nocapture   # the commands, click to frame, record
cargo test -p eludite browser                                  # the window, headless GPUI, fake engine
```

## 9. Tests

| Where | Tests | What |
|---|---|---|
| `browsers/chromium/tests/engine.rs` (real CEF, Xvfb) | 7 new | No request on `about:blank` in 10 s of net log; popups become tabs keeping `window.opener`; a `<select>` list drawn into the frame and flagged, picked with the keys, cursor changes, the context menu; `alert`, `confirm`, `prompt` answered by the shell and a file chooser; a geolocation request denied, an authentication challenge answered, downloads landing and refused over the limit; DevTools as a tab, closed with its page; IME composition and commit, history and the favicon in `tab/state` |
| `browsers/chromium/src/privacy.rs`, `window.rs` | 4, 3 | The profile seeded keeping its keys, the account check and updater pointed off the network, feature lists merged, each feature named once with what it silences; popup rectangles clipped to the view, cursor, permission and disposition names, safe unique download names |
| `crates/browser/src/keys.rs` | 1 | The shell's key table: every key the window sends has a Windows code, an X11 keycode, a DOM key and code, none ambiguous |
| `crates/browser/src/browser/record.rs`, `window.rs` | 2, 1 | A moving page gives one GIF frame per tick and a still one a single long frame; wide frames scaled to 1280; interrupts count from a call's start |
| `crates/browser/tests/window.rs` (fake engine) | 3 | `record` makes a GIF with one frame per tick of the engine's frames; `input` reports the dialog it opened and `dialog` answers it; `devtools`, `navigate`'s stop, and the person's hand ending `wait` with `interrupted_by: "user"` and failing a stopped call |
| `crates/browser/tests/embedded.rs` (real engine) | 1 new | Every key of the table reaches the page with its key code, key and code; an agent's click opening `confirm` gets it in its output and `dialog` accepts it; DevTools opens as a tab `tabs` does not list; `record` of an animating page (8 to 16 frames for 1.5 s at 10 fps; 2 or more when the load average is over 4), 800 by 600, `GIF89a`; click to the next frame p95 under 50 ms (asserted unless the machine is loaded) |
| `crates/browser/src/embedded.rs` | 2 new, 1 extended | The engine follows the setting and what is found; a download's end makes the Output line and its progress none; dirty rectangles of announced frames, known only when each was announced |
| `crates/commands/src/browser.rs` | 1 | `record`, `devtools`, `dialog` and `stop` parse and refuse bad input |
| `crates/docking` | 1 | `view.show web_browser` opens a document tab, once, activated again |
| `crates/eludite/src/shell/browser_window_tests.rs` (headless GPUI, fake engine) | 7 | The window opens from View > Other Windows > Web Browser with a tab and nothing at startup; the address bar navigates through the bus (audited); Back and Forward enable and disable (Alt+Left, the toolbar); F5 reloads only inside the window; Ctrl+L selects the address so typing replaces it; the strip follows `tabs`, Ctrl+T, Ctrl+W, +, a click selects through `tab_select`; the "Agent is driving" strip while a fake agent's call is in flight, Stop interrupting its `wait`, the person's click ending its `wait` with `interrupted_by` and its next action refused until it reads `tabs`; a confirm, a prompt typed into, a permission blocked and an alert closed by the agent's answer, the keys back to the page after; the context menu's Copy Link and Copy, F12 DevTools beside its page, its close button closing only it and the address returning to the page's, a download's Output line, the cursor; closing the window lingers (a reopen keeps the engine) and then closes the engine, closing the workspace closes it at once; the engine-missing message with `tools/cef/fetch.sh`, and the `external` setting's |
| `crates/eludite/src/shell/browser_view.rs` | 2 | IME composition reaches the page; tiles: the first frame uploads all, the next only the touched tile, across a boundary four, a skipped unknown frame all, skipped known frames their union |
| `crates/ui/src/menu.rs` | (extended) | View > Other Windows > Web Browser |

Final run, `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` with CEF, Xvfb, `ELUDITE_CHROME`,
`ELUDITE_CHROME_NO_SANDBOX=1` and `ELUDITE_DBG_MONO`: 740 passed, 0 failed, 1 ignored (section 10, item 1).

## 10. Deviations, decisions and open points

1. **The final full run** (load average 1 to 4, the other worktree idle or building): **740 passed, 0 failed, 1
   ignored** (a doc example); no test skipped (CEF, Chrome for Testing, Mono and lldb-dap present).
2. **Files outside the brief's list**, each needed by a listed one: `crates/eludite/src/bench.rs` (the window's page in
   `--bounds-out` for the Xvfb run; the bench's box page and tile counts, which the tiles step's measurement needs),
   `crates/eludite/src/args.rs` (their help), `crates/eludite/src/app.rs` (binding the window's keys),
   `crates/commands` (the commands and settings), `crates/eludite/results/linux-browser-window.json` (the measurements,
   as brief 0031 committed its own).
3. **DevTools is not `ShowDevTools`:** CEF 154 cannot show DevTools windowless (it always makes a Chrome-style window
   and logs that windowless DevTools is not supported). The engine loads Chromium's own front end
   (`devtools://devtools/bundled/inspector.html`) in a windowless tab and bridges it to the page through a websocket
   on 127.0.0.1 (random port, unguessable path, open only while that tab is), sharing the page's DevTools session with
   the commands (message ids moved apart). Because the front end then sees a remote target, it shows its screencast
   panel beside the Elements panel ([screenshot](../../crates/eludite/screenshots/linux-browser-window-devtools.png));
   it can be turned off in DevTools' own toolbar. Open point: hide it by default.
4. **The address bar** is the shell's simple text box (`eludite_ui::text_box`): typing, Backspace, select-all on focus,
   Enter, Escape; no caret movement inside the text or partial selection. Enough for this brief; a real single-line
   editor belongs to `crates/ui`.
5. **The Xvfb run allows the agent's calls by clicking Allow** in the Agents window (class `execute` prompts by default;
   an `agents-policy.json` written into the run's folder workspace was not picked up, since the policy is looked up
   beside a solution). Five prompts,
   allowed; the screenshot of the strip is taken while the agent's `wait` (class read, no prompt) is in flight: the
   fixture form answers 4 s after Submit.
6. **`beforeunload`, file choosers and authentication** have no shell-side (fake engine) test; their engine-side round
   trips are tested against real CEF except `beforeunload`, which Chromium shows only after a user gesture (section 7).
7. **The frame-cost verdict** still needs the owner's GPU run; the tiles step is done because none was reported.
8. **A layout restored with the Web Browser open** opens the window (and starts the engine) at startup, as Visual
   Studio restores its Web Browser; the default layout does not have it.

## 11. What briefs C and E need

**Brief C (launch integration, 0037):**

- Open a url in the window: `eludite.view.show {"id": "web_browser"}` then `eludite.browser.tab_open {"url": ...}` (or
  `navigate` on the active tab); both are commands, so F5 can run them through the bus. The proposal's
  `browser.useBuiltIn` is this brief's `browser.engine` (`embedded` when the engine is present); use it rather than a
  second setting. `open_external` is unchanged.
- The embedded engine's tab exposes CDP through the engine (`tab/cdp`), not a DevTools port: a JavaScript debugger
  (brief D) attaching to a tab needs a websocket endpoint. The DevTools bridge of item 3 above is one per DevTools tab;
  generalizing it (an endpoint per tab on request, same random port and path rules) is the smallest step.
- `wait_until` tied to a server's readiness: `navigate`'s `wait_until: "none"` returns at once and `wait` can poll a
  selector or a function; a "server is up" wait (retry the url until it answers) is not here.
- Closing the workspace closes the engine at once; a debugging session's end should not (the window keeps its tabs).

**Brief E (packaging, 0039):**

- Ship `eludite-chromium` beside `eludite` with CEF's runtime files beside it as cargo lays them out (`libcef.so`, the
  paks, `locales/`, `icudtl.dat`, `v8_context_snapshot.bin`, SwiftShader), about 530 MB on Linux; the shell finds the
  engine beside itself and CEF beside the engine (`ChromiumSearch`).
- The sandbox: `chrome-sandbox` setuid root, or the user-namespace rule of brief 0031's report (section 10, item 10);
  as root nothing but `ELUDITE_CHROME_NO_SANDBOX=1` adds `--no-sandbox`.
- The engine now also needs `tungstenite` (already in the build) and writes managed policies and preferences into the
  profile (`<workspace>/.eludite/browser/profile`); an installer must not replace them.
- Windows (CEF's `bootstrap.exe` hosting the engine as a DLL, named file mappings for the ring) and macOS (the nested
  app bundle with five helpers, `shm_open`) remain as brief 0031 specified; nothing of this brief is Linux-specific
  in the shell, so the window works there once the engine does.
- The privacy switches depend on Chromium 154's feature names; a CEF update must rerun the net-log test, which names
  any new destination.
