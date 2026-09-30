# ADR-0005: GPL core, MIT edges

Status: Accepted, 2026-10-01
Plan reference: PLAN.md section 2 (principle 7), section 3 (D5), section 14 (item 1)

## Context

The owner prefers copyleft: improvements to the IDE should stay public. Copyleft also makes Zed's GPL-3.0 crates legally available (ADR-0001), which the permissive-only v0.1 plan had ruled out.

At the same time, people who write an ACP agent, an MCP client, an alternative language host or a WASM extension against Niello must be able to do so under any license. A GPL core must never become a reason someone cannot build on it. Zed makes the same split: Apache extension API, GPL editor.

The project is run by one person and agents, so the contribution process must be light.

## Decision

- Shell, hosts, debuggers and web tooling: GPL-3.0-or-later. The root `LICENSE` is the GPL text.
- `protocol/` (schemas and generated bindings, crate `niello-protocol`) and `extension-sdk/` (crate `niello-extension-sdk`): MIT. Each carries its own license file and `license = "MIT"` in its manifest.
- Dependencies must be compatible with the directory they land in. GPL-3.0 can include MIT, Apache-2.0 and BSD code (Roslyn, netcoredbg, GPUI, libgit2, tree-sitter, wasmtime). The MIT directories may only depend on MIT-compatible code.
- Each new dependency's SPDX id is recorded in the PR description (CLAUDE.md).
- Contributor terms: Developer Certificate of Origin, no CLA. Agent output is contributed under the same terms by the human who directs the agent.
- AGPL is not adopted now. It only matters for a network service such as an extension registry or collaboration server. Decide when such a service exists.
- Trademark: "Niello" and the logo are held separately from the code, so forks can exist without confusion.

## Alternatives considered

- Permissive (MIT or Apache-2.0) for everything: maximal adoption, but anyone could close-source a fork, and we would be barred from Zed's GPL crates.
- AGPL-3.0 for everything: closes the network-service loophole, but there is no network service today, and it complicates embedding for little gain.
- GPL everywhere including the protocol and SDK: would force every third-party agent or extension author to be GPL. That contradicts principle 7.
- A CLA with a relicensing grant: keeps future options open, but adds friction and trust cost. DCO is enough for now.
- Dual licensing or a commercial edition: no plan for it, and it would require a CLA.

## Consequences

Positive:
- Improvements to the product stay public.
- Zed's GPL-3.0 crates may be vendored, subject to the per-crate audit.
- Third parties can write extensions, agents and alternative hosts under any license.
- DCO sign-off is trivial to enforce in CI and carries no copyright assignment.

Negative:
- GPL discourages some commercial adopters and embedders.
- Two license regimes in one repository require care: dependency checks must know which directory they apply to.
- Without a CLA, relicensing later needs consent of every contributor or a rewrite of their code.
- Some extension authors may want to link against core crates and cannot unless they accept the GPL.

## Revisit when

- A network service (extension registry, collaboration server) is planned. Evaluate AGPL for that service.
- A needed dependency has a license that is incompatible with GPL-3.0-or-later.
- Outside contribution volume makes relicensing flexibility valuable enough to reconsider a CLA.
- Before the first release: check the trademark, crates.io names, the GitHub org name and a domain (PLAN.md section 14, item 6).
