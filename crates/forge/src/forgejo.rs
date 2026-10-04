//! Forgejo and Gitea (Codeberg is a Forgejo instance): the REST API at `/api/v1` (`Authorization: token <t>`).
//! Pull requests, reviews (a review's comments sent in one call), issues with labels, milestones and assignees,
//! commit statuses as checks, an Actions job's log (Forgejo 16 and Gitea 1.24 have the `actions/runs` API; older
//! versions answer 404 and the log is the status's page). Drafts are the `WIP:` title prefix, as both forges do it.
//! What the API lacks, the capabilities say: no thread resolution (the API shows a thread's resolver but cannot set
//! it), no reruns.

use serde_json::{Value, json};

use crate::client::{Client, time_of};
use crate::common::{collect, label_names, logins, per_page};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::forge::{Forge, PendingMode, capabilities};
use crate::model::*;
use crate::util::{encode, first_line, normalize_time, query, str_at, str_of, u64_of};

/// The title prefix that makes a pull request a draft.
pub const WIP: &str = "WIP: ";

pub struct Forgejo {
    pub repo: Repository,
    pub client: Client,
}

impl Forgejo {
    pub fn new(repo: Repository, client: Client) -> Self {
        let client = client.with_header("Accept", "application/json");
        Self { repo, client }
    }

    fn r(&self, rest: &str) -> String {
        format!(
            "/repos/{}/{}{rest}",
            encode(&self.repo.owner),
            encode(&self.repo.name)
        )
    }

    fn number(&self, item: &ItemRef) -> Result<u64> {
        item.number().ok_or_else(|| {
            ForgeError::invalid(format!(
                "{} pull requests and issues have numbers, not `{item}`",
                self.name()
            ))
        })
    }

    fn user(v: &Value, key: &str) -> Option<String> {
        v.get(key)
            .and_then(|u| u.get("login").or_else(|| u.get("username")))
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn summary(v: &Value) -> Option<PullSummary> {
        let number = u64_of(v, "number")?;
        let state = if v["merged"] == true {
            State::Merged
        } else if v["state"] == "closed" {
            State::Closed
        } else {
            State::Open
        };
        let title = str_of(v, "title").unwrap_or_default();
        let base_repo = str_at(v, "/base/repo/full_name");
        let head_repo = str_at(v, "/head/repo/full_name");
        Some(PullSummary {
            number: Some(number),
            id: number.to_string(),
            draft: v["draft"] == true || title.starts_with("WIP:") || title.starts_with("[WIP]"),
            title,
            state,
            author: Self::user(v, "user").unwrap_or_default(),
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
            id: v["id"].to_string(),
            author: Self::user(v, "user").unwrap_or_default(),
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
            author: Self::user(v, "user"),
            assignees: logins(v.get("assignees"), "login"),
            labels: label_names(v.get("labels")),
            milestone: str_at(v, "/milestone/title"),
            comments: u64_of(v, "comments"),
            created_at: time_of(v, "created_at"),
            updated_at: time_of(v, "updated_at"),
            url: str_of(v, "html_url").unwrap_or_default(),
        })
    }

    /// Label names to the ids the API takes.
    fn label_ids(&self, names: &[String]) -> Result<Vec<u64>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let (all, _) = self.client.get_all(&self.r("/labels?limit=50"), 500)?;
        names
            .iter()
            .map(|n| {
                all.iter()
                    .find(|l| {
                        l["name"]
                            .as_str()
                            .is_some_and(|x| x.eq_ignore_ascii_case(n))
                    })
                    .and_then(|l| u64_of(l, "id"))
                    .ok_or_else(|| ForgeError::invalid(format!("no label named `{n}`")))
            })
            .collect()
    }

    fn milestone_id(&self, title: &str) -> Result<u64> {
        if title.is_empty() {
            return Ok(0);
        }
        let (all, _) = self
            .client
            .get_all(&self.r("/milestones?state=all&limit=50"), 300)?;
        all.iter()
            .find(|m| {
                m["title"]
                    .as_str()
                    .is_some_and(|t| t.eq_ignore_ascii_case(title))
            })
            .and_then(|m| u64_of(m, "id"))
            .ok_or_else(|| ForgeError::invalid(format!("no milestone named `{title}`")))
    }

