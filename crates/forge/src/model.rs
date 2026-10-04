//! The forge-neutral model: what every family's answers are turned into, serialized as the `eludite.forge.*` output
//! schemas (`protocol/schemas/forge-*.output.json`) describe them.

use serde::{Deserialize, Serialize};

/// A forge family (`forge-detect.output.json`'s `family`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "gitlab")]
    GitLab,
    #[serde(rename = "azure_devops")]
    AzureDevOps,
    Forgejo,
    Gitea,
    Tangled,
    #[default]
    None,
}

impl Family {
    /// The schema name (`azure_devops`).
    pub fn as_str(self) -> &'static str {
        match self {
            Family::GitHub => "github",
            Family::GitLab => "gitlab",
            Family::AzureDevOps => "azure_devops",
            Family::Forgejo => "forgejo",
            Family::Gitea => "gitea",
            Family::Tangled => "tangled",
            Family::None => "none",
        }
    }

    /// From the schema name.
    pub fn parse(s: &str) -> Option<Family> {
        Some(match s {
            "github" => Family::GitHub,
            "gitlab" => Family::GitLab,
            "azure_devops" => Family::AzureDevOps,
            "forgejo" => Family::Forgejo,
            "gitea" => Family::Gitea,
            "tangled" => Family::Tangled,
            "none" => Family::None,
            _ => return None,
        })
    }

    /// The name the person reads ("GitHub", "Azure DevOps").
    pub fn display(self) -> &'static str {
        match self {
            Family::GitHub => "GitHub",
            Family::GitLab => "GitLab",
            Family::AzureDevOps => "Azure DevOps",
            Family::Forgejo => "Forgejo",
            Family::Gitea => "Gitea",
            Family::Tangled => "Tangled",
            Family::None => "no forge",
        }
    }

    /// What this forge calls a pull request, in text the person reads (never in command ids).
    pub fn pull_noun(self) -> &'static str {
        match self {
            Family::GitLab => "merge request",
            _ => "pull request",
        }
    }

    /// What this forge calls an issue.
    pub fn issue_noun(self) -> &'static str {
        match self {
            Family::AzureDevOps => "work item",
            _ => "issue",
        }
    }
}

/// Which repository on which forge (`forge-detect.output.json`'s `repository`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Repository {
    pub family: Family,
    pub host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub web_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    /// The API's base url (not in the schema; the forge implementations read it).
    #[serde(skip)]
    pub api: String,
}

impl Repository {
    /// A key naming this repository on disk (`github.com/owner/name`), safe as a folder path.
    pub fn cache_key(&self) -> String {
        let mut parts = vec![self.host.clone(), self.owner.clone()];
        if let Some(p) = &self.project {
            parts.push(p.clone());
        }
        parts.push(self.name.clone());
        parts
            .iter()
            .map(|p| crate::cache::safe_component(p))
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// What a forge offers (`forge-detect.output.json`'s `capabilities`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Capabilities {
    pub pull_requests: bool,
    pub pull_create: bool,
    pub reviews: bool,
    pub pending_reviews: bool,
    pub review_threads: bool,
    pub thread_resolution: bool,
    pub draft_pull_requests: bool,
    pub merge_methods: Vec<MergeMethod>,
    pub auto_merge: bool,
    pub request_review: bool,
    pub issues: bool,
    pub issue_create: bool,
    pub labels: bool,
    pub milestones: bool,
    pub assignees: bool,
    pub checks: bool,
    pub check_logs: bool,
    pub check_rerun: bool,
    pub branch_from_issue: bool,
    pub branch_link: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub approvals: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub votes: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub work_item_types: bool,
}

/// A merge method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
    RebaseMerge,
    FastForward,
    SemiLinear,
}

impl MergeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
            MergeMethod::RebaseMerge => "rebase_merge",
            MergeMethod::FastForward => "fast_forward",
            MergeMethod::SemiLinear => "semi_linear",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "merge" => MergeMethod::Merge,
            "squash" => MergeMethod::Squash,
            "rebase" => MergeMethod::Rebase,
            "rebase_merge" => MergeMethod::RebaseMerge,
            "fast_forward" => MergeMethod::FastForward,
            "semi_linear" => MergeMethod::SemiLinear,
            _ => return None,
        })
    }

    /// What the confirmation names ("Squash and merge").
    pub fn label(self) -> &'static str {
        match self {
            MergeMethod::Merge => "Create a merge commit",
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
            MergeMethod::RebaseMerge => "Rebase and merge (semi-linear)",
            MergeMethod::FastForward => "Fast-forward merge",
            MergeMethod::SemiLinear => "Merge commit with semi-linear history",
        }
    }
}

