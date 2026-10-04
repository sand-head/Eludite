//! Git in the shell (brief 0040): the Git Changes and Git Repository windows, the status bar's branch, pending count
//! and sync arrows, the Workspace window's and the document tabs' status glyphs, Compare with Unmodified and Blame
//! documents, the editor's change margin, the confirmations, the commit message drafts and the automatic fetch.
//!
//! - **One status.** The [`service::GitService`] keeps the repository's status (recomputed off the UI thread by
//!   `eludite-git`'s watcher); when its generation grows the UI reads it ([`service::GitService::snapshot`], no
//!   libgit2) and draws everything from it: the windows, the glyphs, the tabs, the status bar. An older generation
//!   than the one on screen is dropped ([`Shell::git_apply_status`], CLAUDE.md invariant 12).
//! - **Every action is a command.** Buttons, menus, keys and the status bar dispatch `eludite.git.*`; [`Shell::run`]
//!   hands them to [`Shell::run_git`], which asks first where Visual Studio asks (Undo Changes, Reset > Delete
//!   Changes, a stash drop, a branch delete, any `force`) and then invokes the command on a background thread: the UI
//!   thread never waits on libgit2.
//! - **Drafts.** The commit message box keeps a draft per repository across restarts, in
//!   `<config dir>/eludite/git/drafts.json`, read and written off the UI thread.
//! - **Credentials** (brief 0045). A fetch, pull, push or sync the person started that fails with
//!   `credentials_required` opens the credential prompt ([`credentials::CredentialPrompt`], `git.credentialPrompt`)
//!   for the host it names; OK hands the answer to the service's in-memory store (for this transfer, or for the
//!   session with "Remember for this session") and runs the command again. An agent's command never reaches it: the
//!   bus answers the agent with `credentials_required`. A refused certificate is shown with its host like any other
//!   failure, and the Git Changes window shows a warning line while `http.sslVerify` is false.

pub mod changes;
pub mod compare;
pub mod credentials;
pub mod gutter;
pub mod repository;
pub mod service;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::git as cmds;
use eludite_commands::git::{BlameOutput, BranchesOutput, LogOutput, StashOutput, WorktreesOutput};
use eludite_commands::{CommandError, workspace};
use eludite_git::{GlyphIndex, Status};
use eludite_ui::diff::{DiffLine, diff_lines};
use eludite_ui::slots;
use gpui::{AnyView, AppContext as _, Context, Entity, PromptLevel, Task, Window};
use serde_json::{Value, json};

use super::Shell;
use super::documents::normalize_path;
use changes::{ChangesEvent, ChangesModel, GitChanges};
use credentials::{CredentialEvent, CredentialPrompt};
use repository::{GitRepositoryWindow, RepositoryEvent};
use service::{GitEvent, GitService, GitSettings, GitSetup};

/// Status bar slots (right): the pending changes (opens Git Changes), the incoming and outgoing arrows (Pull, Push),
/// Fetch, and while a transfer runs its progress and Cancel. The branch is the built-in `branch` slot (opens the Git
/// Repository window).
pub const PENDING_SLOT: &str = "git_pending";
pub const INCOMING_SLOT: &str = "git_incoming";
pub const OUTGOING_SLOT: &str = "git_outgoing";
pub const FETCH_SLOT: &str = "git_fetch";
pub const PROGRESS_SLOT: &str = "git_progress";
pub const CANCEL_SLOT: &str = "git_cancel";

/// Documents the git module draws (Compare with Unmodified, Blame), by tab id.
pub type Documents = Rc<RefCell<HashMap<String, AnyView>>>;
/// The change margins over open editors, by document id.
pub type Margins = Rc<RefCell<HashMap<String, Entity<gutter::ChangeMarks>>>>;

/// When things happened (the report's numbers).
#[derive(Debug, Default, Clone)]
pub struct GitTimings {
    /// Compare with Unmodified: from the command to the document's view.
    pub compare: Vec<Duration>,
    /// The change margin: from the edit to the marks.
    pub margin: Vec<Duration>,
}

/// The commit message drafts, per repository (in the service's state folder: `drafts.json`).
#[derive(Default)]
struct Drafts {
    map: BTreeMap<String, String>,
    write: Option<Task<()>>,
}

/// A command waiting on the credential prompt's answer.
#[derive(Debug, Clone)]
pub struct CredentialRequest {
    pub host: String,
    pub command: String,
    pub args: Value,
    pub then_push: bool,
}

