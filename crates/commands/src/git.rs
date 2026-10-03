//! The git commands (brief 0040): `eludite.git.*`, the Git Changes and Git Repository windows' actions for the user
//! and agents alike.
//!
//! The schemas are `protocol/schemas/git-*.json` (checked in first, CLAUDE.md invariant 4). This module parses input
//! into a typed [`GitRequest`], serializes the typed outputs ([`GitOutput`]), declares each command's permission class
//! and registers the escalation hooks that apply the solution policy's `git` object
//! ([`crate::policy::GitPolicy`]). The shell implements [`GitCommands`] over libgit2 (`eludite-git`), on the invoking
//! thread: the UI invokes these commands off its own thread, agents from theirs.
//!
//! Classes: the reads (`status`, `diff`, `log`, `blame`, `branches`, the `stash` and `worktrees` lists) are read;
//! staging, commit, checkout, merge, rebase, cherry-pick, reset, fetch, pull, push, init, a stash push, apply or pop,
//! adding or removing a worktree and deleting a branch are execute; `discard`, `reset` with `hard` and dropping a
//! stash are dangerous. For agents the policy's `git.commit` (commits), `git.push` (pushes, `prompt` by default) and
//! `git.history` (amend, reset, rebase, a pull that rebases, aborting a merge; `prompt` by default) raise a call to
//! dangerous or refuse it, and any `force` is refused outright.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::{GitCall, PolicyView};
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, Escalation, EscalationHook,
    PermissionClass,
};

pub const STATUS: &str = "eludite.git.status";
pub const INIT: &str = "eludite.git.init";
pub const STAGE: &str = "eludite.git.stage";
pub const UNSTAGE: &str = "eludite.git.unstage";
pub const DISCARD: &str = "eludite.git.discard";
pub const COMMIT: &str = "eludite.git.commit";
pub const DIFF: &str = "eludite.git.diff";
pub const LOG: &str = "eludite.git.log";
pub const BRANCHES: &str = "eludite.git.branches";
pub const CHECKOUT: &str = "eludite.git.checkout";
pub const MERGE: &str = "eludite.git.merge";
pub const REBASE: &str = "eludite.git.rebase";
pub const CHERRY_PICK: &str = "eludite.git.cherry_pick";
pub const RESET: &str = "eludite.git.reset";
pub const STASH: &str = "eludite.git.stash";
pub const FETCH: &str = "eludite.git.fetch";
pub const PULL: &str = "eludite.git.pull";
pub const PUSH: &str = "eludite.git.push";
pub const CANCEL: &str = "eludite.git.cancel";
pub const WORKTREES: &str = "eludite.git.worktrees";
pub const BLAME: &str = "eludite.git.blame";

pub const ALL: [&str; 21] = [
    STATUS,
    INIT,
    STAGE,
    UNSTAGE,
    DISCARD,
    COMMIT,
    DIFF,
    LOG,
    BRANCHES,
    CHECKOUT,
    MERGE,
    REBASE,
    CHERRY_PICK,
    RESET,
    STASH,
    FETCH,
    PULL,
    PUSH,
    CANCEL,
    WORKTREES,
    BLAME,
];

/// Files a status group lists by default.
pub const DEFAULT_STATUS_ITEMS: usize = 1_000;
/// Changed lines a diff lists at most (brief 0040), and its default context.
pub const MAX_DIFF_LINES: usize = 2_000;
pub const DEFAULT_CONTEXT: usize = 3;
/// Commits a log lists by default and at most.
pub const DEFAULT_LOG: usize = 100;
pub const MAX_LOG: usize = 5_000;
/// Lines a blame lists by default and at most.
pub const DEFAULT_BLAME_LINES: usize = 10_000;
pub const MAX_BLAME_LINES: usize = 100_000;

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    macro_rules! s {
        ($name:literal) => {
            (
                include_str!(concat!(
                    "../../../protocol/schemas/git-",
                    $name,
                    ".input.json"
                )),
                include_str!(concat!(
                    "../../../protocol/schemas/git-",
                    $name,
                    ".output.json"
                )),
            )
        };
    }
    let (title, (input, output), class) = match id {
        STATUS => ("Git: Status", s!("status"), Read),
        INIT => ("Git: Create Git Repository", s!("init"), Execute),
        STAGE => ("Git: Stage", s!("stage"), Execute),
        UNSTAGE => ("Git: Unstage", s!("unstage"), Execute),
        DISCARD => ("Git: Undo Changes", s!("discard"), Dangerous),
        COMMIT => ("Git: Commit", s!("commit"), Execute),
        DIFF => ("Git: Compare with Unmodified", s!("diff"), Read),
        LOG => ("Git: View History", s!("log"), Read),
        BRANCHES => ("Git: Manage Branches", s!("branches"), Read),
        CHECKOUT => ("Git: Checkout", s!("checkout"), Execute),
        MERGE => ("Git: Merge", s!("merge"), Execute),
        REBASE => ("Git: Rebase", s!("rebase"), Execute),
        CHERRY_PICK => ("Git: Cherry-Pick", s!("cherry-pick"), Execute),
        RESET => ("Git: Reset", s!("reset"), Execute),
        STASH => ("Git: Stash", s!("stash"), Read),
        FETCH => ("Git: Fetch", s!("fetch"), Execute),
        PULL => ("Git: Pull", s!("pull"), Execute),
        PUSH => ("Git: Push", s!("push"), Execute),
        CANCEL => ("Git: Cancel", s!("cancel"), Execute),
        WORKTREES => ("Git: Worktrees", s!("worktrees"), Read),
        BLAME => ("Git: Blame", s!("blame"), Read),
        other => unreachable!("not a git command: {other}"),
    };
    (title, input, output, class)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebaseAction {
    Start,
    Continue,
    Abort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StashAction {
    List,
    Push,
    Apply,
    Pop,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeAction {
    List,
    Add,
    Remove,
}

/// A commit's author when it is not the committer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    pub name: String,
    pub email: String,
}

/// A parsed, validated git command.
#[derive(Debug, Clone, PartialEq)]
pub enum GitRequest {
    Status {
        max_items: usize,
        include_ignored: bool,
    },
    Init {
        path: Option<String>,
    },
    /// `paths`, or every change when `None`.
    Stage {
        paths: Option<Vec<String>>,
    },
    Unstage {
        paths: Option<Vec<String>>,
    },
    Discard {
        paths: Option<Vec<String>>,
        staged: bool,
    },
    Commit {
        message: String,
        all: bool,
        amend: bool,
        author: Option<Author>,
    },
    Diff {
        path: String,
        against: String,
        staged: bool,
        context: usize,
        max_lines: usize,
    },
    Log {
        revision: Option<String>,
        all: bool,
        path: Option<String>,
        max: usize,
        skip: usize,
    },
    Branches {
        delete: Option<String>,
        force: bool,
        remote: bool,
    },
    Checkout {
        name: String,
        create: bool,
        start_point: Option<String>,
        force: bool,
    },
    Merge {
        branch: Option<String>,
        no_ff: bool,
        message: Option<String>,
        abort: bool,
    },
    Rebase {
        onto: Option<String>,
        action: RebaseAction,
    },
    CherryPick {
        commit: String,
    },
    Reset {
        revision: String,
        mode: ResetMode,
    },
    Stash {
        action: StashAction,
        message: Option<String>,
        include_untracked: bool,
        index: usize,
    },
    Fetch {
        remote: Option<String>,
        prune: bool,
    },
    Pull {
        remote: Option<String>,
        rebase: bool,
    },
    Push {
        remote: Option<String>,
        branch: Option<String>,
        set_upstream: bool,
        force: bool,
    },
    Cancel,
    Worktrees {
        action: WorktreeAction,
        name: Option<String>,
        path: Option<String>,
        branch: Option<String>,
        force: bool,
    },
    Blame {
        path: String,
        max_lines: usize,
    },
}

impl GitRequest {
    pub fn command(&self) -> &'static str {
        match self {
            GitRequest::Status { .. } => STATUS,
            GitRequest::Init { .. } => INIT,
            GitRequest::Stage { .. } => STAGE,
            GitRequest::Unstage { .. } => UNSTAGE,
            GitRequest::Discard { .. } => DISCARD,
            GitRequest::Commit { .. } => COMMIT,
            GitRequest::Diff { .. } => DIFF,
            GitRequest::Log { .. } => LOG,
            GitRequest::Branches { .. } => BRANCHES,
            GitRequest::Checkout { .. } => CHECKOUT,
            GitRequest::Merge { .. } => MERGE,
            GitRequest::Rebase { .. } => REBASE,
            GitRequest::CherryPick { .. } => CHERRY_PICK,
            GitRequest::Reset { .. } => RESET,
            GitRequest::Stash { .. } => STASH,
            GitRequest::Fetch { .. } => FETCH,
            GitRequest::Pull { .. } => PULL,
            GitRequest::Push { .. } => PUSH,
            GitRequest::Cancel => CANCEL,
            GitRequest::Worktrees { .. } => WORKTREES,
            GitRequest::Blame { .. } => BLAME,
        }
    }

    /// Whether the call may change the repository (everything but the reads).
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            GitRequest::Status { .. }
                | GitRequest::Diff { .. }
                | GitRequest::Log { .. }
                | GitRequest::Blame { .. }
                | GitRequest::Cancel
                | GitRequest::Branches { delete: None, .. }
                | GitRequest::Stash {
                    action: StashAction::List,
                    ..
                }
                | GitRequest::Worktrees {
                    action: WorktreeAction::List,
                    ..
                }
        )
    }
}