/// The signed-in account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Account {
    pub login: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A pull request's or an issue's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Open,
    Closed,
    Merged,
}

/// All of a commit's checks together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChecksState {
    Success,
    Failure,
    Pending,
    #[default]
    None,
}

impl ChecksState {
    /// The combined state of `checks`: failure wins, then pending, then success.
    pub fn of(checks: &[Check]) -> ChecksState {
        if checks.is_empty() {
            return ChecksState::None;
        }
        if checks.iter().any(|c| c.failed()) {
            ChecksState::Failure
        } else if checks.iter().any(|c| c.status != CheckStatus::Completed) {
            ChecksState::Pending
        } else {
            ChecksState::Success
        }
    }
}

/// A pull request in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PullSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    pub id: String,
    pub title: String,
    pub state: State,
    #[serde(default)]
    pub draft: bool,
    pub author: String,
    pub head: String,
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub review_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<ChecksState>,
    /// Who was asked to review (not in the summary schema; the `review_requested` filter reads it).
    #[serde(skip)]
    pub requested_reviewers: Vec<String>,
}

impl PullSummary {
    /// `#12` or the record's key on Tangled.
    pub fn label(&self) -> String {
        item_label(self.number, &self.id)
    }
}

/// `#12`, or the last piece of an AT-URI for Tangled's records.
pub fn item_label(number: Option<u64>, id: &str) -> String {
    match number {
        Some(n) => format!("#{n}"),
        None => id.rsplit('/').next().unwrap_or(id).to_owned(),
    }
}

/// A comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// Which side of the diff a line is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Right,
    Left,
}

/// A review thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Thread {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    pub resolved: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outdated: bool,
    pub comments: Vec<Comment>,
}

/// A review's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Approved,
    ApprovedWithSuggestions,
    ChangesRequested,
    WaitingForAuthor,
    Rejected,
    Commented,
    Pending,
    Dismissed,
    Requested,
}

/// A review (or an approval, or a vote).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub author: String,
    pub state: ReviewState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
}

/// A check's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    #[default]
    CheckRun,
    Status,
    Pipeline,
    Job,
    Policy,
    Workflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    #[default]
    Queued,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Conclusion {
    Success,
    Failure,
    Neutral,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
    #[default]
    None,
}

/// A check on a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Check {
    pub id: String,
    pub name: String,
    pub kind: CheckKind,
    pub status: CheckStatus,
    pub conclusion: Conclusion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    pub has_log: bool,
    pub can_rerun: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

impl Check {
    /// It completed without success (failure, timed out, action required, cancelled).
    pub fn failed(&self) -> bool {
        self.status == CheckStatus::Completed
            && matches!(
                self.conclusion,
                Conclusion::Failure
                    | Conclusion::TimedOut
                    | Conclusion::ActionRequired
                    | Conclusion::Cancelled
            )
    }
}

/// Whether a pull request can merge now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MergeState {
    Clean,
    Conflicts,
    ChecksFailing,
    ChecksPending,
    Blocked,
    Draft,
    #[default]
    Unknown,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Mergeable {
    pub state: MergeState,
    pub methods: Vec<MergeMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: u64,
    pub deletions: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    #[default]
    Modified,
    Deleted,
    Renamed,
    Copied,
}

/// A commit of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Commit {
    pub sha: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// GitLab's approvals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Approvals {
    pub required: u64,
    pub given: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved_by: Vec<String>,
}

/// The signed-in account's pending review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PendingReview {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub comments: u64,
}

/// One pull request with everything the document shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Pull {
    #[serde(flatten)]
    pub summary: PullSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_clone_url: Option<String>,
    #[serde(default)]
    pub commits: Vec<Commit>,
    #[serde(default)]
    pub files: Vec<FileChange>,
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(default)]
    pub conversation: Vec<Comment>,
    #[serde(default)]
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub check_items: Vec<Check>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<Mergeable>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_merge: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approvals: Option<Approvals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iterations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_review: Option<PendingReview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unresolved_threads: Option<u64>,
}

