//! Azure DevOps (dev.azure.com, the legacy `*.visualstudio.com` urls and Azure DevOps Server): the REST API with
//! `api-version=7.1` on every call. Pull requests with threads anchored to lines and iterations, reviewer votes
//! (approve 10, approve with suggestions 5, wait for author -5, reject -10) as the review submissions, completion with
//! the merge strategies (no fast-forward, squash, rebase, rebase and merge), auto-complete, drafts, and the branch
//! policies' evaluations reported as checks; work items are the issues (the process decides the types and the
//! states); builds with their logs and retries. No pending reviews: each comment posts at once.

use serde_json::{Value, json};

use crate::client::{Client, time_of};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::forge::{Forge, PendingMode, capabilities};
use crate::http::{Method, Request};
use crate::model::*;
use crate::util::{encode, first_line, str_at, str_of, u64_of};

/// The pinned REST API version.
pub const API_VERSION: &str = "7.1";
/// Previews some resources still need (policy evaluations, work item comments).
pub const API_PREVIEW: &str = "7.1-preview.1";
pub const COMMENTS_PREVIEW: &str = "7.1-preview.4";

/// States that close a work item, whatever the process.
const CLOSED_STATES: [&str; 6] = ["Closed", "Done", "Removed", "Resolved", "Completed", "Cut"];

pub struct AzureDevOps {
    pub repo: Repository,
    pub client: Client,
}

impl AzureDevOps {
    pub fn new(repo: Repository, client: Client) -> Self {
        let client = client.with_header("Accept", "application/json");
        Self { repo, client }
    }

    fn project(&self) -> String {
        encode(self.repo.project.as_deref().unwrap_or(&self.repo.name))
    }

    /// `{project}/_apis/git/repositories/{repo}{rest}` with the api-version.
    fn g(&self, rest: &str) -> String {
        with_version(
            &format!(
                "/{}/_apis/git/repositories/{}{rest}",
                self.project(),
                encode(&self.repo.name)
            ),
            API_VERSION,
        )
    }

    fn pid(&self, item: &ItemRef) -> Result<u64> {
        item.number().ok_or_else(|| {
            ForgeError::invalid(format!(
                "Azure DevOps pull requests and work items have ids, not `{item}`"
            ))
        })
    }

    fn web(&self) -> String {
        self.repo.web_url.clone()
    }

