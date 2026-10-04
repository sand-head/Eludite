//! Forges in the shell (brief 0046): the Pull Requests and Issues windows (View > Other Windows), the pull request
//! document, the Create Pull Request form, the sign-in dialog, the Checks log document, review threads in the
//! editor's margin, the status bar's pull request slot and the Git Changes window's "Create a Pull Request" link,
//! over `eludite-forge` and `eludite.forge.*`.
//!
//! - **One service.** [`ForgeService`] implements `eludite.forge.*` on the invoking thread through
//!   `eludite_forge::ops` (an agent's call on the agent's thread, the person's on a background thread, never the UI
//!   thread), with [`ShellGit`] for what needs the repository (the remote, the current branch, a checkout, a new
//!   branch), through `eludite.git.*` and `eludite-git`.
//! - **Every action is a command.** The windows' buttons dispatch `eludite.forge.*`; [`Shell::run_forge`] opens the
//!   form, the dialog and the documents for the commands that have one, asks before a merge or a close, and runs the
//!   rest off the UI thread. Answers older than the repository's generation are dropped (invariant 12).
//! - **Offline first.** Reads answer from the cache on disk (`.eludite/forge/`) at once and refresh off-thread; the
//!   hub's events redraw the windows when a refresh lands; an answer younger than the hub's freshness window is not
//!   refreshed again. Nothing reaches a forge until a window opens or a command runs: not at startup, and the status
//!   bar shows a pull request (and reads its checks) only once a list has been read.

pub mod create;
pub mod document;
pub mod issues;
pub mod log;
pub mod margin;
pub mod pulls;
pub mod signin;
pub mod widgets;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use eludite_commands::forge::{self as cmds, ForgeCommands};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_forge::cache::Cache;
use eludite_forge::credentials::Credentials;
use eludite_forge::http::{Transport, UreqTransport};
use eludite_forge::hub::{Hub, HubConfig, HubEvent, Listener};
use eludite_forge::ops::{self, GitSide};
use eludite_forge::{Capabilities, Family, ItemRef, MergeMethod, Pull, Repository};
use eludite_ui::slots;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui::{AnyView, AppContext as _, Context, Entity, PromptLevel, Task, Window};
use serde_json::{Value, json};

use super::Shell;
use super::documents::normalize_path;
use super::git::service::GitService;

/// The status bar slot of the current branch's pull request and its checks.
pub const PULL_SLOT: &str = "forge_pull";
/// Document tab ids.
pub const PULL_PREFIX: &str = "forge-pull:";
pub const LOG_PREFIX: &str = "forge-log:";
pub const CREATE_TAB: &str = "forge-create-pull";

/// The tab of pull request `id`.
pub fn pull_tab(id: &str) -> String {
    format!("{PULL_PREFIX}{id}")
}

/// Documents the forge draws, by tab id.
pub type Documents = Rc<RefCell<HashMap<String, AnyView>>>;
/// Review thread marks over open editors, by document id.
pub type Margins = Rc<RefCell<HashMap<String, Entity<margin::ThreadMarks>>>>;

/// What `eludite.forge.detect` answered, as the windows use it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detected {
    pub family: Family,
    pub host: String,
    pub path: String,
    pub signed_in: bool,
    pub account: Option<String>,
    pub capabilities: Capabilities,
    /// "No supported forge for <remote>".
    pub message: Option<String>,
}

impl Detected {
    pub fn from_json(v: &Value) -> Self {
        Self {
            family: v
                .pointer("/repository/family")
                .and_then(Value::as_str)
                .and_then(Family::parse)
                .unwrap_or_default(),
            host: v
                .pointer("/repository/host")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            path: v
                .pointer("/repository/path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            signed_in: v["signed_in"] == true,
            account: v
                .pointer("/account/login")
                .and_then(Value::as_str)
                .map(str::to_owned),
            capabilities: serde_json::from_value(v["capabilities"].clone()).unwrap_or_default(),
            message: v["message"].as_str().map(str::to_owned),
        }
    }

    pub fn supported(&self) -> bool {
        self.family != Family::None
    }
}

/// Where a list's answer came from, for the windows' banner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    pub stale: bool,
    pub fetched_at: Option<String>,
    pub age_seconds: u64,
    pub error: Option<String>,
}

impl Provenance {
    pub fn from_json(v: &Value) -> Self {
        Self {
            stale: v["stale"] == true,
            fetched_at: v["fetched_at"].as_str().map(str::to_owned),
            age_seconds: v["age_seconds"].as_u64().unwrap_or(0),
            error: v
                .pointer("/refresh_error/message")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }

    /// The banner: "Showing cached results from <time>" and why, or nothing for a fresh answer.
    pub fn banner(&self, refreshing: bool) -> Option<String> {
        if !self.stale {
            return None;
        }
        let when = self.fetched_at.clone().unwrap_or_default();
        let age = widgets::age(self.age_seconds);
        Some(match (&self.error, refreshing) {
            (Some(e), _) => format!("Showing cached results from {when} ({age}): {e}"),
            (None, true) => {
                format!("Showing cached results from {when} ({age}); refreshing\u{2026}")
            }
            (None, false) => format!("Showing cached results from {when} ({age})"),
        })
    }
}

// ----- The service -----

/// What the commands need of the workspace repository, over the git service and `eludite.git.*`.
pub struct ShellGit {
    pub git: Arc<GitService>,
    /// The bus `eludite.git.checkout` runs through (set once the shell holds it).
    pub registry: Arc<Mutex<Weak<CommandRegistry>>>,
}

impl ShellGit {
    fn registry(&self) -> Result<Arc<CommandRegistry>, String> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .upgrade()
            .ok_or_else(|| "the window is closed".to_owned())
    }

    fn repo(&self) -> Option<eludite_git::git2::Repository> {
        self.git.repository()?.repository().ok()
    }
}

impl GitSide for ShellGit {
    fn remote(&self, remote: Option<&str>) -> Option<(String, String)> {
        let repo = self.repo()?;
        let names: Vec<String> = repo
            .remotes()
            .ok()?
            .iter()
            .flatten()
            .map(str::to_owned)
            .collect();
        let name = match remote {
            Some(r) => names.iter().find(|n| *n == r)?.clone(),
            None => names
                .iter()
                .find(|n| *n == "origin")
                .or_else(|| (names.len() == 1).then(|| &names[0]))?
                .clone(),
        };
        let url = repo.find_remote(&name).ok()?.url()?.to_owned();
        Some((name, url))
    }

    fn current_branch(&self) -> Option<String> {
        let repo = self.repo()?;
        let head = repo.head().ok()?;
        head.is_branch()
            .then(|| head.shorthand().map(str::to_owned))
            .flatten()
    }

    fn resolve(&self, revision: Option<&str>) -> Option<String> {
        let repo = self.repo()?;
        let obj = repo.revparse_single(revision.unwrap_or("HEAD")).ok()?;
        Some(obj.peel_to_commit().ok()?.id().to_string())
    }

