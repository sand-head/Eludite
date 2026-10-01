# Brief 0003 report: legacy project load, WebForms code-behind IntelliSense

Status: Linux done; Windows not run on this machine (none available). Spike code, not production.
Branch: `brief/0003-legacy-project-load`. Date: 2026-10-01.

## Summary

- **Mono's MSBuild works without root.** Arch's `mono` 6.12.0 and `mono-msbuild` 16.10.1 packages, extracted into
  `~/.local/opt/mono-root`, evaluate legacy projects once four environment variables are set. eludite-host finds that
  layout (and system installs) on its own.
- **Linux matrix (Mono MSBuild plus eludite-host's design-time corrections):** 27 of 29 corpus projects load in the
  Roslyn language server. 23 have zero Roslyn compilation errors. The errors in 2 more come from a broken upstream
  commit (`packages.config` bumped without updating `HintPath`s), so Windows MSBuild would hit them too. By the
  brief's rule that makes **25 of 29 (86 %)**. On Windows this is inferred from reading the project files, not
  measured.
  Without the corrections (projects loaded exactly as written), the count is 18 of 29 (62 %). Without Mono (the .NET
  SDK's MSBuild only), it is 20 of 29 (69 %).
- **WebForms code-behind completion passes on Linux**, with both Mono's and the SDK's MSBuild under Roslyn. Completion
  on a button declared only in markup lists `Text`, `OnClientClick` and `CommandName`. Completion on a label whose tag
  prefix is registered only in `web.config` `<pages><controls>` lists `Text` and `AssociatedControlID`.
- **Budgets hold.** The slowest single-project evaluation in a cold process is 2.1 s (budget 5 s). A whole solution
  in one cold Mono process takes 2.1 s for 11 projects and 2.5 s for 15 (budget 15 s). Roslyn reports every project
  loaded 4.5 s after `initialize`, or 6.8 to 7.4 s with eludite-host's preparation in front.
- **Recommendations:** **locate** Mono, do not bundle it. A **from-scratch evaluator fallback is not needed**: the
  fallback should be the .NET SDK's MSBuild in-process plus a small set of corrections. See below for both.
- Windows: **not run on this machine.** `run.ps1` and `BuildToolsInstallation` (vswhere) are written but untested.
  Every Windows column below says "not run".

## Machine

| | |
|---|---|
| CPU | AMD Ryzen 9 7940HS (8 cores, 16 threads) |
| RAM | 30 GiB |
| OS | CachyOS, kernel 7.2.8-2-cachyos, rebooted shortly before the run |
| .NET | SDK 10.0.302, runtime 10.0.10 |
| Mono | 6.12.0.206 with Mono MSBuild 16.10.1.36301, user-space extract of the Arch packages (see `tools/legacy-load/README.md`) |
| Roslyn LS | brief 0002 pin, `~/.cache/eludite/roslyn/artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/Release/net10.0/` |
| Reference assemblies | `Microsoft.NETFramework.ReferenceAssemblies.net40` through `net481`, 1.0.3 (MIT), in `~/.nuget/packages` |

**Measurement caveats.**
- "Cold" means a new process for each measurement. The page cache could not be dropped (no root), and earlier steps
  had already read the corpus and Mono, so these are not cold-disk numbers.
- The machine was shared. A game and another agent's Rust build were running during part of the Roslyn phases (load
  average 9 to 19). The evaluation phase ran before most of that load. The Roslyn load times, especially the "fixed"
  modes, are probably pessimistic, but no quiet rerun was done.

## What was built

| Path | What |
|---|---|
| `dotnet/src/Eludite.Host/Legacy/MonoInstallation.cs` | Finds Mono and its `MSBuild.dll`: `ELUDITE_MONO_PREFIX`, then `mono` on `PATH`, then `~/.local/opt/mono-root/usr`, `/usr`, `/usr/local`, the macOS framework. Supplies the environment a relocated Mono needs. |
| `Legacy/BuildToolsInstallation.cs` | `vswhere -latest -products * -requires Microsoft.Component.MSBuild -find MSBuild\**\Bin\MSBuild.exe` (Windows; untested) |
| `Legacy/CommandLineMsBuildEvaluator.cs` | Runs a located MSBuild (Mono, Build Tools, or `dotnet msbuild`) once per batch. It uses a generated traversal project and an injected `EluditeDesignTimeDump` target that runs `ResolveReferences` with `ContinueOnError` and writes Compile items, resolved references, defines, TFM, project references and markup items. It never compiles. |
| `Legacy/InProcessMsBuildEvaluator.cs` | Same through the .NET SDK's MSBuild in-process (Microsoft.Build.Locator), optionally ignoring missing imports |
| `Legacy/ReferenceAssemblies.cs` | `TargetFrameworkRootPath` from the reference-assembly packages, with a merged symlink root covering every version |
| `Legacy/DesignTimeProperties.cs` | Design-time global properties (`DesignTimeBuild`, `BuildingProject=false`, `SkipCompilerExecution`, ...) |
| `Legacy/FailureClassifier.cs` | Sorts failures into missing targets, COM, packages, web targets and other |
| `Legacy/PathCaseFixups.cs` | Finds Compile items whose letter case differs from the file on disk |
| `Legacy/WebFormsDesignerService.cs` | Generates designer partials (Eludite.Web) and writes the injected targets file: designer partials, case fixups, and `COMReference` removal off Windows |
| `Legacy/LegacyDesignTime.cs`, `SolutionProjects.cs` | Runs before Roslyn starts: evaluates the solution's legacy projects, writes the targets, and gives Roslyn's process the environment (Mono on `PATH`, `TargetFrameworkRootPath`, `CustomAfterMicrosoftCommonTargets`) |
| `Lsp/LspProxy.cs`, `Lsp/RoslynProcessLauncher.cs`, `Program.cs` | Pre-launch hook, extra child environment, `project/open` when `solutionPath` is a bare `.csproj` |
| `dotnet/src/Eludite.Web/{ControlRegistration, ControlTypeResolver, DesignerControlScanner, DesignerPartialGenerator, MetadataTypeCatalog, RegisterDirectiveParser, WebConfigControls}.cs` | Designer-partial prototype: finds server controls (skipping templates and `asp:Content`), reads `Register` directives and `web.config` `<pages><controls>` (plus the framework defaults), resolves control types against the referenced assemblies' metadata, and renders the partial class |
| `corpus/legacy/manifest.json`, `fetch.sh` | Corpus manifest (SPDX id, pinned commit) and a shallow, idempotent fetch into `.checkout/` (gitignored) |
| `tools/legacy-load/run.sh`, `run.ps1`, `runner/` | Matrix runner. Phases: restore, eval, getitem, compile, roslyn, designer. Writes a JSON file per project and per phase, plus `matrix.md`, to `results/<stamp>/` (gitignored). `run.ps1` is untested. |
| `dotnet/tests/Eludite.Host.Tests/LegacyEvaluatorTests.cs` | Unit and integration tests for the evaluators, injection, case fixups, classifier, solution parsing and designer modes |
| `dotnet/tests/Eludite.Host.Tests/LegacyWebFormsCompletionTests.cs` | Proving test for WebForms completion. It skips when the Roslyn build, the corpus checkout or the reference packages are absent. |
| `dotnet/tests/Eludite.Web.Tests/DesignerPartialGeneratorTests.cs` | Generator and resolver tests |

Two agents wrote most of this before an interruption (WIP commit). This pass verified it builds, re-measured
everything, and changed three things:
1. **The designer service supplements by default instead of replacing.** It keeps the checked-in `.designer.cs` and
   generates only the fields that file lacks. It also skips fields the code-behind declares itself, and skips pages
   whose code-behind class is not `partial` (ASP.NET 1.x style). Replace mode is still available
   (`DesignerMode.Replace`). The reason: in Replace mode, Umbraco.Web had 24 errors that the unmodified project does
   not have. Stale checked-in designers declare fields the markup no longer has, and old-style pages declare their
   controls themselves. With the change, the count is 0.
2. The runner gained the `mono-fixed` and `sdk-fixed` Roslyn modes (the host's corrections on) and merges results
   across runs.
3. `run.ps1` no longer assigns PowerShell's automatic `$args`.

Tests: `dotnet test dotnet/Eludite.slnx` runs 82 tests: 81 pass and 1 is skipped (brief 0002's 200-project
integration test, whose generated solution is not present). On this machine that includes the two WebForms
completion cases and the Mono evaluator test. On a fresh clone the corpus, Roslyn and Mono tests skip with a message.
`dotnet build dotnet/Eludite.slnx` reports 0 warnings.

New dependencies (central pins in `dotnet/Directory.Packages.props`), all MIT:
- `Microsoft.Build` 17.11.48 and `Microsoft.Build.Framework` 17.11.48: compile-time only; the SDK's copies are
  loaded at run time.
- `Microsoft.Build.Locator` 1.10.2.
- `Microsoft.CodeAnalysis.CSharp` 5.0.0: the `tools/legacy-load` runner only.

Mono itself is located, never redistributed. Its packages are MIT, LGPL and GPL mixtures, installed by the user.

## Corpus

`corpus/legacy/manifest.json`; `corpus/legacy/fetch.sh` clones each repo at depth 1 at the pinned commit (blobless
sparse checkout for the two sample repos).

| Entry | Repo @ commit | SPDX | Projects | Covers |
|---|---|---|---|---|
| webforms-changepk | aspnet/samples @ `f7d4a776` | Apache-2.0 | 1 | WebForms (`.aspx`, `.ascx`, `.master`, `.designer.cs`), packages.config |
| webforms-sqlmembership | aspnet/samples @ `f7d4a776` | Apache-2.0 | 1 | WebForms, packages.config |
| wcf-webhosted | dotnet/samples @ `acb39ceb` (`framework/wcf/.../Web-Hosted/CS/Service`) | MIT | 1 | WCF service (`.svc`) |
| sharex | ShareX/ShareX @ `fcf846cd` (last commit before "Convert project to SDK style") | GPL-3.0-or-later | 11 | 10+ project .NET Framework 4.8 solution, **PackageReference in non-SDK projects**, WinForms, resx, a COM reference |
| umbraco7 | umbraco/Umbraco-CMS @ `da6bc2bd` (tag release-7.15.9) | MIT | 15 | 10+ project solution, WebForms (32 pages with designers), packages.config, net472 |

That is 5 entries from 4 repositories and 29 projects. Every brief requirement is covered: WebForms with
`.designer.cs`, a WCF service, two solutions of 10 or more projects, packages.config, and PackageReference in a
non-SDK project. Nothing third-party is committed.

## Exit criteria

### 1. Matrix of corpus project by OS

Columns:
- **Linux Mono**: Roslyn LS through eludite-host with Mono on `PATH`, so Roslyn loads non-SDK projects with its Mono
  build host. The host's corrections are on (mode `mono-fixed`).
- **as written**: the same without corrections (mode `mono`).
- **Linux SDK**: Mono hidden, so Roslyn uses its .NET SDK build host; corrections on (mode `sdk-fixed`).
- **Windows Build Tools**: not run on this machine.

"Errors" counts Roslyn compilation errors (`CS*`, severity Error) from `textDocument/diagnostic` pulled for every
Compile item. Eval time is Mono MSBuild evaluation plus `ResolveReferences` for that project alone, in a new process.

| Project | Eval (Mono, cold) | Files / refs | Linux Mono | as written | Linux SDK | Windows Build Tools |
|---|---|---|---|---|---|---|
| changepk / PrimaryKeysConfigTest | 577 ms | 39 / 42 (3 unresolved) | loaded, 54 err (packages, upstream) | loaded, 54 | loaded, 54 | not run |
| sqlmembership / SQLMembership-Identity-OWIN | 572 ms | 32 / 33 (3 unresolved) | loaded, 21 err (packages, upstream) | loaded, 21 | loaded, 21 | not run |
| wcf / service | failed | - | **failed (missing targets: case)** | failed | failed | not run |
| sharex / ShareX | 1,543 ms | 137 / 120 | loaded, 17 err (packages: Windows SDK contracts) | 17 | 74 | not run |
| sharex / ShareX.HelpersLib | 727 ms (COM error) | 235 / 21 | loaded, 9 err (COM) | 9 | 143 | not run |
| sharex / ShareX.HistoryLib | 604 ms | 24 / 11 | loaded, 0 | 0 | 115 | not run |
| sharex / ShareX.ScreenCaptureLib | 1,082 ms | 101 / 13 | loaded, 0 | 0 | 76 | not run |
| sharex / ShareX.UploadersLib | 616 ms | 207 / 23 | loaded, 0 | 0 | 312 | not run |
| sharex / ShareX.IndexerLib | 591 ms | 14 / 10 | loaded, 0 | 0 | 7 | not run |
| sharex / ShareX.ImageEffectsLib | 611 ms | 64 / 9 | loaded, 0 | 0 | 16 | not run |
| sharex / ShareX.Setup | 560 ms | 3 / 5 | loaded, 0 | 0 | 0 | not run |
| sharex / ShareX.MediaLib | 579 ms | 28 / 13 | loaded, 0 | 0 | 0 | not run |
| sharex / ShareX.Steam | 514 ms | 6 / 6 | loaded, 0 | 0 | 0 | not run |
| sharex / ShareX.NativeMessagingHost | 580 ms | 3 / 5 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / Umbraco.Web.UI | failed | - | **failed (web targets: WebPublishingTasks)** | failed | failed | not run |
| umbraco7 / Umbraco.Web | 1,720 ms | 1,160 / 121 | loaded, 0 | 128 | 0 | not run |
| umbraco7 / umbraco.businesslogic | 1,027 ms | 39 / 25 | loaded, 0 | 2 | 0 | not run |
| umbraco7 / umbraco.cms | 1,129 ms | 147 / 24 | loaded, 0 | 26 | 0 | not run |
| umbraco7 / umbraco.interfaces | 511 ms | 28 / 6 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / umbraco.editorControls | 2,070 ms | 165 / 20 | loaded, 0 | 4 | 0 | not run |
| umbraco7 / umbraco.providers | 1,243 ms | 11 / 13 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / umbraco.datalayer | 606 ms | 44 / 12 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / umbraco.controls | 1,206 ms | 29 / 15 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / SqlCE4Umbraco | 557 ms | 11 / 11 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / umbraco.MacroEngines | 1,534 ms | 55 / 34 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / Umbraco.Core | 653 ms | 1,513 / 98 | loaded, 0 | 75 | 0 | not run |
| umbraco7 / Umbraco.Tests | 2,041 ms | 438 / 70 (1 unresolved) | loaded, 0 | 105 | 0 | not run |
| umbraco7 / UmbracoExamine | 1,225 ms | 38 / 18 | loaded, 0 | 0 | 0 | not run |
| umbraco7 / Umbraco.Tests.Benchmarks | 1,165 ms | 12 / 146 | loaded, 0 | 1 | 0 | not run |

Most projects are "loaded with warnings" in Roslyn's terms. The warnings are Visual Studio code-analysis rule sets
that do not exist off Windows (`MinimumRecommendedRules.ruleset`, `AllRules.ruleset`), and NuGet asking for a `win`
RuntimeIdentifier. They do not affect analysis.

Failure reasons and classes:

| Project | Class | Reason (Linux) | Would it happen on Windows? |
|---|---|---|---|
| wcf / service | missing targets | `<Import Project="$(MSBuildBinPath)\Microsoft.CSHARP.Targets" />`: the file is `Microsoft.CSharp.targets`, and Linux file names are case-sensitive. MSB4019 in Mono and in `dotnet msbuild`; Roslyn reports "Project does not contain 'Compile' target". | No |
| umbraco7 / Umbraco.Web.UI | web targets | `umbraco.presentation.targets` has `<UsingTask AssemblyFile="$(WebPublishingTasks)">`. That property comes from Visual Studio's `Microsoft.Web.Publishing.targets`, which Mono does not ship (MSB4022). | No (needs the VS web workload or Build Tools' web targets) |
| changepk, sqlmembership (errors, not a load failure) | packages | The upstream commit bumped `packages.config` (Microsoft.Owin 4.2.2, Newtonsoft.Json 13.0.1) without updating the `HintPath`s (2.1.0 / 2.0.0, 6.0 / 4.5.11), so `Microsoft.Owin`, `Microsoft.Owin.Security.Cookies` and `Newtonsoft.Json` do not resolve after a correct restore | Yes, by reading the project files (not verified) |
| sharex / ShareX (errors) | packages | `Microsoft.Windows.SDK.Contracts` WinRT metadata needs the `System.Runtime` facade (CS0012 in `OCRHelper.cs`), which reference resolution does not add on Linux | Probably not (not verified) |
| sharex / ShareX.HelpersLib (errors) | COM | `COMReference` IWshRuntimeLibrary. `ResolveComReference` needs `AxImp.exe`/tlbimp and the registry; eludite-host drops COM references off Windows, so code using the type library shows CS0246 | No |
| (as written) Umbraco.Core, .Web, .cms, .businesslogic, .editorControls, .Tests, .Tests.Benchmarks | other: file-name case | 9 Compile items differ in case from the files on disk (for example `LazyManyObjectsResolverBase.cs` vs `LazyManyObjectsResolverbase.cs`); the missing types cascade into 341 errors | No. Fixed by eludite-host's case fixups. |
| (Linux SDK) 7 ShareX projects | packages | The .NET SDK's MSBuild does not resolve `PackageReference` assets for non-SDK projects (Newtonsoft.Json, ImageListView and so on are missing); Mono's MSBuild does | No |

### 2. At least 80 percent load with zero errors not also present on Windows

**Linux: pass, with caveats.** 25 of 29 (86 %) with Mono plus eludite-host's corrections:
- 23 load with zero errors.
- ChangePK and SQLMembership have errors only from the upstream package/HintPath mismatch, which Windows MSBuild
  would also hit. This is judged from the project files; it was not verified on Windows.
- Failing: wcf/service (missing targets), Umbraco.Web.UI (web targets), ShareX (packages), ShareX.HelpersLib (COM).

| Configuration | Zero errors, or errors Windows also has |
|---|---|
| Mono + corrections (`mono-fixed`) | 25 / 29 (86 %) |
| .NET SDK MSBuild + corrections (`sdk-fixed`) | 20 / 29 (69 %) |
| Mono, as written (`mono`) | 18 / 29 (62 %) |
| .NET SDK MSBuild, as written (`sdk`) | 13 / 29 (45 %) |

Windows Build Tools: not run on this machine.

**Evaluator versus Roslyn.** The runner's `compile` phase feeds the evaluator's Compile items and resolved
references straight into a `CSharpCompilation`. For 25 of the 27 loaded projects, that gives exactly the same error
count as the Roslyn language server in the same configuration (`compile.json` against `roslyn.json`, as-written
modes), so Roslyn's build host and eludite-host's evaluator agree. The two WebForms samples are the exception: Roslyn
reports more errors (54 against 35, 21 against 16), all from the same unresolved OWIN and Newtonsoft references
(Roslyn reports more CS0012 occurrences).

**Compile-item comparison.** The brief asks for a comparison with real MSBuild `-getItem` on Windows; that was not
run. On Linux, an independent reference was used instead: `dotnet msbuild -getItem:Compile` with no injected targets,
given Mono's `VSToolsPath` for web projects. It produced item lists for 27 projects. Every evaluator that loaded a given
project agrees with it on all 27 (0 differences; the strict and command-line SDK evaluators cannot load the two
WebForms samples). The other 2, wcf/service and Umbraco.Web.UI, cannot be evaluated there for the reasons
above. `run.ps1` includes the Windows comparison against `MSBuild.exe -getItem:Compile` for 4 projects (ChangePK,
the WCF service, ShareX.HelpersLib and Umbraco.Core), untested.

### 3. WebForms code-behind completion test passes on both OSes

**Linux: pass (both MSBuild variants).** Windows: not run on this machine.

`LegacyWebFormsCompletionTests.CodeBehind_CompletionOnMarkupOnlyField_ListsControlMembers(mono|sdk)`:
1. Copies the ChangePK project to a temp directory.
2. Gives the "Log in" `asp:Button` an `ID="EluditeProbeButton"`, which the checked-in `.designer.cs` lacks.
3. Adds `<eludite:Label ID="EluditeProbeLabel">`, whose `eludite` prefix is registered only in `web.config`
   `<pages><controls>`.
4. Starts the real eludite-host with the Roslyn LS on the bare `.csproj`.
5. eludite-host evaluates the project (Mono: 562 ms; SDK in-process: 274 ms), generates 16 designer partials, and
   injects them.
6. The test asserts the generated `Login.aspx.g.cs` declares `global::System.Web.UI.WebControls.Button
   EluditeProbeButton` and `...Label EluditeProbeLabel`.
7. It edits `Login.aspx.cs` in memory, pulls `textDocument/diagnostic` (full semantics; see the brief 0002 report),
   and asserts no error mentions the probes.
8. It asserts completion after `EluditeProbeButton.` contains `Text`, `OnClientClick` and `CommandName`, and
   completion after `EluditeProbeLabel.` contains `Text` and `AssociatedControlID`.

Both cases together take under 10 s. The test skips with a message when the Roslyn build, the corpus checkout or the
`net45` reference package is absent. The `mono` case also skips without Mono.

Generator fidelity against the checked-in designers (runner `designer` phase):

| Entry | Pages with a designer | Exact | Fields matched with type |
|---|---|---|---|
| changepk | 16 | 15 | 42 / 43 |
| sqlmembership | 10 | 10 | 56 / 56 |
| umbraco7 | 32 | 31 | 232 / 239 |

All 8 misses are **stale checked-in designers**: fields for controls that are no longer in the markup
(`Manage.aspx` `changePasswordUserName`; 7 fields in Umbraco.Web's older copy of `editPackage.aspx`). No field
present in markup got a different type. This is why supplementing, not replacing, is the default.

### 4. Windows-only targets that fail on Mono, and the Output window message

Observed in the corpus, plus the two the plan names that the corpus did not exercise:

| Target or feature | Seen in | Failure off Windows | Output window message (proposed) |
|---|---|---|---|
| `ResolveComReference` (tlbimp, AxImp; `COMReference` items) | ShareX.HelpersLib | Mono: `Task could not find "AxImp.exe" using the SdkToolsPath ...`; .NET SDK: MSB4803 | `warning ELUDITE0101: COM reference 'IWshRuntimeLibrary' was skipped: resolving COM type libraries (ResolveComReference) runs only on Windows. Code that uses it shows errors here. Build on Windows, or reference a checked-in interop assembly.` |
| `Microsoft.Web.Publishing.targets` (`WebPublishingTasks`, Web Publishing Pipeline) | Umbraco.Web.UI | MSB4022 on `$(WebPublishingTasks)`; the project does not load | `error ELUDITE0102: Umbraco.Web.UI needs Visual Studio's web publishing targets (Microsoft.Web.Publishing.targets), which are not available on Linux/macOS. The project is shown but not analyzed. Open it on Windows with Build Tools' web workload, or remove the publishing import from design-time builds.` |
| `Microsoft.WebApplication.targets` (`$(VSToolsPath)\WebApplications`) | ChangePK, SQLMembership, Umbraco | Present in Mono (`lib/mono/xbuild/Microsoft/VisualStudio/v*`); missing in the .NET SDK (MSB4019) | Only without Mono: `warning ELUDITE0103: Web application targets (Microsoft.WebApplication.targets) not found; loading without them. Install Mono (Linux/macOS) or Build Tools (Windows) for full web project support.` |
| Windows SDK / WinRT contracts (`Microsoft.Windows.SDK.Contracts`) | ShareX | CS0012: `System.Runtime` facade not referenced | `warning ELUDITE0104: ShareX references Windows Runtime APIs (Microsoft.Windows.SDK.Contracts). They are only partly available off Windows; errors in files that use them are expected.` |
| Case-insensitive file names (imports and Compile items) | wcf/service (import), Umbraco (9 Compile items) | Import: MSB4019, the project does not load. Compile items: silently dropped. | Import: `error ELUDITE0105: service.csproj imports '$(MSBuildBinPath)\Microsoft.CSHARP.Targets', which exists only as 'Microsoft.CSharp.targets' on this case-sensitive file system.` Compile item (eludite-host already logs this): `warning ELUDITE0106: Compile item 'ObjectResolution\LazyManyObjectsResolverBase.cs' differs in case from 'LazyManyObjectsResolverbase.cs' on disk; using the file on disk.` |
| Visual Studio code-analysis rule sets (`CodeAnalysisRuleSet`) | ShareX, Umbraco | Warning: `Could not find rule set file "MinimumRecommendedRules.ruleset"` | `info ELUDITE0107: Code-analysis rule set 'MinimumRecommendedRules.ruleset' ships with Visual Studio and is not available; analyzers use their defaults.` |
| `cmd.exe` build events (`PreBuildEvent`) | ShareX.UploadersLib (creates `APIKeysLocal.cs`) | Not run at design time; the generated file is missing (no errors in this case) | On build: `error ELUDITE0108: The pre-build event of ShareX.UploadersLib is a Windows command script (cmd.exe) and cannot run on Linux.` |
| SGen (`GenerateSerializationAssemblies`) | not in corpus | (PLAN.md section 6) | On build: `warning ELUDITE0109: XML serialization assemblies (SGen) can only be generated on Windows; skipped.` |
| IIS/`aspnet_compiler` (`MvcBuildViews`, `PrecompileBeforePublish`) | not in corpus | (PLAN.md section 6) | On build: `warning ELUDITE0110: ASP.NET precompilation (aspnet_compiler) runs only on Windows; skipped.` |

The message ids are placeholders. Error List and Output wiring are protocol gaps (below). Today these appear only as
`[legacy] warning: ...` lines in the host's stderr log.

### 5. Mono: bundle or locate? Evaluator fallback?

**Locate, do not bundle.** Evidence:
- Locating works without root. The Arch packages are 63 MB compressed and 311 MB extracted, and run relocated with
  `PATH`, `LD_LIBRARY_PATH`, `MONO_CFG_DIR` and `MONO_GAC_PREFIX`, which eludite-host sets itself
  (`MonoInstallation.EnvironmentFor`). `MonoInstallation.Locate()` finds a system install (`/usr`), a `PATH` install
  and this user-space layout with no configuration.
- Mono buys a lot. With it, 25 of 29 projects pass; with only the .NET SDK, 20. All 7 ShareX projects that use
  `PackageReference` from a non-SDK project lose package resolution without Mono (743 errors instead of 26). Roslyn's own
  Mono build host (`mono Microsoft.CodeAnalysis.Workspaces.MSBuild.BuildHost.exe`, seen in `/proc` during every
  `mono` run) picks up a located Mono from `PATH` with no Roslyn changes. Mono also supplies
  `Microsoft.WebApplication.targets`.
- Bundling costs about 311 MB on disk, ties Eludite to a toolchain whose MSBuild froze at 16.10 (2021), and adds a
  native-library redistribution and update burden.
- Product shape: detect Mono at startup without blocking (`eludite/host/info` should report it). When it is missing,
  show one non-modal notice ("Install Mono for full .NET Framework project support: `sudo pacman -S mono
  mono-msbuild` / your distribution's packages") and fall back to the SDK's MSBuild. An opt-in "download Mono into
  ~/.local" action is a reasonable later convenience (user-initiated, never at startup, no network by default). It
  is not part of this recommendation.

**Evaluator fallback: needed only in a thin form.** A from-scratch MSBuild evaluator is not justified:
- Mono's MSBuild evaluated 27 of 29 projects. The two failures are a missing Visual Studio target (no evaluator
  makes it exist) and a case-mismatched import.
- When Mono is absent, the .NET SDK's MSBuild in-process with `IgnoreMissingImports` (`InProcessMsBuildEvaluator`)
  already loads 28 of 29 projects, with the same Compile items as `-getItem`.

What the fallback needs is a correction layer in front of whichever MSBuild runs:
1. Compile-item case fixups (done).
2. Case-insensitive import resolution: not done; needs a project-load hook or evaluating a shadow copy of the
   project XML.
3. COM-reference removal off Windows (done).
4. Supplementing designer partials (done).
5. PackageReference asset resolution for non-SDK projects when Mono is absent, read from `obj/project.assets.json`.
   That last one is the only real piece of "own evaluator" work.

## Protocol gaps (not edited; `protocol/**` is out of scope)

`protocol/schemas/host-rpc.md` has only `initialize`, `eludite/ping`, `eludite/host/info`, `shutdown` and `exit`.
Missing for this feature:
1. **Toolchain discovery** in `eludite/host/info`: located Mono (prefix, MSBuild version, source), Build Tools
   (vswhere result), and the reference-assembly packages found.
2. **Project-load diagnostics** for the Error List: a notification such as `eludite/project/diagnostics` with
   project, severity, code, message, class (missing targets, COM, packages, web targets, other) and solution
   generation. Today they go only to stderr.
3. **An Output window channel**: a `eludite/output` notification (pane, text) for the messages in exit criterion 4.
4. **Project-system state**: per project, a request for loaded or failed, evaluator used, Compile item count and
   reference counts (the matrix's columns), so Solution Explorer can show "(load failed)" like Visual Studio.
5. **Design-time settings** that are environment variables today (`ELUDITE_LEGACY*`, `ELUDITE_MONO_PREFIX`,
   `ELUDITE_CACHE_DIR`, designer mode) need a settings method or `initialize` options.
6. **`initialize.solutionPath` accepting a bare `.csproj`**: the host does this already; the schema says "solution".
7. **Load progress**: Roslyn's `workspace/projectInitializationComplete` is forwarded as is. A Eludite progress
   notification covering the pre-launch evaluation (2.3 to 2.6 s on the 10-plus-project solutions) is missing.

## Not covered

- **Windows, all of it:** the Build Tools column, the `-getItem` comparison against real `MSBuild.exe`, the
  completion test, `run.ps1`, `BuildToolsInstallation`. Not run on this machine.
- **True cold-disk timings:** the page cache could not be dropped without root.
- **Evaluation runs before Roslyn starts, one after the other.** That adds the batch evaluation time (2.3 to 2.6 s on
  ShareX and Umbraco) to time-to-first-semantic-result. Running it alongside Roslyn's startup, caching it keyed by
  project-file hashes, and evaluating only WebForms and case-fixup candidates is follow-up work.
- Case-insensitive import resolution (the WCF project) is not implemented; it is only diagnosed.
- Designer generation does not use `TemplateInstanceAttribute`: a template is approximated as any element named
  `*Template` except `ContentTemplate`. No mismatch was seen in the corpus.
- Package restore is assumed done (`restore` phase, Mono `msbuild -t:restore`). A user-space Mono needs `cert-sync`
  for HTTPS restore (README).

## Size of the follow-up brief

**Medium**, about the size of brief 0002's follow-up. It covers:
- promoting `Legacy/` into a cached, cancellable project-system service in eludite-host (evaluation alongside Roslyn
  startup, invalidation on project-file change, solution generation numbers);
- the protocol additions above (schema first), with the Error List and Output wiring;
- case-insensitive import resolution;
- PackageReference asset resolution for the no-Mono fallback;
- a Windows run of this matrix and of the completion test.

The last item could be a small brief of its own on any Windows machine with Build Tools: run
`tools\legacy-load\run.ps1` and fill in the Windows column.

## Reproduce

```
corpus/legacy/fetch.sh                               # pinned shallow clones into corpus/legacy/.checkout/
tools/roslyn-pin/build.sh                            # Roslyn LS (brief 0002)
# Mono without root: tools/legacy-load/README.md
tools/legacy-load/run.sh                             # all phases -> tools/legacy-load/results/<stamp>/
dotnet test dotnet/Eludite.slnx                       # includes the WebForms completion test when the above exist
```

Raw data for this report: `tools/legacy-load/results/20261001-195344/` (gitignored). That directory holds
`summary.json`, `matrix.md`, `eval/<entry>/<evaluator>/<project>.json`, `getitem.json`, `compile.json`,
`roslyn.json` and the per-entry Roslyn logs. The `mono-fixed` and `sdk-fixed` modes come from a rerun at 20:10 after
the designer change.
