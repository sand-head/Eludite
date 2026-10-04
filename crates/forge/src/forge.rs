//! The [`Forge`] trait one implementation per family serves, and the [`Capabilities`] table the windows hide what a
//! forge lacks by.

use crate::error::{ForgeError, Result};
use crate::model::*;

/// How a forge holds a review's comments before it is submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingMode {
    /// Eludite collects them and the submit sends them in one call (GitHub, Forgejo, Gitea).
    Local,
    /// The forge keeps them as drafts (GitLab's draft notes); the submit publishes them.
    Server,
    /// No pending reviews: each comment posts at once and the submit posts the summary (Azure DevOps).
    None,
}

/// One forge repository, signed in or not. Every method sends requests through the forge's [`crate::client::Client`]
/// on the calling thread (never the UI thread), checking its cancellation between requests.
pub trait Forge: Send + Sync {
    fn repository(&self) -> &Repository;
    fn capabilities(&self) -> Capabilities;
    fn pending_mode(&self) -> PendingMode {
        PendingMode::Local
    }
    /// The forge's name for the person ("GitHub").
    fn name(&self) -> &'static str {
        self.repository().family.display()
    }

    /// The signed-in account, from the forge (the sign-in check).
    fn account(&self) -> Result<Account>;

    fn pulls(&self, query: &PullQuery) -> Result<Page<PullSummary>>;
    fn pull(&self, item: &ItemRef) -> Result<Pull>;

    fn create_pull(&self, _new: &NewPull) -> Result<PullSummary> {
        Err(self.unsupported("creating pull requests"))
    }
    fn update_pull(&self, _item: &ItemRef, _edit: &PullEdit) -> Result<PullSummary> {
        Err(self.unsupported("editing pull requests"))
    }
    /// A conversation comment, a comment on a line, or a reply to a thread.
    fn comment_pull(&self, item: &ItemRef, comment: &DraftComment) -> Result<Posted>;
    /// [`PendingMode::Server`]: add a draft comment to the pending review.
    fn add_draft(&self, _item: &ItemRef, _comment: &DraftComment) -> Result<Posted> {
        Err(self.unsupported("pending review comments"))
    }
    /// [`PendingMode::Server`]: drop the drafts.
    fn discard_drafts(&self, _item: &ItemRef) -> Result<()> {
        Ok(())
    }
    /// Submit a review with `event`, `body` and (for [`PendingMode::Local`]) the collected `comments`, in one call.
    fn submit_review(
        &self,
        _item: &ItemRef,
        _event: ReviewEvent,
        _body: Option<&str>,
        _comments: &[DraftComment],
    ) -> Result<Review> {
        Err(self.unsupported("reviews"))
    }
    /// Ask `reviewers` to review again.
    fn request_review(&self, _item: &ItemRef, _reviewers: &[String]) -> Result<Vec<String>> {
        Err(self.unsupported("requesting reviews"))
    }
    /// Where a pull request's head is fetched from, and the local branch by the forge's convention.
    fn checkout_ref(&self, pull: &Pull) -> CheckoutRef;
    fn merge(&self, _item: &ItemRef, _merge: &MergeRequest) -> Result<MergeOutcome> {
        Err(self.unsupported("merging"))
    }
    /// Close (`open: false`) or reopen.
    fn set_pull_state(&self, _item: &ItemRef, _open: bool) -> Result<State> {
        Err(self.unsupported("closing pull requests"))
    }
    fn set_draft(&self, _item: &ItemRef, _draft: bool) -> Result<bool> {
        Err(self.unsupported("draft pull requests"))
    }
    fn resolve_thread(&self, _item: &ItemRef, _thread: &str, _resolved: bool) -> Result<bool> {
        Err(self.unsupported("resolving review threads"))
    }

    fn issues(&self, _query: &IssueQuery) -> Result<Page<IssueSummary>> {
        Err(self.unsupported("issues"))
    }
    fn issue(&self, _item: &ItemRef) -> Result<Issue> {
        Err(self.unsupported("issues"))
    }
    fn create_issue(&self, _new: &NewIssue) -> Result<IssueSummary> {
        Err(self.unsupported("creating issues"))
    }
    fn comment_issue(&self, _item: &ItemRef, _body: &str) -> Result<Posted> {
        Err(self.unsupported("issue comments"))
    }
    fn update_issue(&self, _item: &ItemRef, _edit: &IssueEdit) -> Result<IssueSummary> {
        Err(self.unsupported("editing issues"))
    }
    /// Record on the forge that `branch` (at `commit`) works on the issue; `false` where the forge has no link.
    fn link_branch(&self, _item: &ItemRef, _branch: &str, _commit: &str) -> Result<bool> {
        Ok(false)
    }

    /// The checks of `commit` (and, given `pull`, its policies).
    fn checks(&self, _commit: &str, _pull: Option<&ItemRef>) -> Result<Vec<Check>> {
        Ok(Vec::new())
    }
    fn check_log(&self, _id: &str) -> Result<LogText> {
        Err(self.unsupported("check logs"))
    }
    fn rerun(&self, _id: &str) -> Result<String> {
        Err(self.unsupported("rerunning checks"))
    }

    /// `ForgeError::unsupported` naming this forge.
    fn unsupported(&self, what: &str) -> ForgeError {
        ForgeError::unsupported(what, self.name())
    }
}

