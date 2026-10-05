# Brief 0055: Self-update from GitHub releases, the unstable channel first

Status: done (this change; the release job that publishes `unstable-<build>` is the packaging work on `ci/package-artifacts`)
Phase: 2 (PLAN.md section 10: "auto-update")
Plan reference: PLAN.md sections 2 (principles 1, 3), 10 (Phase 2), 11 (bus factor: release scripts in the repo), 13
Related ADRs: ADR-0011 (new), ADR-0009 (the dangerous class of `apply`)
Related: proposal 0004; `tools/package/RELEASE.md` (the contract with the packaging job)
Depends on: the `package` job (`ci/package-artifacts`) producing the archives; a release job publishing them under `unstable-<build>` with `SHA256SUMS` and `build.json` in each archive (owed by that work, see Contract)

## Goal

A packaged Eludite keeps itself current from the repository's GitHub releases without the person downloading
anything by hand. Release **channels** name which releases a build follows; the first is **unstable**, one release
per green build of `main`. The person picks the channel and how automatic updates are (`updates.channel`,
`updates.mode`); Help > Check for Updates asks now; the status bar says when a build is available, downloads it in
the background when allowed, and offers the restart that installs it. Agents see and drive the same through
`eludite.update.*`. Nothing reaches the network at startup, and a development build (cargo's target folder) says it
cannot update and does nothing.

## Files in scope

- `crates/update` (new, `eludite-update`): `build.rs` (`build.json`, `Channel`, `BuildId`, `Platform`), `release.rs`
  (the release list, the channel's newest, the archive and `SHA256SUMS` assets), `http.rs` (`Transport`, `ureq`
  streaming), `download.rs` (conditional requests, the verified download), `extract.rs` and `zip.rs` (`.tar.gz`,
  `.zip`), `stage.rs` (`.eludite-update/`, `staged.json`), `apply.rs` (the plan, the applier, the swap), `updater.rs`
  (the worker, `Status`, `Mode`), `time.rs`, `test_support.rs` (the loopback release server, feature
  `test-support`), `tests/updater.rs`.
- `crates/commands/src/update.rs` (new) and `lib.rs`; `crates/commands/src/build.rs` (`OutputSource::Updates`).
- `protocol/schemas/update-{status,check,download,apply}.{input,output}.json` (new), `settings.json`
  (`updates.channel`, `updates.mode`, the section Environment > Updates), `output-show.input.json` and
  `output-clear.input.json` (`updates`).
- `crates/eludite/src/shell/update.rs` (new), `update_tests.rs` (new), `shell.rs` (the service in `Services`, the
  install, the `run` dispatch), `shell/settings.rs` (the settings hook), `shell/output.rs` (the Updates source),
  `args.rs` and `main.rs` (`--apply-update`, `--updated-from`), `app.rs` (no question on measurement runs),
  `Cargo.toml`; `crates/ui/src/menu.rs` (Help > Check for Updates...).
- `tools/package/RELEASE.md` (new, the contract), `tools/package/build-json.sh` (new).
- `Cargo.toml` (the workspace member), `CLAUDE.md` (the crate map), `docs/adr/0011-self-update.md` and the ADR
  index, `docs/proposals/0004-self-update.md` and its index, this brief and the briefs index.

## Contract

- **The release** (`tools/package/RELEASE.md`): tag `unstable-<build>` with `<build>` = `<YYYYMMDD>.<run number>`,
  a pre-release; assets `eludite-<version>-<os>-<arch>.tar.gz` (Linux, macOS) or `.zip` (Windows) with one top
  folder, and `SHA256SUMS`; `build.json` inside each archive's folder naming the version, channel, build, commit,
  os and arch (`tools/package/build-json.sh --channel unstable --build <build>` writes it; `linux.sh` and `shell.sh`
  on `ci/package-artifacts` are to take `--channel` and `--build` and place it in the layout). The updater checks
  all of it after unpacking and refuses what disagrees.
- **Reading GitHub**: `GET /repos/sand-head/Eludite/releases?per_page=30`, anonymous, `If-None-Match` from the last
  answer's `ETag` (a 304 is free against the rate limit), no token ever; `ELUDITE_UPDATE_API` replaces the base.
  The channel's newest is the greatest build id among the tags the channel accepts, drafts skipped. Newer than the
  installed build: a greater id in the same channel, or another channel.
- **The download**: streamed to `<install>/.eludite-update/<tag>/<archive>.part`, SHA-256 as it arrives, kept only
  when it matches `SHA256SUMS`, else deleted with `verification` reported; unpacked to `.eludite-update/<tag>/layout/`
  with the top folder stripped, entries that would escape refused (`..`, absolute paths, links out of the folder);
  checked (`eludite[.exe]`, `build.json`); recorded in `.eludite-update/staged.json`. An install folder that cannot
  be written fails the download with the folder named (`apply` kind); nothing else is tried.
- **The swap** (`eludite --apply-update PLAN`): a copy of the staged executable in `.eludite-update/apply/`, started
  with its stdin piped from the shell; it waits for that pipe's end (the shell's exit, at most 120 s), moves the
  install folder's entries the layout carries to `.eludite-previous/` and the layout's in, undoes every rename on an
  error and starts the old build instead, writes `.eludite-previous/applied.json`, removes `staged.json` and the
  release folder, and starts `<install>/eludite` with the relaunch arguments (`--solution`, `--folder`, `--theme`,
  `--open-file`, `--agent`, `--no-persist` carried; measurement and one-shot flags not) plus `--updated-from <old
  tag>`. The next successful start removes `.eludite-previous/` and `apply/` about 15 s after the first frame.
