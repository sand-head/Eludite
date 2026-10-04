//! The `eludite.forge.*` commands' behavior, independent of the shell: each takes the command's input (already
//! checked against its schema by `eludite-commands`) and answers its output JSON, through the [`Hub`] and a
//! [`GitSide`] for what needs the repository. The shell registers these on the command bus; the agents and the
//! windows call the same commands.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth;
use crate::credentials::{Secret, SignInMethod};
use crate::error::{ErrorKind, ForgeError, Result};
use crate::forge::{Forge, PendingMode, capabilities};
use crate::http::Cancel;
use crate::hub::{Answer, Hub};
use crate::model::*;

/// The largest log piece one call answers, and what the Checks log document shows (2 MB).
pub const LOG_CAP: usize = 2 * 1024 * 1024;
/// A log piece's default size.
pub const LOG_DEFAULT: usize = 256 * 1024;
/// List page sizes.
pub const DEFAULT_MAX: usize = 50;
pub const MAX_MAX: usize = 200;

/// What the commands need of the workspace repository (the shell answers through `eludite-git`).
pub trait GitSide: Send + Sync {
    /// The remote's name and url: `remote`, else `origin`, else the only remote.
    fn remote(&self, remote: Option<&str>) -> Option<(String, String)>;
    /// The checked-out branch.
    fn current_branch(&self) -> Option<String>;
    /// A revision's full commit id (`HEAD` when `None`).
    fn resolve(&self, revision: Option<&str>) -> Option<String>;
    /// The commits on `head` that `base` lacks, newest first: (id, message).
    fn commits_between(&self, base: &str, head: &str) -> Vec<(String, String)>;
    /// The remote's default branch (`origin/HEAD`), if known.
    fn default_branch(&self, remote: &str) -> Option<String>;
    /// Fetch `refspec` from `url` (or the remote) into local `branch` and check it out through the dirty-tree rule;
    /// answers the commit and the status generation.
    fn fetch_and_checkout(
        &self,
        remote: &str,
        url: Option<&str>,
        refspec: &str,
        branch: &str,
    ) -> std::result::Result<(String, u64), String>;
    /// Create `name` at `base` (default HEAD), checked out when `checkout`; answers the commit.
    fn create_branch(
        &self,
        name: &str,
        base: Option<&str>,
        checkout: bool,
    ) -> std::result::Result<String, String>;
}

/// The repository a command is about: the workspace remote's forge.
pub fn repository(hub: &Hub, git: &dyn GitSide, input: &Value) -> Result<Repository> {
    let remote = input.get("remote").and_then(Value::as_str);
    let (name, url) = git.remote(remote).ok_or_else(|| {
        ForgeError::new(
            ErrorKind::NotFound,
            match remote {
                Some(r) => format!("no git remote named `{r}` in this repository"),
                None => "this repository has no git remote: push it to a forge first".to_owned(),
            },
        )
    })?;
    let mut repo = hub.detect(&url, true);
    repo.remote = Some(name);
    Ok(repo)
}

fn need_forge(repo: &Repository) -> Result<()> {
    if repo.family == Family::None {
        return Err(ForgeError::new(
            ErrorKind::Unsupported,
            format!(
                "No supported forge for {}",
                repo.remote_url
                    .as_deref()
                    .or(repo.remote.as_deref())
                    .unwrap_or("this repository's remote")
            ),
        ));
    }
    Ok(())
}

fn item_of(input: &Value) -> Result<ItemRef> {
    if let Some(n) = input.get("number").and_then(Value::as_u64) {
        return Ok(ItemRef::Number(n));
    }
    if let Some(s) = input.get("id").and_then(Value::as_str) {
        return Ok(ItemRef::Id(s.to_owned()));
    }
    Err(ForgeError::invalid(
        "give `number` (or `id`, as a list answered it)",
    ))
}

fn s(input: &Value, k: &str) -> Option<String> {
    input.get(k).and_then(Value::as_str).map(str::to_owned)
}

fn b(input: &Value, k: &str) -> Option<bool> {
    input.get(k).and_then(Value::as_bool)
}

fn list(input: &Value, k: &str) -> Option<Vec<String>> {
    input.get(k).and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    })
}

fn max_of(input: &Value) -> usize {
    input
        .get("max")
        .and_then(Value::as_u64)
        .map(|m| (m as usize).clamp(1, MAX_MAX))
        .unwrap_or(DEFAULT_MAX)
}

fn to_value<T: Serialize>(t: &T) -> Value {
    serde_json::to_value(t).expect("serializes")
}

/// Run `f` with the forge, refreshing an OAuth token once when the forge refused the stored one.
fn with_forge<T>(
    hub: &Hub,
    repo: &Repository,
    write: bool,
    f: impl Fn(&dyn Forge) -> Result<T>,
) -> Result<T> {
    let forge = if write {
        hub.signed_in_forge(repo, Cancel::new())?
    } else {
        hub.forge(repo, Cancel::new())?
    };
    match f(&*forge) {
        Err(e) if e.kind == ErrorKind::SignInRequired && refresh_token(hub, repo) => {
            let forge = hub.forge(repo, Cancel::new())?;
            f(&*forge)
        }
        other => other,
    }
}

