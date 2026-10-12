# ADR-0015: The Roslyn language server is built from source by CI and shipped beside the host

Status: Accepted, 2026-10-12 (the owner's direct request for an alpha that works out of the box; brief 0080)
Plan reference: PLAN.md sections 2 (principles 2, 6), 3 (D2, D3), 4.3, 13

## Context

C# and Visual Basic IntelliSense come from Roslyn's language server (`Microsoft.CodeAnalysis.LanguageServer`, MIT),
which `eludite-host` runs as its child (ADR-0002) and proxies over LSP. Brief 0002 pinned a commit of dotnet/roslyn
and built the server from source (`tools/roslyn-pin`); brief 0063 added `vb.patch`. Until now the server was
"located on the machine, never packaged", like netcoredbg and rust-analyzer: the host looked at `--roslyn-ls`,
`ELUDITE_ROSLYN_LS` and the build script's output folder. No release ships that folder and the build needs a clone of
Roslyn, its own SDK and several minutes, so every downloaded Eludite showed "C#: unavailable" and the developer
hint lived only in Output > Host. CI never built the server either, so the host's Roslyn tests skipped there.

Unlike netcoredbg or rust-analyzer, there is no upstream release of this server to fetch by checksum that carries our
patch: the package Microsoft publishes for VS Code's C# extension, on its own feed, is built without Visual Basic in
its composition (brief 0063's Spike 2) and moves with that extension rather than with our pin. The server is
framework-dependent and started as `dotnet <dll>`, so one build runs on every platform the host runs on.

## Decision

- CI builds the server once per pin on Ubuntu (`roslyn` job: `tools/roslyn-pin/build.sh`), cached by the hash of
  `COMMIT`, every `tools/roslyn-pin/*.patch` and the build scripts, and hands the output folder to the other jobs as
  the artifact `roslyn-server`.
- The packaging scripts lay it out as `eludite-host/roslyn/` in every archive and the MSI, with Roslyn's
  `License.txt` and `THIRD-PARTY-NOTICES.txt` in `licenses/roslyn/`. It is versioned with the release: a pin bump is a
  normal change that the next release carries.
- The host looks for it in this order: `--roslyn-ls` (the shell passes the setting `languageServers.roslynPath`),
  `ELUDITE_ROSLYN_LS`, `roslyn/` beside the host, then the build script's output. Explicit choices win so a person
  can try another server; a named path that does not exist is reported, not replaced.
- The rule "pinned external tools are located, never vendored" (CLAUDE.md, `tools/`) still holds for everything else:
  this is a build output of a pinned source, like the CEF runtime files `tools/package/linux.sh` lays out (ADR-0008).

## Alternatives considered

- Keep it located and tell the person to run `tools/roslyn-pin/build.sh`: needs a repository checkout, git, several
  GB and minutes; no IDE asks that of its users.
- Download it on first use from our own release: a network call the person did not ask for, and a second artifact
  per release to keep in step with the host.
- The server package published for VS Code's C# extension: no Visual Basic, and versions chosen by that extension.
- Build per platform: the output is portable (`dotnet <dll>`), so three builds would only cost runner time.

## Consequences

Positive:
- A downloaded archive or the MSI serves C# and VB completion, diagnostics and navigation with nothing else installed
  but the .NET 10 SDK; the packaged smoke test proves it on CI.
- CI's real-host and Roslyn tests run instead of skipping (`ELUDITE_REQUIRE` names `roslyn` and `host`).

Negative:
- The archives grow by about 140 MB unpacked (about 50 MB compressed); the self-updater downloads it with each build.
- A cold `roslyn` job costs about 6 minutes of CI; a cache hit costs one download.
- Eludite now redistributes Roslyn: its MIT notice and third-party notices must ship with every archive.

## Revisit when

- Roslyn publishes a server package with Visual Basic and redistribution terms that allow shipping it.
- The updater gains per-component archives (ADR-0011), so an unchanged server is not downloaded again.
