# Brief 0059 report: an ACP agent over OpenAI-compatible servers

Status: in progress. Done on Linux (uncommitted): the adapter (phase one) and the shell (phase two). Not done: the
real-server test against the owner's `llama-server` (not run: no server here) and the two manual screenshots (the
orchestrator takes them from the recipe in section 6). Windows and macOS: not run. CI: not run (nothing pushed). The
.NET SDK is not installed here, so `dotnet build` and `dotnet test` were not run; this brief changes no .NET code.
Branch: `brief/0059-openai-compatible-agent`, on `main` with briefs 0056, 0057 and 0058's uncommitted work applied.
Date: 2026-10-05. Brief: [0059-openai-compatible-agent.md](0059-openai-compatible-agent.md).

## 1. Summary

Phase one (the adapter and the contracts) is described in `agents/openai-acp/README.md`, ADR-0012 and the schemas; it
is not repeated here beyond what the shell builds on. Phase two, the shell:

- **The commands on the bus.** `eludite.file.read` and `eludite.file.edit` (`crates/commands/src/files.rs`) and
  `eludite.agents.provider_set`, `provider_remove` and `provider_models` are registered by
  `crates/eludite/src/shell/agents.rs` right after the shell is built (a deferred call from `Agents::new`, so
  `shell.rs` is unchanged). All five are agent-visible.