fn refresh_token(hub: &Hub, repo: &Repository) -> bool {
    let Some((cred, _)) = hub.credential(&repo.host) else {
        return false;
    };
    match auth::refresh(&*hub.transport(), repo, &hub.config(), &cred) {
        Some(new) => hub.credentials().set(&repo.host, &new, true).is_ok(),
        None => false,
    }
}

fn me(hub: &Hub, repo: &Repository) -> Option<String> {
    hub.account(&repo.host).map(|a| a.login)
}

/// Cache keys, shared with the windows (they read the same answers).
pub fn pulls_key(
    filter: PullFilter,
    state: StateFilter,
    text: Option<&str>,
    max: usize,
    cursor: Option<&str>,
) -> String {
    format!(
        "pulls:{}:{}:{}:{max}:{}",
        serde_json::to_string(&filter)
            .unwrap_or_default()
            .trim_matches('"'),
        serde_json::to_string(&state)
            .unwrap_or_default()
            .trim_matches('"'),
        text.unwrap_or(""),
        cursor.unwrap_or("")
    )
}

pub fn issues_key(q: &IssueQuery) -> String {
    format!(
        "issues:{}:{}:{}:{}:{}:{}",
        serde_json::to_string(&q.filter)
            .unwrap_or_default()
            .trim_matches('"'),
        serde_json::to_string(&q.state)
            .unwrap_or_default()
            .trim_matches('"'),
        q.labels.join(","),
        q.text.as_deref().unwrap_or(""),
        q.max,
        q.cursor.as_deref().unwrap_or("")
    )
}

pub fn pull_key(item: &ItemRef) -> String {
    format!("pull:{}", item.id())
}

pub fn issue_key(item: &ItemRef) -> String {
    format!("issue:{}", item.id())
}

pub fn checks_key(commit: &str) -> String {
    format!("checks:{commit}")
}

/// The output of `checks`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecksOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub state: ChecksState,
    pub items: Vec<Check>,
}

/// Run command `id` (its last dotted piece: `pulls`, `pull_create`) with `input`.
pub fn run(hub: &Arc<Hub>, git: &dyn GitSide, id: &str, input: &Value) -> Result<Value> {
    let name = id.rsplit('.').next().unwrap_or(id);
    match name {
        "detect" => detect(hub, git, input),
        "auth" => auth_cmd(hub, git, input),
        "refresh" => refresh(hub, git, input),
        _ => {
            let repo = repository(hub, git, input)?;
            need_forge(&repo)?;
            match name {
                "pulls" => pulls(hub, &repo, input),
                "pull" => pull(hub, &repo, input),
                "pull_create" => pull_create(hub, git, &repo, input),
                "pull_update" => pull_update(hub, &repo, input),
                "pull_comment" => pull_comment(hub, &repo, input),
                "pull_review" => pull_review(hub, &repo, input),
                "pull_checkout" => pull_checkout(hub, git, &repo, input),
                "pull_merge" => pull_merge(hub, &repo, input),
                "pull_close" => pull_close(hub, &repo, input),
                "pull_ready" => pull_ready(hub, &repo, input),
                "thread_resolve" => thread_resolve(hub, &repo, input),
                "issues" => issues(hub, &repo, input),
                "issue" => issue(hub, &repo, input),
                "issue_create" => issue_create(hub, &repo, input),
                "issue_comment" => issue_comment(hub, &repo, input),
                "issue_update" => issue_update(hub, &repo, input),
                "branch_from_issue" => branch_from_issue(hub, git, &repo, input),
                "checks" => checks(hub, git, &repo, input),
                "check_log" => check_log(hub, &repo, input),
                "check_rerun" => check_rerun(hub, &repo, input),
                other => Err(ForgeError::invalid(format!("not a forge command: {other}"))),
            }
        }
    }
}

fn detect(hub: &Hub, git: &dyn GitSide, input: &Value) -> Result<Value> {
    let probe = b(input, "probe").unwrap_or(true);
    let mut repo = match s(input, "url") {
        Some(url) => hub.detect(&url, probe),
        None => {
            let remote = input.get("remote").and_then(Value::as_str);
            match git.remote(remote) {
                Some((name, url)) => {
                    let mut r = hub.detect(&url, probe);
                    r.remote = Some(name);
                    r
                }
                None => Repository::default(),
            }
        }
    };
    let caps = capabilities(repo.family);
    if repo.family == Family::None {
        let what = repo
            .remote_url
            .clone()
            .or(repo.remote.clone())
            .unwrap_or_else(|| "this workspace (no git remote)".into());
        repo.host = repo.host.clone();
        return Ok(json!({
            "repository": to_value(&repo),
            "signed_in": false,
            "capabilities": to_value(&caps),
            "message": format!("No supported forge for {what}"),
        }));
    }
    let account = hub.account(&repo.host);
    let mut out = json!({
        "repository": to_value(&repo),
        "signed_in": hub.credential(&repo.host).is_some(),
        "capabilities": to_value(&caps),
    });
    if let Some(a) = account {
        out["account"] = to_value(&a);
    }
    Ok(out)
}

