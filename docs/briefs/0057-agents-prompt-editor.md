# Brief 0057: The Agents window's prompt editor and the slash-command menu

Status: done on Linux (uncommitted; screenshots not taken), see [0057-report.md](0057-report.md)
Phase: 2
Plan reference: PLAN.md sections 2 (principles 1, 3, 5), 5.2, 8
Depends on: brief 0016 (the Agents window), brief 0009 (the editor core), brief 0043 (the transcript's Markdown)
Followed by: briefs 0058 (model, effort and mode) and 0059 (usage, header and transcript polish), which edit the same
window after this one lands

## Goal

The prompt box of the Agents window is a real text editor instead of a string that grows at the end: the caret goes
where the person clicks or moves it, text can be selected, cut, copied, pasted and undone, long prompts wrap and the
box grows with them, and a previous prompt comes back with Up. Typing `/` at the start opens a menu of the agent's
slash commands (ACP's `available_commands_update`), filtered as the person types, so `/model`, `/effort`, `/compact`
and the rest are discoverable instead of memorized. The native Claude Code adapter starts sending that list, which it
does not today. Nothing about how prompts reach the agent changes: Enter still runs `eludite.agents.prompt`.

Today (`crates/eludite/src/shell/agents/window.rs`, `on_key`): the box keeps a `String`, `backspace` pops the last
character, there is no caret position, selection, clipboard, undo, mouse placement or wrapping, and the caret is a
`│` drawn after the text. The same pattern is in `eludite_ui::text_box` (the Error List's search box, the Find in
Files dialog) and `LineInput` in `crates/eludite/src/shell/debug/windows.rs`; those stay as they are in this brief,
but the widget built here takes a `multiline` flag so a later brief can move them to it.

## Files in scope

- `crates/editor/src/input.rs` (new): `TextInput`, a GPUI view over the editor core (`crates/editor/src/editor.rs`,
  `Editor`) with its own element and no gutter, margin, line numbers, highlighting thread, find bar, popups or
  decorations. `crates/editor/src/lib.rs` (export, docs), `crates/editor/src/view.rs` only to add the `TextInput` key
  context's bindings to `key_bindings()`, a `crates/editor/CLAUDE.md` only if the input needs a non-obvious rule (none exists today).
- `crates/eludite/src/shell/agents/window.rs` (the prompt box over `TextInput`, the slash menu, prompt history),
  `crates/eludite/src/shell/agents/transcript.rs` (the command list), `crates/eludite/src/shell/agents.rs` (the state
  output's `commands`), `crates/eludite/src/shell/agents/tests.rs`, `crates/eludite/src/bench.rs` (the prompt bench),
  `crates/eludite/src/args.rs` (its flag).
- `crates/acp/src/protocol.rs` (`AvailableCommand`, `SessionUpdate::available_commands`), `crates/acp/src/fake_agent.rs`
  (the fake agent sends its commands).
- `agents/claude-acp/src/agent.rs`, `agents/claude-acp/src/process.rs` (the `initialize` reply's `commands` kept),
  `agents/claude-acp/src/translate.rs` (the `available_commands_update` after `session/new`), its tests and a new
  recorded fixture under `agents/claude-acp/tests/fixtures/` (recorded with `agents/claude-acp/tools/record.py`, redacted
  with `redact.py`; never written by hand).
- `protocol/schemas/agents-state.output.json` first and alone (the `commands` array), then `crates/commands/src/agents.rs`.
- `crates/eludite/tools/agents.py` and `agents-linux.sh` (the two new screenshots), `docs/briefs/README.md`, this file,
  `docs/briefs/0057-report.md` (new).

## Contract

### The prompt editor

- `TextInput::new(multiline: bool, cx)` wraps `Editor::new(Buffer::new(""))` with no language. Its public API is
  `text()`, `set_text()`, `clear()`, `is_empty()`, `caret()` (byte offset), `set_caret()`, `select_all()`,
  `placeholder()`, `on_submit` and `on_escape` (the view emits `TextInputEvent::{Submit, Escape, Changed, Up, Down}`;
  `Up` and `Down` fire only when the caret is on the first or the last visual row with no selection, so the window can
  use them for history). Everything else goes through the editor core's existing methods: `insert`, `backspace`,
  `delete`, `delete_word_left`, `delete_word_right`, `move_left`, `move_right`, `move_word_left`, `move_word_right`,
  `move_home`, `move_end`, `move_to_start`, `move_to_end`, `select_all`, `click`, `drag_to`, `cut`, `paste`, `undo`,
  `redo` and `selected_text`. Vertical movement is by visual row (see wrapping), not buffer row, so the editor core's
  `move_vertical` is not used; the input computes the target offset from its last layout.
- Keys, in the key context `TextInput` (bindings added to `eludite_editor::key_bindings()` with that context; the
  Visual Studio keymap preset is the only one): Left, Right, Up, Down, Home, End, with Shift to extend, Ctrl+Left and
  Ctrl+Right by word, Ctrl+Home and Ctrl+End, Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+Z, Ctrl+Y, Backspace, Delete,
  Ctrl+Backspace, Ctrl+Delete, Enter (submit), Shift+Enter (newline; also Ctrl+Enter), Escape. On macOS Cmd replaces
  Ctrl for the clipboard, undo and select-all, and Alt+Left and Alt+Right move by word. Typed text, including IME
  composition, arrives through `EntityInputHandler` (`replace_text_in_range`, `replace_and_mark_text_in_range`) as the
  editor view does, so the test harness's `simulate_keystrokes` keeps working (`tests.rs`'s `type_prompt`).
- Wrapping: lines wrap at the box's width (`window.text_system().shape_text` with a wrap width, as GPUI's text
  elements do), never scroll horizontally. The caret, a click, a drag and Up and Down map through the wrapped rows of
  the last layout. The box is at least 2 rows and at most 8 rows tall in the Agents window (the view takes
  `min_rows` and `max_rows`; a single-line input is 1 and 1), then scrolls vertically, keeping the caret visible.
- Selection is drawn in `EditorStyle::selection`'s color, the caret as a 1 px bar in the text color (blinking is not
  required), the placeholder muted in the text's position while the text is empty and the box unfocused or focused
  (so the person sees where to type). Focus ring: the border in the accent color, as today.
- Mouse: click places the caret (`ClickKind::Single`), double-click selects the word, triple-click the row; drag
  selects; Shift+click extends. The context menu is out of scope.
- The Agents window keeps the caret and the text across turns and across the window hiding and showing. Submitting
  clears the box and pushes the text onto the prompt history (last 50, in memory for the session); Up on the first
  visual row with an empty or unedited box recalls the previous one, Down the next, and the newest-plus-one entry is
  the empty box again, as a terminal does. Escape with text and a menu open closes the menu; Escape otherwise still
  cancels the turn (brief 0016).
- Enter while a turn runs does nothing (as today). Enter with only whitespace does nothing.
- The prompt element keeps the id `PROMPT_BOX` (`agents-prompt`) and its painted bounds for the real-input driver.

### The slash-command menu

- ACP: a `session/update` of kind `available_commands_update` carries `availableCommands: [{name, description,
  input?: {hint}}]` (ACP schema 1.9's `AvailableCommand`; `crates/acp/src/protocol.rs` gets `AvailableCommand` and
  `SessionUpdate::available_commands()` the way `plan_entries()` reads `plan`, so unknown fields never break decoding).
  The transcript keeps the latest list (`Transcript::commands`); a new list replaces the old one in full.
- The native adapter: `claude`'s `initialize` control reply (`agents/claude-acp/src/agent.rs`, `new_session`) lists
  `commands` as `[{name, description, argumentHint, aliases?, builtin?}]` (verified on 2.1.289). The adapter keeps that
  list in the `Session` and sends one `available_commands_update` right after `session/new` answers, from the
  connection's task (`cx.send_notification`), because ACP clients expect the list before the first prompt. It maps
  `argumentHint` to `input.hint` when it is not empty and drops commands whose name starts with `__`. Aliases are not
  sent (ACP has no field). The Node adapter's list (fixture
  `crates/acp/tests/fixtures/claude-agent-acp-0.85.0-diagnostics.jsonl`) is the reference for the shape.
