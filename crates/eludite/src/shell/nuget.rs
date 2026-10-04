//! NuGet in the shell (brief 0048; PLAN.md 4.7): the Manage NuGet Packages window ([`window::NuGetWindow`], a document
//! tab), `eludite.nuget.*` ([`NuGetService`]), the Output window's Package Manager source, the Error List's NuGet rows,
//! the credential prompt ([`credentials::CredentialPrompt`]), Tools > Options > NuGet Package Manager > Package Sources
//! ([`sources_page::PackageSourcesPage`]) and the Workspace window's Dependencies glyphs.
//!
//! - **One service.** [`NuGetService`] implements every command on the invoking thread, through the host
//!   (`eludite/nuget/*`): an agent's call on the agent's thread, the person's on a thread of its own, never the UI
//!   thread. A call carries the current generation (a stale answer is asked again under the new one), an operation id
//!   for its output, and `interactive` for the person's calls only, so the host asks the shell for credentials only for
//!   them; an agent gets `credentials_required` and is told to ask the person.
//! - **The window** runs the same commands ([`window::WindowEvent`] to [`Shell::nuget_spawn`]) and shows their answers;
//!   Browse searches 300 ms after the last key ([`SEARCH_DEBOUNCE`]); a newer search's answer replaces an older one,
//!   whose late answer is dropped.
//! - **Every caller's effects** arrive as [`NuGetEvent`]s: a restore's diagnostics become the Error List's NuGet rows
//!   (at the project file), a change makes the session ask for the tree again (the host advanced the generation), and
//!   the sources' vulnerability and deprecation data give the Dependencies node its yellow glyphs. NuGet's output
//!   (`eludite/nuget/update`) goes to the Output window's Package Manager source.
//! - **Nothing at startup.** No call reaches the host's NuGet service until the window opens, a tab asks, or an agent
//!   calls.

pub mod credentials;
pub mod sources_page;
pub mod window;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, ThreadId};
use std::time::Duration;

use eludite_commands::build::OutputSource;
use eludite_commands::diagnostics::{RowSource, Severity};
use eludite_commands::nuget::{
    self as cmds, ChangeAction, ChangeOutput, InstalledOutput, InstalledPackage, InstalledProject,
    NuGetCommands, NuGetOutput, NuGetRequest, PackageVersion, RestoreDiagnostic, RestoreOutput,
    SearchOutput, SearchRow, SourceOut, SourceRow, SourcesAction, SourcesOutput, Tab, UpdateRow,
    UpdatesOutput, Vulnerability,
};
use eludite_commands::solution::SolutionTreeOutput;
use eludite_commands::{CommandError, CommandRegistry, current_caller};
use eludite_docking::ids;
use eludite_lsp::Id;
use eludite_lsp::host::{self, Generation};
use eludite_protocol::RequestType;
use eludite_ui::Theme;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use gpui::{AppContext as _, Context, Entity, Focusable as _, Task, Window};
use serde_json::{Value, json};

use self::credentials::{CredentialEvent, CredentialPrompt};
use self::sources_page::{PackageSourcesPage, SourcesEvent};
use self::window::{NuGetWindow, Scope, WindowEvent};
use super::Shell;
use super::error_list::ErrorRow;
use super::session::{Reply, RequestError, ServerSession};

/// Browse searches this long after the last key in its search box.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// How long a call waits for the host: searches and reads, and changes and restores (a restore can take minutes).
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const CHANGE_TIMEOUT: Duration = Duration::from_secs(600);
/// How long an agent's eludite.nuget.manage waits for the UI thread.
const UI_TIMEOUT: Duration = Duration::from_secs(30);
/// Results the window asks a search for.
pub const WINDOW_RESULTS: usize = 50;

/// What an agent is told when a feed needs credentials (brief 0045's refusal).
pub const AGENT_CANNOT_ANSWER: &str = "Agents cannot answer the credential prompt: ask the person to run this in \
                                       Eludite (Manage NuGet Packages) and sign in there, or to install the feed's \
                                       credential provider; do not retry it another way.";

/// A version's sort key: its numeric parts, then a release before its prereleases.
pub fn version_key(v: &str) -> (Vec<u64>, bool, String) {
    let (core, pre) = v.split_once('-').unwrap_or((v, ""));
    let parts = core
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    (parts, pre.is_empty(), pre.to_owned())
}

/// The `nuget.*` settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NuGetSettings {
    pub include_prerelease: bool,
    pub restore_on_change: bool,
    pub lock_files: host::NuGetLockFiles,
}

impl Default for NuGetSettings {
    fn default() -> Self {
        Self {
            include_prerelease: false,
            restore_on_change: true,
            lock_files: host::NuGetLockFiles::Respect,
        }
    }
}

/// What the service and the person's calls tell the UI thread.
#[derive(Debug)]
pub enum NuGetEvent {
    /// A call the window (or the Options page) made answered.
    Answer {
        token: u64,
        result: Result<Value, String>,
    },
    /// A restore ran, for any caller: its diagnostics are the Error List's NuGet rows.
    Restored {
        diagnostics: Vec<host::NuGetDiagnostic>,
    },
    /// Projects changed, for any caller: the window reads its tab again.
    Changed,
    /// Vulnerability and deprecation data, for the Dependencies glyphs.
    Metadata {
        packages: Vec<host::NuGetPackageMetadata>,
    },
    /// An icon the host fetched (or `None`).
    Icon { url: String, path: Option<PathBuf> },
}

/// An agent's `eludite.nuget.manage`, for the UI thread to apply.
pub struct NuGetJob {
    pub request: NuGetRequest,
    pub reply: mpsc::SyncSender<Result<NuGetOutput, CommandError>>,
}

type Outcome = Result<NuGetOutput, CommandError>;

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on the UI thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// `eludite.nuget.*` on the invoking thread, through the host.
pub struct NuGetService {
    session: ServerSession,
    ui_thread: ThreadId,
    tree: Arc<Mutex<SolutionTreeOutput>>,
    settings: Mutex<NuGetSettings>,
    events: UnboundedSender<NuGetEvent>,
    jobs: UnboundedSender<NuGetJob>,
    next_operation: AtomicU64,
}

