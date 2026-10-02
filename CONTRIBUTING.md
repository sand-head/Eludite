# Contributing

Niello is built by one person directing agents, but outside contributions are welcome. Read [CLAUDE.md](CLAUDE.md) first; it is the rulebook for humans and agents alike.

## Picking work

- Work is issued as briefs in `docs/briefs/`. Open briefs are listed in `docs/briefs/README.md`.
- Comment on or open an issue naming the brief before starting, so two people do not take the same one.
- One brief per branch and worktree. Name the branch `brief/NNNN-slug`.
- If a brief cannot be satisfied as written, say so instead of widening it.

## Pull requests

- Meet the definition of done in CLAUDE.md: green CI on every OS the job covers, a test for every behavior change, no benchmark regression over 5 percent, an ADR for structural decisions, and the crate map and README kept current.
- State the SPDX license id of any new dependency in the PR description.
- Spike briefs produce throwaway code plus a report; production code lands through a later brief.

## Commit messages

One line, plain, direct, active voice. No body, no trailers, no co-author or sign-off lines. Say what the commit does, for example `Add schemas for the diagnostics.list command` or `Merge brief 0003`.

## Licensing

- The product (shell, hosts, debuggers, web tooling) is GPL-3.0-or-later. `protocol/`, `extension-sdk/` and `agents/claude-acp/` are MIT.
- By submitting a contribution you agree it is licensed under the license of the directory it lands in. There is no CLA.
