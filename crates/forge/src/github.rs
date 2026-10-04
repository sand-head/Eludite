//! GitHub (github.com and GitHub Enterprise Server): REST API v3 (`X-GitHub-Api-Version: 2022-11-28`), and GraphQL
//! where REST lacks a feature: review threads' resolution, draft and ready, auto-merge, and the issue's linked branch
//! (the development panel). Checks are check runs and commit statuses; an Actions job's log and rerun.

use serde_json::{Value, json};

use crate::client::{Client, duration_between, time_of};
use crate::common::{collect, label_names, logins, per_page};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::forge::{Forge, PendingMode, capabilities};
use crate::model::*;
use crate::util::{encode, first_line, query, str_at, str_of, u64_of};

/// The REST API version every call names.
pub const API_VERSION: &str = "2022-11-28";

pub struct GitHub {
    pub repo: Repository,
    pub client: Client,
}

impl GitHub {
    pub fn new(repo: Repository, client: Client) -> Self {
        let client = client
            .with_header("Accept", "application/vnd.github+json")
            .with_header("X-GitHub-Api-Version", API_VERSION);
        Self { repo, client }
    }

    fn r(&self, rest: &str) -> String {
        format!(
            "/repos/{}/{}{rest}",
            encode(&self.repo.owner),
            encode(&self.repo.name)
        )
    }

    fn graphql_url(&self) -> String {
        match self.client.base.strip_suffix("/v3") {
            Some(api) => format!("{api}/graphql"),
            None => format!("{}/graphql", self.client.base),
        }
    }

    fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let v = self.client.post(
            &self.graphql_url(),
            &json!({ "query": query, "variables": variables }),
        )?;
        if let Some(errs) = v.get("errors").and_then(Value::as_array)
            && let Some(e) = errs.first()
        {
            let message = e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("GraphQL error")
                .to_owned();
            let kind = match e.get("type").and_then(Value::as_str) {
                Some("NOT_FOUND") => ErrorKind::NotFound,
                Some("FORBIDDEN") => ErrorKind::Forbidden,
                _ => ErrorKind::Invalid,
            };
            return Err(ForgeError::new(kind, format!("GitHub: {message}")));
        }
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
    }

    fn number(&self, item: &ItemRef) -> Result<u64> {
        item.number().ok_or_else(|| {
            ForgeError::invalid(format!(
                "GitHub pull requests and issues have numbers, not `{item}`"
            ))
        })
    }

    fn summary(v: &Value) -> Option<PullSummary> {
        let number = u64_of(v, "number")?;
        let state = if v.get("merged_at").is_some_and(|m| !m.is_null()) || v["merged"] == true {
            State::Merged
        } else if v["state"] == "closed" {
            State::Closed
        } else {
            State::Open
        };
        let base_repo = str_at(v, "/base/repo/full_name");
        let head_repo = str_at(v, "/head/repo/full_name");
        Some(PullSummary {
            number: Some(number),
            id: number.to_string(),
            title: str_of(v, "title").unwrap_or_default(),
            state,
            draft: v["draft"] == true,
            author: str_at(v, "/user/login").unwrap_or_default(),
            head: str_at(v, "/head/ref").unwrap_or_default(),
            base: str_at(v, "/base/ref").unwrap_or_default(),
            head_repository: head_repo.filter(|h| Some(h) != base_repo.as_ref()),
            created_at: time_of(v, "created_at"),
            updated_at: time_of(v, "updated_at"),
            url: str_of(v, "html_url").unwrap_or_default(),
            labels: label_names(v.get("labels")),
            comments: u64_of(v, "comments"),
            review_requested: false,
            checks: None,
            requested_reviewers: logins(v.get("requested_reviewers"), "login"),
        })
    }

    fn comment(v: &Value) -> Comment {
        Comment {
            id: v
                .get("id")
                .map(|x| x.to_string().trim_matches('"').to_owned())
                .unwrap_or_default(),
            author: str_at(v, "/user/login").unwrap_or_default(),
            body: str_of(v, "body").unwrap_or_default(),
            created_at: time_of(v, "created_at"),
            url: str_of(v, "html_url"),
            pending: false,
        }
    }

    fn issue_summary(v: &Value) -> Option<IssueSummary> {
        let number = u64_of(v, "number")?;
        Some(IssueSummary {
            number: Some(number),
            id: number.to_string(),
            title: str_of(v, "title").unwrap_or_default(),
            state: if v["state"] == "closed" {
                IssueState::Closed
            } else {
                IssueState::Open
            },
            kind: None,
            author: str_at(v, "/user/login"),
            assignees: logins(v.get("assignees"), "login"),
            labels: label_names(v.get("labels")),
            milestone: str_at(v, "/milestone/title"),
            comments: u64_of(v, "comments"),
            created_at: time_of(v, "created_at"),
            updated_at: time_of(v, "updated_at"),
            url: str_of(v, "html_url").unwrap_or_default(),
        })
    }

    /// Review threads with their resolution through GraphQL, keyed by the first comment's database id.
    fn graphql_threads(&self, number: u64) -> Option<Vec<(String, String, bool, bool)>> {
        let q = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{id isResolved isOutdated comments(first:1){nodes{databaseId}}}}}}}";
        let data = self
            .graphql(
                q,
                json!({"owner": self.repo.owner, "name": self.repo.name, "number": number}),
            )
            .ok()?;
        let nodes = data
            .pointer("/repository/pullRequest/reviewThreads/nodes")?
            .as_array()?;
        Some(
            nodes
                .iter()
                .filter_map(|n| {
                    let first = n.pointer("/comments/nodes/0/databaseId")?.as_u64()?;
                    Some((
                        first.to_string(),
                        str_of(n, "id")?,
                        n["isResolved"] == true,
                        n["isOutdated"] == true,
                    ))
                })
                .collect(),
        )
    }

    fn node_id(&self, kind: &str, number: u64) -> Result<String> {
        let v = self.client.get(&self.r(&format!("/{kind}/{number}")))?;
        str_of(&v, "node_id").ok_or_else(|| ForgeError::other("GitHub answered no node_id"))
    }

    fn milestone_number(&self, title: &str) -> Result<Option<u64>> {
        if title.is_empty() {
            return Ok(None);
        }
        let (all, _) = self
            .client
            .get_all(&self.r("/milestones?state=all&per_page=100"), 300)?;
        all.iter()
            .find(|m| {
                m["title"]
                    .as_str()
                    .is_some_and(|t| t.eq_ignore_ascii_case(title))
            })
            .and_then(|m| u64_of(m, "number"))
            .map(Some)
            .ok_or_else(|| ForgeError::invalid(format!("no milestone named `{title}`")))
    }

    fn check_run(v: &Value, repo_actions: bool) -> Check {
        let status = match v["status"].as_str() {
            Some("completed") => CheckStatus::Completed,
            Some("in_progress") => CheckStatus::InProgress,
            _ => CheckStatus::Queued,
        };
        let conclusion = match v["conclusion"].as_str() {
            Some("success") => Conclusion::Success,
            Some("failure") => Conclusion::Failure,
            Some("neutral") => Conclusion::Neutral,
            Some("cancelled") => Conclusion::Cancelled,
            Some("skipped") => Conclusion::Skipped,
            Some("timed_out") => Conclusion::TimedOut,
            Some("action_required") => Conclusion::ActionRequired,
            _ => Conclusion::None,
        };
        let actions = repo_actions && str_at(v, "/app/slug").as_deref() == Some("github-actions");
        let started = time_of(v, "started_at");
        let completed = time_of(v, "completed_at");
        Check {
            id: format!("run:{}", v["id"]),
            name: str_of(v, "name").unwrap_or_default(),
            kind: CheckKind::CheckRun,
            status,
            conclusion,
            url: str_of(v, "html_url"),
            duration_seconds: duration_between(started.as_deref(), completed.as_deref()),
            started_at: started,
            has_log: actions,
            can_rerun: actions && status == CheckStatus::Completed,
            required: false,
        }
    }

    fn status_check(v: &Value) -> Check {
        let (status, conclusion) = match v["state"].as_str() {
            Some("success") => (CheckStatus::Completed, Conclusion::Success),
            Some("failure") => (CheckStatus::Completed, Conclusion::Failure),
            Some("error") => (CheckStatus::Completed, Conclusion::Failure),
            _ => (CheckStatus::InProgress, Conclusion::None),
        };
        Check {
            id: format!("status:{}", v["id"]),
            name: str_of(v, "context").unwrap_or_default(),
            kind: CheckKind::Status,
            status,
            conclusion,
            url: str_of(v, "target_url"),
            started_at: time_of(v, "created_at"),
            duration_seconds: None,
            has_log: false,
            can_rerun: false,
            required: false,
        }
    }

    fn methods(&self) -> Vec<MergeMethod> {
        let Ok(r) = self.client.get(&self.r("")) else {
            return vec![MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase];
        };
        let mut m = Vec::new();
        if r["allow_merge_commit"] != false {
            m.push(MergeMethod::Merge);
        }
        if r["allow_squash_merge"] != false {
            m.push(MergeMethod::Squash);
        }
        if r["allow_rebase_merge"] != false {
            m.push(MergeMethod::Rebase);
        }
        m
    }
}

