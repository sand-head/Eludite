# tools/roslyn-pin

The pinned Roslyn language server (`Microsoft.CodeAnalysis.LanguageServer`, MIT), built from source. `COMMIT` names
the dotnet/roslyn commit; `build.sh` and `build.ps1` clone it blob-less into `~/.cache/eludite/roslyn` (or
`ROSLYN_SRC_DIR`), check it out, apply `vb.patch`, and run Roslyn's own `build.sh` / `Build.cmd` on the server
project. Roslyn's build downloads the SDK its `global.json` pins into the clone's `.dotnet`. The scripts print the
built DLL, which `eludite-host` finds through `--roslyn-ls`, `ELUDITE_ROSLYN_LS`, or the default output under
`ROSLYN_SRC_DIR` (`dotnet/src/Eludite.Host/Lsp/RoslynProcessLauncher.cs`). `ROSLYN_CONFIGURATION=Debug` builds Debug.

```
tools/roslyn-pin/build.sh          # Linux and macOS
pwsh tools/roslyn-pin/build.ps1    # Windows (long paths must be enabled)
```

## vb.patch: Visual Basic in the server (brief 0063, PLAN.md section 4.3 Spike 2)

At the pinned commit the server's project,
`src/LanguageServer/Microsoft.CodeAnalysis.LanguageServer/Microsoft.CodeAnalysis.LanguageServer.csproj`, lists under
"Dlls we don't directly reference but need to include to build the MEF composition" one language:
`..\..\Features\CSharp\Portable\Microsoft.CodeAnalysis.CSharp.Features.csproj`. The composition is built from every
`Microsoft.CodeAnalysis*.dll` in the server's folder (`LanguageServerExportProviderBuilder.FindMefAssemblies`), so
without the Visual Basic assemblies the server knows the language by name only: `ProtocolConstants.RoslynLspLanguages`
lists C#, Visual Basic and F#, and `src/LanguageServer/Protocol/LanguageInfoProvider.cs` maps `.vb` and the language
id `vb` to `LanguageNames.VisualBasic`. Projects are dropped in
`HostWorkspace/LanguageServerProjectLoader.cs` line 291: a loaded project whose language has no
`ICommandLineParserService` in the workspace returns `null` ("the out-of-proc build host supports more languages than
we may actually have Workspace binaries for"), with the comment at line 266 naming "Loading VB projects" as the case.
A `.vbproj` therefore loads in the MSBuild build host and is then silently discarded; its `.vb` documents get no
diagnostics, hover or completion.

`vb.patch` adds one project reference next to the C# one:

```xml
<!-- Eludite (brief 0063): Visual Basic in the MEF composition -->
<ProjectReference Include="..\..\Features\VisualBasic\Portable\Microsoft.CodeAnalysis.VisualBasic.Features.vbproj" />
```

which brings `Microsoft.CodeAnalysis.VisualBasic.dll` (the compiler), `Microsoft.CodeAnalysis.VisualBasic.Workspaces.dll`
and `Microsoft.CodeAnalysis.VisualBasic.Features.dll` into the output folder, where the composition picks them up.
The scripts apply it after the checkout with `git apply --check` first; when `git apply --reverse --check` succeeds the
patch is already in the tree and is skipped; anything else fails the build loudly. `COMMIT` is unchanged by the patch,
and bumping it means re-checking that the hunk still applies.

## What the patched server exposes for Visual Basic

Measured through `eludite-host` by `dotnet/tests/Eludite.Host.Tests/VisualBasicTests.cs` on
`corpus/projects/VisualBasic` (a `net10.0` console project with `Option Strict On`), opened with
`eludite/solution/open` and `textDocument/didOpen` with the language id `vb`:

- The project loads (`[LanguageServerProjectSystem] Successfully completed load of .../VisualBasic.vbproj`); Roslyn
  logs its requests under `[Visual Basic]`.
- `textDocument/diagnostic` (and the host's warming publish) reports BC30512 on the `Dim n As Integer = "x"` line with
  the Option Strict message.
- `textDocument/hover` on a property answers VB syntax: ``Property Greeter.Name As String`` with the `'''` summary.
- `textDocument/completion` after `Console.` lists `WriteLine` (49 items).
- `textDocument/definition` from `New Greeter(...)` lands on `Public Sub New` in the same document.
- `textDocument/rename` of the property returns one `WorkspaceEdit` with the declaration and its uses (4 edits).

Build on a 4-core Linux machine, Release: 217 s for the server project after a restore (about 4 minutes wall from a
blob-less clone, including the SDK download); the output folder is 137 MB.

The "before" of Spike 2, measured with the same test against a copy of that folder with the three Visual Basic DLLs
removed (the composition the unpatched pin produces): the server accepts `project/open`, logs
`Completed (re)load of 1 project(s)` but never `Successfully completed load of ...VisualBasic.vbproj` (the loader
returned `null` at line 291), and `textDocument/diagnostic` on the `.vb` document stays empty until the test gives up
after three minutes. Nothing is logged about the dropped project.