impl Pull {
    /// Fill in what follows from the rest: the checks' state, the unresolved count, the mergeability's check states.
    pub fn finish(&mut self) {
        let state = ChecksState::of(&self.check_items);
        if !self.check_items.is_empty() || self.summary.checks.is_none() {
            self.summary.checks = Some(state);
        }
        self.unresolved_threads = Some(
            self.threads
                .iter()
                .filter(|t| !t.resolved && t.path.is_some())
                .count() as u64,
        );
        if let Some(m) = &mut self.mergeable {
            match self.summary.state {
                State::Merged => m.state = MergeState::Merged,
                State::Closed => m.state = MergeState::Closed,
                State::Open => {
                    if self.summary.draft && m.state == MergeState::Clean {
                        m.state = MergeState::Draft;
                    }
                    if matches!(m.state, MergeState::Clean | MergeState::Unknown) {
                        match state {
                            ChecksState::Failure => m.state = MergeState::ChecksFailing,
                            ChecksState::Pending if m.state == MergeState::Clean => {
                                m.state = MergeState::ChecksPending
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

/// An issue in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct IssueSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    pub id: String,
    pub title: String,
    pub state: IssueState,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "type")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub url: String,
}

impl IssueSummary {
    pub fn label(&self) -> String {
        item_label(self.number, &self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    #[default]
    Open,
    Closed,
}

/// One issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Issue {
    #[serde(flatten)]
    pub summary: IssueSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub conversation: Vec<Comment>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub fields: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<String>,
}

/// One page of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// A pull request or issue as a command names it: its number, or its id (Tangled's AT-URI).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ItemRef {
    Number(u64),
    Id(String),
}

impl ItemRef {
    /// The number, from a number or a numeric id.
    pub fn number(&self) -> Option<u64> {
        match self {
            ItemRef::Number(n) => Some(*n),
            ItemRef::Id(s) => s.trim_start_matches('#').parse().ok(),
        }
    }

    /// The id: the number as text, or the id.
    pub fn id(&self) -> String {
        match self {
            ItemRef::Number(n) => n.to_string(),
            ItemRef::Id(s) => s.clone(),
        }
    }

    /// A short label (`#12`, a record key).
    pub fn label(&self) -> String {
        item_label(self.number(), &self.id())
    }
}

impl std::fmt::Display for ItemRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// Which pull requests to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullFilter {
    #[default]
    All,
    Mine,
    ReviewRequested,
}

/// Which states to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateFilter {
    #[default]
    Open,
    Closed,
    Merged,
    All,
}

/// A pull request list query.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct PullQuery {
    pub filter: PullFilter,
    pub state: StateFilter,
    pub text: Option<String>,
    pub max: usize,
    pub cursor: Option<String>,
    /// The signed-in account's login (for `mine` and `review_requested`).
    pub me: Option<String>,
}

impl PullQuery {
    /// Whether `p` passes the filters (used where a forge cannot filter on the server, and on the cache).
    pub fn matches(&self, p: &PullSummary) -> bool {
        let state = match self.state {
            StateFilter::All => true,
            StateFilter::Open => p.state == State::Open,
            StateFilter::Closed => p.state == State::Closed,
            StateFilter::Merged => p.state == State::Merged,
        };
        let me = self.me.as_deref();
        let filter = match self.filter {
            PullFilter::All => true,
            PullFilter::Mine => me.is_some_and(|m| p.author.eq_ignore_ascii_case(m)),
            PullFilter::ReviewRequested => {
                p.review_requested
                    || me.is_some_and(|m| {
                        p.requested_reviewers
                            .iter()
                            .any(|r| r.eq_ignore_ascii_case(m))
                    })
            }
        };
        let text = self.text.as_deref().is_none_or(|t| {
            let t = t.to_lowercase();
            p.title.to_lowercase().contains(&t) || p.head.to_lowercase().contains(&t)
        });
        state && filter && text
    }
}

/// Which issues to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueFilter {
    #[default]
    All,
    Mine,
    Assigned,
}

/// Issue states to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStateFilter {
    #[default]
    Open,
    Closed,
    All,
}

/// An issue list query.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct IssueQuery {
    pub filter: IssueFilter,
    pub state: IssueStateFilter,
    pub labels: Vec<String>,
    pub text: Option<String>,
    pub max: usize,
    pub cursor: Option<String>,
    pub me: Option<String>,
}