    fn methods(&self) -> Vec<MergeMethod> {
        let Ok(r) = self.client.get(&self.r("")) else {
            return vec![MergeMethod::Merge];
        };
        let mut m = Vec::new();
        for (key, method) in [
            ("allow_merge_commits", MergeMethod::Merge),
            ("allow_squash_merge", MergeMethod::Squash),
            ("allow_rebase", MergeMethod::Rebase),
            ("allow_rebase_explicit", MergeMethod::RebaseMerge),
            ("allow_fast_forward_only_merge", MergeMethod::FastForward),
        ] {
            if r[key] == true {
                m.push(method);
            }
        }
        m
    }

    /// A comment on a line as the review API takes it: `new_position` on the head's side, `old_position` on the
    /// base's.
    fn review_comment(c: &DraftComment) -> Value {
        let (new, old) = match c.side {
            Side::Right => (c.line.unwrap_or(0), 0),
            Side::Left => (0, c.line.unwrap_or(0)),
        };
        json!({"path": c.path, "body": c.body, "new_position": new, "old_position": old})
    }

    fn status_check(sha: &str, v: &Value, web: &str) -> Check {
        let (status, conclusion) = match v["status"].as_str().or_else(|| v["state"].as_str()) {
            Some("success") => (CheckStatus::Completed, Conclusion::Success),
            Some("failure") | Some("error") => (CheckStatus::Completed, Conclusion::Failure),
            Some("warning") => (CheckStatus::Completed, Conclusion::Neutral),
            _ => (CheckStatus::InProgress, Conclusion::None),
        };
        let target = str_of(v, "target_url").unwrap_or_default();
        // `/owner/repo/actions/runs/<run index>/jobs/<job index>`: an Actions job, whose log the API serves.
        let job = target.split("/actions/runs/").nth(1).and_then(|rest| {
            let mut p = rest.split('/');
            let run = p.next()?.parse::<u64>().ok()?;
            (p.next()? == "jobs").then_some(())?;
            let job = p.next()?.parse::<u64>().ok()?;
            Some((run, job))
        });
        let url = if target.starts_with('/') {
            let origin = web.split('/').take(3).collect::<Vec<_>>().join("/");
            Some(format!("{origin}{target}"))
        } else if target.is_empty() {
            None
        } else {
            Some(target.clone())
        };
        Check {
            id: match job {
                Some((run, j)) => format!("job:{sha}:{run}:{j}"),
                None => format!("status:{}", v["id"]),
            },
            name: str_of(v, "context").unwrap_or_default(),
            kind: if job.is_some() {
                CheckKind::Job
            } else {
                CheckKind::Status
            },
            status,
            conclusion,
            url,
            started_at: time_of(v, "created_at"),
            duration_seconds: None,
            has_log: job.is_some(),
            can_rerun: false,
            required: false,
        }
    }
}

impl Forge for Forgejo {
    fn repository(&self) -> &Repository {
        &self.repo
    }

    fn capabilities(&self) -> Capabilities {
        let mut c = capabilities(self.repo.family);
        c.thread_resolution = false;
        c
    }

    fn pending_mode(&self) -> PendingMode {
        PendingMode::Local
    }

    fn account(&self) -> Result<Account> {
        let v = self.client.get("/user")?;
        Ok(Account {
            login: str_of(&v, "login")
                .or_else(|| str_of(&v, "username"))
                .unwrap_or_default(),
            name: str_of(&v, "full_name").filter(|s| !s.is_empty()),
            url: str_of(&v, "html_url"),
        })
    }