/// The git half of the shell.
pub struct GitUi {
    pub service: Arc<GitService>,
    pub changes: Entity<GitChanges>,
    pub repository: Entity<GitRepositoryWindow>,
    pub documents: Documents,
    pub margins: Margins,
    /// The repository on screen, the generation and status drawn.
    pub root: Option<PathBuf>,
    pub shown: u64,
    pub status: Option<Arc<Status>>,
    pub glyphs: Rc<GlyphIndex>,
    margin_tasks: HashMap<String, Task<()>>,
    /// When each document was last edited (for the margin's timing).
    edited: HashMap<String, Instant>,
    /// What the Git Repository window and the stash list were loaded for.
    heads: Option<(Option<eludite_git::Oid>, Option<String>, usize, usize)>,
    drafts: Drafts,
    auto_fetch: Option<(u64, Task<()>)>,
    pub timings: GitTimings,
    /// The credential prompt, while it is open, and the command it answers for.
    pub prompt: Option<Entity<CredentialPrompt>>,
    pub prompt_request: Option<CredentialRequest>,
    /// Hosts whose one-time answer the running retry uses (forgotten when it ends).
    retrying: Vec<String>,
}

impl GitUi {
    pub fn new(
        service: Arc<GitService>,
        theme: eludite_ui::Theme,
        cx: &mut Context<Shell>,
    ) -> Self {
        Self {
            changes: cx.new(|cx| GitChanges::new(theme, cx)),
            repository: cx.new(|cx| GitRepositoryWindow::new(theme, cx)),
            service,
            documents: Rc::default(),
            margins: Rc::default(),
            root: None,
            shown: 0,
            status: None,
            glyphs: Rc::default(),
            margin_tasks: HashMap::new(),
            edited: HashMap::new(),
            heads: None,
            drafts: Drafts::default(),
            auto_fetch: None,
            timings: GitTimings::default(),
            prompt: None,
            prompt_request: None,
            retrying: Vec::new(),
        }
    }
}

