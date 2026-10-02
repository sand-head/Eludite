# Brief 0001 report: GPUI shell and docking spike, Zed vendoring audit

Date: 2026-10-01. Brief: [0001-gpui-shell-and-docking-spike.md](0001-gpui-shell-and-docking-spike.md). Prototype: [`spikes/0001-gpui-shell/`](../../spikes/0001-gpui-shell/). Raw results: [`spikes/0001-gpui-shell/results/`](../../spikes/0001-gpui-shell/results/).

## 1. Summary

- The prototype builds and runs on Linux with GPUI at the pinned rev `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8`. No other rev was needed. Windows and macOS were **not run on this machine**.
- All five docking features work in the prototype and are covered by headless GPUI tests that drive them with real GPUI mouse events. One VS behavior does not map onto GPUI: dragging a *floating* OS window back onto the main window's dock guides (section 4).
- Cold start to first presented frame: **130 ms** median (warm, XWayland), **142 ms** (Wayland, nested compositor). Resident memory with the 100k-line buffer: **~94 MB**. Both are far inside budget.
- Scrolling the 100k-line file: **zero dropped frames** at 60 Hz under Wayland and XWayland in a nested KWin. On the real 165 Hz panel through XWayland, **4.5 to 5.1 % of frame intervals were 2 refresh periods**, with only 3.5 to 4 ms of frame work. That points at GPUI's free-running X11 frame timer, not at rendering cost, but it was measured with the user session locked (next point), so treat it as unconfirmed.
- **The user session was locked for the whole run.** KWin stops sending Wayland frame callbacks to a hidden surface, so GPUI on the real Wayland display draws nothing after the first frame, and every frame-paced measurement stalls. I measured Wayland inside a nested `kwin_wayland --virtual` (60 Hz virtual output, real GPU) instead, and XWayland both on the real display server and nested. **A real-display Wayland run at 165 Hz is still owed**, as is the real-keyboard (`/dev/uinput`) latency run, which the injector refuses to do while the session is locked.
- Keystroke to present, p99: **9.7 ms** (XWayland, 165 Hz), **17 ms** (Wayland, 60 Hz), **19.6 ms** (XWayland, 60 Hz). The frame's own cost, render to present, p99: **3.4 to 4.9 ms** everywhere. As written, the budget (under 8 ms p99) fails, but it cannot be met by any renderer that waits for the display's next frame at 60 Hz: one refresh interval is 16.7 ms. Section 3.4 asks for a decision on what the 8 ms measures.
- Audit: vendor `sum_tree`, `rope`, `text`, `clock`, `fuzzy` (later `streaming_diff`); keep `gpui`, `gpui_platform` and GPUI's Apache support crates as pinned git dependencies; rewrite the tree-sitter glue by porting `language/src/syntax_map.rs`, because the `language` crate depends on `theme`.
- Verdicts (section 8): Linux Wayland **GO, provisional**; Linux XWayland **GO, provisional**; Windows **undetermined, pending runs**; macOS **undetermined, pending runs**.

## 2. Machine

| | |
|---|---|
| Machine | Laptop, AMD Ryzen 9 7940HS (8 cores, 16 threads), 30 GB RAM |
| GPUs | AMD Radeon 780M (integrated, RADV PHOENIX) and AMD Radeon RX 7700S (discrete, RADV NAVI33) |
| Driver | Mesa 26.2.2 (RADV, Vulkan 1.4.354), amdgpu kernel driver |
| GPU GPUI used | The **integrated 780M**: GPUI's wgpu backend takes the first adapter that passes its configuration test. `ZED_DEVICE_ID=0x7480` selects the discrete GPU. |
| OS | CachyOS (Arch-based), kernel 7.2.4-1-cachyos |
| Compositor | KDE Plasma 6.7.5, KWin 6.7.5 (Wayland session), Xwayland 24.1.13 |
| Display | Built-in eDP panel, 2560x1600 at **165.00 Hz**, scale 1.25, VRR "Automatic" (KWin enables it only for full-screen windows, so not in effect here). Read with `kscreen-doctor -o`. |
| Nested compositor | `kwin_wayland --virtual --xwayland --no-lockscreen --width 1920 --height 1200`: virtual output at ~59.9 Hz (estimated from frame intervals), scale 1.0, clients still render on the real GPU through Vulkan |
| Toolchain | rustc 1.98.1, release profile with thin LTO, codegen-units 16 |

