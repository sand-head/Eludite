# Brief 0040 report: Git basics: the Git Changes window, status decorations, diffs, branches and sync

Status: done on Linux. Windows and macOS: not run (no machines; nothing in the git code is platform-specific beyond
libgit2 itself). CI: not run (nothing pushed).
Branch: `brief/0040-git-changes`, based on `main` at `72916ba`; not rebased (the coordinator merges). Date: 2026-10-03.
Brief: [0040-git-changes.md](0040-git-changes.md).

## 1. Summary

- **Git Changes** (View > Git Changes, Ctrl+0, Ctrl+G; Git > Commit or Stash...) is Visual Studio's: the branch with
  "N outgoing / N incoming" and Fetch, Pull, Push and Sync; the message box (Ctrl+Enter commits) with Commit All
  (Commit Staged once something is staged), Commit All and Push, Stash All and Amend; the Merge Changes, Staged Changes
  and Changes groups, each file with its glyph, Stage (+), Unstage (−) and Undo Changes (↶), a double-click for
  Compare with Unmodified; the stashes with Apply, Pop and Drop; "View all commits"; and in a folder with no
  repository, Create Git Repository. The list is virtualized.
- **Git Repository** (Git > Manage Branches, Ctrl+0, Ctrl+R; the status bar's branch) lists branches, remote
  branches, tags, stashes and worktrees beside the history with its graph (lanes computed by `eludite-git`), with
  Checkout, Merge into Current, Rebase Current onto, Delete for a branch; Checkout, Cherry-Pick, Reset (Keep Changes),
  Reset (Delete Changes) for a commit; a New Branch box (Git > New Branch... focuses it); Continue and Abort while a
  rebase or merge is stopped.
- **One status everywhere.** `eludite-git`'s `StatusCache` is recomputed off the UI thread by its watcher; the Git
  Changes window, the Workspace window's glyphs and colors (modified ✓, added +, untracked ?, deleted −, renamed R,
  ignored ⊘, conflicted !), the document tabs' glyphs and the status bar (`⎇ main`, `✎ N` pending, `↓N` incoming
  (Pull), `↑N` outgoing (Push) or `↑ Publish`, Fetch, and while a transfer runs its progress and Cancel) draw from the
  same snapshot. An answer older than the generation on screen is dropped.
- **Compare with Unmodified** (the Workspace file's context menu, Ctrl+D on the selected file, a double-click in Git
  Changes, `eludite.git.diff` from the UI) opens a read-only two-pane document from brief 0016's line diff, both panes
  in one virtualized list so they scroll together, the diff colors and signs, F8 and Shift+F8 between differences.
  **Blame** (the context menu) opens each line with its commit, author and date.
- **The change margin**: the editor's lines that differ from the index (added green, modified blue, a red wedge for
  deleted lines), recomputed off the UI thread 120 ms after the editor goes idle and whenever the index changes (a stage
  clears them); a file with conflict markers shows its sides ("ours" blue, "theirs" orange) instead.
