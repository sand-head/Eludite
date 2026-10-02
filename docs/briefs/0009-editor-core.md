# Brief 0009: Vendor Zed's text crates and build the editor core

Status: done on Linux ([report](0009-report.md)); memory budget for the 100k-line file not met, Windows and macOS not run
Phase: 1
Plan reference: PLAN.md sections 2 (principle 1), 3 (D1, D5), 4.1, 7, 9, 13 (risk 5), 14 (decision 8)
Related ADRs: ADR-0001, ADR-0005
Depends on: brief 0001 (report, section on the vendoring audit)

## Goal

Give Eludite a real editor core. Vendor the Zed crates the brief 0001 audit approved, replace the `ropey` stand-in in `crates/editor` with them, and build the editor view: a GPUI element that renders a buffer with line numbers, scrolls at refresh rate on 100,000 lines, supports cursors, selections, typing, undo and redo, and highlights C# and Rust with tree-sitter. After this brief, any later brief can show a file in the document area and wire LSP features to it.

## Files in scope

- `vendor/**` (new vendored crates, each with `WHY.md`; plus `vendor/sync.sh` that re-fetches at the pinned commit and reports drift)
- root `Cargo.toml` (add `vendor/*` workspace members and a `[patch]` section so GPUI's own dependencies resolve to the vendored copies, giving one copy of each crate in the build)
- `Cargo.lock`
- `crates/editor/**`
- `docs/briefs/0009-report.md` (new)

Do not touch `crates/eludite`, `crates/docking`, `crates/ui`, `crates/lsp`, the host, `docs/adr/**`, `spikes/**`. The editor is exposed as a library element plus an example binary (`cargo run -p eludite-editor --example viewer -- <file>`) so it can be verified without the app; wiring into the document area is the next brief.

## Contract

- Vendor exactly the crates the audit approved as "vendor": `sum_tree`, `rope`, `text`, `clock`, `fuzzy`, from zed-industries/zed at the GPUI pin `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8`. Each gets `vendor/<crate>/WHY.md` stating upstream path, commit, SPDX license (from the crate's own `Cargo.toml` or LICENSE file; the audit found `sum_tree` Apache-2.0 and the others GPL-3.0-or-later), transitive Zed dependencies and every local change (ideally none). Preserve upstream LICENSE files. Do not vendor anything visual (ADR-0001). PLAN.md decision 8 says vendor over own: if `text` needs a small Zed support crate to compile, vendor that too and record it rather than rewriting it.
- `crates/editor`: `Buffer` built on vendored `text::Buffer` (anchors, edits, undo history, line endings preserved, BOM preserved, large-file threshold documented); `EditorView` GPUI element with line numbers, soft scrolling, multiple cursors, selections (mouse and keyboard), insert, delete, undo, redo, find-in-buffer; a `Highlighter` using the `tree-sitter` crate with `tree-sitter-c-sharp` and `tree-sitter-rust` grammars and highlight queries, incremental re-parse on edit, mapped to the `crates/ui` theme token colors. Record the SPDX ids of the tree-sitter crates (MIT expected).
- The UI thread never waits on parsing: parse and highlight off-thread, render stale highlights until the new ones arrive (PLAN.md 4.1 and principle 1).
- Keep the public API narrow and documented in the crate's `//!` docs so the next brief can embed `EditorView` without reading its internals.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers, no co-author or sign-off lines. Vendoring is its own commit before any editor code.

## Proving test

- `cargo test -p eludite-editor`: buffer edits and undo, anchor stability across edits, line-ending and BOM preservation, multi-cursor insert, selection semantics, highlighting of C# and Rust fixtures (assert token kinds at positions), and headless GPUI tests for keyboard and mouse input.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check` green with the vendored crates in the workspace (if a vendored crate fails workspace lints, scope the lint exception to `vendor/*` in the root manifest and say so).
- `cargo build --workspace` shows a single copy of each vendored crate (`cargo tree -d` has no duplicates for them).
- The example viewer on a generated 100k-line C# file and on a 20k-line Rust file, with a `--bench-scroll` and `--bench-type` flag printing frame-cost p50/p95/p99 as JSON, run under the nested compositor if the session is locked (method in `spikes/0005-acp-panel/tools/nested.sh`).

## Budget

- Scroll frame cost under 8 ms p99 on the 100k-line file; keystroke frame cost under 8 ms p99 (PLAN.md section 9, the frame-submitted row).
- Memory for the 100k-line buffer with highlighting under 150 MB in the example viewer.
- Opening a 10 MB file renders first text within 300 ms with highlighting streaming in after.

## Exit criterion

1. The five crates are vendored with `WHY.md` files, `vendor/sync.sh` reports no drift at the pin, and the build has one copy of each.
2. `crates/editor` passes its tests and the workspace stays green.
3. The example viewer meets the budgets on Linux, numbers in the report.
4. The report lists every local change to vendored code (goal: none), the tree-sitter and grammar crate versions and licenses, and sizes the next brief (wire into the document area, LSP decorations: diagnostics, inlay hints, semantic tokens).

## Out of scope

- Wiring into the app or the docking document area.
- LSP-driven features: diagnostics squiggles, completion popup, hover, inlay hints, semantic tokens. Leave hooks (decoration layers) and document them.
- Languages beyond C# and Rust grammars (the mechanism must make adding one a registration).
- Folding, minimap, sticky scroll, IME, right-to-left text; note known gaps.
- Windows and macOS runs: report them as not run.
