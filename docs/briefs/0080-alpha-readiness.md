# Brief 0080: Alpha readiness

Status: in progress
Phase: 2 (PLAN.md section 10; the owner's request of 2026-10-11: "fully fleshed out and out of a prototype stage ... even if it 'just' ends up in alpha")
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 10), 3 (D2: the host; D3: protocols), 4.3 (Roslyn through the host), 9 (no telemetry, offline), 10, 13 (packaging)
Related ADRs: ADR-0002 (process topology: Roslyn is a child of the host), ADR-0005 (licensing: Roslyn is MIT), ADR-0011 (self-update: the archives and `build.json`), ADR-0014 (deterministic tests), ADR-0015 (new: Roslyn is built by CI and shipped beside the host)
Depends on: brief 0002 (the Roslyn pin), brief 0039 (the Linux package and its smoke test), brief 0055 (versions, `build.json`, the release job), brief 0063 (`vb.patch`).

## Goal

A person who downloads an Eludite archive (or the MSI) and opens a C# or Visual Basic solution gets IntelliSense
without building anything: the pinned Roslyn language server is built from source by CI and shipped inside
`eludite-host/roslyn/`. When something the IDE needs is missing on the machine (the .NET SDK a `global.json` asks
for, the .NET 10 runtime, Roslyn, netcoredbg, Node.js, Mono), the Welcome page and the opening of a workspace say
what is missing in one line each with where to get it, instead of a developer message. Help > View Help opens user
documentation that exists, the build says its version everywhere (`eludite --version`, the host, the status bar), and
CI proves a packaged archive works end to end.

## Files in scope

- `docs/adr/0015-roslyn-shipped.md` (new).
- `.github/workflows/ci.yml`: a `roslyn` job (Linux, cached on `tools/roslyn-pin/COMMIT`, `tools/roslyn-pin/*.patch`
  and the build scripts; uploads the server folder as the artifact `roslyn-server`), the rust, dotnet and package jobs
  download it; the rust job builds `eludite-host` before its tests; `ELUDITE_REQUIRE` gains `roslyn` and `host`;
  macOS joins the dotnet matrix; the package job runs the packaged smoke test on Linux; the release job's notes list
  the commit subjects since the previous `unstable-*` tag. netcoredbg's steps stay brief 0065's.
