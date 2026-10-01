# clock

| | |
|---|---|
| Upstream | https://github.com/zed-industries/zed, path `crates/clock` |
| Commit | `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8` (the GPUI pin in the root `Cargo.toml`) |
| SPDX license | `GPL-3.0-or-later`, from `license` in this crate's own `Cargo.toml`; upstream text in `LICENSE-GPL` |
| Transitive Zed dependencies (non-dev) | none |
| Brief | docs/briefs/0009-editor-core.md; audit in docs/briefs/0001-report.md sections 6 and 7 |

## Why vendored

Lamport and global clocks that `text` anchors and undo history need.

## Local changes

None. Every file except this `WHY.md` is byte-identical to upstream at the commit above.

Packaging only, not a code change: upstream's `LICENSE-GPL` is a symlink to Zed's root license file (`../../LICENSE-GPL`); here it is the dereferenced file, so the license travels with the crate. `vendor/sync.sh` compares through the symlink.

The crate's `*.workspace = true` keys and `[lints] workspace = true` resolve against `vendor/Cargo.toml`, which mirrors the relevant parts of Zed's root manifest at the pin, so the manifest needs no edits.

## Sync

Only with a deliberate GPUI bump (an ADR note per CLAUDE.md). Run `vendor/sync.sh` to diff against upstream at the pin, or `vendor/sync.sh --rev <new-rev>` to see what a bump would change, then copy and update this file.
