# Brief 0057 report: model, effort and mode in the Agents window

Status: done on Linux (uncommitted), except the manual screenshot and the turn after picking Opus (section 6).
Windows and macOS: not run. CI: not run (nothing pushed). The .NET host is not installed here, so `dotnet build` and
`dotnet test` were not run; this brief changes no .NET code.
Branch: `brief/0057-agents-model-effort-mode`, based on `main` at `f86ede6` with brief 0056's uncommitted work applied
on top (its prompt editor and slash menu are the base this brief's footer sits under).
Date: 2026-10-05. Brief: [0057-agents-model-effort-mode.md](0057-agents-model-effort-mode.md).

## 1. Summary

- **Schemas first.** `protocol/schemas/agents-configure.input.json` (new: `{option, value}`), `agents-state.output.json`
  gains `mode: {current, available: [{id, name, description?}]}` and `options: [{id, name, description?, category?,
  current, choices: [{value, name, description?}]}]`, both absent when the agent offers none. The settings
  `agents.model` and `agents.effort` (strings, default empty, user scope) are declared in `protocol/schemas/settings.json`.
- **`eludite.agents.configure`** (`crates/commands/src/agents.rs`): `AgentsRequest::Configure { option, value }`,
  agent-visible, class `execute`; the state's `ModeOutput`, `ModeRow`, `OptionRow`, `ChoiceRow`.
- **The ACP client** (`crates/acp`): `NewSessionRequest` carries `_meta`; `NewSessionResponse` gains `modes`
  (`SessionModeState`, `SessionMode`) and `config_options` (`SessionConfigOption` with `SessionConfigKind::Select`
  or `Other(raw)`: a boolean or newer kind decodes and is not shown; ACP's grouped select choices are flattened; an
  entry that does not decode is skipped, a malformed `modes` reads as none). `methods::SESSION_SET_MODE` and
  `SESSION_SET_CONFIG_OPTION`, `AcpClient::set_mode`, `set_config_option` and `new_session_with_meta`;
  `SessionUpdate::current_mode()` and `config_options()` read the two updates from `Other`, like `plan_entries()`.
- **The session** (`crates/acp/src/session.rs`): it keeps the agent's modes and options (from `session/new`, the two
  updates and each `set_config_option` answer) and reports the whole set as `SessionEvent::Options` after `session/new`
  and after each change; `AgentSession::set_mode` and `set_option` queue a job on `acp-driver` (the prompt channel is
  now a job channel), which answers `Options` or `SessionEvent::OptionFailed { id, error }` (the agent's `data` when it
  gives one). `AgentSession::start_with_meta` passes `_meta` to `session/new`; `start` is unchanged.