Session state: `loginctl show-session` reported `LockedHint=yes` for the whole run.

## 3. Measurements

All runs used the release build, the 100,000-line generated buffer, a 1400x900 logical window, the default layout, and the iGPU unless noted. `tools/bench_all.py` runs each set and writes `results/<label>.json` with every run's raw output. Medians and ranges below are across runs; per-run numbers are percentiles over frames or keystrokes.

Configurations:
- **XWayland (real)**: `WAYLAND_DISPLAY` unset, so GPUI used its X11 backend against the session's Xwayland on KWin and the 165 Hz panel. This is XWayland, **not a native X11 session**, and the session was locked.
- **Wayland (nested)**: GPUI's Wayland backend against a nested virtual KWin at 60 Hz.
- **XWayland (nested)**: GPUI's X11 backend against the nested KWin's Xwayland at 60 Hz.
- **Wayland (real)**: not measurable while locked (section 5, item 1).

### 3.1 Cold start

Method: `bench_all.py` records `time.time_ns()` just before it spawns `spike-gpui-shell --bench-start` and passes it in `SPIKE_LAUNCH_WALL_NS`. The app generates the 100k-line buffer, opens the window, and, in a `defer` callback scheduled from the text view's first `render`, takes the wall time. GPUI runs that callback after `Window::present` returns for the first frame. Launch to that moment counts as "start to interactive window": the window is focused and accepts keys from then on. 20 runs per configuration, 0.3 s apart. The first run is reported separately and the other 19 are "warm".

| Configuration | Warm, median (min to max) | Main to first present, median | First run |
|---|---|---|---|
| XWayland (real) | **129.8 ms** (123.8 to 138.2) | 126.5 ms | 975 ms |
| Wayland (nested) | **142.1 ms** (135.8 to 151.6) | 138.7 ms | 166 ms |
| XWayland (nested) | **128.7 ms** (122.8 to 134.2) | 125.8 ms | 256 ms |

Budget under 300 ms: **pass**. Where the time goes (warm, XWayland): buffer generation 18 ms, GPUI app init 18 ms, `open_window` about 85 ms (GPU device and surface setup), first frame 5 to 10 ms.

The **first launch of a freshly built binary takes 1.0 to 2.0 s**, all of it inside `open_window`. It reproduces after copying the binary to a new path, and it is off-CPU: 57 ms user and 274 ms system time over 984 ms wall clock. Disabling Mesa's shader cache (`MESA_SHADER_CACHE_DISABLE=true`) does not reproduce it, which rules out shader compilation. I did not find the cause (no `strace` on this machine). It matters for first run after install, not for warm starts.

### 3.2 Scrolling the 100k-line buffer

Method: `--bench-scroll` waits 60 frames, then on every frame (a `Window::on_next_frame` callback chained from itself) it advances the `uniform_list` scroll offset by N lines and notifies the view, from the top of the file to the bottom. **Frame interval** is the time between successive frame starts (the `on_next_frame` callbacks GPUI runs at the start of each platform frame request), which is the cadence at which new frames reach the compositor. **Frame work** is frame start to the end of `present` for that frame. It includes layout, painting, glyph rasterization, and any blocking in the swapchain present. Dropped frames are counted as intervals over 1.5x and over 2x the refresh interval. 40 lines per frame (2,500 frames) is the stress case: every visible line is new, about 45 lines, so every line is shaped every frame. 8 lines per frame (12,500 frames) is a fast but readable scroll.

