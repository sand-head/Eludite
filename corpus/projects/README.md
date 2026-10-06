# corpus/projects

Project files, launch settings and solutions for brief 0049's project property pages, launch profiles and
configuration tests, and brief 0057's Visual Basic project (MIT, written for the tests). The host's tests copy this
folder to a temporary directory and edit the copies; nothing here is built (`VisualBasicTests` restores its copy of
`VisualBasic/` so Roslyn can load it).

| Entry | What it exercises |
|---|---|
| `Console/Console.csproj` | An SDK console project with comments (one inside a property group), blank lines, a `Release\|AnyCPU` group written with Visual Studio's spaced condition, and `Properties/launchSettings.json` with an unknown member, an environment table out of alphabetical order and an Executable profile |
| `Tabs/Tabs.csproj` | An XML declaration, a byte order mark, CRLF line endings, tab indentation with one element indented oddly, and a `'$(Configuration)'=='Debug'` group |
| `Multi/Multi.csproj` | A multi-targeted project (`net8.0;net10.0`) with a `'$(TargetFramework)' == 'net8.0'` group |
| `Inherited/` | `Directory.Build.props` (LangVersion, Nullable, TreatWarningsAsErrors, Authors) inherited by `Lib/Lib.csproj` |
| `Empty/Empty.csproj` | An SDK project without a property group (the edit creates one) |
| `Web/` | An ASP.NET Core project with Visual Studio's template `launchSettings.json`: `iisSettings`, `http`, `https` and `IIS Express` profiles |
| `Legacy/Legacy.csproj` | A legacy (non-SDK) project, shown read-only |
| `VisualBasic/VisualBasic.vbproj` | A Visual Basic SDK console project (`net10.0`, `Option Strict On`) whose `Program.vb` has a class with a property and a method, a module with `Sub Main`, and one deliberate `Dim n As Integer = "x"` (BC30512); brief 0057's `VisualBasicTests` opens it through the host with the patched Roslyn server (`tools/roslyn-pin`) |
| `Corpus.sln` | The five SDK projects with a byte order mark and CRLF, platforms `Any CPU` and `x64`, `Lib` not built in Release |
| `Corpus.slnx` | The same in `.slnx` form, `Lib` in a solution folder with a `<Build Solution="Release\|*" Project="false" />` rule |
| `Legacy.sln` | The legacy project alone |

Expected outputs are in the tests (`dotnet/tests/Eludite.Host.Tests/ProjectProperties*.cs`, `LaunchProfiles*.cs`,
`SolutionConfigurations*.cs`): every catalog property read and written, edits that change exactly one element, the
condition and inherited rules, launch profile order and unknown members, and `.sln` and `.slnx` mapping edits.