// ----- Outputs -----

/// A changed file of `git-status.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeOut {
    pub path: String,
    /// `added`, `modified`, `deleted`, `renamed`, `typechange`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusTotals {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

/// `git-status.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusOutput {
    /// `none`, `loading` or `ready`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub detached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default)]
    pub staged: Vec<ChangeOut>,
    #[serde(default)]
    pub unstaged: Vec<ChangeOut>,
    #[serde(default)]
    pub untracked: Vec<String>,
    #[serde(default)]
    pub conflicted: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored: Option<Vec<String>>,
    #[serde(default)]
    pub stashes: u32,
    #[serde(default)]
    pub totals: StatusTotals,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitOutput {
    pub repository: String,
    pub branch: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageOutput {
    pub staged: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved: Vec<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnstageOutput {
    pub unstaged: Vec<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardOutput {
    pub restored: Vec<String>,
    pub deleted: Vec<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitOutput {
    pub commit: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub amended: bool,
    pub files: u32,
    pub generation: u64,
}

/// A line of a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLineOut {
    /// `same`, `added` or `removed`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunkOut {
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    pub lines: Vec<DiffLineOut>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffOutput {
    pub path: String,
    pub old_label: String,
    pub new_label: String,
    pub binary: bool,
    pub old_lines: u32,
    pub new_lines: u32,
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<DiffHunkOut>,
    pub omitted: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphOut {
    pub lane: u32,
    pub edges: Vec<[u32; 2]>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overflow: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntryOut {
    pub commit: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub summary: String,
    pub refs: Vec<String>,
    pub graph: GraphOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogOutput {
    pub entries: Vec<LogEntryOut>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchOut {
    pub name: String,
    pub remote: bool,
    pub head: bool,
    pub commit: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagOut {
    pub name: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchesOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    pub branches: Vec<BranchOut>,
    pub tags: Vec<TagOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub detached: bool,
    pub commit: String,
    pub created: bool,
    pub generation: u64,
}

/// `git-merge`, `git-pull` and `git-cherry-pick` outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeOutput {
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub conflicts: Vec<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebaseOutput {
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    pub conflicts: Vec<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetOutput {
    pub commit: String,
    pub mode: ResetMode,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashOut {
    pub index: u32,
    pub message: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashOutput {
    pub result: String,
    pub stashes: Vec<StashOut>,
    pub conflicts: Vec<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchOutput {
    pub remote: String,
    pub updated: Vec<String>,
    pub received_objects: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushOutput {
    pub remote: String,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    pub commit: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOutput {
    pub canceled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeOut {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub main: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreesOutput {
    pub worktrees: Vec<WorktreeOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameLineOut {
    pub line: u32,
    pub commit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameCommitOut {
    pub commit: String,
    pub short: String,
    pub author: String,
    pub time: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameOutput {
    pub path: String,
    pub lines: Vec<BlameLineOut>,
    pub commits: Vec<BlameCommitOut>,
    pub truncated: bool,
}

/// A git command's typed result.
#[derive(Debug, Clone, PartialEq)]
pub enum GitOutput {
    Status(Box<StatusOutput>),
    Init(InitOutput),
    Stage(StageOutput),
    Unstage(UnstageOutput),
    Discard(DiscardOutput),
    Commit(CommitOutput),
    Diff(Box<DiffOutput>),
    Log(Box<LogOutput>),
    Branches(Box<BranchesOutput>),
    Checkout(CheckoutOutput),
    Merge(MergeOutput),
    Rebase(RebaseOutput),
    Reset(ResetOutput),
    Stash(StashOutput),
    Fetch(FetchOutput),
    Push(PushOutput),
    Cancel(CancelOutput),
    Worktrees(WorktreesOutput),
    Blame(Box<BlameOutput>),
}

impl GitOutput {
    pub fn to_json(&self) -> Value {
        match self {
            GitOutput::Status(o) => serde_json::to_value(o),
            GitOutput::Init(o) => serde_json::to_value(o),
            GitOutput::Stage(o) => serde_json::to_value(o),
            GitOutput::Unstage(o) => serde_json::to_value(o),
            GitOutput::Discard(o) => serde_json::to_value(o),
            GitOutput::Commit(o) => serde_json::to_value(o),
            GitOutput::Diff(o) => serde_json::to_value(o),
            GitOutput::Log(o) => serde_json::to_value(o),
            GitOutput::Branches(o) => serde_json::to_value(o),
            GitOutput::Checkout(o) => serde_json::to_value(o),
            GitOutput::Merge(o) => serde_json::to_value(o),
            GitOutput::Rebase(o) => serde_json::to_value(o),
            GitOutput::Reset(o) => serde_json::to_value(o),
            GitOutput::Stash(o) => serde_json::to_value(o),
            GitOutput::Fetch(o) => serde_json::to_value(o),
            GitOutput::Push(o) => serde_json::to_value(o),
            GitOutput::Cancel(o) => serde_json::to_value(o),
            GitOutput::Worktrees(o) => serde_json::to_value(o),
            GitOutput::Blame(o) => serde_json::to_value(o),
        }
        .expect("git outputs serialize")
    }

    /// The status generation the answer carries, if any.
    pub fn generation(&self) -> Option<u64> {
        Some(match self {
            GitOutput::Status(o) => o.generation,
            GitOutput::Init(o) => o.generation,
            GitOutput::Stage(o) => o.generation,
            GitOutput::Unstage(o) => o.generation,
            GitOutput::Discard(o) => o.generation,
            GitOutput::Commit(o) => o.generation,
            GitOutput::Branches(o) => o.generation,
            GitOutput::Checkout(o) => o.generation,
            GitOutput::Merge(o) => o.generation,
            GitOutput::Rebase(o) => o.generation,
            GitOutput::Reset(o) => o.generation,
            GitOutput::Stash(o) => o.generation,
            GitOutput::Fetch(o) => o.generation,
            GitOutput::Push(o) => o.generation,
            _ => return None,
        })
    }
}

/// Whatever owns the repository (the shell). Called on the invoking thread, never the UI thread's.
pub trait GitCommands: Send + Sync {
    fn apply(&self, request: GitRequest) -> Result<GitOutput, CommandError>;
}

// ----- Input -----

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StatusIn {
    max_items: Option<u64>,
    include_ignored: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct InitIn {
    path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PathsIn {
    paths: Option<Vec<String>>,
    all: Option<bool>,
    staged: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CommitIn {
    message: Option<String>,
    all: Option<bool>,
    amend: Option<bool>,
    author: Option<Author>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DiffIn {
    path: Option<String>,
    against: Option<String>,
    staged: Option<bool>,
    context: Option<u64>,
    max_lines: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct LogIn {
    revision: Option<String>,
    all: Option<bool>,
    path: Option<String>,
    max: Option<u64>,
    skip: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BranchesIn {
    action: Option<String>,
    name: Option<String>,
    force: Option<bool>,
    remote: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CheckoutIn {
    name: Option<String>,
    create: Option<bool>,
    start_point: Option<String>,
    force: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct MergeIn {
    branch: Option<String>,
    no_ff: Option<bool>,
    message: Option<String>,
    abort: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RebaseIn {
    onto: Option<String>,
    action: Option<RebaseAction>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CherryPickIn {
    commit: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResetIn {
    revision: Option<String>,
    mode: Option<ResetMode>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StashIn {
    action: Option<StashAction>,
    message: Option<String>,
    include_untracked: Option<bool>,
    index: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FetchIn {
    remote: Option<String>,
    prune: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PullIn {
    remote: Option<String>,
    rebase: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PushIn {
    remote: Option<String>,
    branch: Option<String>,
    set_upstream: Option<bool>,
    force: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CancelIn {}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WorktreesIn {
    action: Option<WorktreeAction>,
    name: Option<String>,
    path: Option<String>,
    branch: Option<String>,
    force: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BlameIn {
    path: Option<String>,
    max_lines: Option<u64>,
}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

fn non_empty(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(s) if s.trim().is_empty() => Err(invalid(format!("`{name}` must not be empty"))),
        other => Ok(other),
    }
}

fn required(name: &str, v: Option<String>) -> Result<String, CommandError> {
    non_empty(name, v)?.ok_or_else(|| invalid(format!("`{name}` is required")))
}

fn bounded(
    name: &str,
    v: Option<u64>,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, CommandError> {
    match v {
        None => Ok(default),
        Some(n) if (min as u64..=max as u64).contains(&n) => Ok(n as usize),
        Some(_) => Err(invalid(format!("`{name}` is {min} to {max}"))),
    }
}

/// `paths` or `all`, exactly one.
fn paths_or_all(i: &PathsIn) -> Result<Option<Vec<String>>, CommandError> {
    match (&i.paths, i.all.unwrap_or(false)) {
        (Some(_), true) => Err(invalid("give `paths` or `all`, not both")),
        (None, false) => Err(invalid("give `paths`, or `all: true`")),
        (None, true) => Ok(None),
        (Some(p), false) => {
            if p.is_empty() || p.iter().any(|x| x.trim().is_empty()) {
                return Err(invalid("`paths` lists at least one non-empty path"));
            }
            Ok(Some(p.clone()))
        }
    }
}

/// Parse and validate the input of git command `id`.
pub fn parse(id: &str, value: Value) -> Result<GitRequest, CommandError> {
    Ok(match id {
        STATUS => {
            let i: StatusIn = input(value)?;
            GitRequest::Status {
                max_items: bounded("max_items", i.max_items, DEFAULT_STATUS_ITEMS, 1, 100_000)?,
                include_ignored: i.include_ignored.unwrap_or(false),
            }
        }
        INIT => {
            let i: InitIn = input(value)?;
            GitRequest::Init {
                path: non_empty("path", i.path)?,
            }
        }
        STAGE | UNSTAGE => {
            let i: PathsIn = input(value)?;
            if i.staged.is_some() {
                return Err(invalid("`staged` is for eludite.git.discard"));
            }
            let paths = paths_or_all(&i)?;
            if id == STAGE {
                GitRequest::Stage { paths }
            } else {
                GitRequest::Unstage { paths }
            }
        }
        DISCARD => {
            let i: PathsIn = input(value)?;
            GitRequest::Discard {
                paths: paths_or_all(&i)?,
                staged: i.staged.unwrap_or(false),
            }
        }
        COMMIT => {
            let i: CommitIn = input(value)?;
            let message = required("message", i.message)?;
            if let Some(a) = &i.author
                && (a.name.trim().is_empty() || a.email.trim().is_empty())
            {
                return Err(invalid("`author` needs a name and an email"));
            }
            GitRequest::Commit {
                message,
                all: i.all.unwrap_or(false),
                amend: i.amend.unwrap_or(false),
                author: i.author,
            }
        }
        DIFF => {
            let i: DiffIn = input(value)?;
            GitRequest::Diff {
                path: required("path", i.path)?,
                against: non_empty("against", i.against)?.unwrap_or_else(|| "index".into()),
                staged: i.staged.unwrap_or(false),
                context: bounded("context", i.context, DEFAULT_CONTEXT, 0, 100)?,
                max_lines: bounded("max_lines", i.max_lines, MAX_DIFF_LINES, 1, MAX_DIFF_LINES)?,
            }
        }
        LOG => {
            let i: LogIn = input(value)?;
            let all = i.all.unwrap_or(false);
            if all && i.revision.is_some() {
                return Err(invalid("give `revision` or `all`, not both"));
            }
            GitRequest::Log {
                revision: non_empty("revision", i.revision)?,
                all,
                path: non_empty("path", i.path)?,
                max: bounded("max", i.max, DEFAULT_LOG, 1, MAX_LOG)?,
                skip: bounded("skip", i.skip, 0, 0, usize::MAX >> 1)?,
            }
        }
        BRANCHES => {
            let i: BranchesIn = input(value)?;
            let delete = match i.action.as_deref() {
                None | Some("list") => {
                    if i.name.is_some() || i.force.is_some() {
                        return Err(invalid("`name` and `force` are for `action: delete`"));
                    }
                    None
                }
                Some("delete") => Some(required("name", i.name)?),
                Some(other) => return Err(invalid(format!("unknown action `{other}`"))),
            };
            GitRequest::Branches {
                delete,
                force: i.force.unwrap_or(false),
                remote: i.remote.unwrap_or(true),
            }
        }
        CHECKOUT => {
            let i: CheckoutIn = input(value)?;
            let create = i.create.unwrap_or(false);
            if i.start_point.is_some() && !create {
                return Err(invalid("`start_point` is for `create`"));
            }
            GitRequest::Checkout {
                name: required("name", i.name)?,
                create,
                start_point: non_empty("start_point", i.start_point)?,
                force: i.force.unwrap_or(false),
            }
        }
        MERGE => {
            let i: MergeIn = input(value)?;
            let abort = i.abort.unwrap_or(false);
            let branch = non_empty("branch", i.branch)?;
            if abort && (branch.is_some() || i.message.is_some()) {
                return Err(invalid("`abort` takes no branch"));
            }
            if !abort && branch.is_none() {
                return Err(invalid("`branch` is required"));
            }
            GitRequest::Merge {
                branch,
                no_ff: i.no_ff.unwrap_or(false),
                message: non_empty("message", i.message)?,
                abort,
            }
        }
        REBASE => {
            let i: RebaseIn = input(value)?;
            let action = i.action.unwrap_or(RebaseAction::Start);
            let onto = non_empty("onto", i.onto)?;
            match (action, &onto) {
                (RebaseAction::Start, None) => return Err(invalid("`onto` is required")),
                (RebaseAction::Continue | RebaseAction::Abort, Some(_)) => {
                    return Err(invalid("`onto` is for `start`"));
                }
                _ => {}
            }
            GitRequest::Rebase { onto, action }
        }
        CHERRY_PICK => {
            let i: CherryPickIn = input(value)?;
            GitRequest::CherryPick {
                commit: required("commit", i.commit)?,
            }
        }
        RESET => {
            let i: ResetIn = input(value)?;
            GitRequest::Reset {
                revision: non_empty("revision", i.revision)?.unwrap_or_else(|| "HEAD".into()),
                mode: i.mode.unwrap_or(ResetMode::Mixed),
            }
        }
        STASH => {
            let i: StashIn = input(value)?;
            let action = i.action.unwrap_or(StashAction::List);
            if action != StashAction::Push && (i.message.is_some() || i.include_untracked.is_some())
            {
                return Err(invalid("`message` and `include_untracked` are for `push`"));
            }
            if matches!(action, StashAction::List | StashAction::Push) && i.index.is_some() {
                return Err(invalid("`index` is for `apply`, `pop` and `drop`"));
            }
            GitRequest::Stash {
                action,
                message: non_empty("message", i.message)?,
                include_untracked: i.include_untracked.unwrap_or(false),
                index: bounded("index", i.index, 0, 0, 100_000)?,
            }
        }
        FETCH => {
            let i: FetchIn = input(value)?;
            GitRequest::Fetch {
                remote: non_empty("remote", i.remote)?,
                prune: i.prune.unwrap_or(false),
            }
        }
        PULL => {
            let i: PullIn = input(value)?;
            GitRequest::Pull {
                remote: non_empty("remote", i.remote)?,
                rebase: i.rebase.unwrap_or(false),
            }
        }
        PUSH => {
            let i: PushIn = input(value)?;
            GitRequest::Push {
                remote: non_empty("remote", i.remote)?,
                branch: non_empty("branch", i.branch)?,
                set_upstream: i.set_upstream.unwrap_or(false),
                force: i.force.unwrap_or(false),
            }
        }
        CANCEL => {
            let _: CancelIn = input(value)?;
            GitRequest::Cancel
        }
        WORKTREES => {
            let i: WorktreesIn = input(value)?;
            let action = i.action.unwrap_or(WorktreeAction::List);
            let name = non_empty("name", i.name)?;
            match action {
                WorktreeAction::List
                    if name.is_some() || i.path.is_some() || i.branch.is_some() =>
                {
                    return Err(invalid(
                        "`name`, `path` and `branch` are for `add` and `remove`",
                    ));
                }
                WorktreeAction::Add | WorktreeAction::Remove if name.is_none() => {
                    return Err(invalid("`name` is required"));
                }
                WorktreeAction::Remove if i.path.is_some() || i.branch.is_some() => {
                    return Err(invalid("`path` and `branch` are for `add`"));
                }
                _ => {}
            }
            if let Some(n) = &name
                && !n
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err(invalid("`name` takes letters, digits, `.`, `_` and `-`"));
            }
            GitRequest::Worktrees {
                action,
                name,
                path: non_empty("path", i.path)?,
                branch: non_empty("branch", i.branch)?,
                force: i.force.unwrap_or(false),
            }
        }
        BLAME => {
            let i: BlameIn = input(value)?;
            GitRequest::Blame {
                path: required("path", i.path)?,
                max_lines: bounded(
                    "max_lines",
                    i.max_lines,
                    DEFAULT_BLAME_LINES,
                    1,
                    MAX_BLAME_LINES,
                )?,
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

/// The public description of command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

fn flag(input: &Value, name: &str) -> bool {
    input.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn action(input: &Value) -> Option<&str> {
    input.get("action").and_then(Value::as_str)
}

/// What a call does for the policy, and the class its input alone raises it to (a stash drop is dangerous whatever
/// the policy says).
pub fn classify_call(
    id: &str,
    input: &Value,
) -> (GitCall, Option<(PermissionClass, &'static str)>) {
    let mut call = GitCall {
        force: flag(input, "force"),
        ..GitCall::default()
    };
    let mut raise = None;
    match id {
        COMMIT => {
            if flag(input, "amend") {
                call.history = true;
            } else {
                call.commit = true;
            }
        }
        PUSH => call.push = true,
        RESET => {
            call.history = true;
            if input.get("mode").and_then(Value::as_str) == Some("hard") {
                raise = Some((
                    PermissionClass::Dangerous,
                    "a hard reset discards the working tree's changes",
                ));
            }
        }
        REBASE => call.history = true,
        PULL => call.history = flag(input, "rebase"),
        MERGE => call.history = flag(input, "abort"),
        STASH => {
            raise = match action(input) {
                Some("push" | "apply" | "pop") => {
                    Some((PermissionClass::Execute, "it changes the working tree"))
                }
                Some("drop") => Some((PermissionClass::Dangerous, "a dropped stash is lost")),
                _ => None,
            }
        }
        BRANCHES => {
            if action(input) == Some("delete") {
                raise = Some((PermissionClass::Execute, "it deletes a branch"));
            }
        }
        WORKTREES => {
            if matches!(action(input), Some("add" | "remove")) {
                raise = Some((PermissionClass::Execute, "it changes the worktrees"));
            }
        }
        _ => {}
    }
    (call, raise)
}

/// The escalation hook of git command `id`, if its calls can escalate: the policy's `git` object (tool rules first,
/// `force` refused), and the class a call's input raises it to.
pub fn escalation(id: &'static str) -> Option<EscalationHook> {
    if matches!(
        id,
        STATUS
            | DIFF
            | LOG
            | BLAME
            | CANCEL
            | INIT
            | STAGE
            | UNSTAGE
            | DISCARD
            | CHERRY_PICK
            | FETCH
    ) {
        return None;
    }
    let tool = id.replace('.', "-");
    Some(Arc::new(move |input: &Value, view: &PolicyView| {
        let (call, raise) = classify_call(id, input);
        let policy = view
            .git()
            .decide_for(call, &view.policy().rules, &tool, input);
        match (policy, raise) {
            (Some(Escalation::Refuse(why)), _) => Some(Escalation::Refuse(why)),
            (None, None) => None,
            (None, Some((class, why))) => Some(Escalation::raise(class, why)),
            (Some(Escalation::Raise { class, reason, .. }), raise) => {
                let (class, reason) = match raise {
                    Some((c, why)) if c > class => (c, format!("{why}; {reason}")),
                    Some((_, why)) => (class, format!("{reason}; {why}")),
                    None => (class, reason),
                };
                Some(Escalation::raise(class, reason))
            }
        }
    }))
}

/// Register every git command, applying them to `target`, with their escalation hooks.
pub fn register(registry: &CommandRegistry, target: Arc<dyn GitCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{
        AgentPolicy, CommitPolicy, GitPolicy, GuardPolicy, PolicyRule, PolicySnapshot, RuleDecision,
    };
    use crate::{Caller, with_caller};
    use serde_json::json;

    /// `value` against `schema`: required members, no undeclared member where `additionalProperties` is false,
    /// enums, integer bounds and types, through arrays.
    fn conforms(schema: &str, value: &Value) {
        let root: Value = serde_json::from_str(schema).unwrap();
        check(&root, value, "$");
    }

    fn check(schema: &Value, value: &Value, at: &str) {
        if let Some(e) = schema["enum"].as_array() {
            assert!(e.contains(value), "{at}: {value} not in {e:?}");
        }
        if let Some(p) = schema["pattern"].as_str() {
            let re = regex::Regex::new(p).unwrap();
            assert!(re.is_match(value.as_str().unwrap()), "{at}: {value} !~ {p}");
        }
        match schema["type"].as_str() {
            Some("object") => {
                let obj = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{at}: not an object"));
                for r in schema["required"].as_array().into_iter().flatten() {
                    assert!(obj.contains_key(r.as_str().unwrap()), "{at}: missing {r}");
                }
                for (k, v) in obj {
                    let p = &schema["properties"][k];
                    assert!(!p.is_null(), "{at}: unexpected {k}");
                    check(p, v, &format!("{at}.{k}"));
                }
            }
            Some("array") => {
                let a = value
                    .as_array()
                    .unwrap_or_else(|| panic!("{at}: not an array"));
                for (i, v) in a.iter().enumerate() {
                    check(&schema["items"], v, &format!("{at}[{i}]"));
                }
            }
            Some("integer") => {
                let n = value
                    .as_i64()
                    .unwrap_or_else(|| panic!("{at}: not an integer"));
                if let Some(min) = schema["minimum"].as_i64() {
                    assert!(n >= min, "{at}: {n} < {min}");
                }
            }
            Some("string") => assert!(value.is_string(), "{at}: not a string"),
            Some("boolean") => assert!(value.is_boolean(), "{at}: not a boolean"),
            _ => {}
        }
    }

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn every_schema_parses_and_names_its_command() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
            assert_eq!(s.escalates().is_some(), escalation(id).is_some(), "{id}");
        }
        let classes: Vec<(&str, PermissionClass)> =
            ALL.iter().map(|id| (*id, spec(id).permission)).collect();
        use PermissionClass::*;
        for (id, c) in classes {
            let want = match id {
                STATUS | DIFF | LOG | BLAME | BRANCHES | STASH | WORKTREES => Read,
                DISCARD => Dangerous,
                _ => Execute,
            };
            assert_eq!(c, want, "{id}");
        }
    }

    #[test]
    fn parses_and_validates_input() {
        assert_eq!(
            parse(STATUS, Value::Null).unwrap(),
            GitRequest::Status {
                max_items: DEFAULT_STATUS_ITEMS,
                include_ignored: false
            }
        );
        assert_eq!(
            parse(STAGE, json!({"paths": ["a.cs"]})).unwrap(),
            GitRequest::Stage {
                paths: Some(vec!["a.cs".into()])
            }
        );
        assert_eq!(
            parse(UNSTAGE, json!({"all": true})).unwrap(),
            GitRequest::Unstage { paths: None }
        );
        assert!(parse(STAGE, json!({})).is_err());
        assert!(parse(STAGE, json!({"paths": ["a"], "all": true})).is_err());
        assert!(parse(STAGE, json!({"paths": []})).is_err());
        assert!(parse(STAGE, json!({"all": true, "staged": true})).is_err());
        assert_eq!(
            parse(DISCARD, json!({"all": true, "staged": true})).unwrap(),
            GitRequest::Discard {
                paths: None,
                staged: true
            }
        );
        assert!(parse(COMMIT, json!({})).is_err());
        assert!(parse(COMMIT, json!({"message": " "})).is_err());
        assert!(
            parse(
                COMMIT,
                json!({"message": "m", "author": {"name": "", "email": "e"}})
            )
            .is_err()
        );
        assert_eq!(
            parse(COMMIT, json!({"message": "m", "all": true})).unwrap(),
            GitRequest::Commit {
                message: "m".into(),
                all: true,
                amend: false,
                author: None
            }
        );
        assert_eq!(
            parse(DIFF, json!({"path": "a.cs"})).unwrap(),
            GitRequest::Diff {
                path: "a.cs".into(),
                against: "index".into(),
                staged: false,
                context: 3,
                max_lines: MAX_DIFF_LINES
            }
        );
        assert!(parse(DIFF, json!({"path": "a", "max_lines": 2001})).is_err());
        assert!(parse(LOG, json!({"all": true, "revision": "HEAD"})).is_err());
        assert!(parse(LOG, json!({"max": 5001})).is_err());
        assert_eq!(
            parse(
                BRANCHES,
                json!({"action": "delete", "name": "x", "force": true})
            )
            .unwrap(),
            GitRequest::Branches {
                delete: Some("x".into()),
                force: true,
                remote: true
            }
        );
        assert!(parse(BRANCHES, json!({"action": "delete"})).is_err());
        assert!(parse(BRANCHES, json!({"name": "x"})).is_err());
        assert!(parse(CHECKOUT, json!({"name": "x", "start_point": "main"})).is_err());
        assert!(parse(MERGE, json!({})).is_err());
        assert!(parse(MERGE, json!({"abort": true, "branch": "x"})).is_err());
        assert!(matches!(
            parse(MERGE, json!({"abort": true})).unwrap(),
            GitRequest::Merge { abort: true, .. }
        ));
        assert!(parse(REBASE, json!({})).is_err());
        assert!(parse(REBASE, json!({"action": "continue", "onto": "main"})).is_err());
        assert_eq!(
            parse(REBASE, json!({"action": "abort"})).unwrap(),
            GitRequest::Rebase {
                onto: None,
                action: RebaseAction::Abort
            }
        );
        assert_eq!(
            parse(RESET, json!({})).unwrap(),
            GitRequest::Reset {
                revision: "HEAD".into(),
                mode: ResetMode::Mixed
            }
        );
        assert!(parse(RESET, json!({"mode": "keep"})).is_err());
        assert!(parse(STASH, json!({"action": "list", "message": "m"})).is_err());
        assert!(parse(STASH, json!({"action": "push", "index": 1})).is_err());
        assert!(matches!(
            parse(STASH, json!({"action": "pop", "index": 2})).unwrap(),
            GitRequest::Stash {
                action: StashAction::Pop,
                index: 2,
                ..
            }
        ));
        assert!(parse(WORKTREES, json!({"action": "add"})).is_err());
        assert!(parse(WORKTREES, json!({"action": "add", "name": "a b"})).is_err());
        assert!(parse(WORKTREES, json!({"name": "x"})).is_err());
        assert!(parse(CANCEL, json!({"x": 1})).is_err());
        assert!(parse(BLAME, json!({})).is_err());
        assert!(parse("eludite.git.nope", json!({})).is_err());
        assert!(!parse(STATUS, json!({})).unwrap().mutates());
        assert!(!parse(STASH, json!({})).unwrap().mutates());
        assert!(parse(STASH, json!({"action": "drop"})).unwrap().mutates());
        assert!(parse(STAGE, json!({"all": true})).unwrap().mutates());
    }

    #[test]
    fn outputs_follow_their_schemas() {
        let change = ChangeOut {
            path: "b.cs".into(),
            kind: "renamed".into(),
            old_path: Some("a.cs".into()),
        };
        let cases: Vec<(&str, GitOutput)> = vec![
            (
                STATUS,
                GitOutput::Status(Box::new(StatusOutput {
                    state: "ready".into(),
                    repository: Some("/r".into()),
                    generation: 3,
                    branch: Some("main".into()),
                    head: Some(OID.into()),
                    upstream: Some("origin/main".into()),
                    ahead: Some(1),
                    behind: Some(0),
                    operation: Some("merge".into()),
                    staged: vec![change.clone()],
                    unstaged: vec![],
                    untracked: vec!["n.cs".into()],
                    conflicted: vec!["c.cs".into()],
                    ignored: Some(vec!["bin/".into()]),
                    stashes: 2,
                    totals: StatusTotals {
                        staged: 1,
                        unstaged: 0,
                        untracked: 1,
                        conflicted: 1,
                    },
                    truncated: false,
                    ..Default::default()
                })),
            ),
            (
                STATUS,
                GitOutput::Status(Box::new(StatusOutput {
                    state: "none".into(),
                    ..Default::default()
                })),
            ),
            (
                INIT,
                GitOutput::Init(InitOutput {
                    repository: "/r".into(),
                    branch: "main".into(),
                    generation: 1,
                }),
            ),
            (
                STAGE,
                GitOutput::Stage(StageOutput {
                    staged: vec!["a".into()],
                    resolved: vec!["a".into()],
                    generation: 2,
                }),
            ),
            (
                UNSTAGE,
                GitOutput::Unstage(UnstageOutput {
                    unstaged: vec!["a".into()],
                    generation: 2,
                }),
            ),
            (
                DISCARD,
                GitOutput::Discard(DiscardOutput {
                    restored: vec![],
                    deleted: vec!["n".into()],
                    generation: 2,
                }),
            ),
            (
                COMMIT,
                GitOutput::Commit(CommitOutput {
                    commit: OID.into(),
                    summary: "s".into(),
                    branch: Some("main".into()),
                    amended: false,
                    files: 1,
                    generation: 4,
                }),
            ),
            (
                DIFF,
                GitOutput::Diff(Box::new(DiffOutput {
                    path: "a.cs".into(),
                    old_label: "index".into(),
                    new_label: "working tree".into(),
                    binary: false,
                    old_lines: 3,
                    new_lines: 3,
                    added: 1,
                    removed: 1,
                    hunks: vec![DiffHunkOut {
                        old_start: 1,
                        old_len: 1,
                        new_start: 1,
                        new_len: 1,
                        lines: vec![
                            DiffLineOut {
                                kind: "same".into(),
                                old: Some(1),
                                new: Some(1),
                                text: "a".into(),
                            },
                            DiffLineOut {
                                kind: "removed".into(),
                                old: Some(2),
                                new: None,
                                text: "b".into(),
                            },
                            DiffLineOut {
                                kind: "added".into(),
                                old: None,
                                new: Some(2),
                                text: "B".into(),
                            },
                        ],
                    }],
                    omitted: 0,
                    truncated: false,
                })),
            ),
            (
                LOG,
                GitOutput::Log(Box::new(LogOutput {
                    entries: vec![LogEntryOut {
                        commit: OID.into(),
                        short: "0123456".into(),
                        parents: vec![OID.into()],
                        author: "A".into(),
                        email: "a@x".into(),
                        time: 1,
                        date: Some("1970-01-01T00:00:01+00:00".into()),
                        summary: "s".into(),
                        refs: vec!["HEAD -> main".into()],
                        graph: GraphOut {
                            lane: 0,
                            edges: vec![[0, 0], [0, 1]],
                            overflow: false,
                        },
                    }],
                    truncated: true,
                })),
            ),
            (
                BRANCHES,
                GitOutput::Branches(Box::new(BranchesOutput {
                    current: Some("main".into()),
                    branches: vec![BranchOut {
                        name: "main".into(),
                        remote: false,
                        head: true,
                        commit: OID.into(),
                        summary: "s".into(),
                        upstream: Some("origin/main".into()),
                        ahead: Some(0),
                        behind: Some(2),
                    }],
                    tags: vec![TagOut {
                        name: "v1".into(),
                        commit: OID.into(),
                    }],
                    deleted: Some("old".into()),
                    generation: 2,
                })),
            ),
            (
                CHECKOUT,
                GitOutput::Checkout(CheckoutOutput {
                    branch: Some("x".into()),
                    detached: false,
                    commit: OID.into(),
                    created: true,
                    generation: 3,
                }),
            ),
            (
                MERGE,
                GitOutput::Merge(MergeOutput {
                    result: "conflicts".into(),
                    commit: Some(OID.into()),
                    conflicts: vec!["a.cs".into()],
                    generation: 3,
                }),
            ),
            (
                PULL,
                GitOutput::Merge(MergeOutput {
                    result: "fast_forward".into(),
                    commit: Some(OID.into()),
                    conflicts: vec![],
                    generation: 3,
                }),
            ),
            (
                CHERRY_PICK,
                GitOutput::Merge(MergeOutput {
                    result: "picked".into(),
                    commit: Some(OID.into()),
                    conflicts: vec![],
                    generation: 3,
                }),
            ),
            (
                REBASE,
                GitOutput::Rebase(RebaseOutput {
                    result: "conflicts".into(),
                    commit: Some(OID.into()),
                    step: Some(2),
                    steps: Some(3),
                    conflicts: vec!["a.cs".into()],
                    generation: 5,
                }),
            ),
            (
                RESET,
                GitOutput::Reset(ResetOutput {
                    commit: OID.into(),
                    mode: ResetMode::Hard,
                    generation: 6,
                }),
            ),
            (
                STASH,
                GitOutput::Stash(StashOutput {
                    result: "pushed".into(),
                    stashes: vec![StashOut {
                        index: 0,
                        message: "On main: wip".into(),
                        commit: OID.into(),
                    }],
                    conflicts: vec![],
                    generation: 7,
                }),
            ),
            (
                FETCH,
                GitOutput::Fetch(FetchOutput {
                    remote: "origin".into(),
                    updated: vec!["origin/main".into()],
                    received_objects: 3,
                    behind: Some(1),
                    generation: 8,
                }),
            ),
            (
                PUSH,
                GitOutput::Push(PushOutput {
                    remote: "origin".into(),
                    branch: "main".into(),
                    upstream: Some("origin/main".into()),
                    commit: OID.into(),
                    generation: 9,
                }),
            ),
            (
                CANCEL,
                GitOutput::Cancel(CancelOutput {
                    canceled: true,
                    operation: Some("fetch".into()),
                }),
            ),
            (
                WORKTREES,
                GitOutput::Worktrees(WorktreesOutput {
                    worktrees: vec![WorktreeOut {
                        name: "main".into(),
                        path: "/r".into(),
                        branch: Some("main".into()),
                        commit: Some(OID.into()),
                        main: true,
                        locked: false,
                    }],
                    added: None,
                    removed: Some("fix".into()),
                }),
            ),
            (
                BLAME,
                GitOutput::Blame(Box::new(BlameOutput {
                    path: "a.cs".into(),
                    lines: vec![BlameLineOut { line: 1, commit: 0 }],
                    commits: vec![BlameCommitOut {
                        commit: OID.into(),
                        short: "0123456".into(),
                        author: "A".into(),
                        time: 1,
                        date: None,
                        summary: "s".into(),
                    }],
                    truncated: false,
                })),
            ),
        ];
        for (id, out) in cases {
            conforms(schemas(id).2, &out.to_json());
        }
    }

    fn view(policy: AgentPolicy) -> PolicyView {
        PolicyView::of(PolicySnapshot {
            policy,
            ..Default::default()
        })
    }

    fn hook(id: &'static str, input: Value, policy: AgentPolicy) -> Option<Escalation> {
        escalation(id).expect("a hook")(&input, &view(policy))
    }

    #[test]
    fn the_git_policy_object_applies() {
        let none = AgentPolicy::default();
        // Defaults: commits keep their class; push and history prompt; force is refused.
        assert_eq!(hook(COMMIT, json!({"message": "m"}), none.clone()), None);
        assert!(matches!(
            hook(COMMIT, json!({"message": "m", "amend": true}), none.clone()),
            Some(Escalation::Raise { class: PermissionClass::Dangerous, reason, .. }) if reason.contains("git.history")
        ));
        assert!(matches!(
            hook(PUSH, json!({}), none.clone()),
            Some(Escalation::Raise { class: PermissionClass::Dangerous, reason, .. }) if reason.contains("git.push: prompt")
        ));
        assert!(matches!(
            hook(PUSH, json!({"force": true}), none.clone()),
            Some(Escalation::Refuse(_))
        ));
        assert!(matches!(
            hook(CHECKOUT, json!({"name": "x", "force": true}), none.clone()),
            Some(Escalation::Refuse(_))
        ));
        assert_eq!(hook(CHECKOUT, json!({"name": "x"}), none.clone()), None);
        assert!(matches!(
            hook(RESET, json!({}), none.clone()),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        assert!(matches!(
            hook(REBASE, json!({"onto": "main"}), none.clone()),
            Some(Escalation::Raise { .. })
        ));
        assert_eq!(hook(PULL, json!({}), none.clone()), None);
        assert!(matches!(
            hook(PULL, json!({"rebase": true}), none.clone()),
            Some(Escalation::Raise { .. })
        ));
        assert!(matches!(
            hook(MERGE, json!({"abort": true}), none.clone()),
            Some(Escalation::Raise { .. })
        ));
        assert_eq!(hook(MERGE, json!({"branch": "x"}), none.clone()), None);
        // Input-raised classes.
        assert_eq!(hook(STASH, json!({}), none.clone()), None);
        assert!(matches!(
            hook(STASH, json!({"action": "push"}), none.clone()),
            Some(Escalation::Raise {
                class: PermissionClass::Execute,
                ..
            })
        ));
        assert!(matches!(
            hook(STASH, json!({"action": "drop"}), none.clone()),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        assert!(matches!(
            hook(
                BRANCHES,
                json!({"action": "delete", "name": "x"}),
                none.clone()
            ),
            Some(Escalation::Raise {
                class: PermissionClass::Execute,
                ..
            })
        ));
        assert!(matches!(
            hook(
                BRANCHES,
                json!({"action": "delete", "name": "x", "force": true}),
                none.clone()
            ),
            Some(Escalation::Refuse(_))
        ));
        assert!(matches!(
            hook(
                WORKTREES,
                json!({"action": "remove", "name": "x", "force": true}),
                none.clone()
            ),
            Some(Escalation::Refuse(_))
        ));
        assert!(matches!(
            hook(
                WORKTREES,
                json!({"action": "add", "name": "x"}),
                none.clone()
            ),
            Some(Escalation::Raise {
                class: PermissionClass::Execute,
                ..
            })
        ));
        // The policy's knobs.
        let strict = AgentPolicy {
            git: Some(GitPolicy {
                commit: Some(CommitPolicy::Prompt),
                push: Some(GuardPolicy::Deny),
                history: Some(GuardPolicy::Deny),
            }),
            ..AgentPolicy::default()
        };
        assert!(matches!(
            hook(COMMIT, json!({"message": "m"}), strict.clone()),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        assert!(
            matches!(hook(PUSH, json!({}), strict.clone()), Some(Escalation::Refuse(r)) if r.contains("git.push to deny"))
        );
        assert!(matches!(
            hook(RESET, json!({"mode": "hard"}), strict.clone()),
            Some(Escalation::Refuse(_))
        ));
        let deny_commit = AgentPolicy {
            git: Some(GitPolicy {
                commit: Some(CommitPolicy::Deny),
                ..Default::default()
            }),
            ..AgentPolicy::default()
        };
        assert!(matches!(
            hook(COMMIT, json!({"message": "m"}), deny_commit),
            Some(Escalation::Refuse(_))
        ));
        // A tool rule turns a policy refusal into a prompt the rule decides; never a force.
        let ruled = AgentPolicy {
            rules: vec![PolicyRule {
                tool: "eludite-git-push".into(),
                command_prefix: None,
                decision: RuleDecision::Allow,
            }],
            ..strict
        };
        assert!(matches!(
            hook(PUSH, json!({}), ruled.clone()),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        assert!(matches!(
            hook(PUSH, json!({"force": true}), ruled),
            Some(Escalation::Refuse(_))
        ));
        assert!(escalation(STATUS).is_none());
        assert!(escalation(DISCARD).is_none());
    }

    struct Echo;
    impl GitCommands for Echo {
        fn apply(&self, request: GitRequest) -> Result<GitOutput, CommandError> {
            match request {
                GitRequest::Cancel => Ok(GitOutput::Cancel(CancelOutput {
                    canceled: false,
                    operation: None,
                })),
                other => Err(CommandError::Failed(format!("{other:?}"))),
            }
        }
    }

    #[test]
    fn registers_routes_and_refuses_force_for_agents() {
        let r = CommandRegistry::new();
        register(&r, Arc::new(Echo));
        assert_eq!(
            r.invoke(CANCEL, json!({})).unwrap(),
            json!({"canceled": false})
        );
        assert!(matches!(
            r.invoke(STAGE, json!({})),
            Err(CommandError::InvalidInput(_))
        ));
        let push = r.classify(PUSH, &json!({})).unwrap();
        assert_eq!(push.class, PermissionClass::Dangerous);
        // A forced push reaches the handler for the user, and is refused for an agent before it runs.
        let user = r
            .invoke(PUSH, json!({"force": true}))
            .unwrap_err()
            .to_string();
        assert!(user.contains("Push"), "{user}");
        let agent = with_caller(
            Caller::Agent {
                agent: "a".into(),
                call: 1,
                tool_call: None,
            },
            || r.invoke(PUSH, json!({"force": true})),
        )
        .unwrap_err()
        .to_string();
        assert!(
            agent.contains("permission denied") && agent.contains("force"),
            "{agent}"
        );
        assert!(r.has_escalation(RESET));
        assert!(!r.has_escalation(STATUS));
    }
}