- **The native adapter** (`agents/claude-acp`): `translate::SessionOptions` (pure) holds the modes `default`
  ("Manual", "Eludite reviews each edit and prompts as the policy says") and `plan` ("Plan", "Plans before making
  changes"), the `model` option (one choice per entry of the `initialize` reply's `models`, `displayName` as the name)
  and the `effort` option (`default`, "The model decides", then the current model's `supportedEffortLevels`
  capitalized; absent for a model without `supportsEffort`). `session/new` answers them; `session/set_mode` sends
  `set_permission_mode`; `session/set_config_option` sends `set_model` for the model and writes `/effort LEVEL` for the
  effort, consuming the local command's `assistant` and `result` (no transcript turn). Both are refused with
  `invalid_request` and the data `in_turn: ...` while a turn runs (checked with the turn lock's `try_lock`, before
  anything is written to `claude`), answer the whole state and notify `current_mode_update` or `config_option_update`.
  `default` effort is refused (`invalid_params`); a model change keeps a supported effort, else goes back to `default`.
  `/model X` and `/effort X` typed as prompts still run as Claude Code's own commands; the adapter sees the `result`'s
  `local_command` and notifies `config_option_update` (X taken only when it is one of the option's values), and a
  turn's model (`usage_update`'s, tracked by the translator) corrects the model option when the option resolves to
  another model. `--effort LEVEL` and `ELUDITE_CLAUDE_EFFORT` join `--model`; `_meta.claudeCode.options.effort` is read
  beside `.model` (`default` and unknown levels pass nothing). `agentCapabilities` are unchanged.
- **The shell** (`crates/eludite/src/shell/agents.rs`): the modes and options are kept in `Agents` and listed in the
  state output. `eludite.agents.configure` checks the session (`not supported` with no session or no options), the
  turn (`busy: a turn is in progress; wait for the turn to end`), the option (`unknown option`) and the value
  (`unknown value`), then has the session ask the agent. From the UI thread it returns at once; from another thread the
  caller waits for the agent's answer: `AgentsJob::reply` is now a `JobReply`, whose `send` (called by the existing
  pump in `shell.rs`, unchanged, right after `apply_agents`) hands the caller the receiver `apply_agents` left in a
  UI-thread slot, so the UI thread never waits for the agent. A pick made in the window (not one made through the bus)
  is written to `agents.model` or `agents.effort` (user scope; by ACP category `model` and `thought_level`, else the ids
  `model` and `effort`) once the agent took it; every `session/new` gets the two settings in
  `_meta.claudeCode.options`. The mode is not remembered. `Agents::status_text` adds the model's name: `Fake options:
  ready · Smart`.
- **The window** (`crates/eludite/src/shell/agents/window.rs`): the prompt box now has its own row and a footer under
  it: the model picker, the effort picker, any other select option, the mode picker, a spacer, the Send/Stop button
  (moved here). `window::pickers` orders them by ACP category; a config option of category `mode` is not shown when the
  agent also offers modes (the Node adapter offers both). Each picker is a flat `Name ▾` button
  (`agents-option-<id>`, `agents-mode`) opening a `popup_panel` list above it (`agents-option-<id>-<value>`,
  `agents-mode-<id>`), the current choice checked and the highlighted one in the accent colour, each with its
  description muted after it. Up, Down, Enter and Escape work while one is open: the prompt box keeps the focus, and
  the footer takes `text_input::MoveUp`/`MoveDown`/`Submit`/`Escape` with `capture_action` before the box and its
  slash menu (no new key binding). A mouse-down outside closes it (a click on its own button toggles it). A pick shows
  the new name muted until the agent answers; a refusal reverts it and adds an error row ("Could not change the
  effort: ..."). While a turn runs the pickers are muted, open nothing and have the tooltip "Wait for the turn to end".
  The pickers are computed when the options change, not per frame. The shell traces `agents options model=... ...`
  and `agents option <key> = <value>` for the screenshot driver.
- **The fake agent**'s `--options` (`crates/acp/src/fake_agent.rs`, any scenario): modes `default` ("Manual") and
  `plan`; options `model` (`fast`, `smart`; current `smart`) and `effort` (`default`, `low`, `high`, `max`; current
  `default`), their current values taken from `session/new`'s `_meta.claudeCode.options`; `session/set_mode` and
  `session/set_config_option` answer and notify; an unlisted value is `invalid_params`, and so is the listed effort
  `max` (`REFUSED_EFFORT`), so the shell's refusal path is tested with an agent that refuses a choice it offered.

## 2. Tests

- `agents/claude-acp`:
  - `conformance::recorded_options_follow_mode_model_and_effort_changes`: the real adapter binary against
    `eludite-fake-claude` replaying the new fixture. `session/new`'s `modes` (the two) and `configOptions` (the model
    with its 12 choices, current `default`; the effort `default`, `low` … `max`); `session/set_mode` `plan` and
    `default` (answer `{}`, one `current_mode_update` each), `bypassPermissions` refused (`unknown mode`);
    `session/set_config_option` `model` = `opus` and `effort` = `high`, each under 2 s, answering the whole list with
    the new value and notifying the same list, with no `agent_message_chunk`, no thought and no `usage_update` for
    the effort's local command; `effort` = `default`, `huge`, an unknown option and an unknown model refused. The ACP
    side is pinned in the golden `claude-2.1.289-options.acp.jsonl` (`UPDATE_GOLDEN=1 cargo test --test conformance`
    rewrites it). The child side: the controls `initialize`, `set_permission_mode` (`plan`, `default`), `set_model`
    (`opus`) in order, one user message `/effort high`, no mismatch.
  - `conformance::a_change_during_a_turn_is_refused_without_writing_to_claude`: during a streamed turn
    (`FAKE_CLAUDE_SCENARIO=stream`), `set_config_option` (model, effort) and `set_mode` answer `in_turn`; the fake's
    log shows only `initialize`, the turn's prompt and the cancel's `interrupt`. The brief asks for a unit test; the
    check needs a live `claude` child and its stdin, so it is this integration test over the fake's input log.
  - `conformance::a_typed_local_command_keeps_its_reply_and_moves_the_option`: the same recorded `/effort high`
    reached through `session/prompt`: the reply is the turn's text and a `config_option_update` follows with
    `effort` = `high` (the model `opus` kept).
  - `conformance::effort_comes_from_the_environment_or_the_session_meta`: `ELUDITE_CLAUDE_EFFORT=high` and
    `_meta.claudeCode.options` (`opus`, `max`) reach `claude`'s argv as `--effort` and `--model`; `default` and `huge`
    pass nothing.
  - `translate::tests` (3): the options from 2.1.289's `models` shape (names, categories, descriptions, capitalized
    levels, no options without models); a resolved launch model shown as its entry, the launch effort current, a model
    change keeping a supported effort and dropping an unsupported one (Opus 4.6 has no `xhigh`; Haiku has no effort
    option), `default` never settable; `/model X` and `/effort X` (and `/modelx`, unknown values), the turn's model
    correcting the option, `<synthetic>` ignored, `local_command` read from `result` only.
- `crates/acp`: `protocol::tests::session_new_decodes_with_and_without_modes_and_config_options` (the Node adapter's
  shape, a grouped select, a boolean option kept raw, an entry with no id skipped, a malformed `modes`, `_meta` only
  when given) and `mode_and_config_option_updates_decode_and_an_unknown_kind_is_not_listed` (a `slider` option
  decodes, is `Other` and is not listed; round trip; the set answer; the set request's JSON);
  `fake_agent::tests::the_options_flag_offers_modes_and_options_and_echoes_changes`.
- `crates/commands`: `configure` parses and validates, is `execute` and agent-visible; the state's `mode` and
  `options` serialize to the schema's members. `settings::tests` lists the two new keys.
- `crates/eludite`:
  - `shell::agents::tests::the_footer_picks_the_model_and_the_next_session_starts_with_it`: with the fake agent given
    `--options`, the footer shows "Smart ▾", "Default ▾" and "Manual ▾" left to right under the prompt box, then Send;
    the state output and the status bar (`Fake options: ready · Smart`); a click opens the model list (the current row
    highlighted), a click on `fast` runs `eludite.agents.configure` (audited), the picker says "Fast ▾", the state
    says `options[model].current == "fast"`, `agents.model` is `fast` from the user file, the status bar says `· Fast`;
    Down, Down, Enter on the effort picker picks `high` (sends no prompt) and remembers it; Escape closes the mode
    picker without cancelling anything; a second click on an open picker's button closes it; picking `max`, which the
    fake refuses, reverts the picker to "High ▾" with an error row naming the reason and leaves the setting; a
    restart starts with `model` = `fast` and `effort` = `high` (read back from the fake through the state's
    `options`), the mode back at `default`.
  - `shell::agents::tests::configure_on_the_bus_sets_the_mode_and_is_refused_during_a_turn`: from another thread,
    `{option: "mode", value: "plan"}` answers with `mode.current == "plan"` (after the agent answered) and the picker
    says "Plan ▾"; a model change from the bus writes no setting; `unknown option`, `unknown value` (also for a mode
    not offered); during a turn the command fails `busy`, the picker is muted and a click opens nothing; after the turn
    it opens again; the `stream` scenario without `--options` shows no picker, the state has no `mode` or `options`,
    the command answers `not supported`, and the status bar has no model.
  - `settings::tests::the_agents_model_and_effort_are_unset_by_default_and_set_in_the_user_file`.
  - The existing Agents tests pass unchanged with the Send button in the footer.

Commands run, from the worktree root (the adapter's from `agents/claude-acp`), all with `CARGO_INCREMENTAL=0`:
`cargo test -p eludite-acp -p eludite-commands`, `cargo test -p eludite --bin eludite agents::` (42 passed),
`cargo test -p eludite --bin eludite settings` (16 passed), `cargo fmt --check` (clean),
`cargo clippy --workspace --all-targets -- -D warnings` (clean), `cargo build --workspace` (builds), every workspace
crate's `cargo test -p` in place of `cargo test --workspace` (section 7), and in `agents/claude-acp`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test` (12 unit, 13 conformance, 5 golden).

