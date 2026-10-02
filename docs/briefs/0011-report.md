# Brief 0011 report: bring editor memory under budget

Date: 2026-10-02. Brief: [0011-editor-memory.md](0011-editor-memory.md). Code: [`crates/editor/src/syntax/alloc.rs`](../../crates/editor/src/syntax/alloc.rs), [`crates/editor/src/syntax/highlighter.rs`](../../crates/editor/src/syntax/highlighter.rs). Benchmark driver: [`crates/editor/tools/bench.sh`](../../crates/editor/tools/bench.sh).

## 1. Summary

- **The 100k-line C# file now uses 98 MB highlighted and idle** (was 241 MB), against a 150 MB budget. Typing 500 keystrokes adds 8.3 to 8.5 MB (was 15 to 17 MB), against a 10 MB budget. Both pass.
- **The 10.6 MB file uses 148 MB idle once highlighting completes** (was 609 MB). It is under 150 MB, so no per-size budget line is proposed and PLAN.md is unchanged. Peak RSS while a highlight pass holds the full tree is still about 604 MB (section 5).
- **Speed is unchanged within 10 percent.** Scroll and keystroke frame cost p99 stay between 1.7 and 3.0 ms, and the 10.6 MB file paints first text in 152 to 156 ms.
- **Allocator decision.** mimalloc is used for tree-sitter's allocations only, on a heap of its own. It is **not** the global allocator. Measured as the global allocator, it cost 18 to 24 MB more at idle in the viewer and 40 MB more in the `eludite` shell, and gave no speed gain. The brief says to keep it only if it helps without regressing the other measure, so the system allocator stays. `crates/eludite/src/main.rs` is unchanged (section 3).
- **Size gate:** `TREE_RETAIN_LIMIT` is 1 MiB. Below it the tree is kept for incremental re-parsing. Above it the tree is dropped when a highlight pass completes, and the next edit re-parses from scratch on the syntax thread while stale highlights stay on screen.
- **Cause of the growth while typing:** heap fragmentation. Every incremental re-parse frees thousands of tree nodes and allocates new ones. Those nodes were interleaved, in the same allocator pages, with long-lived objects made on the same thread, mainly each row's highlight spans. So the allocator could not return the freed pages. It was not a leak, and it was not plain allocator caching (section 4).
- **Tests:** `cargo test -p eludite-editor` runs 58 tests, up from 51, all passing on 6 consecutive runs. After rebasing on `main` at `b33328f`, the workspace passes 228 tests, with 0 failed and 1 ignored. fmt is clean, and clippy is clean with `-D warnings`.
- **Not run:** Windows and macOS (out of scope).

## 2. What changed

**`syntax::alloc` (new).**
- tree-sitter's C core is pointed at mimalloc through `tree_sitter::set_allocator`, on a dedicated `mi_heap_new()` heap. mimalloc 3 heaps allocate from any thread.
- `release_free_memory()` calls `mi_heap_collect(heap, true)` and then `mi_collect(true)`. The highlighter calls it on the syntax thread whenever a highlight pass completes. It took 0.02 to 16 ms in the measurements: 6 ms after dropping the 100k-line tree, 16 ms after the 10.6 MB one.
- `live_bytes()` counts tree-sitter's live bytes using mimalloc's usable size. It measures tree memory without depending on RSS or on the binary's global allocator.
- `install()` runs once per process from `Language::new` and `Highlighter::new`, before any tree-sitter object exists, as tree-sitter requires. No other crate uses tree-sitter.
- On Linux, `install()` also calls `prctl(PR_SET_THP_DISABLE)` for the process. This is what mimalloc's own `MIMALLOC_ALLOW_THP=0` does. With the kernel's THP mode set to `always` (this machine, CachyOS), mimalloc's arenas otherwise fault in as 2 MiB huge pages. Without the call, idle RSS was 112 MB instead of 98 MB for the 100k-line file, and 162 MB instead of 148 MB for the 10.6 MB file (3 runs each). Under the more common `madvise` mode the call changes nothing.
- Since brief 0012, the shell builds its `LanguageRegistry` at startup, so this now applies to the `eludite` process too. Measured cold start against `main`, 5 runs each: first present in 149 to 159 ms against 146 to 157 ms, and peak RSS of 87.1 to 87.9 MB against 83.8 to 84.8 MB.
- The THP call is a process-wide setting made from a library crate. If the human prefers to make it in the binary's `main`, it can move there; this brief may change only the allocator line in `main.rs`.