/// Register `eludite.nuget.*` on `commands`. Call on the UI thread.
pub fn register(
    commands: &CommandRegistry,
    session: ServerSession,
    tree: Arc<Mutex<SolutionTreeOutput>>,
) -> (
    Arc<NuGetService>,
    UnboundedReceiver<NuGetEvent>,
    UnboundedReceiver<NuGetJob>,
) {
    let (events, events_rx) = unbounded();
    let (jobs, jobs_rx) = unbounded();
    let service = Arc::new(NuGetService {
        session,
        ui_thread: thread::current().id(),
        tree,
        settings: Mutex::new(NuGetSettings::default()),
        events,
        jobs,
        next_operation: AtomicU64::new(1),
    });
    cmds::register(commands, service.clone());
    (service, events_rx, jobs_rx)
}

/// Waits for a session reply off the UI thread, at most `timeout`.
fn wait_reply<T: Send + 'static>(
    rx: oneshot::Receiver<Reply<T>>,
    timeout: Duration,
) -> Result<T, RequestError> {
    let (tx, done) = mpsc::sync_channel(1);
    let spawned = thread::Builder::new()
        .name("eludite-nuget-wait".into())
        .spawn(move || {
            let _ = tx.send(futures::executor::block_on(rx));
        });
    if spawned.is_err() {
        return Err(RequestError::Failed("cannot start a waiter thread".into()));
    }
    match done.recv_timeout(timeout) {
        Ok(Ok(reply)) => reply.result,
        Ok(Err(_)) => Err(RequestError::NoHost),
        Err(_) => Err(RequestError::Failed(format!(
            "eludite-host did not answer within {} s",
            timeout.as_secs()
        ))),
    }
}

fn vulns(v: &Option<Vec<host::NuGetVulnerability>>) -> Vec<Vulnerability> {
    v.iter()
        .flatten()
        .map(|x| Vulnerability {
            severity: x.severity.clone(),
            advisory_url: x.advisory_url.clone(),
        })
        .collect()
}

fn source_rows(rows: &[host::NuGetSourceResult]) -> Vec<SourceRow> {
    rows.iter()
        .map(|s| SourceRow {
            name: s.name.clone(),
            count: s.count,
            error: s.error.clone(),
        })
        .collect()
}

fn severity_name(s: host::BuildDiagnosticSeverity) -> &'static str {
    match s {
        host::BuildDiagnosticSeverity::Error => "error",
        host::BuildDiagnosticSeverity::Warning => "warning",
        host::BuildDiagnosticSeverity::Message => "message",
    }
}

/// A restore's outcome as the commands name it.
pub fn restore_output(o: &host::NuGetRestoreOutcome) -> RestoreOutput {
    let count = |s| o.diagnostics.iter().filter(|d| d.severity == s).count() as u32;
    RestoreOutput {
        result: match o.result {
            host::NuGetRestoreState::Succeeded => "succeeded",
            host::NuGetRestoreState::Failed => "failed",
            host::NuGetRestoreState::Canceled => "canceled",
        }
        .into(),
        errors: count(host::BuildDiagnosticSeverity::Error),
        warnings: count(host::BuildDiagnosticSeverity::Warning),
        elapsed_ms: o.elapsed_ms,
        locked_mode: o.locked_mode,
        lock_files: o.lock_files.clone(),
        diagnostics: o
            .diagnostics
            .iter()
            .take(cmds::MAX_DIAGNOSTICS)
            .map(|d| RestoreDiagnostic {
                severity: severity_name(d.severity).into(),
                code: d.code.clone(),
                message: d.message.clone(),
                path: d.file.clone().or_else(|| d.project.clone()),
                line: d.line,
                column: d.column,
            })
            .collect(),
    }
}