fn auth_cmd(hub: &Arc<Hub>, git: &dyn GitSide, input: &Value) -> Result<Value> {
    let action = s(input, "action").unwrap_or_else(|| "status".into());
    let repo = match s(input, "host") {
        Some(h) => {
            // A host alone: detect it as the url `https://<host>/x/y` would.
            let mut r = hub.detect(&format!("https://{h}/_/_"), true);
            if r.family == Family::None
                && let Ok(w) = repository(hub, git, input)
                && w.host == h
            {
                r = w;
            }
            r.host = h;
            r
        }
        None => repository(hub, git, input).unwrap_or_default(),
    };
    let status = |message: Option<String>| -> Value {
        let cred = hub.credential(&repo.host);
        let mut out = json!({
            "host": repo.host,
            "family": repo.family.as_str(),
            "signed_in": cred.is_some(),
            "hosts": hub.credentials().hosts().into_iter().map(|(h, kind)| {
                let c = hub.credential(&h).map(|(c, _)| c);
                let mut v = json!({"host": h, "family": c.as_ref().map(|c| c.family.as_str()).unwrap_or("none"), "signed_in": c.is_some(), "store": kind.as_str()});
                if let Some(a) = c.and_then(|c| c.account) { v["account"] = to_value(&a); }
                v
            }).collect::<Vec<_>>(),
        });
        if let Some((c, kind)) = cred {
            out["store"] = json!(kind.as_str());
            out["method"] = to_value(&c.method);
            if let Some(a) = c.account {
                out["account"] = to_value(&a);
            }
        }
        if let Some(f) = hub.flow(&repo.host) {
            out["pending"] = json!(true);
            out["device"] = json!({"user_code": f.start.user_code, "verification_uri": f.start.verification_uri, "expires_in": f.start.expires_in, "interval": f.start.interval});
        }
        if let Some(m) = message {
            out["message"] = json!(m);
        }
        out
    };
    match action.as_str() {
        "status" => Ok(status(None)),
        "sign_out" => {
            hub.sign_out(&repo.host)?;
            Ok(status(Some(format!("Signed out of {}", repo.host))))
        }
        "cancel" => {
            hub.cancel_flow(&repo.host);
            Ok(status(Some("The device flow was canceled".into())))
        }
        "sign_in" => {
            need_forge(&repo)?;
            let allow_file = b(input, "allow_file_store").unwrap_or(false);
            let method = s(input, "method")
                .and_then(|m| SignInMethod::parse(&m))
                .unwrap_or(SignInMethod::Token);
            match method {
                SignInMethod::Device => {
                    let start = hub.start_device_flow(&repo, allow_file)?;
                    let mut out = status(Some(format!(
                        "Enter the code {} at {} to sign in to {}",
                        start.user_code, start.verification_uri, repo.host
                    )));
                    out["pending"] = json!(true);
                    out["method"] = json!("device");
                    out["device"] = json!({"user_code": start.user_code, "verification_uri": start.verification_uri, "expires_in": start.expires_in, "interval": start.interval});
                    Ok(out)
                }
                SignInMethod::Cli => {
                    let token = auth::cli_token(
                        repo.family,
                        &repo.host,
                        hub_cli_override(hub, repo.family).as_deref(),
                    )?;
                    let (a, _) = hub.sign_in_token(&repo, method, token, None, allow_file)?;
                    Ok(status(Some(format!(
                        "Signed in to {} as {}",
                        repo.host, a.login
                    ))))
                }
                SignInMethod::Token | SignInMethod::AppPassword => {
                    let token = s(input, "token")
                        .ok_or_else(|| ForgeError::invalid("`token` is required"))?;
                    let (a, _) = hub.sign_in_token(
                        &repo,
                        method,
                        Secret::new(token),
                        s(input, "user").as_deref(),
                        allow_file,
                    )?;
                    Ok(status(Some(format!(
                        "Signed in to {} as {}",
                        repo.host, a.login
                    ))))
                }
            }
        }
        other => Err(ForgeError::invalid(format!("unknown action `{other}`"))),
    }
}

/// The CLI path a test set (`Hub::set_cli`), else none (PATH).
fn hub_cli_override(hub: &Hub, family: Family) -> Option<std::path::PathBuf> {
    hub.cli_override(family)
}

fn pulls(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let filter: PullFilter = input
        .get("filter")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let state: StateFilter = input
        .get("state")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let text = s(input, "text");
    let max = max_of(input);
    let cursor = s(input, "cursor");
    let key = pulls_key(filter, state, text.as_deref(), max, cursor.as_deref());
    let q = PullQuery {
        filter,
        state,
        text,
        max,
        cursor,
        me: me(hub, repo),
    };
    if filter != PullFilter::All && q.me.is_none() {
        return Err(ForgeError::sign_in_required(
            &repo.host,
            "`mine` and `review_requested` need the signed-in account.",
        ));
    }
    let a = hub.read(repo, &key, b(input, "refresh").unwrap_or(false), move |f| {
        f.pulls(&q)
    })?;
    Ok(to_value(&a))
}