/// The question Visual Studio asks before `command` runs with `args`, if it asks: (message, detail).
pub fn confirmation(command: &str, args: &Value) -> Option<(String, String)> {
    let force = args.get("force").and_then(Value::as_bool) == Some(true);
    let paths = || {
        args.get("paths")
            .and_then(Value::as_array)
            .map(|p| {
                p.iter()
                    .filter_map(Value::as_str)
                    .map(|s| format!("'{s}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "all files".into())
    };
    Some(match command {
        cmds::DISCARD => (
            format!("Are you sure you want to undo changes to {}?", paths()),
            "The changes will be lost. Untracked files are deleted. This cannot be undone.".into(),
        ),
        cmds::RESET if args.get("mode").and_then(Value::as_str) == Some("hard") => (
            format!(
                "Reset the current branch to {} and delete your changes?",
                args.get("revision")
                    .and_then(Value::as_str)
                    .unwrap_or("HEAD")
            ),
            "git reset --hard: uncommitted changes in the working tree are lost.".into(),
        ),
        cmds::STASH if args.get("action").and_then(Value::as_str) == Some("drop") => (
            format!(
                "Are you sure you want to delete stash@{{{}}}?",
                args.get("index").and_then(Value::as_u64).unwrap_or(0)
            ),
            "The stashed changes will be lost.".into(),
        ),
        cmds::BRANCHES if args.get("action").and_then(Value::as_str) == Some("delete") => (
            format!(
                "Are you sure you want to delete the branch '{}'?",
                args.get("name").and_then(Value::as_str).unwrap_or_default()
            ),
            if force {
                "It is not fully merged: its commits will be lost.".into()
            } else {
                "The branch is merged into the current one.".into()
            },
        ),
        cmds::CHECKOUT | cmds::PUSH | cmds::WORKTREES if force => (
            match command {
                cmds::CHECKOUT => "Check out and overwrite your local changes?".to_owned(),
                cmds::PUSH => "Force push and overwrite the remote branch?".to_owned(),
                _ => "Remove the worktree and its changes?".to_owned(),
            },
            "Force: what it overwrites cannot be recovered.".into(),
        ),
        _ => return None,
    })
}

fn short(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

/// What a background git call brings back besides the command's answer.
enum Extra {
    None,
    /// Compare with Unmodified: the texts' diff.
    Compare {
        path: String,
        old_label: String,
        new_label: String,
        binary: bool,
        lines: Vec<DiffLine>,
    },
    /// Blame: the file's text at HEAD.
    Blame(String),
    /// The Git Repository window's data.
    Repository(
        Option<BranchesOutput>,
        Option<LogOutput>,
        Option<StashOutput>,
        Option<WorktreesOutput>,
    ),
}

impl Shell {
    /// Wire the git windows and status bar (called once, at the end of [`Shell::new`]).
    pub(super) fn git_install(
        &mut self,
        mut events: futures::channel::mpsc::UnboundedReceiver<GitEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use eludite_ui::SlotAlign;
        use futures::StreamExt as _;
        for slot in [
            PROGRESS_SLOT,
            CANCEL_SLOT,
            FETCH_SLOT,
            INCOMING_SLOT,
            OUTGOING_SLOT,
            PENDING_SLOT,
        ] {
            self.status.add_slot(slot, SlotAlign::Right);
        }
        self.status.set_action(
            slots::BRANCH,
            "eludite.view.show",
            json!({ "id": eludite_docking::ids::GIT_REPOSITORY }),
        );
        self.status.set_action(
            PENDING_SLOT,
            "eludite.view.show",
            json!({ "id": eludite_docking::ids::GIT_CHANGES }),
        );
        self.status.set_action(INCOMING_SLOT, cmds::PULL, json!({}));
        self.status.set_action(OUTGOING_SLOT, cmds::PUSH, json!({}));
        self.status.set_action(FETCH_SLOT, cmds::FETCH, json!({}));
        self.status.set_action(CANCEL_SLOT, cmds::CANCEL, json!({}));
        cx.observe(&self.git.changes, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&self.git.repository, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe_in(
            &self.git.changes,
            window,
            |shell, _, e: &ChangesEvent, window, cx| match e {
                ChangesEvent::Draft(text) => shell.git_save_draft(text.clone(), cx),
                ChangesEvent::CommitAndPush(args) => {
                    shell.git_spawn(cmds::COMMIT.into(), args.clone(), true, window, cx)
                }
            },
        )
        .detach();
        cx.subscribe_in(
            &self.git.repository,
            window,
            |shell, _, e: &RepositoryEvent, window, cx| match e {
                RepositoryEvent::Load => shell.git_load_repository(window, cx),
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
                            shell.on_git_event(e, window, cx);
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

    fn on_git_event(&mut self, event: GitEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            GitEvent::Repository(root) => {
                self.git.root = root.clone();
                self.git.shown = 0;
                self.git.status = None;
                self.git.heads = None;
                self.git_load_draft(window, cx);
                self.git_refresh(cx);
            }
            GitEvent::Status(_) => self.git_refresh(cx),
            GitEvent::Progress(text) => {
                self.status
                    .set(PROGRESS_SLOT, text.clone().unwrap_or_default());
                self.status
                    .set(CANCEL_SLOT, if text.is_some() { "Cancel" } else { "" });
                self.git
                    .changes
                    .update(cx, |c, cx| c.set_progress(text, cx));
                cx.notify();
            }
        }
    }

    /// Draw the service's status, if newer than the one on screen.
    fn git_refresh(&mut self, cx: &mut Context<Self>) {
        let snap = self.git.service.snapshot();
        if snap.repository != self.git.root && snap.repository.is_some() {
            self.git.root = snap.repository.clone();
            self.git.shown = 0;
        }
        if snap.repository.is_none() {
            self.git_show_none(snap.loading, cx);
            return;
        }
        if let Some(status) = snap.status {
            self.git_apply_status(snap.generation, status, cx);
        } else {
            self.git.changes.update(cx, |c, cx| {
                c.set_model(
                    ChangesModel {
                        repository: Some(true),
                        loading: true,
                        ..Default::default()
                    },
                    cx,
                )
            });
        }
    }

    /// No repository: Create Git Repository, no glyphs, empty slots.
    fn git_show_none(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.git.status = None;
        self.git.glyphs = Rc::default();
        let known = self.workspace_root().is_some();
        self.git.changes.update(cx, |c, cx| {
            c.set_model(
                ChangesModel {
                    repository: (known && !loading).then_some(false),
                    loading,
                    ..Default::default()
                },
                cx,
            )
        });
        for slot in [
            slots::BRANCH,
            PENDING_SLOT,
            INCOMING_SLOT,
            OUTGOING_SLOT,
            FETCH_SLOT,
        ] {
            self.status.set(slot, "");
        }
        self.explorer.update(cx, |e, cx| e.set_git(None, cx));
        self.git_update_tabs(cx);
        cx.notify();
    }

    /// Draw `status` at `generation`, unless an equal or newer one is on screen (a stale answer is dropped). Returns
    /// whether it was drawn.
    pub(super) fn git_apply_status(
        &mut self,
        generation: u64,
        status: Arc<Status>,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation <= self.git.shown && self.git.status.is_some() {
            return false;
        }
        self.git.shown = generation;
        self.git.glyphs = Rc::new(status.glyphs());
        self.git.status = Some(status.clone());
        let model = ChangesModel::from_status(&status, generation);
        self.git.changes.update(cx, |c, cx| c.set_model(model, cx));
        // The status bar, as Visual Studio's bottom right: the arrows, the pending count, the branch.
        let pending = status.changed_files();
        self.status.set(
            PENDING_SLOT,
            if pending > 0 {
                format!("\u{270E} {pending}")
            } else {
                String::new()
            },
        );
        let branch = match (&status.branch, status.detached) {
            (Some(b), _) => format!("\u{2387} {b}"),
            (None, _) => "\u{2387} (detached)".into(),
        };
        self.status.set(slots::BRANCH, branch);
        self.status.set(FETCH_SLOT, "Fetch");
        if status.upstream.is_some() {
            self.status
                .set(INCOMING_SLOT, format!("\u{2193}{}", status.behind));
            self.status
                .set(OUTGOING_SLOT, format!("\u{2191}{}", status.ahead));
        } else {
            self.status.set(INCOMING_SLOT, "");
            self.status.set(
                OUTGOING_SLOT,
                if status.branch.is_some() && status.head.is_some() {
                    "\u{2191} Publish"
                } else {
                    ""
                },
            );
        }
        let root = self.git.root.clone();
        let glyphs = self.git.glyphs.clone();
        self.explorer
            .update(cx, |e, cx| e.set_git(root.map(|r| (r, glyphs)), cx));
        self.git_update_tabs(cx);
        // The Git Repository window and the stash list follow HEAD, the refs and the stashes.
        let heads = (
            status.head,
            status.branch.clone(),
            status.ahead + status.behind * 100_000,
            status.stashes,
        );
        if self.git.heads.as_ref() != Some(&heads) {
            let stashes_changed = self.git.heads.as_ref().is_none_or(|h| h.3 != heads.3);
            self.git.heads = Some(heads);
            let operation = status.operation.map(|o| o.as_str().to_owned());
            self.git.repository.update(cx, |r, cx| {
                r.operation = operation;
                r.invalidate(cx)
            });
            if stashes_changed {
                self.git_load_stashes(cx);
            }
        } else {
            let operation = status.operation.map(|o| o.as_str().to_owned());
            self.git.repository.update(cx, |r, cx| {
                if r.operation != operation {
                    r.operation = operation;
                    cx.notify();
                }
            });
        }
        // The index may have changed: every open editor's margin.
        let ids: Vec<String> = self.documents.keys().cloned().collect();
        for id in ids {
            self.git_margin_later(&id, Duration::ZERO, cx);
        }
        cx.notify();
        true
    }

    /// The glyph of every open document's tab.
    fn git_update_tabs(&mut self, _cx: &mut Context<Self>) {
        let root = self.git.root.as_ref().map(|r| normalize_path(r));
        for id in self.documents.keys() {
            let badge = root.as_ref().and_then(|root| {
                let p = normalize_path(Path::new(id));
                let rel = p.strip_prefix(root).ok()?;
                let rel = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                self.git.glyphs.get(&rel)
            });
            self.controller
                .set_document_badge(id, badge.map(|g| (g.glyph().to_owned(), g.color())));
        }
    }

    /// A `eludite.git.*` command from the UI (a button, a menu, a key, the status bar). Asks first where Visual Studio
    /// asks, then runs the command through the bus off the UI thread. Returns false for other commands.
    pub(super) fn run_git(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // Git > Open in File Explorer: the repository's folder.
        if command == eludite_commands::project::OPEN_CONTAINING_FOLDER
            && args.get("path").is_none()
            && let Some(root) = &self.git.root
        {
            *args = json!({ "path": root.to_string_lossy() });
            return false;
        }
        if !cmds::ALL.contains(&command) {
            return false;
        }
        // Git > New Branch...: the Git Repository window's New Branch box.
        if command == cmds::CHECKOUT && args.get("name").is_none() {
            let _ = self.invoke(
                "eludite.view.show",
                json!({ "id": eludite_docking::ids::GIT_REPOSITORY }),
                window,
                cx,
            );
            self.git
                .repository
                .update(cx, |r, cx| r.focus_new_branch(window, cx));
            cx.notify();
            return true;
        }
        let args = std::mem::take(args);
        if let Some((message, detail)) = confirmation(command, &args) {
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
                        shell.git_spawn(command, args, false, window, cx)
                    });
                }
            })
            .detach();
            return true;
        }
        self.git_spawn(command.to_owned(), args, false, window, cx);
        true
    }

    /// Invoke `command` on the bus from a background thread, then apply its answer on the UI thread.
    fn git_spawn(
        &mut self,
        command: String,
        args: Value,
        then_push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = self.commands.clone();
        let service = self.git.service.clone();
        let started = Instant::now();
        self.status
            .set(slots::STATE, format!("{}\u{2026}", title_of(&command)));
        cx.notify();
        let work = {
            let (command, args) = (command.clone(), args.clone());
            cx.background_spawn(async move {
                let result = commands.invoke(&command, args.clone());
                let extra = match (command.as_str(), &result) {
                    (cmds::DIFF, Ok(out)) => {
                        let path = out["path"].as_str().unwrap_or_default().to_owned();
                        let against = args["against"].as_str().unwrap_or("index").to_owned();
                        let staged = args["staged"].as_bool().unwrap_or(false);
                        match service.diff_texts(&path, &against, staged) {
                            Ok(t) => Extra::Compare {
                                lines: if t.binary {
                                    Vec::new()
                                } else {
                                    diff_lines(&t.old, &t.new)
                                },
                                path: t.path,
                                old_label: t.old_label,
                                new_label: t.new_label,
                                binary: t.binary,
                            },
                            Err(_) => Extra::None,
                        }
                    }
                    (cmds::BLAME, Ok(out)) => {
                        let path = out["path"].as_str().unwrap_or_default();
                        let text = service
                            .repository()
                            .and_then(|r| r.revision_blob("HEAD", path).ok().flatten())
                            .map(|b| String::from_utf8_lossy(&b).into_owned())
                            .unwrap_or_default();
                        Extra::Blame(text)
                    }
                    _ => Extra::None,
                };
                (result, extra)
            })
        };
        cx.spawn_in(window, async move |this, cx| {
            let (result, extra) = work.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.git_finished(
                    &command, &args, result, extra, then_push, started, window, cx,
                )
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn git_finished(
        &mut self,
        command: &str,
        args: &Value,
        result: Result<Value, CommandError>,
        extra: Extra,
        then_push: bool,
        started: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A retry with the prompt's one-time answer ended: forget the answer.
        for host in std::mem::take(&mut self.git.retrying) {
            self.git.service.credentials().forget_once(&host);
        }
        let out = match result {
            Ok(v) => v,
            Err(e) => {
                let text = match &e {
                    CommandError::Failed(m) | CommandError::InvalidInput(m) => m.clone(),
                    other => other.to_string(),
                };
                eprintln!("eludite: {command}: {text}");
                if let Some(host) = service::credentials_required_host(&text) {
                    let refused = text.contains(" refused the user name and password");
                    self.git_ask_credentials(
                        CredentialRequest {
                            host: host.to_owned(),
                            command: command.to_owned(),
                            args: args.clone(),
                            then_push,
                        },
                        refused,
                        window,
                        cx,
                    );
                }
                self.status.set(slots::STATE, text.clone());
                self.git
                    .changes
                    .update(cx, |c, cx| c.set_info(Some((text.clone(), true)), cx));
                self.git.repository.update(cx, |r, cx| {
                    r.message = Some(text);
                    cx.notify()
                });
                cx.notify();
                return;
            }
        };
        let conflicts: Vec<String> = out
            .get("conflicts")
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let info = match command {
            cmds::COMMIT => {
                self.git.changes.update(cx, |c, cx| {
                    c.set_message(String::new(), cx);
                    c.set_amend(false, cx);
                });
                self.git_save_draft(String::new(), cx);
                if then_push {
                    self.git_spawn(cmds::PUSH.into(), json!({}), false, window, cx);
                }
                Some(format!(
                    "Commit {} created locally{}: {}",
                    short(out["commit"].as_str().unwrap_or_default()),
                    out["branch"]
                        .as_str()
                        .map(|b| format!(" on {b}"))
                        .unwrap_or_default(),
                    out["summary"].as_str().unwrap_or_default()
                ))
            }
            cmds::DIFF => {
                if let Extra::Compare {
                    path,
                    old_label,
                    new_label,
                    binary,
                    lines,
                } = extra
                {
                    let against = args["against"].as_str().unwrap_or("index");
                    let staged = args["staged"].as_bool().unwrap_or(false);
                    self.git_open_compare(
                        path, old_label, new_label, binary, &lines, against, staged, window, cx,
                    );
                    self.git.timings.compare.push(started.elapsed());
                }
                None
            }
            cmds::BLAME => {
                if let (Extra::Blame(text), Ok(blame)) =
                    (extra, serde_json::from_value::<BlameOutput>(out.clone()))
                {
                    self.git_open_blame(blame, text, cx);
                }
                None
            }
            cmds::BRANCHES | cmds::WORKTREES if matches!(extra, Extra::None) => {
                self.git.repository.update(cx, |r, cx| r.invalidate(cx));
                None
            }
            cmds::FETCH => Some(format!(
                "Fetched from {}{}",
                out["remote"].as_str().unwrap_or("origin"),
                out["behind"]
                    .as_u64()
                    .map(|b| format!(": {b} incoming"))
                    .unwrap_or_default()
            )),
            cmds::PUSH => Some(format!(
                "Pushed {} to {}",
                out["branch"].as_str().unwrap_or_default(),
                out["remote"].as_str().unwrap_or_default()
            )),
            cmds::CHECKOUT => Some(match out["branch"].as_str() {
                Some(b) if out["created"] == json!(true) => format!("Created and checked out {b}"),
                Some(b) => format!("Checked out {b}"),
                None => format!(
                    "Checked out {}",
                    short(out["commit"].as_str().unwrap_or_default())
                ),
            }),
            cmds::INIT => Some("Created a Git repository".into()),
            _ if !conflicts.is_empty() => Some(format!(
                "Conflicts in {} file{}: resolve them, stage them, then {}",
                conflicts.len(),
                if conflicts.len() == 1 { "" } else { "s" },
                if command == cmds::REBASE {
                    "continue the rebase"
                } else {
                    "commit"
                }
            )),
            _ => out
                .get("result")
                .or_else(|| out.get("pull"))
                .and_then(Value::as_str)
                .map(|r| format!("{}: {}", title_of(command), r.replace('_', " "))),
        };
        // The files with conflict markers open, as Visual Studio opens its merge editor.
        if let Some(root) = self.git.root.clone() {
            for c in &conflicts {
                let path = root.join(c);
                self.run(
                    workspace::FILE_OPEN,
                    json!({ "path": path.to_string_lossy() }),
                    window,
                    cx,
                );
            }
        }
        if let Some(i) = &info {
            self.git
                .changes
                .update(cx, |c, cx| c.set_info(Some((i.clone(), false)), cx));
        }
        if matches!(
            command,
            cmds::CHECKOUT
                | cmds::MERGE
                | cmds::REBASE
                | cmds::CHERRY_PICK
                | cmds::RESET
                | cmds::FETCH
                | cmds::PULL
                | cmds::PUSH
                | cmds::SYNC
                | cmds::BRANCHES
                | cmds::WORKTREES
                | cmds::COMMIT
                | cmds::STASH
        ) {
            self.git.repository.update(cx, |r, cx| {
                r.message = None;
                r.invalidate(cx)
            });
        }
        if let Ok(st) = serde_json::from_value::<StashOutput>(out.clone()) {
            self.git
                .changes
                .update(cx, |c, cx| c.set_stashes(st.stashes, cx));
        }
        self.status
            .set(slots::STATE, info.unwrap_or_else(|| "Ready".into()));
        self.git_refresh(cx);
        cx.notify();
    }

    /// Open the credential prompt for `request`'s host (Visual Studio asks in a dialog too); its answer runs the
    /// command again. One prompt at a time.
    fn git_ask_credentials(
        &mut self,
        request: CredentialRequest,
        refused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git.prompt.is_some() {
            return;
        }
        let theme = self.theme;
        let host = request.host.clone();
        let prompt = cx.new(|cx| CredentialPrompt::new(theme, host, refused, String::new(), cx));
        cx.subscribe_in(
            &prompt,
            window,
            |shell, _, e: &CredentialEvent, window, cx| {
                shell.git_credential_answer(e.clone(), window, cx)
            },
        )
        .detach();
        gpui::Focusable::focus_handle(&prompt, cx).focus(window, cx);
        self.git.prompt = Some(prompt);
        self.git.prompt_request = Some(request);
        cx.notify();
    }

    /// The prompt's OK (supply the answer, run the command again) or Cancel.
    fn git_credential_answer(
        &mut self,
        event: CredentialEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.git.prompt = None;
        let Some(request) = self.git.prompt_request.take() else {
            return;
        };
        self.focus.focus(window, cx);
        match event {
            CredentialEvent::Ok {
                username,
                password,
                remember,
            } => {
                self.git.service.credentials().supply(
                    &request.host,
                    eludite_git::UserPass { username, password },
                    remember,
                );
                if !remember {
                    self.git.retrying.push(request.host.clone());
                }
                self.git_spawn(request.command, request.args, request.then_push, window, cx);
            }
            CredentialEvent::Cancel => {
                let text = format!(
                    "{} canceled: no credentials for {}",
                    title_of(&request.command),
                    request.host
                );
                self.status.set(slots::STATE, text.clone());
                self.git
                    .changes
                    .update(cx, |c, cx| c.set_info(Some((text, true)), cx));
            }
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn git_open_compare(
        &mut self,
        path: String,
        old_label: String,
        new_label: String,
        binary: bool,
        lines: &[DiffLine],
        against: &str,
        staged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = compare::compare_tab(&path, against, staged);
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let theme = self.theme;
        let view = cx.new(|cx| {
            compare::CompareView::new(theme, path, old_label.clone(), new_label, binary, lines, cx)
        });
        self.git
            .documents
            .borrow_mut()
            .insert(id.clone(), view.clone().into());
        self.controller
            .open_document(&id, &format!("{name} vs. {name} ({old_label})"));
        gpui::Focusable::focus_handle(&view, cx).focus(window, cx);
        self.dock.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    fn git_open_blame(&mut self, blame: BlameOutput, text: String, cx: &mut Context<Self>) {
        let id = compare::blame_tab(&blame.path);
        let name = blame
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&blame.path)
            .to_owned();
        let theme = self.theme;
        let view = cx.new(|_| compare::BlameView::new(theme, blame, text));
        self.git
            .documents
            .borrow_mut()
            .insert(id.clone(), view.into());
        self.controller
            .open_document(&id, &format!("{name} (Blame)"));
        self.dock.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// The Git Repository window asked for its data: branches, the graph's log, stashes and worktrees, read on a
    /// background thread through the bus.
    fn git_load_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.root.is_none() {
            self.git.repository.update(cx, |r, cx| {
                r.set_data(None, Vec::new(), Vec::new(), Vec::new(), cx)
            });
            return;
        }
        let commands = self.commands.clone();
        let work = cx.background_spawn(async move {
            let read = |id: &str, args: Value| commands.invoke(id, args).ok();
            Extra::Repository(
                read(cmds::BRANCHES, json!({})).and_then(|v| serde_json::from_value(v).ok()),
                read(cmds::LOG, json!({ "all": true, "max": 500 }))
                    .and_then(|v| serde_json::from_value(v).ok()),
                read(cmds::STASH, json!({})).and_then(|v| serde_json::from_value(v).ok()),
                read(cmds::WORKTREES, json!({})).and_then(|v| serde_json::from_value(v).ok()),
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let data = work.await;
            let _ = this.update(cx, |shell, cx| {
                if let Extra::Repository(b, l, s, w) = data {
                    shell.git.repository.update(cx, |r, cx| {
                        r.set_data(
                            b,
                            l.map(|l| l.entries).unwrap_or_default(),
                            s.map(|s| s.stashes).unwrap_or_default(),
                            w.map(|w| w.worktrees).unwrap_or_default(),
                            cx,
                        )
                    });
                }
            });
        })
        .detach();
    }

    fn git_load_stashes(&mut self, cx: &mut Context<Self>) {
        let commands = self.commands.clone();
        let work = cx.background_spawn(async move { commands.invoke(cmds::STASH, json!({})) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(st)) = work.await.map(serde_json::from_value::<StashOutput>) {
                let _ = this.update(cx, |shell, cx| {
                    shell
                        .git
                        .changes
                        .update(cx, |c, cx| c.set_stashes(st.stashes, cx))
                });
            }
        })
        .detach();
    }

    // ----- The change margin -----

    /// Recompute `id`'s change margin after `delay` (the editor idle after an edit), off the UI thread.
    fn git_margin_later(&mut self, id: &str, delay: Duration, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        if self.git.root.is_none() || doc.read_only {
            self.git.margins.borrow_mut().remove(id);
            return;
        }
        let view = doc.view.clone();
        let path = doc.path.clone();
        let service = self.git.service.clone();
        let id_owned = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(text) = this.update(cx, |_, cx| view.read(cx).editor().text()) else {
                return;
            };
            let marks = cx
                .background_spawn(async move {
                    let base = service.index_text(&path).unwrap_or_default();
                    gutter::marks(&base, &text)
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.git_set_margin(&id_owned, view, marks, cx)
            });
        });
        self.git.margin_tasks.insert(id.to_owned(), task);
    }

    fn git_set_margin(
        &mut self,
        id: &str,
        view: Entity<eludite_editor::EditorView>,
        marks: Vec<gutter::Mark>,
        cx: &mut Context<Self>,
    ) {
        self.git.margin_tasks.remove(id);
        let existing = self.git.margins.borrow().get(id).cloned();
        let entity = match existing {
            Some(e) => e,
            None => {
                let e = cx.new(|cx| gutter::ChangeMarks::new(view, cx));
                self.git
                    .margins
                    .borrow_mut()
                    .insert(id.to_owned(), e.clone());
                e
            }
        };
        entity.update(cx, |m, cx| m.set(marks, cx));
        if let Some(at) = self.git.edited.remove(id) {
            self.git.timings.margin.push(at.elapsed());
        }
        self.dock.update(cx, |_, cx| cx.notify());
    }

    /// A document opened: its tab's glyph and its margin.
    pub(super) fn git_document_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        self.git_update_tabs(cx);
        self.git_margin_later(id, Duration::ZERO, cx);
    }

    /// An editor's text changed: its margin once the editor is idle.
    pub(super) fn git_editor_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        self.git
            .edited
            .entry(id.to_owned())
            .or_insert_with(Instant::now);
        self.git_margin_later(id, gutter::IDLE, cx);
    }

    /// A document was saved: the status follows after the watcher's debounce.
    pub(super) fn git_saved(&mut self) {
        self.git.service.touch();
    }

    /// A document closed (an editor, or a git document).
    pub(super) fn git_document_closed(&mut self, id: &str) {
        self.git.margins.borrow_mut().remove(id);
        self.git.margin_tasks.remove(id);
        self.git.documents.borrow_mut().remove(id);
    }

    /// The open workspace changed: look for its repository (off the UI thread).
    pub(super) fn git_workspace_changed(&mut self, folder: Option<&Path>) {
        self.git.service.set_workspace(folder);
    }

    // ----- Settings, drafts, automatic fetch -----

    /// `git.*` from the settings store.
    pub(super) fn git_apply_settings(&mut self, cx: &mut Context<Self>) {
        let (enabled, name, email, minutes) = {
            let s = self.settings.lock();
            (
                s.bool("git.enabled"),
                s.string("git.userName"),
                s.string("git.userEmail"),
                s.effective("git.autoFetchMinutes").0.as_u64().unwrap_or(0),
            )
        };
        let fallback = (!name.trim().is_empty() || !email.trim().is_empty()).then(|| {
            eludite_git::commit::Identity {
                name: name.trim().to_owned(),
                email: email.trim().to_owned(),
            }
        });
        let settings = GitSettings { enabled, fallback };
        if self.git.service.settings() != settings {
            self.git.service.set_settings(settings);
        }
        self.git_schedule_fetch(minutes, cx);
    }

    /// Fetch every `minutes` (0: never, the default: no network unless asked).
    fn git_schedule_fetch(&mut self, minutes: u64, cx: &mut Context<Self>) {
        if self.git.auto_fetch.as_ref().map(|(m, _)| *m) == Some(minutes) {
            return;
        }
        if minutes == 0 {
            self.git.auto_fetch = None;
            return;
        }
        let commands = self.commands.clone();
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(minutes * 60))
                    .await;
                let has_repo = this
                    .update(cx, |shell, _| shell.git.root.is_some())
                    .unwrap_or(false);
                if has_repo {
                    let c = commands.clone();
                    let _ = cx
                        .background_spawn(async move { c.invoke(cmds::FETCH, json!({})) })
                        .await;
                }
            }
        });
        self.git.auto_fetch = Some((minutes, task));
    }

    fn git_drafts_file(&self) -> Option<PathBuf> {
        self.git
            .service
            .setup()
            .state_dir
            .map(|d| d.join("drafts.json"))
    }

    /// Restore the repository's draft into the message box (read off the UI thread).
    fn git_load_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git.root.clone() else {
            return;
        };
        let Some(file) = self.git_drafts_file() else {
            return;
        };
        let read = cx.background_spawn(async move {
            std::fs::read_to_string(&file)
                .ok()
                .and_then(|t| serde_json::from_str::<BTreeMap<String, String>>(&t).ok())
                .unwrap_or_default()
        });
        cx.spawn_in(window, async move |this, cx| {
            let map = read.await;
            let _ = this.update(cx, |shell, cx| {
                let key = root.to_string_lossy().into_owned();
                let draft = map.get(&key).cloned().unwrap_or_default();
                shell.git.drafts.map = map;
                if shell.git.root.as_deref() == Some(root.as_path()) {
                    shell.git.changes.update(cx, |c, cx| {
                        if c.message().is_empty() {
                            c.set_message(draft, cx)
                        }
                    });
                }
            });
        })
        .detach();
    }

    /// Keep `text` as the repository's draft (written off the UI thread, 300 ms after the last keystroke).
    fn git_save_draft(&mut self, text: String, cx: &mut Context<Self>) {
        let (Some(root), Some(file)) = (self.git.root.clone(), self.git_drafts_file()) else {
            return;
        };
        let key = root.to_string_lossy().into_owned();
        if text.is_empty() {
            self.git.drafts.map.remove(&key);
        } else {
            self.git.drafts.map.insert(key, text);
        }
        let map = self.git.drafts.map.clone();
        self.git.drafts.write = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            cx.background_spawn(async move {
                if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = file.with_extension("json.tmp");
                if let Ok(text) = serde_json::to_string_pretty(&map)
                    && std::fs::write(&tmp, text).is_ok()
                {
                    let _ = std::fs::rename(&tmp, &file);
                }
            })
            .await;
        }));
    }

    #[cfg(test)]
    pub fn git(&self) -> &GitUi {
        &self.git
    }

    /// How often the automatic fetch runs, in minutes (`None`: never).
    #[cfg(test)]
    pub fn git_auto_fetch_minutes(&self) -> Option<u64> {
        self.git.auto_fetch.as_ref().map(|(m, _)| *m)
    }
}

/// A command's title for the status bar (`Fetch`).
fn title_of(command: &str) -> String {
    let name = command.rsplit('.').next().unwrap_or(command);
    let name = name.replace('_', "-");
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The setup the shell starts the service with.
pub fn setup() -> GitSetup {
    GitSetup::from_env()
}