## 3. The recorded fixture and `claude` 2.1.289's models

`agents/claude-acp/tests/fixtures/claude-2.1.289-options.jsonl` was recorded with `tools/record.py` from `claude`
2.1.289 (`/opt/node22/bin/claude`, logged in) and an MCP config with no servers, then redacted with `tools/redact.py`.
`record.py` gained a `control` entry (the text is a control request, sent instead of a user message, waiting for its
reply), so the recording makes no model call: `initialize`, `set_permission_mode` `plan` (answered `{mode: "plan"}`
and a `system` `status` message), `set_permission_mode` `default`, `set_model` `opus` (answered success with no
payload), the user message `/effort high` (a `system` `init` naming `claude-opus-5-5`, an `assistant` message with
`model: "<synthetic>"`, `local_command_run: {command: "effort", args: "high"}` and the text "Set effort level to high
(this session only): Comprehensive implementation with extensive testing and documentation", then a `result` with
`local_command: "effort"`, `num_turns: 0`, `total_cost_usd: 0`), and the exit. With `--replay-user-messages` the local
command's user message was not echoed. `redact.py` now keeps the `initialize` reply's `models` (beside 0056's
built-in commands); `/effort` refused as `default` is the adapter's own refusal and needs no `claude` traffic.

The `models` list `claude` 2.1.289 reported (in this order; no current-effort field in the reply, as verified):