/// Read one pull request through the cache (the document and `pull`).
pub fn read_pull(
    hub: &Arc<Hub>,
    repo: &Repository,
    item: &ItemRef,
    wait: bool,
) -> Result<Answer<Pull>> {
    let it = item.clone();
    hub.read(repo, &pull_key(item), wait, move |f| f.pull(&it))
}

fn pull(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let mut a = read_pull(hub, repo, &item, b(input, "refresh").unwrap_or(false))?;
    match s(input, "threads").as_deref() {
        Some("unresolved") => a.value.threads.retain(|t| !t.resolved),
        Some("none") => a.value.threads.clear(),
        _ => {}
    }
    if let Some(p) = hub.pending(repo, &item) {
        a.value.pending_review = Some(PendingReview {
            id: None,
            comments: p.comments.len() as u64 + p.server_drafts,
        });
    }
    Ok(to_value(&a))
}

/// The title and body a new pull request from `head` onto `base` is prefilled with: the single commit's title (or
/// the branch name) and the commits' messages.
pub fn prefill(git: &dyn GitSide, base: &str, head: &str) -> (String, String) {
    let commits = git.commits_between(base, head);
    match commits.as_slice() {
        [(_, msg)] => {
            let mut lines = msg.lines();
            let title = lines.next().unwrap_or("").trim().to_owned();
            let body = lines.collect::<Vec<_>>().join("\n").trim().to_owned();
            (
                if title.is_empty() {
                    head.to_owned()
                } else {
                    title
                },
                body,
            )
        }
        _ => {
            let title = head
                .rsplit('/')
                .next()
                .unwrap_or(head)
                .replace(['-', '_'], " ");
            let mut t = title.trim().to_owned();
            if let Some(c) = t.get(..1) {
                t = c.to_uppercase() + &t[1..];
            }
            let body = commits
                .iter()
                .rev()
                .map(|(_, m)| format!("- {}", m.lines().next().unwrap_or("").trim()))
                .collect::<Vec<_>>()
                .join("\n");
            (if t.is_empty() { head.to_owned() } else { t }, body)
        }
    }
}

fn pull_create(
    hub: &Arc<Hub>,
    git: &dyn GitSide,
    repo: &Repository,
    input: &Value,
) -> Result<Value> {
    if !capabilities(repo.family).pull_create {
        return Err(ForgeError::unsupported(
            "creating pull requests from Eludite",
            repo.family.display(),
        ));
    }
    let head = s(input, "head")
        .or_else(|| git.current_branch())
        .ok_or_else(|| {
            ForgeError::invalid("HEAD is detached: check out the branch to propose, or give `head`")
        })?;
    let remote = repo.remote.clone().unwrap_or_else(|| "origin".into());
    let base = s(input, "base")
        .or_else(|| git.default_branch(&remote))
        .unwrap_or_else(|| "main".into());
    if head == base {
        return Err(ForgeError::invalid(format!(
            "`{head}` is the base branch: create a branch for the change first"
        )));
    }
    let (title, body) = prefill(git, &format!("{remote}/{base}"), &head);
    let new = NewPull {
        title: s(input, "title").unwrap_or(title),
        body: s(input, "body").unwrap_or(body),
        head: head.clone(),
        base: base.clone(),
        draft: b(input, "draft").unwrap_or(false),
        reviewers: list(input, "reviewers").unwrap_or_default(),
        labels: list(input, "labels").unwrap_or_default(),
    };
    let created = with_forge(hub, repo, true, |f| f.create_pull(&new))?;
    let mut out = json!({"id": created.id, "url": created.url, "title": created.title, "draft": created.draft, "head": head, "base": base});
    if let Some(n) = created.number {
        out["number"] = json!(n);
    }
    Ok(out)
}

fn after_write(hub: &Hub, repo: &Repository, item: &ItemRef) {
    hub.forget(repo, &pull_key(item));
}

fn pull_update(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let edit = PullEdit {
        title: s(input, "title"),
        body: s(input, "body"),
        draft: b(input, "draft"),
        base: s(input, "base"),
    };
    let p = with_forge(hub, repo, true, |f| f.update_pull(&item, &edit))?;
    after_write(hub, repo, &item);
    let mut out = json!({"id": p.id, "url": p.url, "title": p.title, "state": to_value(&p.state), "draft": p.draft, "base": p.base});
    if let Some(n) = p.number {
        out["number"] = json!(n);
    }
    Ok(out)
}

fn draft_of(input: &Value) -> DraftComment {
    DraftComment {
        body: s(input, "body").unwrap_or_default(),
        path: s(input, "path"),
        line: input.get("line").and_then(Value::as_u64),
        start_line: input.get("start_line").and_then(Value::as_u64),
        side: if s(input, "side").as_deref() == Some("left") {
            Side::Left
        } else {
            Side::Right
        },
        reply_to: s(input, "reply_to"),
    }
}

