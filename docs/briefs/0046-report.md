# Brief 0046 report: forge integration

Status: done on Linux, against recorded and synthesized fixtures; no real-service write ran (no tokens here, section 4).
Windows and macOS: not built here (no targets or machines); nothing in the forge crate is platform-specific except the
credential store's backend, which `keyring` picks per platform. CI: not run (nothing pushed).
Branch: `brief/0046-forge-integration`, based on `main` at `8f38a0f`; not rebased (the coordinator merges).
Date: 2026-10-04. Brief: [0046-forge-integration.md](0046-forge-integration.md).

## 1. Summary

- **`eludite-forge`** (new crate, `crates/forge`): one `Forge` trait and a capabilities table (section 3) over
  GitHub (REST `2022-11-28` plus GraphQL for thread resolution, draft and ready, auto-merge and linked branches),
  GitLab (REST v4: merge requests, discussions, draft notes, approvals, head pipelines and jobs), Azure DevOps (REST
  7.1: pull requests, threads, votes, iterations, policy evaluations, builds, work items through WIQL and JSON Patch),
  Forgejo and Gitea (`/api/v1`, one implementation, told apart by the version probe) and Tangled (the appview's XRPC
  reads and `com.atproto.repo.createRecord` writes under `sh.tangled.*`, handles through the PLC directory and
  `did:web`). Detection from the remote url and `forge.hosts` (looked up by the remote's host and by the forge's web
  host, so `tangled.sh` maps to `tangled.org`), with version probes for unknown hosts (`/api/v4/version`,
  `/api/v1/version` and `/api/forgejo/v1/version`, `/api/v3/meta`). The HTTP client on `ureq` 3 with `rustls`:
  conditional requests (a 304 answers the kept body), rate limits (`X-RateLimit-*`, `RateLimit-*`, the IETF
  structured `RateLimit` header Codeberg sends, 429 with `Retry-After`) answered with the reset time and no further
  request until then, a 30 s timeout, cancellation between pages, the proxy from the environment (loopback bypasses
  it), `SSL_CERT_FILE` roots. A 203 or an HTML body (Azure DevOps' sign-in redirect) is `sign_in_required`.
- **Stale while refreshing.** Every read goes through a cache under the workspace's `.eludite/forge/` (never a token;
  a test greps every cache file for it): a cached answer comes back at once with `stale`, `fetched_at` and
  `age_seconds` while one background refresh per key runs; a failed refresh keeps the cache and the answer carries
  `refresh_error`. Each repository has a generation (workspace, account); a refresh of an older generation is dropped.
  A freshness window (30 s) keeps a window that redraws on each refresh from refreshing again, and keeps a failed
  refresh from being retried in a loop.
- **Credentials.** The operating system's store through `keyring` 4 (Secret Service over zbus on Linux, Keychain,
  Credential Manager), service `eludite-forge`; when the store is unavailable the sign-in dialog offers a file under
  the user's config directory (`eludite/forge-credentials.json`, mode 0600) and uses it only after the person ticks
  it. Sign-in by personal access token (every family; validated by reading the account), the OAuth device flow
  (GitHub, GitLab, Azure DevOps' Microsoft identity platform; the client ids are build-time constants the owner sets,
  or `forge.githubClientId`, `forge.gitlabApplicationId`, `forge.azureApplicationId`), the `gh` or `glab` CLI's token
  (a process started only at sign-in), and Tangled's handle and app password (an atproto session, kept an hour). An
  OAuth token is refreshed once on `sign_in_required`. `Debug` of a secret prints nothing; the audit replaces bodies
  by their length and drops tokens.
