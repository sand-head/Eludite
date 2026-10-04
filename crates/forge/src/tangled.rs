//! Tangled (tangled.org, formerly tangled.sh, and self-hosted knots): a forge on the AT Protocol. Repositories live
//! on knots; their records (issues, pull requests, comments, states) live in each author's personal data server
//! under the `sh.tangled.*` lexicons (pinned in `protocol/forge/tangled/`); the appview's API (`api.tangled.org`,
//! the service its source calls bobbin) indexes them. Reads go to the appview's XRPC queries without a token
//! (`sh.tangled.repo.listPulls`, `getPull`, `listIssues`, `getIssue`, `sh.tangled.feed.listComments`,
//! `sh.tangled.repo.pull.listStatuses`, `sh.tangled.repo.compare`); writes are records the signed-in account creates
//! in its own personal data server (`com.atproto.repo.createRecord`) with a session made from an app password.
//!
//! What Tangled has no place for in the trait, the capabilities say: no reviews or approvals, no line-anchored
//! threads (comments are on the pull request), no drafts, no labels, milestones or assignees through records Eludite
//! writes, no merge from Eludite (a knot merges the patch for the appview), no creating pull requests (a pull request
//! carries its patch as a blob), and CI (spindles) is not read. Items have no numbers: their id is the record's
//! AT-URI.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::client::{Client, time_of};
use crate::credentials::Credential;
use crate::error::{ErrorKind, ForgeError, Result};
use crate::forge::{Forge, PendingMode, capabilities};
use crate::http::{Cancel, Method, Request, Transport, is_loopback};
use crate::model::*;
use crate::util::{encode, first_line, now_secs, query, rfc3339, str_at, str_of, u64_of};

pub struct Tangled {
    pub repo: Repository,
    pub client: Client,
    cred: Option<Credential>,
    resolved: OnceLock<Result<RepoIds>>,
}

/// The repository's identities: its record's AT-URI and its own DID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIds {
    pub record: String,
    pub did: String,
    pub knot: Option<String>,
}

/// A personal data server session.
#[derive(Debug, Clone)]
pub struct Session {
    pub did: String,
    pub handle: String,
    pub pds: String,
    pub access: String,
    created: i64,
}

/// Sessions by account, kept for an hour so writes do not create one each (createSession is rate-limited).
fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static S: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Handles by DID, resolved through the PLC directory once per process.
fn handles() -> &'static Mutex<HashMap<String, String>> {
    static H: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Where identities resolve: a handle through `com.atproto.identity.resolveHandle`, a DID's document through the PLC
/// directory. Against a loopback appview (the fixture server) both are the appview itself.
fn resolver(appview: &str) -> (String, String) {
    if is_loopback(appview) {
        let b = appview.trim_end_matches('/').to_owned();
        (b.clone(), format!("{b}/plc"))
    } else {
        ("https://bsky.social".into(), "https://plc.directory".into())
    }
}

fn get_json(t: &dyn Transport, url: &str) -> Result<Value> {
    let r = t.send(
        &Request::new(Method::Get, url).header("Accept", "application/json"),
        &Cancel::new(),
    )?;
    if r.status >= 400 {
        return Err(ForgeError::new(
            if r.status == 404 || r.status == 400 {
                ErrorKind::NotFound
            } else {
                ErrorKind::Other
            },
            format!(
                "{url} answered HTTP {}: {}",
                r.status,
                crate::client::forge_message(&r).unwrap_or_default()
            ),
        ));
    }
    r.json()
}

/// The DID of `handle` (a DID is kept).
pub fn resolve_handle(t: &dyn Transport, appview: &str, handle: &str) -> Result<String> {
    let handle = handle.trim_start_matches('@');
    if handle.starts_with("did:") {
        return Ok(handle.to_owned());
    }
    let (res, _) = resolver(appview);
    let v = get_json(
        t,
        &format!(
            "{res}/xrpc/com.atproto.identity.resolveHandle?handle={}",
            encode(handle)
        ),
    )?;
    str_of(&v, "did").ok_or_else(|| {
        ForgeError::new(
            ErrorKind::NotFound,
            format!("the handle {handle} did not resolve"),
        )
    })
}