fn pull_comment(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let c = draft_of(input);
    if c.line.is_some() != c.path.is_some() {
        return Err(ForgeError::invalid(
            "a comment on a line needs both `path` and `line`",
        ));
    }
    if b(input, "pending").unwrap_or(false) {
        let out = review_add(hub, repo, &item, c)?;
        return Ok(json!({"id": out.0, "pending": out.1}));
    }
    let posted = with_forge(hub, repo, true, |f| f.comment_pull(&item, &c))?;
    after_write(hub, repo, &item);
    let mut out = json!({"id": posted.id, "pending": false});
    if let Some(t) = posted.thread {
        out["thread"] = json!(t);
    }
    if let Some(u) = posted.url {
        out["url"] = json!(u);
    }
    Ok(out)
}

/// Add a review comment: held locally, kept as a draft on the forge, or posted at once, by the forge's mode.
fn review_add(
    hub: &Arc<Hub>,
    repo: &Repository,
    item: &ItemRef,
    c: DraftComment,
) -> Result<(String, bool)> {
    let forge = hub.signed_in_forge(repo, Cancel::new())?;
    match forge.pending_mode() {
        PendingMode::Local => {
            if c.reply_to.is_none() && (c.path.is_none() || c.line.is_none()) {
                return Err(ForgeError::invalid(
                    "a review comment needs `path` and `line` (or `reply_to`)",
                ));
            }
            let p = hub.update_pending(repo, item, |p| p.comments.push(c));
            Ok((format!("pending-{}", p.comments.len()), true))
        }
        PendingMode::Server => {
            let posted = forge.add_draft(item, &c)?;
            hub.update_pending(repo, item, |p| p.server_drafts += 1);
            Ok((posted.id, true))
        }
        PendingMode::None => {
            if !forge.capabilities().reviews {
                return Err(forge.unsupported("reviews"));
            }
            let posted = forge.comment_pull(item, &c)?;
            after_write(hub, repo, item);
            Ok((posted.id, false))
        }
    }
}

fn pull_review(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let action = s(input, "action").unwrap_or_default();
    let caps = capabilities(repo.family);
    if !caps.reviews && action != "request" {
        return Err(ForgeError::unsupported("reviews", repo.family.display()));
    }
    let count = |hub: &Hub| {
        hub.pending(repo, &item)
            .map(|p| p.comments.len() as u64 + p.server_drafts)
            .unwrap_or(0)
    };
    let mut out = json!({"action": action});
    match action.as_str() {
        "start" => {
            hub.signed_in_forge(repo, Cancel::new())?;
            hub.start_pending(repo, &item);
            out["message"] = json!(match crate::hub::make_forge(
                repo.clone(),
                hub.client(repo, Cancel::new()),
                None
            )?
            .pending_mode()
            {
                PendingMode::None =>
                    "this forge has no pending reviews: each comment posts at once and the submit posts the summary",
                _ => "a pending review is open: add comments, then submit",
            });
        }
        "add" => {
            let mut c = draft_of(input);
            c.body = s(input, "body").ok_or_else(|| ForgeError::invalid("`body` is required"))?;
            let (id, pending) = review_add(hub, repo, &item, c)?;
            out["review_id"] = json!(id);
            out["submitted"] = json!(!pending);
        }
        "submit" => {
            let event: ReviewEvent = input
                .get("event")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let body = s(input, "body");
            let comments = hub
                .pending(repo, &item)
                .map(|p| p.comments)
                .unwrap_or_default();
            let review = with_forge(hub, repo, true, |f| {
                f.submit_review(&item, event, body.as_deref(), &comments)
            })?;
            hub.clear_pending(repo, &item);
            after_write(hub, repo, &item);
            out["submitted"] = json!(true);
            out["state"] = to_value(&review.state);
            if let Some(id) = review.id {
                out["review_id"] = json!(id);
            }
            out["message"] = json!(format!(
                "review submitted with {} comment(s)",
                comments.len()
            ));
        }
        "discard" => {
            let forge = hub.signed_in_forge(repo, Cancel::new())?;
            if forge.pending_mode() == PendingMode::Server {
                forge.discard_drafts(&item)?;
            }
            hub.clear_pending(repo, &item);
        }
        "request" => {
            if !caps.request_review {
                return Err(ForgeError::unsupported(
                    "requesting reviews",
                    repo.family.display(),
                ));
            }
            let reviewers = match list(input, "reviewers").filter(|r| !r.is_empty()) {
                Some(r) => r,
                None => {
                    let p = read_pull(hub, repo, &item, false)?.value;
                    let mut r: Vec<String> = p
                        .reviews
                        .iter()
                        .map(|x| x.author.clone())
                        .filter(|a| !a.is_empty() && Some(a) != me(hub, repo).as_ref())
                        .collect();
                    r.dedup();
                    r
                }
            };
            if reviewers.is_empty() {
                return Err(ForgeError::invalid(
                    "no previous reviewers: name `reviewers`",
                ));
            }
            let requested = with_forge(hub, repo, true, |f| f.request_review(&item, &reviewers))?;
            after_write(hub, repo, &item);
            out["requested"] = json!(requested);
        }
        other => return Err(ForgeError::invalid(format!("unknown action `{other}`"))),
    }
    out["pending_comments"] = json!(count(hub));
    Ok(out)
}

