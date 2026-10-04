# Brief 0053: The commit history CodeLens

Status: open
Phase: 2 (PLAN.md section 10: "CodeLens"; section 4.8: log and blame in the shell)
Plan reference: PLAN.md sections 2 (principles 1, 3, 5, 12), 4.1 (CodeLens in the editor core), 4.8 (git in the shell: log, blame; no `git` process on hot paths), 8 (Visual Studio names: the "N authors, M changes" indicator, the commit history popup with its author, date and message per commit, "View Diff"), 9, 10 (Phase 2)
Related ADRs: ADR-0001, ADR-0002
Depends on: brief 0052 (the lens rows, the request and resolve pipeline, the popup frame, the settings), brief 0040 (`crates/git`: blame, log, `diff_texts`, the Compare document, the Git Repository window's commit rows), brief 0045 (nothing network-side is needed; named so the credential path is not touched).

## Goal

Above each class, method and property that brief 0052 already decorates, a third indicator reads Visual Studio's "N authors, M changes" for that member's lines, computed from the repository's history without a `git` process: the member's current line range (from the same symbol positions the references lens uses) is followed back through the commits that touched it, as `git log -L` does, with libgit2's blame for the range as the first step and the diff between each parent and child for the range's movement. Clicking it opens the commit history popup: one row per commit with the author, the relative date, the summary and the first line of the message, newest first; Enter or "View Diff" opens brief 0040's Compare document for that commit and file with the member's lines in view; "View Commit" selects it in the Git Repository window; "Copy SHA". Uncommitted changes to the member show as a first row, "Uncommitted changes", that opens Compare with Unmodified. The indicator is off outside a repository and when `editor.codeLens.history` is off, counts are capped (the lens says "20+ changes" beyond `codeLens.historyLimit`), the computation runs on the git worker with a cancellation token and a cache keyed by (HEAD commit, path, range) invalidated by brief 0040's status watcher, and nothing runs until a lens row is visible. Agents get `eludite.git.history` (read): `path`, `line_start`, `line_end`, `max` (default 20, cap 200), answering the same rows the popup shows, so an agent can ask who last changed a method before editing it.

## Files in scope

- `protocol/schemas/` first and alone: `git-history.{input,output}.json` (path, the line range, `max`, `include_uncommitted`; output: `authors` (count), `changes` (count, `truncated`), rows with sha, author name and email, author date, summary, the range in that commit, `uncommitted` as the first row when present), `settings.json` (`editor.codeLens.history` default on where `editor.codeLens` is on; `editor.codeLens.historyLimit` default 20, cap 200; the per-language overrides of brief 0052 apply).
- `crates/git/src/history.rs` (new: `Repo::line_history(path, range, limit, cancel)`: the range's blame at HEAD for the first set of commits, then, per commit newest to oldest, the diff of the file between the commit and its first parent (`diff_texts` against a revision, or libgit2's blob diff with `DiffOptions` on the two blobs) to map the range into the parent and continue, stopping at the file's creation, a rename (followed through `find_similar` once per step) or the limit; `authors` as distinct author emails; the uncommitted row from the working tree's diff of the range; a cache in the `Repo` handle keyed by (HEAD id, path, range) and cleared on the watcher's generation bump), `crates/git/tests/history.rs` (temporary repositories: a method edited in three commits by two authors; a rename with the history followed; a range whose lines moved after an insertion above; a file created in the last commit (one change); the uncommitted row; the limit and `truncated`; cancellation mid-walk on a 2,000-commit history; the budget below).
- `crates/commands/src/git.rs` (the `history` command, `read`), `crates/mcp` (the tool), `crates/eludite/src/shell/git.rs` and `git/` (the history provider for the lens: a request per visible lens row's member range on the git worker, answered into the lens's resolve; the popup rows reusing the Git Repository window's commit row element; View Diff opening Compare for `sha^..sha` on the file at the member's lines (brief 0040's Compare document gains a revision pair if it only takes `index` and `head` today; say so); View Commit selecting the commit in the Git Repository window; Copy SHA), `crates/editor/src/intellisense.rs` or wherever brief 0052 put the lens pipeline (a third lens kind supplied by the shell, not by a language server: the lens row composes server lenses and the shell's history lens in Visual Studio's order: references, history, tests), `crates/eludite/src/shell/codelens_tests.rs` (the headless tests below), `crates/eludite/src/shell/options.rs` (the two settings), `docs/agents/git.md` (one sentence on `history`, under the 800-word cap), `crates/eludite/tools/codelens-linux.sh` (the run gains the history lens and its popup on this repository; screenshots), `docs/briefs/README.md`, `docs/briefs/0053-report.md` (new).

## Contract

- **Counts.** `authors` is the number of distinct author emails among the commits that touched the member's lines, `changes` the number of those commits, up to the limit with `truncated`; a member whose lines were never committed shows "Uncommitted" alone; outside a repository the indicator is absent and the other lenses are unchanged.
- **The walk.** Follows the range through line movement and through one rename per step as `git log -L` does; stops at the file's first commit; never spawns `git`; runs on the git worker with brief 0040's cancellation token; the cache answers repeat requests for the same HEAD without a walk; the status watcher's generation bump clears it.
- **The popup.** Rows newest first with the author, relative date, summary; Enter or View Diff opens the Compare document for the commit's change to the file, scrolled to the member; View Commit selects it in the Git Repository window; Copy SHA; Escape closes; the Uncommitted row opens Compare with Unmodified.
- **Agents.** `history` answers the rows and counts for a range; it is `read`; the audit entry carries the path and range.
- The UI thread never waits; a lens row shows "..." while the walk runs and the count when it ends; stale answers (older HEAD or document version) are dropped.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/git`: the scenarios under Files in scope with exact counts and rows; the rename; moved lines; cancellation; the cache hit after a repeat and the miss after a commit.
- `crates/eludite` headless tests (a temporary repository with history, fake host for the symbol positions): the history lens shows the counts beside the references lens in Visual Studio's order, the popup lists the commits and View Diff opens Compare at the member, View Commit selects it in the Git Repository window, the Uncommitted row, the setting off hides only this lens, `historyLimit` caps with "20+", outside a repository nothing shows, an agent's `history` matches the popup, a commit in the repository refreshes the counts through the watcher, the keystroke budget with 200 lens rows still holds (`assert_budget`).
- The Xvfb run's screenshots in the report on this repository's own history.

## Budget

- A member's history on this repository (about 500 commits) within 150 ms warm from the cache's miss path, under 1 ms from a hit; the walk for 50 visible members within 1 s total on the git worker, with the lens rows filling as answers arrive.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green; `dotnet build` and `dotnet test` unchanged.
2. The report records the budget numbers, the walk's rules as shipped (renames, moved lines, merge commits), and how the counts compare with `git log -L` on three members of this repository.
3. The briefs index, the MCP tool list and `docs/agents/git.md` match the repository.

## Out of scope

- Blame annotations in the margin (the Blame document exists); history for a selection rather than a member (a later command); forge links from the popup to the pull request that merged a commit (brief 0046's data could supply it; a later brief); Windows and macOS runs beyond CI.
