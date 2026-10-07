# Brief 0057 report: the Agents window's prompt editor and the slash-command menu

Status: done on Linux (uncommitted), except the two manual screenshots (section 5). Windows and macOS: not run. CI:
not run (nothing pushed). The .NET host was not installed here, so `dotnet build` and `dotnet test` were not run; this
brief changes no .NET code.
Branch: `brief/0057-agents-prompt-editor`, based on `main` at `f86ede6`.
Date: 2026-10-05. Brief: [0057-agents-prompt-editor.md](0057-agents-prompt-editor.md).

## 1. Summary

- **`eludite_editor::TextInput`** (`crates/editor/src/input.rs`): a text box over the editor core's `Editor` with its
  own element and none of the editor view's gutter, highlighting, popups or decorations. Text is shaped with
  `TextSystem::shape_text` at the box's width (2 px left for the caret), so it wraps and never scrolls sideways; an
  `InputLayout` of visual rows (byte ranges of each logical line between GPUI's wrap boundaries) maps the caret, a
  click, a drag, Up and Down. The element measures itself (`request_measured_layout`): `min_rows` to `max_rows` rows of
  the inherited line height (2 to 8 multiline, 1 single-line), then it scrolls vertically and keeps the caret's row in
  view. Selection in `EditorStyle::selection`'s colour, a 1 px caret in the text colour, the placeholder in the theme's
  muted colour while empty (focused or not).
- **Keys** in the context `TextInput` (added to `eludite_editor::key_bindings()`; their actions are in the
  `text_input` namespace, so the shell's filter of `editor::Undo` and friends does not drop them): arrows with Shift,
  Ctrl+Left/Right with Shift, Home/End, Ctrl+Home/End, Ctrl+A/C/X/V/Z/Y (Cmd on macOS, plus Alt+Left/Right by word),
  Backspace, Delete, Ctrl+Backspace, Ctrl+Delete, Enter (`Submit`), Shift+Enter and Ctrl+Enter (a line break), Escape.
  Up on the first visual row and Down on the last with no selection emit `TextInputEvent::Up` / `Down` instead of
  moving; with a selection they go to the start or the end. Typed text and IME composition arrive through
  `EntityInputHandler` (`replace_and_mark_text_in_range` marks and underlines the composition; `replace_text_in_range`
  commits it). Cut and paste are undo steps of their own; typing groups as in the editor. Copy and Cut with an empty
  selection do nothing (the code editor's whole-line copy is not a text box's).
- **The prompt box** (`crates/eludite/src/shell/agents/window.rs`) is a multiline `TextInput` owned by the window, so
  its text and caret survive turns and the window hiding. Enter sends (`eludite.agents.prompt`, unchanged) unless a
  turn runs or the text is only whitespace; sending clears the box and pushes the prompt on an in-memory history (50,
  a repeat of the last kept once). Up on the first row of an empty or unedited box recalls the previous prompt, Down
  the next, then the empty box. Escape cancels the turn (brief 0016) unless the slash menu is open.
- **The slash menu**: ACP's `available_commands_update` decodes through `SessionUpdate::available_commands()`
  (`crates/acp/src/protocol.rs`, kept as `Other` like `plan`; a malformed entry is skipped, an input of another shape
  reads as none). The transcript keeps the latest list (`Transcript::commands`, replaced in full, cleared when a new
  session starts). The menu opens while the text starts with `/` and the caret is in the first word, lists the
  commands whose name contains the typed word (ASCII case-insensitive; prefix matches first, each group alphabetical),
  at most 9 rows around the selection, drawn with `popup_panel` and `completion_row` above the box (anchored
  bottom-left to its top-left), the name in the mono font with the match bold and the description muted, then one
  muted line with the selected command's hint and description. Up and Down move (wrapping), Tab completes to
  `/name ` with the caret after the space, Enter completes unless the typed word is already the selected name (then it
  sends: `/compact` Enter Enter), Escape closes it without changing the text (until the text changes), a click on a row
  completes it, a mouse-down outside closes it. The window takes those keys with `capture_action` on the box's
  wrapper, before the input sees them. No commands, no menu.
- **State output**: `agents-state.output.json` gains `commands: [{name, description, hint?}]` (schema first), typed as
  `CommandRow` in `crates/commands/src/agents.rs` and filled by `Shell::agents_state`.
- **The native adapter** (`agents/claude-acp`): `InitializeReply::parse` (`process.rs`) keeps `claude`'s `commands`
  from the `initialize` control reply in the `Session`; right after `session/new` answers, the same task sends one
  `available_commands_update` (`translate::available_commands_update`): `argumentHint` as `input.hint` when not blank,
  names starting with `__` dropped, entries without a name skipped, aliases not sent.
- **The fake agent**'s `stream` scenario sends two commands (`compact`, `model`) after `session/new` and answers a
  prompt starting with `/` with one line (`Ran the local command /compact`) instead of streaming.
- The shell traces `agents commands N` when a list arrives (the screenshot driver waits on it), and the Start button's
  bounds are recorded for `--bounds-out`.

## 2. Tests

- `crates/editor` (`input::tests`, headless GPUI, 8 tests): typing and Backspace at a caret in the middle, Shift+Left
  and Shift+Right, Home, End, Ctrl+Left/Right with Shift, typing over a selection, the word deletions; Ctrl+A, copy,
  cut and paste through the test clipboard, undo and redo (Ctrl+Y and Ctrl+Shift+Z), copy with no selection; Enter
  emits `Submit` while Shift+Enter and Ctrl+Enter break the line, Escape; a single-line input refuses line breaks;
  the box grows 2, 2, 3 … 8 rows as 20-character rows are typed, then scrolls with the caret's row at the bottom and
  back to the top on Ctrl+Home; a click at a pixel places the caret in the second wrapped row, Shift+click extends,
  double-click selects the word and a drag extends it; Up and Down move by visual row keeping the goal column (also
  through a shorter logical line) and emit `Up`/`Down` only on the first and last row with no selection; IME
  composition marked, grown and committed, read back in UTF-16, with its bounds.
- `crates/eludite` (`shell::agents::tests`): `the_prompt_box_edits_at_the_caret_and_recalls_prompts` (Home, type, End
  arrives edited; Up recalls and Enter re-sends; Down to the empty box; Up leaves a draft alone; the draft and caret
  survive hiding the window behind Workspace; whitespace sends nothing; the history keeps a repeat once) and
  `the_slash_menu_offers_the_agents_commands` (the state output lists the two commands; `/` opens the menu above the
  box with both; Down/Up wrap; `co` filters to `compact`; Escape closes it, leaves `/co` and cancels nothing; typing
  reopens it; Tab completes to `/compact ` with the caret at 9; Enter sends it and the fake agent's turn echoes it;
  `/mo` Enter completes without sending; `/model` Enter sends; a click on a row completes; Backspace past `/` closes
  it; no menu outside the first word; the `edit` scenario has no commands, `/co` is plain text and the state lists
  none). The existing tests that type with `type_prompt` pass unchanged. `transcript::tests` gains the command-list
  replacement test; `args` tests the new flag.
- `crates/acp`: `available_commands_decode_and_a_bad_entry_is_skipped` (the Node adapter's shape, a nameless entry, a
  bad input, round trip) and the fake agent's `the_stream_scenario_lists_its_commands_after_session_new_and_answers_one`.
- `crates/commands`: the state output's `commands` rows serialize to the schema's members.
- `agents/claude-acp`: `golden::recorded_commands_map_exactly` (the new fixture's golden
  `claude-2.1.289-commands.acp.jsonl`: one `available_commands_update` from the `initialize` reply, with `model`,
  `effort`, `compact`, `clear` and `context`, no `__` name, `compact`'s hint, no input for `context`, no aliases),
  `golden::commands_drop_internal_names_nameless_entries_and_empty_hints`, and
  `conformance::recorded_commands_follow_session_new` (the real adapter binary against `fake_claude` replaying the
  fixture: exactly one list, for the new session, after `session/new`, with the five names and no `__` name, and the
  replay matched).

Commands run, all from the worktree root (the adapter's from `agents/claude-acp`): `cargo test -p eludite-editor`,
`cargo test -p eludite-acp -p eludite-commands`, `cargo test -p eludite agents::` (40 passed), `cargo test -p eludite
args`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace`,
`cargo test --workspace`, and in `agents/claude-acp`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test` (9 unit, 9 conformance, 5 golden).

Results: `cargo fmt --check` clean (both workspaces); `cargo clippy --workspace --all-targets -- -D warnings` clean;
`cargo build --workspace` builds; the adapter's clippy and tests clean. `cargo test --workspace` could not finish
here: the shared disk filled twice during it (the worktree's debug `target/` is about 26 GB) and the run was stopped
before it ran out. Instead every crate this brief touches or that depends on one was tested: `eludite-editor` (all
78 lib tests and its integration tests), `eludite-acp`, `eludite-commands`, `eludite-mcp`, `eludite-docking`,
`eludite-search`, `eludite-browser`, and all of `eludite` (399 of 402 passed). The three failures are timing budgets
asserted only while the load average is under the core count: `git_tests::a_thousand_changed_files_draw_in_a_frame`
and `search_tests::ten_thousand_matches_draw_in_a_frame` pass when rerun alone;
`forge_tests::budgets_of_the_cached_list_the_large_document_and_memory` measured a scrolling frame p99 of 10.1 to
10.8 ms against 8 ms on three reruns (load average 2.3 to 3.4 on 4 cores, another agent building). That test draws the
forge document with the Agents window behind Workspace, so no `TextInput` is laid out; a baseline on `main` was not
measured here (no disk for a second build), so it needs a check on a quiet machine or in CI.

## 3. The recorded fixture

`agents/claude-acp/tests/fixtures/claude-2.1.289-commands.jsonl` was recorded with `tools/record.py` from `claude`
2.1.289 (`/opt/node22/bin/claude`) with no prompt (`'[]'`, so no model call: the `initialize` request, its reply and
the exit) and an MCP config with no servers, then redacted. `tools/redact.py` as checked in replaces the reply's
`commands` with `[]`, which would leave nothing to prove, and `tools/` is not in this brief's files; the fixture was
redacted with a copy of `redact.py` changed to keep the commands marked `builtin: true` (dropping the user's own
skills and commands, as the redaction intends). That two-line change is not applied to `tools/redact.py` here; it
should be, so the fixture can be re-made with the checked-in tool:

```diff
-                    "commands": [], "agents": [], "output_style": "default", "models": [],
+                    "commands": [c for c in resp.get("commands", []) if c.get("builtin") is True],
+                    "agents": [], "output_style": "default", "models": [],
```

`claude` 2.1.289 reported 54 commands: 53 with `builtin: true` and one of the user's own (a skill, dropped by the
redaction). The adapter sends 52, all but `__remote-workflow`: `deep-research`, `design`, `design-sync`, `dataviz`,
`update-config`, `verify`, `debug`, `code-review`, `simplify`, `batch`, `fewer-permission-prompts`, `doctor`, `loop`,
`schedule`, `claude-api`, `workflow-authoring`, `run`, `run-skill-generator`, `plugin-authoring`, `advisor`, `agents`,
`auto-mode-setup`, `autocompact`, `clear`, `color`, `compact`, `config`, `output-style`, `context`, `effort`, `fast`,
`focus`, `heapdump`, `init`, `mcp`, `import`, `model`, `workflow-launch-exec`, `reload-plugins`, `reload-skills`,
`rename`, `ultrareview`, `security-review`, `usage`, `insights`, `recap`, `skill-doctor`, `goal`, `design-consent`,
`design-revoke`, `list-agents`, `team-onboarding`. Several of these are bundled skills of the environment the
recording ran in (a cloud container); a desktop install lists fewer. `argumentHint` is often empty (`context`,
`init`, `usage`); `aliases` appear on 9 (`clear`: `reset`, `new`).

## 4. Benchmarks

`--bench-agent-prompt PATH` (`crates/eludite/src/bench.rs`, flag in `args.rs`) rides on `--bench-agent-stream`'s
setup: the flag sets `bench_agent_stream` to PATH (so `app.rs`'s existing wiring registers the fake streamer and runs
`bench::agent_stream`, with no change to `app.rs`, which is not in this brief's files) and `bench_agent_prompt`, which
`agent_stream` reads back by parsing the process arguments with `Args::parse`. It types 2,000 characters
(`lorem ipsum …`, words of 2 to 11 letters) into the focused prompt box with `Window::dispatch_keystroke`, 8 to 14 ms
apart, and reports keystroke-to-frame as `--bench-type` does (key handler plus the next frame's render to the end of
present); then it puts 500 characters in the focused box and runs the 2,000-chunk stream at 200 chunks/s, reporting
its frame work.

Run here: debug build, Xvfb `:99` (1280x800 window) with Mesa's software Vulkan (lavapipe, installed for the run), on
4 cores shared with another agent's build (load average 2 to 4.6). Rendering is software rasterization, so these are
not the budget's numbers (brief 0016's and 0052's runs used a real GPU); the same display gives the code editor's own
`--bench-type 500` a keystroke-to-frame p99 of 487 ms (p50 275 ms).

| Run | Prompt typing keystroke-to-frame p50 / p99 (ms) | Key handler p50 | Stream frame work p50 / p99, empty box | Stream frame work p50 / p99, 500 chars in the focused box |
|---|---|---|---|---|
| 1 | 79.3 / 193.8 | 0.38 | 92.4 / 160.3 | 97.4 / 188.6 |
| 2 | 80.7 / 207.8 | 0.37 | 69.0 / 130.5 | 72.0 / 116.7 |
| 3 | 83.3 / 214.7 | 0.37 | 64.6 / 156.0 | 72.0 / 136.7 |

The 2,000-character prompt wrapped to 78 visual rows (the box shows 8 and scrolls). The 8 ms budget cannot be judged
on this machine: the frame cost is dominated by lavapipe (the handler alone is 0.37 ms at the median, and the
editor's own typing is 2.5 times slower here). The stream's frame work with the box holding 500 characters is within
the run-to-run spread of the empty-box runs (medians +5 %, +4 %, +11 %; p99 +18 %, −11 %, −12 %), which is wider than
the 5 % budget; the comparison needs the reference machine. The menu's filtering is a substring scan of the list
per render (52 commands from the real adapter); it was not timed separately.

## 5. Not done

- **The two screenshots** (`linux-agents-prompt-editor.png`, `linux-agents-slash-menu.png`). `tools/agents.py` now
  types a prompt that wraps to three rows, clicks into its second row and shoots `agents-prompt-editor.png`, clears
  it, presses Start, waits for the `agents commands N` trace, types `/mo` and shoots `agents-slash-menu.png`, all
  before the first real prompt; `tools/agents-linux.sh` documents it and adds `--bench-agent-prompt` to the bench
  step. Neither ran: this machine has no KWin, spectacle, python-xlib or Eludite host build, and the run needs a real
  desktop session with the native adapter. Nothing was faked; `crates/eludite/screenshots/` is unchanged.
- Windows and macOS: not built or run. The macOS bindings (Cmd, Alt+Left/Right) are untested.
- `dotnet build` / `dotnet test`: the SDK is not installed here.

## 6. Notes and choices

- Tab is bound in the `TextInput` context to `text_input::Tab`, which the input does not handle: its owner can take
  it (the slash menu does), else the keystroke falls through to the next binding. The brief's key list has no Tab, but
  the menu needs one the input's context reaches first.
- `set_text` starts a new undo history (history recall and menu completion use it); Ctrl+Z does not bring back the
  text a recall replaced. Home and End are the editor core's (`move_home`, `move_end`): the logical line's start
  (first non-blank, then column 0) and end, not the visual row's; triple-click selects the logical line.
- The placeholder now also names Shift+Enter and `/`: "Ask the agent… (Enter to send, Shift+Enter for a new line, /
  for commands, Esc to stop)".

## 7. Other text boxes to move to `TextInput` (a later brief)

All draw `eludite_ui::text_box` over a `String` they push to and pop from (no caret, selection, clipboard or undo):

| Where | Boxes | Size |
|---|---|---|
| Error List search (`error_list.rs`), Test Explorer search (`tests_window.rs`), NuGet search (`nuget/window.rs`) | 3 single-line | S: same pattern each, filter on `Changed` |
| Find in Files dialog (`search/dialog.rs`): Find, Replace, Look in, File types | 4 single-line | M: Tab order between boxes, history drop-downs |
| Watch window, Immediate and the other debugger inputs (`debug/windows.rs`, `LineInput`) | 1 shared type, about 5 uses | M: Enter evaluates, Up/Down history like the prompt |
| Rename dialog (`rename.rs`), Options (`options.rs`), project properties (`project_properties/pages.rs`) | 3 | S to M: select-all on open, validation |
| Credentials prompts (`git/credentials.rs`, `nuget/credentials.rs`, `forge/signin.rs`) | 6, two of them passwords | M: needs a masked mode in `TextInput` |
| Forge windows (`forge/create.rs`, `issues.rs`, `pulls.rs`, `document.rs`, `margin.rs`): titles, bodies, comments | 7, several multiline | M: the multiline ones gain the most (wrapping, Shift+Enter) |
| Git Repository (`git/repository.rs`), NuGet sources (`nuget/sources_page.rs`), Web Browser address and dialogs (`browser_window.rs`) | 6 | S to M; the address bar wants select-all on focus |
| Terminal's find box (`crates/terminal/src/view.rs`) | 1 | S, but in another crate (it would depend on `eludite-editor`) |

A masked (password) mode and select-all-on-focus are the two features `TextInput` lacks for all of them.
