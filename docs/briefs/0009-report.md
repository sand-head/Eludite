# Brief 0009 report: vendor Zed's text crates and build the editor core

Date: 2026-10-01. Brief: [0009-editor-core.md](0009-editor-core.md). Code: [`vendor/`](../../vendor/) and [`crates/editor/`](../../crates/editor/). Benchmark driver: [`crates/editor/tools/bench.sh`](../../crates/editor/tools/bench.sh).

## 1. Summary

- **Vendored, unchanged:** `sum_tree` (Apache-2.0), `rope`, `text`, `clock` and `fuzzy` (all GPL-3.0-or-later) from zed-industries/zed at `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8`. Each has a `WHY.md` and its upstream license file. **No local changes to vendored code.** No extra Zed support crate had to be vendored. `vendor/sync.sh` reports no drift at the pin, both from GitHub and offline from cargo's checkout.
- **One copy each:** GPUI's `sum_tree` is patched to `vendor/sum_tree`, and `cargo tree -d` lists none of the five crates and no tree-sitter crate. One deviation from the brief's wiring is explained in section 2: the vendored crates are members of their own workspace (`vendor/Cargo.toml`), not the root one.
- **Editor core works** and is covered by 51 tests: 43 unit tests and 8 headless GPUI tests. It has a `Buffer` on vendored `text`, an `Editor` model, an `EditorView` GPUI element, and a tree-sitter `Highlighter` running on a dedicated syntax thread.
- **Budgets (Linux, nested KWin at 60 Hz, quiet machine, 3 runs each):**

  | Budget | Limit | Result |
  |---|---|---|
  | Scroll frame cost, 100k-line C#, 40 lines/frame | < 8 ms p99 | **2.43 to 2.47 ms** (pass) |
  | Scroll frame cost, 100k-line C#, 8 lines/frame | < 8 ms p99 | **2.12 to 2.18 ms** (pass) |
  | Scroll frame cost, 20k-line Rust, 40 lines/frame | < 8 ms p99 | **2.37 to 2.46 ms** (pass) |
  | Keystroke frame cost, 100k-line C# | < 8 ms p99 | **1.93 to 2.15 ms** (pass) |
  | Keystroke frame cost, 20k-line Rust | < 8 ms p99 | **2.81 to 3.03 ms** (pass) |
  | Memory, 100k-line C# with highlighting | < 150 MB | **249 MB (FAIL)**; see section 6. The 20k-line Rust file uses 108 MB. |
  | 10 MB file, first text | < 300 ms | **152 to 164 ms** (pass); highlighting completes after 3.2 s |

- **The memory budget fails, and the cause is structural.** The tree-sitter syntax tree for the 100k-line C# file (1.0 M nodes) takes about 144 MB on its own. The viewer without any highlighting already uses 93 MB. No layout that keeps a full tree for incremental re-parsing fits in 150 MB. Section 6 gives options. This needs a decision from the human.
- **tree-sitter crates:** `tree-sitter` 0.27.0, `tree-sitter-c-sharp` 0.23.5 and `tree-sitter-rust` 0.24.2 are all MIT. `tree-sitter-language` 0.1.8 and `streaming-iterator` 0.1.9 come in transitively, under MIT and MIT OR Apache-2.0 respectively.
- **Not run:** Windows and macOS (out of scope).
- **Not done:** IME, folding and the other gaps the brief lists as out of scope; see section 8. Two doc lines outside this brief's file list are now stale; see section 9.

## 2. Vendoring

| Crate | Upstream path | SPDX (from its `Cargo.toml`) | Local changes | Transitive Zed deps (non-dev) |
|---|---|---|---|---|
| `sum_tree` | `crates/sum_tree` | Apache-2.0 | none | collections, gpui_util, zlog, ztracing, ztracing_macro |
| `rope` | `crates/rope` | GPL-3.0-or-later | none | sum_tree (vendored); collections, gpui_util, path, util, zlog, ztracing, ztracing_macro |
| `text` | `crates/text` | GPL-3.0-or-later | none | clock, rope, sum_tree (vendored); collections, gpui_util, path, util, zlog, ztracing, ztracing_macro |
| `clock` | `crates/clock` | GPL-3.0-or-later | none | none |
| `fuzzy` | `crates/fuzzy` | GPL-3.0-or-later | none | sum_tree (vendored); gpui and its Apache-2.0 support crates |

