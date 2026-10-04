//! GitLab (gitlab.com and self-managed): the REST API v4 (`Authorization: Bearer <t>`, personal access tokens and
//! OAuth alike). Merge requests are this crate's pull requests: diff discussions anchored to lines, draft notes as
//! the pending review (published together), approvals and approval rules, draft merge requests (the `Draft:` title
//! prefix), the project's merge method (merge commit, semi-linear, fast-forward) with squash, merge when the pipeline
//! succeeds; issues with labels, milestones, assignees and weight; pipelines and jobs as checks with job logs and
//! retries.

use serde_json::{Value, json};

use crate::client::{Client, time_of};
use crate::common::{collect, label_names, logins, per_page};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::forge::{Forge, PendingMode, capabilities};
use crate::model::*;
use crate::util::{encode, first_line, normalize_time, query, str_at, str_of, u64_of};

pub const DRAFT: &str = "Draft: ";

pub struct GitLab {
    pub repo: Repository,
    pub client: Client,
}

impl GitLab {
    pub fn new(repo: Repository, client: Client) -> Self {
        Self { repo, client }
    }

    fn p(&self, rest: &str) -> String {
        format!(
            "/projects/{}{rest}",
            encode(&format!("{}/{}", self.repo.owner, self.repo.name))
        )
    }

    fn iid(&self, item: &ItemRef) -> Result<u64> {
        item.number().ok_or_else(|| {
            ForgeError::invalid(format!(
                "GitLab merge requests and issues have numbers (iid), not `{item}`"
            ))
        })
    }

    fn summary(v: &Value) -> Option<PullSummary> {
        let iid = u64_of(v, "iid")?;
        let state = match v["state"].as_str() {
            Some("merged") => State::Merged,
            Some("closed") | Some("locked") => State::Closed,
            _ => State::Open,
        };
        let fork = v["source_project_id"] != v["target_project_id"];
        Some(PullSummary {
            number: Some(iid),
            id: iid.to_string(),
            title: str_of(v, "title").unwrap_or_default(),
            state,
            draft: v["draft"] == true || v["work_in_progress"] == true,
            author: str_at(v, "/author/username").unwrap_or_default(),
            head: str_of(v, "source_branch").unwrap_or_default(),
            base: str_of(v, "target_branch").unwrap_or_default(),
            head_repository: fork.then(|| format!("project {}", v["source_project_id"])),
            created_at: time_of(v, "created_at"),
            updated_at: time_of(v, "updated_at"),
            url: str_of(v, "web_url").unwrap_or_default(),
            labels: label_names(v.get("labels")),
            comments: u64_of(v, "user_notes_count"),
            review_requested: false,
            checks: v
                .pointer("/head_pipeline/status")
                .and_then(Value::as_str)
                .map(pipeline_state),
            requested_reviewers: logins(v.get("reviewers"), "username"),
        })
    }

    fn note(v: &Value) -> Comment {
        Comment {
            id: v["id"].to_string(),
            author: str_at(v, "/author/username").unwrap_or_default(),
            body: str_of(v, "body").unwrap_or_default(),
            created_at: time_of(v, "created_at"),
            url: None,
            pending: false,
        }
    }

    fn issue_summary(v: &Value) -> Option<IssueSummary> {
        let iid = u64_of(v, "iid")?;
        Some(IssueSummary {
            number: Some(iid),
            id: iid.to_string(),
            title: str_of(v, "title").unwrap_or_default(),
            state: if v["state"] == "closed" {
                IssueState::Closed
            } else {
                IssueState::Open
            },
            kind: str_of(v, "issue_type").filter(|t| t != "issue"),
            author: str_at(v, "/author/username"),
            assignees: logins(v.get("assignees"), "username"),
            labels: label_names(v.get("labels")),
            milestone: str_at(v, "/milestone/title"),
            comments: u64_of(v, "user_notes_count"),
            created_at: time_of(v, "created_at"),
            updated_at: time_of(v, "updated_at"),
            url: str_of(v, "web_url").unwrap_or_default(),
        })
    }

