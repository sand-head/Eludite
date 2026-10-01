# Brief 0001: GPUI shell and docking prototype, Zed vendoring audit

Status: in progress (Linux measured; real-display Wayland re-run owed; Windows/macOS pending). Report: [0001-report.md](0001-report.md)
Plan reference: PLAN.md sections 3 (D1), 8, 9, 10 (Phase 0 item 1), 13 (risk 1)
Related ADR: ADR-0001

## Goal

Decide whether GPUI is fit to build Eludite's shell on Windows, Linux and macOS, and decide which Zed crates may be vendored. Produce a throwaway prototype window with a docking layout and a large text buffer, a measured frame-rate report on all three OSes, and a crate-by-crate license and fitness audit.

## Files in scope

- `spikes/0001-gpui-shell/**` (new throwaway crate, not part of the production workspace members unless the human says so)
- `docs/briefs/0001-report.md` (the written report, new)
- `docs/adr/0001-shell-rust-gpui.md`: only the "Revisit when" dated note, if the result changes it

Do not touch `crates/**`, `protocol/**`, `dotnet/**` or `vendor/**`. The spike may read them.

## Contract

- Use GPUI from the git dependency pinned in the workspace (zed-industries/zed at rev `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8`). If a different rev is needed, justify it in the report.
- Rust toolchain from `rust-toolchain.toml` (1.98.1).
- Prototype features:
  - One window with a VS-style layout: a document area with tabs, a tool window docked right, one docked bottom.
  - Drag a tool window to another dock, tab it with another, float it, and auto-hide it. Persist and restore the layout from a JSON file.
  - A scrolling text view over a 100,000-line generated buffer (use a monospace font, ASCII plus some multi-byte lines, mixed line endings).
  - Typing into the buffer inserts text and re-renders.
- The audit lists every Zed crate the shell might want to reuse: GPUI, rope, sum-tree, text/buffer, anchors, fuzzy matcher, tree-sitter glue. For each, record SPDX license, upstream path and commit, transitive Zed-crate dependencies, and a verdict: vendor, depend by git, or rewrite.
- Zed crates that decide what a user sees (`editor`, `workspace`, `ui`, `theme`, `project`, `terminal_view`, agent panel) are excluded by ADR-0001 and are not audited for reuse.

## Proving test

- `cargo run -p spike-gpui-shell --release` starts the prototype. A headless GPUI test covers layout save and restore round-trip: `cargo test -p spike-gpui-shell`.
- A frame-time harness (`--bench-scroll`) scrolls the 100k-line buffer top to bottom and prints frame-time p50, p95 and p99 as JSON.
- Run on a real Windows 11, a Linux machine under Wayland, a Linux machine under X11, and macOS. VMs without a GPU do not count. Record the GPU, driver and display refresh rate.

## Budget

From PLAN.md section 9, applied to the prototype:
- Cold start to interactive window under 300 ms (release build, warm disk cache).
- Keystroke to pixel under 8 ms at p99 in the 100k-line buffer.
- Scrolling sustains the monitor refresh rate. Record dropped frames.
- Resident memory with the 100k-line buffer open under 400 MB.

## Exit criterion

All must hold, or the report states clearly which failed and why:
1. The prototype builds and runs on Windows, Linux and macOS from a clean checkout following only the README instructions.
2. On each OS, the 100k-line scroll shows zero frames above 2x the refresh interval at p99, and keystroke to pixel under 8 ms p99.
3. Docking supports drag to dock, tab, float, auto-hide, and layout restore on all three OSes. List any that did not work per OS.
4. The audit covers every candidate crate with SPDX id, pinned commit and verdict.
5. `docs/briefs/0001-report.md` ends with an explicit GO or NO-GO on ADR-0001 for each OS, with the numbers, and a list of vendored crates to take (name, path, commit, planned `WHY.md` content).
6. If any OS is NO-GO, the report names the specific failing metric and a one-paragraph fallback assessment (Avalonia with NativeAOT).

## Out of scope

- Production code in `crates/**`, and any design of the command bus or settings.
- Themes, icons, final visuals, menus, the status bar, keymaps.
- Syntax highlighting, tree-sitter integration, LSP or any .NET process.
- Accessibility, IME, and right-to-left text beyond noting known gaps.
- Actually vendoring crates into `vendor/`. The audit only decides.
- Fixing GPUI bugs upstream. Record them in the report with a minimal repro.