- **Confirmations** (Visual Studio's Yes/No): Undo Changes, Reset (Delete Changes), a stash drop, a branch delete and
  any `force`.
- **Agents call the same 22 commands** (`eludite.git.*`, schemas in `protocol/schemas/git-*.json`), the policy's `git`
  object applies (section 4), `eludite://git/status` is a live MCP resource and `eludite://guides/git`
  ([docs/agents/git.md](../agents/git.md), 703 words) the guide.
- **No `git` process** is started by any of it: libgit2 (git2 0.20, bundled libgit2 1.9.7) does everything.
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug build with optimized dependencies, the other agent's builds
  running beside: load average 4 to 8):

  | Budget | Result |
  |---|---|
  | `status` 50 ms p95 from the cache, 10,000 files, 100 changes | **2.25 ms p95** through the bus from an agent thread, 200 calls (`git_tests::status_answers_from_the_cache_on_ten_thousand_files`); the cache read itself **0.06 ms p95** (`crates/git/tests/budget.rs`). Pass |
  | First status of such a repository under 500 ms off-thread | **30 ms** (a recompute 43 ms p95; the test asserts 500 ms). Pass |
  | 1,000 changed files, no frame over 8 ms | the Git Changes window drawn with 1,000 untracked files: **median 4.6 ms, slowest of 20 frames 5.4 ms** (whole window: layout, prepaint, paint). Pass |
  | Compare with Unmodified of a 5,000-line file under 100 ms | **16.4 ms** from the command to the document's view (libgit2 reads, the line diff, the side-by-side rows). Pass |
  | The margin follows the index within 200 ms of an edit on idle | **120 ms idle + 2.2 ms** to read the index and diff (a Program.cs). Pass |
  | No new dependency | none: `eludite-git` was already in the workspace; `Cargo.lock` gains only `eludite` → `eludite-git`. `notify` is not in the build (section 7.1), so the watcher polls. Pass |

- **Tests**: `cargo test --workspace` 862 passed, 2 failed (two debugger replays disturbed by the other worktree's
  run, passing alone), 1 ignored (section 10); `cargo fmt --check`, `cargo clippy --workspace --all-targets --features
  eludite-chromium/cef -- -D warnings` and `dotnet build dotnet/Eludite.slnx` (0 warnings) clean.

## 2. What was built

Commits on top of `72916ba`, in order:

1. `ad88315` Mark brief 0040 in progress.
2. `548957d` The schemas, first and alone: `git-*.input/output.json` for status, init, stage, unstage, discard,
   commit, diff, log, branches, checkout, merge, rebase, cherry-pick, reset, stash, fetch, pull, push, cancel,
   worktrees and blame; `agents-policy.json`'s `git` object; the `git.*` settings; `view-show`'s `git_repository`;
   `mcp-resource.json` for `eludite://git/status`.
3. `6272746` `crates/git`: the library and its tests (section 3).
4. `1558013` `crates/commands/src/git.rs` (parsing, typed outputs, classes, the escalation hooks), the policy's
   `GitPolicy`, the Integer setting kind; `crates/mcp`: the git guide and the live status resource.
