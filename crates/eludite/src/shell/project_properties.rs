//! Project property pages, launch profiles and the configuration and platform selection (brief 0049).
//!
//! - **Commands.** `eludite.project.properties`, `set_property`, `launch_profiles`, `set_launch_profile` and
//!   `eludite.solution.configurations`, `select_configuration`, `set_configuration` run through [`PropertiesBus`]: on
//!   the UI thread the shell applies the request at once and stages its answer (the UI never waits on the host: what
//!   needs the host answers from what the shell has, `pending`, and the pages show the host's answer when it comes);
//!   from another thread (an agent) the request is posted to the UI and the caller waits for the host's answer.
//! - **The pages** are a document tab per project (`project_properties:<project file>`, [`pages::PropertyPages`]),
//!   opened by Project > Properties, Alt+Enter or a double-click on a project in Workspace, and by
//!   `eludite.view.show` with `project_properties` (the selected project). Their values come from
//!   `eludite/project/properties` in the configuration and platform shown, cached per generation; Save
//!   (`eludite.editor.save` on the tab, Ctrl+S) sends every dirty value in one `eludite/project/setProperty`; the host
//!   reloads the solution, and the new generation's values replace the old.
//! - **The selection** (the Standard toolbar's Solution Configurations and Solution Platforms lists, the Target
//!   Framework list of a multi-targeted startup project, the Debug toolbar's launch profile) is kept per solution beside
//!   the breakpoints (`<config dir>/eludite/breakpoints/solutions/<solution>-<hash>.configuration.json`), told to the host
//!   (`eludite/solution/setConfiguration` `select`), and used by Build (the solution configuration and platform, a
//!   project build's mapped configuration, Visual Studio's `Skipped Build` line for a project the mapping does not
//!   build), Debug (`launch_config_in` with the project's configuration, framework and profile) and Test (the
//!   configuration, and the framework's containers of a multi-targeted project).

pub mod pages;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use eludite_commands::project::properties::{
    self as props, ConfigurationsOutput, EnvRow, LaunchProfilesOutput, MappingRow, PageRow,
    ProfileAction, ProfileRow, ProjectConfigurationsRow, PropertiesOutput, PropertiesOutputs,
    PropertiesRequest, PropertiesTarget, PropertyInputValue, PropertyRow, SelectOutput,
    SelectionRow, SetConfigurationOutput, SetPropertyOutput,
};
use eludite_commands::view::ViewTarget as _;
use eludite_commands::workspace::{CloseSave, SaveOutput, WorkspaceOutput};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_lsp::host::{self, EditStatus, ProjectPropertiesResult};
use eludite_protocol::RequestType;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{AnyView, AppContext as _, Context, Entity, PromptLevel, Window};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::Shell;
use super::session::RequestError;

/// The document tab of a project's property pages: `project_properties:<absolute project file>`.
pub const TAB_PREFIX: &str = "project_properties:";

/// `eludite.view.show`'s id for the selected project's pages (a document window that becomes the project's tab).
pub use eludite_docking::ids::PROJECT_PROPERTIES;

pub fn tab_id(project: &str) -> String {
    format!("{TAB_PREFIX}{project}")
}

/// The project file of a property pages tab id.
pub fn project_of_tab(id: &str) -> Option<&str> {
    id.strip_prefix(TAB_PREFIX)
}

pub type Documents = Rc<RefCell<HashMap<String, AnyView>>>;

type Outcome = Result<PropertiesOutputs, CommandError>;

/// How long an agent's call waits for the UI and the host.
const AGENT_TIMEOUT: Duration = Duration::from_secs(120);

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The answer the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// A request from another thread, for the UI thread; the reply is the host's answer.
pub struct PropertiesJob {
    pub request: PropertiesRequest,
    pub reply: mpsc::SyncSender<Outcome>,
}

/// The shell's [`PropertiesTarget`].
pub struct PropertiesBus {
    pub ui_thread: ThreadId,
    pub jobs: UnboundedSender<PropertiesJob>,
}

impl PropertiesTarget for PropertiesBus {
    fn apply(&self, request: PropertiesRequest) -> Outcome {
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(PropertiesJob { request, reply })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(AGENT_TIMEOUT)
            .map_err(|_| CommandError::Failed("the host did not answer in time".into()))?
    }
}

/// Register brief 0049's seven commands; returns what the shell drains on the UI thread.
pub fn register(commands: &CommandRegistry) -> UnboundedReceiver<PropertiesJob> {
    let (tx, rx) = unbounded();
    let bus: std::sync::Arc<dyn PropertiesTarget> = std::sync::Arc::new(PropertiesBus {
        ui_thread: std::thread::current().id(),
        jobs: tx,
    });
    props::register(commands, bus.clone());
    eludite_commands::solution::register_configurations(commands, bus);
    rx
}

/// The selection kept per solution and per user.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSelection {
    pub version: u32,
    pub configuration: Option<String>,
    pub platform: Option<String>,
    /// The Target Framework list's choice per project file.
    pub frameworks: BTreeMap<String, String>,
    /// The Debug toolbar's launch profile per project file.
    pub profiles: BTreeMap<String, String>,
}

/// When the steps of the pages happened (the budgets in the brief 0049 report).
#[derive(Debug, Default, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct PropertiesTimings {
    /// The open command was applied.
    pub opened: Option<Instant>,
    /// The pages had their values.
    pub shown: Option<Instant>,
    pub save_sent: Option<Instant>,
    pub save_answered: Option<Instant>,
}

/// The toolbar list that is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarList {
    Configuration,
    Platform,
    Framework,
    Profile,
}

#[derive(Default)]
pub struct PropertiesUi {
    pub documents: Documents,
    pub pages: HashMap<String, Entity<pages::PropertyPages>>,
    pub configurations: Option<host::SolutionConfigurationsResult>,
    pub selection: SavedSelection,
    /// Launch profiles per project file, for the toolbar's list and the Debug page.
    pub launch: HashMap<String, host::LaunchProfilesResult>,
    /// `eludite/project/properties` answers of the current generation, by project, configuration, platform, framework.
    pub cache: HashMap<String, ProjectPropertiesResult>,
    pub manager: Option<Entity<super::configuration_manager::ConfigurationManager>>,
    pub toolbar_menu: Option<ToolbarList>,
    /// `eludite.debug.start`'s `framework`, for the launch it starts.
    pub start_framework: Option<String>,
    /// The solution whose selection is loaded.
    pub solution: Option<PathBuf>,
    pub timings: PropertiesTimings,
}

fn cache_key(
    project: &str,
    configuration: &str,
    platform: &str,
    framework: Option<&str>,
) -> String {
    format!(
        "{project}|{configuration}|{}|{}",
        msbuild_platform(platform),
        framework.unwrap_or_default()
    )
}

/// MSBuild's spelling of a platform (`AnyCPU` for the solution's `Any CPU`).
pub fn msbuild_platform(platform: &str) -> String {
    if platform.replace(' ', "").eq_ignore_ascii_case("anycpu") {
        "AnyCPU".into()
    } else {
        platform.to_owned()
    }
}

fn describe(e: RequestError) -> String {
    match e {
        RequestError::NoHost => "no .NET solution is open (eludite-host is not running)".into(),
        RequestError::Canceled => "canceled".into(),
        RequestError::Stale => {
            "the solution changed while the host answered (its generation moved on); ask again"
                .into()
        }
        RequestError::Failed(m) => m,
    }
}

fn page_id(id: &str) -> String {
    match id {
        "codeAnalysis" => "code_analysis".into(),
        other => other.to_owned(),
    }
}

fn page_state(state: &str) -> String {
    match state {
        "launchProfiles" => "launch_profiles".into(),
        "notYet" => "not_yet".into(),
        other => other.to_owned(),
    }
}