- `tools/package/companions.sh` (Roslyn's lines only: `eludite-host/roslyn/` with `licenses/roslyn/`; the host's
  `-p:Version=`), `shell.sh` and `linux.sh` (`docs/` in the layout), `README-companions.in`, `README.md`,
  `smoke.sh` (new, the packaged smoke test's driver).
- `tools/roslyn-pin/README.md` (the shipped layout).
- `dotnet/src/Eludite.Host/Lsp/RoslynProcessLauncher.cs` (`Locate`), `dotnet/src/Eludite.Host/Program.cs` (the
  setting's argument, `--version`), the ELUDITE0001 text in `dotnet/src/Eludite.Host/Lsp/LspProxy.cs` (one marked
  line), `dotnet/Directory.Build.props` (the version is overridable), `dotnet/tests/Eludite.Host.Tests/` (new test
  file for `Locate`).
- `crates/eludite/src/shell/environment.rs` (new) and `environment_tests.rs` (new): the check, the info bar, the help
  commands, the build identity.
- One marked hook each in `crates/eludite/src/shell/welcome.rs`, `crates/eludite/src/args.rs`,
  `crates/eludite/src/main.rs`, `crates/eludite/src/shell.rs` (mod lines, registration, the `Services` field, the
  workspace-open check), `crates/eludite/src/shell/session.rs` (the host's extra arguments),
  `crates/eludite/src/shell/settings.rs` (the setting reaches the session).
- `crates/commands/src/environment.rs` (new: `eludite.environment.check`, `eludite.help.view`,
  `eludite.help.release_notes` specs and parsing), its `mod` line in `crates/commands/src/lib.rs`, the
  `languageServers.roslynPath` entry in `protocol/schemas/settings.json` and the key list test in
  `crates/commands/src/settings.rs`.
- `crates/ui/src/menu.rs` (Help > Release Notes), `crates/ui/src/keymap.rs` (F1).
- `protocol/schemas/environment-check.input.json`, `environment-check.output.json`, `help-view.input.json`,
  `help-view.output.json`, `help-release-notes.input.json`, `help-release-notes.output.json` (new).
- `docs/user/` (new): `README.md`, `getting-started.md`, `features.md`, `keyboard.md`, `troubleshooting.md`,
  `known-limitations.md`.
- `.github/ISSUE_TEMPLATE/bug.md`, `.github/ISSUE_TEMPLATE/config.yml` (new).
- `crates/lsp/tests/package_smoke.rs` (new, gated on `ELUDITE_PACKAGE_ARCHIVE`).
- This brief and `docs/briefs/0080-report.md`.

Not `README.md`, `CLAUDE.md`, `docs/briefs/README.md` or `docs/PLAN.md` (the report's Integration notes carry their
lines), nothing of brief 0065 (netcoredbg), 0072 (`welcome.rs` beyond the hook, `args.rs` beyond the block,
`folder.rs`), 0078 (the About dialog, logs) or 0079 (Razor in `tools/roslyn-pin`, the launcher's arguments).

## Contract

**Roslyn shipped (ADR-0015).**
- CI's `roslyn` job runs `tools/roslyn-pin/build.sh` on Ubuntu when its cache misses; the key hashes
  `tools/roslyn-pin/COMMIT`, `tools/roslyn-pin/*.patch`, `build.sh` and `build.ps1` (brief 0079's patches join the key
  by the glob). The cached and uploaded folder is the server's output folder
  (`artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/Release/net10.0/`) plus Roslyn's `License.txt` and
  `THIRD-PARTY-NOTICES.txt`. The server is framework-dependent and run as `dotnet <dll>`, so the Linux build serves
  Windows and macOS.
- `companions.sh` copies that folder into `eludite-host/roslyn/` and the license texts into `licenses/roslyn/`. The
  folder comes from `ELUDITE_ROSLYN_DIR`, else the default `tools/roslyn-pin` output; when neither holds the server,
  it fails naming both, unless `ELUDITE_PACKAGE_NO_ROSLYN=1` (a developer's package without it, said loudly).
- `RoslynProcessLauncher.Locate(explicitPath, hostDirectory)`: `--roslyn-ls` (the shell passes the setting
  `languageServers.roslynPath` this way), then `ELUDITE_ROSLYN_LS`, then `roslyn/` beside the host's own assembly,
  then the `tools/roslyn-pin` output under `ROSLYN_SRC_DIR` (default `~/.cache/eludite/roslyn`). Explicit choices
  (the setting, the variable) win over the shipped copy so a person can try another server; a named path that does
  not exist is null (not a silent fallback), and the host's log says which source it used or why none.
- The setting `languageServers.roslynPath` (Tools > Options, "Language servers"; a file path, empty by default) is
  read by the shell when it starts the host: `--roslyn-ls <path>` is appended to the host's command (one marked hook
  in `session.rs`, fed by `ServerSession::set_host_args` from the settings apply). A change applies to the next host
  started.
- ELUDITE0001 when no server was found: `C# and Visual Basic IntelliSense is unavailable: the Roslyn language server
  was not found beside eludite-host, at the setting languageServers.roslynPath (Tools > Options) or ELUDITE_ROSLYN_LS.
  See docs/user/troubleshooting.md#roslyn.` (marked line).
- The `README-companions.in` text says Roslyn ships in `eludite-host/roslyn/`.

**First-run environment check.**
- `eludite.environment.check` (schema first: `environment-check.input.json` `{ "workspace"?: string }`,
  `environment-check.output.json` `{ "items": [{ "id", "label", "status": "ok" | "missing" | "warning", "detail",
  "fix"?, "link"? }], "problems": integer }`), permission read, agent visible. Items, in order: `dotnet` (on
  `PATH` or `DOTNET_ROOT`, its SDKs from `dotnet --list-sdks` and runtimes from `dotnet --list-runtimes`), `runtime`
  (a `Microsoft.NETCore.App 10.*` runtime, which the host needs), `sdk` (only with a workspace: the SDK its nearest
  `global.json` asks for, honoring `rollForward` the way the muxer does for `latestPatch` (default), `latestFeature`,
  `latestMinor`, `latestMajor`, `disable` and the `major`/`minor`/`feature`/`patch` forms), `roslyn` (the same order
  as the host's `Locate`, mirrored), `netcoredbg` (`eludite_dap::discovery::AdapterSearch::from_env().find_netcoredbg()`),
  `node` (`eludite_lsp::node::NodeSearch::from_env()`, a warning, needed only for web projects), `mono` (Linux and
  macOS only, a warning, needed only for .NET Framework projects). Missing `dotnet`, runtime, SDK or Roslyn are
  `missing`; the others `warning`. Each non-ok item has one actionable `fix` line and a `link`
  (`https://dot.net/download`, the `docs/user/troubleshooting.md` anchor on GitHub, nodejs.org, mono-project.com).
- The check runs on a background thread (never the UI thread), runs only local programs with a 10 s bound each, and
  makes no network call. The shell runs it once after the first frame and again on each workspace or solution open
  (with that path). The command runs it on the calling thread.
- The latest report is a GPUI global; the Welcome page's one marked hook draws `environment::info_bar` under the
  hint when the report has problems or warnings (one line per item: label, detail, fix, a link button that opens
  the link through the opener). On a workspace open with `missing` items the Output window's "Environment" lines
  list them and the status bar's `environment` slot says `Environment: N problems`; clicking it runs
  `eludite.environment.check` and shows the Welcome page's bar again (it opens the Welcome document).

**Help.**
- `eludite.help.view` (`help-view.input.json` `{ "topic"?: "getting-started" | "features" | "keyboard" |
  "troubleshooting" | "known-limitations" }`, output `{ "url": string }`): opens
  `https://github.com/sand-head/Eludite/blob/<ref>/docs/user/<topic or README>.md` in the system browser, where
  `<ref>` is the build's commit (`build.json`'s, else the compile-time `ELUDITE_COMMIT`), else `main`. F1 runs it
  when no other context binds F1 (none does today).
- `eludite.help.release_notes` (output `{ "url" }`): the release page of the running build
  (`releases/tag/unstable-<build>`) when `build.json` names a build, else the releases list. Help > Release Notes.
- The opener is replaceable (`environment::Opener`, an `Arc<dyn Fn(&str) -> io::Result<()>>` in `Services`); tests
  record instead of starting a browser.
- The archives carry `docs/user/` as `docs/` beside the executable for offline reading; the README names it.

**Versioning.**
- The host's assembly version is the release version: `companions.sh` publishes with `-p:Version=<workspace
  version>`; `Directory.Build.props` keeps `0.1.0` only as the default. The host's `initialize` reports it (already
  read from the assembly), so the status bar's `eludite-host <version>` matches the shell.
- `eludite --version` prints `eludite <version>`, then `channel <c>, build <b>` when `build.json` exists, then
  `commit <sha>` when known, and exits without a window. `environment::build_info()` returns the same fields for the
  About dialog (brief 0078). `eludite-host --version` prints its version and exits.

**Packaged smoke test.**
- `crates/lsp/tests/package_smoke.rs`, skipped unless `ELUDITE_PACKAGE_ARCHIVE` names an archive (fails instead
  when `ELUDITE_REQUIRE` lists `package`): unpacks it into a temp folder, runs `eludite --version` from it with an
  isolated `HOME`, then starts the packaged `eludite-host --stdio` with no `ELUDITE_ROSLYN_LS` and no Roslyn cache,
  speaks the host protocol to it (`eludite/initialize` reports the version and a configured language server,
  `eludite/solution/open` on a copy of `corpus/projects/Console`, `textDocument/completion` after `Console.`
  returns `WriteLine`, `eludite/tests/discover` finds the corpus tests when `corpus/tests` is built). CI's package job
  runs it on Linux with `ELUDITE_REQUIRE=package`.

**Issue template and release notes.**
- `.github/ISSUE_TEMPLATE/bug.md` asks for `eludite --version`'s output, the OS, the steps, and the logs (Output >
  Host, the paths in `docs/user/troubleshooting.md`).
- The release job lists the commit subjects of the first-parent history since the previous `unstable-*` tag (at most
  50) under the existing sentence.

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite environment`: the check against a fake `PATH` with fake `dotnet` scripts printing SDK and
  runtime lists (no dotnet, no runtime 10, a `global.json` satisfied and not satisfied under each roll-forward
  policy, Roslyn beside the host and missing), the info bar lines, Help > View Help and Release Notes opening the
  expected URLs through the recording opener (by the menu's command), the host's extra arguments from the setting,
  `--version`'s text.
- `cargo test -p eludite-commands`: the specs validate against the schemas; the settings key list.
- `dotnet test --filter RoslynLocateTests`: the order (explicit, environment, beside the host, cache) and the missing
  cases.
- The packaged smoke test end to end where Roslyn and the package can be built (the report says whether it ran).
- `cargo fmt --check`, `cargo clippy` on the touched crates, `dotnet build` and `dotnet test`.

## Budget

The check adds nothing to the UI thread: under 1 ms to post the job and apply the report (asserted with
`assert_budget`). The archive grows by the Roslyn folder (about 140 MB unpacked, about 50 MB compressed).

## Exit criterion

An archive from CI's package job, unpacked on a machine with the .NET 10 SDK and nothing else, serves C# completion
through the packaged host with no environment variables; the smoke test proves it on CI's Linux runner.

## Out of scope

The About dialog and logs (brief 0078), Razor (brief 0079), netcoredbg's packaging (brief 0065), signing,
the macOS bundle, a docs website.