**Size gate in `Highlighter`.**
- `TREE_RETAIN_LIMIT` is 1 MiB, adjustable with `Highlighter::set_retain_limit` and `EditorView::set_syntax_tree_limit`. `has_tree()` reports whether a tree is held.
- `HighlightStats` gains `dropped_tree` and `release`.
- When a pass completes on a buffer over the limit, the tree is dropped.
- The next step after an edit has no tree. It moves the old spans through the edits, marks every row dirty but keeps its spans, and parses from scratch. Visible rows are redone first, then 8,000 rows per step.
- Edits that arrive while a pass is still running re-parse incrementally from that pass's tree. A burst of typing therefore pays for only one full parse, and the tree is dropped once the burst's highlighting completes.
- A completed pass on an unchanged version without a tree returns at once, with no re-parse.
- Above the existing 32 MiB `LARGE_FILE_THRESHOLD`, highlighting stays off as before.

**Benchmarks.**
- `bench.sh` adds a typing run on the 10.6 MB file.
- `VIEWER=` runs a prebuilt binary and `TAG=` labels results. The script prints a memory table per file: idle highlighted, peak RSS at open, settled RSS after 500 keystrokes, and peak RSS while typing.
- The viewer's open run adds RSS measured 2 s after highlighting completes ("idle").
- The viewer's type run adds:
  - RSS every 50 keystrokes;
  - RSS 300 ms after the last keystroke;
  - "settled" RSS: highlighting caught up, then 2 s idle;
  - peak RSS (VmHWM);
  - tree-sitter's live MiB.
- New viewer flags:
  - `--tree-limit BYTES`;
  - `--measure-tree`, which needs no window and prints tree memory and the cost of an edit with the tree kept and dropped;
  - `--features mimalloc-global`, which builds the viewer with mimalloc as the global allocator for comparison.

**Dependencies.** Both are MIT, with SPDX id `MIT`:
- `libmimalloc-sys` 0.1.49 with the `extended` feature, a dependency of `eludite-editor`. It bundles mimalloc C 3.3.2 (`MI_MALLOC_VERSION 30302`; v3 is the crate default).
- `mimalloc` 0.1.52, a dev-dependency of `eludite-editor` for the viewer's comparison build only.

Both are declared in the root `[workspace.dependencies]`.

## 3. Allocator: with and without

All runs are in the nested KWin at 60 Hz. Each benchmark ran 3 times, and every run waited for a 1-minute load under 3. RSS is in MiB. "Growth" is settled RSS after 500 keystrokes minus RSS just before typing.

| Configuration | 100k C# idle | 100k growth | 20k Rust idle | 20k growth | 10.6 MB idle | 10.6 MB growth |
|---|---|---|---|---|---|---|
| Before (0009 code, system allocator) | 241.4 to 241.7 | 15.0 to 17.1 | 100.9 to 101.3 | 20.8 to 21.2 | 608.7 to 609.0 | 34.3 to 42.3 |
| 0009 code, mimalloc global | 293.8 to 295.3 | -3.9 to 4.8 | 155.6 to 157.7 | 4.1 to 7.6 | 663.5 to 669.5 | 24.1 to 26.0 |
| This brief, mimalloc global | 116.1 to 120.1 | 1.1 to 3.5 | 114.3 to 116.0 | 8.4 | 171.8 to 175.1 | -5.8 to -4.9 |
| **This brief, system global (shipped)** | **97.8 to 98.1** | **8.3 to 8.5** | **100.9 to 101.1** | **11.5 to 11.9** | **148.0 to 148.2** | **9.2 to 9.5** |

