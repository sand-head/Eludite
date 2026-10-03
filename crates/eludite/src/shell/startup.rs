//! The startup project and the Workspace context menu's commands that are not builds (brief 0020):
//! `eludite.workspace.set_startup_project` and `eludite.workspace.open_containing_folder`.
//!
//! - **Startup project.** Set as Startup Project names the project F5 and Ctrl+F5 build and run. It is kept per
//!   solution and per user, as Visual Studio keeps it in the `.suo`: in the solution's file under
//!   `<config dir>/eludite/breakpoints/solutions/`, beside its breakpoints and watches (`debug::state::Persisted`).
//!   Without one, the startup project is the solution's first executable project, found off the UI thread when the
//!   tree arrives. A member package of the open folder's Cargo workspace can be the startup project too (brief 0029,
//!   `debug::native`), kept as its `Cargo.toml`. Workspace draws the startup project bold, and `eludite.workspace.tree` marks it `startup`.
//! - **Multiple startup projects** (brief 0028). `projects` sets Visual Studio's multiple startup projects, each with
//!   an action (Start, Start without debugging, None); F5 starts each in a debugging session of its own. They persist
//!   in the same store (version 3), `startup_project` naming the first that starts for older readers; Workspace draws
//!   each bold. Project > Set Startup Projects... (the command without arguments, from the UI) opens the Startup
//!   Projects dialog (`eludite_ui::startup`), whose OK runs the command with `projects`.
//! - **Open Containing Folder** hands the folder to the system's file manager (`xdg-open`, Explorer, Finder) on a
//!   thread of its own; tests replace the opener.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eludite_commands::CommandError;
use eludite_commands::project::{
    ContainingFolderOutput, ProjectOutput, ProjectRequest, ProjectTarget, StartupAction,
    StartupEntry, StartupProjectOutput, StartupProjectRow,
};
use futures::channel::mpsc::UnboundedSender;
use gpui::{AppContext as _, Context, Window};

use super::Shell;
use super::documents::normalize_path;

/// Shows a folder in the system's file manager.
pub type FolderOpener = Arc<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>;

/// The system's file manager, started on a thread of its own (the UI never waits for it).
pub fn system_folder_opener() -> FolderOpener {
    Arc::new(|folder: &Path| {
        let program = if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let folder = folder.to_path_buf();
        std::thread::Builder::new()
            .name("eludite-open-folder".into())
            .spawn(move || {
                let started = std::process::Command::new(program)
                    .arg(&folder)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .and_then(|mut c| c.wait());
                if let Err(e) = started {
                    eprintln!("eludite: {program} {}: {e}", folder.display());
                }
            })?;
        Ok(())
    })
}

type Outcome = Result<ProjectOutput, CommandError>;

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on the UI thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// A project command from another thread (an agent), for the UI thread to apply.
pub struct ProjectJob {
    pub request: ProjectRequest,
    pub reply: mpsc::SyncSender<Outcome>,
}

/// The shell's [`ProjectTarget`]: applied on the UI thread (it changes the debugger's persisted state and the
/// Workspace window).
pub struct ProjectBus {
    pub ui_thread: std::thread::ThreadId,
    pub jobs: UnboundedSender<ProjectJob>,
}

impl ProjectTarget for ProjectBus {
    fn apply(&self, request: ProjectRequest) -> Outcome {
        if request == ProjectRequest::StartupProjectsDialog
            && std::thread::current().id() != self.ui_thread
        {
            return Err(CommandError::InvalidInput(
                "name the startup project (`project`) or the startup projects with their actions (`projects`); \
                 without either the Startup Projects dialog opens, which is the person's"
                    .into(),
            ));
        }
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(
                    "workspace project commands run on the UI thread through the shell".into(),
                ))
            });
        }
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(ProjectJob { request, reply })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| CommandError::Failed("the UI did not answer".into()))?
    }
}

impl Shell {
    /// Apply a project command (the UI-thread half of [`ProjectBus`]).
    pub(super) fn apply_project(
        &mut self,
        request: ProjectRequest,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        match request {
            ProjectRequest::SetStartupProject { project } => {
                // A member package of the open folder's Cargo workspace (brief 0029).
                if let Some(out) = self.set_cargo_startup(&project, cx) {
                    return out;
                }
                let solution = self.solution.clone().ok_or_else(|| {
                    CommandError::Failed(
                        "no .NET solution is open: there is no startup project to set".into(),
                    )
                })?;
                let (name, path) = self.find_solution_project(&project)?;
                self.debug.model.startup_project = Some(path.to_string_lossy().into_owned());
                // One startup project replaces multiple ones (brief 0028).
                self.debug.model.startup_projects.clear();
                self.debug_persist(cx);
                self.refresh_startup(cx);
                super::documents::trace(format_args!("startup project: {}", path.display()));
                Ok(ProjectOutput::StartupProject(StartupProjectOutput {
                    projects: Vec::new(),
                    project: name,
                    path: path.to_string_lossy().into_owned(),
                    solution: solution.to_string_lossy().into_owned(),
                }))
            }
            ProjectRequest::SetStartupProjects { projects } => {
                self.set_startup_projects(projects, cx)
            }
            ProjectRequest::StartupProjectsDialog => {
                self.open_startup_dialog(_window, cx)?;
                self.startup_output()
            }
            ProjectRequest::OpenContainingFolder { path } => {
                let p = Path::new(&path);
                let folder = if p.is_dir() {
                    p.to_path_buf()
                } else {
                    p.parent().map(Path::to_path_buf).unwrap_or_default()
                };
                (self.folder_opener)(&folder)
                    .map_err(|e| CommandError::Failed(format!("{}: {e}", folder.display())))?;
                Ok(ProjectOutput::ContainingFolder(ContainingFolderOutput {
                    folder: folder.to_string_lossy().into_owned(),
                }))
            }
        }
    }

