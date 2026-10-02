# Brief 0003: Legacy project load, WebForms code-behind IntelliSense

Status: done on Linux; Windows not run (see [0003-report.md](0003-report.md))
Plan reference: PLAN.md sections 3 (D4), 4.9, 6, 10 (Phase 0 item 3), 13 (risk 4)
Related ADR: ADR-0004

## Goal

Show that legacy (non-SDK) `.csproj` projects can be evaluated and analyzed on Linux with Mono MSBuild and on Windows with Visual Studio Build Tools, and that a WebForms project gives IntelliSense in its code-behind. Produce a pass/fail matrix against a small real-world corpus and a list of the evaluation gaps.

## Files in scope

- `dotnet/Eludite.Host/**` additions for design-time evaluation (new files under `dotnet/Eludite.Host/Legacy/`)
- `dotnet/Eludite.Web/**` for a minimal designer-partial-class prototype
- `corpus/legacy/**` (new): corpus manifest and fetch script. Use git submodules or a script that clones at pinned commits. Do not commit third-party source directly.
- `tools/legacy-load/**` (new): runner scripts for both OSes
- `docs/briefs/0003-report.md` (new)

Do not edit `protocol/**`.

## Contract

- Corpus: at least 5 real open-source projects with permissive or GPL-compatible licenses, recorded with SPDX id and commit in the manifest. It must include:
  - one WebForms application (`.aspx`, `.ascx`, `.master`, with `.designer.cs` files),
  - one WCF service project,
  - one multi-project .NET Framework solution of 10 projects or more,
  - one project using packages.config and one using `PackageReference` in a non-SDK project.
- Linux: locate Mono's MSBuild (`msbuild` from the Mono distribution) and load projects through it. Reference assemblies come from the `Microsoft.NETFramework.ReferenceAssemblies` packages.
- Windows: locate MSBuild from Visual Studio Build Tools (a free installer on the machine, never bundled). Use `vswhere` for discovery.
- Design-time evaluation must not run a full build. Record any case that needed one.
- WebForms: parse `.aspx.cs` against a generated partial class for the page (control fields from the markup) so that, in code-behind, completion on a server control field lists that control's members. Hand-written markup parsing is enough for the spike; it must handle runat="server" controls and a `web.config` `<pages><controls>` registration.
- The `eludite/*` vocabulary and method framing are in `protocol/schemas/host-rpc.md`. Use only what exists there and report gaps.

## Proving test

- `tools/legacy-load/run.sh` (Linux) and `run.ps1` (Windows) load each corpus project through the host's evaluator and write a JSON result per project: loaded or failed, number of source files, number of resolved references, number of Roslyn compilation errors, time taken.
- For the WebForms project, an integration test requests completion at a recorded position in a code-behind file, for a field declared only in markup, and asserts the expected members appear.
- Compare the evaluator's source file lists against a real MSBuild `-preprocess` or `-getItem` result on Windows for at least 3 projects and report differences.

## Budget

- Design-time evaluation of any single corpus project under 5 s cold, and a 10-project solution under 15 s total, on the reference machine.
- No modal dialogs or blocking prompts on any failure. Failures become diagnostics.
- Memory and shell budgets are not affected: all work runs in `eludite-host`.

## Exit criterion

1. Matrix of corpus project by OS (Linux Mono, Windows Build Tools) showing loaded or failed, with the failure reason for each failure.
2. At least 80 percent of corpus projects load with zero Roslyn compilation errors that do not also occur in the real MSBuild on Windows. Failures are classified (missing targets, COM, packages, web targets, other).
3. The WebForms code-behind completion test passes on both OSes.
4. A written list of Windows-only targets that fail on Mono, with the message the Output window should show.
5. A recommendation on whether Mono MSBuild should be bundled or located, and whether the in-house evaluator fallback is needed, with evidence.

## Out of scope

- Actually building legacy projects (only evaluating them), except to compare results on Windows.
- Automatic `.designer.cs` regeneration and byte-for-byte validation against Visual Studio. This spike only generates enough of a partial class for IntelliSense.
- IntelliSense inside `<% %>` blocks and markup completion.
- A WebForms visual designer, IIS Express, WCF tooling, `web.config` editing.
- Debugging of any kind, NuGet restore UI, and anything on macOS.
- Committing third-party corpus sources into this repo.