**RSS at first paint** (empty highlights):

| File | system allocator | mimalloc global |
|---|---|---|
| 100k C# | 86 to 87 MB | 108 to 110 MB |
| 20k Rust | 84 to 85 MB | 100 MB |
| 10.6 MB | 103 MB | 135 to 139 MB |

**The `eludite` shell, `--bench-start`, 3 runs each:**

| Allocator | Peak RSS | First present |
|---|---|---|
| system | 73 MB | 98 to 104 ms |
| mimalloc global | 113 to 117 MB | 101 to 103 ms |
| mimalloc global with `MIMALLOC_ALLOW_THP=0` | 77 MB | |

The `MIMALLOC_ALLOW_THP=0` setting cannot be applied from the allocator line alone.

**Speed:** no gain from mimalloc. Keystroke p99 on the 100k file was 2.43 to 2.58 ms with mimalloc global and 1.73 to 2.21 ms with the system allocator. Scroll p99 was equal within 0.05 ms.

**Decision:** keep the system global allocator. mimalloc helps with typing growth but regresses idle memory everywhere, and growth with the system allocator is already under budget. The brief's rule ("keep it only if it helps memory or speed without regressing the other") therefore excludes it as the global allocator. It stays for tree-sitter, where it is what lets a dropped tree's memory return to the OS.

## 4. The growth while typing

**Before:** the 100k-line file grew from 241 MB to 257 to 259 MB over 500 keystrokes, with the 0009 code on glibc.

**Diagnosis.** A diagnostic build wrapped the global allocator in a byte counter (not committed; run with no load gate, memory only):
- Live Rust heap grew from 21.95 MB to 24.87 MB over the 500 keystrokes. That growth is undo history and highlight rows.
- tree-sitter's live bytes stayed flat.
- RSS grew far more than both, so the extra memory was freed but not returned.

**Cause.** The syntax thread allocates the re-parsed tree's nodes and each row's `Arc<[Span]>` from the same heap pages. A page whose tree nodes were all freed still holds a few live span slices, so neither glibc nor mimalloc can release it.
- With mimalloc as the global allocator and tree-sitter on the default heap, growth was 35 MB (single unloaded runs).
- Moving tree-sitter to its own heap cut that to 0.5 MB.

**Fix.** tree-sitter has its own mimalloc heap, so tree pages never hold spans. The heap is collected after every completed pass. Above the limit, the whole tree is dropped at that point.

**After.** 8.3 to 8.5 MB over 500 keystrokes on the 100k file. RSS every 50 keystrokes, run 1 of each configuration:

| Keystrokes | 0 | 50 | 100 to 350 | 400 | 450 | settled |
|---|---|---|---|---|---|---|
| before (system) | 241 | 257 | 257 | 258 | 258 | 258 |
| after (system, shipped) | 98 | 252 | 252 | 255 | 255 | 107 |

**While a typing burst runs**, the tree for the current pass is alive (about 140 MB), as it was before; RSS during typing peaks at 255 MB. After the burst, highlighting completes, the tree is dropped and RSS returns to within 9 MB of its idle level.

**What remains.** Most of the remaining 8 MB is the glibc arena retaining the highlighter's freed Rust buffers; about 3 MB is live growth. The 20k Rust file keeps its tree, which is under the limit. It grows 11.5 to 11.9 MB, against 21 MB before. The brief sets no budget for that file.

## 5. The size gate and its threshold

`--measure-tree` measurements, without a window, on the system allocator. Load was 2.3 to 7.4: the parse is single-threaded, and the machine has 16 threads.