| Configuration | Hz | Lines/frame | Runs | Interval p50 | Interval p95 | Interval p99 | Max | >1.5x | >2x | Work p99 |
|---|---|---|---|---|---|---|---|---|---|---|
| XWayland (real) | 165 | 40 | 3 | 6.06 ms | 12.11 to 12.12 | 12.16 to 12.20 | 12.8 to 13.1 | 156 to 195 of 2,499 | **113 to 122** (4.5 to 4.9 %) | 6.5 to 6.8 ms |
| XWayland (real) | 165 | 8 | 1 | 6.06 | 12.12 | 12.18 | 13.4 | 992 of 12,496 | **640** (5.1 %) | 6.6 ms |
| XWayland (real), dGPU | 165 | 40 | 1 | 6.06 | 12.12 | 12.14 | 12.8 | 162 | 113 | 6.5 ms |
| Wayland (nested) | 60 | 40 | 3 | 16.70 to 16.71 | 17.40 to 17.45 | 17.67 to 17.79 | 18.4 to 18.9 | **0** | **0** | 5.1 to 5.4 ms |
| Wayland (nested) | 60 | 8 | 1 | 16.73 | 17.39 | 17.72 | 26.6 | 1 | **0** | 4.3 ms |
| XWayland (nested) | 60 | 40 | 3 | 16.70 | 16.73 to 16.76 | 16.80 to 17.19 | 16.9 to 18.3 | **0** | **0** | 5.3 to 5.7 ms |
| XWayland (nested) | 60 | 8 | 1 | 16.70 | 16.77 | 17.01 | 18.2 | 0 | **0** | 4.5 ms |

Budget "sustains the monitor refresh rate" and exit criterion 2 "zero frames above 2x the refresh interval at p99":
- At 60 Hz (nested, Wayland and XWayland): **pass**. No interval exceeded 2x the refresh interval in about 30,000 frames, and only one exceeded 1.5x.
- At 165 Hz (XWayland, real display server, session locked): **fail**. p99 sits at 2.0x (12.16 to 12.20 ms against 12.12 ms) and about 5 % of frames miss a vblank. Work p99 is about 6.6 ms against a 6.06 ms period, but median work is 3.5 to 4 ms. The discrete GPU gives the same drop rate, and the 8-lines-per-frame run (little new text) drops at the same rate as the 40-line run, so neither GPU throughput nor text shaping explains it. GPUI's X11 backend drives frames from a calloop timer at the mode's refresh rate (`start_refresh_loop` in `gpui_linux/src/linux/x11/client.rs`), not from vblank. With a FIFO swapchain, a timer that drifts against the real vblank will periodically wait in present and lose a slot. This is my leading explanation; it is not proven. A locked session may also change how Xwayland paces presents, so this needs re-measuring unlocked, and on a native X11 session.

### 3.3 Resident memory

Method: `VmRSS` and `VmHWM` from `/proc/self/status`, read at the first presented frame, after each scroll benchmark, and after each keystroke benchmark (typing more than 500 characters into the 100k-line buffer).

| Point | XWayland (real) | Wayland (nested) | XWayland (nested) |
|---|---|---|---|
| First frame, median of 19 | 89.8 MB | 89.1 MB | 89.8 MB |
| After full 100k scroll | 93.3 to 93.9 MB | 93.4 to 95.0 MB | 93.4 to 97.3 MB |
| After 500 keystrokes | 92.7 to 92.9 MB | 92.8 to 93.2 MB | 92.7 to 93.0 MB |

Peak RSS equals final RSS in every run. Budget under 400 MB: **pass**, with about 300 MB to spare. The naive `Vec<String>` buffer is about 6 MB of that.

### 3.4 Keystroke to present

Method (`--bench-keys 500`): the cursor is placed at line 50,000. A task sleeps a random 15 to 45 ms, so keys arrive at random phase relative to the display's refresh, then calls `Window::dispatch_keystroke`. That goes through GPUI's normal key dispatch to the text view's `on_key_down` handler. The handler records `Instant::now()`, edits the buffer, and notifies. The next `render` of the view schedules a `defer` callback, which GPUI runs after that frame's `present`. **Key to present** is handler entry to end of present. **Render to present** is the view's render (inside `Window::draw`) to end of present, the frame's own cost without the wait for the next frame tick. The keys are letters, plus Backspace every 17th key and Enter every 40th, so lines split and join in the 100k-line vector. Three runs of 500 keystrokes per configuration.

| Configuration | Hz | Key to present p50 | p95 | **p99** | max | Render to present p99 |
|---|---|---|---|---|---|---|
| XWayland (real) | 165 | 6.1 to 6.3 ms | 8.9 to 9.3 | **9.66 to 9.87** (median 9.69) | 10.7 | 4.59 to 4.94 |
| Wayland (nested) | 60 | 2.8 to 3.1 | 6.6 to 7.1 | **16.1 to 17.6** (median 17.2) | 19.5 | 3.35 to 3.63 |
| XWayland (nested) | 60 | 10.9 to 11.6 | 18.6 to 19.2 | **19.4 to 20.1** (median 19.6) | 21.0 | 3.59 to 3.68 |