- **`eludite.forge.*`** (23 commands, schemas `protocol/schemas/forge-*.json`, checked before the shell sees them):
  detect, auth, pulls, pull, pull_create, pull_update, pull_comment, pull_review, pull_checkout, pull_merge,
  pull_close, pull_ready, thread_resolve, issues, issue, issue_create, issue_comment, issue_update,
  branch_from_issue, checks, check_log, check_rerun, refresh. Classes: reads `read`; writes `execute` under the
  policy's `forge.write` (`prompt` by default: an agent's first write asks, "Allow for this session" holds); merge and
  close `dangerous` under `forge.merge` (`deny` by default for agents; `force` never for agents); `sign_in` refused for
  agents. `eludite://forge/pull/current` and `eludite://guides/forge` ([docs/agents/forge.md](../agents/forge.md),
  603 words) for agents.
- **The shell** (`crates/eludite/src/shell/forge.rs` and `forge/`): View > Other Windows > **Pull Requests** (Mine,
  Review requested, All; Open, Closed, Merged, All; search; Refresh; the "Not signed in" line with Sign in; the
  "Showing cached results from <time> (<age>)" banner with the reason when a refresh failed; virtualized rows) and
  **Issues** (filters, search, New Issue, the selected issue with its fields and conversation, Comment, Close or
  Reopen, Create Branch). The **pull request document**: header (state, branches, checks glyph, mergeability,
  unresolved threads, iterations), Refresh, Check Out, Ready for review, the merge method choice and Merge, Close or
  Reopen, the review bar (Start Review, the pending count, Submit as Approve, Request Changes or Comment), tabs
  Overview (description and conversation as Markdown through brief 0043's renderer, links open in the browser;
  reviewers; approvals), Commits, Files (a double-click opens brief 0040's Compare document of the base and head
  blobs, read from the local repository), Threads (Resolve or Unresolve where the forge allows), Checks (Log opens a
  read-only, virtualized document capped at 2 MB with the url for the rest; Rerun where allowed); lists virtualized.
  **Review threads in the editor's margin**: on the checked-out branch (the pull request's head, or `pr/<n>` or
  `mr/<n>`), a glyph with the comment count at each thread's line of an open file; clicking opens the thread with a
  reply box (Comment posts at once, Add to Review joins the pending review); a "+" at the caret's line starts a new
  line comment. **Create Pull Request** (Git > Create Pull Request, and Git Changes' "Create a Pull Request" link
  shown after a push or while the branch is ahead of its upstream): title from the branch's single commit (or its
  name), body from the commits, base from the remote's default branch, Draft, reviewers and labels where the forge
  takes them. **Sign in** (Git > Sign in to Forge..., the windows' Sign in button): the methods the forge takes, the
  device code with Copy and Open the page, the token box as bullets, the file consent. The **status bar**: the current
  branch's open pull request with its checks as a glyph and a count (`⇄ #12 ✗ 2/3`), clicking opens it. Merge and
  Close ask first, naming the method. Every button runs the same command an agent calls.
- **Nothing at startup.** Detection, lists and checks run only when a window opens or a command runs; the fixture
  server counts zero requests until then (shell test and Xvfb run). The forge's calls run on background threads with
  the bus's cancellation and the repository's generation; the UI thread applies answers.

## 2. Commits

| Commit | What it adds |
|---|---|
| `8d56828` Mark brief 0046 in progress | the brief's status |
| `848bdc1` the forge command schemas, the policy's forge object, the forge settings and the forge MCP resource | 46 `forge-*.json` schemas (alone, before any code), `agents-policy.json`'s `forge`, the "Source Control > Forges" settings, `view-show` and `mcp-resource` additions |
| `51f2b90` eludite-forge | the crate: trait, five families, client, cache, credentials, hub, ops, replay transports |
| `709bfa7` the forge fixtures, the recorder, the pinned API descriptions and the fixture and real-service tests | `crates/forge/testdata/`, `tools/forge-corpus/`, `protocol/forge/`, `crates/forge/tests/` |
| `27f84d5` the eludite.forge commands, the policy's forge object, the audit redaction, the guide and the resource | `crates/commands/src/forge.rs`, `policy.rs`, `registry.rs` (audit redaction), `crates/mcp` (guide, resource, tests), `docs/agents/forge.md` |
| `22dfa56` a refspec fetch to eludite-git | `Repo::fetch_refspec` for checking out `refs/pull/<n>/head` and the like (outside the brief's files: section 9) |
| `c852855` a freshness window in the hub | no refresh loop from a window that redraws on each refresh |
| `f9d80d2` the shape check accepts every schema member | `head`, `offset`, `max_bytes`, `type` were refused; a test now walks every schema |
| `e342de7` the shell | windows, document, form, dialog, margin, status bar, link, menus, the two window ids, headless tests |
| `d6a0721` probes and layout | `--bounds-out` keys for the forge views, the fixture server example, steadier layouts |
| `a602d7b` the Xvfb run | `crates/eludite/tools/forge-linux.sh` and [the screenshots](0046-run/screenshots/) |

## 3. Capabilities as shipped

| Capability | GitHub | GitLab | Azure DevOps | Forgejo, Gitea | Tangled |
|---|---|---|---|---|---|
| pull requests | yes | yes (merge requests) | yes | yes | yes (no numbers: AT-URI ids) |
| create a pull request | yes | yes | yes | yes | no |
| reviews | yes | yes | yes (votes) | yes | no |
| pending reviews | yes (held locally, one call) | yes (draft notes on the server, bulk publish) | no (each comment posts) | yes (held locally) | no |
| review threads | yes | yes (discussions) | yes (threads) | yes (grouped by path, line, side) | no |
| thread resolution | yes (GraphQL) | yes | yes (thread status) | no (no endpoint) | no |
| draft pull requests | yes | yes (`Draft:`) | yes | yes (`WIP:` prefix) | no |
| merge methods | merge, squash, rebase | merge, squash, fast-forward, semi-linear (as the project allows) | merge, squash, rebase, rebase-merge | merge, squash, rebase, rebase-merge, fast-forward | none |
| merge when checks pass | yes | yes (merge when pipeline succeeds) | yes (auto-complete) | yes | no |
| request review | yes | yes | yes | yes | no |
| issues | yes | yes | yes (work items) | yes | yes |
| create issues | yes | yes | yes (default type Issue, Bug or Task by process) | yes | yes |
| labels, milestones, assignees | yes | yes | tags, iteration path, assigned to | yes | no |
| checks | check runs and statuses | head pipeline and jobs | builds and policy evaluations | statuses (newest per context) and Actions jobs | no |
| check logs | Actions jobs | job traces | build logs | Actions job logs | no |
| check reruns | Actions jobs | job retry | build retry, policy requeue | no | no |
| branch from issue | yes | yes | yes | yes | yes |
| branch link recorded | development panel (`createLinkedBranch`) | a note on the issue | ArtifactLink relation on the work item | the issue's `ref` | a comment |
| family extras | | approvals (`given` of `required`) | votes (approve, approve with suggestions, waiting for author, reject), iterations, work item types | | |

Checkout refs: `refs/pull/<n>/head` into `pr/<n>` (GitHub, Forgejo, Gitea), `refs/merge-requests/<n>/head` into
`mr/<n>` (GitLab), the source branch into `pr/<n>` (Azure DevOps), the head branch from its clone url (Tangled and
forks), each through `eludite.git.checkout` with brief 0040's dirty-tree refusal.

## 4. Per family: the live services, what changed, and the real tests

Reachability from this container on 2026-10-04 (through the agent proxy):

| Service | Reachable | What the first step found and what was corrected |
|---|---|---|
| github.com | **no**: api.github.com answers 403 through the proxy (raw.githubusercontent.com and `git ls-remote` work) | Nothing could be verified live. Every GitHub fixture is synthesized from the pinned OpenAPI description (`protocol/forge/github/`, `2022-11-28`); the recorder and `live.rs` cover github.com on the owner's machine. GraphQL is used for thread resolution (threads keyed by their first comment's `databaseId`), draft and ready, auto-merge and `createLinkedBranch`. The merge queue is not reached (no fixture, no live service): `when_checks_pass` maps to auto-merge. |
| codeberg.org (Forgejo) | yes, anonymous: Forgejo `16.0.0-dev-753-6bcc6da0+gitea-1.22.0` | Recorded `forgejo/forgejo` pull 14628 and issue 14684. Corrections: a changed file's status is `changed`, not `modified`; there is no endpoint to resolve a review thread (capability off); commit statuses repeat per context (the newest per context is kept); an Actions log is found by `runs?run_number=`, then the run's jobs, then the job's log (looking runs up by head sha missed some); reruns are not exposed (off). Codeberg sends the IETF `RateLimit` header, which the client reads. The version's `+gitea-` suffix (or `/api/forgejo/v1/version`) tells Forgejo from Gitea. |
| gitlab.com | yes, anonymous | Recorded `gitlab-org/cli` merge request 3992 and issue 8577. Corrections: `/api/v4/version` and `/metadata` answer 401 without a token, so the probe counts a 401 with GitLab's message as GitLab; discussions, notes and job traces need a token (anonymous 401 or 403 reads as no threads, and the fixtures for them are synthesized); a fork's merge request has no pipelines by sha in the target project, so checks come from the merge request's `head_pipeline` and its `project_id`. GitLab's OpenAPI description is CC BY-SA 4.0, not MIT as the brief assumed: it is pinned by commit in `protocol/forge/gitlab/PIN` and not copied. |
| dev.azure.com | yes, anonymous for public projects | Recorded `dnceng-public/public/dotnet-public-wiki` pull request 5. Corrections: iterations, policy evaluations and WIQL redirect an anonymous reader to sign-in with a 203 and an HTML page, now `sign_in_required` (synthesized fixtures cover them); comments need `7.1-preview.4` and policy `7.1-preview.1`. |
| tangled.sh | yes: `tangled.sh` redirects (301) to `tangled.org`; the appview's XRPC at `api.tangled.org` serves `listPulls`, `getPull`, `pull.listStatuses`, `compare`, `feed.listComments`, `listIssues`, `getIssue`, `issue.listStates` | **Pull requests are reachable, so the Tangled part was not reduced.** Recorded `tangled.org/core` and a merged pull request (`at://did:plc:xasnlahkri4ewmbuzly2rlc5/sh.tangled.repo.pull/3mwqc5pt6dc5d`). Corrections: items have AT-URI ids and no numbers (the schemas take `number` or `id`); comments are `sh.tangled.feed.comment` records (on a pull's latest round, `pullRoundIdx`); `compare` takes the repository's record AT-URI; `did:web` handles resolve through their own host, not plc.directory. Lexicons for reviews, line comments, drafts, labels, merging and creating pull requests were not found at the pinned commit (`3f736f0b`): those capabilities are off and the windows hide them. |

