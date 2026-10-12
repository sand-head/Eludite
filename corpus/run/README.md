# corpus/run

Projects for brief 0074's run target and publish tests (MIT, written for the tests). The host's tests copy this folder
to a temporary directory and build or publish the copies; nothing here is built in place (`.gitignore` keeps the
outputs out if it is).

| Entry | What it exercises |
|---|---|
| `Artifacts/Directory.Build.props` | `UseArtifactsOutput` (outputs under `Artifacts/artifacts/bin/<project>/<configuration>/`), `AssemblyName` `Corpus.$(MSBuildProjectName)` and `RunWorkingDirectory` `Artifacts/work/`, all set outside the project file |
| `Artifacts/App/App.csproj` | An SDK console project (`net10.0`) with no `AssemblyName` of its own: its `TargetPath` is `artifacts/bin/App/debug/Corpus.App.dll` in Debug; `Program.cs` prints its assembly name and current directory |
| `Artifacts/work/` | The run working directory (`.gitkeep` only) |
| `Echo/Echo.csproj` | A console program (`net10.0`) that prompts `Name? ` without a newline, reads a line, prints `Hello, <name>!` and exits with code 3, so stdin and the exit code are observable; the publish target publishes it |

Expected outputs are in the tests (`dotnet/tests/Eludite.Host.Tests/RunTargetTests.cs`, `PublishTests.cs`).
