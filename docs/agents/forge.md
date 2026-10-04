# Pull requests and issues in Eludite: a guide for agents

Eludite's forge commands (`eludite.forge.*`) are the ones the Pull Requests and Issues windows run, on GitHub, GitLab
(merge requests), Azure DevOps (work items), Forgejo and Gitea (Codeberg), and Tangled. They work on the forge the
workspace repository's remote lives on. Use them rather than a forge's CLI or web API: they share the person's
sign-in, cache and policy.

## 1. Know the forge

`eludite.forge.detect` answers the `family`, the repository, whether the person is `signed_in`, and the
`capabilities`: only call what they allow (Tangled has no reviews or line comments; Forgejo cannot resolve threads).
`family: none` means no supported forge.

## 2. Answer the review comments on your pull request: the loop in four calls

1. Read the resource `eludite://forge/pull/current`: the current branch's pull request with only its unresolved
   review threads (`{"pull": null}` when the branch has none). Or call `eludite.forge.pull` with `number` and
   `threads: "unresolved"`. Each thread has its `id`, `path`, `line` (in the head commit), `side` and `comments`.
2. Fix the code: `eludite.workspace.*` edits, then `eludite.git.commit` and `eludite.git.push`.
3. Answer each thread with `eludite.forge.pull_comment` (`number`, `body`, `reply_to`: the thread's `id`), and
   resolve it with `eludite.forge.thread_resolve` (`thread`) where the forge supports it.
4. Ask for another look with `eludite.forge.pull_review` (`action: "request"`); without `reviewers` it asks the
   previous reviewers again.

## 3. Reading

Reads answer from the cache at once. `stale: true` with `age_seconds` means a refresh is running; pass
`refresh: true` to wait for a fresh answer. `refresh_error` says why a refresh failed (rate limited until `reset_at`,
network, sign-in) while the cached answer is kept.

- `eludite.forge.pulls`: `filter` (`all`, `mine`, `review_requested`), `state`, `text`, `max` (default 50, at most
  200) and `cursor` (the `next_cursor` of the previous page).
- `eludite.forge.pull`: description (Markdown), commits, files, threads, reviews, `check_items`, `mergeable`.
- `eludite.forge.issues` and `eludite.forge.issue`.
- `eludite.forge.checks` (a commit's checks, or a pull request's head with `number`) and `eludite.forge.check_log`
  (`id` from a check with `has_log`; paged by `offset` and `max_bytes`).

Tangled's items have no numbers: pass the `id` (an `at://` uri) a list answered.

## 4. Writing

`pull_comment` (on the conversation, on a line with `path` and `line`, or `reply_to`), `pull_review` (`start`, `add`,
`submit` with `event` `approve`, `request_changes` or `comment`, `discard`, `request`), `pull_create`, `pull_update`,
`pull_ready`, `thread_resolve`, `issue_create`, `issue_comment`, `issue_update`, `branch_from_issue` and
`check_rerun` write to the forge. A review's comments are held and sent together on `submit` where the forge has
pending reviews; elsewhere each posts at once (`pending: false` says which).

`pull_checkout` fetches a pull request's head into `pr/<number>` (GitLab: `mr/<number>`) and checks it out; a dirty
working tree it would overwrite is refused.

## 5. What needs permission

The solution's policy (`.eludite/agents-policy.json`, the `forge` object) decides:

- Reads always run, unless `forge.read` is `deny`.
- Writes follow `forge.write`: `prompt` (the default) asks the person before your first write of the session; their
  "Allow for this session" lets the rest run. `allow` runs them; `deny` refuses them.
- `pull_merge` and `pull_close` follow `forge.merge`: `deny` (the default) refuses them, `prompt` asks every time.
  A merge's `force` is always refused for you. Leave merging to the person unless they asked.
- `eludite.forge.auth` `sign_in` and `sign_out` are always refused: you never sign in.

## 6. When something is refused

- `sign_in_required: <host>`: no token is stored for the host, or the forge refused it. Tell the person to sign in
  (Git > Sign in to the forge); do not retry another way.
- `rate_limited`: wait until `reset_at`; the cached answers still read.
- `merge refused: ...`: conflicts, a draft, a block, or failing checks. Report it.
- `... does not support ...`: the capability is false for this forge.