5. `7743d66` The Options dialog edits whole-number settings.
6. `c1a3569` `git-sync` schemas (Git > Sync is Visual Studio's pull then push; it needed a command of its own).
7. `e39f5d0` The git settings' Options page listed last.
8. `ffcb220` `crates/ui` and `crates/docking`: clickable status bar slots, the tree row badge, the tab badge, the Git
   menu, Ctrl+0, Ctrl+R, the Workspace git context items, the `git_repository` tool window.
9. `b32d676` The shell: `shell/git.rs` and `shell/git/` (service, changes, repository, compare, gutter), the hooks,
   and `shell/git_tests.rs`.
10. `e7498c3` `eludite.git.sync` in the commands (it belongs before 9: commit 9 does not build alone; see section 9).
11. `6c443ea` `crates/eludite/tools/git-linux.sh` and its screenshots.
12. The "no workspace" text of Git Changes, the transports test, this report, the brief's status, the briefs index
    and `CLAUDE.md`.

## 3. `crates/git`

A `Repo` holds paths only and opens libgit2's repository per call, so it is `Send + Sync`; the shell's service
serializes changes. Modules: `status` (rename detection both ways, ahead/behind, the operation in progress, stash count,
ignored folders without descending, the `GlyphIndex`), `index` (stage resolves conflicts, unstage handles both paths of
a rename and an unborn branch, discard restores from the index and deletes untracked files), `commit` (the identity
rule; merge and cherry-pick completion), `diff` (the texts; binary by git's NUL probe), `log` (topological + time walk,
path filter as `git log -- path`, paging, refs labels, the graph lanes with at most 32 lanes, cancel every 64 commits),
`branches` (list, delete with the merged rule, checkout with safe-checkout conflict reporting, remote branches as
tracking branches, merge, abort, rebase start/continue/abort, cherry-pick, reset), `stash` (a conflicting pop keeps the
stash and leaves the markers), `remote` (fetch, pull, push), `worktree` (`<main>/.worktrees/<name>` by default; remove
refuses a dirty one), `blame`, and `watch` (`StatusCache`, `Watcher`).

Findings:

- **Non-fast-forward pushes**: libgit2's local transport does not refuse them, and `Remote::list()` in git2 0.20
  panics on an empty remote (a null slice). The push checks in `push_negotiation`, before anything is sent, so it
  refuses on every transport.
- **Cherry-picks made in the same second** as their original, onto the same parent, are the same object: nothing to
  pick. Not a bug, but it bit a test.
- **Ties in the graph**: commits made in one second can come in a different order on the next walk.

## 4. The permission table

| Command | Declared class | For an agent |
|---|---|---|
| `status`, `diff`, `log`, `blame` | read | always allowed |
| `branches` (list), `stash` (list), `worktrees` (list) | read | always allowed |
| `branches` `delete` | read, raised | execute; `force` refused |
| `stash` `push`, `apply`, `pop` | read, raised | execute |
| `stash` `drop` | read, raised | dangerous |
| `worktrees` `add`, `remove` | read, raised | execute; `force` refused |
| `init`, `stage`, `unstage`, `checkout`, `merge`, `cherry_pick`, `fetch`, `cancel` | execute | the policy's `execute` (prompt by default); `checkout` `force` refused |
| `commit` | execute | `git.commit`: `allow` (default: execute), `prompt` (dangerous), `deny` (refused) |
| `commit` with `amend`, `reset`, `rebase`, `pull` with `rebase`, `merge` `abort` | execute | `git.history`: `prompt` (default: dangerous), `deny` |
| `reset` `hard` | execute, raised | dangerous (and `git.history`) |
| `push`, `sync` | execute | `git.push`: `prompt` (default: dangerous), `deny`; `force` refused |
| `discard` | dangerous | dangerous |
| `pull` (no rebase) | execute | execute |

Tool rules are checked first (as the debug policy's): a matching rule turns a policy refusal into a prompt the rule
decides; a `force` is refused whatever the policy and the rules say (`policy::GIT_FORCE_REFUSED`). For the user the
same commands run without the gate; Visual Studio's confirmations ask instead.

## 5. Credentials

`remote.rs`'s callback, per libgit2's request, tries once each: for `SSH_KEY`, the ssh agent (`SSH_AUTH_SOCK`,
`Cred::ssh_key_from_agent`); for `USER_PASS_PLAINTEXT`, the `credential.helper` the repository's or global config
names (`Cred::credential_helper`); for `DEFAULT`, the system's default credentials; then it fails with "Authentication
failed for URL: tried the ssh agent (SSH_AUTH_SOCK), then the credential helper `store`. Set up a credential helper
(`git config --global credential.helper <helper>`, such as Git Credential Manager or `store`), or for ssh add your key
to the ssh agent (`ssh-add`)." (unit-tested; no authenticating server was available here).

Two limits, both of the build rather than the code:

- **No https or ssh transport.** The system libgit2 here is 1.7.2, older than libgit2-sys 0.18 accepts, so the
  bundled libgit2 is built, and the workspace's `git2` has `default-features = false`: no `https` (OpenSSL) and no
  `ssh` (libssh2) features. Local paths, `file://`, `git://` and plain `http://` remotes work; `https://` and ssh
  remotes fail at once, before any network: https with libgit2's "there is no TLS stream available", ssh as an
  unsupported protocol (`tests/remote.rs::https_and_ssh_remotes_need_transports_this_build_lacks`). Turning the features on adds `openssl-sys`,
  `openssl-probe` and `libssh2-sys` (new dependencies: the brief allows none). The owner's decision; the callback
  above is ready for them.
- **A helper named without a path** (`store`, `manager`) is run by git2 as `git credential-<name>`: that is a `git`
  process, started only when an http remote asks for credentials, never on a hot path. Git Credential Manager and
  `store` are such helpers; a helper given by absolute path, or as a `!command`, is run directly.

## 6. Tests

What each proves:

- **`crates/git`** (26 unit, 7 integration): status groups, renames, ignored folders and glyphs; detached HEAD and
  path normalization; discover and init with the refusal inside a repository; stage, unstage (unborn branch too),
  discard (untracked deleted; staged back to HEAD); Commit All, Amend, nothing to commit; **the identity rule and its
  refusal** (repository config, then a global file, then the settings; the message names `git config --global
  user.name` and `git.userName`); diff texts against the index, HEAD, a revision, staged, new and binary files;
  **graph lanes of a merge-heavy history** (exact edges) and the 32-lane bound; log with refs, path filter, paging and a
  pre-set cancel; branches, checkout, New Branch, delete with the merged rule; **checkout of a dirty tree refused** with
  the paths, a non-conflicting change carried, force; **merge**: fast-forward, up to date, **conflict with markers,
  staging resolves, commit makes the merge commit**, a clean merge commit, abort; **rebase** stopping at step 2 of 2,
  continue refused while conflicted, continue, abort, up to date; **cherry-pick** clean and conflicted, completed by
  commit; reset soft, mixed, hard; **stash** push (untracked too), list, apply, pop, drop, **a conflicting pop keeps
  the stash**; **worktree add, list and remove** (dirty refused, force, a given path, an existing branch); **blame**
  per line; the watcher: **generation grows only on change, an index write bumps it, a touch and the rescan find
  working-tree changes**; credential messages; an unknown remote and a canceled fetch. Integration
  (`tests/remote.rs`, a bare repository on disk): **fetch updates the incoming count, pull fast-forwards**; ahead
  counts, **a non-fast-forward push refused**, pull merges, push, force push; a new branch pushed with its upstream, a
  remote branch checked out as a tracking branch, **a pull that conflicts stops**, a canceled push; https and ssh
  remotes refused by this build's libgit2. `tests/budget.rs`:
  the 10,000-file status, the merge-heavy graph through `log`, **a 5,000-commit log canceled mid-walk**.
- **`crates/commands`**: every git schema parses, its title names the command, inputs reject unknown members; parsing
  and validation of every command; **every output conforms to its schema** (with a small checker: required, enums,
  patterns, integers); classes; **the `git` policy object** (defaults, prompt, deny, tool rules, force refused); an
  agent's forced push is refused before the handler and the user's reaches it; the policy file round-trips the `git`
  object and follows the schema; the Integer setting kind.
- **`crates/mcp`**: the guides list (two now), **`eludite://git/status` read through the bus as the agent** (audited),
  its schema; the git guide under 800 words naming only real commands.
- **`crates/docking`**, **`crates/ui`**: the tab badge, the slot actions; the default-layout tests with the new window.
- **`crates/eludite` headless** (`shell/git_tests.rs`, 20 tests, real libgit2 and the real watcher threads): the window
  lists the groups with their glyphs; **Stage moves a file between groups**, Unstage, Stage All; **Commit All commits
  with the message and clears the box; Amend replaces the commit**; an empty message is refused in the window; **Undo
  Changes asks** (No keeps the file, Yes restores it); Reset (Delete Changes) asks; **the status bar shows the branch,
  the pending count and Publish; the Workspace glyph and the tab glyph change with the file** (modified, untracked,
  added, cleared by a commit); **Compare with Unmodified opens two panes with the right hunks, F8 and Shift+F8 move**,
  the title, the agent's capped answer, the 100 ms budget; Ctrl+D in the Workspace window and the context menu's
  Blame; **the margin appears after an edit (not before the editor is idle) and clears after a stage**; **checkout of a
  dirty tree is refused with the message; New Branch creates and switches** (Git > New Branch... focuses the box);
  **a merge conflict opens the file with markers, the margin shows the sides, staging resolves it**, the commit ends
  the merge; **Fetch (the status bar) against the bare remote updates the incoming count**, the incoming arrow pulls;
  **Push asks for an agent** (the MCP gate sees class dangerous with `git.push: prompt`, nothing is pushed; a force is
  refused without reaching the gate) **and runs for the user**; **a stale status (older generation) is not drawn**, a
  newer one is; **an agent's `status` answer matches the window** (generation, groups, branch); **no repository shows
  Create Git Repository, and it creates one**; **the draft message survives closing and reopening the workspace**;
  1,000 changed files drawn under 8 ms; `status` p95 from the cache on 10,000 files; the identity from the settings
  after the refusal, the Options page's whole-number setting (`git.autoFetchMinutes` 15 schedules the fetch, 0 stops
  it); the Git menu's items, Open in File Explorer, Manage Branches with the history, Ctrl+0, Ctrl+G and Ctrl+0,
  Ctrl+R, the status bar's branch; Stash All, the stash list, Pop, Drop asks; the Git Repository window's Cherry-Pick
  and Reset (Keep Changes).

Counts: section 10.

## 7. Deviations and gaps

1. **`notify` is not in the build.** `Cargo.lock` has `notify-rust` (desktop notifications), not `notify`. Adding it
   would be a new dependency, which the brief forbids, so the watcher polls as the settings store does: stats of
   `.git/index`, `HEAD`, the operation files, `packed-refs`, `FETCH_HEAD` and up to 2,000 files under `refs/` every
   100 ms, a recompute 200 ms after the last change seen or after a save (`Watcher::touch`), and a working-tree rescan
   every second, or ten times the last status's cost when that is longer (a 30 ms status rescans every second, 3% of a
   core). Changes made outside Eludite to the working tree are therefore seen within about a second.
