# sum_tree

| | |
|---|---|
| Upstream | https://github.com/zed-industries/zed, path `crates/sum_tree` |
| Commit | `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8` (the GPUI pin in the root `Cargo.toml`) |
| SPDX license | `Apache-2.0`, from `license` in this crate's own `Cargo.toml`; upstream text in `LICENSE-APACHE` |
| Transitive Zed dependencies (non-dev) | `ztracing`, `ztracing_macro`, `zlog`, `collections`, `gpui_util` (all Apache-2.0, by git at the pin) |
| Brief | docs/briefs/0009-editor-core.md; audit in docs/briefs/0001-report.md sections 6 and 7 |

## Why vendored

The B+tree with summaries under `rope`, `text` and GPUI's own lists. Vendored so the editor core does not move when GPUI is bumped, and patched into GPUI (root `[patch."https://github.com/zed-industries/zed"]`) so the build has exactly one `sum_tree`.

## Local changes

None. Every file except this `WHY.md` is byte-identical to upstream at the commit above.

Packaging only, not a code change: upstream's `LICENSE-APACHE` is a symlink to Zed's root license file (`../../LICENSE-APACHE`); here it is the dereferenced file, so the license travels with the crate. `vendor/sync.sh` compares through the symlink.

The crate's `*.workspace = true` keys and `[lints] workspace = true` resolve against `vendor/Cargo.toml`, which mirrors the relevant parts of Zed's root manifest at the pin, so the manifest needs no edits.

## Sync

Only with a deliberate GPUI bump (an ADR note per CLAUDE.md). Run `vendor/sync.sh` to diff against upstream at the pin, or `vendor/sync.sh --rev <new-rev>` to see what a bump would change, then copy and update this file.
