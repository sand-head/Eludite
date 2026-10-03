//! The startup project and the Workspace context menu's commands that are not builds (brief 0020):
//! `eludite.workspace.set_startup_project` and `eludite.workspace.open_containing_folder`.
//!
//! - **Startup project.** Set as Startup Project names the project F5 and Ctrl+F5 build and run. It is kept per
//!   solution and per user, as Visual Studio keeps it in the `.suo`: in the solution's file under
//!   `<config dir>/eludite/breakpoints/solutions/`, beside its breakpoints and watches (`debug::state::Persisted`).
//!   Without one, the startup project is the solution's first executable project, found off the UI thread when the
//!   tree arrives. A member package of the open folder's Cargo workspace can be the startup project too (brief 0029,
//!   `debug::native`), kept as its `Cargo.toml`. Workspace draws the startup project bold, and `eludite.workspace.tree` marks it `startup`.
//! - **Open Containing Folder** hands the folder to the system's file manager (`xdg-open`, Explorer, Finder) on a
//!   thread of its own; tests replace the opener.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eludite_commands::CommandError;
use eludite_commands::project::{
    ContainingFolderOutput, ProjectOutput, ProjectRequest, ProjectTarget, StartupProjectOutput,
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
                self.debug_persist(cx);
                self.refresh_startup(cx);
                super::documents::trace(format_args!("startup project: {}", path.display()));
                Ok(ProjectOutput::StartupProject(StartupProjectOutput {
                    project: name,
                    path: path.to_string_lossy().into_owned(),
                    solution: solution.to_string_lossy().into_owned(),
                }))
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
        let startup = self.startup_project();
        self.explorer
            .update(cx, |e, cx| e.set_startup(startup.clone(), cx));
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
