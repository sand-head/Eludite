//! Build in the shell (brief 0017): the `eludite.build.*` and `eludite.output.*` commands, the build's state, the
//! Output window's Build and Host sources, the status bar's build slot, the configuration and platform dropdowns,
//! and build on save.
//!
//! - **One path for everyone.** Menus, keys and agents run the same commands. On the UI thread the shell applies a
//!   request first and stages the result for the bus handler (as `target` does); from another thread the request is
//!   posted to the UI as a [`BuildJob`].
//! - **The UI never waits.** Starting a build hands it to the session worker; the host's reply, output chunks,
//!   progress and result arrive as session events. A build command invoked off the UI thread (an agent) blocks *its*
//!   thread on [`BuildShared`] until the build finishes (or is canceled), unless it passed `wait: false`.
//! - **One build at a time.** While one runs, a second start is refused here (the host refuses one too), and the
//!   Build menu's start items are disabled ([`Builds::building`]); Cancel is enabled only then.
//! - **Two build systems** (brief 0019). MSBuild runs in the host; Cargo runs in the shell (`cargo_build`), reported
//!   in the same `eludite/build/*` shapes, so both stream into the same Output pane, Error List rows and status bar.
//!   Ctrl+Shift+B builds the active system: the one owning the active document (a file of a Cargo package: cargo; a
//!   file of the solution: MSBuild), or every system the workspace has, one after the other, when no document is
//!   active or it belongs to neither. Build Project builds the active document's project or Cargo package.
//! - **Recovery after a host restart** (brief 0020). When `eludite-host` exits mid-build, the build is kept until the
//!   restarted host answers `eludite/build/status`: a build that still runs is replayed (the Build pane is refilled
//!   with its output so far, and later chunks are applied from `nextSeq` on); otherwise the build is reported as
//!   ended, with the host's last result when it finished meanwhile.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::build::{
    BuildCommandOutput, BuildCommands, BuildKind, BuildRequest, BuildResultOutput,
    BuildSystemChoice, CancelOutput, MAX_RESULT_DIAGNOSTICS, OutputClearOutput, OutputShowOutput,
    OutputSource, ResultDiagnostic, ResultProject,
};
use eludite_commands::view::{ViewRequest, ViewTarget as _};
use eludite_docking::ids;
use eludite_lsp::host::{
    BuildDiagnostic, BuildDiagnosticSeverity, BuildFinished, BuildProgress, BuildResult,
    BuildStartParams, BuildStartResult, BuildSummary, BuildSystem, BuildTarget,
};
use eludite_ui::{Theme, toggle_button};
use futures::channel::mpsc::UnboundedSender;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, px,
};

use super::Shell;
use super::cargo_build::{self, CargoBuildSpec, CargoRun, Member};
use super::folder::CargoState;

/// Status bar slot: the build's state (left, after the solution's).
pub const BUILD_SLOT: &str = "build";

/// The build id of `--bench-output`'s stand-in build.
const BENCH_BUILD_ID: u64 = u64::MAX;

/// How long an agent's build command waits for the build at most.
const AGENT_BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How long a command from another thread waits for the UI thread to take it.
const UI_TIMEOUT: Duration = Duration::from_secs(30);

/// The configurations the dropdown offers (brief 0017: Debug and Release only).
pub const CONFIGURATIONS: [&str; 2] = ["Debug", "Release"];

/// Debug selectors of the toolbar.
pub const CONFIGURATION_BUTTON: &str = "build-configuration";
pub const PLATFORM_BUTTON: &str = "build-platform";

/// Debug selector of entry `ix` of the configuration (`kind` "configuration") or platform dropdown.
pub fn toolbar_item_selector(kind: &str, ix: usize) -> String {
    format!("build-{kind}-{ix}")
}

type Outcome = Result<BuildCommandOutput, CommandError>;

thread_local! {
    static STAGED: RefCell<Option<(Outcome, Option<u64>)>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some((outcome, None)));
}

/// A build or output command from another thread, for the UI thread to apply. The reply carries the build's ticket
/// when the request started one.
pub struct BuildJob {
    pub request: BuildRequest,
    pub reply: mpsc::SyncSender<(Outcome, Option<u64>)>,
}

/// What a waiting build command reads: the results of finished builds by ticket.
#[derive(Default)]
pub struct BuildShared {
    results: Mutex<VecDeque<(u64, BuildResultOutput)>>,
    changed: Condvar,
}

impl BuildShared {
    fn publish(&self, ticket: u64, result: BuildResultOutput) {
        let mut r = self.results.lock().unwrap_or_else(|e| e.into_inner());
        r.push_back((ticket, result));
        while r.len() > 16 {
            r.pop_front();
        }
        self.changed.notify_all();
    }