| File | Bytes | Tree (MiB) | Bytes of tree per byte | Initial parse | Edit, tree kept | Edit, tree dropped: parse / visible rows / all rows |
|---|---|---|---|---|---|---|
| 20k Rust | 0.50 MB | 17.8 | 37.1 | 41 ms | 6 ms | 41 / 57 / 86 ms |
| 32k C# | 1.03 MB | 45.0 | 45.6 | 222 ms | 14 ms | 222 / 235 / 297 ms |
| 100k C# | 3.25 MB | 140.6 | 45.4 | 693 ms | 47 ms | 693 / 707 / 941 ms |
| 10.6 MB C# | 10.57 MB | 456.9 | 45.3 | 2,242 ms | 159 ms | 2,237 / 2,253 / 3,122 ms |

**Threshold: 1 MiB.** The viewer with a C# file just under the limit (32,001 lines, 1,034,360 bytes, tree kept) used:
- 137.0 to 137.4 MB idle;
- 142.5 to 142.8 MB after 500 keystrokes (3 runs each).

That is inside 150 MB with about 7 MB to spare. A file just over the limit (33,021 lines, 1,067,359 bytes, tree dropped) used:
- 92.0 to 92.5 MB idle;
- 96.1 to 96.3 MB after typing.

Twice the limit would keep about 90 MB of tree and could not fit. The 0.5 MB Rust file stays incremental: 6 ms per edit.

**Cost above the limit.** The first edit after a completed pass waits for a full parse before visible rows get fresh colors: about 0.22 s at 1 MiB, 0.7 s at 3.2 MB and 2.2 s at 10.6 MB. The UI thread is not involved, and until then the old colors are shown, moved with the text. Keystroke frame cost is unaffected (section 6).

**Peak memory.** A highlight pass still builds the whole tree, so peak RSS is unchanged in kind:
- 237 MB at open for the 100k file, and 255 MB while typing;
- 604 MB at open for the 10.6 MB file, and 644 MB while typing.

PLAN.md section 9 budgets the whole shell (400 MB for a 100-project solution with 20 tabs), and this brief's budget is for idle memory. Cutting the peak needs per-region parsing, option 3 in the 0009 report. It is not proposed here.

## 6. Speed against brief 0009

The shipped configuration ran 3 times per benchmark at load under 3.

| Benchmark | Brief 0009 | Before (rerun) | After | Budget |
|---|---|---|---|---|
| Scroll p99, 100k C#, 40 lines/frame | 2.43 to 2.47 ms | 2.46 to 2.48* | 2.48 to 2.51 | < 8 ms |
| Scroll p99, 100k C#, 8 lines/frame | 2.12 to 2.18 | 2.16 to 2.43* | 2.14 to 2.17 | < 8 ms |
| Scroll p99, 20k Rust, 40 lines/frame | 2.37 to 2.46 | 2.37 to 2.43 | 2.35 to 2.43 | < 8 ms |
| Keystroke p99, 100k C# | 1.93 to 2.15 | 1.85 to 2.10 | 1.73 to 2.21 | < 8 ms |
| Keystroke p99, 20k Rust | 2.81 to 3.03 | 3.00 to 3.08 | 2.96 to 3.02 | < 8 ms |
| Keystroke p99, 10.6 MB C# | not run | 1.82 | 1.72 to 1.73 | < 8 ms |
| First paint, 10.6 MB | 152 to 164 ms | 154 to 161 | 152 to 156 | < 300 ms |
| First paint, 100k C# | 146 to 147 ms | 149 to 178 | 150 to 179 | |
| Highlighting complete, 100k C# | 1.05 to 1.06 s | 1.05 to 1.07 | 1.08 to 1.11 | |
| Highlighting complete, 10.6 MB | 3.16 to 3.19 s | 3.12 to 3.15 | 3.28 to 3.28 | |