fn source_name(s: host::PropertySource) -> &'static str {
    match s {
        host::PropertySource::Project => "project",
        host::PropertySource::Conditioned => "conditioned",
        host::PropertySource::Inherited => "inherited",
        host::PropertySource::Default => "default",
    }
}

fn type_name(t: host::PropertyType) -> &'static str {
    match t {
        host::PropertyType::String => "string",
        host::PropertyType::Bool => "bool",
        host::PropertyType::Enum => "enum",
        host::PropertyType::List => "list",
        host::PropertyType::Path => "path",
        host::PropertyType::Multiline => "multiline",
    }
}

fn status_name(s: EditStatus) -> &'static str {
    match s {
        EditStatus::Written => "written",
        EditStatus::Removed => "removed",
        EditStatus::Unchanged => "unchanged",
        EditStatus::Inherited => "inherited",
    }
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// `properties`' output from the host's answer, filtered to `page` when given.
pub fn properties_output(r: &ProjectPropertiesResult, page: Option<&str>) -> PropertiesOutput {
    PropertiesOutput {
        project: project_name(&r.project),
        path: r.project.clone(),
        kind: r.kind.clone(),
        configuration: r.configuration.clone(),
        platform: r.platform.clone(),
        framework: r.framework.clone(),
        configurations: r.configurations.clone(),
        platforms: r.platforms.clone(),
        frameworks: r.frameworks.clone(),
        pages: r
            .pages
            .iter()
            .map(|p| PageRow {
                id: page_id(&p.id),
                title: p.title.clone(),
                state: page_state(&p.state),
                note: p.note.clone(),
            })
            .collect(),
        properties: r
            .properties
            .iter()
            .filter(|p| page.is_none_or(|g| page_id(&p.page) == g))
            .map(|p| PropertyRow {
                name: p.name.clone(),
                page: page_id(&p.page),
                section: p.section.clone(),
                label: p.label.clone(),
                kind: type_name(p.kind).into(),
                values: p.values.iter().flatten().map(|v| v.value.clone()).collect(),
                per_configuration: p.per_configuration,
                value: p.value.clone(),
                raw: p.raw.clone(),
                source: source_name(p.source).into(),
                file: p.defined_in.as_ref().map(|d| d.file.clone()),
                line: p.defined_in.as_ref().map(|d| d.line),
                condition: p.defined_in.as_ref().and_then(|d| d.condition.clone()),
                inherited_from: p.inherited_from.clone(),
                conditions: p.conditions.clone().unwrap_or_default(),
                read_only: p.read_only,
            })
            .collect(),
        generation: r.generation,
        opened: false,
        pending: false,
    }
}

fn profile_row(p: &host::LaunchProfile) -> ProfileRow {
    ProfileRow {
        name: p.name.clone(),
        command_name: p.command_name.clone(),
        command_line_args: p.command_line_args.clone(),
        working_directory: p.working_directory.clone(),
        environment_variables: p
            .environment_variables
            .iter()
            .map(|e| EnvRow {
                name: e.name.clone(),
                value: e.value.clone(),
            })
            .collect(),
        launch_browser: p.launch_browser,
        launch_url: p.launch_url.clone(),
        application_url: p.application_url.clone(),
        dotnet_run_messages: p.dotnet_run_messages,
        hot_reload_enabled: p.hot_reload_enabled,
        executable_path: p.executable_path.clone(),
        read_only: p.read_only,
        unknown: p.unknown.clone().unwrap_or_default(),
    }
}

/// snake_case `values` of the command to the host's camelCase members.
fn camel_values(values: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    for (k, v) in values {
        let mut camel = String::new();
        let mut up = false;
        for c in k.chars() {
            if c == '_' {
                up = true;
            } else if up {
                camel.extend(c.to_uppercase());
                up = false;
            } else {
                camel.push(c);
            }
        }
        out.insert(camel, v.clone());
    }
    Value::Object(out)
}