2. **https and ssh remotes** do not work in this build (section 5).
3. **Files outside the brief's list**, each a hook or a widget piece: `crates/docking/src/controller.rs` and `view.rs`
   (the tab badge), `crates/ui/src/status.rs` (clickable slots), `tree.rs` (the badge), `lib.rs` (exports);
   `crates/commands/src/settings.rs` (the Integer kind for `git.autoFetchMinutes`), `lib.rs`;
   `crates/eludite/src/shell/explorer.rs` (glyphs, the git context items, Ctrl+D), `documents.rs` (four hook lines:
   opened, edited, saved, closed), `settings.rs` (two: the workspace and the settings), `options.rs` (whole numbers),
   `crates/eludite/Cargo.toml` (`eludite-git`), `protocol/schemas/mcp-resource.json`. `shell.rs` gains the module, the
   `Services` fields, the registration, the `git` field, the two body closures' parameters, the `run` hook, the probe
   and the install call.
4. **Visual Studio's dialogs not reproduced**: New Branch is a box in the Git Repository window (no "Based on" picker:
   the selected branch or commit is the start point); Blame is a document, not an editor margin; Commit or Stash...
   shows Git Changes without focusing the box.
5. **The Compare document is a snapshot**: it does not follow later edits (open it again).
6. **The automatic fetch** is tested to be scheduled and stopped; its timer was not let run to a fetch in a test.
7. **Windows and macOS** were not run.