fn cached_or_fetch_pull(hub: &Arc<Hub>, repo: &Repository, item: &ItemRef) -> Result<Pull> {
    match hub.cached::<Pull>(repo, &pull_key(item)) {
        Some(a) => Ok(a.value),
        None => Ok(read_pull(hub, repo, item, true)?.value),
    }
}

fn pull_checkout(
    hub: &Arc<Hub>,
    git: &dyn GitSide,
    repo: &Repository,
    input: &Value,
) -> Result<Value> {
    let item = item_of(input)?;
    let pull = cached_or_fetch_pull(hub, repo, &item)?;
    let forge = hub.forge(repo, Cancel::new())?;
    let cref = forge.checkout_ref(&pull);
    let branch = s(input, "branch").unwrap_or(cref.branch.clone());
    let remote = repo.remote.clone().unwrap_or_else(|| "origin".into());
    let (commit, generation) = git
        .fetch_and_checkout(&remote, cref.url.as_deref(), &cref.refspec, &branch)
        .map_err(|e| ForgeError::new(ErrorKind::Conflict, e))?;
    let mut out = json!({"id": pull.summary.id, "branch": branch, "ref": cref.refspec, "commit": commit, "generation": generation});
    if let Some(n) = pull.summary.number {
        out["number"] = json!(n);
    }
    Ok(out)
}

