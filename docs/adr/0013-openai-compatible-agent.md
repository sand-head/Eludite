# ADR-0013: A first-party ACP agent over OpenAI-compatible servers, with the IDE's tools as its only tools

Status: Proposed
Plan reference: PLAN.md sections 2 (principles 2, 3, 5), 5.1, 5.2 (the pinned first-party agent), 5.3, 11

## Context

PLAN.md 5.2 pins a native first-party agent "until ACP hosting is excellent": Eludite hosts agents others build
(Claude Code through `eludite-claude-acp`, any ACP agent in `agents.json`) rather than writing its own loop. That
leaves out every model that has no agent CLI to wrap: a llama.cpp `llama-server` on the person's machine, Ollama,
vLLM and LM Studio, and the hosted APIs that speak the same OpenAI Chat Completions dialect (OpenRouter, Groq,
Together, DeepSeek, Mistral, OpenAI). They answer requests; they do not run tools. Something has to run the loop of
request, tool calls and request again, and that something is an agent. The owner lifted the pin for this one case on
2026-10-05 (brief 0060). ACP hosting is good enough to build on: the Agents window streams, renders tool calls,
prompts, holds edits for review and shows usage, and brief 0058 added the model picker over ACP config options.

The forces: an agent must not be privileged (5.2: "it uses the same ACP surface internally"); every action is a
command on the bus with a permission class (invariant 3, 5.3); agents run out of the shell's process (invariant 2);
local models have small context windows (8k to 32k) and weak tool use; keys must never land in files or logs.

## Decision

- Build the loop as its own program, `eludite-openai-acp` in `agents/openai-acp/`: an ACP agent on stdio, MIT, its
  own Cargo workspace like `agents/claude-acp/`, found beside the IDE's executable, on `PATH` or through
  `ELUDITE_OPENAI_ACP`. Zed or any ACP client can run it too.
- Give it no tools of its own. Its tools are exactly the MCP tools of the servers the client passes in `session/new`;
  in Eludite that is the endpoint every hosted agent gets (brief 0016), so every read, edit, build, test and debug
  step is an Eludite command with its class, the policy's gate, the audit and the pending-change review. The adapter
  never raises `session/request_permission`: the IDE's gate already decides each call (ADR-0009).
- Add the two file commands an agent loop needs and other agents lacked as commands: `eludite.file.read` (class
  `read`) and `eludite.file.edit` (class `edit_buffer`, one `eludite.workspace.apply_edit`), visible to every agent.
- Keep servers in `agents.json`'s `providers` (`name`, `baseUrl`, `defaultModel`, `headers`, `models`, `tools`) and
  their keys in the credential store under `provider:<name>`, handed to the adapter per launch in
  `ELUDITE_OPENAI_API_KEY`; edit them through `eludite.agents.provider_set` (`dangerous`, key redacted in the audit),
  `provider_remove` and `provider_models`.
- Fit small windows: send a core tool set plus a meta tool (`eludite-tools`) that lists and enables the rest; keep
  the system prompt under 4,000 estimated tokens; trim old tool results and summarize past 85 percent of a known
  window or after a context-length error.

## Alternatives considered

- An agent loop inside the shell: privileged by construction (it could call the bus without the MCP gate), against
  invariant 2 and 5.2, and GPL, so no other client could use it.
- Wrapping an existing open-source agent CLI (OpenCode, Aider, Goose): each brings its own file and shell tools that
  bypass Eludite's commands, review and audit, and a runtime (Node, Python) the shell does not otherwise need.
- One adapter per vendor API (Anthropic Messages, Gemini, Responses): the Chat Completions dialect already reaches
  local servers and most gateways; other dialects can follow as their own adapters if needed.

## Consequences

Positive:
- Local and hosted open models work in the Agents window with the same tools, permissions and review as Claude Code.
- The adapter is unprivileged and replaceable: it reaches the IDE only through ACP and MCP.
- `eludite.file.read` and `eludite.file.edit` help every agent, not only this one.

Negative:
- Eludite now owns an agent loop: prompt wording, compaction and the server quirks are ours to maintain.
- Small models misuse tools; the adapter can only hand their mistakes (malformed arguments, invented names) back.
- What is not built: a sandbox or container, tools of the agent's own (shell, file writes outside `eludite.*`, web
  fetch, subagents), a model router, pricing, prompt caching and parallel tool calls.

## Revisit when

- ACP gains a standard way for a client to offer models and providers that makes a dedicated adapter unnecessary.
- A second API dialect (Anthropic Messages, Responses) is wanted: decide between a sibling adapter and a dialect
  switch here.
- Users need the agent to work outside an IDE that offers MCP tools (it has none of its own).