    /// A .NET project of the open solution by name or by project file path (absolute or relative to the solution).
    fn find_solution_project(&self, project: &str) -> Result<(String, PathBuf), CommandError> {
        let tree = self.tree.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(p) = tree.projects.iter().find(|p| p.name == project) {
            return Ok((p.name.clone(), PathBuf::from(&p.path)));
        }
        let given = Path::new(project);
        let abs = if given.is_absolute() {
            given.to_path_buf()
        } else {
            self.solution_dir().unwrap_or_default().join(given)
        };
        let wanted = normalize_path(&abs);
        tree.projects
            .iter()
            .find(|p| normalize_path(Path::new(&p.path)) == wanted)
            .map(|p| (p.name.clone(), PathBuf::from(&p.path)))
            .ok_or_else(|| {
                CommandError::InvalidInput(format!(
                    "{project} is not a project of the open solution (only .NET projects can be started)"
                ))
            })
    }

    /// A project of the startup set by name or path: a Cargo workspace member, else a project of the open solution.
    fn find_startup_candidate(&self, project: &str) -> Result<(String, PathBuf), CommandError> {
        if let Some(m) = self
            .cargo_context()
            .and_then(|c| c.member(project).cloned())
        {
            return Ok((m.name, m.manifest));
        }
        if self.solution.is_none() {
            return Err(CommandError::Failed(
                "no .NET solution is open: there is no startup project to set".into(),
            ));
        }
        self.find_solution_project(project)
    }

    /// Multiple startup projects (brief 0028): each project with its action, persisted per solution; `startup_project`
    /// names the first that starts (with action `start` first).
    fn set_startup_projects(
        &mut self,
        projects: Vec<StartupEntry>,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let mut resolved: Vec<(String, StartupAction)> = Vec::new();
        for e in &projects {
            let (_, path) = self.find_startup_candidate(&e.project)?;
            let path = path.to_string_lossy().into_owned();
            resolved.retain(|(p, _)| *p != path);
            resolved.push((path, e.action));
        }
        let first = resolved
            .iter()
            .find(|(_, a)| *a == StartupAction::Start)
            .or_else(|| resolved.iter().find(|(_, a)| *a != StartupAction::None))
            .map(|(p, _)| p.clone());
        self.debug.model.startup_project = first;
        self.debug.model.startup_projects = resolved;
        self.debug_persist(cx);
        self.refresh_startup(cx);
        super::documents::trace(format_args!(
            "startup projects: {:?}",
            self.debug.model.startup_projects
        ));
        self.startup_output()
    }