All commits are the GPUI pin `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8`. The non-vendored Zed crates stay git dependencies at the same pin, all Apache-2.0, as the 0001 audit decided.

**Packaging difference (not a code change).** Upstream's `LICENSE-GPL` and `LICENSE-APACHE` are symlinks to Zed's root license files. Here they are the dereferenced files, so each license travels with its crate. `sync.sh` diffs through the symlinks.

**Wiring, and the deviation from the brief.** The brief asked for `vendor/*` as root workspace members, with any lint exception scoped to `vendor/*` in the root manifest. Cargo cannot scope lints per member from the root manifest. A member either inherits `[workspace.lints]` (`clippy::all` and `unsafe_code`) or carries its own `[lints]` table, and adding one would be a local change to upstream manifests. Under Niello's lints the vendored code fails `-D warnings`: it uses `unsafe`, and Zed allows clippy's `style` group. Membership would also pull Zed's dev-dependencies into the root workspace: proptest from git, criterion, gpui test-support. So:

- `vendor/Cargo.toml` is a separate workspace holding the five crates. It mirrors the parts of Zed's root manifest they inherit: dependency versions and Zed's own lint table. The vendored manifests stay byte-identical to upstream.
- The root `Cargo.toml` has `exclude = ["vendor"]`. It uses the crates by path (`text = { path = "vendor/text" }`) and has `[patch."https://github.com/zed-industries/zed"] sum_tree = { path = "vendor/sum_tree" }`. GPUI depends on no other crate in the list, so `sum_tree` is the only patch.
- Upstream's tests run with `cargo test --manifest-path vendor/Cargo.toml --workspace --all-features`: 81 pass (fuzzy 9, rope 24, sum_tree 10, text 38). Clippy under Zed's lints with `-D warnings` is clean.
- `fuzzy` is vendored and tested but no Niello crate depends on it yet, so the root build has zero copies of it.

The root `Cargo.lock` gains Zed's `util` crate (Apache-2.0), which `rope` and `text` need, plus its dependencies. New third-party licenses in the lockfile: Zlib, MIT, Apache-2.0, BSD-3-Clause (`zstd-safe`, `zstd-sys`) and Unlicense OR MIT. All are GPL-3.0-compatible. `ropey` and `str_indices` are gone, and the `ropey` workspace dependency is removed.

## 3. The editor crate

Public API, documented in the `crates/editor/src/lib.rs` crate docs:

- **`Buffer`** wraps `text::Buffer`: anchors, transactions, undo and redo with 300 ms grouping, and offsets as UTF-8 bytes into `\n`-normalized text.
  - **Line endings:** the dominant ending (LF, CRLF or CR) is applied on save. Line breaks that had a different ending keep it while they exist, tracked by two anchors around each `\n`, so mixed files round-trip byte for byte. Inserted `\r\n` and `\r` are normalized.
  - **BOM:** a UTF-8 byte-order mark is stripped and restored on save.
  - **Large files:** `LARGE_FILE_THRESHOLD` is 32 MiB. Above it a file opens without highlighting.
  - **Encoding:** non-UTF-8 input is rejected (`LoadError::InvalidUtf8`).
- **`Editor`** (no GPUI) holds multiple anchored selections that merge when they overlap, with Visual Studio behavior:
  - smart Home, Ctrl+Left/Right word stops, goal column on vertical moves, page up and down;
  - Enter keeps indentation, Tab inserts to the next 4-column stop;
  - Ctrl+Backspace/Delete, and copy or cut of the whole line when nothing is selected;
  - a multi-line paste is distributed over carets;
  - add caret above and below, Shift+Alt+. for the next occurrence;
  - click, double-click (word), triple-click (line), Shift+click, Ctrl+Alt+click and drag;
  - find next and previous with wrap and case sensitivity;
  - undo and redo restore the selections of each step.
