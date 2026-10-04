# Proposals

A proposal is a design for a feature that PLAN.md names but does not yet detail, or that is not in PLAN.md at all. It is the step between an idea and briefs: it fixes the scope, the command surface, the schemas, the process model and the sequence of briefs, so each brief can be written precisely enough to delegate (see [../briefs/README.md](../briefs/README.md)).

A proposal is accepted when the owner says so. Acceptance produces, in the same change: the PLAN.md edits the proposal lists under "Changes to PLAN.md", the ADRs it names (moved from Proposed to Accepted), and the first brief. Nothing in a proposal is binding until then; CLAUDE.md's rule that features absent from PLAN.md are not built still applies.

## Index

| Proposal | Title | Status |
|---|---|---|
| [0001](0001-agent-debugging-suite.md) | The agent debugging suite | Accepted 2026-10-02; first brief [0022](../briefs/0022-mono-debug-adapter.md) (E1) |
| [0002](0002-web-browser-window.md) | The Web Browser window and agent control of it | Accepted 2026-10-02; first briefs [0023](../briefs/0023-browser-automation-read.md) and 0024 (A, in two halves) |
| [0003](0003-full-dotnet-framework-off-windows.md) | The full .NET Framework on Linux and macOS: the real framework under Wine, a Windows guest, Mono as the fallback, `eludite-webhost` | Proposed 2026-10-04; spike done on Linux; ADR-0011 proposed |
