# corpus/nuget

Brief 0048's NuGet corpus (MIT, see `LICENSE`): small packages and a solution for the Manage NuGet Packages window,
`eludite.nuget.*` and the host's NuGet client. Nothing here touches the network: `NuGet.config` clears every other
source, lists only the local feed `feed/`, and keeps a global packages folder of its own (`.packages/`).

| Path | What it is |
|---|---|
| `packages/Eludite.Corpus.Logging/` | A one-method library, packed as `Eludite.Corpus.Logging` 1.0.0 |
| `packages/Eludite.Corpus.Greeter/` | A one-method library depending on Logging, packed as `Eludite.Corpus.Greeter` 1.0.0, 1.1.0 and 2.0.0-beta.1 |
| `Corpus.slnx` | `App`, `Lib` and `Shared` |
| `App/` | An executable with `Eludite.Corpus.Greeter` 1.0.0 and a project reference to `Shared` |
| `Lib/` | A library with `Eludite.Corpus.Greeter` 1.1.0 (with App, Consolidate's case) |
| `Shared/` | A library with no packages |
| `build.sh`, `build.ps1` | Pack the feed in place (`dotnet pack`, offline), or in a copy given as the argument |

Expected outputs, with the feed built:

- Search `Corpus`: `Eludite.Corpus.Greeter` 1.1.0 (versions 1.1.0, 1.0.0; 2.0.0-beta.1 first with prerelease) and
  `Eludite.Corpus.Logging` 1.0.0, from the source `corpus`.
- Installed after a restore: App has Greeter 1.0.0 (requested `1.0.0`) and, transitively, Logging 1.0.0; Lib has Greeter
  1.1.0; Shared has none. The Dependencies node of App: Packages (Greeter, with Logging under it), Projects (Shared),
  Frameworks (Microsoft.NETCore.App).
- Updates: App's Greeter 1.0.0 to 1.1.0 (both to 2.0.0-beta.1 with prerelease). Consolidate: Greeter, App at 1.0.0 and
  Lib at 1.1.0, 1.1.0 by default.
- Restoring `Corpus.slnx` succeeds offline in about a second.
