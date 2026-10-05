# Releases and the self-updater's contract

Brief 0055 (ADR-0011). A packaged Eludite updates itself from the repository's GitHub releases (`crates/update`,
`eludite-update`). This file is the contract between the packaging job (`tools/package/`, the `package` job in
`.github/workflows/ci.yml`) and the updater: what a release must carry for an installed Eludite to find, verify
and install it. Nothing here is read at startup; the updater runs on Help > Check for Updates, on `eludite.update.*`
and, once the person has said so (`updates.mode`), on a timer.

## Channels

A channel is a rule for which releases belong to it. The first channel is **unstable**: one release per green build
of `main`.

| Channel | Release tag | GitHub pre-release | Who gets it |
|---|---|---|---|
| `unstable` | `unstable-<build>` | yes | every build of `main` whose `rust` and `dotnet` jobs are green on every platform |

A later `stable` channel will use `v<version>` tags and `prerelease: false`; it is not read by the updater until it
is added to `Channel` in `crates/update/src/build.rs` and to `updates.channel` in `protocol/schemas/settings.json`.

## The build id

`<build>` is the build's id within its channel: dot-separated segments of digits or ASCII letters, compared segment by
segment (numbers by value, else as text; a longer id with the same prefix is newer). CI uses `<YYYYMMDD>.<run
number>` in UTC, so `20261006.3` is newer than `20261005.142`. The id never carries the commit (its hash does not
order); the commit goes in `build.json`.

## What a release carries

- One archive per platform, named as the packaging scripts name them: `eludite-<version>-<os>-<arch>.tar.gz` on
  Linux and macOS, `eludite-<version>-<os>-<arch>.zip` on Windows, with `<version>` the workspace version
  (`Cargo.toml`), `<os>` one of `linux`, `windows`, `macos`, and `<arch>` from `uname -m` (`x86_64`, `aarch64`;
  macOS says `arm64`, which the updater takes as `aarch64`). The archive holds one top-level folder,
  `eludite-<version>-<os>-<arch>/`, with the layout `tools/package/README.md` describes.
- `SHA256SUMS`: `sha256sum`'s format, one line per archive, `<64 hex digits>  <archive name>`. The updater refuses
  an archive whose digest is not listed or does not match, and deletes it.
- Inside each archive's folder, **`build.json`** (`crates/update/src/build.rs`, `Build`):

```json
{
  "version": "0.1.0",
  "channel": "unstable",
  "build": "20261005.142",
  "commit": "0123456789abcdef0123456789abcdef01234567",
  "os": "linux",
  "arch": "x86_64",
  "published": "2026-10-05T12:00:00Z"
}
```

`version`, `channel`, `build`, `os` and `arch` are required; `commit` and `published` are read when present.
`channel` and `build` must match the release's tag, and `os` and `arch` the archive's name: the updater checks
all four after unpacking and refuses the archive otherwise. `linux.sh` and `shell.sh` take the channel and build id
as `--channel` and `--build` and write the file through `tools/package/build-json.sh`; CI passes `unstable` and the
run's build id. A build made without them has no `build.json`, and the updater says "a development build" and stays
off.

## How the updater reads a release

1. `GET https://api.github.com/repos/sand-head/Eludite/releases?per_page=30` with `Accept:
   application/vnd.github+json`, `X-GitHub-Api-Version: 2022-11-28` and the last answer's `ETag` in
   `If-None-Match` (a 304 costs nothing against the unauthenticated rate limit of 60 requests an hour per address).
   `ELUDITE_UPDATE_API` names another base (a mirror, the tests' loopback server). No token is ever sent.
2. Drafts are skipped. The channel's newest release is the one whose tag the channel accepts with the greatest
   build id, whatever the list's order. It is newer than the installed build when the installed `build.json` names
   the same channel and a smaller id, or another channel.
3. The archive for this platform and `SHA256SUMS` are the release's assets by name (`browser_download_url`).
4. The archive streams to `.eludite-update/<tag>/` inside the install folder (so the swap is a rename on one
   filesystem), hashed as it arrives; it is kept only when the digest matches. It is unpacked to
   `.eludite-update/<tag>/layout/` with its top folder stripped, checked (`eludite`, `build.json` as above) and
   recorded in `.eludite-update/staged.json`.
5. On the person's restart, a copy of the new executable runs `eludite --apply-update <plan>`: once the shell has
   exited it moves the install folder's entries to `.eludite-previous/`, the layout's entries in, and starts the new
   `eludite` with the old arguments and `--updated-from <old tag>`. A failure undoes the renames and starts the old
   build. The next successful start deletes `.eludite-previous/`. An install folder that cannot be written (a
   root-owned `/opt`) is reported in the status bar and the Output window; nothing is tried.

## The release job

`.github/workflows/ci.yml` does this on every push to `main`:

- `build-id` computes the run's build id once, `<YYYYMMDD>.<run number>` in UTC, so the three archives and the
  release agree even when the run crosses midnight.
- `package` (after `rust` and `dotnet` are green on every platform) passes `--channel unstable --build <id>` to
  `linux.sh` and `shell.sh`, which write `build.json` into the layout through `build-json.sh`. A pull request's
  archives get no `build.json`: they are development builds to the updater.
- `release` downloads the three archives, writes `SHA256SUMS` over them with `sha256sum`, creates the pre-release
  `unstable-<id>` at the built commit with `gh release create`, and deletes `unstable-*` releases beyond the newest
  30 (the page the updater reads) with their tags.

An asset is never re-uploaded under an existing tag: a tag is one run's bytes, and an installed build may have cached
the list's `ETag`.

## Checking a release by hand

```
tar -tzf eludite-0.1.0-linux-x86_64.tar.gz | head -3            # one top folder
tar -xzOf eludite-0.1.0-linux-x86_64.tar.gz '*/build.json'       # the identity
sha256sum -c SHA256SUMS                                          # the digests
```

`cargo test -p eludite-update` runs the whole path against a loopback server (`crates/update/tests/updater.rs`);
`cargo test -p eludite update_tests` runs it through the shell.
