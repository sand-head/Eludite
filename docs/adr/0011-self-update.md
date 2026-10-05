# ADR-0011: Self-update from GitHub releases by channel, staged beside the install and swapped on restart

Status: Accepted, 2026-10-05 (the owner's direct request; proposal 0004, brief 0055)
Plan reference: PLAN.md sections 2 (principles 1, 3), 10 (Phase 2: auto-update), 11, 13

## Context

Eludite is built by agents many times a day and run by its owner from the archives CI packages (`tools/package/`,
brief 0039 and the `package` job). Getting a new build meant downloading a tarball and unpacking it over the old
one. PLAN.md names auto-update in Phase 2 without detailing it. The constraints: no network call at startup and
no telemetry (CLAUDE.md), every user-visible action a command (invariant 3), the UI thread never waits (invariant
1), a GPL product with no signing infrastructure yet, three platforms with one tarball-or-zip layout and no installer,
and a browser engine of hundreds of megabytes inside the layout.

## Decision

- Updates come from the repository's GitHub releases, read anonymously with conditional requests. A **channel** is a
  rule for which releases belong to it: `unstable` is the pre-release `unstable-<build>` CI publishes per green
  build of `main`; `stable` (`v<version>`) follows when there is one. Build ids order by dot-separated segments.
- A packaged build knows itself from **`build.json`** beside its executable (version, channel, build, commit,
  platform), written by the packaging job. Without it the updater says "a development build" and does nothing.
- A release is **verified by `SHA256SUMS`**, hashed as the archive streams; nothing unverified is unpacked. There is no
  signature yet: the trust is GitHub's TLS and the repository's write access, as for the archives themselves.
- The download is **staged inside the install folder** (`.eludite-update/`), so installing is a rename on one
  filesystem, and the install folder's writability decides whether an update is possible at all.
- Installing is a **swap on restart by a copy of the new executable** (`eludite --apply-update`): entries move to
  `.eludite-previous/`, the layout's move in, a failure undoes the renames, the new build starts with the old
  arguments. The running shell never overwrites itself, which Windows forbids and which would race the engine.
- The updater is **off until the person says**: the setting `updates.mode` starts at `ask`; a packaged build asks once
  after its first window; the timer starts 15 s after the first frame and checks at most every 4 hours.
- Everything is a command: `eludite.update.status`, `check`, `download`, `apply` (the last dangerous: it quits the
  IDE). Help > Check for Updates, the status bar's update slot and agents call the same commands.

## Alternatives considered

- A manifest file in the release (`eludite-update.json`) instead of the asset list and `SHA256SUMS`: one more
  artifact to generate and keep consistent; the release's own asset list and the standard sums file carry the same
  facts.
- Rolling tags (`unstable` moved to each build): loses history and breaks the cached `ETag`; the build id in the tag
  orders releases without dates.
- Installers (`.deb`, `.rpm`, MSI, a signed `.dmg`) and their package managers: Phase 3 work (`tools/package/README.md`);
  an installer owns its files and would replace this swap for its users, which the design allows (a read-only
  install folder is reported, not fought).
- A separate updater program shipped in the layout: one more binary to build and sign per platform; a copy of the
  new `eludite` runs the swap and ships its fixes with the update.
- Auto-update on by default, as VS Code does: contradicts "no network calls at startup" and the no-telemetry
  stance; one question on the first start costs little.
- Delta updates (bsdiff, zsync): the Linux tarball is 200 MB because of CEF, which changes rarely; worth revisiting
  with per-component archives, not before.
- The `self_update` crate: pulls `reqwest`, `tokio` and `zip`; the pieces needed (`ureq`, `sha2`, `tar`, `flate2`)
  are in the build already and the zip reader is 200 lines.

## Consequences

Positive:
- A tester on the unstable channel is one restart behind `main`; the owner's dogfooding loop closes.
- No new dependency; the whole path runs against a loopback server in `cargo test`.
- The release contract (`tools/package/RELEASE.md`) is small enough for the packaging job to satisfy in one step.

Negative:
- A tarball install needs a writable install folder; system-wide installs wait for installers.
- The previous build is kept until the next start (up to 630 MB with CEF on Linux).
- No signature: a compromised GitHub account could publish a build; signing is a Phase 3 item with the installers.

## Revisit when

- Installers ship (Phase 3): decide whether they take over the swap on their platforms.
- A `stable` channel exists: the rule for switching channels downward (a stable build older than the installed
  unstable one) needs a decision; today another channel's newest build is always offered.
- Releases are signed: the updater verifies the signature over `SHA256SUMS`.