    fn commits_between(&self, base: &str, head: &str) -> Vec<(String, String)> {
        let Some(repo) = self.repo() else {
            return Vec::new();
        };
        let Ok(mut walk) = repo.revwalk() else {
            return Vec::new();
        };
        let Ok(head) = repo.revparse_single(head).and_then(|o| o.peel_to_commit()) else {
            return Vec::new();
        };
        let _ = walk.push(head.id());
        if let Ok(b) = repo.revparse_single(base).and_then(|o| o.peel_to_commit()) {
            let _ = walk.hide(b.id());
        }
        walk.flatten()
            .take(50)
            .filter_map(|oid| {
                let c = repo.find_commit(oid).ok()?;
                Some((oid.to_string(), c.message().unwrap_or("").to_owned()))
            })
            .collect()
    }

    fn default_branch(&self, remote: &str) -> Option<String> {
        let repo = self.repo()?;
        if let Ok(r) = repo.find_reference(&format!("refs/remotes/{remote}/HEAD"))
            && let Some(target) = r.symbolic_target()
        {
            return target
                .strip_prefix(&format!("refs/remotes/{remote}/"))
                .map(str::to_owned);
        }
        ["main", "master", "trunk", "develop"]
            .into_iter()
            .find(|b| {
                repo.find_reference(&format!("refs/remotes/{remote}/{b}"))
                    .is_ok()
            })
            .map(str::to_owned)
    }

    fn fetch_and_checkout(
        &self,
        remote: &str,
        url: Option<&str>,
        refspec: &str,
        branch: &str,
    ) -> Result<(String, u64), String> {
        let repo = self
            .git
            .repository()
            .ok_or("the workspace is in no Git repository")?;
        let dest = match url {
            Some(_) => format!("refs/remotes/forge/{branch}"),
            None => format!("refs/remotes/{remote}/{branch}"),
        };
        let fetched = repo
            .fetch_refspec(
                Some(remote),
                url,
                &format!("+{refspec}:{dest}"),
                &eludite_git::Cancel::new(),
            )
            .map_err(|e| e.message)?;
        let registry = self.registry()?;
        let exists = self.repo().is_some_and(|r| {
            r.find_branch(branch, eludite_git::git2::BranchType::Local)
                .is_ok()
        });
        let out = if exists {
            let out = registry
                .invoke("eludite.git.checkout", json!({"name": branch}))
                .map_err(|e| e.to_string())?;
            // Update after a push: bring the branch to the head fetched (a fast-forward).
            let short = dest.trim_start_matches("refs/remotes/");
            match registry.invoke("eludite.git.merge", json!({"branch": short})) {
                Ok(m) => m,
                Err(_) => out,
            }
        } else {
            registry
                .invoke(
                    "eludite.git.checkout",
                    json!({"name": branch, "create": true, "start_point": fetched.to_string()}),
                )
                .map_err(|e| e.to_string())?
        };
        self.git.touch();
        Ok((
            self.resolve(None).unwrap_or_else(|| fetched.to_string()),
            out["generation"].as_u64().unwrap_or(0),
        ))
    }

    fn create_branch(
        &self,
        name: &str,
        base: Option<&str>,
        checkout: bool,
    ) -> Result<String, String> {
        if checkout {
            let registry = self.registry()?;
            let mut args = json!({"name": name, "create": true});
            if let Some(b) = base {
                args["start_point"] = json!(b);
            }
            let out = registry
                .invoke("eludite.git.checkout", args)
                .map_err(|e| e.to_string())?;
            return Ok(out["commit"].as_str().unwrap_or_default().to_owned());
        }
        let repo = self.repo().ok_or("the workspace is in no Git repository")?;
        let commit = repo
            .revparse_single(base.unwrap_or("HEAD"))
            .and_then(|o| o.peel_to_commit())
            .map_err(|e| e.message().to_owned())?;
        repo.branch(name, &commit, false)
            .map_err(|e| e.message().to_owned())?;
        Ok(commit.id().to_string())
    }
}

/// `eludite.forge.*` over the hub.
pub struct ForgeService {
    hub: RwLock<Arc<Hub>>,
    pub git: Arc<ShellGit>,
    /// Tells the UI thread what the hub did; given to each hub.
    listener: Listener,
}

impl ForgeService {
    pub fn hub(&self) -> Arc<Hub> {
        self.hub.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Reach forges through `setup` from now on, keeping the configuration and the cache (tests give the fixture
    /// server and a memory store).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_setup(&self, setup: ForgeSetup) {
        let old = self.hub();
        let hub = Hub::new(setup.transport, setup.credentials);
        hub.set_config(old.config());
        hub.set_cache(old.cache());
        hub.add_listener(self.listener.clone());
        *self.hub.write().unwrap_or_else(|e| e.into_inner()) = hub;
    }
}

impl ForgeCommands for ForgeService {
    fn apply(&self, id: &'static str, input: Value) -> Result<Value, CommandError> {
        ops::run(&self.hub(), &*self.git, id, &input).map_err(|e| CommandError::Failed(e.message))
    }