- **`EditorView`** is a GPUI view with a custom element:
  - line numbers, current-line outline, pixel-precise vertical and horizontal scrolling, tab expansion;
  - selections, carets, find bar (Ctrl+F, F3, Shift+F3, Alt+C, Esc) with match highlights;
  - text arrives through GPUI's input handler, keys through `key_bindings()` (VS defaults, context `Editor`), and the mouse through listeners;
  - `update_editor` is the entry point for programmatic edits.
- **`syntax`:**
  - **Languages as data.** `LanguageConfig` holds the id, name, suffixes, grammar function and query. `LanguageRegistry::with_builtins()` registers `CSHARP` and `RUST`, and adding a grammar is one more `register` call.
  - **Highlighter.** `Highlighter::step` re-parses incrementally: it applies tree edits from the buffer's edit list and re-highlights only edited rows and rows in `changed_ranges`. It highlights visible rows first, then 8,000 rows per step, so large files stream in. It can be cancelled.
  - **Stale highlights.** `LineHighlights` stores spans per row and `interpolate` moves them through edits in O(edited rows). The view always shows the last result, moved to the current text.
  - **Colors.** `SyntaxTheme` maps highlight kinds to colors.

**Off the UI thread.** Highlight steps run on `SyntaxThread`, one OS thread per process. The first version used GPUI's background pool. glibc's per-thread malloc arenas then fragmented: RSS grew 150 MB over 300 keystrokes in the 100k-line file, against 14 MB with `MALLOC_ARENA_MAX=1`. Keystroke frame cost p99 also fell from 6.5 ms to 2.1 ms after the change. The UI thread only interpolates spans through edits and swaps in results. Dropping old maps also happens on the syntax thread.

**Highlight queries.** These are Niello's own files in `crates/editor/queries/`:

- **C#:** adapted from tree-sitter-c-sharp 0.23.5's query (MIT). The catch-all identifier and punctuation captures are dropped, and captures are added for invocations, object creation, properties, using directives, preprocessor directives and interpolated string text.
- **Rust:** tree-sitter-rust 0.24.2's query (MIT) with a regex bug fixed (`"...+$'"` never matched) and the doc-comment patterns moved before the plain comment patterns.
- **Precedence rule:** the earlier pattern wins for the same node, and inner captures paint over outer ones.
- **Color mapping:** `SyntaxTheme::vs_dark(&niello_ui::Theme)` uses the theme's `text` token for uncolored kinds. `crates/ui` has no syntax tokens, and this brief may not edit it, so the Visual Studio Dark syntax colors live in `crates/editor` for now. They should move into `niello_ui::Theme`.

New direct dependencies: `tree-sitter` 0.27 (MIT), `tree-sitter-c-sharp` 0.23 (MIT), `tree-sitter-rust` 0.24 (MIT), and `futures` 0.3 (MIT OR Apache-2.0, already in the build through GPUI). `clock`, `text` and `niello-ui` are also direct dependencies.