Real-service tests (`crates/forge/tests/live.rs`, five scenarios gated on `ELUDITE_GITHUB_TOKEN`,
`ELUDITE_GITLAB_TOKEN` with `ELUDITE_GITLAB_HOST`, `ELUDITE_AZDO_TOKEN` with `ELUDITE_AZDO_ORG` and
`ELUDITE_AZDO_PROJECT`, `ELUDITE_FORGEJO_TOKEN` with `ELUDITE_FORGEJO_HOST`, `ELUDITE_TANGLED_APP_PASSWORD` with
`ELUDITE_TANGLED_HANDLE`, plus a throwaway repository each): **none ran here** (no tokens); they skip and say so. The
recorder (`tools/forge-corpus/record.sh`, through `crates/forge/tests/record.rs`, gated on
`ELUDITE_FORGE_RECORD`) ran here anonymously against Codeberg, gitlab.com, Azure DevOps and Tangled: those are the
`recorded/` fixtures. Sign-in methods proven: token (all five, against fixtures), device flow (GitHub, GitLab, Azure
DevOps, against fixtures, and in the shell's dialog), CLI (`gh`, `glab`, with a script standing in), app password
(Tangled). None was proven against a real service.

Concepts of a forge's own with no place in the trait, and how they are shown: GitLab's approvals (a capability and
`approvals: {given, required}` on the pull request, shown in Overview) and draft notes (the trait's server-side
pending mode); GitLab's fast-forward and semi-linear merges (merge method values); GitLab's issue weight (an issue
field); Azure DevOps' votes (extra review states, "approved with suggestions" and "waiting for author"), iterations (a
count in the header), policies (checks with `policy:<id>`, requeue as rerun), work item types and fields (an issue's
`type` and its fields listed in the Issues window); Forgejo's `WIP:` drafts (the trait's draft flag); Tangled's
rounds (comments go to the latest round) and AT-URI ids (`id` in every schema, shown as the record key).