- **Commands** (`eludite.update.*`, all agent-visible): `status` (read), `check` (execute: `wait_ms` 0 to 600000,
  `download`), `download` (execute: `wait_ms`), `apply` (dangerous: quits the IDE). The UI thread never waits
  (`wait_ms` is 0 there); `apply` from another thread is a job the UI applies; from the UI it is staged for the bus
  after the confirmation, so the audit log carries it either way. The output is the updater's `Status`
  (`update-status.output.json`): `enabled`, `reason`, `build`, `install_dir`, `channel`, `mode`, `state` (`idle`,
  `up_to_date`, `no_release`, `no_archive`, `available`, `downloading`, `unpacking`, `ready`, `failed`),
  `last_check`, `busy`, `writable`.
- **Settings**: `updates.channel` (`unstable`), `updates.mode` (`ask` default, `notify`, `download`, `off`), section
  Environment > Updates. A packaged build in `ask` asks once, 2 s after its first window (never on a measurement
  run or with `--exit-after-ms`): "Yes, download them" / "Only tell me" / "No" write `download`, `notify` or `off`
  through `eludite.settings.set` and, for the first two, check at once. The timer starts 15 s after the first frame,
  asks every minute whether a check is due, and checks at most every 4 hours while the mode is `notify` or
  `download` (`download` stages what it finds). A channel change forgets the last check.
- **The UI**: Help > Check for Updates... runs `check`; the status bar's `update` slot (right) reads "Update
  available: <tag>" (click: `download`), "Downloading update… N%", "Unpacking update…", "Restart to update (<tag>)"
  (click: `apply`, after "Restart Eludite now to install <tag>?" with Restart Now / Later), "Update failed" (click:
  `check`); the status text answers a person's check ("Eludite is up to date (<tag>)", "Update failed (network); see
  Output > Updates"); the Output window's Updates source has one line per state change and the applied update at a
  start with `--updated-from`.
- **Invariants**: no network call at startup (the first call is the person's command, the answered question, or the
  timer 15 s after the first frame with consent); the UI thread never waits (the worker thread does the network and
  the disk; the applier runs out of process); every action is a command; no new dependency (`ureq`, `sha2`, `tar`,
  `flate2` are in the build; the zip reader is this crate's).
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite-update`: build ids, channel tags, archive names, `SHA256SUMS`, the zip reader (CRC,
  modes), `.tar.gz` and `.zip` unpacking with the escape refusals, staging and its checks, the swap and its undo, the
  applier in process; `tests/updater.rs` against the loopback server: check (`ETag`, 304), download, verify, stage,
  the staged build found on the next start with no network call, `download` mode checking then staging, a
  development build asking nothing, a corrupt archive refused and deleted, a cancel mid-download cleaning up, a 404
  reported, a channel change.
- `cargo test -p eludite-commands update`: schemas, classes, input checks, the target.
- `cargo test -p eludite update_tests` (headless GPUI, the loopback server, a fake install folder): a development
  build reports itself and never asks the network; `download` mode stages on the timer, shows "Restart to update",
  and `apply` from an agent's thread and from the status bar's click with the confirmation hands the applier its plan
  (two audit entries, no further network call); the first-start question writes `updates.mode: notify` and the check
  runs without a download; Help > Check for Updates on the newest build says "Eludite is up to date".
- By hand, on a packaged build with a published release: Help > Check for Updates, the download, Restart Now, the
  new build's "Eludite updated from ... to ..." line.

## Budget

- Cold start to interactive window: unchanged (the updater reads `build.json` and `staged.json` on its own thread;
  no network, no timer before 15 s after the first frame).
- Keystroke to frame while a download runs: unchanged (status snapshots at most ten a second, coalesced per
  frame).
- No new dependency; `eludite-update` adds under 1 s to a clean debug build.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green on
   Linux (CI on Windows and macOS: the `.zip` path is tested on every platform by the zip reader's tests; the
   applier's rename semantics on Windows are to be watched on the first real update there).
2. The release contract is written (`tools/package/RELEASE.md`) and `build-json.sh` produces a file the updater
   accepts.
3. The briefs index, the ADR index, the proposals index and CLAUDE.md match the repo.

## Out of scope

- The release job itself (the packaging work on `ci/package-artifacts` creates the release and uploads the assets).
- A `stable` channel, installers, signing, delta updates, a rollback command (`.eludite-previous/` is the manual
  rollback until the next start), updating the pinned external tools (`tools/`), updating the host separately from
  the shell (one archive carries both).
