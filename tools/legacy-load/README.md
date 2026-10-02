# tools/legacy-load (brief 0003)

Loads every project in `corpus/legacy/manifest.json` through eludite-host's legacy design-time evaluators
(`dotnet/src/Eludite.Host/Legacy/`) and the Roslyn language server, and writes per-project JSON plus `matrix.md`
under `results/<stamp>/` (gitignored). The findings are in `docs/briefs/0003-report.md`.

```
tools/legacy-load/run.sh                   # Linux: fetch corpus, restore, all phases
tools/legacy-load/run.sh --prepare-only    # fetch, reference-assembly packages, build; no measurement
PHASES=eval,compile ENTRIES=webforms-changepk tools/legacy-load/run.sh
RESULTS=tools/legacy-load/results/<stamp> PHASES=roslyn ROSLYN_MODES=mono-fixed tools/legacy-load/run.sh  # add a mode
tools\legacy-load\run.ps1                  # Windows; UNTESTED (no Windows machine was available)
```

Phases (`--phases`, default all):

| Phase | What |
|---|---|
| `restore` | `msbuild -t:restore -p:RestorePackagesConfig=true` per entry with the located MSBuild (Mono or Build Tools). Not part of evaluation timing. |
| `eval` | Each evaluator, **one cold process per project** (the runner re-executes itself), then one cold process for the whole entry. |
| `compile` | Builds a `CSharpCompilation` from the evaluator's output alone (Compile items, resolved references, defines, project references) and counts errors. |
| `roslyn` | Starts eludite-host with the Roslyn LS, opens the entry (`solution/open`, or `project/open` for a bare `.csproj`), waits for `workspace/projectInitializationComplete`, parses the server's load log and pulls `textDocument/diagnostic` for every Compile item. Mode `mono` puts Mono on the server's `PATH` (Roslyn then uses its Mono build host); mode `sdk` hides Mono (`ELUDITE_LEGACY_MONO=0`, .NET SDK build host). These two load projects exactly as written (`ELUDITE_LEGACY_DESIGNERS=0`). Modes `mono-fixed` and `sdk-fixed` add eludite-host's design-time corrections (designer partials, Compile-item case fixups). Default `mono,sdk,mono-fixed` on Linux, `sdk,sdk-fixed` on Windows (where Roslyn's .NET Framework build host uses the located Visual Studio MSBuild); `ROSLYN_MODES` overrides. |
| `designer` | Generates designer partials from markup and compares their fields with the checked-in `.designer.cs` files. |

Evaluators (`--evaluators`, Linux default `mono-msbuild,sdk-msbuild,sdk-msbuild-strict,sdk-msbuild-cli`):

| Name | What |
|---|---|
| `mono-msbuild` | Mono's MSBuild command line, injected `EluditeDesignTimeDump` target (`ResolveAssemblyReferences` only, `SkipCompilerExecution=true`, `BuildingProject=false`) |
| `buildtools-msbuild` | Same with Visual Studio Build Tools' `MSBuild.exe`, located with `vswhere` (Windows) |
| `sdk-msbuild` | The .NET SDK's MSBuild in-process via Microsoft.Build.Locator, `IgnoreMissingImports` (what Roslyn's build host does too) |
| `sdk-msbuild-strict` | Same without ignoring missing imports |
| `sdk-msbuild-cli` | `dotnet msbuild` command line (missing imports are fatal) |

Reference assemblies come from the `Microsoft.NETFramework.ReferenceAssemblies.net4*` 1.0.3 packages (MIT) in the
NuGet cache, passed as `TargetFrameworkRootPath`. `run.sh` downloads them with a throwaway `PackageDownload` project.

## Mono without root (what brief 0003 did on CachyOS)

Mono is not installed system-wide on the reference machine and `sudo` was not available, so Arch's packages were
extracted into the user's home. Nothing outside `~/.cache/eludite` and `~/.local/opt/mono-root` is touched.

```
mkdir -p ~/.cache/eludite/mono-pkgs ~/.local/opt/mono-root
cd ~/.cache/eludite/mono-pkgs
for u in $(pacman -Sp mono mono-msbuild); do curl -fsSLO "$u"; done
#   libgdiplus-6.2-1.2, mono-6.12.0.206-1.1, mono-msbuild-16.10.1.xamarinxplat.2021.05.26.14.00-5.1 (x86_64_v4)
for p in *.pkg.tar.zst; do tar --zstd -xf "$p" -C ~/.local/opt/mono-root/; done
```

Mono finds its managed libraries relative to its own binary, so it relocates. It needs these variables to find its
native libraries and config (eludite-host sets them itself, see `MonoInstallation.EnvironmentFor`):

```
R=~/.local/opt/mono-root/usr
export PATH=$R/bin:$PATH LD_LIBRARY_PATH=$R/lib MONO_CFG_DIR=$R/../etc MONO_GAC_PREFIX=$R
mono --version                                  # Mono JIT compiler version 6.12.0
mono $R/lib/mono/msbuild/Current/bin/MSBuild.dll -version   # 16.10.1.36301
```

`$R/bin/msbuild` is a shell script that hard-codes `/usr/bin/mono` and `/usr/lib/mono/...`; call `mono MSBuild.dll`
directly instead. Without `LD_LIBRARY_PATH`/`MONO_CFG_DIR`, MSBuild dies at start with
`DllNotFoundException: System.Native`.

eludite-host finds this layout without configuration: `MonoInstallation.Locate()` checks `ELUDITE_MONO_PREFIX`, then
`mono` on `PATH`, then `~/.local/opt/mono-root/usr`, then `/usr`, `/usr/local` and the macOS framework.

**Package restore over HTTPS** (only the `restore` phase needs it): a user-space Mono has an empty certificate store,
so NuGet fails with `CERTIFICATE_VERIFY_FAILED`. Import the system bundle into a store under `~/.cache/eludite`
and point Mono at it with `XDG_CONFIG_HOME` (the runner sets that variable for the restore step only):

```
XDG_CONFIG_HOME=~/.cache/eludite/mono-config mono $R/lib/mono/4.5/cert-sync.exe --user /etc/ssl/certs/ca-certificates.crt
#   121 new root certificates were added to your trust store.
```

With a system install (`sudo pacman -S mono mono-msbuild`, or the mono-project.com packages elsewhere) none of the
above is needed.