## 5. Budgets

| Budget | Result |
|---|---|
| A cached list renders within 50 ms of the window asking | **6.6 ms** p99 of 5 at load 9.1 (8.97 ms at load 1.6) (the shell's whole path: the window's Load event, the command off the UI thread, the cached answer drawn; `budgets_of_the_cached_list_the_large_document_and_memory`). Pass |
| A refresh of 50 pull requests within 2 s, 20 ms injected per request | **23.3 ms** (one page of 50; `a_refresh_of_fifty_pull_requests_with_20ms_per_request_is_under_2s`); the same list from the cache 0.43 ms. Pass. Not measured against a real service (no tokens; the anonymous recorder runs took about a second per family over the proxy) |
| The document with 100 files and 300 threads from the cache within 200 ms | **34.6 ms** at load 9.1 (23.1 ms at load 1.6), no request sent. Pass |
| Its frame p99 under 8 ms while scrolling | **5.3 ms** at load 9.1 (5.6 ms at load 1.6; best of three passes of 80 frames over Files and Threads; under the full suite's load it can exceed, which `assert_budget` does not assert above the core count). Pass |
| Shell memory, both windows and a 500-pull-request cache, under 40 MB more | **0.4 MB** more resident memory (VmRSS around loading and drawing both windows). Pass |
| Dependencies | `keyring` and its backends (section 8); `ureq`, `rustls` already in the build. Pass, with the macOS and Windows backends noted |

## 6. Tests and what each proves

`crates/forge` (unit tests in the modules, and `tests/`, all against the loopback fixture server through the real
`ureq` transport; a transport that panics on any non-loopback url guards them):

- `github.rs` (9): detection and the capabilities, nothing read; the list with filters (mine, review requested) and
  paging by cursor; one pull request with threads (a reply joins its thread; GraphQL's resolution), commits, files
  (rename), reviews, checks (two runs and a status), mergeability and the repository's allowed methods; create
  (title and body from the single commit), comment, line comment on the head commit, reply, a review of two pending
  comments sent in one call, re-request; checkout of `refs/pull/12/head` into `pr/12`, the refusals (checks failing,
  a method the repository disallows), merge, auto-merge, a forced merge reaching the forge, close, reopen, ready,
  draft, update, resolve and unresolve by node id; issues (a pull request is not an issue), labels filter, one issue,
  create with the milestone's number, comment, update, branch from issue with `createLinkedBranch`; checks, a log in
  two pages, rerun, a status has no log; sign-in by token (stored, never in the output), sign-out, the device flow
  (pending once, then the token), the `gh` CLI; the cache never holds the token.
- `forgejo.rs` (2): Codeberg's recorded reads (list, the pull request with its files and statuses, issues, an
  Actions log); writes, reviews with pending comments, drafts by `WIP:`, merge.
- `gitlab.rs` (3): merge requests with discussions, approvals and pipelines (recorded and synthesized); draft notes
  as the pending review and merge when pipeline succeeds; the device flow and the version probe.
- `azure.rs` (4): a recorded completed pull request; votes, iterations, policies as checks, auto-complete; work items
  as issues with their types; the device code flow.
- `tangled.rs` (2): only what the lexicons expose (no numbers, handles from the PLC directory, the merged pull
  request with files from `compare`, comments); an app-password session writing records.
- `common.rs` (10): unknown hosts by version probe (GitLab, Forgejo, Gitea, GitHub Enterprise); a 304 reads the kept
  body; a rate limit answers with its reset time and sends nothing more; a refused token is `sign_in_required`;
  cancellation between pages; a stale read answers the cache, refreshes, and a failed refresh says why; a refresh of an
  older generation is dropped; nothing reaches the forge before a command runs; the 50-pull-request budget; inside
  the freshness window a read neither refreshes nor repeats a failed refresh.
- Module tests: url parsing (scp, ssh, https, Azure's legacy and Server forms, Tangled), time and encoding helpers,
  the cache's file names and HTTP entries, secrets' `Debug`, the file store's 0600 mode, the client's rate-limit
  parsing and `Link` paging, the scrubber, the issue branch name.
- `record.rs` (gated) and `live.rs` (gated, five families): section 4.

`crates/commands/src/forge.rs` (6) and `policy.rs` (forge cases): every schema parses and names its command; the
classes; input checked before the shell sees it (unknown members, enums, number or id); every member of every item
schema passes the shape check; the forge policy for agents (read allowed, write prompt with the session grant,
`deny`, merge denied by default, `force` refused); an agent's sign-in refused and audited without its token.
`crates/mcp`: `the_forge_guide_resource_and_tools` (the guide, the current pull request resource, the tools'
classes); the resource and guide counts.

`crates/eludite/src/shell/forge_tests.rs` (14, headless, the fixture server and a memory credential store injected
through `ForgeService::set_setup`, a real repository made with git2): nothing reaches the forge before a window opens,
then the list filters (Mine, search) and a double-click opens the document with its tabs and a check's log; the status
bar shows `⇄ #12 ✗ 2/3` for the branch and opens the pull request, and a conversation comment posts; a thread shows
in the margin on the checked-out branch and a reply posts to its thread; a review collects two comments (a reply and a
new line comment, nothing sent) and submits them in one call; after a push the "Create a Pull Request" link appears,
the form is prefilled from the branch and creates #14, which opens; the Issues window lists, comments, creates, and
Create Branch checks out `issue/42-…` and links it; a merge asks, No sends nothing, Yes is refused while checks fail;
the device flow shows the code and completes when the fixture answers; a pasted token signs in and never reaches the
audit; an agent's write under `deny` fails naming the policy, and under `prompt` asks once and "Allow for this
session" lets the next write run (bodies not audited); a stale list shows the cache with its age, then updates and
does not refresh again; offline shows the cache and the reason; a Tangled repository shows only what its fixtures
support (no review bar, no Threads or Checks tab, no Merge, Create says why not); the budgets. Unit tests: the merge
and close questions name the method; the banner's text.

Elsewhere: `crates/docking` (the two window ids in the default layout, closed, docked right; the tool window count),
`crates/ui` (the View and Git menu items), `crates/eludite/src/shell/git_tests.rs` (the Git menu's labels).

## 7. The Xvfb run

`crates/eludite/tools/forge-linux.sh OUT_DIR` makes a repository on `feature/login`, starts the fixture server
(`cargo build -p eludite-forge --example fixture-server`) on the GitHub fixtures, points `forge.hosts` at it, keeps
credentials under `OUT_DIR/xdg`, and drives the real binary with xdotool against `--bounds-out`. It checks that no
request reached the server before a window opened, that the token never reached the workspace cache, and that the
consented credentials file is mode 0600 (the screen's session has no Secret Service, so the dialog offered the file).
Three consecutive runs passed. [Screenshots](0046-run/screenshots/) and [the server's log](0046-run/fixture-requests.txt):

- [pull-requests.png](0046-run/screenshots/pull-requests.png): the window, signed out, the status bar's `⇄ #12 ✗ 2/3`.
- [sign-in.png](0046-run/screenshots/sign-in.png): the dialog with a pasted token, the store unavailable, the
  consent ticked.
- [pull-request.png](0046-run/screenshots/pull-request.png): #12's Threads tab, the merge methods the repository
  allows, the review bar.
- [thread.png](0046-run/screenshots/thread.png): `src/login.rs` with the thread's glyph at line 14, opened.
- [create-pull-request.png](0046-run/screenshots/create-pull-request.png): the form prefilled from the branch.
- [issues.png](0046-run/screenshots/issues.png): the Issues window with #42 open.

## 8. New dependencies

| Crate | Version | SPDX | Why |
|---|---|---|---|
| keyring | 4.2.0 | MIT OR Apache-2.0 | the credential store (the brief's one dependency) |
| keyring-core | 1.0.0 | MIT OR Apache-2.0 | keyring's store interface |
| zbus-secret-service-keyring-store | 1.0.1 | MIT OR Apache-2.0 | Linux backend (zbus 5 was already in the build) |
| secret-service | 5.2.0 | MIT OR Apache-2.0 | Linux backend |
| aes | 0.9.3 | MIT OR Apache-2.0 | Secret Service session encryption |
| cbc | 0.2.1 | MIT OR Apache-2.0 | same |
| cipher | 0.5.2 | MIT OR Apache-2.0 | same |
| block-padding | 0.4.2 | MIT OR Apache-2.0 | same |
| inout | 0.2.2 | MIT OR Apache-2.0 | same |
| hkdf | 0.13.0 | MIT OR Apache-2.0 | same |
| hmac | 0.13.0 | MIT OR Apache-2.0 | same |
| cmov | 0.5.4 | Apache-2.0 OR MIT | same (constant time) |
| ctutils | 0.4.2 | Apache-2.0 OR MIT | same |
| cpubits | 0.1.1 | MIT OR Apache-2.0 | same |
| apple-native-keyring-store | 1.0.2 | MIT OR Apache-2.0 | macOS backend (macOS targets only) |
| security-framework | 3.7.0 | MIT OR Apache-2.0 | macOS backend |
| security-framework-sys | 2.17.0 | MIT OR Apache-2.0 | macOS backend |
| windows-native-keyring-store | 1.1.0 | MIT OR Apache-2.0 | Windows backend (Windows targets only) |

`ureq` 3.4 (`rustls`, `json` features) and `serde`, `serde_json` were already in the build and are direct
dependencies of `crates/forge`; `tempfile` is a dev-dependency already in the workspace.

## 9. Outside the brief's files, and notes for merging

- `crates/git/src/remote.rs` and `crates/git/tests/remote.rs` (commit `22dfa56`): `Repo::fetch_refspec`, a fetch of
  one refspec from a remote name or a url (a fork's), which `pull_checkout` needs; with its test.
- `crates/eludite/src/shell/git/changes.rs` (the "Create a Pull Request" link: a field, a setter, the element) and
  `crates/eludite/src/shell/git.rs` (hooks: the status, a push, a document opened or closed; `git_open_compare` made
  `pub(super)` for the Files tab).
- `crates/eludite/src/shell/settings.rs` (two calls: apply `forge.*`, the workspace changed),
  `crates/eludite/src/shell.rs` (the module, the `Services` and `Shell` fields, the tool and document bodies, the run
  hook, the dialog, the probe), `crates/eludite/Cargo.toml` (the dependency).
- `crates/docking/src/controller.rs`: a test's tool window count, 19 to 21.
- `crates/eludite/src/shell/git_tests.rs`: the Git menu's expected labels gain Create Pull Request and Sign in to
  Forge....
- `crates/commands/src/registry.rs`, `policy.rs`, `lib.rs` and `crates/mcp/src/server.rs` (commit `27f84d5`): the
  audit's per-command redaction (agents' arguments are redacted when audited, also on a denied call), the policy's
  `forge` object.
- The menu label is "Sign in to Forge..." (the menu's labels are static); the dialog's title and the windows' button
  name the forge ("Sign in to GitHub (github.test)").
- Shared files touched only where the brief allows: `docs/briefs/README.md` (this row), `Cargo.toml` and `Cargo.lock`
  (the member, the workspace dependency `eludite-forge`, keyring's tree), `crates/ui/src/menu.rs` (View > Other
  Windows and the Git menu), `crates/docking/src/model.rs` (two ids; no layout schema bump: a saved layout gains them
  closed through `normalize`), `protocol/schemas/settings.json` and `crates/commands/src/settings.rs` (the `forge.*`
  group, appended last), `protocol/schemas/agents-policy.json` (`forge`), `CLAUDE.md` (the crate row, `protocol/forge/`
  in the protocol row, `tools/forge-corpus/`), `docs/PLAN.md` (the one line in 4.8). `README.md` unchanged: no new
  system package (keyring's Linux backend is pure Rust over zbus).
- Alongside brief 0039: none of 0039's files were touched (`browsers/chromium`, `crates/browser`,
  `crates/eludite/src/shell/browser*.rs`, `tools/package`, `tools/cef`). Likely textual conflicts are only in the
  shared lists: `shell.rs` (module list, `Services`, `register_workspace`, `Shell` fields, `tool_body`,
  `document_body`, `run`, the render's dialog children), `menu.rs`, `Cargo.lock`, the briefs index and `CLAUDE.md`;
  each side adds lines, so a merge keeps both.
- The device flows need client ids: none is built in (`ELUDITE_GITHUB_CLIENT_ID`, `ELUDITE_GITLAB_APPLICATION_ID`,
  `ELUDITE_AZURE_APPLICATION_ID` at build time, or the settings). Until the owner registers them, sign-in is by token
  or CLI.

## 10. Not done, and why

- No real-service run of any write, and nothing of github.com: no tokens here and api.github.com unreachable. The
  GitHub fixtures are synthesized from the pinned description; the owner's `tools/forge-corpus/record.sh` and
  `live.rs` runs replace and check them.
- GitHub's merge queue and GitLab's merge train are not reached (no fixture; the brief lists the merge train's
  administration as out of scope); `when_checks_pass` uses auto-merge and merge when pipeline succeeds.
- Images in descriptions are not fetched (brief 0043's renderer shows their alt text); links open in the browser.
- Windows and macOS were not built here; the credential store's backends there are compiled only on those targets.

## 11. Verification

Run in this worktree on 2026-10-04 (Ubuntu container, 4 cores, debug build; another worktree's agent built beside it),
with the environment the coordinator gave (`DOTNET_ROOT`, `ELUDITE_DBG_MONO`, `corpus/tests/build.sh`, `ELUDITE_CHROME`
with `ELUDITE_CHROME_NO_SANDBOX`, `CEF_PATH`, Node 22 on PATH, `ELUDITE_JS_DEBUG`, `ELUDITE_TEST_SSHD`, `DISPLAY=:99`),
each gated on its real exit code:

| Check | Result |
|---|---|
| `cargo fmt --check` | exit 0 |
| `dotnet build dotnet/Eludite.slnx` | exit 0, 0 warnings, 0 errors |
| `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings` | exit 0 |
| `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` | exit 0: 1,124 passed, 0 failed, 1 ignored, in 87 test binaries (the `eludite` binary's 339 include the 14 forge shell tests; the five real-service tests and the recorder skip without their variables) |

No test needed the load-flaky rerun.
