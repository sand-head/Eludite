# Git in Eludite: a guide for agents

Eludite's git commands (`eludite.git.*`) are the ones the Git Changes and Git Repository windows run. They work on the
repository the open workspace is in, through libgit2: no `git` process is started, so prefer them to running `git`
in a shell. Every answer that changes the repository carries a `generation`; `eludite.git.status` answers with the
same number, so a status older than your last change is stale.

## 1. Read the status first

Call `eludite.git.status` (or read the resource `eludite://git/status`, the same answer). It returns at once from the
status Eludite keeps, which a watcher recomputes when the index, HEAD, the refs or the working tree change:

- `branch`, `upstream`, `ahead` (outgoing) and `behind` (incoming, as of the last fetch);
- `staged` (Staged Changes), `unstaged` (Changes), `untracked`, `conflicted`, each path relative to `repository`;
- `operation` when a merge, rebase or cherry-pick stopped at a conflict;
- `state: none` when the workspace is in no repository (`eludite.git.init` creates one).

Lists stop at `max_items` (default 1000); `totals` counts them all.

## 2. Look at a change before you stage it

- `eludite.git.diff` with `path` gives the hunks of the working tree against the index (`against: head` or a
  revision for others; `staged: true` for the index against HEAD), at most 2,000 changed lines.
- `eludite.git.log` gives the history (`max`, `skip`, `path`, `all`); `eludite.git.blame` gives each line's commit.
- `eludite.git.branches` lists branches with their upstreams and counts, and the tags.

These are class read: they never ask the user.

## 3. Stage and commit

1. `eludite.git.stage` with `paths` (or `all: true`). Staging a conflicted file marks it resolved, so edit the
   conflict markers away first.
2. `eludite.git.commit` with a `message`: one plain line saying what the change does. `all: true` stages everything
   first (Commit All).
3. If the commit is refused for its identity, do not invent one: tell the user to run
   `git config --global user.name` and `user.email`, or set git.userName and git.userEmail in Tools > Options.

`eludite.git.unstage` takes files back out of the index; `eludite.git.discard` (Undo Changes) throws working-tree
changes away and deletes untracked files: it is class dangerous, so the user is asked, and the change is lost.

## 4. Branches, history and sync

- `eludite.git.checkout` switches branch (`create: true` for a new one, from `start_point`). Local changes the
  switch would overwrite refuse it with their paths: commit or stash them (`eludite.git.stash` with `action: push`).
- `eludite.git.merge`, `eludite.git.rebase` and `eludite.git.cherry_pick` stop at the first conflict with
  `result: conflicts` and the paths. Fix each file, stage it, then commit (merge, cherry-pick) or call
  `eludite.git.rebase` with `action: continue`. `abort` returns to where you started.
- `eludite.git.fetch` updates `behind`; `eludite.git.pull` fetches and fast-forwards or merges (never rebases unless
  `rebase: true`); `eludite.git.push` sends the branch (`set_upstream` for a new one). A push the remote would refuse
  as a non-fast-forward is refused before anything is sent: pull first.
- `eludite.git.worktrees` lists, adds (under `.worktrees/<name>`) and removes worktrees.

## 5. What needs permission

| Calls | Class for an agent |
|---|---|
| status, diff, log, blame, branches, the stash and worktree lists | read: always allowed |
| stage, unstage, commit, checkout, merge, cherry-pick, fetch, pull, stash push, apply and pop, worktree add and remove, branch delete, init | execute: the policy's `execute` (ask by default) |
| discard, `reset` with `mode: hard`, stash drop | dangerous: the user is asked |
| push | dangerous while the policy's `git.push` is `prompt` (the default); refused when `deny` |
| commit with `amend`, reset, rebase, pull with `rebase`, merge `abort` | dangerous while `git.history` is `prompt` (the default); refused when `deny` |
| a commit | the policy's `git.commit`: `allow` (default: execute), `prompt` or `deny` |
| any `force` (checkout, push, branch delete, worktree remove) | refused outright: ask the user to do it in the IDE |

The policy is `.eludite/agents-policy.json` beside the solution. A refused call says which key refused it; do not
retry it another way, tell the user.

## 6. Limits

Commits are not signed. Remotes over `https://` and ssh need a libgit2 built with those transports, which this build
is not: local paths, `file://`, `git://` and `http://` remotes work. Interactive rebase, submodules and LFS are not
offered.
