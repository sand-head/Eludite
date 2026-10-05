# Brief 0058 report: the Agents window's usage strip, header and transcript polish

Status: done on Linux (uncommitted), except the screenshots, which the orchestrator takes (section 4). Three files
outside the brief's list were added to its scope by the orchestrator (section 5). Windows and macOS: not run. CI: not run
(nothing pushed). The .NET host is not installed here, so `dotnet build` and `dotnet test` were not run; this brief
changes no .NET code.
Branch: `brief/0058-agents-window-polish`, based on `main` at `f86ede6` with briefs 0056 and 0057's uncommitted work
applied on top.
Date: 2026-10-05. Brief: [0058-agents-window-polish.md](0058-agents-window-polish.md).

## 1. Summary

- **Schemas first.** `agents-state.output.json` gains `usage: {used, size, cost?: {amount, currency}}`;
  `output-show.input.json` lists `agents` in `source`'s enum and its description names the Agents source.
- **`OutputSource::Agents`** (`crates/commands/src/build.rs`, `as_str` `agents`) and the Output window's Agents pane,
  last in "Show output from" (after Updates). The shell writes one line per start (`Starting Claude Code: <command
  line>`), ready (`Claude Code is ready: eludite-claude-acp 0.1.0 (ACP v1). MCP: eludite via stdio relay to
  127.0.0.1:41959`, the former header detail), login state (`... needs login: <label>` and each method's command),
  prompt (`Prompt: <first 80 characters>`, control characters as spaces), turn end (`Turn ended: end_turn in 3.2 s`;
  a failed turn `Turn ended: error in 1.0 s: <message>`), error and exit, and each stderr line of the agent
  (`[stderr] ...`; `ELUDITE_AGENT_STDERR` still also prints them to Eludite's stderr).
- **Theme tokens** (`crates/ui/src/theme.rs`): `warning`, `success` and `panel_raised` for VS Dark (`CCA700`, `89D185`,
  `2D2D30`: today's literals for the first two), Light (`8F6200`, `2B7A2F`, `E7E8EC`) and Blue (`8A5A00`, `26722A`,
  `E6EBF5`). The readability test checks warning and success text on `panel` and body text on `panel_raised` (4:1),
  and that `panel_raised` stands off `panel`. `ToolStatus::color` now takes the theme: success, warning, and a red and
  a blue picked for a dark or a light panel (the only literals left; the window's errors and its running state use
  them). No `rgb(0x...)` literal remains in `window.rs` or either `transcript.rs` of the shell.
- **The transcript widgets** (`crates/ui/src/transcript.rs`): `kind_glyph` over `TOOL_KINDS` (read ▤, edit ✎,
  delete ✕, move ⇄, search ⌕, execute ▶, think …, fetch ⇣, switch_mode ⇅, anything else •); `SPINNER`
  (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`) and `spinner_frame(elapsed)` at 100 ms a step; `elapsed_text` (`0:12`); `tokens` (moved from the
  shell, a trailing `.0` dropped so 1,000,000 is `1M`) and `money` (`$0.95`, `12.50 EUR`); `UsageStrip` (`text`,
  `fraction`, `bar_color`: the accent, `warning` from 80 percent, no bar for size 0) and `usage_strip` (the 80 by 4 px
  bar, then `61k of 1M · $0.95`, small and muted; `No usage yet` without one); `status_line`; `status_badge` with the
  spinner; `user_prompt` (on `panel_raised`, 3 px accent border, body size, line height 1.4, the time at the right of
  the first line); `agent_block` (body size, line height 1.4, 8 px between blocks); `thought_block` (one muted line
  with a chevron, the text under it when expanded); `ToolCard`/`tool_call_card` (one 22 px line: glyph in the muted
  color, the title, the badge, a chevron; expanded: the arguments in the mono font on the background in a border and
  the result; the note always; `grouped` draws the 2 px left rule in the border color, transparent otherwise so the
  width never changes); `clip_lines` (40 lines then `… N more lines`); `plan_card` with `plan_progress` (`2 of 5
  done`); `notice`; `usage_line` (right-aligned, small, muted).
- **The transcript model** (`crates/eludite/src/shell/agents/transcript.rs`): `Row::User { text, time }` (the local
  `HH:MM`; the offset comes from libgit2's `Signature::now`, which reads the system time zone, through the
  `eludite-git` the shell already links: no new dependency); `Row::Thought(ThoughtRow)` with the first and last chunk
  instants and `done` (set when another row follows or the turn ends), `label()` `Thinking…` / `Thought for 4 s`
  (at least 1 s); `ToolRow::name()` prefers the adapter's title, `tool_name()` keeps the tool's own name for the audit,
  the unaudited filter, the tooltip and the record's `tool`; the record gains `title` and `expanded`;
  `toggle_tool` (was `toggle_result`) is the card's state and a debug row's Show snapshot alike, so the debug row
  keeps its own toggle and brief 0027's test is unchanged; `Transcript::usage` (the session's last `TurnUsage`),
  `new_session()` (commands, usage and the running-cost base cleared on Start and Restart: the cost base used to carry
  over a restart), `end_turn()`; `TurnUsage::short()` (`260k in · 2.9k out · $0.17`) is what the row draws, while
  `text()` and the `--transcript-out` record are unchanged.
- **The window** (`crates/eludite/src/shell/agents/window.rs`):
  - Header: one 28 px row with 12 px gutters: the agent picker, `● Ready` in the state's color (its tooltip the
    detail: the ready line, or `Starting <command line>…`), a spacer, the icon button `⟳` (tooltip "Restart the
    agent"; `▶` and "Start the agent" while stopped or in error), id `agents-start` as before. Under it only the login
    block (`agents-login`, the agent's label, then the instructions and commands) or the error block
    (`agents-error`), bordered in the state's color.
  - The turn-end notice only for stop reasons other than `end_turn` (`window::stop_notice`): `cancelled` "Stopped",
    `max_tokens` "The model reached its output limit", `max_turn_requests` "The agent reached its request limit",
    `refusal` "The model declined to continue", anything else its name; a failed turn stays an error row.
  - Tool rows: the card above, collapsed by default; a click anywhere on it toggles it; the tool's own name is its
    tooltip; a running call's badge shows the spinner while the turn runs; consecutive tool rows get the rule; the
    debug line, change links and thumbnails sit around it as before. Each card's bounds go to `--bounds-out`
    (`agents-tool-<row>`, only the cards drawn in the last frame) for the screenshot driver.
  - The status line `agents-status`, `Claude Code is working… 0:12 · Esc to stop` with the spinner, between the
    transcript and the permission prompt, only while a turn runs; then the permission prompt, the pending changes (not
    moved), the usage strip `agents-usage` (its tooltip the last turn's `TurnUsage::text()`), the prompt box and the
    28 px footer.
  - The permission prompt: `Claude Code wants to run **ls** (execute).` (`window::permission_sentence`, the title bold
    through `StyledText` highlights; with an escalation reason `(execute: reason).`); `Prompt` gains `title`. In a
    short window the call's raw detail now shrinks first (it is `min_h_0` and clipped), so the buttons, the strip, the
    prompt box and the footer stay on screen: with the status line and the strip added, the 311 px tall Agents window
    of the headless tests pushed the footer out otherwise (brief 0057's picker test caught it).
  - Animation: a timer task (`AgentsWindow::ticking`) notifies the window every 100 ms while the turn runs, which
    redraws the spinner and the elapsed time; it is dropped when the turn ends, so an idle window requests no frames.
    GPUI's `with_animation` was not used: it requests a frame on every display refresh while the element is drawn,
    where the spinner needs ten a second.
- **The bench** (`--bench-agent-stream`, and `--bench-agent-prompt` on the same path): the JSON line gains
  `usage_strip`, `status_line_after` and `ticking_after`; the run exits 1 after printing when the strip is not
  `61k of 1M · $0.95` (`bench::AGENT_STREAM_STRIP`), the status line is still there or the window still ticks.
- **The driver** (`tools/agents.py`, `tools/agents-linux.sh`): section 4.

## 2. Tests

- `crates/ui`: `transcript::tests::every_acp_tool_kind_has_its_own_glyph_and_unknown_kinds_are_other`,
  `the_spinner_steps_every_100_ms_and_wraps` (and the elapsed time), `the_strip_says_the_context_and_the_cost_and_warns_from_80_percent`
  (`61k of 1M · $0.95` with the accent, size 0 `61k tokens` with no bar, 20 percent the accent, 85 and 80 percent
  `warning` in every theme, clamped over the window, `1.5M of 2M · 12.50 EUR`, the token forms),
  `long_arguments_are_clipped_by_lines_and_plans_count_what_is_done`, `statuses_have_labels_and_distinct_alarm_colors`
  (per theme); `theme::tests::text_is_readable_in_every_theme` covers the new tokens.
- `crates/commands`: `build::tests` parses `{"source": "agents"}`.
- `crates/eludite`, `shell::agents::transcript::tests`: `thinking_is_one_collapsed_block_and_toggles` (`Thinking…`,
  then `Thought for 1 s` when text follows, `Thought for 4 s` for 4.4 s, closed by `end_turn`),
  `prompts_carry_their_time_and_cards_their_title` (`HH:MM`, the record unchanged; `ls` titled card of tool `Bash`,
  collapsed, `toggle_tool` and the record's `tool`, `title`, `expanded`), `a_usage_update_is_one_line_under_its_turn`
  (the session's usage, the short form, `new_session` clears it and the cost base).
- `crates/eludite`, `shell::agents::tests` with the fake agent:
  - `the_usage_strip_shows_the_sessions_context_and_cost`: the strip is drawn and says `No usage yet` before and after
    Start; after the `stream` turn `61k of 1M · $0.95` (the session's usage 61,204 of 1,000,000, $0.9512), the row's
    short form `260k in · 2.9k out · $0.95`, no `Turn ended` notice, no status line and no timer; a restart says
    `No usage yet` again.
  - `a_running_turn_shows_its_status_and_tool_cards_fold`: at the permission prompt the status line is drawn and says
    `Fake agent is working… 0:0x · Esc to stop`, the window ticks; the prompt's title is `` `rm -rf obj/` `` and its
    sentence `Fake agent wants to run `rm -rf obj/` (execute).` with the title's range bold; after Deny the turn
    ends with no notice, no status line, no timer; the diagnostics card is collapsed (`expanded: false` in the
    record), a click expands it (the record says `true`, the card is taller), another folds it.
  - `a_cancel_says_stopped_and_the_output_window_logs_the_session`: Escape at the permission prompt ends the turn
    `cancelled` with the notice "Stopped"; `stop_notice` for every listed reason; Output > Agents holds `Starting Fake
    agent: eludite-fake-acp-agent --scenario diagnostics-then-shell ...`, `Fake agent is ready: eludite-fake-acp-agent
    ... (ACP v1). MCP: eludite via stdio relay to 127.0.0.1:...`, `Prompt: List the errors` and `Turn ended: cancelled
    in N s` in that order; no login or error block under the header; `eludite.output.show` with `source: "agents"`
    from another thread returns them.
  - `an_edit_is_held_for_review_accepted_as_one_undo_and_rejected` also checks the thought reads `Thought for N s`,
    collapsed, after the turn; `an_agent_that_exits_is_an_error_until_restarted` checks the error block.
  - The existing tests pass unchanged, including brief 0027's debug row (`debug::tests`) whose Show snapshot toggles
    `ToolRow::expanded`.

Commands run, all from the worktree root with `CARGO_INCREMENTAL=0`: `cargo fmt --check` (clean),
`cargo clippy --workspace --all-targets -- -D warnings` (clean), `cargo build --workspace` (builds), and in place of
`cargo test --workspace` (which fills this VM's disk) `cargo test -p <crate>` for every workspace member in turn,
deleting each run's test executables after it: `eludite-ui` 41 passed, `eludite-commands` 120 passed, `eludite` 406 of
409 passed, and `eludite-docking`, `eludite-editor`, `eludite-mcp`, `eludite-acp`, `eludite-protocol`,
`eludite-workspace`, `eludite-search`, `eludite-terminal`, `eludite-update`, `eludite-git`, `eludite-lsp`,
`eludite-dap`, `eludite-browser`, `eludite-forge`, `eludite-extensions`, `eludite-extension-sdk`,
`eludite-cdp-generator`, `eludite-dbg-netfx` and `eludite-chromium` all green. After the last change (the ellipsis on
the status line, the strip and the card title) `cargo test -p eludite-ui`, `cargo test -p eludite --bin eludite
agents::` (46 passed) and `debug::tests` (76 passed, brief 0027's debug row among them) ran again, with clippy and fmt. After the
follow-up (section 5): `cargo test -p eludite-commands` (120 passed), `cargo test -p eludite-acp` (all green, the new
`--usage` test among them), `cargo test -p eludite --bin eludite agents::` (46 passed), clippy and fmt clean, and one
more `--bench-agent-stream` run on Xvfb (exit 0; strip and state `usage` as expected; frame work p50 78.5 ms, p99
160.6 ms, software rendering).

The three `eludite` failures are frame-time budgets: `forge_tests::budgets_of_the_cached_list_the_large_document_and_memory`
(the large document's frame p99 9.7 to 15.5 ms against 8 ms, known on this VM), `git_tests::a_thousand_changed_files_draw_in_a_frame`
(8.6 to 17.6 ms) and `codelens_tests::typing_stays_under_the_keystroke_budget_with_200_lens_rows_on_screen` (8.2 to
9.2 ms, passing alone in two of three runs). They fail alone too, at a load average of 1 to 2 on 4 cores. None of them
draws the Agents window: with a temporary trace in `AgentsWindow::render`, the git and forge tests rendered it zero
times; what they share with this brief is the `Theme` value (three more colors) and one more Output pane. No baseline
build of `main` fits on this disk to compare.

## 3. Benchmark

`--bench-agent-stream` (2,000 chunks at 200 per second from the fake agent, a real child process), debug build, on
Xvfb with Mesa's lavapipe (software Vulkan), 1280x800, load average about 2. These are software-rasterized numbers,
like briefs 0056 and 0057's; the 8 ms budget can only be judged on the reference machine.

| Run | Frames | Frame work p50 / p95 / p99 (ms) | Apply per batch p50 / p99 (ms) | Strip at the end | Status line, timer after |
|---|---|---|---|---|---|
| 1 | 85 | 71.6 / 122.5 / 140.3 | 0.39 / 11.3 | `61k of 1M · $0.95` | gone, stopped |
| 2 | 90 | 69.5 / 107.1 / 159.0 | 0.30 / 9.2 | `61k of 1M · $0.95` | gone, stopped |
| 3 | 85 | 71.3 / 124.0 / 158.8 | 0.41 / 11.2 | `61k of 1M · $0.95` | gone, stopped |

Brief 0057's runs on the same kind of display: frame work p50 median 67.9 to 78.9 ms, p99 median 134 to 138 ms; these
(p50 69 to 72 ms, p99 140 to 159 ms) are inside that run-to-run spread for the median and about 10 percent over it for
the p99, with the status line redrawing ten times a second during the stream. A run on the reference machine is owed
before the 5 percent rule can be applied. Each run exited 0: the bench's own check of the strip, the status line and
the timer passed.

## 4. Screenshots

Taken by the orchestrator (this agent has no desktop session wired for screenshots; none are delivered from here).
For its own check of the layout this agent drew the window on Xvfb with the fake agent in the three themes and looked
at the result (not kept as deliverables): the one-row header, the cards (collapsed, expanded with the arguments box and
the result, the tool name as the tooltip), the status line, the strip with its bar and the right-aligned turn line
looked as the contract says; it found the status line wrapping into the strip in a 278 px wide window, now cut with an
ellipsis (as are the strip's text and a card's title). In VS Blue the push buttons (`eludite_ui::push_button`, outside
this brief's files) draw dark text on a dark fill, as before this brief.

| Theme | State | File |
|---|---|---|
| VS Dark | mid-turn: status line, a collapsed and an expanded tool card, the strip | `SHOT_DARK_PLACEHOLDER` |
| VS Light | after the turn | `SHOT_LIGHT_PLACEHOLDER` |
| VS Blue | after the turn | `SHOT_BLUE_PLACEHOLDER` |

With the fake agent instead of Claude Code (how this agent checked the layout): a config directory whose
`settings.json` registers `{"name": "Fake agent", "command": "<target>/debug/eludite-fake-acp-agent", "args":
["--scenario", "diagnostics-then-shell", "--usage"]}` and `{"name": "Fake streamer", "command": "...", "args": ["--scenario",
"stream", "--chunks", "300", "--rate", "100"]}` in `agents.custom`; `ELUDITE_CONFIG_DIR=<dir>
crates/eludite/tools/xvfb-linux.sh OUT --folder <repo> --agent "Fake agent" --theme dark --bounds-out B` with `KEEP=1`;
Ctrl+\, Ctrl+C; click `close-properties` and `agents-prompt` from B; type a prompt and Enter. The `Fake agent` turn
waits at the shell command's permission prompt (status line, the completed diagnostics card and the pending Bash card;
click a card's first 22 px to expand it; the strip says `No usage yet` until the first turn ends); Deny ends it and,
with `--usage`, fills the strip (`61k of 1M · $0.95`); a second prompt then shows a running turn with the strip
filled. `Fake streamer` streams for 3 s and ends with the strip at `61k of 1M · $0.95` and the turn line `260k in ·
2.9k out · $0.95`. `--theme light` and `--theme blue` pick the theme.

`tools/agents-linux.sh` (not run here): during step 2's shell prompt, at the permission prompt, `agents.py` clicks the
topmost tool card drawn (not the pending call's) and takes `agents-polish-running.png`; a new step 2b
(`SKIP_POLISH=1` skips it) starts a second Eludite with `--theme light` (`POLISH_THEMES="light blue"` for both),
sends one prompt that makes Claude run `ls` (allowed), waits for the turn's end, expands the topmost card and takes
`agents-polish-<theme>.png` (`agents.py --polish-light`). Copied into `crates/eludite/screenshots/` with the `linux-`
prefix, as the earlier ones.

## 5. Not done, and files outside the brief

- **Added to the scope by the orchestrator** (the brief's file list missed them):
  - `crates/commands/src/agents.rs`: `AgentsStateOutput::usage: Option<UsageOutput>` (`UsageOutput {used, size,
    cost?}`, `CostOutput {amount: f64, currency}`). `f64` has no `Eq`, so `AgentsStateOutput` and `AgentsOutput` no
    longer derive it (nothing compared them with more than `PartialEq`). The shell fills it in `agents_state` from
    `AgentsWindow::usage_strip()`; `commands::agents` tests its members against the schema;
    `the_usage_strip_shows_the_sessions_context_and_cost` asserts `{"used": 61204, "size": 1000000, "cost":
    {"amount": 0.9512, "currency": "USD"}}` after the turn and no `usage` after a restart; the stream bench's end
    check includes it (`state_usage`).
  - `protocol/schemas/output-clear.input.json`: `agents` in `source`'s enum; `a_cancel_says_stopped_and_the_output_window_logs_the_session`
    clears the Agents source through `eludite.output.clear` and finds it empty. (The enum still lacks `tests`, since
    brief 0035, and `output-show.output.json`'s `source` lists five sources; neither is changed here.)
  - `crates/acp/src/fake_agent.rs`: `--usage` (`Options::usage`) ends every turn of the `diagnostics` and
    `diagnostics-then-shell` scenarios with `stream_usage()`, documented in the module docs;
    `fake_agent::tests::the_usage_flag_ends_a_diagnostics_turn_with_the_streams_usage`. The shell tests' `Fake agent`
    runs with it; `a_running_turn_shows_its_status_and_tool_cards_fold` asserts the strip at `61k of 1M · $0.95` and
    the state's `usage` after Deny. No existing assertion changed.
- Screenshots: section 4. Windows and macOS: not run. `dotnet build` / `dotnet test`: no SDK here.
- `autocompact_state`: the native adapter does not forward Claude Code's compaction threshold; the strip would be the
  place for it (a tick on the bar at the threshold), worth a line in a later adapter brief once `claude` reports it in
  the stream.

## 6. Notes

- The spinner and the status line cost ten frames a second while a turn runs (the budget line's "one frame per
  second" would hold for the elapsed time alone; the brief also asks for a spinner stepping every 100 ms). An idle
  window has no timer.
- The user prompt's time is the local time when the prompt was added to the transcript.
- A running call left `in_progress` by an agent after its turn ended keeps its `running` badge without the spinner
  (the spinner follows the turn, so nothing animates once the turn is over).