/// Why a merge is refused before it is sent, or `None` to send it.
pub fn merge_refusal(
    pull: &Pull,
    method: MergeMethod,
    force: bool,
    when_checks_pass: bool,
) -> Option<String> {
    let m = pull.mergeable.clone().unwrap_or_default();
    match m.state {
        MergeState::Merged => return Some("it is already merged".into()),
        MergeState::Closed => return Some("it is closed: reopen it first".into()),
        _ => {}
    }
    if !m.methods.is_empty() && !m.methods.contains(&method) {
        return Some(format!(
            "this repository does not allow `{}` (it allows {})",
            method.as_str(),
            m.methods
                .iter()
                .map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if force {
        return None;
    }
    let why = match m.state {
        MergeState::Conflicts => Some("the forge reports conflicts with the base branch"),
        MergeState::Blocked => {
            Some("the forge reports it blocked (required reviews or branch rules)")
        }
        MergeState::Draft => Some("it is a draft: mark it ready first"),
        MergeState::ChecksFailing if !when_checks_pass => Some("its checks are failing"),
        _ => None,
    };
    why.map(|w| format!("{w}; `force` merges anyway (not for agents)"))
}

fn pull_merge(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let method = s(input, "method")
        .and_then(|m| MergeMethod::parse(&m))
        .ok_or_else(|| ForgeError::invalid("`method` is required"))?;
    let force = b(input, "force").unwrap_or(false);
    let when = b(input, "when_checks_pass").unwrap_or(false);
    let pull = read_pull(hub, repo, &item, true)?;
    if pull.stale {
        return Err(pull
            .refresh_error
            .unwrap_or_else(|| ForgeError::other("the pull request could not be read afresh")));
    }
    let pull = pull.value;
    if let Some(why) = merge_refusal(&pull, method, force, when) {
        return Err(ForgeError::new(
            ErrorKind::Conflict,
            format!("merge refused: {why}"),
        ));
    }
    let m = MergeRequest {
        method: Some(method),
        title: s(input, "title"),
        message: s(input, "message"),
        delete_branch: b(input, "delete_branch").unwrap_or(false),
        when_checks_pass: when,
        head_sha: pull.head_sha.clone(),
    };
    let r = with_forge(hub, repo, true, |f| f.merge(&item, &m))?;
    after_write(hub, repo, &item);
    let mut out = json!({"id": pull.summary.id, "merged": r.merged, "method": method.as_str(), "auto_merge": r.auto_merge, "message": r.message});
    if let Some(n) = pull.summary.number {
        out["number"] = json!(n);
    }
    if let Some(sha) = r.sha.filter(|s| s.len() == 40) {
        out["sha"] = json!(sha);
    }
    Ok(out)
}

fn pull_close(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let open = b(input, "reopen").unwrap_or(false);
    let st = with_forge(hub, repo, true, |f| f.set_pull_state(&item, open))?;
    after_write(hub, repo, &item);
    let mut out = json!({"id": item.id(), "state": to_value(&st)});
    if let Some(n) = item.number() {
        out["number"] = json!(n);
    }
    Ok(out)
}

fn pull_ready(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    if !capabilities(repo.family).draft_pull_requests {
        return Err(ForgeError::unsupported(
            "draft pull requests",
            repo.family.display(),
        ));
    }
    let draft = with_forge(hub, repo, true, |f| {
        f.set_draft(&item, b(input, "draft").unwrap_or(false))
    })?;
    after_write(hub, repo, &item);
    let mut out = json!({"id": item.id(), "draft": draft});
    if let Some(n) = item.number() {
        out["number"] = json!(n);
    }
    Ok(out)
}

fn thread_resolve(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    if !capabilities(repo.family).thread_resolution {
        return Err(ForgeError::unsupported(
            "resolving review threads",
            repo.family.display(),
        ));
    }
    let thread = s(input, "thread").ok_or_else(|| ForgeError::invalid("`thread` is required"))?;
    let resolved = with_forge(hub, repo, true, |f| {
        f.resolve_thread(&item, &thread, b(input, "resolved").unwrap_or(true))
    })?;
    after_write(hub, repo, &item);
    Ok(json!({"thread": thread, "resolved": resolved}))
}

fn issue_query(hub: &Hub, repo: &Repository, input: &Value) -> IssueQuery {
    IssueQuery {
        filter: input
            .get("filter")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        state: input
            .get("state")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        labels: list(input, "labels").unwrap_or_default(),
        text: s(input, "text"),
        max: max_of(input),
        cursor: s(input, "cursor"),
        me: me(hub, repo),
    }
}

fn issues(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    if !capabilities(repo.family).issues {
        return Err(ForgeError::unsupported("issues", repo.family.display()));
    }
    let q = issue_query(hub, repo, input);
    if q.filter != IssueFilter::All && q.me.is_none() {
        return Err(ForgeError::sign_in_required(
            &repo.host,
            "`mine` and `assigned` need the signed-in account.",
        ));
    }
    let key = issues_key(&q);
    let a = hub.read(repo, &key, b(input, "refresh").unwrap_or(false), move |f| {
        f.issues(&q)
    })?;
    Ok(to_value(&a))
}

/// Read one issue through the cache.
pub fn read_issue(
    hub: &Arc<Hub>,
    repo: &Repository,
    item: &ItemRef,
    wait: bool,
) -> Result<Answer<Issue>> {
    let it = item.clone();
    hub.read(repo, &issue_key(item), wait, move |f| f.issue(&it))
}

fn issue(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    Ok(to_value(&read_issue(
        hub,
        repo,
        &item,
        b(input, "refresh").unwrap_or(false),
    )?))
}

fn issue_create(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let new = NewIssue {
        title: s(input, "title").ok_or_else(|| ForgeError::invalid("`title` is required"))?,
        body: s(input, "body").unwrap_or_default(),
        labels: list(input, "labels").unwrap_or_default(),
        assignees: list(input, "assignees").unwrap_or_default(),
        milestone: s(input, "milestone"),
        kind: s(input, "type"),
    };
    let i = with_forge(hub, repo, true, |f| f.create_issue(&new))?;
    let mut out = json!({"id": i.id, "url": i.url, "title": i.title});
    if let Some(n) = i.number {
        out["number"] = json!(n);
    }
    Ok(out)
}

fn issue_comment(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let body = s(input, "body").ok_or_else(|| ForgeError::invalid("`body` is required"))?;
    let p = with_forge(hub, repo, true, |f| f.comment_issue(&item, &body))?;
    hub.forget(repo, &issue_key(&item));
    let mut out = json!({"id": p.id});
    if let Some(u) = p.url {
        out["url"] = json!(u);
    }
    Ok(out)
}

fn issue_update(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let item = item_of(input)?;
    let edit = IssueEdit {
        state: s(input, "state").map(|x| {
            if x == "closed" {
                IssueState::Closed
            } else {
                IssueState::Open
            }
        }),
        labels: list(input, "labels"),
        assignees: list(input, "assignees"),
        milestone: s(input, "milestone"),
        title: s(input, "title"),
        body: s(input, "body"),
    };
    if edit.is_empty() {
        return Err(ForgeError::invalid(
            "nothing to change: give `state`, `labels`, `assignees`, `milestone`, `title` or `body`",
        ));
    }
    let caps = capabilities(repo.family);
    if (edit.labels.is_some() && !caps.labels)
        || (edit.assignees.is_some() && !caps.assignees)
        || (edit.milestone.is_some() && !caps.milestones)
    {
        return Err(ForgeError::unsupported(
            "those issue fields",
            repo.family.display(),
        ));
    }
    let i = with_forge(hub, repo, true, |f| f.update_issue(&item, &edit))?;
    hub.forget(repo, &issue_key(&item));
    Ok(to_value(&i))
}

fn branch_from_issue(
    hub: &Arc<Hub>,
    git: &dyn GitSide,
    repo: &Repository,
    input: &Value,
) -> Result<Value> {
    let item = item_of(input)?;
    let issue = match hub.cached::<Issue>(repo, &issue_key(&item)) {
        Some(a) => a.value,
        None => read_issue(hub, repo, &item, true)?.value,
    };
    let name = s(input, "name")
        .unwrap_or_else(|| issue_branch_name(&issue.summary.label(), &issue.summary.title));
    let commit = git
        .create_branch(
            &name,
            s(input, "base").as_deref(),
            b(input, "checkout").unwrap_or(true),
        )
        .map_err(|e| ForgeError::new(ErrorKind::Conflict, e))?;
    let mut out = json!({"id": issue.summary.id, "branch": name, "commit": commit, "checked_out": b(input, "checkout").unwrap_or(true), "linked": false});
    if let Some(n) = issue.summary.number {
        out["number"] = json!(n);
    }
    if b(input, "link").unwrap_or(true) && capabilities(repo.family).branch_link {
        match with_forge(hub, repo, true, |f| f.link_branch(&item, &name, &commit)) {
            Ok(linked) => {
                out["linked"] = json!(linked);
                hub.forget(repo, &issue_key(&item));
            }
            Err(e) => {
                out["message"] = json!(format!(
                    "the branch was created; the forge did not record the link: {e}"
                ))
            }
        }
    }
    Ok(out)
}

fn checks(hub: &Arc<Hub>, git: &dyn GitSide, repo: &Repository, input: &Value) -> Result<Value> {
    if !capabilities(repo.family).checks {
        return Ok(json!({"state": "none", "items": [], "stale": false}));
    }
    let item = item_of(input).ok();
    let commit = match (s(input, "ref"), &item) {
        (Some(r), _) if r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit()) => r,
        (Some(r), _) => git
            .resolve(Some(&r))
            .ok_or_else(|| ForgeError::invalid(format!("no revision `{r}`")))?,
        (None, Some(i)) => cached_or_fetch_pull(hub, repo, i)?
            .head_sha
            .ok_or_else(|| ForgeError::other("the pull request has no head commit"))?,
        (None, None) => git
            .resolve(None)
            .ok_or_else(|| ForgeError::invalid("the repository has no commit yet"))?,
    };
    let key = match &item {
        Some(i) => format!("{}:{}", checks_key(&commit), i.id()),
        None => checks_key(&commit),
    };
    let c = commit.clone();
    let it = item.clone();
    let a = hub.read(repo, &key, b(input, "refresh").unwrap_or(false), move |f| {
        let items = f.checks(&c, it.as_ref())?;
        Ok(ChecksOut {
            commit: Some(c.clone()),
            state: ChecksState::of(&items),
            items,
        })
    })?;
    Ok(to_value(&a))
}

fn check_log(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let id = s(input, "id").ok_or_else(|| ForgeError::invalid("`id` is required"))?;
    let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let max = input
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|m| (m as usize).clamp(1, LOG_CAP))
        .unwrap_or(LOG_DEFAULT);
    let log = with_forge(hub, repo, false, |f| f.check_log(&id))?;
    let total = log.text.len();
    let mut start = offset.min(total);
    while !log.text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + max).min(total);
    while !log.text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = json!({"id": id, "text": &log.text[start..end], "offset": start, "total_bytes": total, "truncated": end < total});
    if end < total {
        out["next_offset"] = json!(end);
    }
    if let Some(u) = log.url {
        out["url"] = json!(u);
    }
    Ok(out)
}