    /// Blocks until the build `ticket` finished, up to `timeout`.
    fn wait(&self, ticket: u64, timeout: Duration) -> Option<BuildResultOutput> {
        let deadline = Instant::now() + timeout;
        let mut r = self.results.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some((_, out)) = r.iter().find(|(t, _)| *t == ticket) {
                return Some(out.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            r = self
                .changed
                .wait_timeout(r, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// The shell's [`BuildCommands`].
pub struct BuildBus {
    pub ui_thread: ThreadId,
    pub jobs: UnboundedSender<BuildJob>,
    pub shared: Arc<BuildShared>,
}

impl BuildCommands for BuildBus {
    fn apply(&self, request: BuildRequest) -> Outcome {
        if std::thread::current().id() == self.ui_thread {
            return STAGED
                .with(|s| s.borrow_mut().take())
                .map(|(o, _)| o)
                .unwrap_or_else(|| {
                    Err(CommandError::Failed(format!(
                        "{} runs on the UI thread through the shell",
                        request.command()
                    )))
                });
        }
        // Off the UI thread (an agent): wait for the build unless told not to.
        let wait = matches!(request, BuildRequest::Start { wait, .. } if wait != Some(false));
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(BuildJob { request, reply })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        let (outcome, ticket) = rx
            .recv_timeout(UI_TIMEOUT)
            .map_err(|_| CommandError::Failed("the UI did not answer".into()))?;
        match (outcome, ticket) {
            (Ok(_), Some(ticket)) if wait => self
                .shared
                .wait(ticket, AGENT_BUILD_TIMEOUT)
                .map(|r| BuildCommandOutput::Result(Box::new(r)))
                .ok_or_else(|| CommandError::Failed("the build did not finish in time".into())),
            (outcome, _) => outcome,
        }
    }
}

/// The build that runs.
#[derive(Debug, Clone)]
pub struct CurrentBuild {
    pub ticket: u64,
    /// Which build system runs it (brief 0019).
    pub system: BuildSystem,
    /// What the command asked for (`all` while a build of every system runs).
    pub choice: BuildSystemChoice,
    /// The host's id, once its reply (or its first output chunk) arrived.
    pub id: Option<u64>,
    pub kind: BuildKind,
    pub path: PathBuf,
    pub configuration: String,
    pub platform: Option<String>,
    pub started: Option<BuildStartResult>,
    pub progress: Option<BuildProgress>,
    /// Output chunks with a lower seq are already shown (replayed from `eludite/build/status`).
    pub next_seq: u64,
}

/// When the steps of the last build happened (the budgets in the brief 0017 report).
#[derive(Debug, Default, Clone)]
// Read by the tests and the `--bench-build` harness.
#[allow(dead_code)]
pub struct BuildTimings {
    /// The build command was applied on the UI thread (the key press's handler).
    pub requested: Option<Instant>,
    /// The first output chunk was appended to the Output window.
    pub first_output: Option<Instant>,
    /// The pump received `eludite/build/finished`.
    pub finished_received: Option<Instant>,
    /// The Error List had the build's rows.
    pub rows_set: Option<Instant>,
    /// Output lines appended for the build.
    pub lines: usize,
}

/// The shell's build state.
pub struct Builds {
    pub shared: Arc<BuildShared>,
    /// True while a build runs; the menu bar's enabled check reads it.
    pub building: Arc<AtomicBool>,
    pub current: Option<CurrentBuild>,
    next_ticket: u64,
    /// The last finished build, and its diagnostics for the Error List (replaced by the next build's).
    pub last: Option<BuildFinished>,
    pub diagnostics: Vec<BuildDiagnostic>,
    /// The running Cargo build, to cancel it (brief 0019).
    pub cargo: Option<CargoRun>,
    /// The `cargo` to run: the setting `build.cargoPath` (or `ELUDITE_CARGO`), else `cargo` on PATH.
    pub cargo_program: std::ffi::OsString,
    next_cargo_id: u64,
    /// A build of every system: what is left to run, and what the finished ones produced.
    pub chain: Option<Chain>,
    /// The toolbar's selection.
    pub configuration: String,
    /// `None`: the solution's default platform.
    pub platform: Option<String>,
    /// The platforms the solution file lists (the first is its default).
    pub platforms: Vec<String>,
    pub menu: Option<&'static str>,
    /// Build the saved file's project after Ctrl+S (the setting `build.onSave`, or `ELUDITE_BUILD_ON_SAVE=1`; off by
    /// default).
    pub build_on_save: bool,
    /// F5 and Ctrl+F5 build the startup project first (the setting `build.beforeRun`; on by default).
    pub build_before_run: bool,
    /// Visual Studio's "Show Output window when build starts" (`build.showOutputOnStart`).
    pub show_output_on_start: bool,
    /// Visual Studio's "Always show Error List if build finishes with errors" (`build.showErrorListOnFailure`).
    pub show_error_list_on_failure: bool,
    pub timings: BuildTimings,
    /// The host restarted while its build ran: the build waits for `eludite/build/status`.
    pub awaiting_status: bool,
}

impl Builds {
    pub fn new(building: Arc<AtomicBool>, shared: Arc<BuildShared>) -> Self {
        Self {
            shared,
            building,
            current: None,
            next_ticket: 1,
            last: None,
            diagnostics: Vec::new(),
            cargo: None,
            cargo_program: "cargo".into(),
            next_cargo_id: cargo_build::CARGO_BUILD_ID_BASE,
            chain: None,
            configuration: CONFIGURATIONS[0].to_owned(),
            platform: None,
            platforms: vec!["Any CPU".into()],
            menu: None,
            build_on_save: false,
            build_before_run: true,
            show_output_on_start: true,
            show_error_list_on_failure: true,
            timings: BuildTimings::default(),
            awaiting_status: false,
        }
    }

    pub fn is_building(&self) -> bool {
        self.current.is_some()
    }
}

/// One step of a build plan: the system and the MSBuild project or Cargo package (`None`: all of it).
pub type BuildStep = (BuildSystem, Option<String>);

/// A build of every system of the workspace (brief 0019): one after the other, into one result.
#[derive(Debug, Clone, Default)]
pub struct Chain {
    /// The systems still to run, in order.
    pub next: Vec<BuildStep>,
    pub diagnostics: Vec<BuildDiagnostic>,
    pub summary: BuildSummary,
    pub projects: Vec<eludite_lsp::host::BuildProjectResult>,
    pub failed: bool,
    pub elapsed_ms: f64,
}

/// Whether menu item `command` is enabled given whether a build runs.
pub fn menu_enabled(command: &str, building: bool) -> bool {
    use eludite_commands::build::{CANCEL, STARTS};
    if STARTS.contains(&command) {
        !building
    } else if command == CANCEL {
        building
    } else {
        true
    }
}

/// The platforms a solution file lists (`.sln`: `SolutionConfigurationPlatforms`; `.slnx`: `<Platforms>`), in order,
/// without duplicates; `Any CPU` when it lists none or is a project file.
pub fn solution_platforms(path: &Path, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: &str| {
        let p = p.trim();
        if !p.is_empty() && !out.iter().any(|x| x == p) {
            out.push(p.to_owned());
        }
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext == "sln" {
        let mut inside = false;
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with("GlobalSection(SolutionConfigurationPlatforms)") {
                inside = true;
            } else if l.starts_with("EndGlobalSection") {
                inside = false;
            } else if inside
                && let Some((left, _)) = l.split_once('=')
                && let Some((_, platform)) = left.split_once('|')
            {
                add(platform);
            }
        }
    } else if ext == "slnx" {
        // `<Configurations><Platform Name="x64" /></Configurations>`; a project's own mapping uses `Solution=`.
        for part in text.split("<Platform ").skip(1) {
            let attrs = part.split_once('>').map_or(part, |(a, _)| a);
            if let Some((name, _)) = attrs
                .split_once("Name=\"")
                .and_then(|(_, r)| r.split_once('"'))
            {
                add(name);
            }
        }
    }
    if out.is_empty() {
        out.push("Any CPU".into());
    }
    out
}

fn kind_target(kind: BuildKind) -> BuildTarget {
    match kind {
        BuildKind::Build => BuildTarget::Build,
        BuildKind::Rebuild => BuildTarget::Rebuild,
        BuildKind::Clean => BuildTarget::Clean,
    }
}

fn verb(kind: BuildKind) -> &'static str {
    match kind {
        BuildKind::Build => "Build",
        BuildKind::Rebuild => "Rebuild All",
        BuildKind::Clean => "Clean",
    }
}

impl Shell {
    #[allow(dead_code)]
    pub fn builds(&self) -> &Builds {
        &self.builds
    }

    #[allow(dead_code)]
    pub fn output(&self) -> &gpui::Entity<super::output::OutputWindow> {
        &self.output
    }

    /// Apply a build or output command (the UI-thread half of [`BuildBus`]). Returns the outcome and, when a build
    /// started, its ticket.
    pub(super) fn apply_build(
        &mut self,
        request: BuildRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Outcome, Option<u64>) {
        match request {
            BuildRequest::Start {
                kind,
                project,
                configuration,
                platform,
                system,
                ..
            } => match self.start_build(kind, project, configuration, platform, system, window, cx)
            {
                Ok((out, ticket)) => (Ok(out), Some(ticket)),
                Err(e) => (Err(e), None),
            },
            BuildRequest::Cancel => {
                let out = match &self.builds.current {
                    Some(b) => {
                        // The rest of a build of every system is dropped too.
                        self.builds.chain = None;
                        match (&b.system, &self.builds.cargo) {
                            (BuildSystem::Cargo, Some(run)) => run.cancel(),
                            _ => self.session.build_cancel(),
                        }
                        self.status.set(BUILD_SLOT, "Canceling build\u{2026}");
                        cx.notify();
                        CancelOutput {
                            canceled: true,
                            build_id: b.id,
                        }
                    }
                    None => CancelOutput {
                        canceled: false,
                        build_id: None,
                    },
                };
                (Ok(BuildCommandOutput::Cancel(out)), None)
            }
            BuildRequest::OutputShow { source, tail } => {
                let _ = self.controller.apply(ViewRequest::Show {
                    id: ids::OUTPUT.into(),
                });
                let out = self.output.update(cx, |o, cx| {
                    if let Some(s) = source {
                        o.select(s, cx);
                    }
                    let selected = o.selected();
                    let pane = o.pane(selected);
                    OutputShowOutput {
                        source: selected,
                        line_count: pane.len() as u64,
                        following: o.following(),
                        tail: pane.tail(tail as usize),
                    }
                });
                (Ok(BuildCommandOutput::OutputShow(out)), None)
            }
            BuildRequest::OutputClear { source } => {
                let out = self.output.update(cx, |o, cx| {
                    let s = source.unwrap_or_else(|| o.selected());
                    OutputClearOutput {
                        source: s,
                        cleared: o.clear(s, cx) as u64,
                    }
                });
                (Ok(BuildCommandOutput::OutputClear(out)), None)
            }
        }
    }

    /// The project file a build command names: a project name or path, or (`None`) the active document's project.
    fn resolve_project(&self, project: Option<String>) -> Result<PathBuf, CommandError> {
        let tree = self.tree.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match project {
            Some(p) => {
                if let Some(found) = tree.projects.iter().find(|x| x.name == p) {
                    return Ok(PathBuf::from(&found.path));
                }
                let path = Path::new(&p);
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.solution_dir().unwrap_or_default().join(path)
                };
                let wanted = super::documents::normalize_path(&path);
                tree.projects
                    .iter()
                    .find(|x| super::documents::normalize_path(Path::new(&x.path)) == wanted)
                    .map(|x| PathBuf::from(&x.path))
                    .ok_or_else(|| {
                        CommandError::InvalidInput(format!(
                            "{p} is not a project of the open solution"
                        ))
                    })
            }
            None => {
                let active = self
                    .controller
                    .active_document()
                    .filter(|id| self.documents.contains_key(id))
                    .ok_or_else(|| {
                        CommandError::Failed(
                            "no project given and no document is active to take it from".into(),
                        )
                    })?;
                let active = super::documents::normalize_path(Path::new(&active));
                tree.projects
                    .iter()
                    .find(|x| {
                        x.files
                            .iter()
                            .any(|f| super::documents::normalize_path(Path::new(f)) == active)
                    })
                    .map(|x| PathBuf::from(&x.path))
                    .ok_or_else(|| {
                        CommandError::Failed(format!(
                            "{} is in no project of the open solution",
                            active.display()
                        ))
                    })
            }
        }
    }

    /// The Cargo package a build command names: a package name, or the path of its `Cargo.toml` or folder.
    fn cargo_package(&self, project: &str) -> Option<String> {
        let ws = self.cargo_workspace()?;
        if let Some(p) = ws.package(project) {
            return Some(p.name.clone());
        }
        let path = Path::new(project);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            ws.root.join(path)
        };
        let wanted = super::documents::normalize_path(&path);
        ws.members
            .iter()
            .find(|p| {
                super::documents::normalize_path(&p.manifest_path) == wanted
                    || super::documents::normalize_path(p.dir()) == wanted
            })
            .map(|p| p.name.clone())
    }

    /// The open folder's Cargo workspace, once read.
    pub fn cargo_workspace(&self) -> Option<&eludite_workspace::cargo::CargoWorkspace> {
        match self.folder.as_ref()?.cargo.as_ref()? {
            CargoState::Loaded(ws) => Some(ws),
            _ => None,
        }
    }

    /// The Cargo package of the active document, if it is in the Cargo workspace.
    fn active_cargo_package(&self) -> Option<String> {
        let ws = self.cargo_workspace()?;
        let active = self
            .controller
            .active_document()
            .filter(|id| self.documents.contains_key(id))?;
        ws.package_of(Path::new(&active)).map(|p| p.name.clone())
    }

    /// Whether the active document is a file of the open solution.
    fn active_in_solution(&self) -> bool {
        let Some(active) = self
            .controller
            .active_document()
            .filter(|id| self.documents.contains_key(id))
        else {
            return false;
        };
        let active = super::documents::normalize_path(Path::new(&active));
        self.tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .any(|p| {
                p.files
                    .iter()
                    .any(|f| super::documents::normalize_path(Path::new(f)) == active)
            })
    }

    /// Which systems a build command runs, in order, with the Cargo package or MSBuild project for each.
    fn plan_build(
        &self,
        project: Option<Option<String>>,
        system: Option<BuildSystemChoice>,
    ) -> Result<(BuildSystemChoice, Vec<BuildStep>), CommandError> {
        let has_msbuild = self.solution.is_some();
        let has_cargo = self.cargo_workspace().is_some();
        match project {
            // One project: a Cargo package by name or path, else an MSBuild project.
            Some(Some(p)) => {
                if let Some(pkg) = self.cargo_package(&p) {
                    return Ok((
                        BuildSystemChoice::Cargo,
                        vec![(BuildSystem::Cargo, Some(pkg))],
                    ));
                }
                let path = self.resolve_project(Some(p))?;
                Ok((
                    BuildSystemChoice::Msbuild,
                    vec![(
                        BuildSystem::Msbuild,
                        Some(path.to_string_lossy().into_owned()),
                    )],
                ))
            }
            Some(None) => {
                if let Some(pkg) = self.active_cargo_package() {
                    return Ok((
                        BuildSystemChoice::Cargo,
                        vec![(BuildSystem::Cargo, Some(pkg))],
                    ));
                }
                let path = self.resolve_project(None)?;
                Ok((
                    BuildSystemChoice::Msbuild,
                    vec![(
                        BuildSystem::Msbuild,
                        Some(path.to_string_lossy().into_owned()),
                    )],
                ))
            }
            None => {
                let choice = system.unwrap_or_else(|| {
                    if has_cargo && self.active_cargo_package().is_some() {
                        BuildSystemChoice::Cargo
                    } else if has_msbuild && self.active_in_solution() {
                        BuildSystemChoice::Msbuild
                    } else {
                        BuildSystemChoice::All
                    }
                });
                let plan: Vec<BuildStep> = match choice {
                    BuildSystemChoice::Msbuild if has_msbuild => vec![(BuildSystem::Msbuild, None)],
                    BuildSystemChoice::Cargo if has_cargo => vec![(BuildSystem::Cargo, None)],
                    BuildSystemChoice::Msbuild => {
                        return Err(CommandError::Failed("no solution is open".into()));
                    }
                    BuildSystemChoice::Cargo => {
                        return Err(CommandError::Failed(
                            "no Cargo workspace is open (File > Open > Folder)".into(),
                        ));
                    }
                    BuildSystemChoice::All => {
                        let mut v = Vec::new();
                        if has_msbuild {
                            v.push((BuildSystem::Msbuild, None));
                        }
                        if has_cargo {
                            v.push((BuildSystem::Cargo, None));
                        }
                        v
                    }
                };
                if plan.is_empty() {
                    return Err(CommandError::Failed("no solution is open".into()));
                }
                // A single system of an "all" plan is that system.
                let choice = match (choice, plan.as_slice()) {
                    (BuildSystemChoice::All, [(BuildSystem::Msbuild, _)]) => {
                        BuildSystemChoice::Msbuild
                    }
                    (BuildSystemChoice::All, [(BuildSystem::Cargo, _)]) => BuildSystemChoice::Cargo,
                    (c, _) => c,
                };
                Ok((choice, plan))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_build(
        &mut self,
        kind: BuildKind,
        project: Option<Option<String>>,
        configuration: Option<String>,
        platform: Option<String>,
        system: Option<BuildSystemChoice>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(BuildCommandOutput, u64), CommandError> {
        if let Some(b) = &self.builds.current {
            return Err(CommandError::Failed(format!(
                "a build is already running{}",
                b.id.map(|id| format!(" (build {id})")).unwrap_or_default()
            )));
        }
        let (choice, mut plan) = self.plan_build(project, system)?;
        // An explicit configuration or platform becomes the toolbar's selection, as choosing it there would.
        if let Some(c) = &configuration {
            self.builds.configuration = c.clone();
        }
        if platform.is_some() {
            self.builds.platform = platform.clone();
        }
        let configuration = self.builds.configuration.clone();
        let platform = self.builds.platform.clone();
        let ticket = self.builds.next_ticket;
        self.builds.next_ticket += 1;
        let requested = Instant::now();
        self.builds.timings = BuildTimings {
            requested: Some(requested),
            ..BuildTimings::default()
        };
        self.set_building(true, cx);
        // Visual Studio's "Show Output window when build starts".
        let show = self.builds.show_output_on_start;
        if show {
            let _ = self.controller.apply(ViewRequest::Show {
                id: ids::OUTPUT.into(),
            });
        }
        self.output.update(cx, |o, cx| {
            o.clear(OutputSource::Build, cx);
            if show {
                o.select(OutputSource::Build, cx);
            }
        });
        self.status
            .set(BUILD_SLOT, format!("{} started\u{2026}", verb(kind)));
        let first = plan.remove(0);
        self.builds.chain = (!plan.is_empty()).then(|| Chain {
            next: plan,
            ..Chain::default()
        });
        let path = self.begin_system(ticket, kind, first, choice, window, cx);
        cx.notify();
        Ok((
            BuildCommandOutput::Result(Box::new(BuildResultOutput {
                state: "running".into(),
                system: Some(choice),
                build_id: None,
                target: kind,
                path: path.to_string_lossy().into_owned(),
                configuration,
                platform,
                toolchain: None,
                elapsed_ms: None,
                errors: None,
                warnings: None,
                projects: Vec::new(),
                diagnostics: Vec::new(),
                message: None,
            })),
            ticket,
        ))
    }

    /// Start one system's build for `ticket`; returns the path built (folder, manifest, solution or project).
    fn begin_system(
        &mut self,
        ticket: u64,
        kind: BuildKind,
        (system, target): BuildStep,
        choice: BuildSystemChoice,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> PathBuf {
        let configuration = self.builds.configuration.clone();
        let platform = self.builds.platform.clone();
        let path = match system {
            BuildSystem::Msbuild => target
                .clone()
                .map(PathBuf::from)
                .or_else(|| self.solution.clone())
                .unwrap_or_default(),
            BuildSystem::Cargo => self
                .cargo_workspace()
                .map(|ws| ws.manifest.clone())
                .unwrap_or_default(),
        };
        let shown = if choice == BuildSystemChoice::All {
            self.workspace_root().unwrap_or_else(|| path.clone())
        } else {
            path.clone()
        };
        super::documents::trace(format_args!(
            "build requested: {} {:?} {}",
            kind.as_str(),
            system,
            path.display()
        ));
        let id = match system {
            BuildSystem::Cargo => {
                let id = self.builds.next_cargo_id;
                self.builds.next_cargo_id += 1;
                Some(id)
            }
            BuildSystem::Msbuild => None,
        };
        self.builds.current = Some(CurrentBuild {
            ticket,
            system,
            choice,
            id,
            kind,
            path: shown.clone(),
            configuration: configuration.clone(),
            platform: platform.clone(),
            started: None,
            progress: None,
            next_seq: 0,
        });
        match system {
            BuildSystem::Msbuild => self.session.build_start(
                ticket,
                BuildStartParams {
                    target: kind_target(kind),
                    system: None,
                    project: target,
                    configuration: Some(configuration.clone()),
                    platform: platform.clone(),
                },
            ),
            BuildSystem::Cargo => {
                let ws = self.cargo_workspace().cloned().unwrap_or_else(|| {
                    unreachable!("planned a Cargo build without a Cargo workspace")
                });
                let spec = CargoBuildSpec {
                    ticket,
                    id: id.unwrap_or_default(),
                    generation: self.generation,
                    kind,
                    manifest: ws.manifest.clone(),
                    root: ws.root.clone(),
                    package: target,
                    release: configuration.eq_ignore_ascii_case("release"),
                    members: ws
                        .members
                        .iter()
                        .map(|p| Member {
                            name: p.name.clone(),
                            manifest: p.manifest_path.clone(),
                        })
                        .collect(),
                    program: self.builds.cargo_program.clone(),
                };
                self.builds.cargo = Some(cargo_build::start(spec, self.build_events.clone()));
            }
        }
        let _ = window;
        shown
    }

    fn set_building(&mut self, building: bool, cx: &mut Context<Self>) {
        self.builds.building.store(building, Ordering::SeqCst);
        self.menu.update(cx, |_, cx| cx.notify());
    }

    pub(super) fn on_build_started(
        &mut self,
        ticket: u64,
        result: BuildStartResult,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.builds.current.as_mut().filter(|b| b.ticket == ticket) {
            b.id = Some(result.build_id);
            b.started = Some(result);
            cx.notify();
        }
    }

    pub(super) fn on_build_refused(
        &mut self,
        ticket: u64,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.builds.current.take_if(|b| b.ticket == ticket) else {
            return;
        };
        self.set_building(false, cx);
        self.output.update(cx, |o, cx| {
            o.append(OutputSource::Build, &format!("{message}\n"), cx)
        });
        self.status.set(
            BUILD_SLOT,
            format!("{} did not start: {message}", verb(b.kind)),
        );
        self.builds.chain = None;
        self.builds.shared.publish(
            ticket,
            BuildResultOutput {
                state: "failed".into(),
                system: Some(b.choice),
                build_id: None,
                target: b.kind,
                path: b.path.to_string_lossy().into_owned(),
                configuration: b.configuration,
                platform: b.platform,
                toolchain: None,
                elapsed_ms: None,
                errors: None,
                warnings: None,
                projects: Vec::new(),
                diagnostics: Vec::new(),
                message: Some(message),
            },
        );
        cx.notify();
    }

    /// Whether a host message about build `id` belongs to the build that runs (learning its id if not known yet:
    /// output can overtake the start reply).
    fn is_current_build(&mut self, id: u64) -> bool {
        match self.builds.current.as_mut() {
            Some(b) if b.id.is_none() => {
                b.id = Some(id);
                true
            }
            Some(b) => b.id == Some(id),
            None => false,
        }
    }

    pub(super) fn on_build_output(
        &mut self,
        id: u64,
        seq: u64,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.is_current_build(id)
            || self
                .builds
                .current
                .as_ref()
                .is_some_and(|b| seq < b.next_seq)
        {
            return;
        }
        if self.builds.timings.first_output.is_none() {
            self.builds.timings.first_output = Some(Instant::now());
            super::documents::trace(format_args!(
                "build first output: {}",
                text.lines().next().unwrap_or_default()
            ));
        }
        self.builds.timings.lines += text.matches('\n').count();
        self.output
            .update(cx, |o, cx| o.append(OutputSource::Build, text, cx));
    }

    pub(super) fn on_build_progress(&mut self, progress: BuildProgress, cx: &mut Context<Self>) {
        if !self.is_current_build(progress.build_id) {
            return;
        }
        let kind = self
            .builds
            .current
            .as_ref()
            .map_or(BuildKind::Build, |b| b.kind);
        let mut text = format!(
            "{}: {} of {} projects",
            match kind {
                BuildKind::Clean => "Cleaning",
                _ => "Building",
            },
            progress.projects_completed,
            progress.projects_total
        );
        if progress.errors > 0 {
            text.push_str(&format!(
                ", {} error{}",
                progress.errors,
                if progress.errors == 1 { "" } else { "s" }
            ));
        }
        self.status.set(BUILD_SLOT, text);
        if let Some(b) = self.builds.current.as_mut() {
            b.progress = Some(progress);
        }
        cx.notify();
    }

    pub(super) fn on_build_finished(
        &mut self,
        mut finished: BuildFinished,
        received: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_current_build(finished.build_id) {
            return;
        }
        let Some(b) = self.builds.current.take() else {
            return;
        };
        if b.system == BuildSystem::Cargo {
            self.builds.cargo = None;
        }
        self.builds.timings.finished_received = Some(received);
        // A build of every system (brief 0019): collect this one's result, then run the next one, or report the
        // sum as one build.
        if let Some(chain) = self.builds.chain.as_mut() {
            if finished.generation >= self.generation {
                chain
                    .diagnostics
                    .extend(finished.diagnostics.iter().cloned());
            }
            chain.summary.errors += finished.summary.errors;
            chain.summary.warnings += finished.summary.warnings;
            chain.summary.projects_succeeded += finished.summary.projects_succeeded;
            chain.summary.projects_failed += finished.summary.projects_failed;
            chain.projects.extend(finished.projects.iter().cloned());
            chain.failed |= finished.result == BuildResult::Failed;
            chain.elapsed_ms += finished.elapsed_ms;
            if finished.result != BuildResult::Canceled && !chain.next.is_empty() {
                let next = chain.next.remove(0);
                self.builds.diagnostics = chain.diagnostics.clone();
                self.update_error_list(cx);
                self.begin_system(b.ticket, b.kind, next, b.choice, window, cx);
                cx.notify();
                return;
            }
            let chain = self.builds.chain.take().unwrap_or_default();
            finished.result = match finished.result {
                BuildResult::Canceled => BuildResult::Canceled,
                _ if chain.failed => BuildResult::Failed,
                _ => BuildResult::Succeeded,
            };
            finished.summary = chain.summary;
            finished.projects = chain.projects;
            finished.elapsed_ms = chain.elapsed_ms;
            finished.diagnostics = chain.diagnostics;
            finished.path = b.path.to_string_lossy().into_owned();
        }
        self.set_building(false, cx);
        let s = finished.summary;
        let plural = |n: u32, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        let text = match finished.result {
            BuildResult::Succeeded => format!("{} succeeded", verb(b.kind)),
            BuildResult::Failed => format!(
                "{} failed: {}, {}",
                verb(b.kind),
                plural(s.errors, "error"),
                plural(s.warnings, "warning")
            ),
            BuildResult::Canceled => format!("{} canceled", verb(b.kind)),
        };
        self.status.set(BUILD_SLOT, text);
        // A result computed for an older solution is not shown (CLAUDE.md invariant 12); the build is over all the
        // same.
        let current_generation = finished.generation >= self.generation;
        if current_generation {
            self.builds.diagnostics = finished.diagnostics.clone();
            self.update_error_list(cx);
            self.builds.timings.rows_set = Some(Instant::now());
            super::documents::trace(format_args!(
                "build finished: {:?}, {} errors, {} warnings, Error List rows set in {:.2} ms",
                finished.result,
                s.errors,
                s.warnings,
                received.elapsed().as_secs_f64() * 1e3
            ));
            // Visual Studio's "Always show Error List if build finishes with errors".
            if self.builds.show_error_list_on_failure
                && finished.result == BuildResult::Failed
                && s.errors > 0
            {
                let _ = self.controller.apply(ViewRequest::Show {
                    id: ids::ERROR_LIST.into(),
                });
            }
        }
        let out = self.result_output(&b, &finished);
        self.builds.last = Some(finished);
        self.builds.shared.publish(b.ticket, out);
        let _ = window;
        cx.notify();
    }

    /// The host exited while its build ran: keep the build until the restarted host says whether it still runs.
    pub(super) fn on_host_restarting(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self
            .builds
            .current
            .as_ref()
            .filter(|b| b.system == BuildSystem::Msbuild)
        {
            self.builds.awaiting_status = true;
            self.status.set(
                BUILD_SLOT,
                format!(
                    "{}: eludite-host restarted; recovering the build\u{2026}",
                    verb(b.kind)
                ),
            );
            cx.notify();
        }
    }

    /// `eludite/build/status` from a restarted host (brief 0020): replay the build that still runs, or end the one
    /// that did not survive (with the host's result when it finished meanwhile).
    pub(super) fn on_build_status(
        &mut self,
        status: eludite_lsp::host::BuildStatusResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let awaiting = std::mem::take(&mut self.builds.awaiting_status);
        let Some(r) = status.running else {
            if !awaiting {
                return;
            }
            let current = self.builds.current.as_ref().and_then(|b| b.id);
            match status.last.filter(|l| Some(l.build_id) == current) {
                Some(last) => {
                    super::documents::trace(format_args!(
                        "build status: build {} finished while the host restarted",
                        last.build_id
                    ));
                    let finished = BuildFinished {
                        build_id: last.build_id,
                        generation: last.generation,
                        target: last.target,
                        path: last.path,
                        result: last.result,
                        exit_code: None,
                        elapsed_ms: last.elapsed_ms,
                        summary: last.summary,
                        projects: Vec::new(),
                        diagnostics: Vec::new(),
                        diagnostics_truncated: false,
                        binlog: None,
                        message: Some("finished while eludite-host restarted".into()),
                    };
                    self.on_build_finished(finished, Instant::now(), window, cx);
                }
                None => self.on_build_lost("eludite-host restarted and the build ended", cx),
            }
            return;
        };
        let adopt = match &self.builds.current {
            None => true,
            Some(b) if b.system == BuildSystem::Cargo => false,
            Some(b) => awaiting || b.id.is_none_or(|id| id == r.build_id),
        };
        if !adopt {
            return;
        }
        let kind = match r.target {
            BuildTarget::Build => BuildKind::Build,
            BuildTarget::Rebuild => BuildKind::Rebuild,
            BuildTarget::Clean => BuildKind::Clean,
        };
        let (ticket, choice) = match self.builds.current.take() {
            Some(b) => (b.ticket, b.choice),
            None => {
                let t = self.builds.next_ticket;
                self.builds.next_ticket += 1;
                (t, BuildSystemChoice::Msbuild)
            }
        };
        super::documents::trace(format_args!(
            "build status: replaying build {} from seq {} ({} bytes)",
            r.build_id,
            r.output.first_seq,
            r.output.text.len()
        ));
        self.builds.current = Some(CurrentBuild {
            ticket,
            system: BuildSystem::Msbuild,
            choice,
            id: Some(r.build_id),
            kind,
            path: PathBuf::from(&r.path),
            configuration: r.configuration.clone(),
            platform: r.platform.clone(),
            started: Some(r.start_result()),
            progress: None,
            next_seq: r.output.next_seq,
        });
        self.set_building(true, cx);
        self.output.update(cx, |o, cx| {
            o.clear(OutputSource::Build, cx);
            if r.output.truncated {
                o.append(
                    OutputSource::Build,
                    "(earlier output of this build was dropped by eludite-host)\n",
                    cx,
                );
            }
            o.append(OutputSource::Build, &r.output.text, cx);
        });
        match r.progress {
            Some(p) => self.on_build_progress(p, cx),
            None => {
                self.status
                    .set(BUILD_SLOT, format!("{} running\u{2026}", verb(kind)));
            }
        }
        cx.notify();
    }

    /// The host went away mid-build: the build is over.
    pub(super) fn on_build_lost(&mut self, why: &str, cx: &mut Context<Self>) {
        // Only the host's builds end with the host (a Cargo build runs in the shell).
        self.builds.awaiting_status = false;
        let Some(b) = self
            .builds
            .current
            .take_if(|b| b.system == BuildSystem::Msbuild)
        else {
            return;
        };
        self.set_building(false, cx);
        self.status
            .set(BUILD_SLOT, format!("{} canceled: {why}", verb(b.kind)));
        self.builds.chain = None;
        self.builds.shared.publish(
            b.ticket,
            BuildResultOutput {
                state: "canceled".into(),
                system: Some(b.choice),
                build_id: b.id,
                target: b.kind,
                path: b.path.to_string_lossy().into_owned(),
                configuration: b.configuration,
                platform: b.platform,
                toolchain: None,
                elapsed_ms: None,
                errors: None,
                warnings: None,
                projects: Vec::new(),
                diagnostics: Vec::new(),
                message: Some(why.to_owned()),
            },
        );
    }

    fn result_output(&self, b: &CurrentBuild, f: &BuildFinished) -> BuildResultOutput {
        let root = self.solution_dir();
        let rel = |p: &str| super::relative_path(Path::new(p), root.as_deref());
        let name = |p: &str| self.project_display(p);
        let result = |r: BuildResult| match r {
            BuildResult::Succeeded => "succeeded",
            BuildResult::Failed => "failed",
            BuildResult::Canceled => "canceled",
        };
        BuildResultOutput {
            state: result(f.result).into(),
            system: Some(b.choice),
            build_id: Some(f.build_id),
            target: b.kind,
            path: f.path.clone(),
            configuration: b.configuration.clone(),
            platform: b.platform.clone(),
            toolchain: b.started.as_ref().map(|s| {
                match s.toolchain.kind {
                    eludite_lsp::host::ToolchainKind::Dotnet => "dotnet",
                    eludite_lsp::host::ToolchainKind::Mono => "mono",
                    eludite_lsp::host::ToolchainKind::BuildTools => "buildTools",
                    eludite_lsp::host::ToolchainKind::Cargo => "cargo",
                }
                .to_owned()
            }),
            elapsed_ms: Some(f.elapsed_ms),
            errors: Some(f.summary.errors),
            warnings: Some(f.summary.warnings),
            projects: f
                .projects
                .iter()
                .map(|p| ResultProject {
                    name: p.name.clone(),
                    result: result(p.result).into(),
                    errors: p.errors,
                    warnings: p.warnings,
                    elapsed_ms: p.elapsed_ms,
                })
                .collect(),
            diagnostics: f
                .diagnostics
                .iter()
                .take(MAX_RESULT_DIAGNOSTICS)
                .map(|d| ResultDiagnostic {
                    severity: match d.severity {
                        BuildDiagnosticSeverity::Error => "error",
                        BuildDiagnosticSeverity::Warning => "warning",
                        BuildDiagnosticSeverity::Message => "message",
                    }
                    .into(),
                    code: d.code.clone(),
                    message: d.message.clone(),
                    path: d.file.as_deref().map(rel),
                    line: d.line.filter(|l| *l > 0),
                    column: d.column.filter(|c| *c > 0),
                    project: d.project.as_deref().map(name),
                })
                .collect(),
            message: f.message.clone(),
        }
    }

    /// `--bench-output`: a stand-in build whose output the harness streams through [`Shell::bench_stream_chunk`].
    pub fn bench_stream_begin(&mut self, cx: &mut Context<Self>) {
        self.builds.current = Some(CurrentBuild {
            ticket: 0,
            system: BuildSystem::Msbuild,
            choice: BuildSystemChoice::Msbuild,
            id: Some(BENCH_BUILD_ID),
            kind: BuildKind::Build,
            path: PathBuf::from("bench"),
            configuration: "Debug".into(),
            platform: None,
            started: None,
            progress: None,
            next_seq: 0,
        });
        let _ = self.controller.apply(ViewRequest::Show {
            id: ids::OUTPUT.into(),
        });
        self.output.update(cx, |o, cx| {
            o.clear(OutputSource::Build, cx);
            o.select(OutputSource::Build, cx);
        });
    }

    /// One chunk of the stand-in build's output, through the same handler as the host's.
    pub fn bench_stream_chunk(&mut self, text: &str, cx: &mut Context<Self>) {
        self.on_build_output(BENCH_BUILD_ID, u64::MAX, text, cx);
    }

    /// Lines in the Output window's Build source.
    pub fn output_lines(&self, cx: &gpui::App) -> usize {
        self.output.read(cx).pane(OutputSource::Build).len()
    }

    /// Build on save (off by default): build the saved file's project.
    pub(super) fn build_after_save(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.builds.build_on_save || self.builds.is_building() || self.solution.is_none() {
            return;
        }
        let saved = super::documents::normalize_path(Path::new(path));
        let project = self
            .tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .find(|p| {
                p.files
                    .iter()
                    .any(|f| super::documents::normalize_path(Path::new(f)) == saved)
            })
            .map(|p| p.name.clone());
        let (command, args) = match project {
            Some(p) => (
                eludite_commands::build::PROJECT,
                serde_json::json!({ "project": p }),
            ),
            None => (eludite_commands::build::SOLUTION, serde_json::json!({})),
        };
        self.run(command, args, window, cx);
    }

    /// The solution's platforms, read off the UI thread when a solution opens.
    pub(super) fn load_platforms(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let read = cx.background_spawn({
            let path = path.clone();
            async move {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                solution_platforms(&path, &text)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let platforms = read.await;
            let _ = this.update(cx, |shell, cx| {
                if shell.solution.as_deref() == Some(path.as_path()) {
                    shell.builds.platforms = platforms;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Visual Studio's Solution Configurations and Solution Platforms dropdowns (its Standard toolbar's), drawn at the
    /// right of the menu bar's row.
    pub(super) fn build_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t: Theme = self.theme;
        let platform = self
            .builds
            .platform
            .clone()
            .or_else(|| self.builds.platforms.first().cloned())
            .unwrap_or_else(|| "Any CPU".into());
        let dropdown =
            |id: &'static str, label: String, kind: &'static str, cx: &mut Context<Self>| {
                toggle_button(id, format!("{label} \u{25BE}"), false, &t)
                    .min_w(px(96.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.builds.menu = if this.builds.menu == Some(kind) {
                            None
                        } else {
                            Some(kind)
                        };
                        cx.notify();
                    }))
            };
        let menu = self.builds.menu.map(|kind| {
            let entries: Vec<String> = if kind == "configuration" {
                CONFIGURATIONS.iter().map(|s| (*s).to_owned()).collect()
            } else {
                self.builds.platforms.clone()
            };
            let items = entries.into_iter().enumerate().map(|(ix, value)| {
                let sel = toolbar_item_selector(kind, ix);
                div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .px_2()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.menu_hover))
                    .child(value.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if kind == "configuration" {
                            this.builds.configuration = value.clone();
                        } else {
                            let default = this.builds.platforms.first() == Some(&value);
                            this.builds.platform = (!default).then(|| value.clone());
                        }
                        this.builds.menu = None;
                        cx.notify();
                    }))
            });
            deferred(
                anchored().child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("build-toolbar-menu")
                        .occlude()
                        .min_w(px(120.))
                        .py_1()
                        .mt(px(22.))
                        .ml(px(if kind == "configuration" { 0. } else { 100. }))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.builds.menu = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1)
        });
        div()
            .id("build-toolbar")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .h(t.typography.menu_bar_height)
            .px_2()
            .bg(t.menu_background)
            .text_size(t.typography.ui)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(dropdown(
                        CONFIGURATION_BUTTON,
                        self.builds.configuration.clone(),
                        "configuration",
                        cx,
                    ))
                    .child(dropdown(PLATFORM_BUTTON, platform, "platform", cx))
                    .children(menu),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_items_follow_the_build_state() {
        use eludite_commands::build::{CANCEL, CLEAN, PROJECT, REBUILD, SOLUTION};
        for c in [SOLUTION, PROJECT, REBUILD, CLEAN] {
            assert!(menu_enabled(c, false));
            assert!(!menu_enabled(c, true));
        }
        assert!(!menu_enabled(CANCEL, false));
        assert!(menu_enabled(CANCEL, true));
        assert!(menu_enabled("eludite.editor.save", true));
    }

    #[test]
    fn platforms_come_from_the_solution_file() {
        let sln = "Global\n\tGlobalSection(SolutionConfigurationPlatforms) = preSolution\n\t\tDebug|Any CPU = Debug|Any CPU\n\t\tDebug|x64 = Debug|x64\n\t\tRelease|Any CPU = Release|Any CPU\n\t\tRelease|x64 = Release|x64\n\tEndGlobalSection\n\tGlobalSection(ProjectConfigurationPlatforms) = postSolution\n\t\t{A}.Debug|ARM.ActiveCfg = Debug|ARM\n\tEndGlobalSection\nEndGlobal\n";
        assert_eq!(
            solution_platforms(Path::new("/s/A.sln"), sln),
            ["Any CPU", "x64"]
        );
        let slnx = "<Solution>\n  <Configurations>\n    <BuildType Name=\"Debug\" />\n    <Platform Name=\"Any CPU\" />\n    <Platform Name=\"x86\" />\n  </Configurations>\n  <Project Path=\"a.csproj\">\n    <Platform Solution=\"*|x86\" Project=\"x86\" />\n  </Project>\n</Solution>";
        assert_eq!(
            solution_platforms(Path::new("/s/A.slnx"), slnx),
            ["Any CPU", "x86"]
        );
        assert_eq!(
            solution_platforms(Path::new("/s/A.slnx"), "<Solution />"),
            ["Any CPU"]
        );
        assert_eq!(
            solution_platforms(Path::new("/s/A.csproj"), "<Project />"),
            ["Any CPU"]
        );
    }

    #[test]
    fn a_waiting_agent_gets_its_own_build_result() {
        let shared = Arc::new(BuildShared::default());
        let s2 = shared.clone();
        let waiter = std::thread::spawn(move || s2.wait(2, Duration::from_secs(5)));
        let result = |state: &str| BuildResultOutput {
            state: state.into(),
            system: None,
            build_id: Some(1),
            target: BuildKind::Build,
            path: "/s/A.slnx".into(),
            configuration: "Debug".into(),
            platform: None,
            toolchain: None,
            elapsed_ms: None,
            errors: None,
            warnings: None,
            projects: Vec::new(),
            diagnostics: Vec::new(),
            message: None,
        };
        shared.publish(1, result("failed"));
        std::thread::sleep(Duration::from_millis(20));
        shared.publish(2, result("canceled"));
        assert_eq!(waiter.join().unwrap().unwrap().state, "canceled");
        assert!(shared.wait(3, Duration::from_millis(10)).is_none());
    }
}