impl IssueQuery {
    pub fn matches(&self, i: &IssueSummary) -> bool {
        let state = match self.state {
            IssueStateFilter::All => true,
            IssueStateFilter::Open => i.state == IssueState::Open,
            IssueStateFilter::Closed => i.state == IssueState::Closed,
        };
        let me = self.me.as_deref();
        let filter = match self.filter {
            IssueFilter::All => true,
            IssueFilter::Mine => me.is_some_and(|m| {
                i.author
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(m))
            }),
            IssueFilter::Assigned => {
                me.is_some_and(|m| i.assignees.iter().any(|a| a.eq_ignore_ascii_case(m)))
            }
        };
        let labels = self
            .labels
            .iter()
            .all(|l| i.labels.iter().any(|x| x.eq_ignore_ascii_case(l)));
        let text = self
            .text
            .as_deref()
            .is_none_or(|t| i.title.to_lowercase().contains(&t.to_lowercase()));
        state && filter && labels && text
    }
}

/// A new pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewPull {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
    pub reviewers: Vec<String>,
    pub labels: Vec<String>,
}

/// Changes to a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullEdit {
    pub title: Option<String>,
    pub body: Option<String>,
    pub draft: Option<bool>,
    pub base: Option<String>,
}

/// Where a comment goes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DraftComment {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(default)]
    pub side: Side,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// A posted comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Posted {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A review's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    #[default]
    Comment,
}

/// A merge.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeRequest {
    pub method: Option<MergeMethod>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub delete_branch: bool,
    pub when_checks_pass: bool,
    /// The head commit the person saw (GitHub's `sha`, Azure DevOps' `lastMergeSourceCommit`).
    pub head_sha: Option<String>,
}

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeOutcome {
    pub merged: bool,
    pub sha: Option<String>,
    pub auto_merge: bool,
    pub message: String,
}

/// A new issue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub kind: Option<String>,
}

/// Changes to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IssueEdit {
    pub state: Option<IssueState>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub milestone: Option<String>,
    pub title: Option<String>,
    pub body: Option<String>,
}

impl IssueEdit {
    pub fn is_empty(&self) -> bool {
        *self == IssueEdit::default()
    }
}

/// Where a pull request's head is fetched from for a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRef {
    /// The remote url to fetch from; `None` is the workspace's remote.
    pub url: Option<String>,
    /// The ref on that remote (`refs/pull/12/head`).
    pub refspec: String,
    /// The local branch name by the forge's convention (`pr/12`).
    pub branch: String,
}

/// A piece of a check's log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogText {
    pub text: String,
    pub url: Option<String>,
}

/// The default branch name for an issue: `issue/<n>-<slug>`.
pub fn issue_branch_name(label: &str, title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let label = label.trim_start_matches('#');
    if slug.is_empty() {
        format!("issue/{label}")
    } else {
        format!("issue/{label}-{slug}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_serialize_by_their_schema_names() {
        for f in [
            Family::GitHub,
            Family::GitLab,
            Family::AzureDevOps,
            Family::Forgejo,
            Family::Gitea,
            Family::Tangled,
            Family::None,
        ] {
            assert_eq!(serde_json::to_value(f).unwrap(), f.as_str());
            assert_eq!(Family::parse(f.as_str()), Some(f));
        }
    }

    #[test]
    fn branch_names_from_issue_titles() {
        assert_eq!(
            issue_branch_name("#42", "Crash when opening a .sln with spaces!"),
            "issue/42-crash-when-opening-a-sln-with-spaces"
        );
        assert_eq!(issue_branch_name("7", "???"), "issue/7");
        assert_eq!(
            issue_branch_name("3mwxz7tjlyc44", "UI: make readme more accessible"),
            "issue/3mwxz7tjlyc44-ui-make-readme-more-accessible"
        );
    }

    #[test]
    fn checks_combine_failure_first_then_pending() {
        let mut a = Check {
            status: CheckStatus::Completed,
            conclusion: Conclusion::Success,
            ..Default::default()
        };
        assert_eq!(
            ChecksState::of(std::slice::from_ref(&a)),
            ChecksState::Success
        );
        let b = Check::default();
        assert_eq!(
            ChecksState::of(&[a.clone(), b.clone()]),
            ChecksState::Pending
        );
        a.conclusion = Conclusion::Failure;
        assert_eq!(ChecksState::of(&[a, b]), ChecksState::Failure);
        assert_eq!(ChecksState::of(&[]), ChecksState::None);
    }

    #[test]
    fn item_refs_read_numbers_and_ids() {
        assert_eq!(ItemRef::Number(12).label(), "#12");
        assert_eq!(ItemRef::Id("12".into()).number(), Some(12));
        let at = ItemRef::Id("at://did:plc:x/sh.tangled.repo.pull/3mwq".into());
        assert_eq!(at.number(), None);
        assert_eq!(at.label(), "3mwq");
    }
}