What this measures and what it leaves out:
- It starts at GPUI's key handler. The kernel-to-handler segment, from the input device through libinput and the compositor to the client, is not included. `tools/inject_keys.py` measures that segment with a real `/dev/uinput` keyboard (key written, then present). It refuses to run while the session is locked, because the keys would go to the lock screen, so it has **not been run**.
- It ends when GPUI has handed the frame to the swapchain, not when photons leave the panel. Compositor latency and scan-out add at least another fraction of a refresh period. No software-only measurement covers that segment; a photodiode or high-speed camera would.

Budget "under 8 ms p99": **fails as written in every configuration measured.** The reason is structural. GPUI draws only when the platform asks for a frame: a compositor frame callback on Wayland, the refresh timer on X11. A keystroke therefore waits up to one refresh interval before its frame is drawn. At 60 Hz that interval is 16.7 ms, so no vsync-paced renderer can meet 8 ms p99 there, Avalonia and VS included. Wayland's median is low (2.8 ms) because GPUI draws right away when its frame loop is idle. Its p99 is high because it waits for the compositor's frame callback whenever a frame is already in flight. The frame's own cost, render to present at p99 3.4 to 4.9 ms, is well under 8 ms everywhere. **Decision needed from the human:** either define keystroke to pixel as handler (or OS input event) to frame submitted, excluding the wait for the display's next frame, or state the reference display's refresh rate (at 165 Hz, worst-case wait 6.06 ms plus 3 to 5 ms of render already lands near 9 to 10 ms). I recorded this in ADR-0001's "Revisit when" note.

## 4. Docking features

Prototype layout: the document area has tabs for `Generated100k.cs` (the text view) and Welcome. On the right, Solution Explorer and Git Changes are tabbed above Properties. At the bottom, Error List and Output are tabbed. On the left, Toolbox is auto-hidden. The model is VS's: three dock sides holding tab groups, floating groups in their own OS windows, and per-side auto-hidden windows (`src/layout.rs`).

| Feature | Linux Wayland | Linux XWayland | Windows | macOS | How it works / evidence |
|---|---|---|---|---|---|
| Drag to dock | works | works | not run on this machine | not run on this machine | Drag a tool window's title bar or tab; guides appear on the left, right and bottom of the document area; drop on one to dock. Test `docking_by_mouse` (Output tab to the left guide). |
| Tab with another window | works | works | not run | not run | Drop on another group to add a tab. Test: Properties title bar onto the Error List group. |
| Float | works (OS window; position ignored by the compositor, see below) | works | not run | not run | Drop anywhere that is not a guide or group, or press Float. The window opens as its own GPUI OS window. Dock, or closing the window, docks it back. Test: Output dropped on the document area opens one floating window. |
| Auto-hide | works | works | not run | not run | Auto Hide moves the window to its dock's edge strip; click to slide out; Pin docks it again. Test covers hide, slide out and pin. |
| Persist and restore (JSON) | works | works | not run | not run | Saved atomically on every change to `<config dir>/eludite-spike/layout.json` and restored at startup, floating windows included. Test `layout_save_restore_roundtrip`: drive the shell through all four operations, reload the file it wrote into a new window, check equality and the floating OS window; dock back and check the file again. |

Evidence: `cargo test` in `spikes/0001-gpui-shell` (9 tests, all pass). These run on GPUI's headless test platform with real GPUI mouse and key event dispatch, so they show the docking logic and GPUI's drag-and-drop work. They do not show that it works through a compositor. I also launched the interactive prototype on both backends. Because the session was locked, I could not drag a window with a real pointer or look at the screen, so per-backend "works" means the same code ran on that backend, plus the headless tests. **Real-pointer checks on an unlocked session are still owed for both Linux backends.**

