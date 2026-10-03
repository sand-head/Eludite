# Brief 0031: Spike: CEF offscreen rendering into GPUI, out of process

Status: in progress
Phase: 2 (proposal 0002, brief S; a spike)
Plan reference: PLAN.md sections 1 (the browser engine carve-out), 2 (principles 1, 2, 6), 3 (D2), 4.9 (the Web Browser window), 9, 10 (Phase 2), 12 (`browsers/chromium`, `tools/cef`); proposal 0002 sections 2, 3, 7, 8 (S), 11; ADR-0008
Related ADRs: ADR-0002, ADR-0008
Depends on: brief 0023 (the `Engine` trait and the CDP client in `crates/browser`). Independent of the debugging briefs. Spike rules apply (docs/briefs/README.md): throwaway code is allowed where the report says so, but the engine process skeleton is meant to become brief B's production code, so write it as such.

## Goal

Answer the question ADR-0008 and proposal 0002 rest on: can a Chromium tab rendered offscreen by CEF in its own process be drawn by GPUI inside the shell within the frame budget? Build `eludite-chromium` (`browsers/chromium`, GPL; the name `eludite-browser` is taken by `crates/browser`), CEF's browser process on the `cef` crate (Apache-2.0 OR MIT, tauri-apps/cef-rs, version 154.3.0+154.0.32, which pins CEF 154.0.32+g682c378+chromium-154.0.8037.58; its minimal distributions are on `cef-builds.spotifycdn.com` for linux64 at 326 MB, windows64 173 MB, macosarm64 132 MB, macosx64 139 MB), with one windowless tab whose `OnPaint` BGRA frames and dirty rectangles land in a double-buffered shared-memory ring the shell maps, a JSON-RPC control channel on stdio (create and close a tab, resize, navigate, input events, a `cdp` pass-through), and the shell drawing the frames with GPUI's `img` element in a headless-testable element while the tab plays a 60 fps animation at 1600 by 1000. Measure the shell's frame cost and the end-to-end latency, write the fetch scripts and the findings on the macOS bundle layout and the Linux sandbox, and end with a written GO or the fallback to try (partial atlas updates from dirty rectangles; CEF's accelerated offscreen path with a GPUI patch). The `Engine` trait of brief 0023 gets its second implementation, `EmbeddedChromium`, so `eludite.browser.*` run against the embedded engine unchanged.

What this machine can and cannot measure: there is no GPU; GPUI draws on an Xvfb screen through Mesa's software Vulkan (`crates/eludite/tools/xvfb-linux.sh`), so the frame-cost numbers here are an upper bound and not the reference machine's. The spike's pass or fail on the proposal's 8 ms p99 is decided on the owner's machine with the harness this brief leaves behind; the report says exactly what to run. CEF's own process runs here under Xvfb (CEF on Linux needs a display even windowless) with `--no-sandbox` only through the explicit opt-in below, because this container runs as root.

## Files in scope