| `value` | `resolvedModel` | `displayName` | `description` | effort levels |
|---|---|---|---|---|
| `default` | `claude-fable-5-1` | Default (recommended) | Fable 5.1 | low, medium, high, xhigh, max |
| `opus` | `claude-opus-5-5` | Opus 5.5 | For complex work and everyday tasks | low, medium, high, xhigh, max |
| `fable` | `claude-fable-5-1` | Fable 5.1 | For your toughest challenges | low, medium, high, xhigh, max |
| `sonnet` | `claude-sonnet-5-5` | Sonnet 5.5 | Most efficient for simpler tasks | low, medium, high, xhigh, max |
| `haiku` | `claude-haiku-4-5-20251001` | Haiku 4.5 | Fastest for quick answers | none (`supportsEffort` absent) |
| `claude-sonnet-5` | `claude-sonnet-5` | Sonnet 5 | Efficient for routine tasks | low, medium, high, xhigh, max |
| `claude-opus-5` | `claude-opus-5` | Opus 5 | Best for everyday, complex tasks | low, medium, high, xhigh, max |
| `claude-fable-5` | `claude-fable-5` | Fable 5 | Most capable for your hardest and longest-running tasks | low, medium, high, xhigh, max |
| `claude-opus-4-8` | `claude-opus-4-8` | Opus 4.8 | Best for everyday, complex tasks | low, medium, high, xhigh, max |
| `claude-opus-4-7` | `claude-opus-4-7` | Opus 4.7 | Best for everyday, complex tasks | low, medium, high, xhigh, max |
| `claude-opus-4-6` | `claude-opus-4-6` | Opus 4.6 | Best for everyday, complex tasks | low, medium, high, max |
| `claude-sonnet-4-6` | `claude-sonnet-4-6` | Sonnet 4.6 | Efficient for routine tasks | low, medium, high, max |