    fn audit_arguments(&self, _id: &str, input: &Value) -> Value {
        let repo = self
            .git
            .remote(input.get("remote").and_then(Value::as_str))
            .map(|(_, url)| self.hub().detect(&url, false));
        ops::audit_arguments(input, repo.as_ref())
    }
}

/// How the shell's forge commands reach the network and keep tokens (tests give the fixture server and a memory
/// store).
pub struct ForgeSetup {
    pub transport: Arc<dyn Transport>,
    pub credentials: Credentials,
}

impl ForgeSetup {
    /// The real transport and the operating system's credential store (with the consented 0600 file).
    pub fn system() -> Self {
        Self {
            transport: Arc::new(UreqTransport::default()),
            credentials: Credentials::system(),
        }
    }
}

/// Register `eludite.forge.*` on `commands`. Nothing reaches a forge here.
pub fn register(
    commands: &CommandRegistry,
    git: Arc<GitService>,
    setup: ForgeSetup,
) -> (
    Arc<ForgeService>,
    UnboundedReceiver<HubEvent>,
    Arc<Mutex<Weak<CommandRegistry>>>,
) {
    let hub = Hub::new(setup.transport, setup.credentials);
    let (tx, rx) = unbounded();
    let tx = Mutex::new(tx);
    let listener: Listener = Arc::new(move |e: &HubEvent| {
        let _ = tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unbounded_send(e.clone());
    });
    hub.add_listener(listener.clone());
    let registry: Arc<Mutex<Weak<CommandRegistry>>> = Arc::default();
    let service = Arc::new(ForgeService {
        hub: RwLock::new(hub),
        listener,
        git: Arc::new(ShellGit {
            git,
            registry: registry.clone(),
        }),
    });
    cmds::register(commands, service.clone());
    (service, rx, registry)
}

/// When things happened (the report's numbers).
#[derive(Debug, Default, Clone)]
pub struct ForgeTimings {
    /// From the window's first draw asking for its list to the cached list drawn.
    pub list_open: Vec<Duration>,
    /// From the command to the pull request document drawn.
    pub document_open: Vec<Duration>,
}

/// The forge half of the shell.
pub struct ForgeUi {
    pub service: Arc<ForgeService>,
    pub pulls: Entity<pulls::PullsWindow>,
    pub issues: Entity<issues::IssuesWindow>,
    pub documents: Documents,
    pub margins: Margins,
    pub pull_documents: HashMap<String, Entity<document::PullDocument>>,
    pub create: Option<Entity<create::CreatePullForm>>,
    pub signin: Option<Entity<signin::SignInDialog>>,
    /// What the windows show the forge as.
    pub detected: Option<Detected>,
    /// The repository's generation on screen: answers of an older one are dropped.
    pub generation: u64,
    /// The current branch's pull request (from the cache) and its checks' state, for the status bar.
    pub current: Option<(Value, String)>,
    pub refresh_seconds: u64,
    refresh_timer: Option<Task<()>>,
    signin_poll: Option<Task<()>>,
    pub timings: ForgeTimings,
    /// The Git Changes window's link: after a push, or while the branch is ahead of its upstream.
    pub link: bool,
    /// The registry `eludite.git.checkout` goes through (set at install).
    pub registry_slot: Arc<Mutex<Weak<CommandRegistry>>>,
}

impl ForgeUi {
    pub fn new(
        service: Arc<ForgeService>,
        registry_slot: Arc<Mutex<Weak<CommandRegistry>>>,
        theme: eludite_ui::Theme,
        cx: &mut Context<Shell>,
    ) -> Self {
        Self {
            pulls: cx.new(|cx| pulls::PullsWindow::new(theme, cx)),
            issues: cx.new(|cx| issues::IssuesWindow::new(theme, cx)),
            service,
            documents: Rc::default(),
            margins: Rc::default(),
            pull_documents: HashMap::new(),
            create: None,
            signin: None,
            detected: None,
            generation: 1,
            current: None,
            refresh_seconds: 0,
            refresh_timer: None,
            signin_poll: None,
            timings: ForgeTimings::default(),
            link: false,
            registry_slot,
        }
    }
}

/// What to do with a background command's answer.
#[derive(Debug, Clone)]
enum Then {
    Pulls {
        asked: Instant,
    },
    Issues,
    Pull {
        tab: String,
        asked: Instant,
    },
    Issue,
    Write {
        tab: Option<String>,
        message: String,
    },
    Created,
    Checkout,
    Log {
        tab: String,
    },
    Auth,
    Lists,
}

/// The question asked before a merge or a close.
pub fn confirmation(command: &str, args: &Value, family: Family) -> Option<(String, String)> {
    let item = match (args["number"].as_u64(), args["id"].as_str()) {
        (Some(n), _) => format!("#{n}"),
        (None, Some(id)) => id.rsplit('/').next().unwrap_or(id).to_owned(),
        _ => String::new(),
    };
    let noun = family.pull_noun();
    match command {
        cmds::PULL_MERGE => {
            let method = args["method"]
                .as_str()
                .and_then(MergeMethod::parse)
                .unwrap_or(MergeMethod::Merge);
            Some(if args["when_checks_pass"] == true {
                (
                    format!(
                        "Set {noun} {item} to merge when its checks pass ({})?",
                        method.label()
                    ),
                    "The forge merges it as soon as its required checks succeed.".into(),
                )
            } else {
                (
                    format!("Merge {noun} {item}: {}?", method.label()),
                    "The base branch changes on the forge; this cannot be undone from Eludite."
                        .into(),
                )
            })
        }
        cmds::PULL_CLOSE if args["reopen"] != true => Some((
            format!("Close {noun} {item} without merging?"),
            "It can be reopened later.".into(),
        )),
        _ => None,
    }
}

impl Shell {
    /// Wire the forge windows and status bar (once, at the end of [`Shell::new`]).
    pub(super) fn forge_install(
        &mut self,
        mut events: UnboundedReceiver<HubEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use futures::StreamExt as _;
        *self
            .forge
            .registry_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&self.commands);
        self.status
            .add_slot(PULL_SLOT, eludite_ui::SlotAlign::Right);
        cx.observe(&self.forge.pulls, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&self.forge.issues, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe_in(
            &self.forge.pulls,
            window,
            |shell, _, e: &pulls::PullsEvent, window, cx| match e {
                pulls::PullsEvent::Load => shell.forge_load_pulls(false, window, cx),
                pulls::PullsEvent::Refresh => shell.forge_load_pulls(true, window, cx),
                pulls::PullsEvent::Open(item) => shell.forge_open_pull(item.clone(), window, cx),
                pulls::PullsEvent::SignIn => shell.forge_open_signin(None, window, cx),
            },
        )
        .detach();
        cx.subscribe_in(
            &self.forge.issues,
            window,
            |shell, _, e: &issues::IssuesEvent, window, cx| match e {
                issues::IssuesEvent::Load => shell.forge_load_issues(false, window, cx),
                issues::IssuesEvent::Refresh => shell.forge_load_issues(true, window, cx),
                issues::IssuesEvent::SignIn => shell.forge_open_signin(None, window, cx),
            },
        )
        .detach();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        for e in batch {
                            shell.on_forge_event(e, window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn forge(&self) -> &ForgeUi {
        &self.forge
    }

    fn on_forge_event(&mut self, event: HubEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            HubEvent::Refreshed { key, .. } | HubEvent::RefreshFailed { key, .. } => {
                if key.starts_with("pulls:") {
                    self.forge_load_pulls(false, window, cx);
                } else if key.starts_with("issues:") {
                    self.forge_load_issues(false, window, cx);
                } else if let Some(id) = key.strip_prefix("pull:") {
                    let tab = pull_tab(id);
                    if self.forge.pull_documents.contains_key(&tab) {
                        self.forge_reload_pull(&tab, false, window, cx);
                    }
                }
                self.forge_update_status(cx);
            }
            HubEvent::SignedIn { host, result } => {
                let text = match &result {
                    Ok(a) => format!("Signed in to {host} as {}", a.login),
                    Err(e) => format!("Sign-in to {host} failed: {e}"),
                };
                if let Some(d) = &self.forge.signin {
                    d.update(cx, |d, cx| d.finished(result.clone().map(|a| a.login), cx));
                }
                if result.is_ok() {
                    self.forge.signin = None;
                    self.forge.signin_poll = None;
                    self.forge.detected = None;
                    self.forge_load_pulls(false, window, cx);
                    self.forge_load_issues(false, window, cx);
                }
                self.status.set(slots::STATE, text);
                cx.notify();
            }
        }
    }

    /// The workspace changed: its cache, a new generation, nothing fetched.
    pub(super) fn forge_workspace_changed(&mut self, folder: Option<&Path>) {
        self.forge
            .service
            .hub()
            .set_cache(folder.map(Cache::for_workspace));
        self.forge.generation += 1;
        self.forge.detected = None;
        self.forge.current = None;
        self.status.set(PULL_SLOT, "");
    }

    /// `forge.*` from the settings store.
    pub(super) fn forge_apply_settings(&mut self, cx: &mut Context<Self>) {
        let (hosts, refresh, gh, gl, az) = {
            let s = self.settings.lock();
            let get = |k: &str| Some(s.effective(k).0);
            (
                get("forge.hosts"),
                get("forge.refreshSeconds"),
                get("forge.githubClientId"),
                get("forge.gitlabApplicationId"),
                get("forge.azureApplicationId"),
            )
        };
        let text = |v: Option<Value>| {
            v.and_then(|v| v.as_str().map(str::to_owned))
                .filter(|s| !s.is_empty())
        };
        let config = HubConfig {
            hosts: hosts
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            github_client_id: text(gh),
            gitlab_application_id: text(gl),
            azure_application_id: text(az),
        };
        self.forge.service.hub().set_config(config);
        let seconds = refresh.and_then(|v| v.as_u64()).unwrap_or(0);
        if seconds != self.forge.refresh_seconds {
            self.forge.refresh_seconds = seconds;
            self.forge_schedule_refresh(cx);
        }
    }

    /// The refresh timer (`forge.refreshSeconds`): only while a forge window is visible.
    fn forge_schedule_refresh(&mut self, cx: &mut Context<Self>) {
        self.forge.refresh_timer = None;
        let seconds = self.forge.refresh_seconds;
        if seconds == 0 {
            return;
        }
        self.forge.refresh_timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(seconds))
                    .await;
                let Ok(()) = this.update(cx, |shell, cx| {
                    let layout = shell.controller.layout();
                    let pulls = shown(&layout, eludite_docking::ids::PULL_REQUESTS);
                    let issues = shown(&layout, eludite_docking::ids::ISSUES);
                    if pulls {
                        shell.forge.pulls.update(cx, |w, cx| w.request_refresh(cx));
                    }
                    if issues {
                        shell.forge.issues.update(cx, |w, cx| w.request_refresh(cx));
                    }
                }) else {
                    break;
                };
            }
        }));
    }

    /// A `eludite.forge.*` command from the UI. Returns false for other commands.
    pub(super) fn run_forge(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !cmds::ALL.contains(&command) {
            return false;
        }
        let args = std::mem::take(args);
        match command {
            // Git > Create Pull Request: the form, prefilled.
            cmds::PULL_CREATE if args.get("title").is_none() => {
                self.forge_open_create(window, cx);
                return true;
            }
            // Git > Sign in to the forge: the dialog.
            cmds::AUTH
                if args["action"] == "sign_in"
                    && args.get("token").is_none()
                    && args.get("method").is_none() =>
            {
                self.forge_open_signin(args["host"].as_str().map(str::to_owned), window, cx);
                return true;
            }
            // Opening a pull request (a double-click, the status bar): its document.
            cmds::PULL => {
                if let Ok(item) = item_of(&args) {
                    self.forge_open_pull(item, window, cx);
                }
                return true;
            }
            cmds::CHECK_LOG => {
                if let Some(id) = args["id"].as_str() {
                    self.forge_open_log(
                        id.to_owned(),
                        args["name"].as_str().unwrap_or("Log").to_owned(),
                        window,
                        cx,
                    );
                }
                return true;
            }
            _ => {}
        }
        let family = self
            .forge
            .detected
            .as_ref()
            .map(|d| d.family)
            .unwrap_or_default();
        if let Some((message, detail)) = confirmation(command, &args, family) {
            let answer = window.prompt(
                PromptLevel::Warning,
                &message,
                Some(&detail),
                &["Yes", "No"],
                cx,
            );
            let command = command.to_owned();
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(0) = answer.await {
                    let _ = this.update_in(cx, |shell, window, cx| {
                        let tab = tab_of(&args);
                        shell.forge_spawn(
                            command,
                            args,
                            Then::Write {
                                tab,
                                message: String::new(),
                            },
                            window,
                            cx,
                        )
                    });
                }
            })
            .detach();
            return true;
        }
        let then = match command {
            cmds::PULL_CHECKOUT => Then::Checkout,
            cmds::ISSUES => Then::Issues,
            cmds::PULLS => Then::Pulls {
                asked: Instant::now(),
            },
            cmds::ISSUE => Then::Issue,
            cmds::AUTH => Then::Auth,
            cmds::REFRESH if tab_of(&args).is_some() => Then::Write {
                tab: tab_of(&args),
                message: "Refreshed".into(),
            },
            cmds::REFRESH => Then::Lists,
            _ => Then::Write {
                tab: tab_of(&args),
                message: String::new(),
            },
        };
        self.forge_spawn(command.to_owned(), args, then, window, cx);
        true
    }

    /// Invoke `command` off the UI thread; apply its answer on it, unless the generation moved on.
    fn forge_spawn(
        &mut self,
        command: String,
        args: Value,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = self.commands.clone();
        let generation = self.forge.generation;
        let work = {
            let (command, args) = (command.clone(), args.clone());
            cx.background_spawn(async move {
                let detected = match &then_needs_detect(&command) {
                    true => commands.invoke(cmds::DETECT, json!({})).ok(),
                    false => None,
                };
                (commands.invoke(&command, args), detected)
            })
        };
        cx.spawn_in(window, async move |this, cx| {
            let (result, detected) = work.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                if shell.forge.generation != generation {
                    return; // the workspace changed while it ran
                }
                if let Some(d) = detected {
                    shell.forge_set_detected(Detected::from_json(&d), cx);
                }
                shell.forge_finished(&command, &args, result, then, window, cx)
            });
        })
        .detach();
    }

    fn forge_set_detected(&mut self, d: Detected, cx: &mut Context<Self>) {
        self.forge
            .pulls
            .update(cx, |w, cx| w.set_detected(d.clone(), cx));
        self.forge
            .issues
            .update(cx, |w, cx| w.set_detected(d.clone(), cx));
        self.forge.detected = Some(d);
    }

    #[allow(clippy::too_many_arguments)]
    fn forge_finished(
        &mut self,
        command: &str,
        args: &Value,
        result: Result<Value, CommandError>,
        then: Then,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let error = result.as_ref().err().map(|e| match e {
            CommandError::Failed(m) | CommandError::InvalidInput(m) => m.clone(),
            other => other.to_string(),
        });
        match then {
            Then::Pulls { asked } => {
                let first = self.forge.pulls.read(cx).items.is_empty();
                self.forge.pulls.update(cx, |w, cx| match &result {
                    Ok(v) => w.set_answer(v, cx),
                    Err(_) => w.set_error(error.clone().unwrap_or_default(), cx),
                });
                if first && result.is_ok() {
                    self.forge.timings.list_open.push(asked.elapsed());
                }
                self.forge_update_status(cx);
            }
            Then::Issues => self.forge.issues.update(cx, |w, cx| match &result {
                Ok(v) => w.set_answer(v, cx),
                Err(_) => w.set_error(error.clone().unwrap_or_default(), cx),
            }),
            Then::Issue => self.forge.issues.update(cx, |w, cx| match &result {
                Ok(v) => w.set_issue(v, cx),
                Err(_) => w.set_error(error.clone().unwrap_or_default(), cx),
            }),
            Then::Pull { tab, asked } => {
                if let Some(doc) = self.forge.pull_documents.get(&tab).cloned() {
                    match &result {
                        Ok(v) => {
                            let pull: Option<Pull> = serde_json::from_value(v.clone()).ok();
                            let prov = Provenance::from_json(v);
                            let detected = self.forge.detected.clone().unwrap_or_default();
                            doc.update(cx, |d, cx| d.set_pull(pull, prov, detected, cx));
                            self.forge.timings.document_open.push(asked.elapsed());
                            self.forge_update_margins(cx);
                        }
                        Err(_) => doc.update(cx, |d, cx| {
                            d.set_message(error.clone().unwrap_or_default(), true, cx)
                        }),
                    }
                }
            }
            Then::Write { tab, message } => {
                let text = match (&result, command) {
                    (Err(_), _) => error.clone().unwrap_or_default(),
                    (Ok(_), _) if !message.is_empty() => message,
                    (Ok(v), cmds::PULL_MERGE) => {
                        if v["auto_merge"] == true {
                            "Set to merge when its checks pass".to_owned()
                        } else {
                            format!("Merged ({})", v["method"].as_str().unwrap_or_default())
                        }
                    }
                    (Ok(v), cmds::PULL_REVIEW) => v["message"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("{} pending comment(s)", v["pending_comments"])),
                    (Ok(v), cmds::BRANCH_FROM_ISSUE) => format!(
                        "Created branch {}{}",
                        v["branch"].as_str().unwrap_or_default(),
                        if v["linked"] == true {
                            " and linked it to the issue"
                        } else {
                            ""
                        }
                    ),
                    (Ok(v), cmds::ISSUE_CREATE) => {
                        format!("Created {}", v["url"].as_str().unwrap_or_default())
                    }
                    (Ok(_), c) => format!("{}: done", c.rsplit('.').next().unwrap_or(c)),
                };
                if let Some(tab) = &tab
                    && let Some(doc) = self.forge.pull_documents.get(tab).cloned()
                {
                    let failed = result.is_err();
                    doc.update(cx, |d, cx| {
                        d.after_write(command, &result, text.clone(), failed, cx)
                    });
                    if !failed {
                        // A refresh stored the answer; a write changed the forge, so read it again.
                        self.forge_reload_pull(tab, command != cmds::REFRESH, window, cx);
                    }
                }
                if command.contains("issue") || command == cmds::BRANCH_FROM_ISSUE {
                    self.forge.issues.update(cx, |w, cx| {
                        w.set_message(Some((text.clone(), result.is_err())), cx);
                        if result.is_ok() {
                            w.request_refresh(cx);
                        }
                    });
                }
                self.status.set(slots::STATE, text);
            }
            Then::Created => match &result {
                Ok(v) => {
                    let item = match v["number"].as_u64() {
                        Some(n) => ItemRef::Number(n),
                        None => ItemRef::Id(v["id"].as_str().unwrap_or_default().to_owned()),
                    };
                    self.status.set(
                        slots::STATE,
                        format!(
                            "Created {} {}",
                            self.forge
                                .detected
                                .as_ref()
                                .map(|d| d.family.pull_noun())
                                .unwrap_or("pull request"),
                            item.label()
                        ),
                    );
                    self.controller.close_document(CREATE_TAB);
                    self.forge.documents.borrow_mut().remove(CREATE_TAB);
                    self.forge.create = None;
                    self.forge.link = false;
                    self.forge_update_link(cx);
                    self.forge.pulls.update(cx, |w, cx| w.request_refresh(cx));
                    self.forge_open_pull(item, window, cx);
                }
                Err(_) => {
                    if let Some(f) = &self.forge.create {
                        f.update(cx, |f, cx| {
                            f.set_message(error.clone().unwrap_or_default(), cx)
                        });
                    }
                    self.status
                        .set(slots::STATE, error.clone().unwrap_or_default());
                }
            },
            Then::Checkout => {
                let text = match &result {
                    Ok(v) => format!(
                        "Checked out {} ({})",
                        v["branch"].as_str().unwrap_or_default(),
                        v["ref"].as_str().unwrap_or_default()
                    ),
                    Err(_) => error.clone().unwrap_or_default(),
                };
                if let Some(tab) = tab_of(args)
                    && let Some(doc) = self.forge.pull_documents.get(&tab).cloned()
                {
                    doc.update(cx, |d, cx| d.set_message(text.clone(), result.is_err(), cx));
                }
                self.forge_update_margins(cx);
                self.status.set(slots::STATE, text);
            }
            Then::Log { tab } => {
                if let Some(view) = self.forge.documents.borrow().get(&tab).cloned()
                    && let Ok(log) = view.downcast::<log::CheckLog>()
                {
                    log.update(cx, |l, cx| match &result {
                        Ok(v) => l.set_text(v, cx),
                        Err(_) => l.set_error(error.clone().unwrap_or_default(), cx),
                    });
                }
            }
            Then::Auth => {
                if let Some(d) = &self.forge.signin {
                    d.update(cx, |d, cx| d.set_answer(&result, cx));
                }
                match &result {
                    Ok(v) if v["pending"] == true => self.forge_poll_signin(window, cx),
                    Ok(v) if v["signed_in"] == true && args["action"] == "sign_in" => {
                        self.forge.signin = None;
                        self.forge.detected = None;
                        self.status.set(
                            slots::STATE,
                            v["message"].as_str().unwrap_or("Signed in").to_owned(),
                        );
                        self.forge_load_pulls(false, window, cx);
                        self.forge_load_issues(false, window, cx);
                    }
                    Ok(v) if args["action"] == "sign_out" => {
                        self.status.set(
                            slots::STATE,
                            v["message"].as_str().unwrap_or("Signed out").to_owned(),
                        );
                        self.forge.detected = None;
                    }
                    Err(_) => {
                        self.status
                            .set(slots::STATE, error.clone().unwrap_or_default());
                    }
                    _ => {}
                }
            }
            Then::Lists => {
                self.forge_load_pulls(false, window, cx);
                self.forge_load_issues(false, window, cx);
            }
        }
        cx.notify();
    }

    /// The Pull Requests window's list: the cache at once, a refresh off-thread (or now, with `refresh`).
    fn forge_load_pulls(&mut self, refresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut args = self.forge.pulls.read(cx).query();
        if refresh {
            args["refresh"] = json!(true);
        }
        self.forge.pulls.update(cx, |w, cx| w.set_loading(cx));
        self.forge_spawn(
            cmds::PULLS.into(),
            args,
            Then::Pulls {
                asked: Instant::now(),
            },
            window,
            cx,
        );
    }

    fn forge_load_issues(&mut self, refresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut args = self.forge.issues.read(cx).query();
        if refresh {
            args["refresh"] = json!(true);
        }
        self.forge.issues.update(cx, |w, cx| w.set_loading(cx));
        self.forge_spawn(cmds::ISSUES.into(), args, Then::Issues, window, cx);
    }

    /// Open a pull request's document (or show it), reading it from the cache and refreshing.
    pub(super) fn forge_open_pull(
        &mut self,
        item: ItemRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = pull_tab(&item.id());
        let theme = self.theme;
        if !self.forge.pull_documents.contains_key(&tab) {
            let detected = self.forge.detected.clone().unwrap_or_default();
            let doc = cx.new(|cx| document::PullDocument::new(theme, item.clone(), detected, cx));
            cx.subscribe_in(
                &doc,
                window,
                |shell, _, e: &document::DocumentEvent, window, cx| match e {
                    document::DocumentEvent::Compare { path, base, head } => {
                        shell.forge_compare(path.clone(), base.clone(), head.clone(), window, cx)
                    }
                    document::DocumentEvent::OpenLink(url) => cx.open_url(url),
                },
            )
            .detach();
            self.forge
                .documents
                .borrow_mut()
                .insert(tab.clone(), doc.clone().into());
            self.forge.pull_documents.insert(tab.clone(), doc);
        }
        let noun = self
            .forge
            .detected
            .as_ref()
            .map(|d| d.family.pull_noun())
            .unwrap_or("pull request");
        let title = format!("{} {}", capitalize(noun), item.label());
        self.controller.open_document(&tab, &title);
        self.dock.update(cx, |_, cx| cx.notify());
        self.forge_reload_pull(&tab, false, window, cx);
    }

    fn forge_reload_pull(
        &mut self,
        tab: &str,
        refresh: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.forge.pull_documents.get(tab) else {
            return;
        };
        let item = doc.read(cx).item.clone();
        let mut args = item_args(&item);
        if refresh {
            args["refresh"] = json!(true);
        }
        self.forge_spawn(
            cmds::PULL.into(),
            args,
            Then::Pull {
                tab: tab.to_owned(),
                asked: Instant::now(),
            },
            window,
            cx,
        );
    }

    /// Compare a changed file's base and head texts (brief 0040's Compare document), read off the UI thread from the
    /// local repository: the pull request's commits must have been fetched (Check Out does that).
    fn forge_compare(
        &mut self,
        path: String,
        base: Option<String>,
        head: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let git = self.forge.service.git.git.clone();
        let (b, h) = (base.clone(), head.clone());
        let p = path.clone();
        let work = cx.background_spawn(async move {
            let repo = git.repository().ok_or("the workspace is in no Git repository")?;
            let read = |rev: &Option<String>| -> Result<(String, bool), String> {
                let Some(rev) = rev else { return Ok((String::new(), false)) };
                match repo.revision_blob(rev, &p) {
                    Ok(Some(bytes)) => {
                        let binary = bytes.contains(&0);
                        Ok((String::from_utf8_lossy(&bytes).into_owned(), binary))
                    }
                    Ok(None) => Ok((String::new(), false)),
                    Err(_) => Err(format!(
                        "{} is not in this repository yet: check out the pull request to compare its files",
                        &rev[..rev.len().min(7)]
                    )),
                }
            };
            let (old, ob) = read(&b)?;
            let (new, nb) = read(&h)?;
            Ok::<_, String>((old, new, ob || nb))
        });
        cx.spawn_in(window, async move |this, cx| {
            let answer = work.await;
            let _ = this.update_in(cx, |shell, window, cx| match answer {
                Ok((old, new, binary)) => {
                    let lines = if binary {
                        Vec::new()
                    } else {
                        eludite_ui::diff::diff_lines(&old, &new)
                    };
                    let short = |r: &Option<String>| {
                        r.as_deref()
                            .map(|r| r[..r.len().min(7)].to_owned())
                            .unwrap_or_default()
                    };
                    let against = format!("forge-{}", short(&head));
                    shell.git_open_compare(
                        path,
                        short(&base),
                        short(&head),
                        binary,
                        &lines,
                        &against,
                        false,
                        window,
                        cx,
                    );
                }
                Err(e) => {
                    shell.status.set(slots::STATE, e);
                }
            });
        })
        .detach();
    }

    /// A check's log in a read-only document.
    fn forge_open_log(
        &mut self,
        id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = format!("{LOG_PREFIX}{id}");
        let theme = self.theme;
        let view = cx.new(|cx| log::CheckLog::new(theme, id.clone(), name.clone(), cx));
        self.forge
            .documents
            .borrow_mut()
            .insert(tab.clone(), view.into());
        self.controller
            .open_document(&tab, &format!("{name} (log)"));
        self.dock.update(cx, |_, cx| cx.notify());
        self.forge_spawn(
            cmds::CHECK_LOG.into(),
            json!({"id": id, "max_bytes": ops::LOG_CAP}),
            Then::Log { tab },
            window,
            cx,
        );
    }

    /// Git > Create Pull Request: the form, prefilled off the UI thread from the branch's commits.
    fn forge_open_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme;
        let form = cx.new(|cx| create::CreatePullForm::new(theme, cx));
        cx.subscribe_in(
            &form,
            window,
            |shell, _, e: &create::CreateEvent, window, cx| match e {
                create::CreateEvent::Create(args) => shell.forge_spawn(
                    cmds::PULL_CREATE.into(),
                    args.clone(),
                    Then::Created,
                    window,
                    cx,
                ),
            },
        )
        .detach();
        self.forge
            .documents
            .borrow_mut()
            .insert(CREATE_TAB.into(), form.clone().into());
        self.forge.create = Some(form.clone());
        self.controller
            .open_document(CREATE_TAB, "Create a Pull Request");
        self.dock.update(cx, |_, cx| cx.notify());
        let service = self.forge.service.clone();
        let work = cx.background_spawn(async move {
            let git = &*service.git;
            let branch = git.current_branch();
            let remote = git.remote(None);
            let detected = remote
                .as_ref()
                .map(|(_, url)| service.hub().detect(url, true));
            let base = remote
                .as_ref()
                .and_then(|(name, _)| git.default_branch(name))
                .unwrap_or_else(|| "main".into());
            let (title, body) = match (&branch, &remote) {
                (Some(b), Some((name, _))) => ops::prefill(git, &format!("{name}/{base}"), b),
                _ => (String::new(), String::new()),
            };
            (branch, base, title, body, detected)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (branch, base, title, body, detected) = work.await;
            let _ = this.update(cx, |shell, cx| {
                let caps = detected
                    .as_ref()
                    .map(|r| eludite_forge::forge::capabilities(r.family))
                    .unwrap_or_default();
                let family = detected.as_ref().map(|r| r.family).unwrap_or_default();
                form.update(cx, |f, cx| {
                    f.prefill(branch, base, title, body, family, caps, cx)
                });
                cx.notify();
                let _ = shell;
            });
        })
        .detach();
    }

    /// The sign-in dialog for `host` (default the workspace remote's).
    fn forge_open_signin(
        &mut self,
        host: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme;
        let detected = self.forge.detected.clone().unwrap_or_default();
        let host = host.unwrap_or_else(|| detected.host.clone());
        let family = detected.family;
        let dialog = cx.new(|cx| signin::SignInDialog::new(theme, host, family, cx));
        cx.subscribe_in(
            &dialog,
            window,
            |shell, _, e: &signin::SignInEvent, window, cx| match e {
                signin::SignInEvent::Submit(args) => {
                    shell.forge_spawn(cmds::AUTH.into(), args.clone(), Then::Auth, window, cx)
                }
                signin::SignInEvent::Cancel => {
                    if let Some(d) = shell.forge.signin.take() {
                        let host = d.read(cx).host.clone();
                        let _ = shell
                            .commands
                            .invoke(cmds::AUTH, json!({"action": "cancel", "host": host}));
                    }
                    shell.forge.signin_poll = None;
                    cx.notify();
                }
                signin::SignInEvent::Copy(text) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()))
                }
                signin::SignInEvent::OpenUrl(url) => cx.open_url(url),
            },
        )
        .detach();
        gpui::Focusable::focus_handle(&dialog, cx).focus(window, cx);
        self.forge.signin = Some(dialog);
        cx.notify();
    }

    /// While a device flow waits: read the status every half second until it is signed in (the hub's event closes
    /// the dialog too).
    fn forge_poll_signin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.forge.signin.clone() else {
            return;
        };
        let host = dialog.read(cx).host.clone();
        let commands = self.commands.clone();
        self.forge.signin_poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let commands = commands.clone();
                let host = host.clone();
                let status = cx
                    .background_spawn(async move {
                        commands.invoke(cmds::AUTH, json!({"action": "status", "host": host}))
                    })
                    .await;
                let done = this
                    .update_in(cx, |shell, window, cx| {
                        let Ok(v) = &status else { return true };
                        if v["signed_in"] == true {
                            shell.forge.signin = None;
                            shell.forge.detected = None;
                            shell.status.set(
                                slots::STATE,
                                format!("Signed in to {}", v["host"].as_str().unwrap_or_default()),
                            );
                            shell.forge_load_pulls(false, window, cx);
                            shell.forge_load_issues(false, window, cx);
                            cx.notify();
                            return true;
                        }
                        v["pending"] != true
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        }));
    }

    /// A forge document closed.
    pub(super) fn forge_document_closed(&mut self, id: &str) {
        self.forge.documents.borrow_mut().remove(id);
        self.forge.pull_documents.remove(id);
        self.forge.margins.borrow_mut().remove(id);
        if id == CREATE_TAB {
            self.forge.create = None;
        }
    }

    /// The git status changed (brief 0040's status, drawn): the status bar's pull request, the link, the margins.
    pub(super) fn forge_git_changed(&mut self, cx: &mut Context<Self>) {
        self.forge_update_status(cx);
        self.forge_update_link(cx);
        self.forge_update_margins(cx);
    }

    /// A push succeeded: offer "Create a Pull Request".
    pub(super) fn forge_pushed(&mut self, cx: &mut Context<Self>) {
        self.forge.link = true;
        self.forge_update_link(cx);
    }

    fn forge_update_link(&mut self, cx: &mut Context<Self>) {
        let ahead = self
            .git
            .status
            .as_ref()
            .is_some_and(|s| s.upstream.is_some() && s.ahead > 0);
        let has_remote = self.forge.detected.as_ref().is_none_or(|d| d.supported());
        let current_has_pr = self.forge.current.is_some();
        let show = (self.forge.link || ahead)
            && has_remote
            && !current_has_pr
            && self.git.status.as_ref().is_some_and(|s| s.branch.is_some());
        self.git
            .changes
            .update(cx, |c, cx| c.set_pull_request_link(show, cx));
    }

    /// The status bar: the current branch's open pull request in the cached list, and its checks' state. Reads the
    /// cache off the UI thread; never the network.
    fn forge_update_status(&mut self, cx: &mut Context<Self>) {
        let branch = self.git.status.as_ref().and_then(|s| s.branch.clone());
        let Some(branch) = branch else {
            self.status.set(PULL_SLOT, "");
            return;
        };
        let pulls = self.forge.pulls.read(cx);
        let hit = pulls
            .items
            .iter()
            .find(|p| p["head"] == branch.as_str() && p["state"] == "open")
            .cloned();
        let Some(p) = hit else {
            self.forge.current = None;
            self.status.set(PULL_SLOT, "");
            return;
        };
        let label = match p["number"].as_u64() {
            Some(n) => format!("#{n}"),
            None => p["id"]
                .as_str()
                .unwrap_or("")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_owned(),
        };
        let base = format!("\u{21C4} {label}");
        let args = match p["number"].as_u64() {
            Some(n) => json!({"number": n}),
            None => json!({"id": p["id"]}),
        };
        let same = self
            .forge
            .current
            .as_ref()
            .is_some_and(|(c, b)| c["id"] == p["id"] && *b == branch);
        if !same {
            self.status.set(
                PULL_SLOT,
                format!("{base}{}", checks_glyph(p["checks"].as_str())),
            );
        }
        self.status.set_action(PULL_SLOT, cmds::PULL, args.clone());
        self.forge.current = Some((p.clone(), branch));
        cx.notify();
        // The head commit's checks, as one glyph and a count: read through the cache off the UI thread (a forge
        // window is open or a command ran, or there would be no pull request to show).
        let capable = self
            .forge
            .detected
            .as_ref()
            .is_none_or(|d| d.capabilities.checks);
        if !capable {
            return;
        }
        let commands = self.commands.clone();
        let generation = self.forge.generation;
        let id = p["id"].clone();
        let work = cx.background_spawn(async move { commands.invoke(cmds::CHECKS, args) });
        cx.spawn(async move |this, cx| {
            let Ok(v) = work.await else { return };
            let _ = this.update(cx, |shell, cx| {
                if shell.forge.generation != generation
                    || shell
                        .forge
                        .current
                        .as_ref()
                        .is_none_or(|(c, _)| c["id"] != id)
                {
                    return;
                }
                shell
                    .status
                    .set(PULL_SLOT, format!("{base}{}", checks_summary(&v)));
                cx.notify();
            });
        })
        .detach();
    }

    /// Review threads in the margin of the open editors, for the pull request checked out (its head branch, or the
    /// `pr/<n>` or `mr/<n>` branch Check Out made, is the current one).
    pub(super) fn forge_update_margins(&mut self, cx: &mut Context<Self>) {
        if self.forge.pull_documents.is_empty() && self.forge.margins.borrow().is_empty() {
            return;
        }
        let branch = self.git.status.as_ref().and_then(|s| s.branch.clone());
        let root = self.git.root.clone().map(|r| normalize_path(&r));
        let mut threads: HashMap<String, Vec<margin::Mark>> = HashMap::new();
        let mut checked_out: Option<ItemRef> = None;
        if let (Some(branch), Some(root)) = (&branch, &root) {
            for doc in self.forge.pull_documents.values() {
                let d = doc.read(cx);
                let Some(pull) = &d.pull else { continue };
                let n = pull
                    .summary
                    .label()
                    .trim_start_matches(['#', '!'])
                    .to_owned();
                if *branch != pull.summary.head
                    && *branch != format!("pr/{n}")
                    && *branch != format!("mr/{n}")
                {
                    continue;
                }
                checked_out = Some(d.item.clone());
                for t in &pull.threads {
                    let (Some(path), Some(line)) = (&t.path, t.line) else {
                        continue;
                    };
                    if t.side == Some(eludite_forge::Side::Left) {
                        continue;
                    }
                    let abs = normalize_path(&root.join(path))
                        .to_string_lossy()
                        .into_owned();
                    threads
                        .entry(abs)
                        .or_default()
                        .push((line, t.clone(), d.item.clone()));
                }
            }
        }
        let ids: Vec<String> = self.views.borrow().keys().cloned().collect();
        let theme = self.theme;
        for id in ids {
            let marks = threads.remove(&id).unwrap_or_default();
            // The file's path in the repository, for new comments.
            let target = match (&checked_out, &root) {
                (Some(item), Some(root)) => Path::new(&id).strip_prefix(root).ok().map(|rel| {
                    let rel = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    (item.clone(), rel)
                }),
                _ => None,
            };
            let existing = self.forge.margins.borrow().get(&id).cloned();
            match existing {
                Some(m) => m.update(cx, |m, cx| m.set(marks, target, cx)),
                None if !marks.is_empty() || target.is_some() => {
                    let Some(view) = self.views.borrow().get(&id).cloned() else {
                        continue;
                    };
                    let m = cx.new(|cx| margin::ThreadMarks::new(theme, view, cx));
                    m.update(cx, |m, cx| m.set(marks, target, cx));
                    self.forge.margins.borrow_mut().insert(id.clone(), m);
                }
                None => {}
            }
        }
        self.dock.update(cx, |_, cx| cx.notify());
    }
}