- **`file.read` and `file.edit`** (`ShellFiles`, the shell's `FilesTarget`). The workspace folder and an open
  document's text (unsaved edits included) are asked of the UI thread through the agents channel (`HostMsg::Files`);
  the caller's thread waits, so on the UI thread both commands refuse with "runs off the UI thread" (agents call them
  from the MCP endpoint's threads). `file.edit` becomes one `eludite.workspace.apply_edit` invoked on the calling
  thread, under the calling agent (the caller is thread-local), so the shell holds it as a pending change; `file.edit`
  is in `REVIEWED_COMMANDS`, so the gate lets it through to review and the call waits for the person's decision, and
  its answer is amended as `workspace.apply_edit`'s is.
- **`provider_*`** (`providers::ProviderStore`, the shell's `ProviderTarget`). `provider_set` stores the key with
  `eludite_forge::credentials::Credentials` under `provider:<name>` (a `Credential` with `Family::None` and
  `SignInMethod::Token`), writes `agents.json`'s `providers` (refusing to overwrite a file that does not parse), and
  asks the UI to search the registry again; `apiKey` absent keeps the stored key, `""` deletes it. Brief 0046's rule for
  the file fallback applies: without the system's store the save fails with `store_unavailable` unless the new input
  `allowFileStore: true` (added to `agents-provider-set.input.json` and the parser) carries the person's consent, which
  the dialog asks for with a check box. `provider_remove` deletes the entry and the key. `provider_models` runs
  `eludite-openai-acp models` with the given key, else the stored one for `name`; the window runs every one of them off
  the UI thread (a `background_spawn` through `CommandRegistry::invoke`). The audit record keeps `apiKey` as
  `<redacted>` (phase one's `replace_with_redaction`).
- **The registry.** `AgentsSetup` gains `agents_file` (`agents.json`; tests a temporary one), `credentials` (the
  system's stores; tests `MemoryStore`) and `openai_adapter`. Every search (`RegistryJob`, off the UI thread) reads
  `agents.json` once: its `providers` always, its `agents` and `default` only without `agents.custom` (the bug phase one
  reported: servers were dropped when `agents.custom` was set), and the servers' keys from the credential store. A
  fixed registry (tests, the harness) keeps its entries and gains the servers after them. Searches carry a sequence
  number; an older one arriving late is dropped. A launch adds the server's key to the agent's environment
  (`with_provider_key`, `ELUDITE_OPENAI_API_KEY`) from the keys read with the last search, so the UI thread never waits
  on the keyring; the key is never in the registry, the command line shown, a log line or the state output. A server
  whose adapter is missing starts in the `error` state with "eludite-openai-acp was not found" and launches nothing.
  A model picked for a server is not remembered in `agents.model`, and a server's `session/new` carries no
  `_meta.claudeCode.options` (those settings are Claude Code's; a server starts on `defaultModel` or its first model).
- **The window.** The agent picker's list ends with "Add server…" (`agents-picker-add-server`), which opens the Add
  server dialog (`providers::ProviderDialog`, drawn by the Agents window as a modal over the shell): the presets as a
  row of options (llama.cpp, Ollama, LM Studio, vLLM, OpenAI, OpenRouter, Groq, Together, DeepSeek, Mistral, Custom; no
  xAI), the name and base URL boxes (brief 0056's `TextInput`, single-line), the key box (masked with bullets; Ctrl+V
  pastes; `TextInput` has no masked mode and `crates/editor` is not in this brief's files, so the key box is the forge
  sign-in dialog's bullet box), the hint under the presets' key needs ("No key needed", "Only if the server was started
  with one", "Required"; "A key is stored; leave empty to keep it" when editing), Test (`provider_models`: "2 models",
  "1 model", or the adapter's message) and Save (`provider_set`; closes the dialog, selects the new server). Enter
  saves, Escape cancels, Tab moves between the boxes. Renaming a server whose key is stored asks for the key again
  (the dialog never reads a key back).
- **Tools > Options > Agents** lists the servers under the Agents settings (`providers::ProvidersPage`, through the new
  `OptionsDialog::append_to_page`): name, base URL, "(key stored)", Edit (closes Options, shows the Agents window and
  opens the dialog on that server) and Remove (`provider_remove`, off the UI thread), and "Add server…".
- **Settings table.** `crates/eludite/src/shell/settings.rs` has the row for `agents.json`'s `providers`.
- **Credentials.** `crates/forge/src/credentials.rs` documents that a credential's key is any string and tests a
  `provider:<name>` key through the memory store and the consented file.
- **Packaging.** `tools/package/companions.sh` builds `agents/openai-acp` in release and installs `eludite-openai-acp`
  beside the shell with its `LICENSE` and `NOTICE` in `licenses/eludite-openai-acp/`, the same rule as
  `eludite-claude-acp`; `linux.sh --with-companions` carries it (its header says so). Checked by hand (`bash -n`, the
  `--help` text); not run (it needs `dotnet publish` first, and no .NET SDK is here). The smoke test in
  `browsers/chromium/tests/package.rs` is untouched.

## 2. Tests

- `crates/eludite`, `shell::agents::tests`:
  - `a_server_added_through_provider_set_runs_a_turn_with_eludites_tools` (builds `agents/openai-acp` once with cargo
    into its own `target/`, or skips with a message): `provider_set` from an agent caller with the adapter's loopback
    fake server (`agents/openai-acp/tests/fake_server.rs`, included by path) and key `sk-test-secret`; the audit record
    has `apiKey: "<redacted>"` and no key anywhere; `agents.json` names the server and holds no key; the memory store
    holds it under `provider:Local llama`; the registry has it with source `provider` and a command line without the
    key. Started for real (the adapter process, its MCP relay the adapter workspace's test relay to Eludite's real
    endpoint): the model picker lists the fake's two models; the adapter's `GET /models` carried `Bearer
    sk-test-secret`. A pick in the model picker reaches the agent (`set_config_option`) and is not remembered in
    `agents.model`; every chat request after it used the picked model. One prompt: `eludite-diagnostics-list` runs
    with no prompt; `eludite-terminal-send` prompts (class execute or dangerous) and is denied, and the model receives
    the refusal as the tool's result; `eludite-file-edit` (arguments split mid-token across fragments) is held as a
    pending change, the file untouched until Accept, then applied, audited as an agent's `workspace.apply_edit` with
    its edit `accepted`, and the model receives the applied answer; the turn ends `end_turn` with the final text; the
    usage strip reads 1,230 of 16,384 (the last response's prompt and completion tokens against llama.cpp's
    `n_ctx_train`). `provider_models` on a 404 server with a catalog answers `listing: "catalog"`; on a saved server
    with its stored key `server` with two models; on a url with no listing `none`. `provider_remove` deletes the
    credential and the registry drops the entry.
  - `a_server_is_added_from_the_picker_tested_and_listed_in_options`: the picker's last row opens the dialog on the
    llama.cpp preset (`http://localhost:8080/v1`, named `llama.cpp`); the OpenRouter preset fills its URL and name;
    the fake's URL typed, a key typed into the masked box; Test shows "2 models"; Save closes the dialog, the server is
    in the registry and selected, the key only in the store; Options > Agents lists it with "key stored", Edit and
    Remove; Edit opens the dialog on it (empty key box: the key is kept); Escape closes it; Remove deletes it and its key
    and the page's rows follow.
  - `a_server_without_the_adapter_is_in_the_error_state`: with no adapter, Start answers "eludite-openai-acp was not
    found", the state is `error` with that message, no session; a key set then deleted with `""`; Remove; removing
    again answers "no server named".
  - `providers_are_kept_when_agents_custom_is_set`: `search_registry` with `agents.custom` keeps the file's servers
    and drops its agents; without it both come; a malformed `agents.json` is reported and adds no server.
  - `file_read_serves_an_open_documents_unsaved_text_off_the_ui_thread`: from an agent's thread, `file.read` answers
    the open editor's unsaved line (`source: "buffer"`), a closed file from disk; on the UI thread it refuses; both
    commands are agent-visible and `file.edit` is reviewed.
  - `providers::tests`: the eleven presets in order with no xAI, their URLs and key hints, `preset_for`; the Test
    summary ("2 models", "1 model", the message); `ProviderKeys`' `Debug` hides keys.
- `crates/commands`: phase one's `files::tests` cover `file.read` on an open buffer with unsaved text, a line range,
  the 2,000-line and 256 KB caps, a binary file, outside the workspace, and `file.edit` with zero, one and two
  occurrences and `replaceAll` (no gap found); `agents::tests` gains the `allowFileStore` consent parse.
- `crates/forge`: `credentials::tests::a_key_that_is_not_a_host_round_trips`.

Commands run, from the worktree root with `CARGO_INCREMENTAL=0` (debug builds only):

| Command | Result |
|---|---|
| `cargo build -p eludite` | builds, no warnings |
| `cargo test -p eludite --bin eludite` | 415 passed, 2 failed: `forge_tests::budgets_of_the_cached_list_the_large_document_and_memory` and `git_tests::a_thousand_changed_files_draw_in_a_frame`, the frame-budget tests that fail on this VM regardless (the third, codelens keystroke, passed this time) |
| `cargo test -p eludite --bin eludite -- shell::agents` | 54 passed |
| `cargo test -p eludite-commands -p eludite-acp -p eludite-forge` | all passed |
| `cargo fmt --check` | clean |
| `cargo build --workspace` | builds |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test -p eludite-mcp` and `-p` each other member in turn | all passed (protocol 48, browser 58, dap 89, lsp 76, docking 39, ui 41, editor 102, git 50, search 17, terminal 43, update 31, workspace 19, extensions 2, extension-sdk 2, cdp-generator 6, dbg-netfx 19, chromium 32, mcp 29) |
| `cd agents/openai-acp && cargo test` | all passed (40 tests; `real_server` skipped: no `ELUDITE_LLAMA_SERVER`) |
| `cd agents/openai-acp && cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` | clean |

## 3. Budgets

The adapter's budgets are phase one's (its `examples/bench.rs` against the release build); they were not re-run in
phase two, where only debug builds are allowed on this VM. Phase one's binary is about 3.5 MB stripped (under 6 MB).
The shell's side adds no work per frame: the registry search, the key reads and every provider command run off the UI
thread; a streamed turn from this adapter goes through the same session channel and batching as brief 0058's, so the
frame p99 during a streamed turn was not measured separately (the reference-machine run owes it with the screenshots).
In the headless test the whole four-request turn (three tool rounds, a denial and a review) ran in 0.84 s with the
debug adapter.

## 4. The real server, models and presets

- The real-server test (`ELUDITE_LLAMA_SERVER=URL cargo test --test real_server` in `agents/openai-acp`): **not run: no
  server here**. The exit criterion's model, quantization, tool rounds for the file-read task and what a small model got
  wrong are owed by the owner's run.
- Presets tried for real: none (no network servers were contacted; llama.cpp is the owner's run). Every preset's URL
  is checked by `providers::tests` against the brief; llama.cpp's flow (no key, `GET /models` with `meta.n_ctx_train`,
  the stream) is what the fake server reproduces.
- How the adapter answers a small model's mistakes is phase one's (malformed `arguments` and unknown tool names go
  back as the tool's failure; see `agents/openai-acp/README.md`).

## 5. Final checks

All in the table of section 2. The test executables were deleted from `target/debug/deps` after each crate, and
`agents/openai-acp/target/` after its run (the shell's provider tests build it again on demand).

## 6. Screenshot recipe (Xvfb)

Build: `cargo build -p eludite` and `(cd agents/openai-acp && cargo build --bins)` (debug is enough; the driver below
starts its own fake server, so nothing else is needed). Then:

```
OUT=/tmp/shots-0059; mkdir -p "$OUT/config"
export ELUDITE_CONFIG_DIR="$OUT/config" ELUDITE_TRACE_LSP=1
export ELUDITE_OPENAI_ACP="$PWD/agents/openai-acp/target/debug/eludite-openai-acp"
KEEP=1 SETTLE=15 crates/eludite/tools/xvfb-linux.sh "$OUT" --reset-layout --folder "$PWD" \
  --bounds-out "$OUT/bounds.json" --transcript-out "$OUT/transcript.json"
TITLE=$(DISPLAY=:99 xdotool search --name '^Eludite' getwindowname | head -1)
DISPLAY=:99 SHOT_X11=1 python3 crates/eludite/tools/agents.py --openai --title "$TITLE" \
  --log "$OUT/eludite.log" --bounds "$OUT/bounds.json" --shots "$OUT" > "$OUT/openai.json"
```

(`xvfb-linux.sh` passes `--no-persist` and writes eludite's stdout and stderr, where the `[lsp] MS ...` trace lines
go, to `eludite.log`; the driver's `--title` must be the window's exact title, hence `TITLE`.) The driver writes:

- `agents-openai-add-server.png`: Ctrl+\, Ctrl+C shows the Agents window; a click on `agents-picker` opens the agent
  list, a click on its last row `agents-picker-add-server` opens the dialog (`agents-provider-dialog`) on the
  llama.cpp preset: name `llama.cpp`, base URL `http://localhost:8080/v1`, key box "No key needed".
- `agents-openai-test.png`: the base URL box (`agents-provider-url`) replaced by the fake's
  `http://127.0.0.1:PORT/v1`, then `agents-provider-test`: the message `agents-provider-message` says "2 models"
  (trace line `agents provider test 2 models`).
- `agents-openai-turn.png`: `agents-provider-save` (trace `agents registry ..., llama.cpp`; the server is selected),
  `agents-start` (trace `agents options model=llama-3.1-8b-instruct-q4_k_m`), the prompt "List the current errors
  with your tools, then say in two sentences what you found"; the fake calls `eludite-diagnostics-list` (no prompt:
  class read) then streams its answer over about 6 s; the shot is 2 s after the tool call: the tool card, the
  streaming answer, the status line and the usage strip (the first response's 6,144 tokens of the fake's 32,768-token
  window).
- `agents-openai-done.png`: after the turn (trace `agents turn ended`).

By hand with xdotool instead: every element above is in `$OUT/bounds.json` (`[x, y, w, h]` in window coordinates);
the config directory holds only `agents.json` after Save:
`{"providers": [{"name": "llama.cpp", "baseUrl": "http://127.0.0.1:PORT/v1"}]}` (no key: the llama.cpp preset needs
none, so no credential store is needed on the Xvfb display). To start with the server already saved, write that file
before starting eludite, then click `agents-picker` and the server's row `agents-agent-N` (registry order: the Claude
Code entries found on the machine first, then the servers; a click on a row starts it), or select it and click
`agents-start`. Element ids: `agents-picker`, `agents-agent-N`, `agents-picker-add-server`, `agents-provider-dialog`,
`agents-provider-preset-0` (llama.cpp) to `-10` (Custom), `agents-provider-name`, `agents-provider-url`,
`agents-provider-key`, `agents-provider-test`, `agents-provider-save`, `agents-provider-cancel`,
`agents-start`, `agents-prompt`, `agents-usage` (all in `bounds.json`); `agents-provider-message`,
`agents-status` and the Options page's `options-providers`, `options-provider-edit-N`, `options-provider-remove-N`,
`options-provider-add` are debug selectors only (the headless tests'), not in `bounds.json`.

Against the owner's `llama-server`: add `--openai-url http://localhost:8080/v1 --shot-name linux-agents-openai-llama`
(the mid-turn shot is the brief's `linux-agents-openai-llama.png`). In the nested KWin run, `tools/agents-linux.sh OUT`
does the same as step 2c (`LLAMA_URL=...` for the real server; `SKIP_DRIVE=1 SKIP_POLISH=1 SKIP_BENCH=1` for this step
alone); it copies the resulting `agents.json` to `OUT/agents-openai.json`.

## 7. Files outside the brief's list, and what is left

- None edited. Registering the five commands needed no change to `crates/eludite/src/shell.rs` (they are registered
  from `agents.rs`, deferred until the shell exists). `tools/package/shell.sh` and `tools/package/README-companions.in`
  (not in the list) still name only `eludite-claude-acp` in their text; `shell.sh` calls `companions.sh`, so its
  archives carry `eludite-openai-acp` too. The driver changes (`crates/eludite/tools/agents.py`,
  `agents-linux.sh`) are the brief's manual step, as the orchestrator asked.
- The `ProviderTarget` input `allowFileStore` is new in `agents-provider-set.input.json` (in the brief's list): the
  consent the 0046 rule needs had no field.
- Left: the real-server run and the two screenshots (owner); the frame p99 during a streamed turn on the reference
  machine; Windows and macOS.
