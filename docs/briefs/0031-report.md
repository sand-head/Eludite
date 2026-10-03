# Brief 0031 report: Spike: CEF offscreen rendering into GPUI, out of process

Status: done on Linux (Xvfb, software rendering). Windows and macOS: not run (no machines; the protocol specifies
them, the fetch scripts are written for them, and the engine builds there only as a stub). CI: not run (nothing
pushed). Branch: `brief/0031-cef-offscreen-spike`, based on `main` at `c548731`. Date: 2026-10-03.
Brief: [0031-cef-offscreen-spike.md](0031-cef-offscreen-spike.md).

## 1. Summary

- **It works end to end.** `eludite-chromium` (`browsers/chromium`, GPL), CEF's browser process on the `cef` crate
  154.3.0+154.0.32 (CEF 154.0.32, Chromium 154.0.8037.58), renders a windowless tab with software `OnPaint` into a
  double-buffered shared-memory ring (a `memfd` whose descriptor reaches the shell once per region over an
  `SCM_RIGHTS` socket), takes JSON-RPC orders on stdio (`protocol/schemas/browser-rpc/`), and the shell draws the
  frames with GPUI's `img` element from a `RenderImage` in a hidden document tab, Web Browser
  ([screenshot](../../crates/eludite/screenshots/linux-browser-spike.png): the animation page at 60 fps, 1600 by 1000,
  inside the shell). `EmbeddedChromium` is the second `Engine` of brief 0023: `tab_open`, `navigate`, `read_page`,
  `evaluate`, `tab_close` and `screenshot` (from the ring) run unchanged against it, and a killed engine is relaunched
  by the next command. Nothing of CEF is mapped in the shell (`shell_maps_libcef: false`).
- **This container needed `ELUDITE_CHROME_NO_SANDBOX=1`** for every run of the engine: it runs as root, and
  Chromium refuses to run as root with its sandbox (checked: with `chrome-sandbox` made setuid root, Chromium exits
  with "Running as root without --no-sandbox is not supported"). Only that variable adds `--no-sandbox`.