- The window opens the menu when the box's text starts with `/` and the caret is in the first word (no whitespace
  between the `/` and the caret). It lists the commands whose name contains the typed text after the `/`
  (case-insensitive; prefix matches first, then the rest, each group alphabetical), at most 9 rows visible
  (`eludite_ui::popup::COMPLETION_ROWS`), drawn with `eludite_ui::popup::popup_panel` and `completion_row` above the
  prompt box, anchored to its top-left, each row the name in the mono font and the description muted after it, the
  selected row highlighted, the matched characters bold as the editor's completion list draws them. The description
  of the selected command and its hint show in one muted line under the list.
- Keys while the menu is open: Up and Down move the selection (wrapping), Tab and Enter complete the selected command
  (the text becomes `/name ` with the caret after the space; Enter completes and does not send when the typed text is
  not already the full name, and sends when it is, so `/compact` Enter Enter sends `/compact`), Escape closes the
  menu without changing the text, typing filters, Backspace past the `/` closes it. A click on a row completes it.
  Mouse down outside closes it.
- No menu when the agent has sent no commands (the fake agent's scenarios that do not send them, an agent that has
  none); `/` is then ordinary text.
- `agents-state.output.json` gains `commands`: the agent's list as `[{name, description, hint?}]`, so an outer agent on
  the bus sees what the person sees (brief 0016's rule that the window shows nothing agents cannot read).

### The record

- A prompt that ran a local command (Claude Code's `/model`, `/effort`, `/compact`, `/clear`) comes back from the
  adapter as agent text (the `assistant` message with `local_command_source`), which brief 0043's Markdown rows show as
  today; nothing new here beyond the menu that helped type it.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/editor` `input` tests (headless GPUI): typing, Backspace at the caret in the middle of the text, Left and
  Right with Shift extending, Home, End, Ctrl+Left, Ctrl+Right, Ctrl+A, cut, copy and paste through the test
  clipboard, undo and redo, Shift+Enter inserting a newline while Enter emits `Submit`, a click at a pixel placing the
  caret in a wrapped row, Up and Down moving by visual row in a wrapped line and emitting `Up`/`Down` only at the first
  and last row, the box growing from 2 to 8 rows and then scrolling, IME composition through
  `replace_and_mark_text_in_range`.
- `crates/eludite` `agents::tests`: `type_prompt` still sends; a prompt edited in the middle (Home, type, End) arrives
  as edited; Up recalls the last prompt and Enter re-sends it; the fake agent's `stream` scenario sends two commands
  after `session/new`, typing `/` opens the menu with both, `co` filters to `compact`, Tab completes to `/compact `, Enter
  sends it and the fake agent's turn text echoes the prompt; Escape closes the menu and leaves `/co`; the state output
  lists the two commands; `/` with the `edit` scenario (no commands) is plain text.
- `agents/claude-acp`: a fixture recorded from `claude` 2.1.289 (`claude-2.1.289-commands.jsonl` and its `.acp.jsonl`
  golden, through `tools/record.py`) proves `available_commands_update` follows `session/new` with `model`, `effort`,
  `compact`, `clear` and `context` in it and no `__`-prefixed name; the conformance test replays it through
  `fake_claude`.
- `crates/eludite --bench-agent-prompt`: 2,000 characters typed into the prompt box one keystroke at a time in the
  headless shell; prints keystroke-to-frame p99.
- Manual, recorded with two screenshots by `tools/agents-linux.sh` (`tools/agents.py` extended): the prompt box with a
  three-row wrapped prompt and the caret placed by a click in its second row (`linux-agents-prompt-editor.png`); the
  slash menu open on `/mo` with the real native adapter (`linux-agents-slash-menu.png`).

## Budget

- Keystroke to frame submitted under 8 ms at p99 with a 2,000-character prompt (`--bench-agent-prompt`, debug build,
  headless; PLAN.md section 9's budget is for the release build on the reference machine).
- The menu opens on the frame after the `/` is typed; filtering 100 commands costs under 1 ms per keystroke.
- Brief 0016's streaming bench (`--bench-agent-stream`, frame p99 under 8 ms at 200 chunks per second) does not
  regress by more than 5 percent with the new prompt box focused and holding 500 characters.
- No new dependency.

## Exit criterion

1. Every test above is green; `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` and `agents/claude-acp`'s own `cargo test` pass.
2. The two screenshots exist under `crates/eludite/screenshots/` and show what the contract says.
3. `docs/briefs/0057-report.md` gives the bench numbers, lists which commands `claude` 2.1.289 reports, and says which
   other text boxes in the shell should move to `TextInput` (a sized follow-up list, not done here).
4. `docs/briefs/README.md` has this brief's row; CLAUDE.md's crate map row for `crates/editor` names the input.

## Out of scope

- Moving the Error List's search box, the Find in Files dialog's boxes, the Watch window's boxes or any other
  `text_box` user to `TextInput` (listed in the report; a later brief).
- Model, effort and mode pickers (brief 0058); the usage strip, header and transcript restyling (brief 0059).
- Markdown preview of the prompt, `@file` mentions, drag-and-drop of files into the prompt, pasted images, a context
  menu on the prompt box, a blinking caret, spell checking.
- Running slash commands locally in the shell: a slash command is text the agent receives, as today.
- Changes to `vendor/` or to the GPUI revision.
