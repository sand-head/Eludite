# Contributing to Niello

Humans and agents are both welcome. Niello is pre-alpha, so the most useful contribution is a finished brief.

## Before you start

- Read [docs/PLAN.md](docs/PLAN.md) for scope and [CLAUDE.md](CLAUDE.md) for the rules every contributor follows, including agents.
- Read the ADRs in [docs/adr/](docs/adr/) that touch the area you will change.
- Run `git fetch origin` and work from the current `main`.

## Picking a brief

- Briefs live in [docs/briefs/](docs/briefs/). Each one states its goal, files in scope, contract, proving test, budget, exit criterion and out-of-scope list.
- Open an issue or comment on an existing one saying which brief you are taking, so two people do not build the same thing.
- Use one git worktree per brief and stay inside the files the brief lists.
- If you want to do work no brief covers, open an issue first. Work that cannot be stated as a brief is not ready to be delegated.

## Sign-off (Developer Certificate of Origin)

Every commit needs a `Signed-off-by` line, added with `git commit -s`:

```
Signed-off-by: Your Name <you@example.com>
```

This certifies the Developer Certificate of Origin 1.1 (https://developercertificate.org). There is no CLA. If an agent wrote the code, the human who directed it signs off and takes responsibility for it.

## Pull request expectations

The full checklist is the "Definition of done" in [CLAUDE.md](CLAUDE.md). In short:

- CI is green on all supported OSes.
- Behavior changes come with tests.
- Benchmarks do not regress by more than 5 percent.
- Structural decisions come with an ADR.
- New dependencies list their SPDX license id in the PR description.
- README and CLAUDE.md still match the repo.

Prefer a few well-tested PRs over many small ones. Review time is the bottleneck.

## License of contributions

- Contributions to `protocol/` and `extension-sdk/` are licensed MIT.
- Everything else is licensed GPL-3.0-or-later.
- Do not add code you cannot license under those terms. Vendored code keeps its upstream license and is recorded in a `WHY.md` under `vendor/`.