/// The status bar's glyph of a checks state.
fn checks_glyph(state: Option<&str>) -> &'static str {
    match state {
        Some("success") => " \u{2713}",
        Some("failure") => " \u{2717}",
        Some("pending") => " \u{25CF}",
        _ => "",
    }
}

/// The glyph and the count of a `eludite.forge.checks` answer: " ✗ 1/3" (passed of all).
pub fn checks_summary(v: &Value) -> String {
    let items = v["items"].as_array().map(Vec::as_slice).unwrap_or_default();
    if items.is_empty() {
        return String::new();
    }
    let passed = items
        .iter()
        .filter(|c| {
            matches!(
                c["conclusion"].as_str(),
                Some("success" | "neutral" | "skipped")
            )
        })
        .count();
    format!(
        "{} {passed}/{}",
        checks_glyph(v["state"].as_str()),
        items.len()
    )
}

/// Whether tool window `id` is on screen: the active tab of a docked group, or floating.
pub fn shown(layout: &eludite_docking::DockLayout, id: &str) -> bool {
    use eludite_docking::model::Place;
    match layout.find(id) {
        Some(Place::Docked { side, group }) => layout
            .dock(side)
            .groups
            .iter()
            .find(|g| g.id == group)
            .is_some_and(|g| g.active_id() == Some(id)),
        Some(Place::Floating { .. }) => true,
        _ => false,
    }
}

