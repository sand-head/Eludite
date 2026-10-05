# Proposal 0004: Self-update by release channel

Status: Accepted by the owner's direct request, 2026-10-05 (built in the same change: ADR-0011, brief 0055; the PLAN.md edit under section 10 awaits the owner's review)
Plan reference: PLAN.md sections 2 (principles 1, 3), 10 (Phase 2: "auto-update, signed installers"), 11, 13
Related: ADR-0011, brief 0055, brief 0039 (the Linux package), the `package` job on `ci/package-artifacts`
New paths: `crates/update` (`eludite-update`), `protocol/schemas/update-*.json`, `tools/package/RELEASE.md`, `tools/package/build-json.sh`

## 1. Goal

A packaged Eludite finds, verifies and installs its own newer builds from the repository's GitHub releases,
following a **release channel** the person chooses. The first channel is `unstable`: every green build of `main`.
The person is asked once whether Eludite may check on its own; Help > Check for Updates asks now; the status bar
shows what is available, downloads it in the background when allowed, and offers the restart that installs it.
Agents drive and read the same through `eludite.update.*`. PLAN.md section 10 names auto-update in Phase 2 without
detail; this proposal details it.

## 2. Channels and builds

A channel is a rule for which releases belong to it, written in `eludite-update`'s `Channel`:

| Channel | Tag | Pre-release | When |
|---|---|---|---|
| `unstable` | `unstable-<build>` | yes | each green build of `main`; `<build>` is `<YYYYMMDD>.<run number>` |
| `stable` (later) | `v<version>` | no | when there is a release worth the name |

Build ids order by dot-separated segments (numbers by value, else text), so a tag alone orders releases without
dates or commits. A packaged build knows its own channel and build from `build.json` beside its executable, which
the packaging job writes; a development build has none and the updater is off, saying so.

## 3. Process model

```
eludite (shell, UI thread) ── jobs ──▶ eludite-update worker thread ── HTTPS (ureq, rustls) ──▶ api.github.com, release assets
        ▲ status snapshots, listener          │ streams the archive to <install>/.eludite-update/<tag>/, SHA-256 as it arrives
        │                                     │ unpacks to .../layout/, checks build.json, writes staged.json
   status slot, Output > Updates              ▼
   Help > Check for Updates          on restart: a copy of the new eludite runs `eludite --apply-update PLAN`
   eludite.update.*                  (waits for the shell's exit, renames the entries, starts the new eludite)
```

- Nothing runs at startup: the worker starts with the shell but reads only `build.json` and `staged.json` on its
  thread. The first network call is a command, the answered first-start question, or the timer 15 s after the
  first frame with the person's consent (`updates.mode`), at most every 4 hours.
- The stage lives inside the install folder, so installing is a rename on one filesystem and the folder's
  writability decides at once whether an update is possible (a root-owned `/opt` is reported, not fought).
- The swap runs out of process after the shell has quit, in a copy of the new executable, so the running program
  never overwrites itself (Windows forbids it) and fixes to the applier ship with the update. A failed swap undoes
  its renames and starts the old build; the old files stay in `.eludite-previous/` until the next successful start.

## 4. The command surface

| Command | Class | Does |
|---|---|---|
| `eludite.update.status` | read | the updater's snapshot: this build, channel and mode, the last check, the state (`idle`, `up_to_date`, `no_release`, `no_archive`, `available`, `downloading`, `unpacking`, `ready`, `failed`), what is staged |
| `eludite.update.check` | execute | read the channel's releases (one conditional request); `download: true` stages what it finds; `wait_ms` waits for the answer off the UI thread |
| `eludite.update.download` | execute | download, verify, unpack and stage the available build |
| `eludite.update.apply` | dangerous | restart into the staged build: the person is asked from the UI; an agent's call goes through the policy's prompt |

Schemas in `protocol/schemas/update-*.json`. The settings `updates.channel` and `updates.mode` (`ask`, `notify`,
`download`, `off`) are `eludite.settings.*`'s, under Tools > Options > Environment > Updates. The Output window
gains the Updates source.

## 5. Trust

A release is verified by its `SHA256SUMS`, computed by CI and read over TLS from GitHub, and `build.json` inside the
archive must agree with the tag and the platform. There is no signature yet: the trust is GitHub's TLS and the
repository's write access, which is also what protects the archives a person downloads by hand today. Signing
belongs with the installers (Phase 3); the updater will verify a signature over `SHA256SUMS` then. No token is ever
sent, no account is needed, nothing is reported back: a 304 on an unchanged list is the whole of a quiet check.

## 6. Protocol artifacts

`update-status.{input,output}.json`, `update-check.{input,output}.json`, `update-download.{input,output}.json`,
`update-apply.{input,output}.json`; `settings.json` (`updates.channel`, `updates.mode`); `output-show.input.json`
and `output-clear.input.json` (`updates`). The release contract, which is not a schema but the packaging job's
obligation, is `tools/package/RELEASE.md`.

## 7. Budgets

Cold start unchanged (two file reads on the worker thread, no timer before 15 s). Keystroke to frame unchanged
while a download runs (status snapshots at most ten a second, coalesced per frame). A 200 MB tarball at 1 MB/s
downloads within the 30-minute request budget. No new dependency.

## 8. Briefs

- **0055** (this change): the crate, the commands, the shell, the `unstable` channel, the release contract, and the
  release job: after the `package` job on `main`, `unstable-<build>` as a pre-release with the three archives and
  `SHA256SUMS`; `linux.sh` and `shell.sh` take `--channel` and `--build` and place `build-json.sh`'s file in the
  layout.
- **Later**: the `stable` channel and the rule for switching channels downward; installers taking over the swap on
  their platforms; signing; a rollback command.

## 9. Changes to PLAN.md on acceptance

Section 10, Phase 2: "auto-update" becomes "self-update by release channel (`unstable` first; proposal 0004,
ADR-0011)". Section 12: `crates/update/` under the Rust workspace. Section 14: a decision row, "Self-update: GitHub
releases by channel, verified by SHA256SUMS, staged beside the install, swapped on restart by the new build; off
until the person says (ADR-0011, 2026-10-05)". Not applied in this change: CLAUDE.md forbids editing PLAN.md
without a brief saying so, and the owner has not reviewed this text.

## 10. Risks

- A tarball install in a folder the person cannot write gets no updates; installers are the answer, and the status
  bar says so meanwhile.
- The previous build doubles the install's size until the next start (630 MB with CEF on Linux).
- GitHub's unauthenticated rate limit (60 an hour per address) is shared by every program on the machine; the
  conditional request keeps the updater's share at zero while nothing changes.
- The applier's rename of a running `eludite.exe` on Windows is allowed by the platform but has not run here (no
  Windows machine); the first real update there is to be watched, with `.eludite-update/apply/apply.log` to read.