    fn user_ids(&self, names: &[String]) -> Result<Vec<u64>> {
        names
            .iter()
            .map(|n| {
                let v = self
                    .client
                    .get(&format!("/users?{}", query(&[("username", n.clone())])))?;
                v.as_array()
                    .and_then(|a| a.first())
                    .and_then(|u| u64_of(u, "id"))
                    .ok_or_else(|| ForgeError::invalid(format!("no GitLab user `{n}`")))
            })
            .collect()
    }

    fn milestone_id(&self, title: &str) -> Result<Value> {
        if title.is_empty() {
            return Ok(Value::Null);
        }
        let v = self.client.get(&self.p(&format!(
            "/milestones?{}",
            query(&[("title", title.to_owned())])
        )))?;
        v.as_array()
            .and_then(|a| a.first())
            .and_then(|m| u64_of(m, "id"))
            .map(Value::from)
            .ok_or_else(|| ForgeError::invalid(format!("no milestone named `{title}`")))
    }

    fn position(&self, mr: &Value, c: &DraftComment) -> Value {
        let path = c.path.clone().unwrap_or_default();
        let mut pos = json!({
            "position_type": "text",
            "base_sha": str_at(mr, "/diff_refs/base_sha"),
            "start_sha": str_at(mr, "/diff_refs/start_sha"),
            "head_sha": str_at(mr, "/diff_refs/head_sha"),
            "new_path": path, "old_path": path,
        });
        match c.side {
            Side::Right => pos["new_line"] = json!(c.line),
            Side::Left => pos["old_line"] = json!(c.line),
        }
        pos
    }

    fn job_check(v: &Value, required: bool) -> Check {
        let s = v["status"].as_str().unwrap_or("");
        let (status, conclusion) = job_status(s);
        let finished = status == CheckStatus::Completed;
        Check {
            id: format!("job:{}", v["id"]),
            name: format!(
                "{} / {}",
                str_of(v, "stage").unwrap_or_default(),
                str_of(v, "name").unwrap_or_default()
            ),
            kind: CheckKind::Job,
            status,
            conclusion,
            url: str_of(v, "web_url"),
            started_at: time_of(v, "started_at"),
            duration_seconds: v["duration"].as_f64().map(|d| d.round() as u64),
            has_log: s != "created" && s != "manual" && s != "skipped",
            can_rerun: finished,
            required,
        }
    }
}

fn job_status(s: &str) -> (CheckStatus, Conclusion) {
    match s {
        "success" => (CheckStatus::Completed, Conclusion::Success),
        "failed" => (CheckStatus::Completed, Conclusion::Failure),
        "canceled" | "canceling" => (CheckStatus::Completed, Conclusion::Cancelled),
        "skipped" => (CheckStatus::Completed, Conclusion::Skipped),
        "manual" => (CheckStatus::Completed, Conclusion::ActionRequired),
        "running" => (CheckStatus::InProgress, Conclusion::None),
        _ => (CheckStatus::Queued, Conclusion::None),
    }
}

fn pipeline_state(s: &str) -> ChecksState {
    match s {
        "success" => ChecksState::Success,
        "failed" | "canceled" => ChecksState::Failure,
        "skipped" => ChecksState::None,
        _ => ChecksState::Pending,
    }
}

/// Additions and deletions of a unified diff.
fn count_diff(diff: &str) -> (u64, u64) {
    let mut add = 0;
    let mut del = 0;
    for l in diff.lines() {
        if l.starts_with('+') && !l.starts_with("+++") {
            add += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            del += 1;
        }
    }
    (add, del)
}

impl Forge for GitLab {
    fn repository(&self) -> &Repository {
        &self.repo
    }

    fn capabilities(&self) -> Capabilities {
        capabilities(Family::GitLab)
    }

    fn pending_mode(&self) -> PendingMode {
        PendingMode::Server
    }