What does not map onto GPUI as VS does it:
- **Dragging a floating window back onto the main window's guides does not work.** GPUI drag-and-drop lives inside one window. Moving a floating OS window is done by the window manager, and on Wayland a client learns neither its window position nor the global pointer position, so the main window cannot show guides under a window being moved. The spike re-docks with a Dock button or by closing the window. A possible route: GPUI can hand an in-window drag to the OS as a platform drag-and-drop when the pointer leaves the window (`external_payload` / `promote_external_drag_to_platform` in `window.rs`). That could carry a tab out of one Eludite window into another. Not tried in the spike.
- **Floating window position is advisory on Wayland.** `WindowBounds` origin is ignored by the compositor, so a restored layout gets the right size but not the right place. This is Wayland policy, not a GPUI bug.
- Drop events carry no position; the spike reads `Window::mouse_position()` in the drop handler. This is a minor API gap.

## 5. GPUI issues found

1. **Wayland: on a hidden surface, `on_next_frame` work never runs.** Repro: on Wayland, open a window, lock the session (or otherwise hide the surface), and chain `window.on_next_frame(|w, cx| { /* record */ w.on_next_frame(...) })`. The first callback runs, then none. GPUI's Wayland frame loop sits in `AwaitingCallback` and `schedule_frame` does nothing until the compositor sends a frame callback, which KWin does not do for hidden surfaces (`gpui_linux/src/linux/wayland/window.rs`, `complete_frame` and `schedule_frame`). This is correct per the protocol and saves power. The consequence: any work that waits on the next frame (animations, benchmarks, the "after first present" hooks in this spike) stalls indefinitely while the window is hidden. Edits do still get drawn: on the locked Wayland session a key bench completed 49 of 50 samples at p50 30 ms. Eludite must not put non-visual work behind `on_next_frame`.
2. **X11: the frame loop is a free-running timer, not vblank-locked; suspected cause of about 5 % missed frames at 165 Hz.** See section 3.2. Repro: `cargo run --release -- --bench-scroll --refresh-hz 165` with `WAYLAND_DISPLAY` unset on a 165 Hz output, then check `intervals_over_2x_refresh`. Unconfirmed: measured on a locked session through Xwayland. At 60 Hz in a nested compositor, the same code drops nothing.
3. **First launch of a new binary takes about 1 s, off-CPU, inside `open_window`** (section 3.1). Not root-caused; not Mesa's shader cache. Repro: build, then run `--bench-start` twice.
4. **Adapter choice on hybrid-GPU laptops**: GPUI takes the first adapter that passes its configuration test, which was the integrated GPU here. That is reasonable for power, but the only override is `ZED_DEVICE_ID`. The discrete GPU did not change frame pacing (section 3.2).
5. **Pitfall, not a bug:** opening a window from inside an entity's `update`, when the new window's root view reads that entity during its first render, panics with "cannot read X while it is already being updated". Floating tool windows must be opened outside the shell's update (the spike defers it, see `Shell::sync_floating_windows`).

## 6. Zed crate audit

Source: the Zed checkout cargo fetched for the pinned GPUI dependency, `~/.cargo/git/checkouts/zed-a70e2ad075855582/20d29fc/`, commit `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8` (committed 2026-10-01). For each crate I read `license` in its `Cargo.toml` and computed its transitive Zed-crate dependencies from the `[dependencies]` and `[target.*.dependencies]` tables (dev-dependencies excluded). "Path" is relative to the Zed repository root. Lines of Rust are `src/**/*.rs`.

Zed crates without a `license` field (`language_core`, `grammars`) carry a `LICENSE-GPL` symlink to Zed's root GPL-3.0 text, Zed's convention for GPL crates; I record them as GPL-3.0-or-later (inferred) and the vendoring step must confirm it with upstream before relying on it.

Verdicts: **vendor** = copy into `vendor/` at this commit with a `WHY.md`, and point GPUI's copy at it with `[patch."https://github.com/zed-industries/zed"]` so there is one copy in the build. **depend by git** = keep as a git dependency at the GPUI rev (it moves when GPUI moves). **rewrite** = write our own (porting GPL code with attribution is allowed, ADR-0005), do not take the crate.