Every entry also carries `supportsAdaptiveThinking` and `supportsAutoMode` (Haiku neither); `opus`,
`claude-opus-5` and `claude-opus-4-8` carry `supportsFastMode`. The model picker therefore lists twelve rows, and the
effort picker drops `xhigh` for Opus 4.6 and Sonnet 4.6 and disappears for Haiku 4.5.

## 4. Timings

Against the real `claude` 2.1.289 through the adapter's debug build (a throwaway script in the scratchpad speaking
ACP on its stdio; no model call), three rounds of `session/set_config_option` model `opus`, effort `high`, model
`sonnet`, effort `low`, then `session/set_mode` `plan` and `default`:

| Request | Answered in (ms) | Budget |
|---|---|---|
| `session/new` | 687 | (brief 0016's) |
| model (`set_model`) | 40.2, 9.4, 4.2, 13.0, 4.8, 3.6 | 1 s |
| effort (`/effort`, one local command round trip) | 195.5, 142.7, 185.5, 94.2, 98.2, 116.0 | 2 s |
| mode (`set_permission_mode`) | 3.3, 2.4 | |

The effort changes produced no `agent_message_chunk`; the notifications were 12 `config_option_update` and 2
`current_mode_update`, as expected.

The footer's frame cost: not measurable against the budget here. Two measurements:

- Headless (GPUI's test window, debug build, CPU only: layout, prepaint and paint of the whole shell window with the
  Agents window shown), 5 rounds of 1,000 forced redraws, the fake agent without and with `--options` (three pickers)
  in the same build: per-round p50 1.86–2.95 ms without, 1.94–3.21 ms with; the median of the per-round p50
  differences is +0.11 ms (with the pickers computed per frame it was +0.14 ms; they are now computed when the
  options change). A throwaway test, not kept.
- `--bench-agent-stream` on Xvfb with Mesa's lavapipe (software Vulkan), 1280x800, debug build, 7 runs each, the fake
  streamer as the bench registers it versus the same fake through a wrapper adding `--options`: frame work p50 median
  67.9 ms without pickers and 78.9 ms with them, p99 median 137.8 ms and 134.1 ms. Variants to find the p50 shift
  (no tooltip, no bounds tracking, no text, no arrow, empty buttons) ranged 63 to 83 ms, inside the run-to-run spread,
  so nothing in the footer's code was isolated as the cause; software rasterization makes these numbers useless for a
  0.3 ms budget (brief 0056's were taken the same way and are not comparable either). The p99 the budget names did not
  move here. A run on the reference machine is owed.

## 5. Other ACP agents

- **The Node Claude Code adapter** (`@agentclientprotocol/claude-agent-acp` 0.85.0, from
  `crates/acp/tests/fixtures/claude-agent-acp-0.85.0-diagnostics.jsonl`): `session/new` answers `modes` with five
  modes (`default` "Manual", `acceptEdits`, `plan`, `auto`, `bypassPermissions`) and `configOptions` with `mode`
  (category `mode`, the same five), `model` (category `model`, 12 choices, current `fable` in that recording) and
  `effort` (category `thought_level`: `default`, `low`, `medium`, `high`, `xhigh`, `max`). The pickers would show the
  model, the effort and one mode picker from `modes` (the duplicate `mode` option is hidden because `modes` is given)
  with all five modes, including the three the native adapter does not offer. Picking `acceptEdits`, `auto` or
  `bypassPermissions` there would stop the shell's review of the agent's own edits (brief 0016's boundary); the window
  does not filter them, since it shows what the agent offers. A later brief could hide them by id for that adapter,
  or the policy could refuse `configure` to them.
- **Gemini CLI and Codex**: neither adapter is installed here (`gemini`, `codex`, `codex-acp` are not on PATH and no
  network fetch was made), so they were not checked. Any ACP agent that answers `modes` or select `configOptions` gets
  pickers without code for it; one that answers neither gets no footer pickers and `configure` answers
  `not supported`.

## 6. Not done

- **The screenshot and the real turn** (`linux-agents-model-picker.png`, then a turn after picking Opus whose usage
  line names `claude-opus-5-5`). `tools/agents.py` now waits for the `agents options` trace after brief 0056's shots,
  clicks `agents-option-model` (shooting `agents-model-picker.png`), clicks `agents-option-model-opus` (the picker
  rows are now tracked for `--bounds-out`), waits for `agents option model = opus`, and after the first real turn
  records the usage lines' models from `--transcript-out` (`usage_after_pick`); `tools/agents-linux.sh` passes the
  transcript and documents the step. It did not run: this machine has Xvfb and lavapipe but no XTest input (no
  `xdotool`, no python-xlib), no KWin, and no .NET host, which the driver needs for its solution and the injected
  error. The adapter's model list and the `set_model` and `/effort` round trips were checked against the real
  `claude` instead (sections 3 and 4). Nothing was faked; no screenshot was added.
- Windows and macOS: not built or run.
- `dotnet build` / `dotnet test`: the SDK is not installed here.

## 7. Notes and choices

- Files outside the brief's list: `protocol/schemas/settings.json` (the two settings must be declared there, or the
  store ignores and reports them; the brief names `crates/eludite/src/settings.rs` for their defaults, which come from
  the schema, so that file only gained a test), and `crates/commands/src/settings.rs` (its test lists every key in
  order; two lines added). `crates/eludite/src/shell.rs` was not changed: the pump of `eludite.agents.*` jobs calls
  `reply.send(outcome)` on whatever `AgentsJob::reply` is, so the deferred answer lives in `agents.rs`
  (`JobReply`). `fake_claude.rs` was not changed.
- `AgentsOutput` gained `#[allow(clippy::large_enum_variant)]`: the state grew past clippy's threshold with `mode`
  and `options`; it is built once per command.
- The `in_turn` refusal uses `invalid_request` (-32600) with the data `in_turn: ...`; ACP names no code for it.
- The adapter's model and effort options carry descriptions ("The model Claude Code uses", "How much the model thinks
  before it answers"); the brief gives none for the options themselves.
- The shell matches a configure to the agent's answer by value (the option's current value becomes the picked one);
  an agent that accepts a value but reports another leaves the caller to its 35 s wait and the picker muted until
  the next change.
- `cargo test --workspace`: one run of it filled the disk with every crate's test binaries (12 GB free at the start),
  so it was stopped and every workspace member was tested on its own instead (`cargo test -p <crate>` for all 22,
  deleting each crate's test executables after its run): all passed except three tests of `eludite` (402 of 405
  passed), all timing-dependent: `codelens_tests::typing_stays_under_the_keystroke_budget_with_200_lens_rows_on_screen`
  (13.1 ms against 8 ms) and `forge_tests::the_sign_in_dialogs_device_flow_shows_the_code_and_completes` (timed out)
  pass when rerun alone; `forge_tests::budgets_of_the_cached_list_the_large_document_and_memory` (frame p99 9.1 to
  11.1 ms against 8 ms) is the budget known to fail on this VM regardless of this change (load average 2 to 3 on 4
  cores).