impl Forge for GitHub {
    fn repository(&self) -> &Repository {
        &self.repo
    }

    fn capabilities(&self) -> Capabilities {
        capabilities(Family::GitHub)
    }

    fn pending_mode(&self) -> PendingMode {
        PendingMode::Local
    }

    fn account(&self) -> Result<Account> {
        let v = self.client.get("/user")?;
        Ok(Account {
            login: str_of(&v, "login").unwrap_or_default(),
            name: str_of(&v, "name"),
            url: str_of(&v, "html_url"),
        })
    }

    fn pulls(&self, q: &PullQuery) -> Result<Page<PullSummary>> {
        let state = match q.state {
            StateFilter::Open => "open",
            StateFilter::Closed | StateFilter::Merged | StateFilter::All => {
                if q.state == StateFilter::All {
                    "all"
                } else {
                    "closed"
                }
            }
        };
        let first = self.client.url(&self.r(&format!(
            "/pulls?{}",
            query(&[
                ("state", state.into()),
                ("per_page", per_page(q.max).to_string()),
                ("sort", "updated".into()),
                ("direction", "desc".into()),
            ])
        )));
        let me = q.me.clone();
        let mut page = collect(
            &self.client,
            first,
            q.cursor.as_deref(),
            q.max,
            Self::summary,
            |p| q.matches(p),
        )?;
        for p in &mut page.items {
            p.review_requested = me.as_deref().is_some_and(|m| {
                p.requested_reviewers
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(m))
            });
        }
        Ok(page)
    }

    fn pull(&self, item: &ItemRef) -> Result<Pull> {
        let n = self.number(item)?;
        let v = self.client.get(&self.r(&format!("/pulls/{n}")))?;
        let summary = Self::summary(&v)
            .ok_or_else(|| ForgeError::other("GitHub answered no pull request"))?;
        let head_sha = str_at(&v, "/head/sha");
        let (commits, _) = self
            .client
            .get_all(&self.r(&format!("/pulls/{n}/commits?per_page=100")), 250)?;
        let commits = commits
            .iter()
            .map(|c| Commit {
                sha: str_of(c, "sha").unwrap_or_default(),
                title: first_line(&str_at(c, "/commit/message").unwrap_or_default()),
                author: str_at(c, "/commit/author/name"),
                date: str_at(c, "/commit/author/date").map(|d| crate::util::normalize_time(&d)),
            })
            .collect();
        let (files, _) = self
            .client
            .get_all(&self.r(&format!("/pulls/{n}/files?per_page=100")), 3000)?;
        let files = files
            .iter()
            .map(|f| FileChange {
                path: str_of(f, "filename").unwrap_or_default(),
                old_path: str_of(f, "previous_filename"),
                status: match f["status"].as_str() {
                    Some("added") => FileStatus::Added,
                    Some("removed") => FileStatus::Deleted,
                    Some("renamed") => FileStatus::Renamed,
                    Some("copied") => FileStatus::Copied,
                    _ => FileStatus::Modified,
                },
                additions: u64_of(f, "additions").unwrap_or(0),
                deletions: u64_of(f, "deletions").unwrap_or(0),
                binary: f.get("patch").is_none() && f["changes"] != 0,
            })
            .collect();
        let (review_comments, _) = self
            .client
            .get_all(&self.r(&format!("/pulls/{n}/comments?per_page=100")), 3000)?;
        let mut threads: Vec<Thread> = Vec::new();
        for c in &review_comments {
            let comment = Self::comment(c);
            match c.get("in_reply_to_id").and_then(Value::as_u64) {
                Some(parent) => {
                    let key = parent.to_string();
                    match threads
                        .iter_mut()
                        .find(|t| t.id == key || t.comments.iter().any(|x| x.id == key))
                    {
                        Some(t) => t.comments.push(comment),
                        None => threads.push(Thread {
                            id: key,
                            comments: vec![comment],
                            ..Default::default()
                        }),
                    }
                }
                None => threads.push(Thread {
                    id: comment.id.clone(),
                    path: str_of(c, "path"),
                    line: u64_of(c, "line").or_else(|| u64_of(c, "original_line")),
                    start_line: u64_of(c, "start_line"),
                    side: Some(if c["side"] == "LEFT" {
                        Side::Left
                    } else {
                        Side::Right
                    }),
                    resolved: false,
                    outdated: c.get("line").is_some_and(Value::is_null),
                    comments: vec![comment],
                }),
            }
        }
        if let Some(gq) = self.graphql_threads(n) {
            for t in &mut threads {
                if let Some((_, _, resolved, outdated)) =
                    gq.iter().find(|(first, ..)| *first == t.id)
                {
                    t.resolved = *resolved;
                    t.outdated = *outdated;
                }
            }
        }
        let (conversation, _) = self
            .client
            .get_all(&self.r(&format!("/issues/{n}/comments?per_page=100")), 1000)?;
        let conversation = conversation.iter().map(Self::comment).collect();
        let (reviews, _) = self
            .client
            .get_all(&self.r(&format!("/pulls/{n}/reviews?per_page=100")), 500)?;
        let mut reviews: Vec<Review> = reviews
            .iter()
            .filter_map(|r| {
                let state = match r["state"].as_str()? {
                    "APPROVED" => ReviewState::Approved,
                    "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
                    "COMMENTED" => ReviewState::Commented,
                    "PENDING" => ReviewState::Pending,
                    "DISMISSED" => ReviewState::Dismissed,
                    _ => return None,
                };
                Some(Review {
                    id: r.get("id").map(|x| x.to_string()),
                    author: str_at(r, "/user/login").unwrap_or_default(),
                    state,
                    body: str_of(r, "body").filter(|b| !b.is_empty()),
                    submitted_at: time_of(r, "submitted_at"),
                })
            })
            .collect();
        for r in &summary.requested_reviewers {
            reviews.push(Review {
                id: None,
                author: r.clone(),
                state: ReviewState::Requested,
                body: None,
                submitted_at: None,
            });
        }
        let check_items = match &head_sha {
            Some(sha) => self.checks(sha, None).unwrap_or_default(),
            None => Vec::new(),
        };
        let mergeable_state = v["mergeable_state"].as_str().unwrap_or("unknown");
        let state = match (v["mergeable"].as_bool(), mergeable_state) {
            (Some(false), _) | (_, "dirty") => MergeState::Conflicts,
            (_, "blocked") => MergeState::Blocked,
            (_, "draft") => MergeState::Draft,
            (_, "unstable") => MergeState::ChecksFailing,
            (Some(true), _) => MergeState::Clean,
            _ => MergeState::Unknown,
        };
        let mut pull = Pull {
            summary,
            body: str_of(&v, "body"),
            head_sha,
            base_sha: str_at(&v, "/base/sha"),
            head_clone_url: str_at(&v, "/head/repo/clone_url").filter(|_| {
                str_at(&v, "/head/repo/full_name") != str_at(&v, "/base/repo/full_name")
            }),
            commits,
            files,
            threads,
            conversation,
            reviews,
            check_items,
            mergeable: Some(Mergeable {
                state,
                methods: self.methods(),
                reason: Some(format!("GitHub: mergeable_state {mergeable_state}")),
            }),
            auto_merge: v.get("auto_merge").is_some_and(|a| !a.is_null()),
            ..Default::default()
        };
        pull.finish();
        Ok(pull)
    }

    fn create_pull(&self, new: &NewPull) -> Result<PullSummary> {
        let v = self.client.post(
            &self.r("/pulls"),
            &json!({"title": new.title, "body": new.body, "head": new.head, "base": new.base, "draft": new.draft}),
        )?;
        let mut s = Self::summary(&v)
            .ok_or_else(|| ForgeError::other("GitHub answered no pull request"))?;
        let n = s.number.unwrap_or(0);
        if !new.reviewers.is_empty() {
            self.client.post(
                &self.r(&format!("/pulls/{n}/requested_reviewers")),
                &json!({"reviewers": new.reviewers}),
            )?;
            s.requested_reviewers = new.reviewers.clone();
        }
        if !new.labels.is_empty() {
            self.client.post(
                &self.r(&format!("/issues/{n}/labels")),
                &json!({"labels": new.labels}),
            )?;
            s.labels = new.labels.clone();
        }
        Ok(s)
    }

    fn update_pull(&self, item: &ItemRef, edit: &PullEdit) -> Result<PullSummary> {
        let n = self.number(item)?;
        let mut body = serde_json::Map::new();
        if let Some(t) = &edit.title {
            body.insert("title".into(), json!(t));
        }
        if let Some(b) = &edit.body {
            body.insert("body".into(), json!(b));
        }
        if let Some(b) = &edit.base {
            body.insert("base".into(), json!(b));
        }
        let mut v = if body.is_empty() {
            self.client.get(&self.r(&format!("/pulls/{n}")))?
        } else {
            self.client
                .patch(&self.r(&format!("/pulls/{n}")), &Value::Object(body))?
        };
        if let Some(d) = edit.draft {
            self.set_draft(item, d)?;
            v["draft"] = json!(d);
        }
        Self::summary(&v).ok_or_else(|| ForgeError::other("GitHub answered no pull request"))
    }

    fn comment_pull(&self, item: &ItemRef, c: &DraftComment) -> Result<Posted> {
        let n = self.number(item)?;
        let v = if let Some(thread) = &c.reply_to {
            self.client.post(
                &self.r(&format!("/pulls/{n}/comments/{}/replies", encode(thread))),
                &json!({"body": c.body}),
            )?
        } else if let (Some(path), Some(line)) = (&c.path, c.line) {
            let pr = self.client.get(&self.r(&format!("/pulls/{n}")))?;
            let mut body = json!({
                "body": c.body, "path": path, "line": line,
                "side": if c.side == Side::Left { "LEFT" } else { "RIGHT" },
                "commit_id": str_at(&pr, "/head/sha"),
            });
            if let Some(s) = c.start_line {
                body["start_line"] = json!(s);
            }
            self.client
                .post(&self.r(&format!("/pulls/{n}/comments")), &body)?
        } else {
            self.client.post(
                &self.r(&format!("/issues/{n}/comments")),
                &json!({"body": c.body}),
            )?
        };
        let id = v["id"].to_string();
        Ok(Posted {
            thread: c
                .reply_to
                .clone()
                .or_else(|| c.path.as_ref().map(|_| id.clone())),
            id,
            url: str_of(&v, "html_url"),
        })
    }

    fn submit_review(
        &self,
        item: &ItemRef,
        event: ReviewEvent,
        body: Option<&str>,
        comments: &[DraftComment],
    ) -> Result<Review> {
        let n = self.number(item)?;
        let comments: Vec<Value> = comments
            .iter()
            .map(|c| {
                let mut v = json!({
                    "path": c.path, "line": c.line, "body": c.body,
                    "side": if c.side == Side::Left { "LEFT" } else { "RIGHT" },
                });
                if let Some(s) = c.start_line {
                    v["start_line"] = json!(s);
                }
                v
            })
            .collect();
        let v = self.client.post(
            &self.r(&format!("/pulls/{n}/reviews")),
            &json!({
                "event": match event {
                    ReviewEvent::Approve => "APPROVE",
                    ReviewEvent::RequestChanges => "REQUEST_CHANGES",
                    ReviewEvent::Comment => "COMMENT",
                },
                "body": body.unwrap_or(""),
                "comments": comments,
            }),
        )?;
        Ok(Review {
            id: Some(v["id"].to_string()),
            author: str_at(&v, "/user/login").unwrap_or_default(),
            state: match event {
                ReviewEvent::Approve => ReviewState::Approved,
                ReviewEvent::RequestChanges => ReviewState::ChangesRequested,
                ReviewEvent::Comment => ReviewState::Commented,
            },
            body: body.map(str::to_owned),
            submitted_at: time_of(&v, "submitted_at"),
        })
    }

    fn request_review(&self, item: &ItemRef, reviewers: &[String]) -> Result<Vec<String>> {
        let n = self.number(item)?;
        let v = self.client.post(
            &self.r(&format!("/pulls/{n}/requested_reviewers")),
            &json!({"reviewers": reviewers}),
        )?;
        Ok(logins(v.get("requested_reviewers"), "login"))
    }

    fn checkout_ref(&self, pull: &Pull) -> CheckoutRef {
        let n = pull.summary.number.unwrap_or(0);
        CheckoutRef {
            url: None,
            refspec: format!("refs/pull/{n}/head"),
            branch: format!("pr/{n}"),
        }
    }

    fn merge(&self, item: &ItemRef, m: &MergeRequest) -> Result<MergeOutcome> {
        let n = self.number(item)?;
        let method = m.method.unwrap_or(MergeMethod::Merge);
        let gh_method = match method {
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
            _ => "merge",
        };
        if m.when_checks_pass {
            let id = self.node_id("pulls", n)?;
            self.graphql(
                "mutation($id:ID!,$method:PullRequestMergeMethod!){enablePullRequestAutoMerge(input:{pullRequestId:$id,mergeMethod:$method}){pullRequest{number}}}",
                json!({"id": id, "method": gh_method.to_ascii_uppercase()}),
            )?;
            return Ok(MergeOutcome {
                merged: false,
                sha: None,
                auto_merge: true,
                message: "auto-merge enabled: GitHub merges it when its checks pass".into(),
            });
        }
        let mut body = json!({"merge_method": gh_method});
        if let Some(t) = &m.title {
            body["commit_title"] = json!(t);
        }
        if let Some(msg) = &m.message {
            body["commit_message"] = json!(msg);
        }
        if let Some(sha) = &m.head_sha {
            body["sha"] = json!(sha);
        }
        let v = self
            .client
            .put(&self.r(&format!("/pulls/{n}/merge")), &body)?;
        if m.delete_branch {
            let pr = self.client.get(&self.r(&format!("/pulls/{n}")))?;
            if str_at(&pr, "/head/repo/full_name") == str_at(&pr, "/base/repo/full_name")
                && let Some(branch) = str_at(&pr, "/head/ref")
            {
                let _ = self.client.send(
                    crate::http::Method::Delete,
                    &self.r(&format!("/git/refs/heads/{}", branch)),
                    None,
                );
            }
        }
        Ok(MergeOutcome {
            merged: v["merged"] == true,
            sha: str_of(&v, "sha"),
            auto_merge: false,
            message: str_of(&v, "message").unwrap_or_else(|| "merged".into()),
        })
    }

    fn set_pull_state(&self, item: &ItemRef, open: bool) -> Result<State> {
        let n = self.number(item)?;
        let v = self.client.patch(
            &self.r(&format!("/pulls/{n}")),
            &json!({"state": if open { "open" } else { "closed" }}),
        )?;
        Ok(Self::summary(&v).map(|s| s.state).unwrap_or(State::Closed))
    }

    fn set_draft(&self, item: &ItemRef, draft: bool) -> Result<bool> {
        let n = self.number(item)?;
        let id = self.node_id("pulls", n)?;
        let mutation = if draft {
            "mutation($id:ID!){convertPullRequestToDraft(input:{pullRequestId:$id}){pullRequest{isDraft}}}"
        } else {
            "mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){pullRequest{isDraft}}}"
        };
        let data = self.graphql(mutation, json!({"id": id}))?;
        let field = if draft {
            "/convertPullRequestToDraft/pullRequest/isDraft"
        } else {
            "/markPullRequestReadyForReview/pullRequest/isDraft"
        };
        Ok(data
            .pointer(field)
            .and_then(Value::as_bool)
            .unwrap_or(draft))
    }

    fn resolve_thread(&self, item: &ItemRef, thread: &str, resolved: bool) -> Result<bool> {
        let n = self.number(item)?;
        let threads = self.graphql_threads(n).ok_or_else(|| {
            ForgeError::other("GitHub's GraphQL API did not answer the review threads")
        })?;
        let (_, node, ..) = threads
            .iter()
            .find(|(first, node, ..)| first == thread || node == thread)
            .ok_or_else(|| {
                ForgeError::new(ErrorKind::NotFound, format!("no review thread `{thread}`"))
            })?;
        let (mutation, field) = if resolved {
            (
                "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}",
                "/resolveReviewThread/thread/isResolved",
            )
        } else {
            (
                "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{isResolved}}}",
                "/unresolveReviewThread/thread/isResolved",
            )
        };
        let data = self.graphql(mutation, json!({"id": node}))?;
        Ok(data
            .pointer(field)
            .and_then(Value::as_bool)
            .unwrap_or(resolved))
    }

    fn issues(&self, q: &IssueQuery) -> Result<Page<IssueSummary>> {
        let mut pairs = vec![
            (
                "state",
                match q.state {
                    IssueStateFilter::Open => "open",
                    IssueStateFilter::Closed => "closed",
                    IssueStateFilter::All => "all",
                }
                .to_owned(),
            ),
            ("per_page", per_page(q.max).to_string()),
            ("labels", q.labels.join(",")),
        ];
        match (q.filter, &q.me) {
            (IssueFilter::Mine, Some(me)) => pairs.push(("creator", me.clone())),
            (IssueFilter::Assigned, Some(me)) => pairs.push(("assignee", me.clone())),
            _ => {}
        }
        let first = self
            .client
            .url(&self.r(&format!("/issues?{}", query(&pairs))));
        collect(
            &self.client,
            first,
            q.cursor.as_deref(),
            q.max,
            |v| {
                if v.get("pull_request").is_some() {
                    None
                } else {
                    Self::issue_summary(v)
                }
            },
            |i| q.matches(i),
        )
    }

    fn issue(&self, item: &ItemRef) -> Result<Issue> {
        let n = self.number(item)?;
        let v = self.client.get(&self.r(&format!("/issues/{n}")))?;
        let summary =
            Self::issue_summary(&v).ok_or_else(|| ForgeError::other("GitHub answered no issue"))?;
        let (comments, _) = self
            .client
            .get_all(&self.r(&format!("/issues/{n}/comments?per_page=100")), 1000)?;
        Ok(Issue {
            summary,
            body: str_of(&v, "body"),
            conversation: comments.iter().map(Self::comment).collect(),
            ..Default::default()
        })
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueSummary> {
        let mut body = json!({"title": new.title, "body": new.body});
        if !new.labels.is_empty() {
            body["labels"] = json!(new.labels);
        }
        if !new.assignees.is_empty() {
            body["assignees"] = json!(new.assignees);
        }
        if let Some(m) = &new.milestone
            && let Some(num) = self.milestone_number(m)?
        {
            body["milestone"] = json!(num);
        }
        let v = self.client.post(&self.r("/issues"), &body)?;
        Self::issue_summary(&v).ok_or_else(|| ForgeError::other("GitHub answered no issue"))
    }

    fn comment_issue(&self, item: &ItemRef, body: &str) -> Result<Posted> {
        let n = self.number(item)?;
        let v = self.client.post(
            &self.r(&format!("/issues/{n}/comments")),
            &json!({"body": body}),
        )?;
        Ok(Posted {
            id: v["id"].to_string(),
            thread: None,
            url: str_of(&v, "html_url"),
        })
    }

    fn update_issue(&self, item: &ItemRef, e: &IssueEdit) -> Result<IssueSummary> {
        let n = self.number(item)?;
        let mut body = serde_json::Map::new();
        if let Some(s) = e.state {
            body.insert(
                "state".into(),
                json!(if s == IssueState::Closed {
                    "closed"
                } else {
                    "open"
                }),
            );
        }
        if let Some(l) = &e.labels {
            body.insert("labels".into(), json!(l));
        }
        if let Some(a) = &e.assignees {
            body.insert("assignees".into(), json!(a));
        }
        if let Some(m) = &e.milestone {
            body.insert("milestone".into(), json!(self.milestone_number(m)?));
        }
        if let Some(t) = &e.title {
            body.insert("title".into(), json!(t));
        }
        if let Some(b) = &e.body {
            body.insert("body".into(), json!(b));
        }
        let v = self
            .client
            .patch(&self.r(&format!("/issues/{n}")), &Value::Object(body))?;
        Self::issue_summary(&v).ok_or_else(|| ForgeError::other("GitHub answered no issue"))
    }

    fn link_branch(&self, item: &ItemRef, branch: &str, commit: &str) -> Result<bool> {
        let n = self.number(item)?;
        let issue = self.node_id("issues", n)?;
        let repo = self.client.get(&self.r(""))?;
        let repo_id = str_of(&repo, "node_id").unwrap_or_default();
        self.graphql(
            "mutation($issue:ID!,$oid:GitObjectID!,$name:String!,$repo:ID!){createLinkedBranch(input:{issueId:$issue,oid:$oid,name:$name,repositoryId:$repo}){linkedBranch{id}}}",
            json!({"issue": issue, "oid": commit, "name": branch, "repo": repo_id}),
        )?;
        Ok(true)
    }

    fn checks(&self, commit: &str, _pull: Option<&ItemRef>) -> Result<Vec<Check>> {
        let mut out = Vec::new();
        let runs = self
            .client
            .get(&self.r(&format!("/commits/{commit}/check-runs?per_page=100")))?;
        for r in runs["check_runs"].as_array().into_iter().flatten() {
            out.push(Self::check_run(r, true));
        }
        if let Ok(status) = self
            .client
            .get(&self.r(&format!("/commits/{commit}/status")))
            && let Some(list) = status["statuses"].as_array()
        {
            out.extend(list.iter().map(Self::status_check));
        }
        Ok(out)
    }

    fn check_log(&self, id: &str) -> Result<LogText> {
        let job = id
            .strip_prefix("run:")
            .ok_or_else(|| self.unsupported("logs of commit statuses (open the status's page)"))?;
        let text = self
            .client
            .get_text(&self.r(&format!("/actions/jobs/{job}/logs")))?;
        Ok(LogText {
            text,
            url: Some(format!("{}/actions/runs/jobs/{job}", self.repo.web_url)),
        })
    }

    fn rerun(&self, id: &str) -> Result<String> {
        let job = id
            .strip_prefix("run:")
            .ok_or_else(|| self.unsupported("rerunning commit statuses"))?;
        self.client
            .post(&self.r(&format!("/actions/jobs/{job}/rerun")), &json!({}))?;
        Ok(format!("GitHub Actions job {job} queued again"))
    }
}
