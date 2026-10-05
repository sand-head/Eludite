# Brief 0059: The Agents window's usage strip, header and transcript polish

Status: done on Linux (uncommitted), except the screenshots; see [0059-report.md](0059-report.md)
Phase: 2
Plan reference: PLAN.md sections 2 (principles 1, 3, 5), 5.2, 8, 9
Depends on: brief 0058 (the footer with the pickers), brief 0057 (the prompt editor), brief 0034 (the usage line),
brief 0043 (Markdown rows), brief 0016 (the Agents window)

## Goal

The Agents window reads like a finished product instead of a debugging view. The context window and the session's
cost are visible all the time in a strip above the prompt box, not only in a line after each turn. The header is one
row (the agent, its state, Restart); the adapter's version, the ACP version and the MCP endpoint leave the window for
a tooltip and the Output window's new Agents source. "Turn ended: end_turn" no longer prints after every turn. A tool
call is one readable line (what it did, from the adapter's title) with a status, and its raw arguments and result
fold under it instead of filling the transcript with JSON. The person's prompts, the agent's text, thoughts, plans
and permission prompts get consistent spacing, type sizes and colors from the theme, and a running turn shows a live
status line with the elapsed time. This brief changes how the window looks and what it shows, not what the agent can
do; every row keeps its element id so the real-input driver and the tests still find it.

Today (`crates/eludite/screenshots/linux-agents-permission-prompt.png`): the header's second line is `eludite-claude-acp
0.1.0 (ACP v1). MCP: eludite via stdio relay to 127.0.0.1:41959`; each tool card prints `Tool Bash execute` and the
compacted JSON of its input; the transcript ends every turn with `Turn ended: end_turn`; usage is one muted line per
turn and the context size is nowhere while a turn runs.

## Files in scope

- `crates/ui/src/transcript.rs` (the card, the strip, the status line, the prompt block, the thought block, the
  notice; the tool-kind glyphs), `crates/ui/src/theme.rs` (only new tokens the strip needs: `warning`, `success`,
  `panel_raised`, with values for the three themes), `crates/ui/src/lib.rs` (docs).
- `crates/eludite/src/shell/agents/window.rs`, `crates/eludite/src/shell/agents/transcript.rs`,
  `crates/eludite/src/shell/agents.rs` (the header, the Output lines, the stop reasons), `crates/eludite/src/shell/output.rs`
  and `crates/commands/src/build.rs` (`OutputSource::Agents`), `protocol/schemas/output-show.input.json` (the source's
  description) first and alone for that part, `crates/eludite/src/shell/agents/tests.rs`, `crates/eludite/src/bench.rs`
  (the stream bench's assertions).
- `crates/acp/src/protocol.rs` only if the strip needs a field the usage reader does not already give.
- `crates/eludite/tools/agents.py` and `agents-linux.sh` (two screenshots), `docs/briefs/README.md`, this file,
  `docs/briefs/0059-report.md` (new).

## Contract

### The usage strip

- One row between the transcript and the prompt box, the small type size, muted: a context bar 80 px wide and 4 px
  tall (filled in the accent color up to `used / size`; `warning` from 80 percent; `size` 0 draws no bar), then
  `61k of 1M` (brief 0034's `tokens()` formatting), then `·`, then the session cost `$0.95` (the agent's running total
  from the last `usage_update`'s `cost`, in its currency), then the last turn's tokens in a tooltip on hover
  (`TurnUsage::text()`). Before the first `usage_update` the strip says `No usage yet` muted; it resets on Restart.
  Element id `agents-usage`; its text is in the state output as `usage: {used, size, cost?}` (`agents-state.output.json`,
  added here, first and alone).
- Brief 0034's per-turn `Row::Usage` stays in the transcript's model and in the `--transcript-out` record unchanged,
  and is still drawn, right-aligned, small and muted (`260k in · 2.9k out · $0.17`), so the per-turn cost remains
  readable in the transcript.

### The header and the Output window

- One row: the agent picker, the state as a colored dot and word (`● Ready`), a spacer, Restart (an icon button,
  `⟳`, with the tooltip "Restart the agent"; `START_BUTTON` keeps its id). The detail line goes: `HeaderState::detail`
  is shown only for `NeedsLogin` (the agent's label) and `Error` (the message), both as today's bordered block under
  the row. The ready detail (`eludite-claude-acp 0.1.0 (ACP v1). MCP: eludite via stdio relay to 127.0.0.1:41959`) becomes
  the tooltip of the state and one line in the Output window.
- `OutputSource::Agents` ("Agents" in the Show output from list, after Updates): one line per start (`Starting Claude
  Code: <command line>`), ready (the former detail), login state, turn start (`Prompt: <first 80 characters>`), turn
  end (`Turn ended: <reason> in <seconds>`), error, exit, and each line of the agent's stderr (today printed only
  with `ELUDITE_AGENT_STDERR`; the variable keeps working and the lines go to both). `eludite.output.show` with
  `source: "agents"` reads it; `output-show.input.json`'s description names it.
- The transcript's turn-end notice shows only for stop reasons other than `end_turn`, in words: `cancelled` ("Stopped"),
  `max_tokens` ("The model reached its output limit"), `max_turn_requests` ("The agent reached its request limit"),
  `refusal` ("The model declined to continue"), anything else as its name; a failed turn stays an error row.

### The transcript

- Tool call rows (`tool_call_card`): one line with a glyph by `kind` (`read` `▤`, `edit` `✎`, `delete` `✕`, `move`
  `⇄`, `search` `⌕`, `execute` `▶`, `think` `…`, `fetch` `⇣`, `switch_mode` `⇅`, other `•`, drawn in the kind's muted
  color), the title the adapter gave (`ls`, `Read Program.cs`, `diagnostics-list`; `ToolRow::name()` prefers the title
  over the tool name, and the tool name goes to the tooltip), the status badge at the right, and a chevron. Collapsed
  by default; a click on the line expands the arguments (pretty-printed JSON, the mono font, at most 40 lines then
  "… N more lines") and the result (as today, clipped at 1,500 characters) under it. A card with a permission note,
  pending changes, thumbnails or a debug line (brief 0027) shows those under the line whether or not it is expanded,
  exactly as today. A running call's badge is `running` with a spinner glyph that cycles (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, one step per
  100 ms, through GPUI's animation, only while running). Consecutive tool rows of one turn get a 2 px left rule in
  the border color so they read as one group. `tool_card(ix)` keeps its id; the expanded state lives in `ToolRow::expanded`
  (today used only for the debug line; a debug row's "Show snapshot" keeps its own toggle).
- The person's prompt (`user_prompt`): a block on `panel_raised` with a 3 px accent left border, the body type size,
  wrapped, with the prompt's time (`14:02`) small and muted at the right of its first line.
- The agent's text: body type size, line height 1.4, paragraphs 8 px apart (`agent_block` already lays out Markdown;
  this adjusts spacing and sizes only).
- Thoughts (`thought_block`): collapsed by default to one muted line `Thinking…` while it streams and `Thought for 4 s`
  after (the transcript records the thought's first and last chunk times), a click expands the text; `thought(ix)`
  keeps its id.
- Plans (`plan_card`): the `Plan` title with a count `2 of 5 done`, the entries as today.
- A status line between the transcript and the strip while a turn runs: `Claude Code is working… 0:12 · Esc to
  stop` (the agent's name, the elapsed time updated each second), with the spinner glyph; it disappears when the
  turn ends. Id `agents-status`.
- The permission prompt (`dialog_panel`): the sentence reads `Claude Code wants to run **ls** (execute).` with the
  tool's title bold, the detail block as today, the buttons as today.
- Spacing: an 8 px rhythm (rows 8 px apart, 12 px side gutters, the header 28 px tall, the footer 28 px); nothing is
  centered; all colors from the theme (`rgb(0x...)` literals in `window.rs` and `transcript.rs` move to theme tokens
  or `ToolStatus::color`). The three themes (VS Dark, Light, Blue) each look right: the report has a screenshot of
  each.

### Nothing else moves

- The review view, the pending-changes block, the Accept and Reject buttons, the debug rows, thumbnails and links keep
  their ids, positions and behavior.

## Proving test

- `crates/ui` tests: the glyph table covers every ACP tool kind and `other`; the strip's text for `size` 0, 20
  percent, 85 percent (warning color) and a cost in another currency; the spinner's frame for a given elapsed time.
- `crates/eludite` `agents::tests` with the fake agent: after the `stream` scenario's turn the strip reads `61k of 1M ·
  $0.95` and the state output's `usage` matches; before any turn it reads `No usage yet`; a tool card is collapsed and
  a click expands its arguments (the row's JSON record shows `expanded`); the turn-end notice is absent after
  `end_turn` and present as "Stopped" after a cancel; the Output window's Agents source holds the start, ready,
  prompt and turn-ended lines in order; a thought row reads `Thought for …` after the turn; the status line exists
  while running and not after.
- `--bench-agent-stream` (brief 0016): frame p99 under 8 ms at 200 chunks per second, with the strip and the status
  line updating; the bench asserts the strip's text at the end.
- Manual, two screenshots by `tools/agents-linux.sh`: the window mid-turn with the status line, a collapsed and an
  expanded tool card and the strip (`linux-agents-polish-running.png`); the same after the turn in VS Light
  (`linux-agents-polish-light.png`). The report adds the VS Blue one.

## Budget

- Frame p99 under 8 ms at 200 chunks per second (brief 0016's bench), within 5 percent of brief 0058's number; the
  status line's once-a-second update costs one frame per second, not continuous redraws.
- The spinner animates only while a call runs or a turn runs; an idle window requests no frames.
- No new dependency.

## Exit criterion

1. Every test above is green; `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
   `cargo test --workspace` pass.
2. The screenshots exist and match the contract; no `rgb(0x...)` literal remains in `window.rs` or `transcript.rs`.
3. `docs/briefs/0059-report.md` has the bench number and the three theme screenshots.
4. `docs/briefs/README.md` has this brief's row; CLAUDE.md's crate map row for `crates/ui` mentions the transcript
   widgets if it did not.

## Out of scope

- Selecting and copying transcript text, syntax highlighting in code blocks, a Copy button on code blocks (a later
  brief; brief 0043 left them out too).
- A "Clear transcript" or "New session" action beyond Restart; session history; multiple agents at once (PLAN.md 5.2,
  Phase 4).
- Showing Claude Code's `autocompact_state` (the threshold at which it compacts) in the strip: the adapter does not
  forward it yet; note in the report whether it should.
- Icons as SVG assets: the glyphs are text until the shell has an icon set (PLAN.md section 8, "our own iconography").
- Anything in brief 0057 or 0058.