- **Measurements** (Xvfb on DISPLAY :98, Mesa lavapipe, no GPU, debug builds with dependencies at opt-level 2, 4
  cores shared with another agent; the reported run's load average was 3.9 to 4.6). Results file:
  [`crates/eludite/results/linux-browser-spike.json`](../../crates/eludite/results/linux-browser-spike.json).
  **Every frame-cost number below is software rasterization of a 2200 by 1500 window and an upper bound, not the
  reference machine's.**

  | Measurement | Result (20 s, 1600x1000 animation page) | Budget |
  |---|---|---|
  | Engine paint rate | **60.0 fps** (1,206 frames announced in 20.1 s) | 60 |
  | Bytes per frame | 6,400,000 (BGRA); dirty rectangle: the whole view every frame (the worst case) | |
  | Engine copy into the slot | p50 **0.84 ms**, p95 1.05, p99 1.46 | |
  | Shell upload: `RenderImage` creation (copy out of shared memory) | p50 **1.15 ms**, p95 1.69, p99 3.06 | |
  | Shell upload: `img` paint with a new image (GPUI's atlas insert, the CPU side of its texture upload) | p50 **1.30 ms**, p95 1.66, p99 2.24 | |
  | Shell frame (DockHost render to the end of its present), 216 frames | p50 **81.4 ms**, p95 103.0, **p99 108.8 ms** | < 8 ms p99 |
  | The same window, same image, nothing uploaded (baseline), 117 frames | p50 73.5 ms, p95 104.4, p99 110.0 | |
  | Frame split, with upload: render to image painted / image painted to present | p50 24.8 / 55.7 ms | |
  | Frame split, baseline | p50 21.5 / 51.9 ms | |
  | Frames the shell drew (the rest were overwritten in the ring) | 10.7 per second | |
  | Paint-to-present latency (engine `OnPaint` to the end of the present showing it) | p50 90.5 ms, p95 113.6, p99 121.5 | |
  | Engine cold start to first frame (spawn, `initialize`, `tab/create`, first `tab/frame`) | **267 ms** (`initialize` answered at 183 ms) | < 2 s |
  | `tab/create` to first frame, engine running | **60 ms** (the shell then needs one frame) | < 300 ms |
  | Shell resident memory: before the engine / with the tab playing | 181.6 / 265.5 MB: **+83.9 MB** (section 4.3) | + 60 MB |
  | Engine processes' memory (8 processes) | PSS **371.5 MB** (RSS sum 942.7, browser process RSS 224.6) | |
  | Cold start of Eludite | unchanged: no CEF in the shell's address space, the engine starts on first use | unchanged |
  | `screenshot` from the ring at 1280 wide (20 calls, test build) | p50 85.7 ms, p95 97.5 ms | < 150 ms p95 |
  | `fetch.sh` (Linux) | 326,397,696 bytes in 4 s, verified and unpacked (with `strip`) in 66 s total; 558 MB cached | < 1.5 GB |
  | The engine crate's build | 551 MB of runtime files copied into `target/debug` plus about 785 MB of artifacts | < 2 GB |

- **The verdict: by the brief's rule, no GO here; the fallback to try is partial uploads from dirty rectangles, then
  the accelerated path** (section 5). The rule reads the Xvfb number (108.8 ms p99), which no design can bring under
  8 ms on this machine: the baseline without any upload is 110.0 ms p99. What the browser itself adds is small and
  measured: 2.5 ms p50 (upload 1.15 + paint 1.30) of CPU in the frame, plus the texture copy inside the present. My
  estimate for the owner's machine is 5.5 to 7 ms p50 and about 8 to 10 ms p99 for this worst-case page (full-frame
  changes at 60 fps), so the software path is close to the line, not clearly over it. The owner's run decides
  (section 8 says what to run); the partial-upload fallback is cheap (2 to 3 agent-days, no GPUI patch).
- **Tests:** `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` (with CEF fetched, under Xvfb,
  with `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1` and `ELUDITE_DBG_MONO`), the final run at load average 6 to 16:
  **620 passed, 1 failed, 1 ignored** (a doc example); the failure is the timing assertion of
  `shell::debug::tests::the_summary_fits_in_8_kb_and_polling_costs_the_ui_little` (agent polling p99 under 8 ms),
  which passes alone; the run before the last code commit, at load 4, passed all 620 then present. The engine tests
  ran, none skipped. 18 tests are new (section 9). fmt clean; clippy `-D warnings`
  clean with and without the feature. `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors; `dotnet test`: 171
  tests, 164 passed, 7 skipped, 0 failed; no .NET file changed.
- **New dependencies:** the `cef` crate (`Apache-2.0 OR MIT`) and `cef-dll-sys` (`Apache-2.0 OR MIT`) are linked into
  `eludite-chromium` on Linux; 40 more crates come in as `cef-dll-sys`'s build-time dependencies (its build script can
  download CEF) or for macOS, and four of those are not MIT, Apache-2.0 or BSD: `ring` (`Apache-2.0 AND ISC`),
  `rustls-webpki` and `untrusted` (`ISC`), `webpki-roots` (`CDLA-Permissive-2.0`), and `libloading` (`ISC`, macOS
  only). Section 7. CEF itself (BSD-3-Clause, Chromium's licenses in `CREDITS.html`) is fetched, never vendored.

## 2. What was built

Commits, in order:

1. `8d59ca3` `protocol/schemas/browser-rpc/` alone, before any code: thirteen method schemas (`initialize`,
   `shutdown`, `tab/create`, `tab/close`, `tab/closed`, `tab/resize`, `tab/resized`, `tab/navigate`, `tab/input` with
   mouse, wheel, key, char, IME and focus events, `tab/cdp`, `tab/cdpEvent`, `tab/frame`, `tab/state`) and
   `browser-rpc.md`: transport, lifecycle, the frame ring's byte layout and rules, resize, Linux `memfd` with
   `SCM_RIGHTS`, macOS `shm_open` and Windows named file mappings, the timestamps.
2. `b675336` `tools/cef/`: `PIN` (version, the four file names, sizes, SHA-256 computed from the downloads on
   2026-10-03, the index's SHA-1), `fetch.sh`, `fetch.ps1`, `README.md`.
3. `0986da0` `browsers/chromium` (`eludite-chromium`): the binary (CEF's browser process and, by `--type=`, its
   subprocesses), `shm` (the ring writer and a reader, descriptor passing), `rpc` (framing, a writer thread), `sandbox`
   (the brief's rule), `engine` (feature `cef`: app, handlers, tabs, input, CDP, shutdown), `README.md`, unit tests
   and `tests/engine.rs`; the workspace member and the `cef` dependency.
4. `3d1125d` `crates/browser/src/embedded.rs`: `EmbeddedChromium: Engine`, `ChromiumSearch` (discovery),
   `TabFrames: FrameSource`, the ring reader, `TabControl`, ring screenshots, `EmbeddedStats`; unit tests and
   `crates/browser/tests/embedded.rs`.
5. `d50b121` `crates/eludite`: `shell/browser_view.rs` (`BrowserSurface`, the spike's document tab, the
   `browser-spike` thread), `--spike-browser URL` and `--bench-browser SECS` (`args.rs`, `app.rs`, `bench.rs`), the
   document area's browser views (`shell.rs`), `tools/browser-spike-linux.sh`.
6. `30843b9` The brief's Status line: in progress.
7. `1b17066` `.github/workflows/ci.yml`: the Linux job caches `~/.cache/eludite/cef` on `tools/cef/PIN`, runs
   `fetch.sh`, builds, lints and tests with `--features eludite-chromium/cef`, the tests under `xvfb-run`; Windows and
   macOS build the crate without the feature and its tests skip.
8. `b1b622f` The engine's sandbox refusal reads as one sentence after its name.
9. `e524e8b` `CLAUDE.md`'s crate map and build notes, `README.md`'s layout and the optional CEF build.
10. `5ff946e` The bench splits each frame at the end of the image's paint.
11. `373cd55` The results file and the screenshot.
12. `fa67c78` More of Chrome's background services off in the engine (section 6, item 9), and a test of its switches.
13. This report, the brief's Status line and the briefs index.

## 3. The design

### 3.1 The engine process (`browsers/chromium`)

- One executable: `main` calls `CefExecuteProcess` first when `--type=` is on the command line (renderer, GPU,
  utility, zygote) and returns its code. The browser process parses its own switches (`--profile`, `--cef-dir`,
  `--frame-socket`), applies the sandbox rule, then duplicates descriptor 1 for the protocol and points descriptor 1 at
  stderr, so CEF, Chromium and every subprocess can only log to stderr (invariant 10's rule, held structurally).
- `CefSettings`: `windowless_rendering_enabled`, `browser_subprocess_path` = itself, `root_cache_path` and
  `cache_path` = the profile (`<workspace>/.eludite/browser/profile`, brief 0023's rule; the shell passes it),
  `resources_dir_path` and `locales_dir_path` = CEF's folder, `no_sandbox` only by the variable, log severity
  warning to stderr. Browser-process switches: software compositing (`--disable-gpu --disable-gpu-compositing`;
  `ELUDITE_CHROMIUM_GPU=1` keeps the GPU process), the background services off (section 6, item 9), and the headless
  Ozone platform when neither `DISPLAY` nor `WAYLAND_DISPLAY` is set (section 6, item 3).
- Threads: CEF's UI thread is the main thread (`CefRunMessageLoop`). A reader thread parses stdin and posts each
  message to the UI thread as a `CefTask`; all handlers and requests run there, so the tabs live in a thread-local
  map. A writer thread owns the protocol output; `OnPaint` and the handlers only queue, never block on the pipe.
  A closed stdin is `shutdown`; `shutdown` closes the browsers, answers when the last `OnBeforeClose` came (5 s at
  most), quits the loop and flushes.
- A tab: `CefBrowserHost::CreateBrowserSync` with `WindowInfo { windowless_rendering_enabled, runtime_style: Alloy,
  shared_texture_enabled: 0, external_begin_frame_enabled: 0 }` and `windowless_frame_rate` 60, opaque white
  background; a client with the render handler (`GetViewRect`, `GetScreenInfo` with the scale, `OnPaint`), the life
  span handler (popups load in the tab; `OnBeforeClose` sends `tab/closed`), the display and load handlers
  (`tab/state`) and the request handler (a renderer crash sends `tab/closed` `crashed` and closes the browser). After
  creation: `WasHidden(false)`, `SetFocus(true)`, `AddDevToolsMessageObserver`.
- CDP: `tab/cdp` goes to `SendDevToolsMessage` unchanged; `OnDevToolsMessage` returns handled and the engine wraps the
  agent's bytes into `tab/cdpEvent` without parsing them. A message for an unknown tab is answered with a CDP error
  (-32000) in a `tab/cdpEvent`, so the shell's caller never waits out a timeout.
- Input: `tab/input` maps to `SendMouseMoveEvent`, `SendMouseClickEvent`, `SendMouseWheelEvent`, `SendKeyEvent`
  (`RAWKEYDOWN`, `KEYUP`, `CHAR`), the IME calls and `SetFocus`.

### 3.2 The frame ring and its transport

The layout is `browser-rpc.md`'s: a 4096-byte header (magic, version, slot count 2, header size, slot size, view
size, region id) with a 320-byte header per slot (an atomic state: free, writing, ready, reading; frame size and
stride; sequence; `OnPaint` timestamp on `CLOCK_MONOTONIC`; copy time; up to 16 dirty rectangles), then two slots of
`round_up(w * h * 4, 4096)` bytes.

- The engine claims a free slot (the older one) with a compare-and-swap, else the older ready slot (an unread frame
  the shell will never see), copies CEF's whole buffer, writes the slot header, stores ready (release), and sends
  `tab/frame`. If neither swap succeeds (the shell holds one slot, the other is being written) the frame is dropped
  and counted. With the shell drawing 10.7 frames a second here, 82 percent of the engine's frames were overwritten
  in the ring, as designed; nothing waits for anyone.
- The shell consumes the newest ready slot (ready to reading, copy, free) and peeks (screenshots) at the newest slot
  in any state, restoring it. A free slot keeps its pixels until the engine reuses it, so a peek works whether or not
  a view consumes the frames.
- Resize: a frame that does not fit the slot size makes a new region (slots never shrink). The region's descriptor
  goes first, over the socket; `tab/resized` follows on stdout; the shell's reader receives the descriptor when that
  message arrives (one `recvmsg`, the payload is the region id, checked) and maps it, the header page read-write and
  the slots read-only.
- **`SCM_RIGHTS`, not a path under the profile** (the brief asked which): a `SOCK_SEQPACKET` socket pair the shell
  creates, the child's end placed at descriptor 3 by `dup2` before `exec`. It costs one `sendmsg` per region (at tab
  creation and when the view grows), never per frame, so the `mkstemp` fallback was not needed.

### 3.3 The shell (`crates/browser/src/embedded.rs`, `crates/eludite/src/shell/browser_view.rs`)

- `EmbeddedChromium` launches the engine (found by `ChromiumSearch`: `ELUDITE_CHROMIUM`, beside the running
  executable, `target/<profile>/` from a test binary in `deps/`, `CARGO_TARGET_DIR`; CEF in `ELUDITE_CEF`, `CEF_PATH`,
  the fetch cache, then beside the engine, which is where cargo's build copies it), with CEF's folder on
  `LD_LIBRARY_PATH`, and answers the `Engine` trait: a tab is its own CDP session; `send` and `send_many` correlate
  CDP ids as `Connection` does on a websocket; subscriptions end with the tab or the engine. A closed stdout fails
  everything pending, logs "The embedded browser exited unexpectedly (...); the next browser command starts it
  again", and the next `launch` starts a new engine (tested by `kill -9`).
- `TabFrames` is the tab's `FrameSource` (sequence, size, stride, pixels, dirty rectangles, paint and copy times)
  with a listener the reader thread calls on each `tab/frame`. `TabControl` forwards input and resizes from any
  thread as notifications.
- `screenshot` reads the ring when the clip lies in the viewport: it first awaits two animation frames in the page
  (so the page's last change is painted) and up to 17 ms for the frame that follows, reads the layout metrics,
  crops, scales, converts BGRA to RGBA and encodes (PNG with fast compression, or JPEG). A full page, or a clip
  outside the viewport, goes to `Page.captureScreenshot`.
- `BrowserSurface` (a GPUI view): the listener feeds a channel; a task on the UI thread drains it and notifies (so
  frames never wait on the UI thread, and the UI thread never waits on the engine). `render` uploads only when the
  sequence changed: one copy out of the slot into a new `RenderImage`, the previous image's atlas tile freed with
  `Window::drop_image`, the dirty rectangles kept. It draws `img(ImageSource::Render(..))` at the frame's size,
  wrapped in an element that times the image's paint; a canvas behind it records the surface's bounds for input
  (window coordinates to the tab's CSS pixels) and, when the tab fits its view, sends `tab/resize`. Mouse buttons,
  moves, wheel and keys (a key table to Windows virtual-key codes, `char` events from `key_char`) are forwarded;
  IME is specified, not forwarded (brief B).
- `--spike-browser URL` opens the hidden Web Browser document tab; a thread named `browser-spike` owns the engine
  (launch, tab, a second tab on request, shutdown), the UI thread only receives the frames and an input handle.

## 4. The measurements

### 4.1 All runs

Each run is `crates/eludite/tools/browser-spike-linux.sh OUT 20` (the first, 10). The machine was shared with
another agent's builds and Mono test processes; the load average (1 minute, at the end of each run) explains the
spread.

| Run | Load | Frame p50 / p99 | Baseline p50 / p99 | Upload p50 / p99 | `img` paint p50 / p99 | Engine fps | Drawn fps | Cold start | `tab/create` to frame |
|---|---|---|---|---|---|---|---|---|---|
| 10 s, first with the baseline | 6.2 | 145.8 / 365.6 | 135.3 / 188.4 | 1.22 / 8.58 | 1.35 / 5.56 | 56.4 | 6.0 | 328 ms | 94 ms |
| 20 s, series 1 | 10.8 | 413.5 / 667.6 | 254.5 / 621.5 | 1.22 / 17.3 | 1.78 / 49.3 | 39.3 | 2.3 | 373 ms | 63 ms |
| 20 s, series 2 | 12.2 | 324.6 / 587.6 | 366.5 / 613.0 | 1.17 / 9.15 | 1.35 / 29.8 | 38.3 | 2.8 | 475 ms | 67 ms |
| 20 s, series 3 | 12.8 | 279.6 / 578.2 | 248.3 / 307.7 | 1.21 / 8.89 | 1.34 / 30.5 | 44.0 | 3.0 | 625 ms | 116 ms |
| 20 s, series 4 | 10.9 | 352.9 / 564.0 | 323.8 / 471.9 | 1.21 / 22.0 | 1.39 / 29.2 | 42.2 | 2.7 | 683 ms | 69 ms |
| **20 s, reported** | **4.6** | **81.4 / 108.8** | **73.5 / 110.0** | **1.15 / 3.06** | **1.30 / 2.24** | **60.0** | **10.7** | **267 ms** | **60 ms** |

The upload and paint medians hold at 1.15 to 1.22 and 1.30 to 1.39 ms whatever the load; their tails and the frame
times follow the load. The engine's own paint rate drops under load too (the animation page runs in its renderer).

### 4.2 Where a frame's time goes

- The frame with an upload and the baseline frame differ by 7.9 ms at p50 (81.4 against 73.5): 3.3 ms before the
  image's paint ends (24.8 against 21.5: the upload and the paint, 2.45 ms by their own timers, and noise) and 3.8 ms
  after it (55.7 against 51.9: GPUI's `queue.write_texture` copy of the pending upload, a new 1600x1024 atlas texture
  per frame, and lavapipe sampling it).
- Everything else is the window: 21.5 ms of CPU to lay out and paint the IDE at this load (on the owner's machine,
  brief 0017's `--bench-output` measured the whole frame at 2.6 ms p50 and 3.7 ms p99), and 52 ms for lavapipe to
  rasterize 3.3 megapixels. A GPU pays neither.
- GPUI's image path costs four copies of each frame on the CPU: ours out of shared memory into the `RenderImage`, the
  atlas's `to_vec` into a pending upload (`swizzle_upload_data`, a plain copy for BGRA), wgpu's staging copy in
  `write_texture`, then the GPU's copy into the texture. And because a 1600 by 1000 tile needs a whole atlas texture
  (`max(size, 1024)` square-ish), every frame frees one texture and creates another.
- The engine side is cheap: 0.84 ms p50 to copy 6.4 MB into the slot, and `tab/frame` is a 300-byte message.

### 4.3 Memory

- The shell grew 83.9 MB with the tab playing, over the brief's 60 MB. Its memory map in the middle of a run: the
  ring's `memfd` (12.5 MB, both slots touched), Mesa's device memory for textures and staging as `memfd`
  allocations (a 32 MB block beside the swapchain's three 12.9 MB images, which were there before the engine), and
  37.8 MB of heap (each frame allocates a 6.4 MB `RenderImage` and GPUI a 6.4 MB pending copy; glibc's dynamic mmap
  threshold keeps such blocks on the heap after the first is freed). On a GPU the textures and much of the staging
  live in device memory, so I expect about +40 to 50 MB there; this needs the owner's run to confirm.
- The engine's eight processes: 371.5 MB PSS (942.7 MB RSS summed, which counts libcef's shared pages in each).

### 4.4 Not measured here

- The GPU-side texture upload and the frame cost on real hardware (no GPU). `--bench-browser` runs unchanged there.
- GPUI's `write_texture` and texture creation, separately: they happen inside the present, behind GPUI's API; the
  frame split brackets them.
- The window-open-to-first-pixel budget as the person sees it: measured to the first `tab/frame` (60 ms); the shell
  draws it within its next frame.

## 5. The verdict

**By the brief's rule: not GO, not PROBABLY GO. The fallback to try is partial uploads from dirty rectangles, then
the accelerated path.** The rule asks for the shell's frame time under Xvfb software rendering under 8 ms p99 for GO
(16 ms for PROBABLY GO); it is 108.8 ms p99, and 110.0 ms p99 for the same window with nothing uploaded, so the rule
cannot be met by any browser design on this machine and the decision is the owner's hardware run.

What the numbers support beyond the rule:

- The browser's own cost in a frame is about 2.5 ms of CPU at p50 (1.15 upload + 1.30 paint), 5.3 ms at p99, plus
  the staging copy and texture churn inside the present.
- Estimate for the owner's machine, worst-case page (every pixel changes at 60 fps): the IDE frame 2.6 ms p50 and
  3.7 ms p99 (brief 0017), plus 2.5 ms (p50) to 5.3 ms (p99) here, plus 0.5 to 1.5 ms for the staging copy and the
  texture creation: **5.5 to 7 ms p50, about 8 to 10 ms p99**. Close to the 8 ms line, not clearly over it; most real
  pages change a small part of the view a few times a second and cost far less.

The fallbacks, in order, with sizes:

1. **Partial uploads from dirty rectangles, without a GPUI patch** (2 to 3 agent-days): draw the tab as a grid of
   tiles (for example 256 by 256 device pixels, 28 tiles at 1600x1000), each its own `RenderImage`, and rebuild only
   the tiles a frame's dirty rectangles touch (they are in the ring and in `SurfaceStats.last_dirty` already). Tiles
   share 1024-square atlas textures, so no texture is created per frame, and a blinking caret or a hover uploads one
   tile instead of 6.4 MB. It does not help a full-frame animation or scrolling, which repaint everything; tile edges
   must stay on whole device pixels. Brief B can do it with `--bench-browser` growing a typical-page mode.
2. **CEF's accelerated offscreen path with a GPUI external-texture element** (1.5 to 2 agent-weeks, a GPUI patch with
   an ADR note): `shared_texture_enabled` and `OnAcceleratedPaint` hand a DMA-BUF (Linux), D3D11 shared handle
   (Windows) or IOSurface (macOS) from CEF's GPU process; the engine passes the handle to the shell (on Linux over the
   same `SCM_RIGHTS` socket, per frame or from a small pool, with a fence), and a patched `gpui_wgpu` imports it as a
   texture (Vulkan external memory, as the `cef` crate's `osr_texture_import` does with `ash`) for a new element.
   macOS already has GPUI's `surface` element for IOSurfaces at the pinned revision. Zero copies; needs a GPU on every
   test runner.
3. Cheaper changes to the software path regardless: reuse the frame buffers instead of allocating 6.4 MB twice per
   frame (needs GPUI to accept an updated image, so it is part of either patch), and draw at most at the display's
   refresh (it already skips frames the ring overwrote).

## 6. CEF findings

1. **The `cef` crate's build script fetches CEF by itself** when `CEF_PATH` is unset (into `OUT_DIR`, checked only
   against the index's SHA-1), and always copies CEF's runtime files into `target/<profile>/`. So the dependency is
   behind `eludite-chromium`'s `cef` feature, off by default: a plain `cargo build --workspace` downloads nothing, and
   the engine is then a stub that says how to build it (deviation 1).
2. **The Linux minimal distribution's `libcef.so` carries 1 GB of DWARF debug information** (1.45 GB file);
   unpacked as is, the cache is 1.5 GB and the target copy another 1.45 GB. `fetch.sh` strips the debug sections
   (465 MB, symbols kept); `CEF_KEEP_DEBUG=1` keeps them.
3. **CEF on Linux needs a display even windowless**: Chromium's default Ozone platform (X11 here) exits with "Missing
   X server or $DISPLAY". `--ozone-platform=headless` renders windowless tabs with no display at all (verified: the
   same frames, CDP and input); the engine uses it when neither `DISPLAY` nor `WAYLAND_DISPLAY` is set. The headless
   platform has no system clipboard, so a desktop session keeps the real platform. The brief's tests ran under Xvfb
   as asked; CI could drop Xvfb for them.
4. **Descriptor 1 is shared with every subprocess** CEF starts; the engine points it at stderr and keeps a private
   copy for the protocol (section 3.1). The frame socket is marked close-on-exec once inherited, so CEF's children do
   not get it.
5. **`OnPaint` always delivers the whole view**, with the dirty rectangles beside it; the first frame of a tab is the
   background color, before the page paints (the tests wait for the expected pixels, not the first frame).
6. **`CloseBrowser(true)` may run `OnBeforeClose` before the request that asked for it is answered**, so
   `tab/closed` can precede `tab/close`'s answer; the protocol allows it and the shell does not depend on the order.
7. **The DevTools agent is the page target**: no `Target.attachToTarget`, no `sessionId`; every CDP command brief
   0023's `Browser` sends (Page, lifecycle events, Runtime, Log, Network with buffers, DOM, Accessibility, layout
   metrics, `captureScreenshot`) worked unchanged. Closing a tab sends `Inspector.detached` ("Render process gone.").
8. **Root needs `--no-sandbox`** whatever the helper (section 1); the GPU process logs "InitializeSandbox() called
   with multiple threads" under `--no-sandbox`, harmlessly.
9. **CEF 154 keeps Chrome's background services and calls Google at startup**, `--disable-background-networking` and
   `--disable-component-update` notwithstanding. A net log of a fresh profile showed `clients2.google.com/time`
   (network time), `accounts.google.com/ListAccounts` (the account cookie check), `update.googleapis.com` (the
   component updater), `www.google.com/` (a preconnect) and `www.google.com/async/folae`. The engine now also passes
   `--no-pings --no-service-autorun --disable-breakpad --disable-client-side-phishing-detection
   --disable-domain-reliability --disable-field-trial-config --disable-search-engine-choice-screen
   --metrics-recording-only --disable-features=NetworkTimeServiceQuerying,OptimizationHints,MediaRouter,
   DialMediaRouteProvider,Translate,CertificateTransparencyComponentUpdater,LensOverlay,AutofillServerCommunication`;
   network time stopped, the other four did not. This conflicts with "no network calls at startup" once the engine is
   started, and with the spirit of "no telemetry"; brief B must silence them (Chrome policies through CEF's
   `chrome_policy_id` or a managed policy file, or the feature names behind each) and keep a net-log test (open
   point 3).
10. **Discovery of `libcef.so`**: cargo's copy sits beside the engine; the binary gets `-Wl,-rpath,$ORIGIN` (from its
    build script, with the feature on Linux), and the shell also puts CEF's folder on `LD_LIBRARY_PATH`, so an
    engine found anywhere loads the CEF the shell found.

## 7. Dependencies

Linked into `eludite-chromium` on Linux (and only into it; the shell links neither):

| Crate | Version | SPDX | Why |
|---|---|---|---|
| `cef` | 154.3.0+154.0.32 | `Apache-2.0 OR MIT` | CEF's C API and the handler wrappers (`default-features = false`) |
| `cef-dll-sys` | 154.3.0+154.0.32 | `Apache-2.0 OR MIT` | The generated bindings; links `libcef` |

`cef` on macOS (not built here): `libloading` 0.9.0 (`ISC`), `objc2` 0.6.4 (`MIT`, already in the build). Build-time
only, as `cef-dll-sys`'s build dependencies (its script can download and unpack CEF, and builds the C++ wrapper with
CMake on macOS): `download-cef` 3.0.0 (`Apache-2.0 OR MIT`), `cmake` 0.1.58 (`MIT OR Apache-2.0`), `ureq` 3.4.2 and
`ureq-proto` 0.6.4 (`MIT OR Apache-2.0`), `rustls` 0.23.45 (`Apache-2.0 OR ISC OR MIT`), `rustls-pki-types` 1.15.1
(`MIT OR Apache-2.0`), `rustls-webpki` 0.103.15 (`ISC`), `ring` 0.17.14 (`Apache-2.0 AND ISC`), `untrusted` 0.9.0
(`ISC`), `webpki-roots` 1.0.9 (`CDLA-Permissive-2.0`), `base64` 0.23.1, `cookie` 0.18.2, `cookie_store` 0.22.1,
`socks` 0.3.4, `utf8-zero` 0.8.1, `tar` 0.4.46, `xattr` 1.6.1, `filetime` 0.2.29, `fs-err` 3.3.1, `clap` 4.6.7,
`clap_builder` 4.6.7, `clap_derive` 4.6.7, `clap_lex` 1.1.1, `anstream` 1.0.0, `anstyle` 1.0.14, `anstyle-parse`
1.0.0, `anstyle-query` 1.1.5, `anstyle-wincon` 3.0.11, `colorchoice` 1.0.5, `is_terminal_polyfill` 1.70.2,
`once_cell_polyfill` 1.70.2, `utf8parse` 0.2.2, `encode_unicode` 1.0.0, `time-macros` 0.2.32 and `windows-sys`
0.52.0 (all `MIT OR Apache-2.0`), `console` 0.16.6, `indicatif` 0.18.6, `unit-prefix` 0.5.2 and `strsim` 0.11.1
(`MIT`). Those 42 are the new `Cargo.lock` entries. Already in the build and now direct: `libc` 0.2.189 (`MIT OR
Apache-2.0`) in `eludite-chromium` and `eludite-browser`, `image` 0.25.10 (`MIT OR Apache-2.0`) in
`eludite-browser`.

Outside "MIT, Apache-2.0 or BSD": `ring` (Apache-2.0 AND ISC), `rustls-webpki` and `untrusted` (ISC),
`webpki-roots` (CDLA-Permissive-2.0, Mozilla's root certificates as data), all build-time only and never in a
binary, and `libloading` (ISC, linked on macOS). ISC is MIT-equivalent and GPL-compatible; CDLA-Permissive-2.0 is a
permissive data license. They cannot be avoided without replacing `cef-dll-sys`'s build script (a `[patch]` of
`download-cef`); the owner decides (open point 1).

## 8. How to reproduce, here and on a machine with a GPU

```
export CEF_PATH="$(tools/cef/fetch.sh)"                 # once: 326 MB, verified, unpacked to ~/.cache/eludite/cef
cargo build -p eludite -p eludite-chromium --features eludite-chromium/cef
# As root (a container) the engine needs ELUDITE_CHROME_NO_SANDBOX=1; as a user, chrome-sandbox setuid root or the variable.
crates/eludite/tools/browser-spike-linux.sh /tmp/spike 20     # Xvfb :98, lavapipe; writes browser-spike.json and .png
cargo test -p eludite-chromium --features cef                 # the engine against the real CEF (Xvfb or no display)
cargo test -p eludite-browser --test embedded -- --nocapture  # brief 0023's commands against the embedded engine
cargo test -p eludite browser_view                            # BrowserSurface, headless GPUI
eludite --spike-browser https://example.com                    # the hidden Web Browser tab, by hand
```

On the owner's machine (the run that decides):

```
export CEF_PATH="$(tools/cef/fetch.sh)"
cargo build --release -p eludite -p eludite-chromium --features eludite-chromium/cef
NATIVE=1 ELUDITE_BIN=target/release/eludite crates/eludite/tools/browser-spike-linux.sh /tmp/spike 20
```

Read `frame_cost.p99_ms` (the budget: under 8 ms), `baseline_no_upload.frame_cost` (the same window without uploads:
the difference is the browser's cost), `upload` and `img_paint`, `paint_to_present`, `shell_rss_before_engine` and
`shell_rss_with_tab` (+60 MB at most) in `/tmp/spike/browser-spike.json`. The window opens at 2200 by 1500 so the
1600 by 1000 tab shows whole; on a smaller display the tab is clipped, which leaves the upload the same. If the
frame p99 is under 8 ms, the software path is GO; if not, start with the partial uploads of section 5.

## 9. Tests

| Where | Tests | What |
|---|---|---|
| `browsers/chromium/src/shm.rs` | 3 | A region written and read through a descriptor passed with `SCM_RIGHTS`; the engine overwrites the older unread frame and never a slot being read; more than 16 dirty rectangles merged |
| `browsers/chromium/src/rpc.rs`, `lib.rs`, `sandbox.rs`, `engine.rs` | 2, 1, 1, 1 | Framing round trip and a bad body; `tab/cdpEvent` wraps the agent's bytes; the options among CEF's switches; the variable is the only way to `--no-sandbox` and a missing or non-setuid helper is refused with the command that installs it; the switches never drop the sandbox and follow the GPU and display settings |
| `browsers/chromium/tests/engine.rs` | 2 | The real engine: `initialize`, a region before `tab/create`'s answer, the four quadrant colors read back from the ring at known points, the title, a CDP `Runtime.evaluate` round trip and a CDP error for a missing tab, a resize into a new region with frames of the new size, navigate, a mouse click and the key A changing the page, error codes -32001 and -32601, close, shutdown with exit 0; and a closed stdin shutting the engine down |
| `crates/browser/src/embedded.rs` | 5 | Discovery order (engine and CEF); the pinned version equals `tools/cef/PIN`; every method used has a schema in `protocol/schemas/browser-rpc/`; answers and events routed by id and tab, listeners, state, a crashed tab ending its subscription, a closed engine failing pending calls; base64 |
| `crates/browser/tests/embedded.rs` | 1 | Brief 0023's subset against `EmbeddedChromium` in an unchanged `Browser`: `tabs` before launch, `tab_open` launching and loading the form (status 200, title), `navigate` with `load` and `network_idle`, `read_page` with refs, values, states and boxes, `evaluate` (value, promise, exception), `screenshot` from the ring checked pixel by pixel on the quadrant page (1280x800, 640 wide, JPEG) and `Page.captureScreenshot` for a full page, 20 screenshots timed, a second tab opened and closed, the engine killed and relaunched by the next `tab_open` |
| `crates/eludite/src/shell/browser_view.rs` | 2 | The proving test: `BrowserSurface` in a headless GPUI window uploads only when the sequence changes (no read and no upload for repeated draws of the same sequence), times the paint of a new image only, reports the dirty rectangles and copy times, and forwards a click in the tab's coordinates; the key table and modifier flags |
| `crates/eludite/src/args.rs` | (extended) | `--bench-browser` and `--spike-browser` |

Every engine test skips with a message when the engine was built without the `cef` feature, when no engine or CEF is
found, or off Linux (verified: `cargo test -p eludite-chromium` and `--test embedded` without the feature print
`skipped`/`SKIPPED` and pass).

## 10. Deviations, decisions and open points

1. **The `cef` dependency is optional** (`eludite-chromium`'s `cef` feature, off by default), because its build
   script would otherwise download 326 MB in every workspace build, unchecked against the pin (section 6, item 1).
   CI's Linux job and the test commands of this report turn it on with `--features eludite-chromium/cef`; Windows and
   macOS "build the crate" as the stub. Open point 1: four build-time dependencies are ISC or CDLA-Permissive-2.0
   (section 7).
2. **Files outside the brief's list**, each needed by a listed one: `crates/browser/src/lib.rs` (the module) and
   `crates/browser/Cargo.toml` (`libc`, `image`); `crates/eludite/src/shell.rs` (the document area draws browser
   views: one field and four lines), `app.rs` (dispatch and the bench's window size), `args.rs` (the two arguments)
   and `bench.rs` (the harness, as the brief names it). The ring layout is implemented twice, in the engine
   (`shm.rs`) and in the shell (`embedded.rs`), both against `browser-rpc.md`: linking the engine crate into the
   shell would pull `cef` in through feature unification.
3. **Background requests to Google** remain (section 6, item 9). Not in this brief's scope to finish; brief B.
4. **`--bench-browser` writes JSON to stdout**, like the other harnesses; `browser-spike-linux.sh` writes it to
   `OUT/browser-spike.json`, and the committed results file wraps the reported run with the other five runs. The
   screenshot is the window cropped from the 2400x1600 screen and scaled to 60 percent (1320x900).
5. **The bench window is 2200 by 1500** (only with `--bench-browser`) so the 1600 by 1000 tab is whole in the
   document area; the Xvfb screen of the script is 2400 by 1600 for it.
6. **The frame-cost baseline and the frame split** are additions to what the brief listed: without them the Xvfb
   number says nothing about the browser.
7. **Debug builds** for every measurement (dependencies at opt-level 2, so GPUI, wgpu and `memcpy` are optimized);
   the owner's run should use release builds (section 8).
8. **The bench ran before the background-service switches** of commit 12; they do not touch rendering.
9. **Windows and macOS** are specified (named file mappings; `shm_open` over the same socket) and not built: the engine
   is Linux-only behind `cfg`, and `EmbeddedChromium::launch` says so elsewhere. On Windows the sandbox needs CEF's
   `bootstrap.exe`/`bootstrapc.exe` hosting the engine built as a DLL (the `cef` crate's `cefsimple` shows it).
10. **The Linux sandbox rule is the brief's** (setuid helper or refuse). Chromium prefers unprivileged user
    namespaces and needs the helper only where they are restricted; this kernel allows them (`max_user_namespaces`
    64301), so brief E may relax the rule to "namespaces or the helper", which avoids asking users for `sudo`.
11. **Shell memory** is over the brief's +60 MB here (+83.9 MB, section 4.3); expected under it on a GPU, unverified.
12. **Environment:** two full-suite runs under load average 10 failed three load-sensitive tests
    (`shell::debug::tests::exception_types_go_as_filter_options_and_the_window_shows_the_tree`,
    `output_by_cursor_exception_info_and_wait`, and `acting_on_the_page_in_a_headless_chrome`); each passed alone, and
    the next full run (load 4) passed all 620. The final run (load 6 to 16, after commit 12's test) failed only
    `the_summary_fits_in_8_kb_and_polling_costs_the_ui_little`'s 8 ms timing assertion, which passes alone. During the measurements I stopped one orphaned, spinning
    `eludite-dbg-mono` process left from the deleted 0026 worktree (parent init, 90 percent of a core for two hours).

## 11. What brief B needs

- **The engine process is production-shaped**: keep `browsers/chromium` (protocol, ring, handlers, CDP, shutdown,
  sandbox rule, stdout discipline) and add: several tabs per view switching, popups as tabs (now they load in the
  same tab), `<select>` popups (`PET_POPUP` frames, ignored now), cursor changes (`OnCursorChange` to a `tab/cursor`
  notification), the clipboard (works through Chromium on X11 and Wayland; nothing on the headless platform),
  `ShowDevTools` as a second windowless tab, downloads, JavaScript dialogs, permission prompts in the Visual Studio
  style, and the background requests silenced with a net-log test.
- **The shell**: the Web Browser window (address bar, tabs, toolbar, "Agent is driving" strip) on `BrowserSurface`;
  IME through GPUI's input handler to `imeSetComposition`/`imeCommitText` (specified in `tab-input.json`); a full key
  table; focus following the tool window (`focus` events); fitting the tab to the view (implemented, `fit`), with
  the scale factor (implemented in the protocol; untested at scales other than 1).
- **Frame cost**: run section 8 on the owner's machine first. If the p99 is over 8 ms, the partial uploads of section
  5 (2 to 3 agent-days) come before anything else; the accelerated path only if those miss.
- **The commands in the window**: `eludite.browser.*` with `EmbeddedChromium` as the shell's engine factory when the
  window is open (`browser.engine` setting: external Chrome or embedded), screenshots from the ring (implemented),
  and the `input` command's click to the next painted frame (the ring's sequence makes it measurable).
- **Packaging (brief E)**: the engine beside `eludite` with CEF's runtime files beside it (as cargo's build lays them
  out), the macOS nested app bundle with five helper apps (`tools/cef/README.md`), the Windows bootstrap, the sandbox
  helper's installation or the user-namespace rule.
