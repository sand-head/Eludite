# Brief 0047: Keep the no-sandbox opt-in in the person's state, not the workspace's settings file

Status: in progress
Phase: 2 (follow-up to brief 0039)
Plan reference: PLAN.md sections 2 (principle 3), 4.12 (settings and state), 9; brief 0039's report section 8, item 1 (the security finding)
Related ADRs: ADR-0008
Depends on: brief 0039 (the opt-in dialog, `browser.allowNoSandbox`, the `x-eludite-scope` keyword).

## Goal

Brief 0039 stores the "Run without the sandbox for this workspace" answer in the workspace's `.eludite/settings.json`, which a team may commit. A cloned repository could then start the engine unsandboxed on a machine where the sandbox cannot start, with the strip and the audit entry as the only signs and no dialog. After this brief the answer lives in the person's own state for that workspace (the per-workspace user state that holds layouts, the search history and the git draft, keyed by the workspace path), the setting in the workspace file is ignored with a warning line in the window and the Output window, and the Options dialog edits the person's answer. A repository can never opt a machine out of the sandbox.

## Files in scope

- `protocol/schemas/settings.json` first and alone (`browser.allowNoSandbox` loses `x-eludite-scope: solution` and gains `x-eludite-scope: user-workspace`, a new scope meaning "the person's state for this workspace"; the description says a value in the workspace file is ignored), `browser-tabs.output.json` (`engine.allowed_by`: `dialog`, `options`, `variable`).
- `crates/commands/src/settings.rs` (the new scope; a value of such a key read from the workspace file is reported as ignored, never applied), `crates/eludite/src/shell/settings.rs` (`set_setting` writes a `user-workspace` key into the workspace's state folder, `settings.json` there, merged after the user file and before the solution file for these keys only), `crates/eludite/src/shell/browser.rs` and `browser_window.rs` (the dialog's Accept writes there; the warning line "browser.allowNoSandbox in .eludite/settings.json is ignored: the sandbox opt-in is per person" while the workspace file has the key), `crates/eludite/src/shell/options.rs` (the Options page shows the key under Browser with "for this workspace, on this machine" in its label), `crates/eludite/src/shell/browser_tests.rs` and `settings_tests.rs`, `docs/briefs/0039-report.md` (a one-line note pointing here), `tools/package/README.md` and `browsers/chromium/README.md` (the storage rule), `docs/briefs/README.md`, `docs/briefs/0047-report.md` (new).

## Contract

- The opt-in is read only from the person's state for the workspace; a `true` in `.eludite/settings.json` never starts the engine without the sandbox and shows the warning; `ELUDITE_CHROME_NO_SANDBOX=1` keeps working for tests and CI; the audit entry says which source allowed it.
- The state file is written with mode 0600 on Unix; the dialog, the Options page and `eludite.settings.set` are the only writers; `eludite.settings.get` reports the effective value with its source.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands`: a `user-workspace` key in the workspace file is reported ignored; the merge order for such keys.
- `crates/eludite` headless tests: Accept writes the state file (0600) and not the workspace file; a workspace file carrying `true` shows the warning and the dialog still appears; the Options page edits the state file; `settings.get` names the source; the audit entry names it; the variable still works.

## Budget

- No change to brief 0039's budgets; no new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`, `cargo test --workspace --features eludite-chromium/cef` green.
2. The report records the storage rule and the merge order; the briefs index matches.

## Out of scope

- Other settings that may deserve the same scope (named in the report, decided later); Flatpak and Snap.