    fn pulls(&self, q: &PullQuery) -> Result<Page<PullSummary>> {
        let state = match q.state {
            StateFilter::Open => "open",
            StateFilter::Closed | StateFilter::Merged => "closed",
            StateFilter::All => "all",
        };
        let first = self.client.url(&self.r(&format!(
            "/pulls?{}",
            query(&[
                ("state", state.into()),
                ("sort", "recentupdate".into()),
                ("limit", per_page(q.max).to_string())
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
            .ok_or_else(|| ForgeError::other("the forge answered no pull request"))?;
        let head_sha = str_at(&v, "/head/sha");
        let (commits, _) = self.client.get_all(
            &self.r(&format!(
                "/pulls/{n}/commits?limit=50&files=false&verification=false"
            )),
            250,
        )?;
        let commits = commits
            .iter()
            .map(|c| Commit {
                sha: str_of(c, "sha").unwrap_or_default(),
                title: first_line(&str_at(c, "/commit/message").unwrap_or_default()),
                author: str_at(c, "/commit/author/name"),
                date: str_at(c, "/commit/author/date").map(|d| normalize_time(&d)),
            })
            .collect();
        let (files, _) = self
            .client
            .get_all(&self.r(&format!("/pulls/{n}/files?limit=50")), 3000)?;
        let files = files
            .iter()
            .map(|f| FileChange {
                path: str_of(f, "filename").unwrap_or_default(),
                old_path: str_of(f, "previous_filename").filter(|p| !p.is_empty()),
                status: match f["status"].as_str() {
                    Some("added") => FileStatus::Added,
                    Some("deleted") | Some("removed") => FileStatus::Deleted,
                    Some("renamed") => FileStatus::Renamed,
                    Some("copied") => FileStatus::Copied,
                    _ => FileStatus::Modified,
                },
                additions: u64_of(f, "additions").unwrap_or(0),
                deletions: u64_of(f, "deletions").unwrap_or(0),
                binary: false,
            })
            .collect();
        let (raw_reviews, _) = self
            .client
            .get_all(&self.r(&format!("/pulls/{n}/reviews?limit=50")), 500)?;
        let mut reviews = Vec::new();
        let mut threads: Vec<Thread> = Vec::new();
        for r in &raw_reviews {
            let state = match r["state"].as_str() {
                Some("APPROVED") => ReviewState::Approved,
                Some("REQUEST_CHANGES") => ReviewState::ChangesRequested,
                Some("COMMENT") => ReviewState::Commented,
                Some("PENDING") => ReviewState::Pending,
                Some("REQUEST_REVIEW") => ReviewState::Requested,
                _ => continue,
            };
            let state = if r["dismissed"] == true {
                ReviewState::Dismissed
            } else {
                state
            };
            reviews.push(Review {
                id: Some(r["id"].to_string()),
                author: Self::user(r, "user").unwrap_or_default(),
                state,
                body: str_of(r, "body").filter(|b| !b.is_empty()),
                submitted_at: time_of(r, "submitted_at"),
            });
            if u64_of(r, "comments_count").unwrap_or(0) == 0 {
                continue;
            }
            let comments = self
                .client
                .get(&self.r(&format!("/pulls/{n}/reviews/{}/comments", r["id"])))?;
            for c in comments.as_array().into_iter().flatten() {
                let position = u64_of(c, "position").unwrap_or(0);
                let original = u64_of(c, "original_position").unwrap_or(0);
                let (line, side) = if position > 0 {
                    (position, Side::Right)
                } else {
                    (original, Side::Left)
                };
                let path = str_of(c, "path");
                let comment = Self::comment(c);
                match threads
                    .iter_mut()
                    .find(|t| t.path == path && t.line == Some(line) && t.side == Some(side))
                {
                    Some(t) => t.comments.push(comment),
                    None => threads.push(Thread {
                        id: comment.id.clone(),
                        path,
                        line: Some(line),
                        start_line: None,
                        side: Some(side),
                        resolved: c.get("resolver").is_some_and(|x| !x.is_null()),
                        outdated: str_of(c, "commit_id").is_some()
                            && str_of(c, "commit_id") != head_sha,
                        comments: vec![comment],
                    }),
                }
            }
        }
        let (conversation, _) = self
            .client
            .get_all(&self.r(&format!("/issues/{n}/comments?limit=50")), 1000)?;
        let check_items = match &head_sha {
            Some(sha) => self.checks(sha, None).unwrap_or_default(),
            None => Vec::new(),
        };
        let state = match v["mergeable"].as_bool() {
            Some(true) => MergeState::Clean,
            Some(false) => MergeState::Conflicts,
            None => MergeState::Unknown,
        };
        let mut pull = Pull {
            summary,
            body: str_of(&v, "body"),
            head_sha,
            base_sha: str_at(&v, "/merge_base").or_else(|| str_at(&v, "/base/sha")),
            head_clone_url: str_at(&v, "/head/repo/clone_url").filter(|_| {
                str_at(&v, "/head/repo/full_name") != str_at(&v, "/base/repo/full_name")
            }),
            commits,
            files,
            threads,
            conversation: conversation.iter().map(Self::comment).collect(),
            reviews,
            check_items,
            mergeable: Some(Mergeable {
                state,
                methods: self.methods(),
                reason: None,
            }),
            ..Default::default()
        };
        pull.finish();
        Ok(pull)
    }

    fn create_pull(&self, new: &NewPull) -> Result<PullSummary> {
        let title = if new.draft && !new.title.starts_with("WIP:") {
            format!("{WIP}{}", new.title)
        } else {
            new.title.clone()
        };
        let mut body =
            json!({"title": title, "body": new.body, "head": new.head, "base": new.base});
        if !new.labels.is_empty() {
            body["labels"] = json!(self.label_ids(&new.labels)?);
        }
        let v = self.client.post(&self.r("/pulls"), &body)?;
        let mut s = Self::summary(&v)
            .ok_or_else(|| ForgeError::other("the forge answered no pull request"))?;
        if !new.reviewers.is_empty() {
            let n = s.number.unwrap_or(0);
            self.client.post(
                &self.r(&format!("/pulls/{n}/requested_reviewers")),
                &json!({"reviewers": new.reviewers}),
            )?;
            s.requested_reviewers = new.reviewers.clone();
        }
        Ok(s)
    }

    fn update_pull(&self, item: &ItemRef, edit: &PullEdit) -> Result<PullSummary> {
        let n = self.number(item)?;
        let current = self.client.get(&self.r(&format!("/pulls/{n}")))?;
        let mut body = serde_json::Map::new();
        let mut title = edit
            .title
            .clone()
            .unwrap_or_else(|| str_of(&current, "title").unwrap_or_default());
        if let Some(d) = edit.draft {
            let bare = title
                .trim_start_matches("WIP:")
                .trim_start_matches("[WIP]")
                .trim_start()
                .to_owned();
            title = if d { format!("{WIP}{bare}") } else { bare };
        }
        if edit.title.is_some() || edit.draft.is_some() {
            body.insert("title".into(), json!(title));
        }
        if let Some(b) = &edit.body {
            body.insert("body".into(), json!(b));
        }
        if let Some(b) = &edit.base {
            body.insert("base".into(), json!(b));
        }
        let v = if body.is_empty() {
            current
        } else {
            self.client
                .patch(&self.r(&format!("/pulls/{n}")), &Value::Object(body))?
        };
        Self::summary(&v).ok_or_else(|| ForgeError::other("the forge answered no pull request"))
    }

    fn comment_pull(&self, item: &ItemRef, c: &DraftComment) -> Result<Posted> {
        let n = self.number(item)?;
        let mut c = c.clone();
        if let Some(thread) = &c.reply_to {
            // A reply is a comment on the thread's line: find it.
            let pull = self.pull(item)?;
            let t = pull
                .threads
                .iter()
                .find(|t| t.id == *thread)
                .ok_or_else(|| {
                    ForgeError::new(ErrorKind::NotFound, format!("no review thread `{thread}`"))
                })?;
            c.path = t.path.clone();
            c.line = t.line;
            c.side = t.side.unwrap_or_default();
        }
        if c.path.is_some() && c.line.is_some() {
            let v = self.client.post(
                &self.r(&format!("/pulls/{n}/reviews")),
                &json!({"event": "COMMENT", "body": "", "comments": [Self::review_comment(&c)]}),
            )?;
            return Ok(Posted {
                id: v["id"].to_string(),
                thread: c.reply_to.clone(),
                url: str_of(&v, "html_url"),
            });
        }
        let v = self.client.post(
            &self.r(&format!("/issues/{n}/comments")),
            &json!({"body": c.body}),
        )?;
        Ok(Posted {
            id: v["id"].to_string(),
            thread: None,
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
        let v = self.client.post(
            &self.r(&format!("/pulls/{n}/reviews")),
            &json!({
                "event": match event {
                    ReviewEvent::Approve => "APPROVED",
                    ReviewEvent::RequestChanges => "REQUEST_CHANGES",
                    ReviewEvent::Comment => "COMMENT",
                },
                "body": body.unwrap_or(""),
                "comments": comments.iter().map(Self::review_comment).collect::<Vec<_>>(),
            }),
        )?;
        Ok(Review {
            id: Some(v["id"].to_string()),
            author: Self::user(&v, "user").unwrap_or_default(),
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
        self.client.post(
            &self.r(&format!("/pulls/{n}/requested_reviewers")),
            &json!({"reviewers": reviewers}),
        )?;
        Ok(reviewers.to_vec())
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
        let mut body = json!({
            "Do": match method {
                MergeMethod::Squash => "squash",
                MergeMethod::Rebase => "rebase",
                MergeMethod::RebaseMerge => "rebase-merge",
                MergeMethod::FastForward => "fast-forward-only",
                _ => "merge",
            },
            "delete_branch_after_merge": m.delete_branch,
            "merge_when_checks_succeed": m.when_checks_pass,
        });
        if let Some(t) = &m.title {
            body["MergeTitleField"] = json!(t);
        }
        if let Some(msg) = &m.message {
            body["MergeMessageField"] = json!(msg);
        }
        if let Some(sha) = &m.head_sha {
            body["head_commit_id"] = json!(sha);
        }
        self.client
            .post(&self.r(&format!("/pulls/{n}/merge")), &body)?;
        if m.when_checks_pass {
            return Ok(MergeOutcome {
                merged: false,
                sha: None,
                auto_merge: true,
                message: "scheduled to merge when its checks succeed".into(),
            });
        }
        let after = self.client.get(&self.r(&format!("/pulls/{n}")))?;
        Ok(MergeOutcome {
            merged: after["merged"] == true,
            sha: str_of(&after, "merge_commit_sha"),
            auto_merge: false,
            message: "merged".into(),
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
        let s = self.update_pull(
            item,
            &PullEdit {
                draft: Some(draft),
                ..Default::default()
            },
        )?;
        Ok(s.draft)
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
            ("type", "issues".into()),
            ("limit", per_page(q.max).to_string()),
            ("labels", q.labels.join(",")),
            ("q", q.text.clone().unwrap_or_default()),
        ];
        match (q.filter, &q.me) {
            (IssueFilter::Mine, Some(me)) => pairs.push(("created_by", me.clone())),
            (IssueFilter::Assigned, Some(me)) => pairs.push(("assigned_by", me.clone())),
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
                if v.get("pull_request").is_some_and(|p| !p.is_null()) {
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
        let summary = Self::issue_summary(&v)
            .ok_or_else(|| ForgeError::other("the forge answered no issue"))?;
        let (comments, _) = self
            .client
            .get_all(&self.r(&format!("/issues/{n}/comments?limit=50")), 1000)?;
        Ok(Issue {
            summary,
            body: str_of(&v, "body"),
            conversation: comments.iter().map(Self::comment).collect(),
            branches: str_of(&v, "ref")
                .filter(|r| !r.is_empty())
                .map(|r| vec![r.trim_start_matches("refs/heads/").to_owned()])
                .unwrap_or_default(),
            ..Default::default()
        })
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueSummary> {
        let mut body = json!({"title": new.title, "body": new.body});
        if !new.labels.is_empty() {
            body["labels"] = json!(self.label_ids(&new.labels)?);
        }
        if !new.assignees.is_empty() {
            body["assignees"] = json!(new.assignees);
        }
        if let Some(m) = &new.milestone {
            body["milestone"] = json!(self.milestone_id(m)?);
        }
        let v = self.client.post(&self.r("/issues"), &body)?;
        Self::issue_summary(&v).ok_or_else(|| ForgeError::other("the forge answered no issue"))
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
        if let Some(labels) = &e.labels {
            let ids = self.label_ids(labels)?;
            self.client.put(
                &self.r(&format!("/issues/{n}/labels")),
                &json!({"labels": ids}),
            )?;
        }
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
        if let Some(a) = &e.assignees {
            body.insert("assignees".into(), json!(a));
        }
        if let Some(m) = &e.milestone {
            body.insert("milestone".into(), json!(self.milestone_id(m)?));
        }
        if let Some(t) = &e.title {
            body.insert("title".into(), json!(t));
        }
        if let Some(b) = &e.body {
            body.insert("body".into(), json!(b));
        }
        let v = if body.is_empty() {
            self.client.get(&self.r(&format!("/issues/{n}")))?
        } else {
            self.client
                .patch(&self.r(&format!("/issues/{n}")), &Value::Object(body))?
        };
        Self::issue_summary(&v).ok_or_else(|| ForgeError::other("the forge answered no issue"))
    }

    fn link_branch(&self, item: &ItemRef, branch: &str, _commit: &str) -> Result<bool> {
        let n = self.number(item)?;
        self.client.patch(
            &self.r(&format!("/issues/{n}")),
            &json!({"ref": format!("refs/heads/{branch}")}),
        )?;
        Ok(true)
    }

    fn checks(&self, commit: &str, _pull: Option<&ItemRef>) -> Result<Vec<Check>> {
        let (statuses, _) = self.client.get_all(
            &self.r(&format!("/commits/{commit}/statuses?limit=50&sort=newest")),
            500,
        )?;
        // The API lists every status ever set; the newest per context is the check.
        let mut out: Vec<Check> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut sorted: Vec<&Value> = statuses.iter().collect();
        sorted.sort_by_key(|s| std::cmp::Reverse(u64_of(s, "id").unwrap_or(0)));
        for s in sorted {
            let ctx = str_of(s, "context").unwrap_or_default();
            if seen.insert(ctx) {
                out.push(Self::status_check(commit, s, &self.repo.web_url));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn check_log(&self, id: &str) -> Result<LogText> {
        let mut p = id
            .strip_prefix("job:")
            .ok_or_else(|| self.unsupported("logs of this status (open its page)"))?
            .split(':');
        let (sha, run, job) = (
            p.next().unwrap_or(""),
            p.next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0),
            p.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0),
        );
        let runs =
            self.client
                .get(&self.r(&format!("/actions/runs?head_sha={sha}&limit=50")))
                .map_err(|e| match e.kind {
                    ErrorKind::NotFound => self
                        .unsupported("Actions logs in this version's API (open the status's page)"),
                    _ => e,
                })?;
        let run_id = runs["workflow_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| u64_of(r, "index_in_repo") == Some(run))
            .and_then(|r| u64_of(r, "id"))
            .ok_or_else(|| {
                ForgeError::new(
                    ErrorKind::NotFound,
                    format!("no Actions run {run} for {sha}"),
                )
            })?;
        let jobs = self
            .client
            .get(&self.r(&format!("/actions/runs/{run_id}/jobs")))?;
        let job_id = jobs
            .as_array()
            .and_then(|a| a.get(job))
            .and_then(|j| u64_of(j, "id"))
            .ok_or_else(|| {
                ForgeError::new(ErrorKind::NotFound, format!("no job {job} in run {run}"))
            })?;
        let text = self
            .client
            .get_text(&self.r(&format!("/actions/jobs/{job_id}/logs")))?;
        Ok(LogText {
            text,
            url: Some(format!(
                "{}/actions/runs/{run}/jobs/{job}",
                self.repo.web_url
            )),
        })
    }
}
