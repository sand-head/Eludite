# Architecture Decision Records

An ADR records one structural decision: what was decided, why, what else was considered, and when to reopen it. Agents read the ADRs before proposing structural changes and write a new one when they make such a change. The master plan is [../PLAN.md](../PLAN.md); ADR-0001 to ADR-0007 correspond to decisions D1 to D7 in its section 3.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-shell-rust-gpui.md) | Shell is Rust on GPUI with our own component layer (D1) | Accepted |
| [0002](0002-process-topology.md) | Out-of-process topology (D2) | Accepted |
| [0003](0003-protocols.md) | Open protocols over plugin APIs (D3) | Accepted |
| [0004](0004-project-system-and-builds.md) | Project system and builds (D4) | Accepted |
| [0005](0005-licensing.md) | GPL core, MIT edges (D5) | Accepted |
| [0006](0006-extensibility.md) | WASM extensions (D6) | Accepted |
| [0007](0007-remote-capable-debuggers.md) | Debuggers are remote-capable from day one (D7) | Accepted |
| [0008](0008-embedded-browser-cef.md) | Embedded browser: Chromium through CEF, out of process, CDP as the automation substrate | Accepted |

## Rules

- Number ADRs sequentially, file name `NNNN-short-slug.md`. Never renumber.
- An accepted ADR is not edited except for its status line and a dated note under "Revisit when". To change a decision, write a new ADR and mark the old one `Superseded by NNNN`.
- Allowed statuses: Proposed, Accepted, Superseded by NNNN, Rejected.
- Keep each ADR between 40 and 90 lines. Link to PLAN.md instead of copying it.
- Write the ADR in the same PR as the change it justifies.

## Template

```markdown
# ADR-NNNN: Title

Status: Proposed | Accepted, YYYY-MM-DD
Plan reference: PLAN.md section X

## Context

The forces at play: requirements, constraints, what we knew and did not know.

## Decision

What we will do, in imperative sentences.

## Alternatives considered

- Option: why it lost.

## Consequences

Positive:
- ...

Negative:
- ...

## Revisit when

- A concrete, observable trigger.
```
