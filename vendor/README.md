# vendor/

Pinned copies of third-party crates, per PLAN.md D1 and section 13 risk 5.

Rules:
- Each vendored crate lives in its own directory with a `WHY.md` naming the upstream repository, the pinned commit, the license, and every local change.
- Only low-level, non-visual crates are candidates (text buffer, rope, fuzzy matching). Nothing that decides what a user sees.
- Re-sync happens deliberately, with a GPUI bump, through `sync.sh`; never by hand.

## Contents

From zed-industries/zed at `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8`, the GPUI pin (brief 0009, from the brief 0001 audit):

| Crate | SPDX | Local changes |
|---|---|---|
| `sum_tree` | Apache-2.0 | none |
| `rope` | GPL-3.0-or-later | none |
| `text` | GPL-3.0-or-later | none |
| `clock` | GPL-3.0-or-later | none |
| `fuzzy` | GPL-3.0-or-later | none |

No extra Zed support crate had to be vendored: `collections`, `util`, `gpui_util`, `path`, `zlog` and `ztracing` (all Apache-2.0) stay git dependencies at the same pin, as the audit decided.

## How it is wired

- `vendor/Cargo.toml` is a separate workspace holding these five crates. It mirrors the parts of Zed's root manifest they inherit (`*.workspace = true`, including Zed's lint table), so the vendored manifests stay identical to upstream and Niello's stricter workspace lints do not apply to code we do not own. The root workspace has `exclude = ["vendor"]`.
- The root `Cargo.toml` uses the crates by path (`text = { path = "vendor/text" }`) and has `[patch."https://github.com/zed-industries/zed"] sum_tree = { path = "vendor/sum_tree" }`, so GPUI's own `sum_tree` resolves to the vendored copy. `cargo tree -d` shows no duplicate of any vendored crate. GPUI depends on no other crate in this list.
- Upstream's tests: `cargo test --manifest-path vendor/Cargo.toml --workspace --all-features`.
- Drift check: `vendor/sync.sh` (fetches the pin from GitHub) or `vendor/sync.sh --from <zed checkout>` (offline). Exit 0 means no drift.