## 8. What later work needs

- **Commit signing (GPG, SSH)**: libgit2 creates the commit object, so signing means `Repository::commit_create_buffer`,
  then running the signing program (`gpg --detach-sign`, or `ssh-keygen -Y sign` for `gpg.format = ssh`) on the buffer,
  then `Repository::commit_signed`, then moving the branch. That is a process per commit and a passphrase prompt (a
  pinentry or an agent), so a dialog in the shell and a policy for agents (an agent should not unlock a key). The
  setting `git.gpgSign` exists with only `never` until then; `commit.gpgSign = true` in a user's config is ignored and
  should at least be reported.
- **PR integration (GitHub, GitLab, Azure DevOps)**: an https transport first (section 5), then each host's REST API
  with a token from the credential helper or a sign-in, a provider trait in a new crate, and commands
  (`eludite.pr.*`) with schemas: list, create from the branch, view checks, comments as review threads. Visual
  Studio's "Create a Pull Request" link after a push is the entry point.
- **The three-way conflict editor**: libgit2 gives the three sides (`IndexConflict` ancestor, ours, theirs) and
  `merge_file` for a result; the editor needs a document with three read-only panes and an editable result, conflict
  navigation, Take Left / Take Right / Take Both per hunk, and `eludite.git.stage` on accept. Today's path (markers in
  the file, the margin's sides, Stage resolves) is the fallback it keeps.