impl NuGetService {
    pub fn settings(&self) -> NuGetSettings {
        *self.settings.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_settings(&self, settings: NuGetSettings) {
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = settings;
    }

    fn operation(&self) -> u64 {
        self.next_operation.fetch_add(1, Ordering::Relaxed)
    }

    /// The solution's projects (name, path), from the tree.
    pub fn projects(&self) -> Vec<(String, String)> {
        self.tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .map(|p| (p.name.clone(), p.path.clone()))
            .collect()
    }

    /// A project by name (case-insensitive) or by the path of its project file.
    pub fn resolve_project(&self, project: &str) -> Result<String, CommandError> {
        let projects = self.projects();
        let normalized = super::documents::normalize_path(Path::new(project));
        projects
            .iter()
            .find(|(name, path)| {
                name.eq_ignore_ascii_case(project)
                    || super::documents::normalize_path(Path::new(path)) == normalized
            })
            .map(|(_, path)| path.clone())
            .ok_or_else(|| {
                CommandError::InvalidInput(format!(
                    "no project named {project} in the open solution; the projects are {}",
                    projects
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    fn resolve_all(&self, projects: &[String]) -> Result<Option<Vec<String>>, CommandError> {
        if projects.is_empty() {
            return Ok(None);
        }
        projects
            .iter()
            .map(|p| self.resolve_project(p))
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    fn error(&self, e: RequestError) -> CommandError {
        match e {
            RequestError::NoHost => CommandError::Failed(
                "no solution is open (eludite-host is not running): open one with eludite.solution.open".into(),
            ),
            RequestError::Canceled => CommandError::Failed("the call was canceled".into()),
            RequestError::Stale => CommandError::Failed(
                "the solution changed while the call ran; call it again".into(),
            ),
            RequestError::Failed(message) => {
                let message = message
                    .split_once(": ")
                    .filter(|(head, _)| head.starts_with("server error"))
                    .map_or(message.as_str(), |(_, rest)| rest)
                    .to_owned();
                if message.contains("credentials_required") && current_caller().is_agent() {
                    CommandError::Failed(format!("{message} {AGENT_CANNOT_ANSWER}"))
                } else {
                    CommandError::Failed(message)
                }
            }
        }
    }

    /// One host request under the current generation; a stale answer (the generation moved) is asked again.
    fn call<R>(
        &self,
        timeout: Duration,
        make: impl Fn(Generation) -> R::Params,
    ) -> Result<R::Result, CommandError>
    where
        R: RequestType + 'static,
        R::Params: Send + 'static,
        R::Result: Send + 'static,
    {
        let mut attempts = 0;
        loop {
            let generation = self.session.shared().generation;
            let (_handle, rx) = self.session.request::<R>(make(generation));
            match wait_reply(rx, timeout) {
                Ok(v) => return Ok(v),
                Err(RequestError::Stale) if attempts < 2 => attempts += 1,
                Err(e) => return Err(self.error(e)),
            }
        }
    }

    fn emit(&self, event: NuGetEvent) {
        let _ = self.events.unbounded_send(event);
    }

    /// `eludite/nuget/icon`, for the window's rows (not a command: it changes nothing anyone sees but the row).
    pub fn icon(&self, url: &str) -> Option<PathBuf> {
        let url = url.to_owned();
        self.call::<host::NuGetIcon>(READ_TIMEOUT, |_| host::NuGetIconParams { url: url.clone() })
            .ok()
            .and_then(|r| r.path.map(PathBuf::from))
    }

    fn search(
        &self,
        query: String,
        source: Option<String>,
        prerelease: Option<bool>,
        max_items: usize,
        skip: usize,
    ) -> Outcome {
        let operation = self.operation();
        let prerelease = prerelease.unwrap_or(self.settings().include_prerelease);
        let interactive = !current_caller().is_agent();
        let r =
            self.call::<host::NuGetSearch>(READ_TIMEOUT, |generation| host::NuGetSearchParams {
                generation,
                operation: Some(operation),
                query: Some(query.clone()),
                source: source.clone(),
                prerelease: Some(prerelease),
                skip: Some(skip as u32),
                take: Some(max_items as u32 + 1),
                interactive: Some(interactive),
            })?;
        let truncated = r.results.len() > max_items;
        Ok(NuGetOutput::Search(Box::new(SearchOutput {
            results: r
                .results
                .into_iter()
                .take(max_items)
                .map(|p| SearchRow {
                    versions: p
                        .versions
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .take(cmds::MAX_VERSIONS)
                        .collect(),
                    vulnerabilities: vulns(&p.vulnerabilities),
                    deprecated: p.deprecation.is_some(),
                    license: p.license_expression.clone().or(p.license_url.clone()),
                    id: p.id,
                    version: p.version,
                    description: p.description,
                    authors: p.authors,
                    source: p.source,
                    downloads: p.downloads,
                    project_url: p.project_url,
                    icon_url: p.icon_url,
                })
                .collect(),
            sources: source_rows(&r.sources),
            truncated,
        })))
    }

    fn installed(
        &self,
        project: Option<String>,
        include_transitive: bool,
        metadata: bool,
    ) -> Outcome {
        let projects = project
            .map(|p| self.resolve_project(&p))
            .transpose()?
            .map(|p| vec![p]);
        let operation = self.operation();
        let interactive = !current_caller().is_agent();
        let r = self.call::<host::NuGetInstalled>(READ_TIMEOUT, |generation| {
            host::NuGetInstalledParams {
                generation,
                operation: Some(operation),
                projects: projects.clone(),
                include_transitive: Some(include_transitive),
                metadata: Some(metadata),
                interactive: Some(interactive),
            }
        })?;
        let known: Vec<host::NuGetPackageMetadata> = r
            .projects
            .iter()
            .flat_map(|p| &p.packages)
            .filter(|p| p.vulnerabilities.is_some() || p.deprecation.is_some())
            .filter_map(|p| {
                Some(host::NuGetPackageMetadata {
                    id: p.id.clone(),
                    version: p.version.clone()?,
                    vulnerabilities: p.vulnerabilities.clone(),
                    deprecation: p.deprecation.clone(),
                })
            })
            .collect();
        if !known.is_empty() {
            self.emit(NuGetEvent::Metadata { packages: known });
        }
        Ok(NuGetOutput::Installed(Box::new(InstalledOutput {
            projects: r
                .projects
                .into_iter()
                .map(|p| InstalledProject {
                    name: p.name,
                    path: p.path,
                    restored: p.restored,
                    central_package_management: p.central_package_management,
                    props_file: p.props_file,
                    lock_file: p.lock_file,
                    packages: p
                        .packages
                        .into_iter()
                        .map(|x| InstalledPackage {
                            vulnerabilities: vulns(&x.vulnerabilities),
                            deprecated: x.deprecation.is_some(),
                            id: x.id,
                            version: x.version,
                            requested: x.requested,
                            source: x.source,
                            transitive: x.transitive,
                            auto_referenced: x.auto_referenced,
                        })
                        .collect(),
                    error: p.error,
                    note: p.note,
                })
                .collect(),
        })))
    }

    fn updates_raw(
        &self,
        projects: Option<Vec<String>>,
        prerelease: bool,
        source: Option<String>,
    ) -> Result<host::NuGetUpdatesResult, CommandError> {
        let operation = self.operation();
        let interactive = !current_caller().is_agent();
        self.call::<host::NuGetUpdates>(READ_TIMEOUT, |generation| host::NuGetUpdatesParams {
            generation,
            operation: Some(operation),
            projects: projects.clone(),
            prerelease: Some(prerelease),
            source: source.clone(),
            interactive: Some(interactive),
        })
    }

    fn updates(
        &self,
        project: Option<String>,
        prerelease: Option<bool>,
        source: Option<String>,
    ) -> Outcome {
        let projects = project
            .map(|p| self.resolve_project(&p))
            .transpose()?
            .map(|p| vec![p]);
        let prerelease = prerelease.unwrap_or(self.settings().include_prerelease);
        let r = self.updates_raw(projects, prerelease, source)?;
        Ok(NuGetOutput::Updates(Box::new(UpdatesOutput {
            updates: r
                .updates
                .into_iter()
                .map(|u| UpdateRow {
                    vulnerable: u.vulnerabilities.as_ref().is_some_and(|v| !v.is_empty()),
                    project: u.project,
                    id: u.id,
                    installed: u.installed,
                    latest: u.latest,
                    source: u.source,
                })
                .collect(),
            sources: source_rows(&r.sources),
        })))
    }

    fn change(&self, action: ChangeAction, change: cmds::Change) -> Outcome {
        let settings = self.settings();
        let projects = self.resolve_all(&change.projects)?;
        let prerelease = change.prerelease.unwrap_or(settings.include_prerelease);
        let packages: Vec<host::NuGetPackageArg> = if change.all {
            let u = self.updates_raw(projects.clone(), prerelease, change.source.clone())?;
            let mut seen = HashSet::new();
            u.updates
                .into_iter()
                .filter(|r| seen.insert(r.id.to_lowercase()))
                .map(|r| host::NuGetPackageArg {
                    id: r.id,
                    version: Some(r.latest),
                })
                .collect()
        } else {
            vec![host::NuGetPackageArg {
                id: change.package.clone().unwrap_or_default(),
                version: change.version.clone(),
            }]
        };
        if packages.is_empty() {
            return Ok(NuGetOutput::Change(Box::new(ChangeOutput {
                action,
                packages: Vec::new(),
                projects: Vec::new(),
                edited: Vec::new(),
                restore: None,
                generation: self.session.shared().generation,
                message: Some("No updates are available.".into()),
            })));
        }
        let host_action = match action {
            ChangeAction::Install => host::NuGetAction::Install,
            ChangeAction::Uninstall => host::NuGetAction::Uninstall,
            ChangeAction::Update => host::NuGetAction::Update,
            ChangeAction::Consolidate => host::NuGetAction::Consolidate,
        };
        let operation = self.operation();
        let interactive = !current_caller().is_agent();
        let r =
            self.call::<host::NuGetChange>(CHANGE_TIMEOUT, |generation| host::NuGetChangeParams {
                generation,
                operation: Some(operation),
                action: host_action,
                packages: packages.clone(),
                projects: projects.clone(),
                prerelease: Some(prerelease),
                source: change.source.clone(),
                include_transitive: None,
                restore: Some(settings.restore_on_change),
                lock_files: Some(settings.lock_files),
                interactive: Some(interactive),
            })?;
        if !r.edited.is_empty() {
            // The host advanced the generation: the tree (and the Dependencies node) follows.
            self.session.refresh_tree();
            self.emit(NuGetEvent::Changed);
        }
        if let Some(restore) = &r.restore {
            self.emit(NuGetEvent::Restored {
                diagnostics: restore.diagnostics.clone(),
            });
        }
        Ok(NuGetOutput::Change(Box::new(ChangeOutput {
            action,
            packages: r
                .packages
                .into_iter()
                .map(|p| PackageVersion {
                    id: p.id,
                    version: p.version,
                })
                .collect(),
            projects: r.projects,
            edited: r.edited.into_iter().map(|e| e.path).collect(),
            restore: r.restore.as_ref().map(restore_output),
            generation: r.generation,
            message: r.message,
        })))
    }

    fn sources(&self, action: SourcesAction, name: Option<String>, url: Option<String>) -> Outcome {
        let operation = self.operation();
        let action = match action {
            SourcesAction::List => host::NuGetSourcesAction::List,
            SourcesAction::Add => host::NuGetSourcesAction::Add,
            SourcesAction::Remove => host::NuGetSourcesAction::Remove,
            SourcesAction::Enable => host::NuGetSourcesAction::Enable,
            SourcesAction::Disable => host::NuGetSourcesAction::Disable,
        };
        let r =
            self.call::<host::NuGetSources>(READ_TIMEOUT, |generation| host::NuGetSourcesParams {
                generation,
                operation: Some(operation),
                action: Some(action),
                name: name.clone(),
                url: url.clone(),
            })?;
        Ok(NuGetOutput::Sources(Box::new(SourcesOutput {
            sources: r
                .sources
                .into_iter()
                .map(|s| SourceOut {
                    name: s.name,
                    url: s.url,
                    enabled: s.enabled,
                    local: s.local,
                    scope: match s.scope {
                        host::NuGetSourceScope::Machine => "machine",
                        host::NuGetSourceScope::User => "user",
                        host::NuGetSourceScope::Solution => "solution",
                        host::NuGetSourceScope::Other => "other",
                    }
                    .into(),
                    config_file: s.config_file,
                })
                .collect(),
            user_config: r.user_config,
            changed: r.changed,
        })))
    }

    fn restore(&self, project: Option<String>, force: bool) -> Outcome {
        let projects = project
            .map(|p| self.resolve_project(&p))
            .transpose()?
            .map(|p| vec![p]);
        let settings = self.settings();
        let operation = self.operation();
        let interactive = !current_caller().is_agent();
        let r = self.call::<host::NuGetRestore>(CHANGE_TIMEOUT, |generation| {
            host::NuGetRestoreParams {
                generation,
                operation: Some(operation),
                projects: projects.clone(),
                lock_files: Some(settings.lock_files),
                force: Some(force),
                interactive: Some(interactive),
            }
        })?;
        self.emit(NuGetEvent::Restored {
            diagnostics: r.outcome.diagnostics.clone(),
        });
        Ok(NuGetOutput::Restore(Box::new(restore_output(&r.outcome))))
    }
}

impl NuGetCommands for NuGetService {
    fn apply(&self, request: NuGetRequest) -> Outcome {
        let on_ui = thread::current().id() == self.ui_thread;
        if matches!(request, NuGetRequest::Manage { .. }) {
            if on_ui {
                return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                    Err(CommandError::Failed(
                        "eludite.nuget.manage runs on the UI thread through the shell".into(),
                    ))
                });
            }
            let (reply, rx) = mpsc::sync_channel(1);
            self.jobs
                .unbounded_send(NuGetJob { request, reply })
                .map_err(|_| CommandError::Failed("the window is closed".into()))?;
            return rx
                .recv_timeout(UI_TIMEOUT)
                .map_err(|_| CommandError::Failed("the UI did not answer".into()))?;
        }
        if on_ui {
            return Err(CommandError::Failed(format!(
                "{} runs off the UI thread (the shell starts it on a thread of its own)",
                request.command()
            )));
        }
        match request {
            NuGetRequest::Manage { .. } => unreachable!("handled above"),
            NuGetRequest::Search {
                query,
                source,
                prerelease,
                max_items,
                skip,
            } => self.search(query, source, prerelease, max_items, skip),
            NuGetRequest::Installed {
                project,
                include_transitive,
                vulnerabilities,
            } => self.installed(project, include_transitive, vulnerabilities),
            NuGetRequest::Updates {
                project,
                prerelease,
                source,
            } => self.updates(project, prerelease, source),
            NuGetRequest::Change { action, change } => self.change(action, change),
            NuGetRequest::Sources { action, name, url } => self.sources(action, name, url),
            NuGetRequest::Restore { project, force } => self.restore(project, force),
        }
    }
}

/// What a person's call is for, so its answer lands in the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Search,
    Installed,
    Updates,
    /// A change; the window shows its label while it runs.
    Change(String),
    /// The Options page's list (and its changes).
    SourcesPage,
    /// The window's source dropdown.
    SourceNames,
}

/// The shell's NuGet state.
pub struct NuGetUi {
    pub service: Arc<NuGetService>,
    pub window: Entity<NuGetWindow>,
    pub sources_page: Entity<PackageSourcesPage>,
    /// The credential prompt, while open, and the host request it answers.
    pub prompt: Option<(Id, Entity<CredentialPrompt>)>,
    queued: VecDeque<(Id, host::NuGetCredentialsParams)>,
    /// The last restore's errors and warnings (the Error List's NuGet rows).
    pub diagnostics: Vec<host::NuGetDiagnostic>,
    /// `id/version` of packages the sources say are deprecated or vulnerable.
    pub warnings: HashSet<String>,
    pending: HashMap<u64, Pending>,
    next_token: u64,
    /// The newest search's token: an older answer is dropped.
    search_token: u64,
    events: UnboundedSender<NuGetEvent>,
    debounce: Option<Task<()>>,
    /// The window was opened at least once (its data is loaded on the first open).
    opened: bool,
}

impl NuGetUi {
    pub fn new(service: Arc<NuGetService>, theme: Theme, cx: &mut Context<Shell>) -> Self {
        let events = service.events.clone();
        Self {
            service,
            window: cx.new(|cx| NuGetWindow::new(theme, cx)),
            sources_page: cx.new(|cx| PackageSourcesPage::new(theme, cx)),
            prompt: None,
            queued: VecDeque::new(),
            diagnostics: Vec::new(),
            warnings: HashSet::new(),
            pending: HashMap::new(),
            next_token: 1,
            search_token: 0,
            events,
            debounce: None,
            opened: false,
        }
    }
}

impl Shell {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn nuget(&self) -> &NuGetUi {
        &self.nuget
    }

    /// Wire the window, the Options page and the event channels. Call once from `Shell::new`.
    pub(super) fn nuget_install(
        &mut self,
        mut events: UnboundedReceiver<NuGetEvent>,
        mut jobs: UnboundedReceiver<NuGetJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browser_views
            .borrow_mut()
            .insert(ids::NUGET.to_owned(), self.nuget.window.clone().into());
        cx.subscribe_in(
            &self.nuget.window,
            window,
            |shell, _, e: &WindowEvent, window, cx| {
                shell.on_nuget_window_event(e.clone(), window, cx)
            },
        )
        .detach();
        cx.subscribe(
            &self.nuget.sources_page,
            |shell, _, e: &SourcesEvent, cx| shell.on_sources_page_event(e.clone(), cx),
        )
        .detach();
        cx.observe(&self.nuget.window, |_, _, cx| cx.notify())
            .detach();
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        for e in batch {
                            shell.on_nuget_event(e, window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let job_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(NuGetJob { request, reply }) = jobs.next().await {
                let outcome = this
                    .update_in(cx, |shell, window, cx| {
                        shell.apply_nuget_manage(request, window, cx)
                    })
                    .unwrap_or_else(|_| Err(CommandError::Failed("the window is closed".into())));
                let _ = reply.send(outcome);
            }
        });
        self._tasks.push(event_task);
        self._tasks.push(job_task);
    }

    /// `nuget.*` settings, from the store.
    pub(super) fn nuget_apply_settings(&mut self) {
        let s = self.settings.lock();
        self.nuget.service.set_settings(NuGetSettings {
            include_prerelease: s.bool("nuget.includePrerelease"),
            restore_on_change: s.bool("nuget.restoreOnChange"),
            lock_files: if s.string("nuget.lockFiles") == "ignore" {
                host::NuGetLockFiles::Ignore
            } else {
                host::NuGetLockFiles::Respect
            },
        });
    }

    /// The UI's `eludite.nuget.*`: Manage NuGet Packages applies here; the other commands run on a thread of their
    /// own. View > ... `eludite.view.show` of the window opens it with its last scope. Returns whether it handled
    /// `command`.
    pub(super) fn run_nuget(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if command == eludite_commands::view::SHOW
            && args.get("id").and_then(Value::as_str) == Some(ids::NUGET)
        {
            let request = NuGetRequest::Manage {
                project: match self.nuget.window.read(cx).scope() {
                    Scope::Project { path, .. } => Some(path.clone()),
                    Scope::Solution => None,
                },
                solution: *self.nuget.window.read(cx).scope() == Scope::Solution,
                tab: None,
                query: None,
            };
            let outcome = self.apply_nuget_manage(request, window, cx);
            if let Err(e) = outcome {
                self.status.set(eludite_ui::slots::STATE, e.to_string());
            }
            return true;
        }
        if !cmds::ALL.contains(&command) {
            return false;
        }
        let args = std::mem::take(args);
        if command == cmds::MANAGE {
            let outcome = cmds::parse(command, args.clone())
                .and_then(|request| self.apply_nuget_manage(request, window, cx));
            stage(outcome);
            if let Err(e) = self.commands.invoke(command, args) {
                self.status.set(eludite_ui::slots::STATE, e.to_string());
            }
            cx.notify();
            return true;
        }
        let pending = match command {
            cmds::SEARCH => Pending::Search,
            cmds::INSTALLED => Pending::Installed,
            cmds::UPDATES => Pending::Updates,
            cmds::SOURCES => Pending::SourcesPage,
            _ => Pending::Change(
                cmds::spec(command)
                    .title
                    .trim_start_matches("NuGet: ")
                    .to_owned(),
            ),
        };
        self.nuget_spawn(vec![(command.to_owned(), args)], pending, cx);
        true
    }

    /// Run `calls` on the bus, in order, on a thread of their own; the last answer (or the first failure) comes back
    /// as [`NuGetEvent::Answer`].
    fn nuget_spawn(
        &mut self,
        calls: Vec<(String, Value)>,
        pending: Pending,
        cx: &mut Context<Self>,
    ) {
        let token = self.nuget.next_token;
        self.nuget.next_token += 1;
        if pending == Pending::Search {
            self.nuget.search_token = token;
        }
        let busy = match &pending {
            Pending::Search => Some("Searching".to_owned()),
            Pending::Installed | Pending::Updates => Some("Loading".to_owned()),
            Pending::Change(label) => Some(format!("{label}: working")),
            Pending::SourcesPage | Pending::SourceNames => None,
        };
        if busy.is_some() {
            self.nuget.window.update(cx, |w, cx| w.set_busy(busy, cx));
        }
        self.nuget.pending.insert(token, pending);
        let commands = self.commands.clone();
        let events = self.nuget.events.clone();
        let spawned = thread::Builder::new()
            .name("eludite-nuget-call".into())
            .spawn(move || {
                let mut result = Ok(Value::Null);
                for (command, args) in calls {
                    result = commands.invoke(&command, args).map_err(|e| e.to_string());
                    if result.is_err() {
                        break;
                    }
                }
                let _ = events.unbounded_send(NuGetEvent::Answer { token, result });
            });
        if spawned.is_err() {
            self.nuget.pending.remove(&token);
        }
    }

    /// Manage NuGet Packages: open the window (a document tab) for the scope, and load what its tab shows.
    pub(super) fn apply_nuget_manage(
        &mut self,
        request: NuGetRequest,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let NuGetRequest::Manage {
            project,
            solution,
            tab,
            query,
        } = request
        else {
            return Err(CommandError::InvalidInput(
                "not eludite.nuget.manage".into(),
            ));
        };
        if self.solution.is_none() {
            return Err(CommandError::Failed(
                "no solution is open: Manage NuGet Packages needs a .NET solution".into(),
            ));
        }
        let projects = self.nuget.service.projects();
        let scope = if solution {
            Scope::Solution
        } else {
            let chosen = match project {
                Some(p) => Some(self.nuget.service.resolve_project(&p)?),
                // Project > Manage NuGet Packages...: the active document's project, else the startup project.
                None => self
                    .active_document()
                    .and_then(|d| {
                        self.last_tree
                            .as_ref()
                            .and_then(|t| project_of(t, Path::new(&d)))
                    })
                    .or_else(|| {
                        self.default_startup
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                    }),
            };
            match chosen.and_then(|path| projects.iter().find(|(_, p)| *p == path).cloned()) {
                Some((name, path)) => Scope::Project { name, path },
                None => Scope::Solution,
            }
        };
        let title = scope.title();
        let prerelease = self.nuget.service.settings().include_prerelease;
        let rescoped = *self.nuget.window.read(cx).scope() != scope;
        // The tab's title names the scope: reopened when it changes.
        if rescoped {
            self.controller.close_document(ids::NUGET);
        }
        self.controller.open_document(ids::NUGET, &title);
        self.nuget.window.update(cx, |w, cx| {
            w.open(scope, projects, tab, query.clone(), prerelease, cx)
        });
        let first = !self.nuget.opened;
        self.nuget.opened = true;
        if first || rescoped {
            self.nuget_spawn(
                vec![(cmds::SOURCES.into(), json!({}))],
                Pending::SourceNames,
                cx,
            );
        }
        self.nuget_load_tab(first || rescoped || query.is_some(), cx);
        Ok(NuGetOutput::Manage(json!({
            "id": ids::NUGET,
            "title": title,
            "state": "document",
            "active": true
        })))
    }

    /// Load what the window's tab shows: Installed always (Consolidate and the badges read it), Updates on its tab,
    /// a search on Browse when `search`.
    fn nuget_load_tab(&mut self, search: bool, cx: &mut Context<Self>) {
        let (tab, project) = {
            let w = self.nuget.window.read(cx);
            (
                w.tab(),
                match w.scope() {
                    Scope::Project { path, .. } => Some(path.clone()),
                    Scope::Solution => None,
                },
            )
        };
        let mut installed = json!({ "vulnerabilities": true });
        if let Some(p) = &project {
            installed["project"] = json!(p);
        }
        self.nuget_spawn(
            vec![(cmds::INSTALLED.into(), installed)],
            Pending::Installed,
            cx,
        );
        match tab {
            Tab::Updates => self.nuget_updates(cx),
            Tab::Browse if search => self.nuget_search(cx),
            _ => {}
        }
    }

    fn nuget_updates(&mut self, cx: &mut Context<Self>) {
        let (prerelease, project) = {
            let w = self.nuget.window.read(cx);
            (
                w.prerelease(),
                match w.scope() {
                    Scope::Project { path, .. } => Some(path.clone()),
                    Scope::Solution => None,
                },
            )
        };
        let mut args = json!({ "prerelease": prerelease });
        if let Some(p) = project {
            args["project"] = json!(p);
        }
        self.nuget_spawn(vec![(cmds::UPDATES.into(), args)], Pending::Updates, cx);
    }

    fn nuget_search(&mut self, cx: &mut Context<Self>) {
        self.nuget.debounce = None;
        let args = {
            let w = self.nuget.window.read(cx);
            let mut args = json!({ "query": w.query(), "prerelease": w.prerelease(), "max_items": WINDOW_RESULTS });
            if let Some(s) = w.source() {
                args["source"] = json!(s);
            }
            args
        };
        self.nuget_spawn(vec![(cmds::SEARCH.into(), args)], Pending::Search, cx);
    }

    fn on_nuget_window_event(
        &mut self,
        event: WindowEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            WindowEvent::QueryChanged => {
                self.nuget.debounce = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SEARCH_DEBOUNCE).await;
                    let _ = this.update(cx, |shell, cx| shell.nuget_search(cx));
                }));
            }
            WindowEvent::Search => self.nuget_search(cx),
            WindowEvent::Refresh => {
                let tab = self.nuget.window.read(cx).tab();
                self.nuget_load_tab(tab == Tab::Browse, cx);
            }
            WindowEvent::Install {
                id,
                version,
                projects,
            } => self.nuget_spawn(
                vec![(
                    cmds::INSTALL.into(),
                    json!({ "package": id, "version": version, "projects": projects }),
                )],
                Pending::Change(format!("Installing {id} {version}")),
                cx,
            ),
            WindowEvent::Update {
                id,
                version,
                projects,
            } => {
                let calls = projects
                    .iter()
                    .map(|p| {
                        (
                            cmds::UPDATE.to_owned(),
                            json!({ "package": id, "version": version, "project": p }),
                        )
                    })
                    .collect();
                self.nuget_spawn(
                    calls,
                    Pending::Change(format!("Updating {id} to {version}")),
                    cx,
                )
            }
            WindowEvent::Uninstall { id, projects } => self.nuget_spawn(
                vec![(
                    cmds::UNINSTALL.into(),
                    json!({ "package": id, "projects": projects }),
                )],
                Pending::Change(format!("Uninstalling {id}")),
                cx,
            ),
            WindowEvent::Consolidate {
                id,
                version,
                projects,
            } => {
                // Consolidate's Install: the checked projects to the version, as one update each.
                let calls = projects
                    .iter()
                    .map(|p| {
                        (
                            cmds::UPDATE.to_owned(),
                            json!({ "package": id, "version": version, "project": p }),
                        )
                    })
                    .collect::<Vec<_>>();
                let calls = if calls.is_empty() {
                    vec![(
                        cmds::CONSOLIDATE.to_owned(),
                        json!({ "package": id, "version": version }),
                    )]
                } else {
                    calls
                };
                self.nuget_spawn(
                    calls,
                    Pending::Change(format!("Consolidating {id} on {version}")),
                    cx,
                )
            }
            WindowEvent::UpdateSelected { ids } => {
                let (all, project, prerelease) = {
                    let w = self.nuget.window.read(cx);
                    let listed: HashSet<String> = w
                        .rows()
                        .iter()
                        .filter_map(|r| r.id().map(str::to_owned))
                        .collect();
                    (
                        ids.iter().cloned().collect::<HashSet<_>>() == listed,
                        match w.scope() {
                            Scope::Project { path, .. } => Some(path.clone()),
                            Scope::Solution => None,
                        },
                        w.prerelease(),
                    )
                };
                // The newest versions as the check box lists them.
                let calls = if all {
                    let mut args = json!({ "all": true, "prerelease": prerelease });
                    if let Some(p) = &project {
                        args["project"] = json!(p);
                    }
                    vec![(cmds::UPDATE.to_owned(), args)]
                } else {
                    ids.iter()
                        .map(|id| {
                            let mut args = json!({ "package": id, "prerelease": prerelease });
                            if let Some(p) = &project {
                                args["project"] = json!(p);
                            }
                            (cmds::UPDATE.to_owned(), args)
                        })
                        .collect()
                };
                self.nuget_spawn(
                    calls,
                    Pending::Change(format!("Updating {} package(s)", ids.len())),
                    cx,
                )
            }
            WindowEvent::Icons(urls) => {
                let service = self.nuget.service.clone();
                let events = self.nuget.events.clone();
                let _ = thread::Builder::new()
                    .name("eludite-nuget-icons".into())
                    .spawn(move || {
                        for url in urls {
                            let path = service.icon(&url);
                            let _ = events.unbounded_send(NuGetEvent::Icon { url, path });
                        }
                    });
            }
            WindowEvent::PackageSources => {
                self.run(
                    eludite_commands::settings::OPTIONS,
                    json!({ "section": eludite_commands::settings::PACKAGE_SOURCES_PAGE }),
                    window,
                    cx,
                );
            }
        }
    }

    fn on_sources_page_event(&mut self, event: SourcesEvent, cx: &mut Context<Self>) {
        let args = match event {
            SourcesEvent::Add { name, url } => json!({ "action": "add", "name": name, "url": url }),
            SourcesEvent::Remove { name } => json!({ "action": "remove", "name": name }),
            SourcesEvent::SetEnabled { name, enabled } => json!({
                "action": if enabled { "enable" } else { "disable" }, "name": name
            }),
        };
        self.nuget_spawn(vec![(cmds::SOURCES.into(), args)], Pending::SourcesPage, cx);
    }

    /// The Options dialog's Package Sources page: its view, filled when the page is first built.
    pub(super) fn nuget_options_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyView {
        if self.solution.is_some() {
            self.nuget_spawn(
                vec![(cmds::SOURCES.into(), json!({}))],
                Pending::SourcesPage,
                cx,
            );
        } else {
            self.nuget.sources_page.update(cx, |p, cx| {
                p.set_message(
                    Some(
                        "Open a solution to see its package sources (its NuGet.config chain)."
                            .into(),
                    ),
                    cx,
                )
            });
        }
        self.nuget.sources_page.clone().into()
    }

    fn on_nuget_event(&mut self, event: NuGetEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event {
            NuGetEvent::Answer { token, result } => {
                let Some(pending) = self.nuget.pending.remove(&token) else {
                    return;
                };
                self.on_nuget_answer(token, pending, result, cx);
            }
            NuGetEvent::Restored { diagnostics } => {
                self.nuget.diagnostics = diagnostics;
                self.update_error_list(cx);
            }
            NuGetEvent::Changed => {
                if self.nuget.opened {
                    let tab = self.nuget.window.read(cx).tab();
                    self.nuget_load_tab(false, cx);
                    if tab != Tab::Updates {
                        // The Updates count in the tab header follows too.
                        self.nuget_updates(cx);
                    }
                }
            }
            NuGetEvent::Metadata { packages } => {
                for p in packages {
                    let key = format!("{}/{}", p.id.to_lowercase(), p.version);
                    if p.deprecation.is_some()
                        || p.vulnerabilities.as_ref().is_some_and(|v| !v.is_empty())
                    {
                        self.nuget.warnings.insert(key);
                    }
                }
                let warnings = self.nuget.warnings.clone();
                self.explorer
                    .update(cx, |e, cx| e.set_package_warnings(warnings, cx));
            }
            NuGetEvent::Icon { url, path } => {
                self.nuget
                    .window
                    .update(cx, |w, cx| w.set_icon(url, path, cx));
            }
        }
    }

    fn on_nuget_answer(
        &mut self,
        token: u64,
        pending: Pending,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let window = self.nuget.window.clone();
        let busy_done = !matches!(pending, Pending::SourcesPage | Pending::SourceNames);
        if busy_done
            && self
                .nuget
                .pending
                .values()
                .all(|p| matches!(p, Pending::SourcesPage | Pending::SourceNames))
        {
            window.update(cx, |w, cx| w.set_busy(None, cx));
        }
        match (pending, result) {
            (Pending::Search, _) if token != self.nuget.search_token => {}
            (Pending::Search, Ok(v)) => {
                if let Ok(out) = serde_json::from_value::<SearchOutput>(v) {
                    let failed = out.sources.iter().filter(|s| s.error.is_some()).count();
                    window.update(cx, |w, cx| {
                        w.set_message(
                            (failed > 0).then(|| {
                                format!(
                                    "{failed} package source(s) failed; the others' results show."
                                )
                            }),
                            cx,
                        );
                        w.set_browse(out, cx);
                    });
                }
            }
            (Pending::Installed, Ok(v)) => {
                if let Ok(out) = serde_json::from_value::<InstalledOutput>(v) {
                    window.update(cx, |w, cx| w.set_installed(out, cx));
                }
            }
            (Pending::Updates, Ok(v)) => {
                if let Ok(out) = serde_json::from_value::<UpdatesOutput>(v) {
                    window.update(cx, |w, cx| w.set_updates(out, cx));
                }
            }
            (Pending::Change(label), Ok(v)) => {
                let out = serde_json::from_value::<ChangeOutput>(v).ok();
                let text = match &out {
                    Some(ChangeOutput {
                        message: Some(m), ..
                    }) => m.clone(),
                    Some(ChangeOutput {
                        restore: Some(r), ..
                    }) if r.result != "succeeded" => format!(
                        "{label}: the project files changed, but the restore failed with {} error(s) (Error List).",
                        r.errors
                    ),
                    _ => format!("{label}: done."),
                };
                window.update(cx, |w, cx| w.set_message(Some(text), cx));
                self.status.set(eludite_ui::slots::STATE, "Ready");
            }
            (Pending::SourceNames, Ok(v)) => {
                if let Ok(out) = serde_json::from_value::<SourcesOutput>(v.clone()) {
                    let names = out
                        .sources
                        .iter()
                        .filter(|s| s.enabled)
                        .map(|s| s.name.clone())
                        .collect();
                    window.update(cx, |w, cx| w.set_sources(names, cx));
                    self.nuget
                        .sources_page
                        .update(cx, |p, cx| p.set_sources(out, cx));
                }
            }
            (Pending::SourcesPage, Ok(v)) => {
                if let Ok(out) = serde_json::from_value::<SourcesOutput>(v) {
                    let names = out
                        .sources
                        .iter()
                        .filter(|s| s.enabled)
                        .map(|s| s.name.clone())
                        .collect();
                    window.update(cx, |w, cx| w.set_sources(names, cx));
                    self.nuget
                        .sources_page
                        .update(cx, |p, cx| p.set_sources(out, cx));
                }
            }
            (Pending::SourcesPage | Pending::SourceNames, Err(e)) => {
                self.nuget
                    .sources_page
                    .update(cx, |p, cx| p.set_message(Some(e), cx));
            }
            (_, Err(e)) => {
                window.update(cx, |w, cx| w.set_message(Some(e.clone()), cx));
                self.status.set(eludite_ui::slots::STATE, e);
            }
        }
        cx.notify();
    }

    /// `eludite/nuget/update`: NuGet's output to the Package Manager source (any caller's, current generation or
    /// not: it is a log); metadata to the Dependencies glyphs.
    pub(super) fn on_nuget_update(
        &mut self,
        update: host::NuGetUpdate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match update.kind {
            host::NuGetUpdateKind::Output => {
                if let Some(text) = &update.text {
                    self.output
                        .update(cx, |o, cx| o.append(OutputSource::PackageManager, text, cx));
                }
            }
            host::NuGetUpdateKind::Progress => {
                if let Some(m) = update.message {
                    self.status
                        .set(eludite_ui::slots::STATE, format!("NuGet: {m}\u{2026}"));
                    cx.notify();
                }
            }
            host::NuGetUpdateKind::Metadata => {
                if update.generation == self.generation {
                    self.on_nuget_event(
                        NuGetEvent::Metadata {
                            packages: update.packages.unwrap_or_default(),
                        },
                        window,
                        cx,
                    );
                }
            }
        }
    }

    /// `eludite/nuget/credentials`: the prompt (one at a time; the others wait their turn).
    pub(super) fn on_nuget_credentials(
        &mut self,
        id: Id,
        params: host::NuGetCredentialsParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.nuget.prompt.is_some() {
            self.nuget.queued.push_back((id, params));
            return;
        }
        let theme = self.theme;
        let prompt = cx.new(|cx| {
            CredentialPrompt::new(
                theme,
                params.source.clone(),
                params.host.clone(),
                params.is_retry,
                String::new(),
                cx,
            )
        });
        let answer_id = id.clone();
        cx.subscribe_in(
            &prompt,
            window,
            move |shell, _, e: &CredentialEvent, window, cx| {
                shell.nuget_credential_answer(answer_id.clone(), e.clone(), window, cx)
            },
        )
        .detach();
        prompt.focus_handle(cx).focus(window, cx);
        self.nuget.prompt = Some((id, prompt));
        cx.notify();
    }

    fn nuget_credential_answer(
        &mut self,
        id: Id,
        event: CredentialEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.nuget.prompt.as_ref().is_none_or(|(p, _)| *p != id) {
            return;
        }
        self.nuget.prompt = None;
        let answer = match event {
            CredentialEvent::Ok { username, password } => Some(host::NuGetCredentialsAnswer {
                username: Some(username),
                password: Some(password),
                remember: Some(true),
                canceled: None,
            }),
            CredentialEvent::Cancel => None,
        };
        self.session.respond_nuget_credentials(id, answer);
        self.focus.focus(window, cx);
        if let Some((next, params)) = self.nuget.queued.pop_front() {
            self.on_nuget_credentials(next, params, window, cx);
        }
        cx.notify();
    }

    /// The Error List's NuGet rows: the last restore's errors and warnings, at the file NuGet names (a project file).
    pub(super) fn nuget_error_rows(&self) -> Vec<ErrorRow> {
        self.nuget
            .diagnostics
            .iter()
            .filter_map(|d| {
                let path = PathBuf::from(d.file.as_ref().or(d.project.as_ref())?);
                let project = path
                    .file_stem()
                    .filter(|_| {
                        path.extension()
                            .is_some_and(|e| e.to_string_lossy().ends_with("proj"))
                    })
                    .map(|s| s.to_string_lossy().into_owned());
                Some(ErrorRow {
                    severity: match d.severity {
                        host::BuildDiagnosticSeverity::Error => Severity::Error,
                        host::BuildDiagnosticSeverity::Warning => Severity::Warning,
                        host::BuildDiagnosticSeverity::Message => Severity::Message,
                    },
                    code: d.code.clone(),
                    message: d.message.clone(),
                    project,
                    file: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    path,
                    line: d.line.unwrap_or(1).max(1),
                    column: d.column.unwrap_or(1).max(1),
                    source: RowSource::NuGet,
                })
            })
            .collect()
    }
}

/// The project of the open solution's tree that lists `file`.
fn project_of(tree: &host::SolutionTree, file: &Path) -> Option<String> {
    let file = super::documents::normalize_path(file);
    tree.projects
        .iter()
        .find(|p| {
            p.files
                .iter()
                .any(|f| super::documents::normalize_path(Path::new(&f.path)) == file)
        })
        .map(|p| p.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_sort_releases_after_their_prereleases() {
        let mut v = vec!["1.0.0", "2.0.0-beta.1", "1.10.0", "1.2.0", "2.0.0"];
        v.sort_by_key(|x| version_key(x));
        assert_eq!(v, ["1.0.0", "1.2.0", "1.10.0", "2.0.0-beta.1", "2.0.0"]);
    }
}
