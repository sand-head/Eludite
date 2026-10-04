# NuGet in Eludite: a guide for agents

Eludite's NuGet commands (`eludite.nuget.*`) are the Manage NuGet Packages window's actions. The host's NuGet client
reads the solution's `NuGet.config` chain, edits project files (or `Directory.Packages.props` under Central Package
Management) and restores out of process. The person sees every change in the window, the Output window's Package
Manager pane and the Workspace window's Dependencies node.

## 1. Read first

- `eludite.nuget.installed` lists each project's packages: `requested` (what the project file says), `version` (what
  restore resolved), `restored`, `central_package_management` with `props_file`, `lock_file`, and `vulnerabilities`
  and `deprecated` from the sources. `include_transitive: true` adds transitive packages; `vulnerabilities: false`
  reads files only, without contacting a source.
- `eludite.nuget.search` searches the sources (`query`, `source`, `prerelease`, `max_items`). One failing source
  answers an `error` in `sources` while the others' results still come.
- `eludite.nuget.updates` lists packages with a newer version, per project.
- `eludite.nuget.sources` lists the sources and the config files they come from.

Name projects by name (`App`) or by the project file's absolute path. Package ids are case-insensitive.

## 2. Change

- `eludite.nuget.install` adds a package (`project`, or `projects`; `version` defaults to the newest).
- `eludite.nuget.update` moves one package (`package`, `version`) or every listed update (`all: true`).
- `eludite.nuget.uninstall` removes a package from `project`, or from every project that references it.
- `eludite.nuget.consolidate` brings a package to one version across projects (the highest by default).
- `eludite.nuget.restore` restores without editing (`force: true` re-evaluates and rewrites lock files).

Each change answers the `edited` files, the `packages` with their resolved versions, the `restore` result with its
`diagnostics`, and the new solution `generation`. A failed restore leaves the edit in place and its errors in the Error
List, as Visual Studio does: read `restore.diagnostics` (`NU1101` unknown package, `NU1102` no such version, `NU1605`
downgrade), then fix the version or undo with the opposite command. Restore follows `nuget.restoreOnChange` and, with
`nuget.lockFiles: respect`, runs in locked mode when a lock file exists.

The window itself (Project > Manage NuGet Packages...) is the person's; you do not need it to change packages.

## 3. Permission

Reads run at once. Install, update, uninstall and consolidate fall under the `nuget.change` policy (`prompt` by
default: the person is asked before each); restore is an ordinary execute command. Adding, removing, enabling or disabling a source (`eludite.nuget.sources` with an `action`) falls
under `nuget.sources` and writes the person's own `NuGet.config`. Every change is audited with its project, package
and version.

## 4. Credentials

A private feed may need credentials. Eludite tries the installed credential providers first; only the person can
answer the sign-in prompt. When a call fails with `credentials_required`, stop and ask the person to open Manage
NuGet Packages and sign in (credentials are kept for the session), or to install the feed's credential provider. Do
not put credentials in a `NuGet.config` or retry another way.