    fn account(&self) -> Result<Account> {
        let v = self.client.get("/user")?;
        Ok(Account {
            login: str_of(&v, "username").unwrap_or_default(),
            name: str_of(&v, "name"),
            url: str_of(&v, "web_url"),
        })
    }

    fn pulls(&self, q: &PullQuery) -> Result<Page<PullSummary>> {
        let mut pairs = vec![
            (
                "state",
                match q.state {
                    StateFilter::Open => "opened",
                    StateFilter::Closed => "closed",
                    StateFilter::Merged => "merged",
                    StateFilter::All => "all",
                }
                .to_owned(),
            ),
            ("order_by", "updated_at".into()),
            ("per_page", per_page(q.max).to_string()),
            ("search", q.text.clone().unwrap_or_default()),
        ];
        match (q.filter, &q.me) {
            (PullFilter::Mine, Some(me)) => pairs.push(("author_username", me.clone())),
            (PullFilter::ReviewRequested, Some(me)) => {
                pairs.push(("reviewer_username", me.clone()))
            }
            _ => {}
        }
        let first = self
            .client
            .url(&self.p(&format!("/merge_requests?{}", query(&pairs))));
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
        let iid = self.iid(item)?;
        let v = self
            .client
            .get(&self.p(&format!("/merge_requests/{iid}")))?;
        let summary = Self::summary(&v)
            .ok_or_else(|| ForgeError::other("GitLab answered no merge request"))?;
        let head_sha = str_at(&v, "/diff_refs/head_sha").or_else(|| str_of(&v, "sha"));
        let (commits, _) = self.client.get_all(
            &self.p(&format!("/merge_requests/{iid}/commits?per_page=100")),
            250,
        )?;
        let commits = commits
            .iter()
            .map(|c| Commit {
                sha: str_of(c, "id").unwrap_or_default(),
                title: str_of(c, "title")
                    .unwrap_or_else(|| first_line(&str_of(c, "message").unwrap_or_default())),
                author: str_of(c, "author_name"),
                date: str_of(c, "authored_date").map(|d| normalize_time(&d)),
            })
            .collect();
        let (diffs, _) = self.client.get_all(
            &self.p(&format!("/merge_requests/{iid}/diffs?per_page=100")),
            3000,
        )?;
        let files = diffs
            .iter()
            .map(|d| {
                let (additions, deletions) = count_diff(d["diff"].as_str().unwrap_or(""));
                FileChange {
                    path: str_of(d, "new_path").unwrap_or_default(),
                    old_path: (d["renamed_file"] == true)
                        .then(|| str_of(d, "old_path"))
                        .flatten(),
                    status: if d["new_file"] == true {
                        FileStatus::Added
                    } else if d["deleted_file"] == true {
                        FileStatus::Deleted
                    } else if d["renamed_file"] == true {
                        FileStatus::Renamed
                    } else {
                        FileStatus::Modified
                    },
                    additions,
                    deletions,
                    binary: d["diff"]
                        .as_str()
                        .is_some_and(|x| x.starts_with("Binary files")),
                }
            })
            .collect();
        // Discussions need a token on gitlab.com: anonymously the thread list is empty, not an error.
        let discussions = match self.client.get_all(
            &self.p(&format!("/merge_requests/{iid}/discussions?per_page=100")),
            2000,
        ) {
            Ok((d, _)) => d,
            Err(e)
                if matches!(e.kind, ErrorKind::SignInRequired | ErrorKind::Forbidden)
                    && !self.client.signed_in() =>
            {
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let mut threads = Vec::new();
        let mut conversation = Vec::new();
        for d in &discussions {
            let notes: Vec<&Value> = d["notes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|n| n["system"] != true)
                .collect();
            let Some(first) = notes.first() else { continue };
            let resolvable = first["resolvable"] == true;
            let pos = first.get("position").filter(|p| !p.is_null());
            if d["individual_note"] == true && pos.is_none() && !resolvable {
                conversation.push(Self::note(first));
                continue;
            }
            let (line, side, path) = match pos {
                Some(p) => match (u64_of(p, "new_line"), u64_of(p, "old_line")) {
                    (Some(n), _) => (Some(n), Some(Side::Right), str_of(p, "new_path")),
                    (None, Some(o)) => (Some(o), Some(Side::Left), str_of(p, "old_path")),
                    _ => (None, None, str_of(p, "new_path")),
                },
                None => (None, None, None),
            };
            threads.push(Thread {
                id: str_of(d, "id").unwrap_or_default(),
                path,
                line,
                start_line: None,
                side,
                resolved: notes.iter().all(|n| n["resolved"] == true) && resolvable,
                outdated: pos.is_some_and(|p| str_of(p, "head_sha") != head_sha),
                comments: notes.iter().map(|n| Self::note(n)).collect(),
            });
        }
        let approvals = self
            .client
            .get(&self.p(&format!("/merge_requests/{iid}/approvals")))
            .ok();
        let mut reviews: Vec<Review> = Vec::new();
        if let Some(a) = &approvals {
            for u in a["approved_by"].as_array().into_iter().flatten() {
                reviews.push(Review {
                    id: None,
                    author: str_at(u, "/user/username").unwrap_or_default(),
                    state: ReviewState::Approved,
                    body: None,
                    submitted_at: None,
                });
            }
        }
        for r in &summary.requested_reviewers {
            if !reviews.iter().any(|x| &x.author == r) {
                reviews.push(Review {
                    id: None,
                    author: r.clone(),
                    state: ReviewState::Requested,
                    body: None,
                    submitted_at: None,
                });
            }
        }
        let check_items = match &head_sha {
            Some(sha) => self.checks(sha, None).unwrap_or_default(),
            None => Vec::new(),
        };
        let state = match v["detailed_merge_status"].as_str().unwrap_or("") {
            "mergeable" => MergeState::Clean,
            "conflict" | "broken_status" => MergeState::Conflicts,
            "ci_must_pass" => MergeState::ChecksFailing,
            "ci_still_running" => MergeState::ChecksPending,
            "draft_status" => MergeState::Draft,
            "not_open" => MergeState::Closed,
            "checking" | "unchecked" | "" => MergeState::Unknown,
            _ => MergeState::Blocked,
        };
        let project = self.client.get(&self.p("")).unwrap_or(Value::Null);
        let mut methods = vec![match project["merge_method"].as_str() {
            Some("ff") => MergeMethod::FastForward,
            Some("rebase_merge") => MergeMethod::SemiLinear,
            _ => MergeMethod::Merge,
        }];
        if project["squash_option"].as_str() != Some("never") {
            methods.push(MergeMethod::Squash);
        }
        let mut pull = Pull {
            summary,
            body: str_of(&v, "description"),
            head_sha,
            base_sha: str_at(&v, "/diff_refs/base_sha"),
            head_clone_url: None,
            commits,
            files,
            threads,
            conversation,
            reviews,
            check_items,
            mergeable: Some(Mergeable {
                state,
                methods,
                reason: str_of(&v, "detailed_merge_status").map(|s| format!("GitLab: {s}")),
            }),
            auto_merge: v["merge_when_pipeline_succeeds"] == true
                || v["auto_merge_enabled"] == true,
            approvals: approvals.as_ref().map(|a| Approvals {
                required: u64_of(a, "approvals_required").unwrap_or(0),
                given: a["approved_by"]
                    .as_array()
                    .map(|x| x.len() as u64)
                    .unwrap_or(0),
                approved_by: a["approved_by"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|u| str_at(u, "/user/username"))
                    .collect(),
            }),
            ..Default::default()
        };
        if v.get("head_pipeline").is_some_and(|p| !p.is_null()) && pull.check_items.is_empty() {
            pull.summary.checks = v
                .pointer("/head_pipeline/status")
                .and_then(Value::as_str)
                .map(pipeline_state);
        }
        pull.finish();
        Ok(pull)
    }

    fn create_pull(&self, new: &NewPull) -> Result<PullSummary> {
        let title = if new.draft && !new.title.starts_with("Draft:") {
            format!("{DRAFT}{}", new.title)
        } else {
            new.title.clone()
        };
        let mut body = json!({
            "source_branch": new.head, "target_branch": new.base, "title": title, "description": new.body,
        });
        if !new.labels.is_empty() {
            body["labels"] = json!(new.labels.join(","));
        }
        if !new.reviewers.is_empty() {
            body["reviewer_ids"] = json!(self.user_ids(&new.reviewers)?);
        }
        let v = self.client.post(&self.p("/merge_requests"), &body)?;
        Self::summary(&v).ok_or_else(|| ForgeError::other("GitLab answered no merge request"))
    }

    fn update_pull(&self, item: &ItemRef, edit: &PullEdit) -> Result<PullSummary> {
        let iid = self.iid(item)?;
        let mut body = serde_json::Map::new();
        let mut title = edit.title.clone();
        if let Some(d) = edit.draft {
            let current = match &title {
                Some(t) => t.clone(),
                None => str_of(
                    &self
                        .client
                        .get(&self.p(&format!("/merge_requests/{iid}")))?,
                    "title",
                )
                .unwrap_or_default(),
            };
            let bare = current
                .trim_start_matches("Draft:")
                .trim_start_matches("[Draft]")
                .trim_start()
                .to_owned();
            title = Some(if d { format!("{DRAFT}{bare}") } else { bare });
        }
        if let Some(t) = title {
            body.insert("title".into(), json!(t));
        }
        if let Some(b) = &edit.body {
            body.insert("description".into(), json!(b));
        }
        if let Some(b) = &edit.base {
            body.insert("target_branch".into(), json!(b));
        }
        let v = self.client.put(
            &self.p(&format!("/merge_requests/{iid}")),
            &Value::Object(body),
        )?;
        Self::summary(&v).ok_or_else(|| ForgeError::other("GitLab answered no merge request"))
    }

    fn comment_pull(&self, item: &ItemRef, c: &DraftComment) -> Result<Posted> {
        let iid = self.iid(item)?;
        if let Some(d) = &c.reply_to {
            let v = self.client.post(
                &self.p(&format!(
                    "/merge_requests/{iid}/discussions/{}/notes",
                    encode(d)
                )),
                &json!({"body": c.body}),
            )?;
            return Ok(Posted {
                id: v["id"].to_string(),
                thread: Some(d.clone()),
                url: None,
            });
        }
        if c.path.is_some() && c.line.is_some() {
            let mr = self
                .client
                .get(&self.p(&format!("/merge_requests/{iid}")))?;
            let v = self.client.post(
                &self.p(&format!("/merge_requests/{iid}/discussions")),
                &json!({"body": c.body, "position": self.position(&mr, c)}),
            )?;
            let note = v.pointer("/notes/0").cloned().unwrap_or(Value::Null);
            return Ok(Posted {
                id: note["id"].to_string(),
                thread: str_of(&v, "id"),
                url: None,
            });
        }
        let v = self.client.post(
            &self.p(&format!("/merge_requests/{iid}/notes")),
            &json!({"body": c.body}),
        )?;
        Ok(Posted {
            id: v["id"].to_string(),
            thread: None,
            url: None,
        })
    }

    fn add_draft(&self, item: &ItemRef, c: &DraftComment) -> Result<Posted> {
        let iid = self.iid(item)?;
        let mut body = json!({"note": c.body});
        if let Some(d) = &c.reply_to {
            body["in_reply_to_discussion_id"] = json!(d);
        } else if c.path.is_some() && c.line.is_some() {
            let mr = self
                .client
                .get(&self.p(&format!("/merge_requests/{iid}")))?;
            body["position"] = self.position(&mr, c);
        }
        let v = self.client.post(
            &self.p(&format!("/merge_requests/{iid}/draft_notes")),
            &body,
        )?;
        Ok(Posted {
            id: v["id"].to_string(),
            thread: c.reply_to.clone(),
            url: None,
        })
    }

    fn discard_drafts(&self, item: &ItemRef) -> Result<()> {
        let iid = self.iid(item)?;
        let drafts = self
            .client
            .get(&self.p(&format!("/merge_requests/{iid}/draft_notes")))?;
        for d in drafts.as_array().into_iter().flatten() {
            self.client.send(
                crate::http::Method::Delete,
                &self.p(&format!("/merge_requests/{iid}/draft_notes/{}", d["id"])),
                None,
            )?;
        }
        Ok(())
    }

    fn submit_review(
        &self,
        item: &ItemRef,
        event: ReviewEvent,
        body: Option<&str>,
        _comments: &[DraftComment],
    ) -> Result<Review> {
        let iid = self.iid(item)?;
        self.client.post(
            &self.p(&format!("/merge_requests/{iid}/draft_notes/bulk_publish")),
            &json!({}),
        )?;
        if let Some(b) = body.filter(|b| !b.is_empty()) {
            self.client.post(
                &self.p(&format!("/merge_requests/{iid}/notes")),
                &json!({"body": b}),
            )?;
        }
        let state = match event {
            ReviewEvent::Approve => {
                self.client.post(
                    &self.p(&format!("/merge_requests/{iid}/approve")),
                    &json!({}),
                )?;
                ReviewState::Approved
            }
            // The REST API has no "request changes" state: the summary note carries it.
            ReviewEvent::RequestChanges => ReviewState::ChangesRequested,
            ReviewEvent::Comment => ReviewState::Commented,
        };
        Ok(Review {
            id: None,
            author: String::new(),
            state,
            body: body.map(str::to_owned),
            submitted_at: None,
        })
    }

    fn request_review(&self, item: &ItemRef, reviewers: &[String]) -> Result<Vec<String>> {
        let iid = self.iid(item)?;
        let ids = self.user_ids(reviewers)?;
        let v = self.client.put(
            &self.p(&format!("/merge_requests/{iid}")),
            &json!({"reviewer_ids": ids}),
        )?;
        Ok(logins(v.get("reviewers"), "username"))
    }

    fn checkout_ref(&self, pull: &Pull) -> CheckoutRef {
        let n = pull.summary.number.unwrap_or(0);
        CheckoutRef {
            url: None,
            refspec: format!("refs/merge-requests/{n}/head"),
            branch: format!("mr/{n}"),
        }
    }

    fn merge(&self, item: &ItemRef, m: &MergeRequest) -> Result<MergeOutcome> {
        let iid = self.iid(item)?;
        let mut body = json!({
            "squash": m.method == Some(MergeMethod::Squash),
            "should_remove_source_branch": m.delete_branch,
        });
        if m.when_checks_pass {
            body["merge_when_pipeline_succeeds"] = json!(true);
            body["auto_merge"] = json!(true);
        }
        if let Some(msg) = &m.message {
            body[if m.method == Some(MergeMethod::Squash) {
                "squash_commit_message"
            } else {
                "merge_commit_message"
            }] = json!(msg);
        }
        if let Some(sha) = &m.head_sha {
            body["sha"] = json!(sha);
        }
        let v = self
            .client
            .put(&self.p(&format!("/merge_requests/{iid}/merge")), &body)?;
        let merged = v["state"] == "merged";
        Ok(MergeOutcome {
            merged,
            sha: str_of(&v, "merge_commit_sha").or_else(|| str_of(&v, "squash_commit_sha")),
            auto_merge: !merged && m.when_checks_pass,
            message: if merged {
                "merged".into()
            } else {
                "set to merge when the pipeline succeeds".into()
            },
        })
    }

    fn set_pull_state(&self, item: &ItemRef, open: bool) -> Result<State> {
        let iid = self.iid(item)?;
        let v = self.client.put(
            &self.p(&format!("/merge_requests/{iid}")),
            &json!({"state_event": if open { "reopen" } else { "close" }}),
        )?;
        Ok(Self::summary(&v).map(|s| s.state).unwrap_or(State::Closed))
    }

    fn set_draft(&self, item: &ItemRef, draft: bool) -> Result<bool> {
        Ok(self
            .update_pull(
                item,
                &PullEdit {
                    draft: Some(draft),
                    ..Default::default()
                },
            )?
            .draft)
    }

    fn resolve_thread(&self, item: &ItemRef, thread: &str, resolved: bool) -> Result<bool> {
        let iid = self.iid(item)?;
        let v = self.client.put(
            &self.p(&format!(
                "/merge_requests/{iid}/discussions/{}",
                encode(thread)
            )),
            &json!({"resolved": resolved}),
        )?;
        Ok(v["notes"]
            .as_array()
            .and_then(|n| n.iter().find(|x| x["resolvable"] == true))
            .map(|x| x["resolved"] == true)
            .unwrap_or(resolved))
    }

    fn issues(&self, q: &IssueQuery) -> Result<Page<IssueSummary>> {
        let mut pairs = vec![
            (
                "state",
                match q.state {
                    IssueStateFilter::Open => "opened",
                    IssueStateFilter::Closed => "closed",
                    IssueStateFilter::All => "all",
                }
                .to_owned(),
            ),
            ("order_by", "updated_at".into()),
            ("per_page", per_page(q.max).to_string()),
            ("labels", q.labels.join(",")),
            ("search", q.text.clone().unwrap_or_default()),
        ];
        match (q.filter, &q.me) {
            (IssueFilter::Mine, Some(me)) => pairs.push(("author_username", me.clone())),
            (IssueFilter::Assigned, Some(me)) => pairs.push(("assignee_username", me.clone())),
            _ => {}
        }
        let first = self
            .client
            .url(&self.p(&format!("/issues?{}", query(&pairs))));
        collect(
            &self.client,
            first,
            q.cursor.as_deref(),
            q.max,
            Self::issue_summary,
            |i| q.matches(i),
        )
    }

    fn issue(&self, item: &ItemRef) -> Result<Issue> {
        let iid = self.iid(item)?;
        let v = self.client.get(&self.p(&format!("/issues/{iid}")))?;
        let summary =
            Self::issue_summary(&v).ok_or_else(|| ForgeError::other("GitLab answered no issue"))?;
        let notes = match self.client.get_all(
            &self.p(&format!("/issues/{iid}/notes?sort=asc&per_page=100")),
            1000,
        ) {
            Ok((n, _)) => n,
            Err(e)
                if !self.client.signed_in()
                    && matches!(e.kind, ErrorKind::SignInRequired | ErrorKind::Forbidden) =>
            {
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let mut fields = std::collections::BTreeMap::new();
        if let Some(w) = u64_of(&v, "weight") {
            fields.insert("Weight".to_owned(), w.to_string());
        }
        if let Some(d) = str_of(&v, "due_date") {
            fields.insert("Due date".to_owned(), d);
        }
        Ok(Issue {
            summary,
            body: str_of(&v, "description"),
            conversation: notes
                .iter()
                .filter(|n| n["system"] != true)
                .map(Self::note)
                .collect(),
            fields,
            branches: Vec::new(),
        })
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueSummary> {
        let mut body = json!({"title": new.title, "description": new.body});
        if !new.labels.is_empty() {
            body["labels"] = json!(new.labels.join(","));
        }
        if !new.assignees.is_empty() {
            body["assignee_ids"] = json!(self.user_ids(&new.assignees)?);
        }
        if let Some(m) = &new.milestone {
            body["milestone_id"] = self.milestone_id(m)?;
        }
        let v = self.client.post(&self.p("/issues"), &body)?;
        Self::issue_summary(&v).ok_or_else(|| ForgeError::other("GitLab answered no issue"))
    }

    fn comment_issue(&self, item: &ItemRef, body: &str) -> Result<Posted> {
        let iid = self.iid(item)?;
        let v = self.client.post(
            &self.p(&format!("/issues/{iid}/notes")),
            &json!({"body": body}),
        )?;
        Ok(Posted {
            id: v["id"].to_string(),
            thread: None,
            url: None,
        })
    }

    fn update_issue(&self, item: &ItemRef, e: &IssueEdit) -> Result<IssueSummary> {
        let iid = self.iid(item)?;
        let mut body = serde_json::Map::new();
        if let Some(s) = e.state {
            body.insert(
                "state_event".into(),
                json!(if s == IssueState::Closed {
                    "close"
                } else {
                    "reopen"
                }),
            );
        }
        if let Some(l) = &e.labels {
            body.insert("labels".into(), json!(l.join(",")));
        }
        if let Some(a) = &e.assignees {
            body.insert("assignee_ids".into(), json!(self.user_ids(a)?));
        }
        if let Some(m) = &e.milestone {
            body.insert("milestone_id".into(), self.milestone_id(m)?);
        }
        if let Some(t) = &e.title {
            body.insert("title".into(), json!(t));
        }
        if let Some(b) = &e.body {
            body.insert("description".into(), json!(b));
        }
        let v = self
            .client
            .put(&self.p(&format!("/issues/{iid}")), &Value::Object(body))?;
        Self::issue_summary(&v).ok_or_else(|| ForgeError::other("GitLab answered no issue"))
    }

    fn link_branch(&self, item: &ItemRef, branch: &str, _commit: &str) -> Result<bool> {
        // GitLab lists an issue's related branches by name (`<iid>-...`); a note records ours.
        self.comment_issue(
            item,
            &format!("Created branch `{branch}` for this issue (Eludite)."),
        )?;
        Ok(true)
    }

    fn checks(&self, commit: &str, _pull: Option<&ItemRef>) -> Result<Vec<Check>> {
        let pipelines = self
            .client
            .get(&self.p(&format!("/pipelines?sha={commit}&per_page=5")))?;
        let Some(p) = pipelines.as_array().and_then(|a| a.first()).cloned() else {
            return Ok(Vec::new());
        };
        let (s, c) = job_status(p["status"].as_str().unwrap_or(""));
        let mut out = vec![Check {
            id: format!("pipeline:{}", p["id"]),
            name: format!("pipeline #{}", p["id"]),
            kind: CheckKind::Pipeline,
            status: s,
            conclusion: c,
            url: str_of(&p, "web_url"),
            started_at: time_of(&p, "created_at"),
            duration_seconds: None,
            has_log: false,
            can_rerun: s == CheckStatus::Completed && c != Conclusion::Success,
            required: false,
        }];
        let (jobs, _) = self.client.get_all(
            &self.p(&format!("/pipelines/{}/jobs?per_page=100", p["id"])),
            300,
        )?;
        out.extend(
            jobs.iter()
                .map(|j| Self::job_check(j, j["allow_failure"] != true)),
        );
        Ok(out)
    }

    fn check_log(&self, id: &str) -> Result<LogText> {
        let job = id
            .strip_prefix("job:")
            .ok_or_else(|| self.unsupported("a pipeline's log (open one of its jobs)"))?;
        let text = self
            .client
            .get_text(&self.p(&format!("/jobs/{job}/trace")))?;
        Ok(LogText {
            text,
            url: Some(format!("{}/-/jobs/{job}", self.repo.web_url)),
        })
    }

    fn rerun(&self, id: &str) -> Result<String> {
        if let Some(job) = id.strip_prefix("job:") {
            let v = self
                .client
                .post(&self.p(&format!("/jobs/{job}/retry")), &json!({}))?;
            return Ok(format!("job {job} retried as job {}", v["id"]));
        }
        if let Some(p) = id.strip_prefix("pipeline:") {
            self.client
                .post(&self.p(&format!("/pipelines/{p}/retry")), &json!({}))?;
            return Ok(format!("pipeline {p}'s failed jobs retried"));
        }
        Err(self.unsupported("rerunning this check"))
    }
}