## 9. For the merge

- **Commit `b32d676` does not build on its own**: it uses `GitRequest::Sync`, which `e7498c3` adds. Merge the branch as
  a whole (not commit by commit); a bisect over it should skip `b32d676`.
- Shared files with brief 0037: `docs/briefs/README.md` (the 0040 row and the guides line), `crates/ui/src/menu.rs`
  (the Git menu, a View item, `WORKSPACE_GIT_ITEMS`), `crates/docking/src/model.rs` (`GIT_REPOSITORY`, one registry
  entry, two test lists), `crates/eludite/src/shell.rs` (listed in section 7.3). The `Services` struct and
  `register_workspace` gain fields at their ends, `Shell::new`'s `tool_body` and `document_body` calls gain arguments.
- `protocol/schemas/settings.json` adds the section "Source Control > Git Global Settings" last, so earlier Options
  pages keep their indexes.

## 10. Counts

This machine, `DISPLAY=:99`, `CEF_PATH`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1` and `ELUDITE_DBG_MONO` set,
the corpus built (`bash corpus/tests/build.sh`), `dotnet build dotnet/Eludite.slnx` first:

- `cargo test --workspace --no-fail-fast --features eludite-chromium/cef`: **862 passed, 2 failed, 1 ignored**. The
  two failures were `shell::debug::conformance_tests::{lldb,mono}_attach_detach`: the replay's attach found a real
  process (pid 784) where the golden has `${PID}`, while the other worktree's agent was running its own debugger tests
  beside this run; both **pass when run again alone** (2 passed, 0.41 s), and nothing in this brief touches the
  debugger. The shell's git tests: 20 passed. Then one more `crates/git` test (the transports), passing.
- `cargo fmt --check`: clean. `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`:
  clean. `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors. `dotnet test` was not rerun: no .NET file changed.

## 11. How to reproduce

```
cargo test -p eludite-git -- --nocapture            # the library, the bare-remote tests and the budget lines
cargo test -p eludite --bins git_tests -- --nocapture --test-threads=1   # the shell; prints the timing lines
crates/eludite/tools/git-linux.sh OUT_DIR           # Xvfb, xdotool, jq, ImageMagick; the screenshots below
```

## 12. Screenshots (Xvfb run)

`crates/eludite/tools/git-linux.sh` on a fixture repository (a merge in its history), in
[0040-run/screenshots](0040-run/screenshots/):

- `changes.png`: a line added to `src/lib.rs` and saved: ✓ on `lib.rs` and ? on `notes.md` in the Workspace window,
  ✓ on the tab, the green change margin on line 15, the status bar's `⎇ main`, Fetch, `↑ Publish`, `✎ 2`.
- `git-changes.png`: Ctrl+0, Ctrl+G: the Git Changes window with Changes (2).
- `compare.png`: a double-click on `lib.rs`: `lib.rs vs. lib.rs (index)`, the added line on the right, "1 difference".
- `committed.png`: a message typed and Commit All: "Commit 5aec85c created locally on main", Changes (0), the tab's
  and the status bar's marks gone.
- `repository.png`: Ctrl+0, Ctrl+R: the branches and the graph (the merge's two lanes), `HEAD -> main` and `feature`.