| Crate | Path | SPDX | Rust LOC | Transitive Zed deps (non-dev) | Verdict | Reason |
|---|---|---|---|---|---|---|
| gpui | crates/gpui | Apache-2.0 | 86,695 | bench_metrics, collections, gpui_macros, gpui_shared_string, gpui_util, http_client, path, refineable, scheduler, sum_tree, util, util_macros, zlog, ztracing, ztracing_macro (all Apache-2.0) | depend by git | Already pinned; no local patch needed yet; vendor only when we must carry a fix (see section 5). |
| gpui_platform | crates/gpui_platform | Apache-2.0 | 230 | gpui, gpui_linux, gpui_wgpu, gpui_windows, gpui_macos, gpui_apple, gpui_web + gpui's set (all Apache-2.0) | depend by git | Required for `application()` at this rev; moves with gpui. |
| gpui_linux, gpui_wgpu, gpui_windows, gpui_macos, gpui_apple | crates/gpui_* | Apache-2.0 | 15,941 (linux), 6,137 (wgpu) | gpui's set | depend by git | Platform backends, pulled by gpui_platform; the X11 frame-loop issue would be patched here if we patch anything. |
| sum_tree | crates/sum_tree | Apache-2.0 | 3,327 | ztracing, ztracing_macro, zlog, collections, gpui_util | vendor | Core B+tree under rope, text and GPUI's own lists; tiny dependency set; we want it stable under the editor. |
| rope | crates/rope | GPL-3.0-or-later | 4,132 | sum_tree, util, ztracing (+ collections, gpui_util, path, util_macros, zlog, ztracing_macro) | vendor | Proven chunked rope with UTF-16/point/offset dimensions; months of work; no UI. |
| text | crates/text | GPL-3.0-or-later | 6,746 | clock, collections, rope, sum_tree, util (+ gpui_util, path, util_macros, zlog, ztracing, ztracing_macro) | vendor | Buffer with anchors, edits, undo/redo and selections; the anchor model lives here. No gpui dependency. |
| clock | crates/clock | GPL-3.0-or-later | 338 | none | vendor | Lamport/global clocks that `text` anchors require. |
| collections | crates/collections | Apache-2.0 | 418 | gpui_util | depend by git | HashMap/HashSet aliases; already in the build through gpui. |
| util | crates/util | Apache-2.0 | 10,164 | collections, gpui_util, path, util_macros | depend by git (transitively only) | Grab-bag (paths, shell, fs helpers); needed by rope/text/gpui; do not use directly, keep Eludite code off it. |
| gpui_util, path, util_macros, refineable, scheduler, gpui_macros, gpui_shared_string, http_client, bench_metrics, zlog, ztracing, ztracing_macro, watch | crates/<name> | Apache-2.0 | small | within this set | depend by git | Support crates of gpui; take whatever gpui's rev brings. `http_client` is linked; the spike makes no network calls, but I did not audit GPUI for any. |
| fuzzy | crates/fuzzy | GPL-3.0-or-later | 1,210 | gpui, gpui_util, path (+ gpui's set) | vendor | Fuzzy matcher for Ctrl+Q / Ctrl+T / Go To; depends on gpui only for its background executor, which we use anyway. |
| fuzzy_nucleo | crates/fuzzy_nucleo | GPL-3.0-or-later | 1,314 | fuzzy, gpui, gpui_util, path (+ gpui's set) | rewrite (defer) | Newer matcher on the third-party `nucleo` crate (MPL-2.0); take `fuzzy` first, revisit if path matching needs nucleo's quality. |
| language (tree-sitter glue: `src/syntax_map.rs`, 2,366 lines) | crates/language | GPL-3.0-or-later | 27,714 | 47 Zed crates incl. **theme**, settings, lsp, rpc, proto, fs, git, task, telemetry, zed_actions | rewrite (port `syntax_map.rs`) | The crate drags in `theme` (excluded by ADR-0001) and Zed's settings, LSP, RPC and project model; port the syntax map (layers, injections, incremental reparse) into our editor crate with attribution instead. |
| language_core | crates/language_core | GPL-3.0-or-later (inferred, no `license` field) | 2,570 | collections, gpui_shared_string, gpui_util, path | rewrite (port grammar/highlight_map/queries) | Half of it is Zed's LSP adapter and toolchain model, which Eludite replaces with its hosts; take the grammar and highlight-map pieces with the syntax-map port. |
| grammars | crates/grammars | GPL-3.0-or-later (inferred) | 100 + query files | language_core, util (+4) | rewrite | Embeds Zed's query files with Zed's capture names; Eludite needs its own queries mapped to VS token colors. |
| streaming_diff | crates/streaming_diff | GPL-3.0-or-later | 1,125 | rope (+ rope's set) | vendor (deferred to Phase 4) | Small and UI-free; useful for streaming agent edits into the review surface (PLAN 5.6); not needed before then. |
| buffer_diff | crates/buffer_diff | GPL-3.0-or-later | 4,404 | language, settings, gpui, text, rope + language's 47 | rewrite | Depends on `language` and therefore on `theme`; write diff-hunk tracking on vendored `text`. |
| multi_buffer | crates/multi_buffer | GPL-3.0-or-later | 17,707 | **theme**, language, buffer_diff, settings + 47 | rewrite | Direct `theme` dependency, excluded by ADR-0001; multibuffers (PLAN 8) are written on vendored `text`. |
| lsp | crates/lsp | GPL-3.0-or-later | 2,878 | gpui, release_channel, util (+ gpui's set) | rewrite | Zed's client assumes Zed's process and settings model; Eludite's `crates/lsp` talks to `eludite-host` (ADR-0003). |
| sqlez | crates/sqlez | GPL-3.0-or-later | 2,620 | collections, util | rewrite | Not needed for Phase 1 state; use a plain SQLite binding if PLAN 4.12 needs one. |

Excluded by ADR-0001 and not audited for reuse: `editor`, `workspace`, `ui`, `theme`, `project`, `terminal_view`, the agent panel crates.

## 7. Crates to take

Each goes to `vendor/<crate>/` at Zed commit `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8` (the GPUI pin). The root `Cargo.toml` gets `[patch."https://github.com/zed-industries/zed"]` entries for the vendored crates, so GPUI and Eludite share one copy. The vendored crates keep depending on GPUI's Apache support crates (`collections`, `util`, `gpui_util`, `path`, `util_macros`, `zlog`, `ztracing`, `ztracing_macro`) by git at the same rev. The actual vendoring is a later brief (out of scope here).

| Crate | Upstream path | Commit | SPDX | Planned `WHY.md` |
|---|---|---|---|---|
| sum_tree | crates/sum_tree | 20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8 | Apache-2.0 | Why: the B+tree with summaries under rope, text and GPUI's lists; vendored so the editor core does not move when GPUI is bumped, and patched into GPUI so there is one copy. Local changes: none. Upstream sync: with each deliberate GPUI bump (ADR note), diff `crates/sum_tree` and re-apply. |
| rope | crates/rope | same | GPL-3.0-or-later | Why: chunked rope with offset, point and UTF-16 dimensions, the text storage of Eludite's editor (PLAN 4.1). Local changes: none planned; candidates are dropping the `rayon` parallel paths if they cost startup, and line-ending tracking for CRLF/CR files (the spike shows mixed endings must round-trip). Sync: with GPUI bumps. |
| text | crates/text | same | GPL-3.0-or-later | Why: buffer, edits, undo/redo, selections and the **anchor** model, which diagnostics, breakpoints and agent edits attach to. Local changes: none planned; the replica-id and Lamport machinery for collaboration stays, because removing it would fork the anchor code. Sync: with GPUI bumps. |
| clock | crates/clock | same | GPL-3.0-or-later | Why: Lamport and global clocks that `text` anchors need. Local changes: none. Sync: together with `text`. |
| fuzzy | crates/fuzzy | same | GPL-3.0-or-later | Why: fuzzy matcher for Ctrl+Q, Ctrl+T, Go To File and the command palette (PLAN 8). Local changes: none; it uses GPUI's background executor, which Eludite already has. Sync: with GPUI bumps. |
| streaming_diff (later) | crates/streaming_diff | same | GPL-3.0-or-later | Why: streams model edits as diffs for the agent review surface (PLAN 5.6). Take it when that work starts, not before. |

Not vendored: `language` and `language_core`. Their tree-sitter glue (`syntax_map.rs`, grammar and highlight-map types) gets ported into Eludite's editor crate with attribution, because `language` depends on `theme`, which ADR-0001 excludes, and on Zed's settings, LSP, RPC and project model.

## 8. GO / NO-GO on ADR-0001

Exit criteria, Linux:
1. Builds and runs from a clean checkout following the README: **pass** on Linux (spike README; `cargo run --release` inside `spikes/0001-gpui-shell`). The brief's literal `cargo run -p spike-gpui-shell` from the repository root does not work, because the spike is its own workspace by design; `--manifest-path spikes/0001-gpui-shell/Cargo.toml` does.
2. Scrolling: **pass at 60 Hz** (Wayland and XWayland, nested); **fail at 165 Hz** through XWayland on a locked session (about 5 % of intervals at 2x; suspected X11 timer issue, unconfirmed). Keystroke under 8 ms p99: **fail as written** in every configuration (9.7 / 17 / 19.6 ms). It passes if the metric excludes the wait for the next frame (render to present p99 at most 4.9 ms). See 3.4.
3. Docking: all five features work on both Linux backends (headless GPUI tests plus launching on each backend); real-pointer checks on an unlocked session are owed. Re-docking a floating window by dragging it is not supported (section 4).
4. Audit: **complete** (section 6).
5. This section, plus the vendor list in section 7.
6. Fallback assessment: below, because metrics failed as written.

| OS | Verdict | Numbers |
|---|---|---|
| **Linux, Wayland** | **GO, provisional.** Owed before it is final: one run on the real 165 Hz display with the session unlocked (`tools/bench_all.py --label linux-wayland-real --refresh-hz 165`, `tools/inject_keys.py`), and the keystroke-metric decision in 3.4. | Nested KWin, 60 Hz: start 142 ms; scroll 0 intervals over 2x (1 over 1.5x) in about 20,000 frames; key to present p99 17.2 ms (render to present 3.5 ms); RSS 94 MB. |
| **Linux, XWayland** (not a native X11 session) | **GO, provisional.** The 165 Hz frame drops need a re-run on an unlocked session; if they persist, patch or report GPUI's X11 frame timer (section 5, item 2). Native X11 was not tested. | Real Xwayland, 165 Hz: start 130 ms; scroll intervals over 2x 4.5 to 5.1 %; key to present p99 9.7 ms (render 4.6 to 4.9 ms); RSS 93 MB. Nested, 60 Hz: 0 drops; key to present p99 19.6 ms. |
| **Windows 11** | **Undetermined, pending runs.** Not run on this machine: every metric (cold start, scroll, keystroke, RSS, docking) is unmeasured. This is still the top risk (PLAN 13, risk 1). | not run on this machine |
| **macOS** | **Undetermined, pending runs.** Not run on this machine: every metric is unmeasured. | not run on this machine |

**Fallback assessment (Avalonia with NativeAOT).** No Linux result calls for the fallback. The metrics that failed as written fail because of the display's refresh interval (keystroke) or, most likely, GPUI's X11 frame timer (165 Hz scroll), not because of GPUI's rendering cost. Render to present stays at or under 5 ms p99 and frame work is 3.5 to 4 ms at the median while it shapes 45 new lines per frame. Avalonia also renders on its compositor's vsync-driven loop, so it would face the same keystroke arithmetic at 60 Hz. It would add GC pauses on the UI thread, the failure mode ADR-0001 exists to avoid, and NativeAOT shortens startup but does not remove the GC. The fallback is worth re-opening only if the Windows run shows GPUI's DirectX backend dropping frames or missing the render-cost budget in ways a GPUI patch cannot fix. ADR-0002 keeps that switch away from the hosts and protocols.

## 9. How to reproduce

From `spikes/0001-gpui-shell/` (see its README):

```
cargo test
cargo build --release
python3 tools/bench_all.py --label linux-wayland-real --refresh-hz 165          # real Wayland (session unlocked)
env -u WAYLAND_DISPLAY python3 tools/bench_all.py --label linux-xwayland --refresh-hz 165
python3 tools/inject_keys.py --count 300 --activate-kwin                         # real keystrokes via /dev/uinput
kwin_wayland --virtual --xwayland --no-lockscreen --socket wayland-spike \
  --width 1920 --height 1200 --exit-with-session <script running bench_all.py>  # nested runs as measured here
```

On Windows and macOS, run `bench_all.py` the same way (`--bench-*` modes are cross-platform; RSS is Linux-only, so take it from Task Manager or `footprint`/Activity Monitor) and record GPU, driver and refresh rate.
