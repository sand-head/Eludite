# Brief 0011: Bring editor memory under budget

Status: open
Phase: 1
Plan reference: PLAN.md sections 2 (principle 1), 4.1, 9
Depends on: brief 0009 (report, memory section)

## Goal

Brief 0009 met every speed budget but used 249 MB for a highlighted 100,000-line C# file against a 150 MB budget, and resident memory kept growing while typing. After this brief the same file highlights within budget, memory stops growing under sustained typing, and very large files degrade gracefully instead of holding a syntax tree the size of the file.

## Files in scope

- `crates/editor/**`
- `crates/eludite/src/main.rs` (one change: the global allocator)
- root `Cargo.toml` and `Cargo.lock` (the allocator dependency only)
- `docs/briefs/0011-report.md` (new)
- `docs/PLAN.md` section 9: only if the budget is restated per file size, with the measured justification

Do not touch `crates/docking`, `crates/ui`, `crates/lsp`, `crates/workspace`, the host, or `vendor/`.

## Contract

- Adopt `mimalloc` (MIT) as the global allocator for the `eludite` binary and the editor example viewer. Record the version and SPDX id. Measure with and without it on the same file; keep it only if it helps memory or speed without regressing the other.
- Size-gate the retained syntax tree: below a documented threshold (bytes or lines, chosen from measurements) the tree is kept for incremental re-parse as today; above it, highlighting still runs but the tree is dropped after each highlight pass and re-parsed from scratch on the next edit, off the UI thread, with stale highlights shown meanwhile. Above the existing 32 MiB large-file threshold, highlighting is off as before.
- Find and fix the growth under typing (brief 0009 measured 249 to 265 MB over 500 keystrokes on the C# file). Name the cause in the report; if it is allocator retention, mimalloc should remove it, and the report shows the before and after curve.
- No regression on the brief 0009 speed numbers: scroll and keystroke frame cost p99 under 8 ms, first paint of the 10.6 MB file under 300 ms.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite-editor` green with new tests for the size gate (tree retained below, dropped above, edits still highlight) and for allocator-independent behavior.
- `crates/editor/tools/bench.sh` extended to print peak RSS and RSS after 500 keystrokes for the 100k-line C# file, the 20k-line Rust file and the 10.6 MB file, 3 runs each at machine load under 3, nested compositor if the session is locked.
- Workspace fmt, clippy with warnings denied, tests green.

## Budget

- 100k-line C# file, highlighted, idle: under 150 MB RSS.
- Growth over 500 keystrokes on that file: under 10 MB.
- 10.6 MB file after highlighting completes: report the number; propose a per-size budget line for PLAN.md section 9 if 150 MB is not attainable without losing highlighting.

## Exit criterion

1. Numbers for the three files, before and after, in the report.
2. The 100k-line file meets 150 MB, or the report states exactly why not and proposes the per-size budget with evidence.
3. Speed numbers unchanged within 10 percent.

## Out of scope

- Any editor feature work. No folding, IME, soft wrap.
- Changing the vendored crates.
- Windows and macOS runs.