/// The personal data server of `did`, from its DID document.
pub fn pds_of(t: &dyn Transport, appview: &str, did: &str) -> Result<String> {
    let (_, plc) = resolver(appview);
    let url = if let Some(domain) = did.strip_prefix("did:web:") {
        format!("https://{domain}/.well-known/did.json")
    } else {
        format!("{plc}/{did}")
    };
    let doc = get_json(t, &url)?;
    doc["service"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| {
            s["id"]
                .as_str()
                .is_some_and(|i| i.ends_with("#atproto_pds"))
        })
        .and_then(|s| str_of(s, "serviceEndpoint"))
        .ok_or_else(|| {
            ForgeError::other(format!(
                "{did}'s DID document names no personal data server"
            ))
        })
}

/// Make a session with an app password (`com.atproto.server.createSession`).
pub fn create_session(
    t: &dyn Transport,
    repo: &Repository,
    handle: &str,
    password: &str,
) -> Result<Session> {
    let did = resolve_handle(t, &repo.api, handle)?;
    let key = format!("{}|{did}", repo.api);
    if let Some(s) = sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        && now_secs() - s.created < 3600
    {
        return Ok(s.clone());
    }
    let pds = pds_of(t, &repo.api, &did)?;
    let r = t.send(
        &Request::new(
            Method::Post,
            format!(
                "{}/xrpc/com.atproto.server.createSession",
                pds.trim_end_matches('/')
            ),
        )
        .json(&json!({"identifier": did, "password": password})),
        &Cancel::new(),
    )?;
    if r.status == 401 || r.status == 400 {
        return Err(ForgeError::new(
            ErrorKind::Unauthorized,
            format!(
                "{pds} refused the app password: {}",
                crate::client::forge_message(&r).unwrap_or_default()
            ),
        )
        .with_host(&repo.host));
    }
    if r.status >= 300 {
        return Err(ForgeError::other(format!(
            "{pds} answered HTTP {}",
            r.status
        )));
    }
    let v = r.json()?;
    let s = Session {
        did: str_of(&v, "did").unwrap_or(did),
        handle: str_of(&v, "handle").unwrap_or_else(|| handle.trim_start_matches('@').to_owned()),
        pds,
        access: str_of(&v, "accessJwt")
            .ok_or_else(|| ForgeError::other("the session answered no token"))?,
        created: now_secs(),
    };
    sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, s.clone());
    Ok(s)
}