fn then_needs_detect(command: &str) -> bool {
    matches!(command, cmds::PULLS | cmds::ISSUES | cmds::PULL)
}

fn item_of(args: &Value) -> Result<ItemRef, ()> {
    match (args["number"].as_u64(), args["id"].as_str()) {
        (Some(n), _) => Ok(ItemRef::Number(n)),
        (None, Some(id)) => Ok(ItemRef::Id(id.to_owned())),
        _ => Err(()),
    }
}

pub fn item_args(item: &ItemRef) -> Value {
    match item {
        ItemRef::Number(n) => json!({"number": n}),
        ItemRef::Id(id) => match id.parse::<u64>() {
            Ok(n) => json!({"number": n}),
            Err(_) => json!({"id": id}),
        },
    }
}

/// The pull request document a command's input names.
fn tab_of(args: &Value) -> Option<String> {
    item_of(args).ok().map(|i| pull_tab(&i.id()))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The repository the windows show (from the hub's detection, no probe).
#[cfg_attr(not(test), allow(dead_code))]
pub fn detect_repository(service: &ForgeService) -> Option<Repository> {
    service
        .git
        .remote(None)
        .map(|(_, url)| service.hub().detect(&url, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_and_close_ask_naming_the_method() {
        let (m, _) = confirmation(
            cmds::PULL_MERGE,
            &json!({"number": 12, "method": "squash"}),
            Family::GitHub,
        )
        .unwrap();
        assert_eq!(m, "Merge pull request #12: Squash and merge?");
        let (m, _) = confirmation(
            cmds::PULL_MERGE,
            &json!({"number": 3, "method": "merge"}),
            Family::GitLab,
        )
        .unwrap();
        assert!(
            m.starts_with("Merge merge request !3") || m.starts_with("Merge merge request #3"),
            "{m}"
        );
        assert!(confirmation(cmds::PULL_CLOSE, &json!({"number": 1}), Family::GitHub).is_some());
        assert!(
            confirmation(
                cmds::PULL_CLOSE,
                &json!({"number": 1, "reopen": true}),
                Family::GitHub
            )
            .is_none()
        );
        assert!(confirmation(cmds::PULL_COMMENT, &json!({"number": 1}), Family::GitHub).is_none());
    }

    #[test]
    fn the_banner_says_how_old_and_why() {
        let p = Provenance {
            stale: true,
            fetched_at: Some("2026-10-04T05:00:00Z".into()),
            age_seconds: 3700,
            error: None,
        };
        assert_eq!(
            p.banner(true).unwrap(),
            "Showing cached results from 2026-10-04T05:00:00Z (1 hour ago); refreshing\u{2026}"
        );
        let p = Provenance {
            error: Some("network: offline".into()),
            ..p
        };
        assert!(p.banner(false).unwrap().ends_with(": network: offline"));
        assert_eq!(Provenance::default().banner(false), None);
    }
}