    fn summary(&self, v: &Value) -> Option<PullSummary> {
        let id = u64_of(v, "pullRequestId")?;
        let state = match v["status"].as_str() {
            Some("completed") => State::Merged,
            Some("abandoned") => State::Closed,
            _ => State::Open,
        };
        Some(PullSummary {
            number: Some(id),
            id: id.to_string(),
            title: str_of(v, "title").unwrap_or_default(),
            state,
            draft: v["isDraft"] == true,
            author: person(&v["createdBy"]),
            head: branch(&str_of(v, "sourceRefName").unwrap_or_default()),
            base: branch(&str_of(v, "targetRefName").unwrap_or_default()),
            head_repository: str_at(v, "/forkSource/repository/name"),
            created_at: time_of(v, "creationDate"),
            updated_at: time_of(v, "closedDate").or_else(|| time_of(v, "creationDate")),
            url: format!("{}/pullrequest/{id}", self.web()),
            labels: v["labels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| str_of(l, "name"))
                .collect(),
            comments: None,
            review_requested: false,
            checks: None,
            requested_reviewers: v["reviewers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(person)
                .collect(),
        })
    }

    /// The signed-in identity's id and unique name (`connectionData`).
    fn me(&self) -> Result<(String, Account)> {
        let v = self.client.get("/_apis/connectionData")?;
        let u = &v["authenticatedUser"];
        let id = str_of(u, "id").ok_or_else(|| {
            ForgeError::sign_in_required(&self.repo.host, "the forge sees no signed-in user.")
        })?;
        let login = str_at(u, "/properties/Account/$value")
            .or_else(|| str_of(u, "providerDisplayName"))
            .unwrap_or_default();
        if login.is_empty()
            || u["descriptor"]
                .as_str()
                .is_some_and(|d| d.contains("Anonymous"))
        {
            return Err(ForgeError::sign_in_required(
                &self.repo.host,
                "the forge sees an anonymous user.",
            ));
        }
        Ok((
            id,
            Account {
                login,
                name: str_of(u, "providerDisplayName"),
                url: None,
            },
        ))
    }

    /// Identity ids for names (unique names, display names or ids).
    fn identity_ids(&self, names: &[String]) -> Result<Vec<String>> {
        let vssps = vssps(&self.client.base);
        names
            .iter()
            .map(|n| {
                if n.len() == 36 && n.chars().filter(|c| *c == '-').count() == 4 {
                    return Ok(n.clone());
                }
                let v = self.client.get(&with_version(
                    &format!(
                        "{vssps}/_apis/identities?searchFilter=General&filterValue={}",
                        encode(n)
                    ),
                    API_VERSION,
                ))?;
                v["value"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|i| str_of(i, "id"))
                    .ok_or_else(|| ForgeError::invalid(format!("no Azure DevOps identity `{n}`")))
            })
            .collect()
    }

    fn json_patch(&self, method: Method, path: &str, ops: &Value) -> Result<Value> {
        let mut req = Request::new(method, self.client.url(path))
            .header("Content-Type", "application/json-patch+json");
        req.body = Some(serde_json::to_vec(ops).expect("serializes"));
        let r = self.client.send_raw(req)?;
        self.client.check(&r)
    }

    fn work_item_summary(&self, v: &Value) -> Option<IssueSummary> {
        let id = u64_of(v, "id")?;
        let f = &v["fields"];
        let state = str_of(f, "System.State").unwrap_or_default();
        Some(IssueSummary {
            number: Some(id),
            id: id.to_string(),
            title: str_of(f, "System.Title").unwrap_or_default(),
            state: if CLOSED_STATES.contains(&state.as_str()) {
                IssueState::Closed
            } else {
                IssueState::Open
            },
            kind: str_of(f, "System.WorkItemType"),
            author: f.get("System.CreatedBy").map(person),
            assignees: f
                .get("System.AssignedTo")
                .map(person)
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect(),
            labels: str_of(f, "System.Tags")
                .map(|t| {
                    t.split(';')
                        .map(|x| x.trim().to_owned())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            milestone: str_of(f, "System.IterationPath"),
            comments: u64_of(f, "System.CommentCount"),
            created_at: time_of(f, "System.CreatedDate"),
            updated_at: time_of(f, "System.ChangedDate"),
            url: format!(
                "{}/{}/_workitems/edit/{id}",
                web_org(&self.web()),
                self.project()
            ),
        })
    }

    /// The type's state for open (`Proposed`) or closed (`Completed`).
    fn state_for(&self, kind: &str, closed: bool) -> String {
        let want = if closed { "Completed" } else { "Proposed" };
        self.client
            .get(&with_version(
                &format!(
                    "/{}/_apis/wit/workitemtypes/{}/states",
                    self.project(),
                    encode(kind)
                ),
                API_VERSION,
            ))
            .ok()
            .and_then(|v| {
                v["value"].as_array().and_then(|a| {
                    a.iter()
                        .find(|s| s["category"] == want)
                        .and_then(|s| str_of(s, "name"))
                })
            })
            .unwrap_or_else(|| {
                if closed {
                    "Closed".into()
                } else {
                    "New".into()
                }
            })
    }

    fn build_check(&self, build: &Value, name: Option<String>, required: bool) -> Check {
        let (status, conclusion) = match (build["status"].as_str(), build["result"].as_str()) {
            (Some("completed"), Some("succeeded")) => (CheckStatus::Completed, Conclusion::Success),
            (Some("completed"), Some("partiallySucceeded")) => {
                (CheckStatus::Completed, Conclusion::Neutral)
            }
            (Some("completed"), Some("canceled")) => {
                (CheckStatus::Completed, Conclusion::Cancelled)
            }
            (Some("completed"), _) => (CheckStatus::Completed, Conclusion::Failure),
            (Some("inProgress"), _) => (CheckStatus::InProgress, Conclusion::None),
            _ => (CheckStatus::Queued, Conclusion::None),
        };
        Check {
            id: format!("build:{}", build["id"]),
            name: name.unwrap_or_else(|| str_at(build, "/definition/name").unwrap_or_default()),
            kind: CheckKind::Pipeline,
            status,
            conclusion,
            url: str_at(build, "/_links/web/href"),
            started_at: time_of(build, "startTime"),
            duration_seconds: crate::client::duration_between(
                time_of(build, "startTime").as_deref(),
                time_of(build, "finishTime").as_deref(),
            ),
            has_log: status != CheckStatus::Queued,
            can_rerun: status == CheckStatus::Completed,
            required,
        }
    }
}

/// `path` with `api-version` appended.
fn with_version(path: &str, version: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}api-version={version}")
}

/// The identities service's base: `vssps.dev.azure.com/{org}` for dev.azure.com, the collection itself on Server.
fn vssps(base: &str) -> String {
    base.replacen("://dev.azure.com/", "://vssps.dev.azure.com/", 1)
}

fn web_org(web: &str) -> String {
    match web.find("/_git/") {
        Some(i) => {
            let before = &web[..i];
            before
                .rsplit_once('/')
                .map(|(org, _)| org.to_owned())
                .unwrap_or_else(|| before.to_owned())
        }
        None => web.to_owned(),
    }
}

/// `refs/heads/x` → `x`.
fn branch(r: &str) -> String {
    r.strip_prefix("refs/heads/").unwrap_or(r).to_owned()
}

/// A person: the unique name, else the display name.
fn person(v: &Value) -> String {
    str_of(v, "uniqueName")
        .filter(|s| !s.is_empty())
        .or_else(|| str_of(v, "displayName"))
        .unwrap_or_default()
}

/// Azure DevOps' HTML (work item descriptions and comments) as text.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let t = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    t.as_str(),
                    "br" | "br/" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3"
                ) && !out.ends_with('\n')
                {
                    out.push('\n');
                }
            }
            c if in_tag => tag.push(c),
            c => out.push(c),
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

impl Forge for AzureDevOps {
    fn repository(&self) -> &Repository {
        &self.repo
    }

    fn capabilities(&self) -> Capabilities {
        capabilities(Family::AzureDevOps)
    }