impl Shell {
    /// Wire the jobs from other threads and `eludite.view.show`'s `project_properties` (call once from `new`).
    pub(super) fn properties_install(
        &mut self,
        mut jobs: UnboundedReceiver<PropertiesJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                let PropertiesJob { request, reply } = job;
                let sent = this.update_in(cx, |shell, window, cx| {
                    let _ = shell.apply_properties(request, Some(reply.clone()), window, cx);
                });
                if sent.is_err() {
                    let _ = reply.send(Err(CommandError::Failed("the window is closed".into())));
                }
            }
        });
        self._tasks.push(task);
        let mut changes = self.controller.subscribe();
        let controller = self.controller.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            while changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                if controller
                    .layout()
                    .documents
                    .get(PROJECT_PROPERTIES)
                    .is_none()
                {
                    continue;
                }
                let done = this.update_in(cx, |shell, window, cx| {
                    shell.controller.close_document(PROJECT_PROPERTIES);
                    shell.run(props::PROPERTIES, json!({ "open": true }), window, cx);
                });
                if done.is_err() {
                    break;
                }
            }
        });
        self._tasks.push(task);
    }

    /// Send `R` to the host; `then` gets the answer on the UI thread (the UI never waits).
    fn host_call<R>(
        &mut self,
        params: R::Params,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Shell, Result<R::Result, String>, &mut Window, &mut Context<Shell>)
        + 'static,
    ) where
        R: RequestType + 'static,
        R::Params: Send + 'static,
        R::Result: Send + 'static,
    {
        let (_, rx) = self.session.request::<R>(params);
        cx.spawn_in(window, async move |this, cx| {
            let result = match rx.await {
                Ok(reply) => reply.result.map_err(describe),
                Err(_) => Err("the host session ended".to_owned()),
            };
            let _ = this.update_in(cx, |shell, window, cx| then(shell, result, window, cx));
        })
        .detach();
    }

    /// The project a command names: a name or path, else the project selected in Workspace, else the active
    /// document's, else the startup project.
    pub(super) fn properties_project(
        &self,
        hint: Option<&str>,
        cx: &gpui::App,
    ) -> Result<String, CommandError> {
        let tree = self.tree.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if tree.projects.is_empty() {
            return Err(CommandError::Failed(
                "open a .NET solution first: the property pages are a project's".into(),
            ));
        }
        let norm = |p: &str| super::documents::normalize_path(Path::new(p));
        if let Some(h) = hint {
            if let Some(p) = tree
                .projects
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(h))
            {
                return Ok(p.path.clone());
            }
            let path = Path::new(h);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.solution_dir().unwrap_or_default().join(path)
            };
            let wanted = super::documents::normalize_path(&path);
            if path.file_name().is_some_and(|n| n == "Cargo.toml") {
                return Err(CommandError::InvalidInput(format!(
                    "{h} is a Cargo package: it has no project property pages (its settings are in Cargo.toml)"
                )));
            }
            // The tree's projects, or the solution file's (Configuration Manager lists every project it maps).
            let mapped = self
                .properties
                .configurations
                .iter()
                .flat_map(|c| c.projects.iter());
            return tree
                .projects
                .iter()
                .map(|p| (p.name.as_str(), p.path.as_str()))
                .chain(mapped.map(|p| (p.name.as_str(), p.path.as_str())))
                .find(|(name, path)| norm(path) == wanted || name.eq_ignore_ascii_case(h))
                .map(|(_, path)| path.to_owned())
                .ok_or_else(|| {
                    CommandError::InvalidInput(format!("{h} is not a project of the open solution"))
                });
        }
        let explorer = self.explorer.read(cx);
        if let Some(path) = explorer
            .selected_id()
            .and_then(|id| explorer.rows().iter().find(|r| r.id == id))
            .and_then(|r| r.path.clone())
            && let Some(p) = tree
                .projects
                .iter()
                .find(|p| norm(&p.path) == super::documents::normalize_path(&path))
        {
            return Ok(p.path.clone());
        }
        if let Some(active) = self.controller.active_document() {
            let active = project_of_tab(&active).map(str::to_owned).unwrap_or(active);
            let a = norm(&active);
            if let Some(p) = tree
                .projects
                .iter()
                .find(|p| norm(&p.path) == a || p.files.iter().any(|f| norm(f) == a))
            {
                return Ok(p.path.clone());
            }
        }
        if let Some(s) = &self.debug.model.startup_project
            && let Some(p) = tree.projects.iter().find(|p| norm(&p.path) == norm(s))
        {
            return Ok(p.path.clone());
        }
        Ok(tree.projects[0].path.clone())
    }

    /// The active solution configuration and platform.
    pub fn active_selection(&self) -> (String, String) {
        let platform = self.builds.platform.clone().unwrap_or_else(|| {
            self.solution_platforms()
                .first()
                .cloned()
                .unwrap_or_else(|| "Any CPU".into())
        });
        (self.builds.configuration.clone(), platform)
    }

    /// The Solution Configurations list: the solution's own (Debug and Release without a solution, or for Cargo).
    pub fn solution_configurations(&self) -> Vec<String> {
        match &self.properties.configurations {
            Some(c) if !c.configurations.is_empty() => c.configurations.clone(),
            _ => vec!["Debug".into(), "Release".into()],
        }
    }

    /// The Solution Platforms list.
    pub fn solution_platforms(&self) -> Vec<String> {
        match &self.properties.configurations {
            Some(c) if !c.platforms.is_empty() => c.platforms.clone(),
            _ => self.builds.platforms.clone(),
        }
    }

    /// What the active selection builds for `project`: its configuration, platform (MSBuild's spelling) and Build.
    pub fn mapped(&self, project: &str) -> Option<(String, String, bool)> {
        let c = self.properties.configurations.as_ref()?;
        let (sc, sp) = self.active_selection();
        let norm = |p: &str| super::documents::normalize_path(Path::new(p));
        let p = c.projects.iter().find(|p| norm(&p.path) == norm(project))?;
        let Some(m) = p.mappings.iter().find(|m| {
            m.solution_configuration.eq_ignore_ascii_case(&sc)
                && m.solution_platform.eq_ignore_ascii_case(&sp)
        }) else {
            return Some((sc, "AnyCPU".into(), false));
        };
        Some((
            m.configuration.clone(),
            msbuild_platform(&m.platform),
            m.build,
        ))
    }

    /// The projects the active selection does not build: their names and mapped configuration and platform.
    pub fn skipped_projects(&self) -> Vec<(String, String)> {
        let Some(c) = &self.properties.configurations else {
            return Vec::new();
        };
        let (sc, sp) = self.active_selection();
        c.projects
            .iter()
            .filter_map(|p| {
                let m = p.mappings.iter().find(|m| {
                    m.solution_configuration.eq_ignore_ascii_case(&sc)
                        && m.solution_platform.eq_ignore_ascii_case(&sp)
                });
                match m {
                    Some(m) if m.build => None,
                    Some(m) => Some((
                        p.name.clone(),
                        format!("{} {}", m.configuration, m.platform),
                    )),
                    None => Some((p.name.clone(), format!("{sc} {sp}"))),
                }
            })
            .collect()
    }

    /// The Target Framework list's choice for `project` (a multi-targeted project).
    pub fn selected_framework(&self, project: &str) -> Option<String> {
        self.properties.selection.frameworks.get(project).cloned()
    }

    /// The target frameworks of `project` as the tree lists them.
    pub fn project_frameworks(&self, project: &str) -> Vec<String> {
        let norm = |p: &str| super::documents::normalize_path(Path::new(p));
        self.tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .find(|p| norm(&p.path) == norm(project))
            .map(|p| p.target_frameworks.clone())
            .unwrap_or_default()
    }

    /// The Debug toolbar's launch profile for `project`.
    pub fn selected_profile(&self, project: &str) -> Option<String> {
        self.properties.selection.profiles.get(project).cloned()
    }

    /// The project F5 starts (Set as Startup Project's, else the first executable the tree lists, else the first).
    pub fn toolbar_project(&self) -> Option<String> {
        if let Some(s) = &self.debug.model.startup_project
            && !s.ends_with("Cargo.toml")
        {
            return Some(s.clone());
        }
        self.tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .first()
            .map(|p| p.path.clone())
    }

    /// The selection file of `solution`.
    fn selection_file(&self, solution: &Path) -> Option<PathBuf> {
        let dir = self.debug.store_dir()?;
        let base = eludite_docking::LayoutStore::new(dir).solution_path(solution);
        Some(base.with_extension("configuration.json"))
    }

    /// Save the selection, off the UI thread.
    fn persist_selection(&mut self, cx: &mut Context<Self>) {
        let Some(solution) = self.solution.clone() else {
            return;
        };
        let Some(file) = self.selection_file(&solution) else {
            return;
        };
        let (configuration, platform) = self.active_selection();
        let saved = SavedSelection {
            version: 1,
            configuration: Some(configuration),
            platform: Some(platform),
            frameworks: self.properties.selection.frameworks.clone(),
            profiles: self.properties.selection.profiles.clone(),
        };
        self.properties.selection = saved.clone();
        let Ok(text) = serde_json::to_string_pretty(&saved) else {
            return;
        };
        cx.background_spawn(async move {
            if let Err(e) = eludite_docking::persist::write_atomic(&file, &text) {
                eprintln!(
                    "eludite: cannot save the configuration selection to {}: {e}",
                    file.display()
                );
            }
        })
        .detach();
    }

    /// A solution opened: its own selection (read off the UI thread), no pages of another solution.
    pub(super) fn properties_solution_opened(
        &mut self,
        solution: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.properties.solution.as_deref() == Some(solution) {
            return;
        }
        self.properties.solution = Some(solution.to_path_buf());
        self.properties.configurations = None;
        self.properties.cache.clear();
        self.properties.launch.clear();
        self.properties.selection = SavedSelection::default();
        for tab in self.properties.pages.keys().cloned().collect::<Vec<_>>() {
            self.drop_pages(&tab);
        }
        let Some(file) = self.selection_file(solution) else {
            return;
        };
        let solution = solution.to_path_buf();
        let read = cx.background_spawn(async move {
            std::fs::read_to_string(&file)
                .ok()
                .and_then(|t| serde_json::from_str::<SavedSelection>(&t).ok())
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(saved) = read.await else { return };
            let _ = this.update_in(cx, |shell, window, cx| {
                if shell.properties.solution.as_deref() != Some(solution.as_path()) {
                    return;
                }
                if let Some(c) = &saved.configuration {
                    shell.builds.configuration = c.clone();
                }
                if let Some(p) = &saved.platform {
                    let default = shell.solution_platforms().first() == Some(p);
                    shell.builds.platform = (!default).then(|| p.clone());
                }
                shell.properties.selection = saved;
                shell.tell_host_selection(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The tree of a generation arrived: the lists and the open pages follow it.
    pub(super) fn properties_tree_arrived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_configurations(window, cx);
        if let Some(p) = self.toolbar_project() {
            self.request_launch_profiles(&p, None, window, cx);
            // Warm the host's evaluation (and this generation's cache) for the startup project, off the UI thread, so
            // its pages open from the cache.
            let mapped = self.mapped(&p);
            let configuration = mapped
                .as_ref()
                .map_or_else(|| self.builds.configuration.clone(), |m| m.0.clone());
            let platform = mapped.map_or_else(|| "AnyCPU".into(), |m| m.1);
            self.properties_values(
                &p,
                &configuration,
                &platform,
                None,
                window,
                cx,
                |_, _, _, _| {},
            );
        }
    }

    /// A new generation: the cached values are stale (CLAUDE.md invariant 12); the open pages are evaluated again.
    pub(super) fn properties_new_generation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.properties.cache.clear();
        for (tab, view) in self.properties.pages.clone() {
            let (project, configuration, platform) = {
                let v = view.read(cx);
                (
                    v.project.clone(),
                    v.configuration.clone(),
                    v.platform.clone(),
                )
            };
            let _ = tab;
            self.load_pages(&project, configuration, platform, window, cx);
        }
    }

    pub(super) fn request_configurations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.solution.is_none() {
            return;
        }
        self.host_call::<host::SolutionConfigurations>((), window, cx, |shell, result, _, cx| {
            match result {
                Ok(c) if c.generation >= shell.generation => {
                    let first_platform = c.platforms.first().cloned();
                    if !c.configurations.is_empty()
                        && !c
                            .configurations
                            .iter()
                            .any(|x| x.eq_ignore_ascii_case(&shell.builds.configuration))
                    {
                        shell.builds.configuration = c.configurations[0].clone();
                    }
                    if let Some(p) = &shell.builds.platform
                        && !c.platforms.iter().any(|x| x == p)
                    {
                        shell.builds.platform = None;
                    }
                    if first_platform.is_some() {
                        shell.builds.platforms = c.platforms.clone();
                    }
                    shell.properties.configurations = Some(c);
                    if let Some(m) = shell.properties.manager.clone() {
                        let rows = shell.configuration_rows();
                        let (sc, sp) = shell.active_selection();
                        m.update(cx, |m, cx| m.set_rows(rows, sc, sp, cx));
                    }
                    cx.notify();
                }
                Ok(_) => {}
                Err(e) => eprintln!("eludite: eludite/solution/configurations: {e}"),
            }
        });
    }

    fn request_launch_profiles(
        &mut self,
        project: &str,
        waiter: Option<mpsc::SyncSender<Outcome>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let params = host::LaunchProfilesParams {
            project: project.to_owned(),
        };
        let project = project.to_owned();
        self.host_call::<host::ProjectLaunchProfiles>(
            params,
            window,
            cx,
            move |shell, result, _, cx| {
                let outcome = match result {
                    Ok(r) => {
                        shell.store_launch(r.clone(), cx);
                        Ok(PropertiesOutputs::LaunchProfiles(
                            shell.launch_output(&project, false),
                        ))
                    }
                    Err(e) => Err(CommandError::Failed(e)),
                };
                if let Some(w) = waiter {
                    let _ = w.send(outcome);
                }
            },
        );
    }

    fn store_launch(&mut self, r: host::LaunchProfilesResult, cx: &mut Context<Self>) {
        let project = r.project.clone();
        for view in self.properties.pages.values() {
            if view.read(cx).project == project {
                view.update(cx, |v, cx| v.set_launch(r.clone(), cx));
            }
        }
        self.properties.launch.insert(project, r);
        cx.notify();
    }

    fn launch_output(&self, project: &str, pending: bool) -> LaunchProfilesOutput {
        let r = self.properties.launch.get(project);
        let profiles: Vec<ProfileRow> = r
            .map(|r| r.profiles.iter().map(profile_row).collect())
            .unwrap_or_default();
        let selected = self.selected_profile(project).or_else(|| {
            profiles
                .iter()
                .find(|p| p.command_name == "Project")
                .map(|p| p.name.clone())
        });
        LaunchProfilesOutput {
            project: project_name(project),
            path: project.to_owned(),
            file: r.map(|r| r.file.clone()).unwrap_or_else(|| {
                Path::new(project)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("Properties")
                    .join("launchSettings.json")
                    .to_string_lossy()
                    .into_owned()
            }),
            exists: r.is_some_and(|r| r.exists),
            selected,
            profiles,
            pending,
        }
    }

    /// Open (or activate) `project`'s pages.
    pub(super) fn open_pages(
        &mut self,
        project: &str,
        page: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.properties.timings = PropertiesTimings {
            opened: Some(Instant::now()),
            ..PropertiesTimings::default()
        };
        let tab = tab_id(project);
        let view = match self.properties.pages.get(&tab) {
            Some(v) => v.clone(),
            None => {
                let theme = self.theme;
                let (p, t) = (project.to_owned(), tab.clone());
                let probe = self.ui_bounds.clone();
                let view = cx.new(|cx| {
                    let mut v = pages::PropertyPages::new(theme, p, t, cx);
                    v.probe = probe;
                    v
                });
                cx.subscribe_in(&view, window, Self::on_pages_event)
                    .detach();
                cx.observe(&view, |_, _, cx| cx.notify()).detach();
                self.properties
                    .documents
                    .borrow_mut()
                    .insert(tab.clone(), view.clone().into());
                self.properties.pages.insert(tab.clone(), view.clone());
                self.controller.open_document(&tab, &project_name(project));
                let mapped = self.mapped(project);
                self.load_pages(
                    project,
                    mapped.as_ref().map(|m| m.0.clone()),
                    mapped.map(|m| m.1),
                    window,
                    cx,
                );
                if let Some(l) = self.properties.launch.get(project).cloned() {
                    view.update(cx, |v, cx| v.set_launch(l, cx));
                } else {
                    self.request_launch_profiles(project, None, window, cx);
                }
                view
            }
        };
        if let Some(page) = page {
            let host_page = if page == "code_analysis" {
                "codeAnalysis"
            } else {
                page
            };
            view.update(cx, |v, cx| v.show_page(host_page, cx));
        }
        let _ = self
            .controller
            .apply(eludite_commands::view::ViewRequest::Show { id: tab });
        cx.notify();
    }

    fn drop_pages(&mut self, tab: &str) {
        self.properties.pages.remove(tab);
        self.properties.documents.borrow_mut().remove(tab);
        self.controller.close_document(tab);
    }

    /// Evaluate the values a project's pages show (cached per generation).
    fn load_pages(
        &mut self,
        project: &str,
        configuration: Option<String>,
        platform: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (configuration, platform) = match (configuration, platform) {
            (Some(c), Some(p)) => (c, p),
            (c, p) => {
                let m = self.mapped(project);
                (
                    c.or_else(|| m.as_ref().map(|m| m.0.clone()))
                        .unwrap_or_else(|| self.builds.configuration.clone()),
                    p.or_else(|| m.map(|m| m.1))
                        .unwrap_or_else(|| "AnyCPU".into()),
                )
            }
        };
        let owned = project.to_owned();
        self.properties_values(
            project,
            &configuration,
            &platform,
            None,
            window,
            cx,
            move |shell, result, _, cx| {
                let tab = tab_id(&owned);
                let Some(view) = shell.properties.pages.get(&tab).cloned() else {
                    return;
                };
                match result {
                    Ok(r) => {
                        shell
                            .properties
                            .timings
                            .shown
                            .get_or_insert_with(Instant::now);
                        view.update(cx, |v, cx| v.set_result(r, cx))
                    }
                    Err(e) => view.update(cx, |v, cx| v.set_error(e, cx)),
                }
            },
        );
    }

    /// `eludite/project/properties` for one configuration, from the cache when the generation has it.
    #[allow(clippy::too_many_arguments)]
    fn properties_values(
        &mut self,
        project: &str,
        configuration: &str,
        platform: &str,
        framework: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(
            &mut Shell,
            Result<ProjectPropertiesResult, String>,
            &mut Window,
            &mut Context<Shell>,
        ) + 'static,
    ) {
        let key = cache_key(project, configuration, platform, framework);
        if let Some(r) = self.properties.cache.get(&key).cloned()
            && r.generation == self.generation
        {
            then(self, Ok(r), window, cx);
            return;
        }
        let params = host::ProjectPropertiesParams {
            project: project.to_owned(),
            configuration: Some(configuration.to_owned()),
            platform: Some(msbuild_platform(platform)),
            framework: framework.map(str::to_owned),
        };
        self.host_call::<host::ProjectProperties>(
            params,
            window,
            cx,
            move |shell, result, window, cx| {
                match result {
                    Ok(r) if r.generation < shell.generation => {
                        // Computed for a solution that has changed since: never shown (invariant 12).
                        then(
                            shell,
                            Err("the solution changed; the values are evaluated again".into()),
                            window,
                            cx,
                        )
                    }
                    Ok(r) => {
                        shell.properties.cache.insert(key, r.clone());
                        then(shell, Ok(r), window, cx)
                    }
                    Err(e) => then(shell, Err(e), window, cx),
                }
            },
        );
    }

    fn on_pages_event(
        &mut self,
        view: &Entity<pages::PropertyPages>,
        event: &pages::PagesEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (project, tab) = {
            let v = view.read(cx);
            (v.project.clone(), v.tab.clone())
        };
        match event {
            pages::PagesEvent::Load {
                configuration,
                platform,
            } => {
                self.load_pages(
                    &project,
                    configuration.clone(),
                    platform.clone(),
                    window,
                    cx,
                );
                if !self.properties.launch.contains_key(&project) {
                    self.request_launch_profiles(&project, None, window, cx);
                }
            }
            pages::PagesEvent::Dirty(dirty) => {
                self.controller.set_document_dirty(&tab, *dirty);
                cx.notify();
            }
            pages::PagesEvent::Save => {
                self.run(
                    eludite_commands::workspace::EDITOR_SAVE,
                    json!({ "path": tab }),
                    window,
                    cx,
                );
            }
            pages::PagesEvent::Browse { key } => {
                let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: true,
                    directories: key.starts_with("profile:working"),
                    multiple: false,
                    prompt: Some("Select".into()),
                });
                let (key, view, project) = (key.clone(), view.clone(), project.clone());
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = paths.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        // A path in the project's folder is written relative to it, as Visual Studio does.
                        let dir = Path::new(&project)
                            .parent()
                            .unwrap_or(Path::new("/"))
                            .to_path_buf();
                        let shown = path
                            .strip_prefix(&dir)
                            .map(Path::to_path_buf)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .into_owned();
                        let _ = this.update_in(cx, |_, window, cx| {
                            view.update(cx, |v, cx| v.edit(&key, Some(shown), window, cx))
                        });
                    }
                })
                .detach();
            }
            pages::PagesEvent::Profile {
                action,
                profile,
                new_name,
                values,
            } => {
                let mut args = json!({ "project": project, "profile": profile,
                                       "action": serde_json::to_value(action).unwrap_or_default() });
                if let Some(n) = new_name {
                    args["new_name"] = json!(n);
                }
                if let Some(v) = values {
                    args["values"] = Value::Object(v.clone());
                }
                if *action == ProfileAction::Create || *action == ProfileAction::Rename {
                    let shown = new_name.clone().unwrap_or_else(|| profile.clone());
                    view.update(cx, |v, cx| v.select_profile(&shown, cx));
                }
                self.run(props::SET_LAUNCH_PROFILE, args, window, cx);
            }
        }
    }

    /// The property pages tab `path` names, or the active document when it is one (Save and Close on a tab).
    pub(super) fn pages_tab(&self, path: Option<&str>) -> Option<String> {
        let id = match path {
            Some(p) => p.to_owned(),
            None => self.controller.active_document()?,
        };
        if self.properties.pages.contains_key(&id) {
            return Some(id);
        }
        // The project file itself names its pages for Save when they are the active tab.
        let tab = tab_id(&id);
        (path.is_some()
            && self.properties.pages.contains_key(&tab)
            && self.controller.active_document().as_deref() == Some(tab.as_str()))
        .then_some(tab)
    }

    /// `eludite.editor.save` on a property pages tab: every dirty value in one `eludite/project/setProperty`.
    pub(super) fn save_pages(
        &mut self,
        tab: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let view = self
            .properties
            .pages
            .get(tab)
            .cloned()
            .ok_or_else(|| CommandError::Failed(format!("{tab} is not open")))?;
        let (project, dirty, generation) = {
            let v = view.read(cx);
            (
                v.project.clone(),
                v.dirty.clone(),
                v.result.as_ref().map_or(self.generation, |r| r.generation),
            )
        };
        let bytes = std::fs::metadata(&project).map(|m| m.len()).unwrap_or(0);
        if dirty.is_empty() {
            return Ok(WorkspaceOutput::Save(SaveOutput {
                path: project,
                bytes,
            }));
        }
        let edits: Vec<host::PropertyEdit> = dirty
            .iter()
            .map(|d| host::PropertyEdit {
                name: d.name.clone(),
                value: d.value.clone(),
                configuration: d.configuration.clone(),
                platform: d.platform.as_deref().map(msbuild_platform),
                framework: None,
                all_configurations: d.all_configurations,
                override_inherited: d.override_inherited,
            })
            .collect();
        self.properties.timings.save_sent = Some(Instant::now());
        let answered = project.clone();
        self.send_edits(&project, generation, edits, window, cx, move |shell, result, _, cx| {
            shell.properties.timings.save_answered = Some(Instant::now());
            let Some(view) = shell.properties.pages.get(&tab_id(&answered)).cloned() else {
                return;
            };
            match result {
                Ok(r) => {
                    let inherited: Vec<String> = r
                        .results
                        .iter()
                        .filter(|x| x.status == EditStatus::Inherited)
                        .map(|x| x.name.clone())
                        .collect();
                    let summary = r
                        .results
                        .iter()
                        .map(|x| format!("{} {}", x.name, status_name(x.status)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let message = if inherited.is_empty() {
                        format!("Saved: {summary}.")
                    } else {
                        format!(
                            "Saved: {summary}. {} inherited: use Override to set it in the project file.",
                            inherited.join(", ")
                        )
                    };
                    view.update(cx, |v, cx| v.saved(message, cx));
                }
                Err(e) => view.update(cx, |v, cx| v.set_message(format!("Not saved: {e}"), cx)),
            }
        });
        Ok(WorkspaceOutput::Save(SaveOutput {
            path: project,
            bytes,
        }))
    }

    /// `eludite/project/setProperty`; after a write the host reloaded the solution: the tree is asked for again.
    fn send_edits(
        &mut self,
        project: &str,
        generation: u64,
        edits: Vec<host::PropertyEdit>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(
            &mut Shell,
            Result<host::ProjectSetPropertyResult, String>,
            &mut Window,
            &mut Context<Shell>,
        ) + 'static,
    ) {
        let params = host::ProjectSetPropertyParams {
            project: project.to_owned(),
            generation,
            edits,
        };
        self.host_call::<host::ProjectSetProperty>(
            params,
            window,
            cx,
            move |shell, result, window, cx| {
                if let Ok(r) = &result
                    && r.written
                {
                    shell.properties.cache.clear();
                    shell.session.refresh_tree();
                }
                if let Err(e) = &result
                    && e.contains("generation")
                {
                    // The values were read under an older generation: show the current ones.
                    shell.properties_new_generation(window, cx);
                }
                then(shell, result, window, cx)
            },
        );
    }

    /// `eludite.file.close` on a property pages tab.
    pub(super) fn close_pages(
        &mut self,
        tab: &str,
        save: Option<CloseSave>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let dirty = self
            .properties
            .pages
            .get(tab)
            .is_some_and(|v| v.read(cx).is_dirty());
        let mut saved = false;
        if dirty {
            match save {
                None => {
                    return Err(CommandError::Failed(format!(
                        "{tab} has unsaved changes; pass \"save\": \"save\" or \"discard\""
                    )));
                }
                Some(CloseSave::Save) => {
                    self.save_pages(tab, window, cx)?;
                    saved = true;
                }
                Some(CloseSave::Discard) => {}
            }
        }
        self.drop_pages(tab);
        cx.notify();
        Ok(WorkspaceOutput::FileClose(
            eludite_commands::workspace::FileCloseOutput {
                path: tab.to_owned(),
                closed: true,
                saved,
            },
        ))
    }

    /// The UI's side of [`PropertiesBus`]: Visual Studio's questions first (Save on closing dirty pages), then the bus.
    /// Returns true when it handled `command`.
    pub(super) fn run_properties(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if command == eludite_commands::workspace::FILE_CLOSE
            && args.get("save").is_none()
            && let Some(path) = args.get("path").and_then(Value::as_str)
            && let Some(view) = self.properties.pages.get(path).cloned()
            && view.read(cx).is_dirty()
        {
            let path = path.to_owned();
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("Save changes to {}?", view.read(cx).name),
                Some("The project's properties have unsaved changes."),
                &["Save", "Don't Save", "Cancel"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let save = match answer.await {
                    Ok(0) => "save",
                    Ok(1) => "discard",
                    _ => return,
                };
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.run(
                        eludite_commands::workspace::FILE_CLOSE,
                        json!({ "path": path, "save": save }),
                        window,
                        cx,
                    )
                });
            })
            .detach();
            return true;
        }
        false
    }

    /// Apply a request of brief 0049's commands. The answer is immediate (staged for a UI-thread call); with `waiter`
    /// (an agent) the host's answer goes there instead when the request needs the host.
    pub(super) fn apply_properties(
        &mut self,
        request: PropertiesRequest,
        waiter: Option<mpsc::SyncSender<Outcome>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let immediate = |waiter: Option<mpsc::SyncSender<Outcome>>, outcome: Outcome| {
            if let Some(w) = waiter {
                let _ = w.send(outcome.clone());
            }
            outcome
        };
        match request {
            PropertiesRequest::Properties(i) => {
                let project = match self.properties_project(i.project.as_deref(), cx) {
                    Ok(p) => p,
                    Err(e) => return immediate(waiter, Err(e)),
                };
                if i.open {
                    self.open_pages(&project, i.page.as_deref(), window, cx);
                }
                let mapped = self.mapped(&project);
                let configuration = i
                    .configuration
                    .clone()
                    .or_else(|| mapped.as_ref().map(|m| m.0.clone()))
                    .unwrap_or_else(|| self.builds.configuration.clone());
                let platform = i
                    .platform
                    .clone()
                    .or_else(|| mapped.map(|m| m.1))
                    .unwrap_or_else(|| "AnyCPU".into());
                let key = cache_key(&project, &configuration, &platform, i.framework.as_deref());
                let cached = self
                    .properties
                    .cache
                    .get(&key)
                    .filter(|r| r.generation == self.generation)
                    .map(|r| properties_output(r, i.page.as_deref()));
                let now = match cached {
                    Some(mut o) => {
                        o.opened = i.open;
                        o
                    }
                    None => PropertiesOutput {
                        project: project_name(&project),
                        path: project.clone(),
                        kind: "sdk".into(),
                        configuration: configuration.clone(),
                        platform: msbuild_platform(&platform),
                        framework: i.framework.clone(),
                        configurations: Vec::new(),
                        platforms: Vec::new(),
                        frameworks: self.project_frameworks(&project),
                        pages: Vec::new(),
                        properties: Vec::new(),
                        generation: self.generation,
                        opened: i.open,
                        pending: true,
                    },
                };
                if !now.pending || waiter.is_none() {
                    if now.pending {
                        // The UI's call: the pages (or the next call) show the host's answer.
                        self.properties_values(
                            &project,
                            &configuration,
                            &platform,
                            i.framework.as_deref(),
                            window,
                            cx,
                            |_, _, _, _| {},
                        );
                    }
                    return immediate(waiter, Ok(PropertiesOutputs::Properties(Box::new(now))));
                }
                let (page, open) = (i.page.clone(), i.open);
                self.properties_values(
                    &project,
                    &configuration,
                    &platform,
                    i.framework.as_deref(),
                    window,
                    cx,
                    move |_, result, _, _| {
                        let outcome = result
                            .map(|r| {
                                let mut o = properties_output(&r, page.as_deref());
                                o.opened = open;
                                PropertiesOutputs::Properties(Box::new(o))
                            })
                            .map_err(CommandError::Failed);
                        if let Some(w) = waiter {
                            let _ = w.send(outcome);
                        }
                    },
                );
                Ok(PropertiesOutputs::Properties(Box::new(now)))
            }
            PropertiesRequest::SetProperty(i) => {
                let project = match self.properties_project(i.project.as_deref(), cx) {
                    Ok(p) => p,
                    Err(e) => return immediate(waiter, Err(e)),
                };
                let entry = self
                    .properties
                    .cache
                    .values()
                    .filter(|r| r.project == project)
                    .find_map(|r| {
                        r.properties
                            .iter()
                            .find(|p| p.name.eq_ignore_ascii_case(&i.property))
                            .cloned()
                    });
                let value = match &i.value {
                    PropertyInputValue::Text(t) => Some(t.clone()),
                    PropertyInputValue::Bool(b) => Some(match (&entry, b) {
                        (Some(e), true) => e.true_value.clone().unwrap_or_else(|| "true".into()),
                        (Some(e), false) => e.false_value.clone().unwrap_or_else(|| "false".into()),
                        (None, b) => b.to_string(),
                    }),
                    PropertyInputValue::Remove => None,
                };
                let platform = match (&i.configuration, &i.platform) {
                    (Some(_), None) => Some(
                        self.mapped(&project)
                            .map_or_else(|| "AnyCPU".into(), |m| m.1),
                    ),
                    (_, p) => p.as_deref().map(msbuild_platform),
                };
                let edit = host::PropertyEdit {
                    name: entry
                        .as_ref()
                        .map_or_else(|| i.property.clone(), |e| e.name.clone()),
                    value,
                    configuration: i.configuration.clone(),
                    platform: platform.clone(),
                    framework: i.framework.clone(),
                    all_configurations: i.all_configurations,
                    override_inherited: i.override_inherited,
                };
                let pending = SetPropertyOutput {
                    project: project_name(&project),
                    path: project.clone(),
                    property: edit.name.clone(),
                    status: "pending".into(),
                    condition: None,
                    line: None,
                    inherited_from: None,
                    removed_conditions: Vec::new(),
                    value: None,
                    generation: self.generation,
                };
                let generation = self.generation;
                let read_back = (
                    i.configuration.clone().unwrap_or_else(|| {
                        self.mapped(&project)
                            .map_or_else(|| self.builds.configuration.clone(), |m| m.0)
                    }),
                    platform.unwrap_or_else(|| {
                        self.mapped(&project)
                            .map_or_else(|| "AnyCPU".into(), |m| m.1)
                    }),
                    i.framework.clone(),
                );
                let name = edit.name.clone();
                self.send_edits(
                    &project,
                    generation,
                    vec![edit],
                    window,
                    cx,
                    move |shell, result, window, cx| {
                        let r = match result {
                            Ok(r) => r,
                            Err(e) => {
                                if let Some(w) = waiter {
                                    let _ = w.send(Err(CommandError::Failed(e)));
                                }
                                return;
                            }
                        };
                        let first = r.results.first().cloned();
                        let mut out = SetPropertyOutput {
                            project: project_name(&r.project),
                            path: r.project.clone(),
                            property: name.clone(),
                            status: first
                                .as_ref()
                                .map_or("unchanged", |f| status_name(f.status))
                                .into(),
                            condition: first.as_ref().and_then(|f| f.condition.clone()),
                            line: first.as_ref().and_then(|f| f.line),
                            inherited_from: first.as_ref().and_then(|f| f.inherited_from.clone()),
                            removed_conditions: first
                                .and_then(|f| f.removed_conditions)
                                .unwrap_or_default(),
                            value: None,
                            generation: r.generation,
                        };
                        if !r.written {
                            if let Some(w) = waiter {
                                let _ = w.send(Ok(PropertiesOutputs::SetProperty(out)));
                            }
                            return;
                        }
                        // The value after the edit, as the pages show it (the host evaluates the reloaded file).
                        let (c, p, f) = read_back;
                        let params = host::ProjectPropertiesParams {
                            project: r.project.clone(),
                            configuration: Some(c),
                            platform: Some(msbuild_platform(&p)),
                            framework: f,
                        };
                        shell.host_call::<host::ProjectProperties>(
                            params,
                            window,
                            cx,
                            move |_, after, _, _| {
                                if let Ok(after) = after {
                                    out.value = after
                                        .properties
                                        .iter()
                                        .find(|x| x.name == name)
                                        .map(|x| x.value.clone());
                                }
                                if let Some(w) = waiter {
                                    let _ = w.send(Ok(PropertiesOutputs::SetProperty(out)));
                                }
                            },
                        );
                    },
                );
                Ok(PropertiesOutputs::SetProperty(pending))
            }
            PropertiesRequest::LaunchProfiles { project } => {
                let project = match self.properties_project(project.as_deref(), cx) {
                    Ok(p) => p,
                    Err(e) => return immediate(waiter, Err(e)),
                };
                let now = self.launch_output(&project, true);
                self.request_launch_profiles(&project, waiter, window, cx);
                Ok(PropertiesOutputs::LaunchProfiles(now))
            }
            PropertiesRequest::SetLaunchProfile(i) => {
                let project = match self.properties_project(i.project.as_deref(), cx) {
                    Ok(p) => p,
                    Err(e) => return immediate(waiter, Err(e)),
                };
                if i.action == ProfileAction::Select {
                    if let Some(l) = self.properties.launch.get(&project)
                        && !l.profiles.iter().any(|p| p.name == i.profile)
                    {
                        return immediate(
                            waiter,
                            Err(CommandError::InvalidInput(format!(
                                "no launch profile `{}` in {}",
                                i.profile, l.file
                            ))),
                        );
                    }
                    self.properties
                        .selection
                        .profiles
                        .insert(project.clone(), i.profile.clone());
                    self.persist_selection(cx);
                    for view in self.properties.pages.values() {
                        if view.read(cx).project == project {
                            view.update(cx, |v, cx| v.select_profile(&i.profile, cx));
                        }
                    }
                    cx.notify();
                    return immediate(
                        waiter,
                        Ok(PropertiesOutputs::LaunchProfiles(
                            self.launch_output(&project, false),
                        )),
                    );
                }
                let action = match i.action {
                    ProfileAction::Set => host::LaunchProfileAction::Set,
                    ProfileAction::Create => host::LaunchProfileAction::Create,
                    ProfileAction::Rename => host::LaunchProfileAction::Rename,
                    ProfileAction::Delete => host::LaunchProfileAction::Delete,
                    ProfileAction::Select => unreachable!("answered above"),
                };
                let params = host::SetLaunchProfileParams {
                    project: project.clone(),
                    generation: self.generation,
                    action,
                    profile: i.profile.clone(),
                    new_name: i.new_name.clone(),
                    values: i.values.as_ref().map(camel_values),
                };
                let now = self.launch_output(&project, true);
                let renamed = (i.action == ProfileAction::Rename)
                    .then(|| (i.profile.clone(), i.new_name.clone()));
                self.host_call::<host::ProjectSetLaunchProfile>(
                    params,
                    window,
                    cx,
                    move |shell, result, _, cx| {
                        let outcome = match result {
                            Ok(r) => {
                                // A renamed profile stays the toolbar's choice.
                                if let Some((old, Some(new))) = renamed
                                    && shell.properties.selection.profiles.get(&project)
                                        == Some(&old)
                                {
                                    shell
                                        .properties
                                        .selection
                                        .profiles
                                        .insert(project.clone(), new);
                                    shell.persist_selection(cx);
                                }
                                shell.store_launch(r, cx);
                                Ok(PropertiesOutputs::LaunchProfiles(
                                    shell.launch_output(&project, false),
                                ))
                            }
                            Err(e) => {
                                for view in shell.properties.pages.values() {
                                    if view.read(cx).project == project {
                                        view.update(cx, |v, cx| {
                                            v.set_message(
                                                format!("Launch profile not changed: {e}"),
                                                cx,
                                            )
                                        });
                                    }
                                }
                                Err(CommandError::Failed(e))
                            }
                        };
                        if let Some(w) = waiter {
                            let _ = w.send(outcome);
                        }
                    },
                );
                Ok(PropertiesOutputs::LaunchProfiles(now))
            }
            PropertiesRequest::Configurations => immediate(
                waiter,
                Ok(PropertiesOutputs::Configurations(
                    self.configurations_output(),
                )),
            ),
            PropertiesRequest::SelectConfiguration(i) => {
                let out = self.select_configuration(i, window, cx);
                immediate(waiter, out.map(PropertiesOutputs::Select))
            }
            PropertiesRequest::SetConfiguration { mappings } => {
                let Some(mappings) = mappings else {
                    if waiter.is_some() {
                        return immediate(
                            waiter,
                            Err(CommandError::InvalidInput(
                                "give `mappings`: without them the Configuration Manager dialog opens, which is for the UI".into(),
                            )),
                        );
                    }
                    self.open_configuration_manager(window, cx);
                    return Ok(PropertiesOutputs::SetConfiguration(
                        SetConfigurationOutput {
                            path: self
                                .solution
                                .as_ref()
                                .map(|p| p.to_string_lossy().into_owned()),
                            written: false,
                            dialog: true,
                            pending: false,
                            projects: self.configuration_rows(),
                        },
                    ));
                };
                let (sc, sp) = self.active_selection();
                let mut edits = Vec::new();
                for m in mappings {
                    let project = match self.properties_project(Some(&m.project), cx) {
                        Ok(p) => p,
                        Err(e) => return immediate(waiter, Err(e)),
                    };
                    edits.push(host::MappingEdit {
                        project,
                        solution_configuration: m
                            .solution_configuration
                            .unwrap_or_else(|| sc.clone()),
                        solution_platform: m.solution_platform.unwrap_or_else(|| sp.clone()),
                        configuration: m.configuration,
                        platform: m.platform,
                        build: m.build,
                    });
                }
                let params = host::SolutionSetConfigurationParams {
                    generation: self.generation,
                    select: None,
                    mappings: Some(edits),
                };
                let now = SetConfigurationOutput {
                    path: self
                        .solution
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    written: false,
                    dialog: false,
                    pending: true,
                    projects: self.configuration_rows(),
                };
                self.host_call::<host::SolutionSetConfiguration>(
                    params,
                    window,
                    cx,
                    move |shell, result, window, cx| {
                        match result {
                            Ok(r) => {
                                if r.written {
                                    shell.session.refresh_tree();
                                }
                                // The mapping after the edit, then the answer.
                                shell.host_call::<host::SolutionConfigurations>(
                                    (),
                                    window,
                                    cx,
                                    move |shell, c, _, cx| {
                                        if let Ok(c) = c {
                                            shell.properties.configurations = Some(c);
                                            if let Some(m) = shell.properties.manager.clone() {
                                                let rows = shell.configuration_rows();
                                                let (sc, sp) = shell.active_selection();
                                                m.update(cx, |m, cx| m.set_rows(rows, sc, sp, cx));
                                            }
                                            cx.notify();
                                        }
                                        if let Some(w) = waiter {
                                            let _ =
                                                w.send(Ok(PropertiesOutputs::SetConfiguration(
                                                    SetConfigurationOutput {
                                                        path: r.path.clone(),
                                                        written: r.written,
                                                        dialog: false,
                                                        pending: false,
                                                        projects: shell.configuration_rows(),
                                                    },
                                                )));
                                        }
                                    },
                                );
                            }
                            Err(e) => {
                                if let Some(w) = waiter {
                                    let _ = w.send(Err(CommandError::Failed(e)));
                                }
                            }
                        }
                    },
                );
                Ok(PropertiesOutputs::SetConfiguration(now))
            }
        }
    }

    /// The projects' rows of `configurations` and Configuration Manager.
    pub fn configuration_rows(&self) -> Vec<ProjectConfigurationsRow> {
        let Some(c) = &self.properties.configurations else {
            return Vec::new();
        };
        c.projects
            .iter()
            .map(|p| {
                let frameworks = self.project_frameworks(&p.path);
                ProjectConfigurationsRow {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    configurations: p.configurations.clone(),
                    platforms: p.platforms.clone(),
                    framework: (frameworks.len() > 1)
                        .then(|| {
                            self.selected_framework(&p.path)
                                .or_else(|| frameworks.first().cloned())
                        })
                        .flatten(),
                    frameworks,
                    profile: self.selected_profile(&p.path),
                    mappings: p
                        .mappings
                        .iter()
                        .map(|m| MappingRow {
                            solution_configuration: m.solution_configuration.clone(),
                            solution_platform: m.solution_platform.clone(),
                            configuration: m.configuration.clone(),
                            platform: m.platform.clone(),
                            build: m.build,
                            deploy: m.deploy,
                        })
                        .collect(),
                }
            })
            .collect()
    }

    fn configurations_output(&self) -> ConfigurationsOutput {
        let (configuration, platform) = self.active_selection();
        let (path, format) = match (&self.properties.configurations, &self.solution) {
            (Some(c), _) => (
                c.path.clone(),
                c.format.map(|f| match f {
                    host::SolutionFormat::Sln => "sln".to_owned(),
                    host::SolutionFormat::Slnx => "slnx".into(),
                    host::SolutionFormat::Project => "project".into(),
                }),
            ),
            (None, Some(s)) => (Some(s.to_string_lossy().into_owned()), None),
            (None, None) => (
                self.cargo_workspace()
                    .map(|w| w.manifest.to_string_lossy().into_owned()),
                self.cargo_workspace().map(|_| "cargo".to_owned()),
            ),
        };
        ConfigurationsOutput {
            path,
            format,
            configurations: self.solution_configurations(),
            platforms: self.solution_platforms(),
            active: SelectionRow {
                configuration,
                platform,
            },
            projects: self.configuration_rows(),
        }
    }

    /// The toolbar's lists and `eludite.solution.select_configuration`.
    pub(super) fn select_configuration(
        &mut self,
        i: props::SelectConfigurationInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<SelectOutput, CommandError> {
        if let Some(c) = &i.configuration {
            let list = self.solution_configurations();
            let Some(c) = list.iter().find(|x| x.eq_ignore_ascii_case(c)) else {
                return Err(CommandError::InvalidInput(format!(
                    "{c} is not a solution configuration ({})",
                    list.join(", ")
                )));
            };
            self.builds.configuration = c.clone();
        }
        if let Some(p) = &i.platform {
            let list = self.solution_platforms();
            let Some(p) = list.iter().find(|x| x.eq_ignore_ascii_case(p)) else {
                return Err(CommandError::InvalidInput(format!(
                    "{p} is not a solution platform ({})",
                    list.join(", ")
                )));
            };
            let default = list.first() == Some(p);
            self.builds.platform = (!default).then(|| p.clone());
        }
        let mut chosen = None;
        if let Some(f) = &i.framework {
            let project = match &i.project {
                Some(p) => self.properties_project(Some(p), cx)?,
                None => self
                    .toolbar_project()
                    .ok_or_else(|| CommandError::Failed("there is no startup project".into()))?,
            };
            let frameworks = self.project_frameworks(&project);
            let Some(f) = frameworks.iter().find(|x| x.eq_ignore_ascii_case(f)) else {
                return Err(CommandError::InvalidInput(format!(
                    "{f} is not a target framework of {} ({})",
                    project_name(&project),
                    frameworks.join(", ")
                )));
            };
            self.properties
                .selection
                .frameworks
                .insert(project.clone(), f.clone());
            chosen = Some((project_name(&project), f.clone()));
        }
        self.properties.toolbar_menu = None;
        self.persist_selection(cx);
        if i.configuration.is_some() || i.platform.is_some() {
            self.tell_host_selection(window, cx);
            // The open pages show the new selection's values.
            for view in self.properties.pages.values().cloned().collect::<Vec<_>>() {
                let project = view.read(cx).project.clone();
                let mapped = self.mapped(&project);
                view.update(cx, |v, cx| {
                    v.configuration = mapped.as_ref().map(|m| m.0.clone());
                    v.platform = mapped.as_ref().map(|m| m.1.clone());
                    cx.notify();
                });
                self.load_pages(
                    &project,
                    mapped.as_ref().map(|m| m.0.clone()),
                    mapped.map(|m| m.1),
                    window,
                    cx,
                );
            }
            if let Some(m) = self.properties.manager.clone() {
                let rows = self.configuration_rows();
                let (sc, sp) = self.active_selection();
                m.update(cx, |m, cx| m.set_rows(rows, sc, sp, cx));
            }
        }
        cx.notify();
        let (configuration, platform) = self.active_selection();
        Ok(SelectOutput {
            configuration,
            platform,
            project: chosen.as_ref().map(|c| c.0.clone()),
            framework: chosen.map(|c| c.1),
        })
    }

    /// The host's default configuration follows the selection (`eludite/solution/setConfiguration` `select`).
    fn tell_host_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.solution.is_none() {
            return;
        }
        let (configuration, platform) = self.active_selection();
        let params = host::SolutionSetConfigurationParams {
            generation: self.generation,
            select: Some(host::Selection {
                configuration,
                platform,
            }),
            mappings: None,
        };
        self.host_call::<host::SolutionSetConfiguration>(params, window, cx, |_, result, _, _| {
            if let Err(e) = result {
                super::documents::trace(format_args!("eludite/solution/setConfiguration: {e}"));
            }
        });
    }

    /// The framework a test run or a debug start uses for `project`: the explicit one, else the Target Framework
    /// list's for a multi-targeted project.
    pub fn framework_for(&self, project: &str, explicit: Option<&str>) -> Option<String> {
        let frameworks = self.project_frameworks(project);
        if let Some(f) = explicit {
            return frameworks
                .iter()
                .find(|x| x.eq_ignore_ascii_case(f))
                .cloned();
        }
        if frameworks.len() > 1 {
            return self.selected_framework(project);
        }
        None
    }

    /// The Output window's `Skipped Build` lines for the projects the active selection does not build (Visual
    /// Studio's text).
    pub(super) fn write_skipped_builds(&mut self, cx: &mut Context<Self>) {
        let skipped = self.skipped_projects();
        if skipped.is_empty() {
            return;
        }
        let mut text = String::new();
        for (name, configuration) in skipped {
            text.push_str(&format!(
                "------ Skipped Build: Project: {name}, Configuration: {configuration} ------\nProject not selected to build for this solution configuration \n"
            ));
        }
        self.output.update(cx, |o, cx| {
            o.append(eludite_commands::build::OutputSource::Build, &text, cx)
        });
    }

    #[cfg(test)]
    pub fn property_pages(&self, project: &str) -> Option<Entity<pages::PropertyPages>> {
        self.properties.pages.get(&tab_id(project)).cloned()
    }

    #[allow(dead_code)]
    pub fn properties_timings(&self) -> &PropertiesTimings {
        &self.properties.timings
    }
}

impl Shell {
    /// The toolbar's choice for the launch F5 starts now (brief 0049); takes `eludite.debug.start`'s `framework`.
    pub(super) fn launch_choice(&mut self) -> super::debug::LaunchChoice {
        let configurations = self
            .properties
            .configurations
            .as_ref()
            .map(|c| {
                c.projects
                    .iter()
                    .filter_map(|p| self.mapped(&p.path).map(|m| (p.path.clone(), m.0)))
                    .collect()
            })
            .unwrap_or_default();
        super::debug::LaunchChoice {
            configuration: self.builds.configuration.clone(),
            configurations,
            frameworks: self.properties.selection.frameworks.clone(),
            profiles: self.properties.selection.profiles.clone(),
            framework: self.properties.start_framework.take(),
        }
    }
}
