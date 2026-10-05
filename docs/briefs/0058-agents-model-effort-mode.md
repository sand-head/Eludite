# Brief 0058: Model, effort and mode in the Agents window

Status: open
Phase: 2
Plan reference: PLAN.md sections 2 (principles 1, 3, 5), 5.2, 5.3, 8
Depends on: brief 0057 (the prompt editor; this brief adds the footer under it), brief 0016 (the Agents window), brief
0006 (the native Claude Code adapter), brief 0020 (the settings store)

## Goal

The Agents window shows which model the agent is using and lets the person change the model, the effort and the
permission mode from pickers under the prompt box, the way Visual Studio's Copilot Chat window puts its model picker
under the input. Today none of that is visible: the model appears only in the per-turn usage line, after a turn, and
changing it means typing `/model opus` from memory. The pickers go through ACP's session modes and config options,
so any ACP agent that offers them gets the same pickers, and the choice is a command on the bus
(`eludite.agents.configure`), so an outer agent can make it too. The last choice is remembered in settings and applied
to the next session.

## Files in scope

- `protocol/schemas/agents-configure.input.json` (new), `agents-state.output.json` (`mode`, `options`), `agents-settings.json`
  (no change; the keys are shell settings, see below), first and alone; then `crates/commands/src/agents.rs` (`CONFIGURE`,
  the state's new fields).
- `agents/claude-acp/src/agent.rs` (`session/new`'s `modes` and `configOptions`, `session/set_mode`,
  `session/set_config_option`), `agents/claude-acp/src/process.rs` (`--effort`, the `set_model` and
  `set_permission_mode` control requests), `agents/claude-acp/src/translate.rs` (the local command's reply),
  `agents/claude-acp/src/main.rs` (`--effort`), its tests, a new recorded fixture under `agents/claude-acp/tests/fixtures/`
  (`tools/record.py`, `redact.py`; never by hand), `agents/claude-acp/README.md`.
- `crates/acp/src/protocol.rs` (`SessionModeState`, `SessionMode`, `SessionConfigOption` and its select kind,
  `SetSessionModeRequest`, `SetSessionConfigOptionRequest`, the `current_mode_update` and `config_option_update`
  readers), `crates/acp/src/client.rs` (`set_mode`, `set_config_option`), `crates/acp/src/session.rs`
  (`SessionEvent::Options`, `AgentSession::set_mode`, `set_option`), `crates/acp/src/fake_agent.rs` (the `options`
  behavior, see the proving test).
- `crates/eludite/src/shell/agents.rs` (the command, the state, the settings), `crates/eludite/src/shell/agents/window.rs`
  (the footer and its pickers), `crates/eludite/src/shell/settings.rs` (the table row), `crates/eludite/src/settings.rs`
  (the two keys' defaults), `crates/eludite/src/shell/agents/tests.rs`, `crates/eludite/src/shell.rs` only for the status
  bar's agent slot.
- `crates/eludite/tools/agents.py` and `agents-linux.sh` (one screenshot), `docs/briefs/README.md`, this file,
  `docs/briefs/0058-report.md` (new).

## Contract

### What the native adapter offers (ACP schema 1.9, crate `agent-client-protocol` 2.2.0 as pinned)

- `session/new` answers `modes` (`SessionModeState`): `currentModeId: "default"`, `availableModes`:
  `default` ("Manual", "Eludite reviews each edit and prompts as the policy says") and `plan` ("Plan", "Plans before
  making changes"). `acceptEdits`, `auto` and `bypassPermissions` are not offered: they stop `claude` from raising
  `can_use_tool`, which is how the shell holds edits for review (brief 0016's interception boundary) and applies
  `agents-policy.json`; the shell's policy, not the agent's mode, decides what runs without a prompt (PLAN.md 5.3).
- `session/new` answers `configOptions` (`SessionConfigOption[]`, each `{id, name, description?, category, type:
  "select", currentValue, options: [{value, name, description?}]}`):
  - `model` (category `model`): one option per entry of the `initialize` control reply's `models` (verified on
    2.1.289: `[{value, resolvedModel, displayName, description, supportsEffort, supportedEffortLevels, ...}]`), `value`
    as given, `name` from `displayName`, `description` from `description`. `currentValue` is the session's model as
    `session_model` chose it (`--model`, `ELUDITE_CLAUDE_MODEL`, `_meta.claudeCode.options.model`), else `default`.
  - `effort` (category `thought_level`): present only when the current model's `supportsEffort` is true; the
    options are `default` ("Default", "The model decides") followed by its `supportedEffortLevels` (`low`, `medium`,
    `high`, `xhigh`, `max` on 2.1.289), each named with its first letter capitalized. `currentValue` is the
    `--effort` the child was launched with (`--effort`, `ELUDITE_CLAUDE_EFFORT`, `_meta.claudeCode.options.effort`),
    else `default`.
- `session/set_mode` sends the `set_permission_mode` control request with `{mode}` (verified: 2.1.289 answers
  `{mode}` and a `system` `status` message) and, on success, notifies `current_mode_update`. A mode not in the list is
  `invalid_params`.
- `session/set_config_option` with `configId: "model"` sends the `set_model` control request with `{model}` (verified:
  success on 2.1.289; `set_effort` and `supported_models` are not control requests there). With `configId: "effort"`
  it writes the user message `/effort <value>` (`/effort` with no value is not sent; `default` is refused with
  `invalid_params` since `claude` has no way back to adaptive effort in a session, verified) and consumes that local
  command's `assistant` message (`local_command_run: {command: "effort"}`, `model: "<synthetic>"`) and `result` (`local_command:
  "effort"`, `num_turns: 0`), which must not become a transcript turn. Both are refused with a JSON-RPC error while a
  turn is in progress (`in_turn`): the effort path writes to the child's stdin in order with the turn's prompt, and a
  model change mid-turn would change what the person is watching. The response carries the whole `configOptions`
  list with the new `currentValue`; the adapter also notifies `config_option_update` with it. A model change
  re-computes the `effort` option's list from the new model's `supportedEffortLevels` and keeps the current effort
  when the new model supports it, else `default`.
- `--effort LEVEL` and `ELUDITE_CLAUDE_EFFORT` join `--model` and `ELUDITE_CLAUDE_MODEL` (`Config`, `Launch::args`
  passes `--effort`); `_meta.claudeCode.options.effort` is read beside `.model` (the key the Node adapter uses).
- The agent's `initialize` reply reports `agentCapabilities` unchanged; modes and config options need no capability.

### The client (`crates/acp`)

- `NewSessionResponse` gains `modes: Option<SessionModeState>` and `config_options: Option<Vec<SessionConfigOption>>`
  (unknown option kinds decode to a raw value and are not shown). `methods::SESSION_SET_MODE` and
  `SESSION_SET_CONFIG_OPTION`; `AcpClient::set_mode(session, mode)` and `set_config_option(session, id, value)`.
- `SessionUpdate::current_mode()` and `config_options()` read `current_mode_update` and `config_option_update` from
  `SessionUpdate::Other`, like `plan_entries()`.
- `SessionEvent::Options { modes, config_options }` after `session/new` and after each update; `AgentSession::set_mode`
  and `set_option` run the request on the session's thread, never on the UI thread, and report the answer as
  `SessionEvent::Options` or `SessionEvent::OptionFailed { id, error }`.

### The command and the state

- `eludite.agents.configure` (`agents-configure.input.json`): `{"option": "mode" | "<config option id>", "value":
  "<mode id or option value>"}`. It answers `agents-state.output.json` once the agent has answered (so the output shows
  the new value) or `CommandError` with the agent's message (`busy` while a turn runs, `unknown option`, `unknown
  value`, `not supported` for an agent with no such option). Agent-visible like the rest of `eludite.agents.*`; its
  permission class is `execute` under the policy's `agents` rules, since an agent changing another agent's model is
  something to prompt about.
- `agents-state.output.json` gains `mode: {current, available: [{id, name, description?}]}` and `options: [{id, name,
  description?, category?, current, choices: [{value, name, description?}]}]`, both absent when the agent offers none.
- Settings `agents.model` and `agents.effort` (user scope by default, workspace scope allowed; strings, default
  unset): the shell writes them when the person picks a value in the window (not when an agent calls the command),
  and passes them to the next `session/new` in `_meta.claudeCode.options` for the native adapter and in the same
  `_meta` for any other agent (which may ignore it). The row goes in `crates/eludite/src/shell/settings.rs`'s table.
  The mode is not remembered: every session starts in `default`.

### The window

- A footer row under the prompt box, left to right: the model picker, the effort picker, the mode picker, a spacer,
  the Send/Stop button (which moves here from beside the box). Each picker is a flat button `Name ▾` showing the
  current choice's name (the model shows `displayName`, so "Sonnet 5.5", never a model id), drawn as the agent
  picker is (`AGENT_PICKER`), with a popup list (`eludite_ui::popup::popup_panel`) of the choices, the current one
  marked, each with its description muted on the right or under it, keyboard: Up, Down, Enter, Escape. A picker is
  absent when the agent has no such option, and disabled (muted, no popup) while a turn runs, with the tooltip "Wait
  for the turn to end". Picking a value runs `eludite.agents.configure`; until the agent answers the picker shows the
  new name muted; on `OptionFailed` it reverts and the transcript gets an error row with the message.
- Element ids for the driver and tests: `agents-option-<id>` (the button), `agents-option-<id>-<value>` (a row),
  `agents-mode`, `agents-mode-<id>`.
- The status bar's agent slot (`Agents::status_text`) adds the model's name after the state: `Claude Code: ready ·
  Sonnet 5.5`.
- A change of mode or option by the agent itself (an update the person did not ask for, such as `/model` typed as a
  prompt) updates the pickers from the `config_option_update`; no transcript row beyond the agent's own reply.

### The adapter's local-command replies

- `/model X` and `/effort X` typed as prompts keep working as today and, after this brief, also update the pickers:
  the adapter recognizes a `result` whose `local_command` is `model` or `effort` and sends `config_option_update`.
  The new value is the X it wrote when X is one of the option's values (the `assistant` text is prose and is not
  parsed); otherwise the option is left as it was, and the `model` of the next `usage_update` (which the translator
  already tracks) corrects the model option after the next turn.

## Proving test

- `agents/claude-acp`: a fixture recorded from `claude` 2.1.289 (`claude-2.1.289-options.jsonl`, golden `.acp.jsonl`)
  with `initialize`, `set_permission_mode` to `plan` and back, `set_model` to `opus`, `/effort high`, `/effort` refused
  as `default`; the conformance test replays it through `fake_claude` and asserts `session/new`'s `modes` and
  `configOptions`, each `session/set_*` answer, the `current_mode_update` and `config_option_update` notifications,
  and that the `/effort` exchange produced no `agent_message_chunk`. A unit test proves `set_config_option` during a
  turn answers an error without writing to stdin.
- `crates/acp`: decoding `session/new` with and without `modes` and `configOptions`; a `config_option_update` with an
  unknown option type decodes and is not listed.
- `crates/eludite` `agents::tests` with the fake agent given `--options` (the fake answers `modes` `default` and
  `plan` and a `model` option with `fast` and `smart`, and an `effort` option; `session/set_mode` and
  `session/set_config_option` echo the value and notify): the footer shows "Smart ▾", "Default ▾" and "Manual ▾";
  picking `fast` runs the command, the state output says `options[model].current == "fast"`, the picker shows "Fast"
  and `agents.model` is `fast` in the user settings; the next session starts with `_meta.claudeCode.options.model ==
  "fast"` (the fake agent records what it received in its `session/new` and the test reads it back through the state's
  `options`); `eludite.agents.configure` with `option: "mode", value: "plan"` from the bus changes the mode picker and
  writes no setting; the command during a turn fails with `busy` and the picker is disabled; a scenario without
  options shows no pickers and the command answers `not supported`.
- Manual, one screenshot by `tools/agents-linux.sh`: the model picker open with the real native adapter's list
  (`linux-agents-model-picker.png`), then a turn after picking Opus whose usage line names `claude-opus-5-5`.

## Budget

- A picker opens on the next frame; the footer adds under 0.3 ms to the window's frame (`--bench-agent-stream`'s frame
  p99 within 5 percent of brief 0057's number).
- `session/set_config_option` for the model answers within 1 s on the real adapter (the control request is answered
  at once by `claude`); for the effort within 2 s (one local command round trip).
- No new dependency.

## Exit criterion

1. Every test above is green; `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` and `agents/claude-acp`'s own `cargo test` pass.
2. The screenshot exists and the real adapter's usage line after the pick names the picked model.
3. `docs/briefs/0058-report.md` records the two timings, the exact `models` list `claude` 2.1.289 reported, and which
   other ACP agents (the Node Claude adapter from its fixture; Gemini CLI and Codex if their adapters are at hand) offer
   modes or config options and how the pickers would look with them.
4. `docs/briefs/README.md` has this brief's row; the settings table and `agents/claude-acp/README.md` name the new
   flags and keys.

## Out of scope

- Offering `acceptEdits`, `auto` or `bypassPermissions` as modes (see the contract), or any mode that changes what
  the shell's policy decides.
- Fast mode, output styles, thinking token budgets (`set_max_thinking_tokens` is answered by `claude` but has no
  ACP surface yet), the account or subscription in the window.
- Remembering the mode; per-agent settings for agents other than the native adapter beyond passing `_meta`.
- The usage strip and the transcript's restyling (brief 0059).
