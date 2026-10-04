# Brief 0047 report: Keep the no-sandbox opt-in in the person's state, not the workspace's settings file

Status: done (Linux, headless tests under Xvfb, run as root in a container). CI: not run (nothing pushed).
Branch: `brief/0047-sandbox-opt-in-user-state`, based on `main` at `916396a`; not rebased (the coordinator merges).
Date: 2026-10-04.
Brief: [0047-sandbox-opt-in-user-state.md](0047-sandbox-opt-in-user-state.md). Resolves brief 0039's report section
10, item 1 (the brief calls it section 8, item 1).

## 1. Summary

- **A repository can no longer opt a machine out of Chromium's sandbox.** `browser.allowNoSandbox` has the new
  settings scope `user-workspace` (`x-eludite-scope`): it is read from the person's own state for the open workspace,
  then the user file, then its default, and never from the workspace's `.eludite/settings.json`. A value there is
  ignored, reported by `eludite.settings.get` (`ignored_keys`), and announced by the line "browser.allowNoSandbox in
  .eludite/settings.json is ignored: the sandbox opt-in is per person" in the Web Browser window (a strip, while the
  file carries the key) and the Output window's Browser pane (each time the key appears). The engine then starts
  without `--allow-no-sandbox`, is refused, and the opt-in dialog comes as on any machine.
- **The dialog, Tools > Options and `eludite.settings.set` write the person's state**, mode 0600 (its folder 0700) on
  Unix. The Options page shows the setting under Web Browser as "Run without Chromium's sandbox (for this workspace, on
  this machine)", with a note saying the answer is kept in Eludite's state for the workspace and, when the workspace
  file carries the key, that it is ignored there.
- **`eludite.settings.get` names the source** `user-workspace` and the file (`user_workspace_file`); `set` writes a
  setting in its own scope when none is given and refuses to write the opt-in in the workspace's file (removing it
  from there, a null value, is allowed).
- **What allowed an unsandboxed start** is in the audit entry (`eludite.browser.engine_start`, `allowed_by`) and in
  `eludite.browser.tabs`' `engine.allowed_by`: `dialog`, `options` or `variable`. `ELUDITE_CHROME_NO_SANDBOX=1` keeps
  working for tests and CI.

## 2. What was built (commits)

| Commit | What |
|---|---|
| `cf98331` | The brief's status: in progress |
| `415e154` | Schemas first and alone: `settings.json` (`browser.allowNoSandbox` with `x-eludite-scope: user-workspace`, its description and label; the file's description documents the scope, the state file and the merge rule), `settings-get.output.json` (`user-workspace` source, `user_workspace_file`, `ignored_keys`), `settings-set.input.json` and `.output.json` (`user-workspace` scope and source; an omitted scope is the setting's own), `browser-tabs.output.json` (`engine.allowed_by`: `dialog`, `options`, `variable`) |
| `e221d6b` | `crates/commands`: `SettingScope::UserWorkspace` and `SettingSource::UserWorkspace` (spelled `user-workspace`), `SettingSpec::resolve` (the precedence, in one place), `SettingLayers`, `SettingsSchema::ignored_in_solution`, the get output's new members, `set`'s scope rule; `EngineRow::allowed_by` |
| `f9fea2a` | The store (`crates/eludite/src/settings.rs`): the third layer, `workspace_state_dir`, `SettingsSetup::state_dir`, the private write (`FileWrite`, 0600 and 0700), `ignored_keys` |
| `fbbf84a` | The shell: the opt-in read from the store as before (now the person's value), the warning strip and Output line, the dialog's Accept writing scope `user-workspace`, `allowed_by` recorded by the browser worker before each command that may launch the engine and given to `tabs` and the audit entry, the Options page's notes; the tests |
| `2034124` | `tools/package/README.md` and `README.in`, `browsers/chromium/README.md`, `tools/cef/README.md` (the storage rule), the note in `0039-report.md` |
| `6f9b61f` | The Options test waits for the dialog's note instead of the store (a race found under load) |
| (this commit) | This report, the brief's status, the index row |

## 3. The storage rule and the merge order

**Where.** `<config dir>/eludite/workspaces/<folder name>-<16 hex digits>/settings.json`, where `<config dir>/eludite`
is the folder of the user settings file and the layouts (`$XDG_CONFIG_HOME/eludite`, default `~/.config/eludite`, on
Linux; `%APPDATA%\eludite` on Windows; `~/Library/Application Support/eludite` on macOS; `ELUDITE_CONFIG_DIR` replaces
it), the folder name is the workspace folder's name (characters other than letters, digits, `-`, `_` and `.` become
`_`), and the hex digits are the FNV-1a 64-bit hash of the folder's absolute path, the same hash that names the
workspace's layout file (taken from `eludite_docking::persist::LayoutStore::solution_path`, so the rule is not
duplicated). The workspace is the one the solution file layer already follows (`Shell::workspace_root`: the open
solution's folder, or the open folder). Today's other per-workspace state lives in the same folder tree: the layouts
in `layouts/solutions/`, the Find in Files history in `search/history.json` and the commit message drafts in
`git/drafts.json` (both keyed by path inside one file). There was no per-workspace folder yet, so `workspaces/` is
new; it is where further per-person workspace settings go.

**Mode.** On Unix the file is written through a temporary file created with mode 0600 and renamed over it, so it never
has another mode; its folder is created 0700. Elsewhere the file has the user profile's default access (`%APPDATA%` is
the user's).

**Writers.** The Web Browser window's dialog (OK with the box checked), the Options page and `eludite.settings.set`
(scope `user-workspace`, the default for this setting); nothing else writes it. The file is read by the settings
thread's poll like the others, so an edit by hand applies live too.

**Merge order.** For every setting except the `user-workspace` ones, as before: the environment variable, then the
solution's file, then the user file, then the default. For a `user-workspace` setting: the environment variable (it
has none today), then the person's state for the workspace, then the user file, then the default; the solution's file
is not read for it, and a value there is listed in `ignored_keys`. No other setting is read from the person's
workspace state. (`ELUDITE_CHROME_NO_SANDBOX=1` is not a settings variable: the shell reads it on its own, as in brief
0039.)