- `tools/cef/PIN` (the CEF version, the four file names, sizes and SHA-256 checksums computed when pinning; the index's SHA-1 noted too), `tools/cef/fetch.sh` and `fetch.ps1` (download the platform's minimal distribution, verify, unpack to `~/.cache/eludite/cef/<version>/`, print the path; `CEF_PATH` for the `cef` crate's build script points there), `tools/cef/README.md` (licenses: CEF BSD-3-Clause and Chromium's; what the minimal distribution holds; the sandbox helper and the macOS bundle).
- `browsers/chromium/` (new crate `eludite-chromium`, GPL, workspace member): `main.rs` (the browser process and, through `--type=`, CEF's subprocesses from the same executable), the control protocol server, the offscreen render handler, the shared-memory writer, the CDP pass-through (`CefBrowserHost::SendDevToolsMessage` and `AddDevToolsMessageObserver`), the per-workspace profile (`--user-data-dir` and `cache_path`), `README.md` (how to run it alone).
- `protocol/schemas/browser-rpc/` (new, MIT): the control methods and their params and results (`initialize`, `tab/create`, `tab/close`, `tab/resize`, `tab/navigate`, `tab/input` with mouse, wheel, key and IME events, `tab/cdp` and the `tab/cdpEvent` notification, `tab/frame` notification with the ring slot, sequence number, dirty rectangles and size, `shutdown`), and `browser-rpc.md` describing the shared-memory layout (header with slot states and sequence numbers, two BGRA slots sized for the largest viewport, resize protocol) and the Linux `memfd`, Windows file-mapping and macOS `shm_open` handles passed as file descriptors or names.
- `crates/browser/src/embedded.rs` (new): `EmbeddedChromium: Engine` launching `eludite-chromium`, mapping the ring, exposing frames as `FrameSource` (sequence, size, pixels, dirty rects) and forwarding CDP; discovery of the engine executable (beside `eludite`, `ELUDITE_CHROMIUM`, the cargo target dir in development) and of CEF (`CEF_PATH`, `ELUDITE_CEF`, the cache).
- `crates/eludite/src/shell/browser_view.rs` (new): a `BrowserSurface` GPUI element that draws the latest frame with `img` from a `RenderImage` built from the shared buffer, uploading only when the sequence changed, with the dirty rectangles available to the fallback; a hidden document tab `--spike-browser URL` that shows it (the Web Browser window proper is brief B), and `--bench-browser SECONDS` that opens the animation page and records per frame: upload time, GPUI frame time, and the engine's paint-to-present latency (engine timestamps in the ring header), writing `results/linux-browser-spike.json`.
- `crates/eludite/tools/browser-spike-linux.sh` (runs the bench under Xvfb here and natively on the owner's machine, takes a screenshot) and `browser-spike.py` if a driver is needed; `crates/eludite/results/linux-browser-spike.json` and a screenshot under `crates/eludite/screenshots/`.
- `browsers/chromium/tests/` (the control protocol against the real engine when CEF is present: create a tab, navigate to a data URL, receive frames with the expected pixels at known points, resize, a `cdp` round trip `Runtime.evaluate`; skipped with a message otherwise) and `crates/browser/tests/embedded.rs` (brief 0023's Chrome test subset against `EmbeddedChromium`: `tab_open`, `navigate`, `screenshot` via the ring, `read_page`, `evaluate`; skipped without CEF).
- `.github/workflows/ci.yml`: the Linux Rust job caches and fetches CEF (326 MB; cache keyed on the PIN) and runs the engine tests under `xvfb-run`; Windows and macOS build the crate but skip its tests unless the cache is warm (leave them skipping; the report says what packaging needs).
- `Cargo.toml` (the member and the `cef` dependency; the SPDX ids in the report), `Cargo.lock`, `CLAUDE.md` (crate map rows), `README.md` (layout, the optional CEF fetch), `docs/briefs/README.md`, `docs/briefs/0031-report.md` (new).

## Contract

- **Process model** as proposal 0002 section 3: the shell launches `eludite-chromium` with the profile directory and the control channel on stdio; CEF's renderer, GPU and utility subprocesses are the same executable with `browser_subprocess_path`; `windowless_rendering_enabled` with `OnPaint` (software) delivering BGRA and dirty rectangles; `external_begin_frame` off; the paint frame rate requested at 60. A crash of the engine loses the tabs, not the shell: the shell notices the control channel closing and the next command relaunches it (as brief 0023 does for Chrome).
- **Shared memory.** A header (magic, version, slot count 2, slot size, width, height, per slot: state (free, writing, ready, reading), sequence number, paint timestamp, dirty rectangle count and up to 16 rectangles) followed by the slots. The engine writes a frame into a free slot, marks it ready and sends `tab/frame`; the shell maps the slot read-only, uploads it, marks it free. Resize reallocates: the engine sends `tab/resized` with a new region; the shell remaps. Linux `memfd_create` with the fd passed over the stdio channel (`SCM_RIGHTS` over a Unix socket pair the shell creates and hands to the child as fd 3; or a path under the profile with `mkstemp` if `SCM_RIGHTS` costs too much time in the spike: say which), Windows a named file mapping, macOS `shm_open`; the spike implements Linux and documents the other two.
- **Input.** `tab/input` carries mouse move, down, up, wheel, key down, up, char and IME composition events in the tab's CSS pixel space with modifiers; the spike forwards mouse and keyboard, documents IME.
- **CDP.** `tab/cdp` sends a message to the tab's DevTools and `tab/cdpEvent` delivers events and responses, so `EmbeddedChromium` reuses brief 0023's `Connection` logic with a different transport. `read_page`, `screenshot` (from the ring, not `Page.captureScreenshot`, when the tab is visible; `Page.captureScreenshot` otherwise) and `evaluate` work against it.
- **Profile and sandbox.** The profile is `<workspace>/.eludite/browser/profile` (brief 0023's rule). On Linux the engine runs with CEF's sandbox when `chrome-sandbox` is present with the SUID bit, else it refuses with the message naming the helper; `ELUDITE_CHROME_NO_SANDBOX=1` (brief 0023's variable) adds `--no-sandbox` and the report records that this container needed it. No `--no-sandbox` is ever added silently.
- **Measurements** (all written to the results file and the report): engine paint rate on the animation page; bytes per frame and the ring's copy time; the shell's upload time per frame (`RenderImage` creation and GPUI's texture upload) and the whole frame time, p50, p95, p99 over at least 600 frames; paint-to-present latency; shell resident memory with the tab open; engine process memory; cold engine start to first frame; `tab_open` to first pixel with the engine running. Each is measured under Xvfb here and the report says so; the harness runs unchanged on a machine with a GPU.
- **The verdict.** The report ends with: GO if the shell's frame time under Xvfb software rendering is under 8 ms p99 (then the GPU path is certainly within budget), PROBABLY GO with the measured number and the reasoning if it is between 8 and 16 ms, or the fallback to try (partial uploads from dirty rectangles; the accelerated path) with its estimated size. The owner's run on real hardware decides finally; the report says precisely how to run it.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `tools/cef/fetch.sh` downloads and verifies the Linux distribution here (the report records the time and size).
- The engine tests and `crates/browser/tests/embedded.rs` pass here under Xvfb with CEF fetched; they skip cleanly without it.
- `--bench-browser 20` under `crates/eludite/tools/browser-spike-linux.sh` produces the results file and the screenshot showing the animation page drawn inside the shell.
- Headless GPUI test: `BrowserSurface` uploads only when the sequence changes and reports the dirty rectangles.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green; `dotnet build` and `dotnet test` unchanged.

## Budget

- The proposal's: shell frame cost under 8 ms p99 while a 1600 by 1000 tab plays a 60 fps animation (decided on the owner's machine; the Xvfb number reported here).
- Window open to first pixel with the engine running under 300 ms; engine cold start under 2 s (CEF present, profile warm); shell memory with the tab open plus 60 MB at most.
- Cold start of Eludite unchanged: `eludite-chromium` and libcef are never loaded by the shell process.
- Disk: the CEF cache under 1.5 GB; the engine crate's build under 2 GB of target.

## Exit criterion

1. The engine process starts, renders a tab offscreen into shared memory, the shell draws it, and the measurements exist in the results file with the screenshot.
2. The report gives the numbers, the GO or fallback verdict with the reasoning, the macOS bundle findings (from CEF's documentation and the `cef` crate's examples, since no macOS machine is here), the Linux sandbox findings, the dependency list with SPDX ids (the `cef` crate's own dependencies included), and what brief B needs.
3. `CLAUDE.md`, `README.md`, the briefs index and `tools/cef/README.md` match the repository; the control schemas are in `protocol/schemas/browser-rpc/`.

## Out of scope

- The Web Browser window, tabs UI, address bar, DevTools tab, the "Agent is driving" strip, `record` (brief B); launch integration (C); JavaScript debugging (D); packaging and the SUID helper installation (E).
- The accelerated offscreen path itself (only its estimate), any GPUI patch, bumping the GPUI revision.
- Windows and macOS runs.