fn check_rerun(hub: &Arc<Hub>, repo: &Repository, input: &Value) -> Result<Value> {
    let id = s(input, "id").ok_or_else(|| ForgeError::invalid("`id` is required"))?;
    let message = with_forge(hub, repo, true, |f| f.rerun(&id))?;
    hub.forget_prefix_checks(repo);
    Ok(json!({"id": id, "rerun": true, "message": message}))
}

fn refresh(hub: &Arc<Hub>, git: &dyn GitSide, input: &Value) -> Result<Value> {
    let repo = repository(hub, git, input)?;
    need_forge(&repo)?;
    let what = list(input, "what")
        .unwrap_or_else(|| vec!["pulls".into(), "issues".into(), "checks".into()]);
    let mut refreshed = Vec::new();
    let mut errors = Vec::new();
    let caps = capabilities(repo.family);
    for w in &what {
        let r = match w.as_str() {
            "pulls" => pulls(hub, &repo, &json!({"refresh": true})),
            "issues" if caps.issues => issues(hub, &repo, &json!({"refresh": true})),
            "issues" => continue,
            "checks" if caps.checks => checks(hub, git, &repo, &json!({"refresh": true})),
            "checks" => continue,
            "pull" => match item_of(input) {
                Ok(_) => {
                    let mut i = input.clone();
                    i["refresh"] = json!(true);
                    i.as_object_mut().map(|o| o.remove("what"));
                    pull(hub, &repo, &i)
                }
                Err(e) => Err(e),
            },
            _ => continue,
        };
        match r {
            Ok(v) if v.get("refresh_error").is_some() => {
                if let Ok(e) = serde_json::from_value::<ForgeError>(v["refresh_error"].clone()) {
                    errors.push(e);
                }
            }
            Ok(_) => refreshed.push(w.clone()),
            Err(e) => errors.push(e),
        }
    }
    Ok(json!({"refreshed": refreshed, "errors": to_value(&errors)}))
}

/// What the audit keeps of a forge command's arguments: never a token, a body's length rather than the body, and
/// the forge and repository it went to.
pub fn audit_arguments(input: &Value, repo: Option<&Repository>) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(r) = repo {
        out.insert("forge".into(), json!(r.family.as_str()));
        out.insert("repository".into(), json!(r.path));
    }
    if let Some(o) = input.as_object() {
        for (k, v) in o {
            match k.as_str() {
                "token" => {
                    out.insert("token".into(), json!("<hidden>"));
                }
                "body" | "message" => {
                    out.insert(
                        format!("{k}_length"),
                        json!(v.as_str().map(str::len).unwrap_or(0)),
                    );
                }
                _ => {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }
    Value::Object(out)
}