    fn pending_mode(&self) -> PendingMode {
        PendingMode::None
    }

    fn account(&self) -> Result<Account> {
        self.me().map(|(_, a)| a)
    }

    fn pulls(&self, q: &PullQuery) -> Result<Page<PullSummary>> {
        let status = match q.state {
            StateFilter::Open => "active",
            StateFilter::Closed => "abandoned",
            StateFilter::Merged => "completed",
            StateFilter::All => "all",
        };
        let mut skip: usize = q
            .cursor
            .as_deref()
            .and_then(|c| c.strip_prefix("skip:"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let top = q.max.clamp(1, 100);
        let mut items = Vec::new();
        let mut more = true;
        for _ in 0..crate::common::MAX_PAGES {
            self.client.cancel.check()?;
            let v = self.client.get(&self.g(&format!(
                "/pullrequests?searchCriteria.status={status}&$top={top}&$skip={skip}"
            )))?;
            let page: Vec<Value> = v["value"].as_array().cloned().unwrap_or_default();
            skip += page.len();
            more = page.len() == top;
            for p in &page {
                if let Some(mut s) = self.summary(p)
                    && q.matches(&s)
                {
                    s.review_requested = q.me.as_deref().is_some_and(|m| {
                        s.requested_reviewers
                            .iter()
                            .any(|r| r.eq_ignore_ascii_case(m))
                    });
                    items.push(s);
                }
            }
            if items.len() >= q.max || !more {
                break;
            }
        }
        items.truncate(q.max);
        Ok(Page {
            items,
            next_cursor: more.then(|| format!("skip:{skip}")),
        })
    }

    fn pull(&self, item: &ItemRef) -> Result<Pull> {
        let id = self.pid(item)?;
        let v = self.client.get(&self.g(&format!("/pullrequests/{id}")))?;
        let summary = self
            .summary(&v)
            .ok_or_else(|| ForgeError::other("Azure DevOps answered no pull request"))?;
        let head_sha = str_at(&v, "/lastMergeSourceCommit/commitId");
        let commits = self
            .client
            .get(&self.g(&format!("/pullrequests/{id}/commits?$top=250")))?;
        let commits = commits["value"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| Commit {
                sha: str_of(c, "commitId").unwrap_or_default(),
                title: first_line(&str_of(c, "comment").unwrap_or_default()),
                author: str_at(c, "/author/name"),
                date: time_of(&c["author"], "date"),
            })
            .collect();
        let iterations = self
            .client
            .get(&self.g(&format!("/pullrequests/{id}/iterations")))
            .ok();
        let last = iterations.as_ref().and_then(|i| {
            i["value"]
                .as_array()
                .and_then(|a| a.last())
                .and_then(|x| u64_of(x, "id"))
        });
        let files = match last {
            Some(it) => {
                let ch = self.client.get(&self.g(&format!(
                    "/pullrequests/{id}/iterations/{it}/changes?$compareTo=0&$top=2000"
                )))?;
                ch["changeEntries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| str_at(c, "/item/gitObjectType").as_deref() != Some("tree"))
                    .map(|c| {
                        let kind = c["changeType"].as_str().unwrap_or("edit");
                        FileChange {
                            path: str_at(c, "/item/path")
                                .unwrap_or_default()
                                .trim_start_matches('/')
                                .to_owned(),
                            old_path: str_of(c, "originalPath")
                                .map(|p| p.trim_start_matches('/').to_owned()),
                            status: if kind.contains("add") {
                                FileStatus::Added
                            } else if kind.contains("delete") {
                                FileStatus::Deleted
                            } else if kind.contains("rename") {
                                FileStatus::Renamed
                            } else {
                                FileStatus::Modified
                            },
                            // The API gives no line counts per file.
                            additions: 0,
                            deletions: 0,
                            binary: false,
                        }
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let threads_v = self
            .client
            .get(&self.g(&format!("/pullrequests/{id}/threads")))?;
        let mut threads = Vec::new();
        let mut conversation = Vec::new();
        for t in threads_v["value"].as_array().into_iter().flatten() {
            if t["isDeleted"] == true {
                continue;
            }
            let comments: Vec<Comment> = t["comments"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["commentType"] != "system" && c["isDeleted"] != true)
                .map(|c| Comment {
                    id: c["id"].to_string(),
                    author: person(&c["author"]),
                    body: str_of(c, "content").unwrap_or_default(),
                    created_at: time_of(c, "publishedDate"),
                    url: None,
                    pending: false,
                })
                .collect();
            if comments.is_empty() {
                continue;
            }
            let ctx = t.get("threadContext").filter(|c| !c.is_null());
            let status = t["status"].as_str();
            match ctx {
                Some(c) => {
                    let (line, side) = match (
                        u64_of(&c["rightFileStart"], "line"),
                        u64_of(&c["leftFileStart"], "line"),
                    ) {
                        (Some(l), _) => (Some(l), Some(Side::Right)),
                        (None, Some(l)) => (Some(l), Some(Side::Left)),
                        _ => (None, None),
                    };
                    threads.push(Thread {
                        id: t["id"].to_string(),
                        path: str_of(c, "filePath").map(|p| p.trim_start_matches('/').to_owned()),
                        line,
                        start_line: None,
                        side,
                        resolved: matches!(
                            status,
                            Some("fixed" | "closed" | "wontFix" | "byDesign")
                        ),
                        outdated: false,
                        comments,
                    });
                }
                None if status.is_some() && status != Some("unknown") && comments.len() > 1 => {
                    threads.push(Thread {
                        id: t["id"].to_string(),
                        resolved: matches!(
                            status,
                            Some("fixed" | "closed" | "wontFix" | "byDesign")
                        ),
                        comments,
                        ..Default::default()
                    })
                }
                None => conversation.extend(comments),
            }
        }
        let reviews = v["reviewers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| Review {
                id: str_of(r, "id"),
                author: person(r),
                state: match r["vote"].as_i64().unwrap_or(0) {
                    10 => ReviewState::Approved,
                    5 => ReviewState::ApprovedWithSuggestions,
                    -5 => ReviewState::WaitingForAuthor,
                    -10 => ReviewState::Rejected,
                    _ => ReviewState::Requested,
                },
                body: None,
                submitted_at: None,
            })
            .collect();
        let check_items = match &head_sha {
            Some(sha) => self.checks(sha, Some(item)).unwrap_or_default(),
            None => Vec::new(),
        };
        let state = match v["mergeStatus"].as_str() {
            Some("succeeded") => MergeState::Clean,
            Some("conflicts") => MergeState::Conflicts,
            Some("rejectedByPolicy") => MergeState::Blocked,
            _ => MergeState::Unknown,
        };
        let mut pull = Pull {
            summary,
            body: str_of(&v, "description"),
            head_sha,
            base_sha: str_at(&v, "/lastMergeTargetCommit/commitId"),
            head_clone_url: str_at(&v, "/forkSource/repository/remoteUrl"),
            commits,
            files,
            threads,
            conversation,
            reviews,
            check_items,
            mergeable: Some(Mergeable {
                state,
                methods: vec![
                    MergeMethod::Merge,
                    MergeMethod::Squash,
                    MergeMethod::Rebase,
                    MergeMethod::RebaseMerge,
                ],
                reason: str_of(&v, "mergeStatus").map(|s| format!("Azure DevOps: mergeStatus {s}")),
            }),
            auto_merge: v.get("autoCompleteSetBy").is_some_and(|a| !a.is_null()),
            iterations: iterations.as_ref().and_then(|i| u64_of(i, "count")),
            ..Default::default()
        };
        pull.finish();
        Ok(pull)
    }

    fn create_pull(&self, new: &NewPull) -> Result<PullSummary> {
        let mut body = json!({
            "sourceRefName": format!("refs/heads/{}", new.head),
            "targetRefName": format!("refs/heads/{}", new.base),
            "title": new.title, "description": new.body, "isDraft": new.draft,
        });
        if !new.reviewers.is_empty() {
            body["reviewers"] = json!(
                self.identity_ids(&new.reviewers)?
                    .into_iter()
                    .map(|id| json!({"id": id}))
                    .collect::<Vec<_>>()
            );
        }
        if !new.labels.is_empty() {
            body["labels"] = json!(
                new.labels
                    .iter()
                    .map(|l| json!({"name": l}))
                    .collect::<Vec<_>>()
            );
        }
        let v = self.client.post(&self.g("/pullrequests"), &body)?;
        self.summary(&v)
            .ok_or_else(|| ForgeError::other("Azure DevOps answered no pull request"))
    }

    fn update_pull(&self, item: &ItemRef, edit: &PullEdit) -> Result<PullSummary> {
        let id = self.pid(item)?;
        let mut body = serde_json::Map::new();
        if let Some(t) = &edit.title {
            body.insert("title".into(), json!(t));
        }
        if let Some(b) = &edit.body {
            body.insert("description".into(), json!(b));
        }
        if let Some(d) = edit.draft {
            body.insert("isDraft".into(), json!(d));
        }
        if let Some(b) = &edit.base {
            body.insert("targetRefName".into(), json!(format!("refs/heads/{b}")));
        }
        let v = self.client.patch(
            &self.g(&format!("/pullrequests/{id}")),
            &Value::Object(body),
        )?;
        self.summary(&v)
            .ok_or_else(|| ForgeError::other("Azure DevOps answered no pull request"))
    }

    fn comment_pull(&self, item: &ItemRef, c: &DraftComment) -> Result<Posted> {
        let id = self.pid(item)?;
        if let Some(t) = &c.reply_to {
            let v = self.client.post(
                &self.g(&format!(
                    "/pullrequests/{id}/threads/{}/comments",
                    encode(t)
                )),
                &json!({"content": c.body, "parentCommentId": 1, "commentType": 1}),
            )?;
            return Ok(Posted {
                id: v["id"].to_string(),
                thread: Some(t.clone()),
                url: None,
            });
        }
        let mut body = json!({
            "comments": [{"parentCommentId": 0, "content": c.body, "commentType": 1}],
            "status": "active",
        });
        if let (Some(path), Some(line)) = (&c.path, c.line) {
            let pos = json!({"line": line, "offset": 1});
            let (start, end) = match c.side {
                Side::Right => ("rightFileStart", "rightFileEnd"),
                Side::Left => ("leftFileStart", "leftFileEnd"),
            };
            body["threadContext"] = json!({"filePath": format!("/{}", path.trim_start_matches('/')), start: pos, end: pos});
        }
        let v = self
            .client
            .post(&self.g(&format!("/pullrequests/{id}/threads")), &body)?;
        Ok(Posted {
            id: v
                .pointer("/comments/0/id")
                .map(Value::to_string)
                .unwrap_or_default(),
            thread: c.path.as_ref().map(|_| v["id"].to_string()),
            url: None,
        })
    }

    fn submit_review(
        &self,
        item: &ItemRef,
        event: ReviewEvent,
        body: Option<&str>,
        _comments: &[DraftComment],
    ) -> Result<Review> {
        let id = self.pid(item)?;
        if let Some(b) = body.filter(|b| !b.is_empty()) {
            self.comment_pull(
                item,
                &DraftComment {
                    body: b.to_owned(),
                    ..Default::default()
                },
            )?;
        }
        let (vote, state) = match event {
            ReviewEvent::Approve => (10, ReviewState::Approved),
            ReviewEvent::RequestChanges => (-5, ReviewState::WaitingForAuthor),
            ReviewEvent::Comment => {
                return Ok(Review {
                    id: None,
                    author: String::new(),
                    state: ReviewState::Commented,
                    body: body.map(str::to_owned),
                    submitted_at: None,
                });
            }
        };
        let (me, account) = self.me()?;
        self.client.put(
            &self.g(&format!("/pullrequests/{id}/reviewers/{me}")),
            &json!({"vote": vote}),
        )?;
        Ok(Review {
            id: Some(me),
            author: account.login,
            state,
            body: body.map(str::to_owned),
            submitted_at: None,
        })
    }

    fn request_review(&self, item: &ItemRef, reviewers: &[String]) -> Result<Vec<String>> {
        let id = self.pid(item)?;
        for rid in self.identity_ids(reviewers)? {
            self.client.put(
                &self.g(&format!("/pullrequests/{id}/reviewers/{rid}")),
                &json!({"vote": 0}),
            )?;
        }
        Ok(reviewers.to_vec())
    }

    fn checkout_ref(&self, pull: &Pull) -> CheckoutRef {
        let n = pull.summary.number.unwrap_or(0);
        CheckoutRef {
            url: pull.head_clone_url.clone(),
            refspec: format!("refs/heads/{}", pull.summary.head),
            branch: format!("pr/{n}"),
        }
    }

    fn merge(&self, item: &ItemRef, m: &MergeRequest) -> Result<MergeOutcome> {
        let id = self.pid(item)?;
        let strategy = match m.method.unwrap_or(MergeMethod::Merge) {
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
            MergeMethod::RebaseMerge => "rebaseMerge",
            _ => "noFastForward",
        };
        let mut options = json!({"mergeStrategy": strategy, "deleteSourceBranch": m.delete_branch});
        if let Some(msg) = m.message.as_ref().or(m.title.as_ref()) {
            options["mergeCommitMessage"] = json!(msg);
        }
        let head = match &m.head_sha {
            Some(h) => h.clone(),
            None => str_at(
                &self.client.get(&self.g(&format!("/pullrequests/{id}")))?,
                "/lastMergeSourceCommit/commitId",
            )
            .unwrap_or_default(),
        };
        if m.when_checks_pass {
            let (me, _) = self.me()?;
            self.client.patch(
                &self.g(&format!("/pullrequests/{id}")),
                &json!({"autoCompleteSetBy": {"id": me}, "completionOptions": options}),
            )?;
            return Ok(MergeOutcome {
                merged: false,
                sha: None,
                auto_merge: true,
                message: "auto-complete set: Azure DevOps completes it when its policies pass"
                    .into(),
            });
        }
        let v = self.client.patch(
            &self.g(&format!("/pullrequests/{id}")),
            &json!({"status": "completed", "lastMergeSourceCommit": {"commitId": head}, "completionOptions": options}),
        )?;
        Ok(MergeOutcome {
            merged: v["status"] == "completed",
            sha: str_at(&v, "/lastMergeCommit/commitId"),
            auto_merge: false,
            message: format!("completed ({strategy})"),
        })
    }

    fn set_pull_state(&self, item: &ItemRef, open: bool) -> Result<State> {
        let id = self.pid(item)?;
        let v = self.client.patch(
            &self.g(&format!("/pullrequests/{id}")),
            &json!({"status": if open { "active" } else { "abandoned" }}),
        )?;
        Ok(self.summary(&v).map(|s| s.state).unwrap_or(State::Closed))
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
        let id = self.pid(item)?;
        let v = self.client.patch(
            &self.g(&format!("/pullrequests/{id}/threads/{}", encode(thread))),
            &json!({"status": if resolved { "fixed" } else { "active" }}),
        )?;
        Ok(matches!(
            v["status"].as_str(),
            Some("fixed" | "closed" | "wontFix" | "byDesign")
        ))
    }

    fn issues(&self, q: &IssueQuery) -> Result<Page<IssueSummary>> {
        let closed = CLOSED_STATES
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(",");
        let mut wiql =
            "Select [System.Id] From WorkItems Where [System.TeamProject] = @project".to_owned();
        match q.state {
            IssueStateFilter::Open => {
                wiql.push_str(&format!(" And [System.State] Not In ({closed})"))
            }
            IssueStateFilter::Closed => {
                wiql.push_str(&format!(" And [System.State] In ({closed})"))
            }
            IssueStateFilter::All => {}
        }
        match q.filter {
            IssueFilter::Mine => wiql.push_str(" And [System.CreatedBy] = @Me"),
            IssueFilter::Assigned => wiql.push_str(" And [System.AssignedTo] = @Me"),
            IssueFilter::All => {}
        }
        for l in &q.labels {
            wiql.push_str(&format!(
                " And [System.Tags] Contains '{}'",
                l.replace('\'', "''")
            ));
        }
        if let Some(t) = &q.text {
            wiql.push_str(&format!(
                " And [System.Title] Contains '{}'",
                t.replace('\'', "''")
            ));
        }
        wiql.push_str(" Order By [System.ChangedDate] Desc");
        let skip: usize = q
            .cursor
            .as_deref()
            .and_then(|c| c.strip_prefix("skip:"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let v = self.client.post(
            &with_version(
                &format!(
                    "/{}/_apis/wit/wiql?$top={}",
                    self.project(),
                    skip + q.max + 1
                ),
                API_VERSION,
            ),
            &json!({"query": wiql}),
        )?;
        let ids: Vec<u64> = v["workItems"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| u64_of(w, "id"))
            .collect();
        let page: Vec<u64> = ids
            .iter()
            .skip(skip)
            .take(q.max.min(200))
            .copied()
            .collect();
        let more = ids.len() > skip + page.len();
        if page.is_empty() {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let list = page
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let items = self.client.get(&with_version(
            &format!(
                "/{}/_apis/wit/workitems?ids={list}&fields=System.Title,System.State,System.WorkItemType,System.AssignedTo,System.CreatedBy,System.Tags,System.IterationPath,System.CreatedDate,System.ChangedDate,System.CommentCount",
                self.project()
            ),
            API_VERSION,
        ))?;
        let items = items["value"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|w| self.work_item_summary(w))
            .collect();
        Ok(Page {
            items,
            next_cursor: more.then(|| format!("skip:{}", skip + page.len())),
        })
    }

    fn issue(&self, item: &ItemRef) -> Result<Issue> {
        let id = self.pid(item)?;
        let v = self.client.get(&with_version(
            &format!(
                "/{}/_apis/wit/workitems/{id}?$expand=relations",
                self.project()
            ),
            API_VERSION,
        ))?;
        let summary = self
            .work_item_summary(&v)
            .ok_or_else(|| ForgeError::other("Azure DevOps answered no work item"))?;
        let comments = self
            .client
            .get(&with_version(
                &format!("/{}/_apis/wit/workItems/{id}/comments", self.project()),
                COMMENTS_PREVIEW,
            ))
            .unwrap_or(Value::Null);
        let mut fields = std::collections::BTreeMap::new();
        let skip = [
            "System.Title",
            "System.State",
            "System.WorkItemType",
            "System.Description",
            "System.Tags",
            "System.AssignedTo",
            "System.CreatedBy",
            "System.ChangedBy",
            "System.IterationPath",
            "System.CreatedDate",
            "System.ChangedDate",
            "System.CommentCount",
            "System.Id",
            "System.Rev",
            "System.TeamProject",
            "System.Watermark",
            "System.AuthorizedAs",
            "System.PersonId",
            "System.AuthorizedDate",
            "System.RevisedDate",
        ];
        if let Some(f) = v["fields"].as_object() {
            for (k, x) in f {
                if skip.contains(&k.as_str()) {
                    continue;
                }
                let text = match x {
                    Value::String(s) => html_to_text(s),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    o @ Value::Object(_) => person(o),
                    _ => continue,
                };
                if !text.is_empty() {
                    let name = k.rsplit('.').next().unwrap_or(k).to_owned();
                    fields.insert(name, text);
                }
            }
        }
        let branches = v["relations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| str_of(r, "url"))
            .filter(|u| u.starts_with("vstfs:///Git/Ref/"))
            .filter_map(|u| {
                crate::util::decode(&u)
                    .rsplit_once("/GB")
                    .map(|(_, b)| b.to_owned())
            })
            .collect();
        Ok(Issue {
            summary,
            body: str_at(&v, "/fields/System.Description").map(|d| html_to_text(&d)),
            conversation: comments["comments"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| Comment {
                    id: c["id"].to_string(),
                    author: person(&c["createdBy"]),
                    body: html_to_text(&str_of(c, "text").unwrap_or_default()),
                    created_at: time_of(c, "createdDate"),
                    url: None,
                    pending: false,
                })
                .collect(),
            fields,
            branches,
        })
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueSummary> {
        let kind = match &new.kind {
            Some(k) => k.clone(),
            None => {
                let types = self
                    .client
                    .get(&with_version(
                        &format!("/{}/_apis/wit/workitemtypes", self.project()),
                        API_VERSION,
                    ))
                    .unwrap_or(Value::Null);
                let names: Vec<String> = types["value"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| str_of(t, "name"))
                    .collect();
                ["Issue", "Bug", "Task"]
                    .iter()
                    .find(|n| names.iter().any(|x| x == *n))
                    .map(|s| (*s).to_owned())
                    .unwrap_or_else(|| "Issue".into())
            }
        };
        let mut ops = vec![
            json!({"op": "add", "path": "/fields/System.Title", "value": new.title}),
            json!({"op": "add", "path": "/fields/System.Description", "value": new.body}),
        ];
        if !new.labels.is_empty() {
            ops.push(
                json!({"op": "add", "path": "/fields/System.Tags", "value": new.labels.join("; ")}),
            );
        }
        if let Some(a) = new.assignees.first() {
            ops.push(json!({"op": "add", "path": "/fields/System.AssignedTo", "value": a}));
        }
        if let Some(m) = &new.milestone {
            ops.push(json!({"op": "add", "path": "/fields/System.IterationPath", "value": m}));
        }
        let v = self.json_patch(
            Method::Post,
            &with_version(
                &format!("/{}/_apis/wit/workitems/${}", self.project(), encode(&kind)),
                API_VERSION,
            ),
            &Value::Array(ops),
        )?;
        self.work_item_summary(&v)
            .ok_or_else(|| ForgeError::other("Azure DevOps answered no work item"))
    }

    fn comment_issue(&self, item: &ItemRef, body: &str) -> Result<Posted> {
        let id = self.pid(item)?;
        let v = self.client.post(
            &with_version(
                &format!("/{}/_apis/wit/workItems/{id}/comments", self.project()),
                COMMENTS_PREVIEW,
            ),
            &json!({"text": body}),
        )?;
        Ok(Posted {
            id: v["id"].to_string(),
            thread: None,
            url: None,
        })
    }

    fn update_issue(&self, item: &ItemRef, e: &IssueEdit) -> Result<IssueSummary> {
        let id = self.pid(item)?;
        let mut ops = Vec::new();
        if let Some(s) = e.state {
            let current = self.client.get(&with_version(
                &format!("/{}/_apis/wit/workitems/{id}", self.project()),
                API_VERSION,
            ))?;
            let kind =
                str_at(&current, "/fields/System.WorkItemType").unwrap_or_else(|| "Issue".into());
            ops.push(json!({"op": "add", "path": "/fields/System.State", "value": self.state_for(&kind, s == IssueState::Closed)}));
        }
        if let Some(l) = &e.labels {
            ops.push(json!({"op": "add", "path": "/fields/System.Tags", "value": l.join("; ")}));
        }
        if let Some(a) = &e.assignees {
            ops.push(json!({"op": "add", "path": "/fields/System.AssignedTo", "value": a.first().cloned().unwrap_or_default()}));
        }
        if let Some(m) = &e.milestone {
            ops.push(json!({"op": "add", "path": "/fields/System.IterationPath", "value": m}));
        }
        if let Some(t) = &e.title {
            ops.push(json!({"op": "add", "path": "/fields/System.Title", "value": t}));
        }
        if let Some(b) = &e.body {
            ops.push(json!({"op": "add", "path": "/fields/System.Description", "value": b}));
        }
        let v = self.json_patch(
            Method::Patch,
            &with_version(
                &format!("/{}/_apis/wit/workitems/{id}", self.project()),
                API_VERSION,
            ),
            &Value::Array(ops),
        )?;
        self.work_item_summary(&v)
            .ok_or_else(|| ForgeError::other("Azure DevOps answered no work item"))
    }

    fn link_branch(&self, item: &ItemRef, branch: &str, _commit: &str) -> Result<bool> {
        let id = self.pid(item)?;
        let repo = self.client.get(&self.g(""))?;
        let (project_id, repo_id) = (
            str_at(&repo, "/project/id").unwrap_or_default(),
            str_of(&repo, "id").unwrap_or_default(),
        );
        let url = format!(
            "vstfs:///Git/Ref/{project_id}%2F{repo_id}%2FGB{}",
            encode(branch)
        );
        self.json_patch(
            Method::Patch,
            &with_version(&format!("/{}/_apis/wit/workitems/{id}", self.project()), API_VERSION),
            &json!([{"op": "add", "path": "/relations/-", "value": {"rel": "ArtifactLink", "url": url, "attributes": {"name": "Branch"}}}]),
        )?;
        Ok(true)
    }

    fn checks(&self, commit: &str, pull: Option<&ItemRef>) -> Result<Vec<Check>> {
        let mut out = Vec::new();
        if let Ok(st) = self
            .client
            .get(&self.g(&format!("/commits/{commit}/statuses?latestOnly=true")))
        {
            for s in st["value"].as_array().into_iter().flatten() {
                let (status, conclusion) = match s["state"].as_str() {
                    Some("succeeded") => (CheckStatus::Completed, Conclusion::Success),
                    Some("failed") | Some("error") => (CheckStatus::Completed, Conclusion::Failure),
                    Some("notApplicable") => (CheckStatus::Completed, Conclusion::Skipped),
                    _ => (CheckStatus::InProgress, Conclusion::None),
                };
                out.push(Check {
                    id: format!("status:{}", s["id"]),
                    name: format!(
                        "{}/{}",
                        str_at(s, "/context/genre").unwrap_or_default(),
                        str_at(s, "/context/name").unwrap_or_default()
                    )
                    .trim_start_matches('/')
                    .to_owned(),
                    kind: CheckKind::Status,
                    status,
                    conclusion,
                    url: str_of(s, "targetUrl"),
                    started_at: time_of(s, "creationDate"),
                    duration_seconds: None,
                    has_log: false,
                    can_rerun: false,
                    required: false,
                });
            }
        }
        if let Some(item) = pull {
            let id = self.pid(item)?;
            let pr = self.client.get(&self.g(&format!("/pullrequests/{id}")))?;
            let project_id = str_at(&pr, "/repository/project/id").unwrap_or_default();
            let artifact = format!("vstfs:///CodeReview/CodeReviewId/{project_id}/{id}");
            let ev = self.client.get(&with_version(
                &format!(
                    "/{}/_apis/policy/evaluations?artifactId={}",
                    self.project(),
                    encode(&artifact)
                ),
                API_PREVIEW,
            ));
            match ev {
                Ok(ev) => {
                    for e in ev["value"].as_array().into_iter().flatten() {
                        let name = str_at(e, "/configuration/settings/displayName")
                            .filter(|s| !s.is_empty())
                            .or_else(|| str_at(e, "/configuration/type/displayName"))
                            .unwrap_or_default();
                        let required = e
                            .pointer("/configuration/isBlocking")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if let Some(build) = e.pointer("/context/buildId").and_then(Value::as_u64)
                            && let Ok(b) = self.client.get(&with_version(
                                &format!("/{}/_apis/build/builds/{build}", self.project()),
                                API_VERSION,
                            ))
                        {
                            out.push(self.build_check(&b, Some(name), required));
                            continue;
                        }
                        let (status, conclusion) = match e["status"].as_str() {
                            Some("approved") => (CheckStatus::Completed, Conclusion::Success),
                            Some("rejected") | Some("broken") => {
                                (CheckStatus::Completed, Conclusion::Failure)
                            }
                            Some("notApplicable") => (CheckStatus::Completed, Conclusion::Skipped),
                            Some("running") => (CheckStatus::InProgress, Conclusion::None),
                            _ => (CheckStatus::Queued, Conclusion::None),
                        };
                        out.push(Check {
                            id: format!("policy:{}", str_of(e, "evaluationId").unwrap_or_default()),
                            name,
                            kind: CheckKind::Policy,
                            status,
                            conclusion,
                            url: Some(format!("{}/pullrequest/{id}", self.web())),
                            started_at: time_of(e, "startedDate"),
                            duration_seconds: None,
                            has_log: false,
                            can_rerun: status == CheckStatus::Completed,
                            required,
                        });
                    }
                }
                Err(e) if e.kind == ErrorKind::SignInRequired && !self.client.signed_in() => {}
                Err(e) if e.kind == ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    fn check_log(&self, id: &str) -> Result<LogText> {
        let build = id
            .strip_prefix("build:")
            .ok_or_else(|| self.unsupported("logs of this check (open its page)"))?;
        let logs = self.client.get(&with_version(
            &format!("/{}/_apis/build/builds/{build}/logs", self.project()),
            API_VERSION,
        ))?;
        let mut text = String::new();
        for l in logs["value"].as_array().into_iter().flatten() {
            let Some(lid) = u64_of(l, "id") else { continue };
            let piece = self.client.get_text(&with_version(
                &format!("/{}/_apis/build/builds/{build}/logs/{lid}", self.project()),
                API_VERSION,
            ))?;
            text.push_str(&format!("===== log {lid} =====\n"));
            text.push_str(&piece);
            if !piece.ends_with('\n') {
                text.push('\n');
            }
            if text.len() > crate::ops::LOG_CAP {
                break;
            }
        }
        Ok(LogText {
            text,
            url: Some(format!(
                "{}/{}/_build/results?buildId={build}",
                web_org(&self.web()),
                self.project()
            )),
        })
    }

    fn rerun(&self, id: &str) -> Result<String> {
        if let Some(build) = id.strip_prefix("build:") {
            self.client.patch(
                &with_version(
                    &format!("/{}/_apis/build/builds/{build}?retry=true", self.project()),
                    API_VERSION,
                ),
                &json!({}),
            )?;
            return Ok(format!("build {build}'s failed jobs queued again"));
        }
        if let Some(ev) = id.strip_prefix("policy:") {
            self.client.patch(
                &with_version(
                    &format!("/{}/_apis/policy/evaluations/{ev}", self.project()),
                    API_PREVIEW,
                ),
                &json!({}),
            )?;
            return Ok("policy evaluation queued again".into());
        }
        Err(self.unsupported("rerunning this check"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_descriptions_become_text() {
        assert_eq!(
            html_to_text("<div>Steps:</div><ol><li>Open &amp; build</li><li>Run</li></ol>"),
            "Steps:\nOpen & build\nRun"
        );
        assert_eq!(
            web_org("https://dev.azure.com/contoso/Fabrikam/_git/web"),
            "https://dev.azure.com/contoso"
        );
        assert_eq!(
            vssps("https://dev.azure.com/contoso"),
            "https://vssps.dev.azure.com/contoso"
        );
    }
}
