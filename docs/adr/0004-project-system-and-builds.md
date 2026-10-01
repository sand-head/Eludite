# ADR-0004: Project system and builds

Status: Accepted, 2026-10-01
Plan reference: PLAN.md section 3 (D4), section 6, section 4.2, section 4.4

## Context

The target workload includes 400-project .NET Framework solutions with WebForms and WCF. Those use legacy (non-SDK) `.csproj` files, which `dotnet build` cannot build. Visual Studio builds them with its own MSBuild; Rider uses Mono's MSBuild off Windows.

Two needs pull apart:
- Design-time analysis (IntelliSense, navigation) must be fast and must work on every OS, even where a real build is impossible.
- Real builds must produce the same output as the user's CI, so they must use the real MSBuild and targets.

The plan also states plainly that building legacy projects off Windows is partial: Windows-only targets (COM, SGen, some web targets) fail (PLAN.md section 6).

## Decision

- SDK-style projects: evaluate and build with the user's installed .NET SDK through the MSBuild APIs inside `niello-host`.
- Legacy projects on Windows: locate MSBuild from Visual Studio Build Tools, which is a free installer on the user's machine. Niello never ships those binaries (CLAUDE.md invariant 9).
- Legacy projects on Linux and macOS: locate or bundle Mono's MSBuild.
- Design-time analysis uses reference assemblies from the `Microsoft.NETFramework.ReferenceAssemblies` NuGet packages, so analysis works everywhere even where building does not.
- Design-time evaluation never blocks on a full build. A fast, cached evaluation pass feeds Roslyn. MSBuild runs for real only on a build command.
- Our own design-time evaluator is the fallback where Mono MSBuild cannot evaluate a project.
- Design-time problems surface as warnings in the Error List, never as modal dialogs.
- Support `.sln`, `.slnx`, `.slnf`, shared projects, multi-targeting, `Directory.Build.props` and `.targets`, `global.json` and `NuGet.config`.
- When a build target is unavailable on the current OS, fail with a clear Output message that names the target.
- Validate the evaluator against a corpus of real open-source legacy solutions, treated as regression tests.

## Alternatives considered

- Write our own MSBuild-compatible build engine: removes the Build Tools dependency, but 20 years of target semantics make it a project in itself and it would diverge from the user's CI.
- Require Visual Studio: contradicts the cross-platform goal and the Visual Studio binary rule.
- Bundle Build Tools: license and redistribution problems, and a large payload.
- Only support SDK-style projects: drops the primary audience (.NET Framework, WebForms, WCF).
- Run a full design-time build to feed Roslyn, as Visual Studio historically did: correct but slow, and the source of the "busy" banner we are avoiding.

## Consequences

Positive:
- Fast first edit: Roslyn gets inputs from the cached evaluator, not from a build.
- Real builds match what the user's tooling produces where MSBuild runs.
- Linux and macOS users can edit and analyze legacy code fully, and build most of it.
- Using the user's own SDK and Build Tools avoids redistribution problems.

Negative:
- Behavior differs by OS: Mono MSBuild cannot build everything. Users see a documented partial result.
- We maintain an evaluator fallback that must agree with real MSBuild on the corpus.
- We depend on discovering installed tools (SDKs, Build Tools, Mono), so discovery logic and clear error messages are product work.
- Mono's MSBuild is a moving target we may need to bundle and track.

## Revisit when

- Brief 0003 shows Mono MSBuild cannot evaluate a meaningful share of the corpus.
- Microsoft ships a supported cross-platform way to build non-SDK projects.
- The cached evaluator disagrees with real MSBuild on the corpus often enough to cause wrong diagnostics.
- A licensing change makes bundling Build Tools possible.