    /// `set_startup_project`'s answer for the current startup projects.
    fn startup_output(&self) -> Outcome {
        let solution = self
            .solution
            .clone()
            .or_else(|| self.native_store_key())
            .ok_or_else(|| {
                CommandError::Failed(
                    "no .NET solution is open: there is no startup project to set".into(),
                )
            })?;
        let tree = self.tree.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let name_of = |path: &str| {
            tree.projects
                .iter()
                .find(|p| normalize_path(Path::new(&p.path)) == normalize_path(Path::new(path)))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| {
                    let p = Path::new(path);
                    if p.file_name().is_some_and(|n| n == "Cargo.toml") {
                        p.parent().and_then(|d| d.file_name())
                    } else {
                        p.file_stem()
                    }
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
                })
        };
        let set = self.debug.model.startup_set();
        let rank = |path: &str| {
            tree.projects
                .iter()
                .position(|p| normalize_path(Path::new(&p.path)) == normalize_path(Path::new(path)))
                .unwrap_or(usize::MAX)
        };
        let mut rows: Vec<StartupProjectRow> = if self.debug.model.startup_projects.is_empty() {
            Vec::new()
        } else {
            set.iter()
                .map(|(path, action)| StartupProjectRow {
                    project: name_of(path),
                    path: path.clone(),
                    action: *action,
                })
                .collect()
        };
        rows.sort_by_key(|r| rank(&r.path));
        let first = self
            .debug
            .model
            .startup_project
            .clone()
            .map(PathBuf::from)
            .or_else(|| self.default_startup.clone())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(ProjectOutput::StartupProject(StartupProjectOutput {
            project: name_of(&first),
            path: first,
            solution: solution.to_string_lossy().into_owned(),
            projects: rows,
        }))
    }

    /// Project > Set Startup Projects...: the dialog with the solution's projects and their actions.
    fn open_startup_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        use eludite_ui::startup::{
            ACTION_NONE, ACTION_START, ACTION_START_WITHOUT_DEBUGGING, StartupProjectsDialog,
            StartupRow,
        };
        let solution = self
            .solution
            .clone()
            .or_else(|| self.native_store_key())
            .ok_or_else(|| {
                CommandError::Failed(
                    "no solution is open: there are no startup projects to set".into(),
                )
            })?;
        let set: Vec<(PathBuf, StartupAction)> = self
            .debug
            .model
            .startup_set()
            .into_iter()
            .map(|(p, a)| (normalize_path(Path::new(&p)), a))
            .collect();
        let default = self.startup_project().map(|p| normalize_path(&p));
        let tree = self.tree.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let rows: Vec<StartupRow> = tree
            .projects
            .iter()
            .map(|p| {
                let path = normalize_path(Path::new(&p.path));
                let action = match set.iter().find(|(s, _)| *s == path) {
                    Some((_, StartupAction::Start)) => ACTION_START,
                    Some((_, StartupAction::StartWithoutDebugging)) => {
                        ACTION_START_WITHOUT_DEBUGGING
                    }
                    Some((_, StartupAction::None)) => ACTION_NONE,
                    None if set.is_empty() && default.as_ref() == Some(&path) => ACTION_START,
                    None => ACTION_NONE,
                };
                StartupRow {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    action,
                }
            })
            .collect();
        let title = solution
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let theme = self.theme;
        let dialog = cx.new(|cx| StartupProjectsDialog::new(theme, title, rows, cx));
        cx.subscribe_in(&dialog, window, Self::on_startup_dialog_event)
            .detach();
        gpui::Focusable::focus_handle(dialog.read(cx), cx).focus(window, cx);
        self.debug.startup_dialog = Some(dialog);
        cx.notify();
        Ok(())
    }

    /// The dialog's OK runs `eludite.workspace.set_startup_project` with `projects` through the bus.
    fn on_startup_dialog_event(
        &mut self,
        dialog: &gpui::Entity<eludite_ui::startup::StartupProjectsDialog>,
        event: &eludite_ui::startup::StartupProjectsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use eludite_ui::startup::{
            ACTION_START, ACTION_START_WITHOUT_DEBUGGING, StartupProjectsEvent,
        };
        match event {
            StartupProjectsEvent::Apply(rows) => {
                let projects: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|r| {
                        let action = match r.action {
                            ACTION_START => "start",
                            ACTION_START_WITHOUT_DEBUGGING => "start_without_debugging",
                            _ => "none",
                        };
                        serde_json::json!({ "project": r.path, "action": action })
                    })
                    .collect();
                match self.invoke(
                    eludite_commands::project::SET_STARTUP_PROJECT,
                    serde_json::json!({ "projects": projects }),
                    window,
                    cx,
                ) {
                    Ok(_) => self.close_startup_dialog(window, cx),
                    Err(e) => dialog.update(cx, |d, cx| d.set_message(Some(e.to_string()), cx)),
                }
            }
            StartupProjectsEvent::Close => self.close_startup_dialog(window, cx),
        }
    }

    pub(super) fn close_startup_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.startup_dialog.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Every startup project's file: the multiple startup projects that start (brief 0028), else the startup project.
    pub fn startup_projects(&self) -> Vec<PathBuf> {
        if self.debug.model.startup_projects.is_empty() {
            return self.startup_project().into_iter().collect();
        }
        self.debug
            .model
            .startup_set()
            .into_iter()
            .map(|(p, _)| PathBuf::from(p))
            .collect()
    }

    /// The startup project: Set as Startup Project's, else the first executable project.
    pub fn startup_project(&self) -> Option<PathBuf> {
        self.debug
            .model
            .startup_project
            .clone()
            .map(PathBuf::from)
            .or_else(|| self.default_startup.clone())
    }

    /// Show the startup project bold in Workspace and in `eludite.workspace.tree`.
    pub(super) fn refresh_startup(&mut self, cx: &mut Context<Self>) {
        let startups = self.startup_projects();
        self.explorer
            .update(cx, |e, cx| e.set_startups(startups, cx));
        self.publish_workspace_tree();
        cx.notify();
    }

    /// A new tree: find the first executable project off the UI thread (it reads the project files).
    pub(super) fn find_default_startup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let projects: Vec<PathBuf> = self
            .tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .map(|p| PathBuf::from(&p.path))
            .collect();
        let generation = self.generation;
        let found =
            cx.background_spawn(async move { eludite_dap::launch::startup_project(&projects) });
        cx.spawn_in(window, async move |this, cx| {
            let found = found.await;
            let _ = this.update(cx, |shell, cx| {
                if shell.generation == generation {
                    shell.default_startup = found;
                    shell.refresh_startup(cx);
                }
            });
        })
        .detach();
    }
}