*Loaded runs. The baseline's first 40-line scroll run on the 100k file measured 6.86 ms p99 at load 2.5 to 2.7, when a sibling agent's build started. Its third 8-line run ended at load 9.6. Both are excluded from the table. In the table, the 8-line before figure is 2 runs.

**Other runs.** In every configuration, the first `open-100k-cs` run paints first text in 178 to 182 ms against about 147 ms for the later two. This is a cold file cache on the first open, so it is shown but not counted as a regression.

**Within 10 percent.** Every frame-cost figure is within 10 percent of brief 0009. Highlighting completes 3 to 5 percent later, which is the cost of counting tree-sitter blocks, mimalloc's heap and the final collect.

**Intermediate configuration.** Three runs of "0009 code, mimalloc global" ended at load 3 or above and are excluded. The configuration was not rerun because no decision depends on it.

## 7. Tests

`cargo test -p eludite-editor` runs 58 tests: 48 unit tests, 9 headless GPUI tests and 1 integration test. That is 7 more than before:
- `buffers_under_the_limit_keep_their_tree`: the tree is retained, and the re-parse is incremental.
- `buffers_over_the_limit_drop_the_tree_and_still_highlight_edits`: the tree is dropped after the pass. A block comment opened over several lines is highlighted after a full re-parse, and the result equals a fresh parse with the tree kept.
- `a_completed_pass_without_a_tree_is_not_reparsed`
- `over_the_limit_stale_spans_stay_until_rows_are_redone`: mid-pass edits re-parse incrementally. After a drop, the visible row is redone first, and far rows keep their old spans, dirty, until a later step.
- `alloc::tests::blocks_round_trip`: tree-sitter's malloc, calloc, realloc and free on the mimalloc heap, with alignment, zeroing and contents.
- `tests/view.rs::highlighting_over_the_tree_limit_follows_edits`: the view with limit 0 highlights two consecutive edits correctly, the first from a kept tree and the second from scratch.
- `tests/tree_memory.rs::dropping_the_tree_returns_its_memory`: this is the allocator-independent check. It uses tree-sitter's live bytes, not RSS. A kept tree is more than 10 times the text size, and after a gated pass and after an edit, less than 5 percent of it remains. It is alone in its test binary so no other test's trees are counted.

**Workspace gate** after rebasing on `main` at `b33328f`:
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean;
- clippy is also clean with `--features mimalloc-global`;
- `cargo test --workspace` passes 228 tests, with 0 failed and 1 ignored (the crate docs' `ignore` example).

## 8. Deviations and things outside this brief's files

- **`crates/eludite/src/main.rs` is unchanged.** The brief allowed one change there for the allocator, and the measurement says not to make it. Adopting mimalloc there would also have needed an edit to `crates/eludite/Cargo.toml`, which is not in this brief's file list.
- **The shell now disables THP** through the editor crate (section 2), which takes effect since brief 0012 wired the editor into the shell. The human may prefer that call in the binary's `main`.
- **The `docs/briefs/README.md` index row for 0011 still says "open".** Only this brief's Status line was updated.
- **The bench results live outside the repo**, in the scratchpad output directory. `bench.sh` reproduces them. The `--measure-tree` threshold runs and the shell start runs used the commands in section 9.

## 9. How to reproduce

```
cargo test -p eludite-editor
RUNS=3 LOAD_MAX=3 crates/editor/tools/bench.sh [OUT_DIR]          # shipped config; prints the memory table
cargo build --release -p eludite-editor --example viewer --features mimalloc-global
RUNS=3 LOAD_MAX=3 VIEWER=target/release/examples/viewer TAG=mimalloc crates/editor/tools/bench.sh [OUT_DIR]
target/release/examples/viewer FILE --measure-tree                 # tree memory and edit cost, kept vs dropped
target/release/examples/viewer --generate csharp 32000 Under1MiB.cs   # threshold check, with --bench-open / --bench-type 500
```
