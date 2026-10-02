# ADR-0009: Per-call permission escalation on the command bus

Status: Accepted, 2026-10-03
Plan reference: PLAN.md sections 2 (principles 1, 3), 5.1, 5.3, 5.4; proposal 0002 sections 4.3 and 5; proposal 0001 brief C

## Context

Every command declares one permission class in its spec (PLAN.md 5.3): read, edit_buffer, execute or dangerous. The MCP boundary applies that class to each agent call, the audit log records it, and `tools/list` advertises it so a model knows what may prompt. The class is a property of the command, stable across calls, and the schemas, the tool list and the policy rules are built on that.

Some commands are not one class. `eludite.browser.navigate` to `http://localhost:5000` is ordinary work; the same command to `https://bank.example` sends the agent to an origin nobody allowed, which is the external network the dangerous class exists for. `eludite.browser.upload` of a file in the workspace is execute, of `~/.ssh/id_rsa` it is dangerous. `eludite.browser.storage` with `get` is a read and with `clear` deletes data. `eludite.debug.attach` (proposal 0001 brief C) to a child of the IDE is not the same as to an unrelated process. The difference is in the input and in the solution's policy (`agents-policy.json`'s `browser` object), not in the command's id.

Splitting each such command in two (`navigate` and `navigate_external`) doubles the surface the model must learn, and the model would choose the lenient one. Declaring them all dangerous makes every localhost click prompt, which trains people to Always Allow everything.

## Decision

- Keep `CommandSpec.permission` as the declared class: what the schemas, `tools/list` and the policy's class defaults are built on. It is the floor of every call.
- Let a command register an escalation hook with its handler: `Fn(&Value, &PolicyView) -> Option<Escalation>`, evaluated per call from the input and the current policy. `PolicyView` is all a hook may read: the solution's `agents-policy.json`, the workspace folder and its launch urls, loaded lazily so a hook that needs none of it costs nothing.
- An escalation raises the class (with a short reason, such as `navigate off the allowed origins: https://example.com`) or refuses the call for an agent because the policy denies it outright (`browser.evaluate: deny`). It never lowers the class: a raised class at or below the spec's is ignored.
- Compute the effective class once per call, at the MCP boundary, before the gate decides; the gate, the permission prompt and the audit entry use that class and reason, and the call runs with it (`CommandRegistry::invoke_as`). `invoke_audited` computes it for every other caller, so every call is audited with its effective class.
- An escalated call is not allowed by a rule written for the tool's ordinary calls unless the hook says rules apply; the hook says what Always Allow remembers: a tool rule, an origin added to `browser.origins`, or nothing (allow once).
- Advertise the possibility: an input schema documents when a call escalates in `x-eludite-escalates`, the spec exposes it as `escalates` (`command-spec.json`), and `tools/list` sends it as `_meta` `eludite/escalates`. The tool's class in `tools/list` stays the declared one.

## Alternatives considered

- Separate commands per class: a larger tool surface, and the model picks the lenient one.
- The highest class for the whole command: every local action prompts; Always Allow becomes the habit.
- Checks inside each handler: the gate and the audit would not know the class, the prompt could not show the reason, and every command would re-implement policy reading.
- A class computed by the gate (the shell) per command id: policy knowledge leaks out of the command layer into each host (the shell, the stdio MCP server, later Slack), and each would drift.

## Consequences

Positive:
- One mechanism for every input-dependent class: the browser commands now, `debug.attach` next.
- The audit log says what a call actually was (class and reason), not only what its command may be.
- Local work stays prompt-free while the dangerous variants still prompt.

Negative:
- The class of a call is no longer readable from the tool list alone; models see `eludite/escalates` instead.
- A hook runs on every call of its command; it must stay cheap and must never block on the UI.
- Hooks cannot see state outside the input and the policy (the active tab's url): `open_external` without a url is judged by its declared class.

## Revisit when

- A hook needs state beyond the input and `PolicyView`, such as the target of a running session.
- Agents or MCP clients start treating `_meta` `eludite/escalates` as insufficient and ask for per-call class previews.