**With no workspace open** there is no state file; the dialog's OK opts in for the session only, as in brief 0039, and
`set` with scope `user-workspace` fails naming why.

## 4. Tests

| Where | What it proves |
|---|---|
| `crates/commands/src/settings.rs` `a_user_workspace_setting_ignores_the_solution_file_and_merges_after_the_user_file` | The merge order for such keys: the workspace file alone gives the default; the user file under the person's state; the state wins over the user file either way; a value of the wrong type counts as absent; other settings keep solution over user and never read the state; the environment wins; `ignored_in_solution` lists only the per-person keys, sorted, once |
| `crates/commands/src/settings.rs` `set_writes_a_user_workspace_setting_only_in_the_persons_state` | `set` without a scope writes the opt-in in the person's state; scope `solution` with a value is refused naming why (null allowed); scope `user` allowed; no other key may go in the person's state |
| `crates/commands/src/settings.rs` (changed) | The schema gives the opt-in scope `user-workspace` and a label with "for this workspace, on this machine"; the new scope and source spell as the schemas' enums; `allowed_by` in a `tabs` sample validates |
| `crates/eludite/src/settings.rs` `the_persons_workspace_state_holds_the_opt_in_and_the_workspace_file_cannot` | The store: the workspace file's `true` gives the default and is reported ignored; the state file's path (name and hash, two folders of one name differ); a set writes only the state, mode 0600 and folder 0700; a new store (the next session) reads it back over a user file's `false`; another workspace has its own state; no workspace, no write |
| `crates/eludite/src/shell/browser_window_tests.rs` `taking_the_opt_in_stores_it_restarts_audits_and_shows_the_strip` (changed) | Accept writes the person's state file (mode 0600), not the workspace's file (which is never created); `settings.get` names the source `user-workspace`; the audit entry and `tabs` say `allowed_by: dialog`; the setting off again (no scope) removes the opt-in from the next start |
| `browser_window_tests.rs` `a_workspace_file_carrying_the_opt_in_is_ignored_with_a_warning_and_the_dialog_still_appears` | A `true` in `.eludite/settings.json` is ignored: the warning strip and the Output line (the brief's exact text), `ignored_keys`, the default as the value, the engine started without the opt-in and refused, the dialog shown; Accept writes the state and leaves the workspace file byte-for-byte; the warning stays while the key does and goes when it is removed (null with scope `solution`); a value with scope `solution` is refused |
| `browser_window_tests.rs` `a_stored_answer_starts_the_engine_without_a_dialog_and_the_audit_names_options` | The person's stored answer (an earlier session's state file) starts the engine with the opt-in and no dialog; `allowed_by: options` in the audit entry and `tabs` |
| `browser_window_tests.rs` `the_variable_still_allows_it_and_the_audit_names_it` | `ELUDITE_CHROME_NO_SANDBOX=1` still starts it without the sandbox, no dialog, nothing stored; `allowed_by: variable` |
| `crates/eludite/src/shell/settings_tests.rs` `the_options_dialog_writes_the_opt_in_in_the_persons_workspace_state` (replaces brief 0039's workspace-file test) | The key is on the Web Browser page with "for this workspace, on this machine" in its label; the Options page writes the state file (0600), never the workspace's or the user's file; `settings.get` names the source and the file; the page's note says where the answer is kept; off writes the state again; a workspace file carrying the key makes the page say it is ignored, and the person's answer stays |

The fake engine of the window tests now reports `sandbox: none` in its launch information when it runs without the
sandbox, as `engine/ready` does, so `tabs` carries `allowed_by` there.

**The full run** (`cargo test --workspace --no-fail-fast --features eludite-chromium/cef` under Xvfb, with
`ELUDITE_DBG_MONO`, the test corpus built, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`, Node 22, `ELUDITE_JS_DEBUG`
and `ELUDITE_TEST_SSHD`, after `dotnet build dotnet/Eludite.slnx` with 0 warnings): 1052 passed, 1 failed, 1 ignored.
The failure was `a_thousand_changed_files_draw_in_a_frame` (8.6 ms against its 8 ms frame budget while another
worktree built), which passes alone. In earlier full runs, `exception_types_go_as_filter_options_and_the_window_shows_the_tree`
(`shell/debug/tests.rs`) failed twice under load and passes alone: it reads the adapter's last
`setExceptionBreakpoints` right after a click without waiting for it, a race in that test unrelated to this brief. One
earlier run also failed the new Options test, which read the dialog's note as soon as the store had the change, before
the shell had applied it; it now waits for the note (`6f9b61f`). `cargo fmt --check` and `cargo clippy --workspace
--all-targets --features eludite-chromium/cef -- -D warnings` are clean.

## 5. Budget

- No change to brief 0039's budgets: nothing new runs at startup (the state file is read by the settings thread's poll
  with the solution file, one more `stat` a tick while a workspace is open); the dialog's path is unchanged.
- No new dependency.

## 6. Deviations and decisions

1. **Files outside the brief's list.** `protocol/schemas/settings-get.output.json`, `settings-set.input.json` and
   `settings-set.output.json` (the contract's "`settings.get` reports the source" and the new scope need their enums;
   `ignored_keys` is the report the brief asks for); `crates/commands/src/browser.rs` (`EngineRow::allowed_by`, for the
   brief's `engine.allowed_by`); `crates/eludite/src/settings.rs` (the store, where the layers and the writes are; the
   brief names `shell/settings.rs`, which only applies them); `crates/eludite/src/shell/browser_window_tests.rs` (the
   dialog's tests are there, not in `browser_tests.rs`, which is unchanged); `tools/package/README.in` (the packaged
   README said the opt-in was in the workspace's file) and `tools/cef/README.md` (the same sentence).
2. **The user file still counts, below the person's workspace state**, as the brief's merge order says ("after the user
   file and before the solution file"). The contract's "read only from the person's state" is read as "only from the
   person's own files, never the workspace's": the user file is the person's too, and a `true` there opts every
   workspace in. Nothing writes the opt-in there unless asked (`set` with scope `user`).
3. **`allowed_by: dialog`** means the opt-in taken in the dialog this session for this workspace; a start allowed by the
   stored answer (from Tools > Options, `set`, or the dialog in an earlier session) is `options`. The file stores only
   the value, so who wrote it is not known later; the schema's description says so. The worker records what allowed
   the launch before each command that may launch the engine, so the audit entry and `tabs` give the launch's reason
   even when the setting changes while the engine runs.
4. **`set` without a scope** now uses the setting's own scope (`x-eludite-scope`) instead of always `user`. Only the
   opt-in has another scope, so nothing else changes. Scope `solution` with a value is refused for it rather than
   written and ignored; null is allowed there so the warning can be cleared through the command.
5. **The Output line** is written each time the key appears in the workspace's file (a workspace opening with it, or
   the file gaining it), not on every reload; the window's strip follows the file.
6. **The schema commit alone** leaves the tree not building at that commit (the commands crate rejects the unknown
   scope until the next commit), as brief 0039's schema commit did.

## 7. Other settings that may deserve the same scope (not changed; to decide later)

A workspace's `.eludite/settings.json` still sets every other setting, and several of them name a program Eludite
runs or decide what agents may do, so a cloned repository can choose them: `build.cargoPath`,
`debugger.netcoredbgPath`, `debugger.monoPrefix`, `debugger.monoAdapterPath`, `debugger.lldbDapPath`,
`debugger.nodePath`, `debugger.jsDebugPath`, `languageServers.rustAnalyzerPath`, `test.vstestConsolePath`,
`agents.claudeCodeAdapterPath`, `agents.custom` (commands and arguments), `terminal.profiles` (commands),
`browser.chromePath`, `browser.enginePath` (user scope, but the workspace file can still set it), and
`debugger.allowAgentsByDefault`. The `user-workspace` scope (or a user-only scope that ignores the workspace file) fits
each; the choice is the owner's.

## 8. Open points

1. Windows: the state file has the profile's default access; whether to set an explicit ACL is for the Windows port
   (the opt-in is inert there per brief 0039's checklist).
2. State folders of deleted workspaces are never removed (as the layouts' files are not).