/// The capabilities table, per family, as shipped (the report repeats it).
pub fn capabilities(family: Family) -> Capabilities {
    use MergeMethod::*;
    match family {
        Family::GitHub => Capabilities {
            pull_requests: true,
            pull_create: true,
            reviews: true,
            pending_reviews: true,
            review_threads: true,
            thread_resolution: true,
            draft_pull_requests: true,
            merge_methods: vec![Merge, Squash, Rebase],
            auto_merge: true,
            request_review: true,
            issues: true,
            issue_create: true,
            labels: true,
            milestones: true,
            assignees: true,
            checks: true,
            check_logs: true,
            check_rerun: true,
            branch_from_issue: true,
            branch_link: true,
            ..Default::default()
        },
        Family::GitLab => Capabilities {
            pull_requests: true,
            pull_create: true,
            reviews: true,
            pending_reviews: true,
            review_threads: true,
            thread_resolution: true,
            draft_pull_requests: true,
            merge_methods: vec![Merge, Squash, FastForward, SemiLinear],
            auto_merge: true,
            request_review: true,
            issues: true,
            issue_create: true,
            labels: true,
            milestones: true,
            assignees: true,
            checks: true,
            check_logs: true,
            check_rerun: true,
            branch_from_issue: true,
            branch_link: true,
            approvals: true,
            ..Default::default()
        },
        Family::AzureDevOps => Capabilities {
            pull_requests: true,
            pull_create: true,
            reviews: true,
            pending_reviews: false,
            review_threads: true,
            thread_resolution: true,
            draft_pull_requests: true,
            merge_methods: vec![Merge, Squash, Rebase, RebaseMerge],
            auto_merge: true,
            request_review: true,
            issues: true,
            issue_create: true,
            labels: true,
            milestones: true,
            assignees: true,
            checks: true,
            check_logs: true,
            check_rerun: true,
            branch_from_issue: true,
            branch_link: true,
            votes: true,
            work_item_types: true,
            ..Default::default()
        },
        Family::Forgejo | Family::Gitea => Capabilities {
            pull_requests: true,
            pull_create: true,
            reviews: true,
            pending_reviews: true,
            review_threads: true,
            thread_resolution: true,
            draft_pull_requests: true,
            merge_methods: vec![Merge, Squash, Rebase, RebaseMerge, FastForward],
            auto_merge: true,
            request_review: true,
            issues: true,
            issue_create: true,
            labels: true,
            milestones: true,
            assignees: true,
            checks: true,
            check_logs: family == Family::Forgejo,
            check_rerun: false,
            branch_from_issue: true,
            branch_link: true,
            ..Default::default()
        },
        Family::Tangled => Capabilities {
            pull_requests: true,
            pull_create: false,
            reviews: false,
            pending_reviews: false,
            review_threads: false,
            thread_resolution: false,
            draft_pull_requests: false,
            merge_methods: Vec::new(),
            auto_merge: false,
            request_review: false,
            issues: true,
            issue_create: true,
            labels: false,
            milestones: false,
            assignees: false,
            checks: false,
            check_logs: false,
            check_rerun: false,
            branch_from_issue: true,
            branch_link: true,
            ..Default::default()
        },
        Family::None => Capabilities::default(),
    }
}
