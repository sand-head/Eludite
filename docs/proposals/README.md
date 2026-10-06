# Proposals

A proposal is a design for a feature that PLAN.md names but does not yet detail, or that is not in PLAN.md at all. It is the step between an idea and briefs: it fixes the scope, the command surface, the schemas, the process model and the sequence of briefs, so each brief can be written precisely enough to delegate (see [../briefs/README.md](../briefs/README.md)).

A proposal is accepted when the owner says so. Acceptance produces, in the same change: the PLAN.md edits the proposal lists under "Changes to PLAN.md", the ADRs it names (moved from Proposed to Accepted), and the first brief. Nothing in a proposal is binding until then; CLAUDE.md's rule that features absent from PLAN.md are not built still applies.

## Index

| Proposal | Title | Status |
|---|---|---|
| [0001](0001-agent-debugging-suite.md) | The agent debugging suite | Accepted 2026-10-02; first brief [0022](../briefs/0022-mono-debug-adapter.md) (E1) |
| [0002](0002-web-browser-window.md) | The Web Browser window and agent control of it | Accepted 2026-10-02; first briefs [0023](../briefs/0023-browser-automation-read.md) and 0024 (A, in two halves) |
| [0004](0004-self-update.md) | Self-update by release channel | Accepted by the owner's direct request 2026-10-05, built in the same change: [ADR-0011](../adr/0011-self-update.md), brief [0055](../briefs/0055-self-update.md); the PLAN.md edit awaits review (0003 is on its own branch) |
| [0005](0005-resx-manager.md) | ResX Manager and the `.resx` editor: every set's cultures in one grid, missing, unused and inconsistent translations, references, changes, Excel exchange, translation by the hosted agent or a provider | Proposed 2026-10-05 |