/// The DID that wrote a record (`at://<did>/<collection>/<rkey>`).
fn author_of(uri: &str) -> String {
    uri.trim_start_matches("at://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_owned()
}

impl Tangled {
    pub fn new(repo: Repository, client: Client, cred: Option<Credential>) -> Self {
        Self {
            repo,
            client,
            cred,
            resolved: OnceLock::new(),
        }
    }

    fn xrpc(&self, nsid: &str, params: &[(&str, String)]) -> Result<Value> {
        self.client.get(&format!("/xrpc/{nsid}?{}", query(params)))
    }

    /// The repository's record and DID: a knot's url names the DID; `owner/name` is looked up in the owner's
    /// repositories.
    pub fn ids(&self) -> Result<RepoIds> {
        self.resolved
            .get_or_init(|| {
                let t = &*self.client.transport;
                if self.repo.owner.is_empty() && self.repo.name.starts_with("did:") {
                    let v = self.xrpc(
                        "sh.tangled.repo.getRepoByRepoDid",
                        &[("repoDid", self.repo.name.clone())],
                    )?;
                    return Ok(RepoIds {
                        record: str_of(&v, "uri").unwrap_or_default(),
                        did: self.repo.name.clone(),
                        knot: str_at(&v, "/value/knot"),
                    });
                }
                let owner = resolve_handle(t, &self.client.base, &self.repo.owner)?;
                let v = self.xrpc(
                    "sh.tangled.repo.listRepos",
                    &[("subject", owner.clone()), ("limit", "100".into())],
                )?;
                let hit = v["items"].as_array().into_iter().flatten().find(|r| {
                    r.pointer("/value/name").and_then(Value::as_str)
                        == Some(self.repo.name.as_str())
                        || r["uri"]
                            .as_str()
                            .is_some_and(|u| u.rsplit('/').next() == Some(self.repo.name.as_str()))
                });
                let hit = hit.ok_or_else(|| {
                    ForgeError::new(
                        ErrorKind::NotFound,
                        format!(
                            "{} has no Tangled repository named {}",
                            self.repo.owner, self.repo.name
                        ),
                    )
                })?;
                Ok(RepoIds {
                    record: str_of(hit, "uri").unwrap_or_default(),
                    did: str_at(hit, "/value/repoDid").ok_or_else(|| {
                        ForgeError::unsupported(
                            "repositories without their own DID (made before repository DIDs)",
                            "Tangled",
                        )
                    })?,
                    knot: str_at(hit, "/value/knot"),
                })
            })
            .clone()
    }

    fn handle(&self, did: &str) -> String {
        if let Some(h) = handles().lock().unwrap_or_else(|e| e.into_inner()).get(did) {
            return h.clone();
        }
        let (_, plc) = resolver(&self.client.base);
        let h = get_json(&*self.client.transport, &format!("{plc}/{did}"))
            .ok()
            .and_then(|d| {
                d["alsoKnownAs"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .map(|a| a.trim_start_matches("at://").to_owned())
            .unwrap_or_else(|| did.to_owned());
        handles()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(did.to_owned(), h.clone());
        h
    }

    fn web(&self, kind: &str, uri: &str) -> String {
        format!(
            "{}/{kind}/{}",
            self.repo.web_url,
            uri.rsplit('/').next().unwrap_or("")
        )
    }

    fn pull_summary(&self, v: &Value, ids: &RepoIds) -> Option<PullSummary> {
        let uri = str_of(v, "uri")?;
        let r = &v["value"];
        let state = match v["state"].as_str() {
            Some("merged") => State::Merged,
            Some("closed") => State::Closed,
            _ => State::Open,
        };
        let source_repo = str_at(r, "/source/repo");
        Some(PullSummary {
            number: None,
            id: uri.clone(),
            title: str_of(r, "title").unwrap_or_default(),
            state,
            draft: false,
            author: self.handle(&author_of(&uri)),
            head: str_at(r, "/source/branch").unwrap_or_default(),
            base: str_at(r, "/target/branch").unwrap_or_default(),
            head_repository: source_repo.filter(|s| *s != ids.did),
            created_at: time_of(r, "createdAt"),
            updated_at: time_of(v, "stateUpdatedAt").or_else(|| time_of(r, "createdAt")),
            url: self.web("pulls", &uri),
            labels: Vec::new(),
            comments: u64_of(v, "commentCount"),
            review_requested: false,
            checks: None,
            requested_reviewers: Vec::new(),
        })
    }

    fn issue_summary(&self, v: &Value) -> Option<IssueSummary> {
        let uri = str_of(v, "uri")?;
        let r = &v["value"];
        Some(IssueSummary {
            number: None,
            id: uri.clone(),
            title: str_of(r, "title").unwrap_or_default(),
            state: if v["state"] == "closed" {
                IssueState::Closed
            } else {
                IssueState::Open
            },
            kind: None,
            author: Some(self.handle(&author_of(&uri))),
            assignees: Vec::new(),
            labels: Vec::new(),
            milestone: None,
            comments: u64_of(v, "commentCount"),
            created_at: time_of(r, "createdAt"),
            updated_at: time_of(v, "stateUpdatedAt").or_else(|| time_of(r, "createdAt")),
            url: self.web("issues", &uri),
        })
    }

    fn comments(&self, subject: &str) -> Result<Vec<Comment>> {
        let v = self.xrpc(
            "sh.tangled.feed.listComments",
            &[
                ("subject", subject.to_owned()),
                ("order", "asc".into()),
                ("limit", "100".into()),
            ],
        )?;
        Ok(v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| {
                let uri = str_of(c, "uri").unwrap_or_default();
                Comment {
                    author: self.handle(&author_of(&uri)),
                    body: str_at(c, "/value/body/text")
                        .or_else(|| str_at(c, "/value/body"))
                        .unwrap_or_default(),
                    created_at: time_of(&c["value"], "createdAt"),
                    url: None,
                    pending: false,
                    id: uri,
                }
            })
            .collect())
    }

    fn uri(&self, item: &ItemRef) -> Result<String> {
        match item {
            ItemRef::Id(s) if s.starts_with("at://") => Ok(s.clone()),
            other => Err(ForgeError::invalid(format!(
                "Tangled's pull requests and issues have no numbers: pass the `id` (an at:// uri) a list answered, not `{other}`"
            ))),
        }
    }

    fn session(&self) -> Result<Session> {
        let cred = self.cred.as_ref().ok_or_else(|| {
            ForgeError::sign_in_required(&self.repo.host, "no app password is stored for Tangled.")
        })?;
        let handle = cred
            .account
            .as_ref()
            .map(|a| a.login.clone())
            .or_else(|| cred.did.clone())
            .unwrap_or_default();
        create_session(
            &*self.client.transport,
            &self.repo,
            &handle,
            cred.token.expose(),
        )
    }

    fn create_record(&self, collection: &str, record: Value) -> Result<(String, String)> {
        let s = self.session()?;
        let r = self.client.transport.send(
            &Request::new(
                Method::Post,
                format!(
                    "{}/xrpc/com.atproto.repo.createRecord",
                    s.pds.trim_end_matches('/')
                ),
            )
            .header("Authorization", format!("Bearer {}", s.access))
            .json(&json!({"repo": s.did, "collection": collection, "record": record})),
            &self.client.cancel,
        )?;
        let v = self.client.check(&r)?;
        Ok((
            str_of(&v, "uri").unwrap_or_default(),
            str_of(&v, "cid").unwrap_or_default(),
        ))
    }

    fn comment_on(
        &self,
        subject_uri: &str,
        subject_cid: &str,
        body: &str,
        round: Option<u64>,
    ) -> Result<Posted> {
        let mut record = json!({
            "$type": "sh.tangled.feed.comment",
            "subject": {"uri": subject_uri, "cid": subject_cid},
            "body": {"$type": "sh.tangled.markup.markdown", "text": body},
            "createdAt": rfc3339(now_secs()),
        });
        if let Some(r) = round {
            record["pullRoundIdx"] = json!(r);
        }
        let (uri, _) = self.create_record("sh.tangled.feed.comment", record)?;
        Ok(Posted {
            id: uri,
            thread: None,
            url: None,
        })
    }
}

impl Forge for Tangled {
    fn repository(&self) -> &Repository {
        &self.repo
    }

    fn capabilities(&self) -> Capabilities {
        capabilities(Family::Tangled)
    }

    fn pending_mode(&self) -> PendingMode {
        PendingMode::None
    }

    fn account(&self) -> Result<Account> {
        let s = self.session()?;
        Ok(Account {
            login: s.handle.clone(),
            name: None,
            url: Some(format!("https://tangled.org/{}", s.handle)),
        })
    }

    fn pulls(&self, q: &PullQuery) -> Result<Page<PullSummary>> {
        let ids = self.ids()?;
        let mut params = vec![
            ("subject", ids.did.clone()),
            ("limit", q.max.clamp(1, 100).to_string()),
        ];
        match q.state {
            StateFilter::Open => params.push(("status", "open".into())),
            StateFilter::Closed => params.push(("status", "closed".into())),
            StateFilter::Merged => params.push(("status", "merged".into())),
            StateFilter::All => {}
        }
        if let Some(c) = &q.cursor {
            params.push(("cursor", c.clone()));
        }
        let v = self.xrpc("sh.tangled.repo.listPulls", &params)?;
        let items = v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| self.pull_summary(p, &ids))
            .filter(|p| q.matches(p))
            .collect();
        Ok(Page {
            items,
            next_cursor: str_of(&v, "cursor"),
        })
    }

    fn pull(&self, item: &ItemRef) -> Result<Pull> {
        let uri = self.uri(item)?;
        let ids = self.ids()?;
        let v = self.xrpc("sh.tangled.repo.getPull", &[("pull", uri.clone())])?;
        let status = self
            .xrpc(
                "sh.tangled.repo.pull.listStatuses",
                &[
                    ("subject", uri.clone()),
                    ("limit", "1".into()),
                    ("order", "desc".into()),
                ],
            )
            .ok()
            .and_then(|s| {
                s.pointer("/items/0/value/status")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let mut listed = v.clone();
        listed["state"] = json!(match status.as_deref() {
            Some(s) if s.ends_with(".merged") => "merged",
            Some(s) if s.ends_with(".closed") => "closed",
            _ => "open",
        });
        let summary = self
            .pull_summary(&listed, &ids)
            .ok_or_else(|| ForgeError::other("Tangled answered no pull request"))?;
        let version = v
            .pointer("/value/versions")
            .and_then(Value::as_array)
            .and_then(|a| a.last())
            .cloned();
        let head_sha = version.as_ref().and_then(|x| str_of(x, "head"));
        let base_sha = version.as_ref().and_then(|x| str_of(x, "base"));
        let (mut commits, mut files) = (Vec::new(), Vec::new());
        if let (Some(h), Some(b)) = (&head_sha, &base_sha)
            && let Ok(cmp) = self.xrpc(
                "sh.tangled.repo.compare",
                &[
                    ("repo", ids.record.clone()),
                    ("rev1", b.clone()),
                    ("rev2", h.clone()),
                ],
            )
        {
            for p in cmp["format_patch"].as_array().into_iter().flatten() {
                commits.push(Commit {
                    sha: str_of(p, "SHA").unwrap_or_default(),
                    title: first_line(&str_of(p, "Title").unwrap_or_default()),
                    author: str_at(p, "/Author/Name"),
                    date: time_of(p, "AuthorDate"),
                });
            }
            for f in cmp["combined_patch"].as_array().into_iter().flatten() {
                let (mut add, mut del) = (0, 0);
                for frag in f["TextFragments"].as_array().into_iter().flatten() {
                    add += u64_of(frag, "LinesAdded").unwrap_or(0);
                    del += u64_of(frag, "LinesDeleted").unwrap_or(0);
                }
                files.push(FileChange {
                    path: str_of(f, "NewName")
                        .filter(|n| !n.is_empty())
                        .or_else(|| str_of(f, "OldName"))
                        .unwrap_or_default(),
                    old_path: (f["IsRename"] == true)
                        .then(|| str_of(f, "OldName"))
                        .flatten(),
                    status: if f["IsNew"] == true {
                        FileStatus::Added
                    } else if f["IsDelete"] == true {
                        FileStatus::Deleted
                    } else if f["IsRename"] == true {
                        FileStatus::Renamed
                    } else {
                        FileStatus::Modified
                    },
                    additions: add,
                    deletions: del,
                    binary: f["IsBinary"] == true,
                });
            }
        }
        let mut pull = Pull {
            body: str_at(&v, "/value/body"),
            head_sha,
            base_sha,
            commits,
            files,
            conversation: self.comments(&uri)?,
            mergeable: Some(Mergeable {
                state: MergeState::Unknown,
                methods: Vec::new(),
                reason: Some("Tangled merges from its web page (a knot applies the patch)".into()),
            }),
            summary,
            ..Default::default()
        };
        pull.summary.comments = Some(pull.conversation.len() as u64);
        pull.finish();
        pull.summary.checks = None;
        Ok(pull)
    }

    fn comment_pull(&self, item: &ItemRef, c: &DraftComment) -> Result<Posted> {
        if c.path.is_some() || c.reply_to.is_some() {
            return Err(self.unsupported(
                "comments on a line or replies to a thread (comment on the pull request)",
            ));
        }
        let uri = self.uri(item)?;
        let v = self.xrpc("sh.tangled.repo.getPull", &[("pull", uri.clone())])?;
        let round = v
            .pointer("/value/versions")
            .and_then(Value::as_array)
            .or_else(|| v.pointer("/value/rounds").and_then(Value::as_array))
            .map(|a| a.len().saturating_sub(1) as u64)
            .unwrap_or(0);
        self.comment_on(
            &uri,
            &str_of(&v, "cid").unwrap_or_default(),
            &c.body,
            Some(round),
        )
    }

    fn checkout_ref(&self, pull: &Pull) -> CheckoutRef {
        let rkey = pull
            .summary
            .id
            .rsplit('/')
            .next()
            .unwrap_or("pull")
            .to_owned();
        let url = pull.summary.head_repository.as_ref().map(|did| {
            let knot = self
                .xrpc(
                    "sh.tangled.repo.getRepoByRepoDid",
                    &[("repoDid", did.clone())],
                )
                .ok()
                .and_then(|v| str_at(&v, "/value/knot"))
                .unwrap_or_else(|| "knot1.tangled.sh".into());
            format!("https://{knot}/{did}")
        });
        CheckoutRef {
            url,
            refspec: format!("refs/heads/{}", pull.summary.head),
            branch: format!("pr/{rkey}"),
        }
    }

    fn set_pull_state(&self, item: &ItemRef, open: bool) -> Result<State> {
        let uri = self.uri(item)?;
        self.create_record(
            "sh.tangled.repo.pull.status",
            json!({
                "$type": "sh.tangled.repo.pull.status",
                "pull": uri,
                "status": if open { "sh.tangled.repo.pull.status.open" } else { "sh.tangled.repo.pull.status.closed" },
                "createdAt": rfc3339(now_secs()),
            }),
        )?;
        Ok(if open { State::Open } else { State::Closed })
    }

    fn issues(&self, q: &IssueQuery) -> Result<Page<IssueSummary>> {
        let ids = self.ids()?;
        let mut params = vec![
            ("subject", ids.did.clone()),
            ("limit", q.max.clamp(1, 100).to_string()),
        ];
        match q.state {
            IssueStateFilter::Open => params.push(("state", "open".into())),
            IssueStateFilter::Closed => params.push(("state", "closed".into())),
            IssueStateFilter::All => {}
        }
        if let (IssueFilter::Mine, Some(did)) =
            (q.filter, self.cred.as_ref().and_then(|c| c.did.clone()))
        {
            params.push(("author", did));
        }
        if let Some(c) = &q.cursor {
            params.push(("cursor", c.clone()));
        }
        let v = self.xrpc("sh.tangled.repo.listIssues", &params)?;
        let items = v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| self.issue_summary(i))
            .filter(|i| {
                let mut q = q.clone();
                if q.filter == IssueFilter::Mine {
                    q.filter = IssueFilter::All; // the appview filtered by DID already
                }
                q.matches(i)
            })
            .collect();
        Ok(Page {
            items,
            next_cursor: str_of(&v, "cursor"),
        })
    }

    fn issue(&self, item: &ItemRef) -> Result<Issue> {
        let uri = self.uri(item)?;
        let v = self.xrpc("sh.tangled.repo.getIssue", &[("issue", uri.clone())])?;
        let state = self
            .xrpc(
                "sh.tangled.repo.issue.listStates",
                &[
                    ("subject", uri.clone()),
                    ("limit", "1".into()),
                    ("order", "desc".into()),
                ],
            )
            .ok()
            .and_then(|s| {
                s.pointer("/items/0/value/state")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let mut listed = v.clone();
        listed["state"] = json!(
            if state.as_deref().is_some_and(|s| s.ends_with(".closed")) {
                "closed"
            } else {
                "open"
            }
        );
        let summary = self
            .issue_summary(&listed)
            .ok_or_else(|| ForgeError::other("Tangled answered no issue"))?;
        let conversation = self.comments(&uri)?;
        Ok(Issue {
            summary: IssueSummary {
                comments: Some(conversation.len() as u64),
                ..summary
            },
            body: str_at(&v, "/value/body"),
            conversation,
            ..Default::default()
        })
    }

    fn create_issue(&self, new: &NewIssue) -> Result<IssueSummary> {
        let ids = self.ids()?;
        let created = rfc3339(now_secs());
        let (uri, _) = self.create_record(
            "sh.tangled.repo.issue",
            json!({"$type": "sh.tangled.repo.issue", "repo": ids.did, "title": new.title, "body": new.body, "createdAt": created}),
        )?;
        Ok(IssueSummary {
            number: None,
            id: uri.clone(),
            title: new.title.clone(),
            state: IssueState::Open,
            author: self
                .cred
                .as_ref()
                .and_then(|c| c.account.as_ref().map(|a| a.login.clone())),
            created_at: Some(created),
            url: self.web("issues", &uri),
            ..Default::default()
        })
    }

    fn comment_issue(&self, item: &ItemRef, body: &str) -> Result<Posted> {
        let uri = self.uri(item)?;
        let v = self.xrpc("sh.tangled.repo.getIssue", &[("issue", uri.clone())])?;
        self.comment_on(&uri, &str_of(&v, "cid").unwrap_or_default(), body, None)
    }

    fn update_issue(&self, item: &ItemRef, e: &IssueEdit) -> Result<IssueSummary> {
        if e.labels.is_some()
            || e.assignees.is_some()
            || e.milestone.is_some()
            || e.title.is_some()
            || e.body.is_some()
        {
            return Err(self.unsupported(
                "editing an issue's labels, assignees, milestone, title or body from Eludite",
            ));
        }
        let uri = self.uri(item)?;
        if let Some(s) = e.state {
            self.create_record(
                "sh.tangled.repo.issue.state",
                json!({
                    "$type": "sh.tangled.repo.issue.state",
                    "issue": uri,
                    "state": if s == IssueState::Closed { "sh.tangled.repo.issue.state.closed" } else { "sh.tangled.repo.issue.state.open" },
                    "createdAt": rfc3339(now_secs()),
                }),
            )?;
        }
        let mut i = self.issue(item)?.summary;
        if let Some(s) = e.state {
            i.state = s;
        }
        Ok(i)
    }

    fn link_branch(&self, item: &ItemRef, branch: &str, _commit: &str) -> Result<bool> {
        self.comment_issue(
            item,
            &format!("Created branch `{branch}` for this issue (Eludite)."),
        )?;
        Ok(true)
    }
}