Screenshots, taken in the nested KWin, show both fixtures with highlighting. They were checked visually and are not committed (outside the brief's file list).

## 4. Tests

`cargo test -p niello-editor` runs 51 tests, all passing, and they passed on 6 repeated runs.

**Unit tests (43):**
- **Buffer:** edits and undo, transactions, grouping interval, anchors through edits and undo, CRLF, mixed endings through edits, inserted CRLF, BOM, invalid UTF-8, save and load bytes, large-file threshold, lines and points, find across chunks and case.
- **Editor:** typing over selections, multi-caret insert, backspace and undo, caret merging, delete and word deletes, indentation, horizontal, vertical, Home and word movement, add carets, next occurrence, merging, clicks and drag, copy, cut and paste, find wrap, undo after an external edit.
- **Highlighting and the rest:** C# and Rust token kinds at positions (23 and 12 assertions), incremental re-parse equal to a fresh parse after an edit that opens a block comment, priority rows first, cancellation, path lookup, interpolation (3 tests), dirty ranges, theme, tab columns, syntax thread.

**Headless GPUI tests (8, `tests/view.rs`):** keyboard typing, editing, undo and redo; multi-caret from the keyboard; copy and paste; mouse click, drag, Shift+click, Ctrl+Alt+click and double-click; scroll wheel and autoscroll; find bar; highlighting off-thread, where stale highlights follow an insert before the fresh result arrives; decorations following edits.

**Workspace gate after rebasing on `main` at `53501ca`:**
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean.
- `cargo test --workspace` passes 194 tests, with 0 failed and 1 ignored. The ignored one is the crate docs' `ignore` example.

## 5. Benchmarks

**Method.** `crates/editor/tools/bench.sh` builds the release viewer and generates the inputs: 100,002-line C# (3.2 MB), 20,032-line Rust (0.5 MB) and 325,014-line C# (10.6 MB). It runs each benchmark 3 times inside the nested KWin (`spikes/0005-acp-panel/tools/nested.sh`, 60 Hz virtual output, real AMD GPU), because the session was locked (`LockedHint=yes`).

**Frame cost** is measured in-process:
- **Scroll:** from the start of the frame (GPUI's `on_next_frame` callback, where the scroll offset advances) to the end of `present`, the spike 0001 definition.
- **Typing:** the key handler (`Window::dispatch_keystroke`) plus the root render to end of `present`, with keys 15 to 45 ms apart and letters plus Backspace and Enter at the middle row.
- Neither includes the wait for the display's next refresh (PLAN.md section 9, frame-submitted row).
- Scrolling starts once highlighting is complete, so it measures highlighted text.

**Machine load.** The first full run overlapped sibling agents' builds (load average 21 to 33 on 16 threads). There, 40-line scroll p99 ranged from 2.9 to 13.5 ms. Those runs were discarded and the frame benchmarks re-run with a gate: each run waits for a 1-minute load under 3, and the load is recorded. Every number below comes from the gated runs, with load at start between 0.95 and 2.93.

| Run | Frame cost p50 | p95 | **p99** | max | Frame interval p99 |
|---|---|---|---|---|---|
| Scroll, 100k C#, 40 lines/frame (2,501 frames) | 1.81 to 1.88 ms | 2.27 to 2.29 | **2.43 to 2.47** | 2.90 to 3.00 | 17.9 ms |
| Scroll, 100k C#, 8 lines/frame (12,501 frames) | 1.54 to 1.59 | 1.99 to 2.04 | **2.12 to 2.18** | 2.52 to 2.57 | 17.9 |
| Scroll, 20k Rust, 40 lines/frame | 1.82 to 1.89 | 2.24 to 2.31 | **2.37 to 2.46** | 2.67 to 2.81 | 17.8 to 18.1 |

| Run (500 keystrokes) | Keystroke frame cost p50 | p95 | **p99** | max | Handler p99 | Key to present p99 |
|---|---|---|---|---|---|---|
| Typing, 100k C# | 1.45 to 1.47 ms | 1.63 to 1.69 | **1.93 to 2.15** | 2.67 to 3.50 | 0.49 to 1.02 | 14.0 to 14.9 |
| Typing, 20k Rust | 1.72 to 1.73 | 2.17 to 2.46 | **2.81 to 3.03** | 3.06 to 3.38 | 1.41 to 1.52 | 16.4 to 16.8 |

Key to present includes the wait for the 60 Hz frame callback, so it is reported for reference only. While typing, 324 to 495 highlight results arrived, and every frame, not only keystroke frames, had render to present p99 of at most 2.1 ms.

| Open (main to …) | First painted frame | Highlighting complete | RSS at first paint | RSS highlighted |
|---|---|---|---|---|
| 100k C# | 146 to 147 ms | 1.05 to 1.06 s | 93 MB | **249 MB** |
| 20k Rust | 144 to 147 ms | 0.22 s | 90 MB | 108 to 109 MB |
| 10.6 MB C# (325k lines) | **152 to 164 ms** | 3.16 to 3.19 s | 110 MB | 617 MB |

RSS also grows while typing: 100k C# from 249 to 265 MB, and 20k Rust from 109 to 130 MB, over 500 keystrokes. Some of that is undo history, but most is likely allocator retention from incremental parses on the syntax thread. It is not a leak that was traced; tracing it is follow-up work.

## 6. The memory budget

Measured in isolation (`malloc_trim` after each step, parse with tree-sitter 0.27):

- the 100k-line C# tree has 1,005,853 nodes and takes 144 MB live, about 144 bytes per node;
- the 20k-line Rust tree has 187,325 nodes and about 20 MB.

The viewer's own baseline (GPUI, fonts, GPU and the buffer) is 93 MB at first paint. Highlight spans for all rows add about 10 MB, so 249 MB is mostly the tree. A test that dropped the tree after highlighting saved nothing: glibc kept the freed pages, and RSS stayed at 249 MB. Incremental re-parsing, which the brief requires, needs the tree kept anyway.

Options for the human:

1. **Set the budget per line count:** 150 MB holds for the 20k-line case (108 MB), and the 100k-line case gets about 260 MB.
2. **Above a node or byte threshold, keep no tree:** parse fully on a timer after edits and highlight in windows. This trades incremental re-parse for memory, and only helps with an allocator that returns memory (`malloc_trim`, or mimalloc or jemalloc in the app binary).
3. **Parse large files in independent top-level chunks,** each with its own tree, and drop the trees of chunks that are not being edited. This needs C# and Rust split points and more design.
4. **Lower `LARGE_FILE_THRESHOLD`** so 100k-line files open as plain text. Simple, but VS users expect large files highlighted.

## 7. Hooks for the next brief

- **Decorations.** `EditorView::set_decorations(layer, Vec<Decoration>)` takes anchor ranges with one of three styles: `Background`, `Underline { wavy }` or `Foreground`. These are drawn now and covered by a test. Diagnostics map to wavy underlines, semantic tokens to foreground, and document highlights to background. Producers stamp results with `Buffer::version()` and drop stale ones (invariant 12).
- **Popups.** `pixel_position_for_offset` anchors completion and hover popups to the text.
- **Inlay hints** need text that is not in the buffer. That needs a display map, a coordinate layer between buffer and screen, which this view lacks.
- **Commands.** Editor actions are GPUI actions with stable names (`editor::MoveLeft` and so on, exported in `niello_editor::actions`). They are not yet registered on the command bus (invariant 3). That belongs to the next brief, with the document-area wiring.
- **Sizing the next brief.** Wire into the document area: open from Solution Explorer, tabs, dirty state and save. Map editor actions to command-bus commands with schemas. Feed decorations from the 0007 LSP bridge: diagnostics and semantic tokens, with generation and version checks. Add the display map for inlay hints. Estimate: one brief of the size of this one for document area, commands, diagnostics and semantic tokens. A second, smaller brief covers the display map and inlay hints, and should also settle the memory option from section 6.

## 8. Known gaps (all out of scope or noted in the crate docs)

- No IME composition; composed text is inserted as typed.
- No folding, minimap, sticky scroll, soft wrap or right-to-left text.
- Caret movement steps by characters, not grapheme clusters.
- Very long lines are shaped whole.
- Find is ASCII case-insensitive only.
- Files must be UTF-8.
- Syntax colors live in the editor crate (section 3).
- Shift+Tab (outdent) is not implemented.
- There is no caret blink.

## 9. Things outside this brief's files

- `CLAUDE.md` says the vendor sync script is "not written yet", and `README.md` labels `vendor/` "(after the audit)". Both are stale now, but neither file is in this brief's scope.
- The `docs/briefs/README.md` index row for 0009 still says "open". Only this brief's Status line was updated.

## 10. How to reproduce

```
cargo test -p niello-editor
cargo test --manifest-path vendor/Cargo.toml --workspace --all-features
vendor/sync.sh                     # or: vendor/sync.sh --from ~/.cargo/git/checkouts/zed-*/20d29fc
cargo tree -d --workspace          # no sum_tree/rope/text/clock/fuzzy/tree-sitter entries
RUNS=3 LOAD_MAX=3 crates/editor/tools/bench.sh [OUT_DIR]     # results in OUT_DIR/results.jsonl
cargo run -p niello-editor --release --example viewer -- path/to/File.cs
```
