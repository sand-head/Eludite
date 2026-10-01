//! The root view: menu bar, docking area and status bar, plus the workspace: the host session, Solution Explorer,
//! the open documents and the Error List (brief 0012), IntelliSense (brief 0013), and navigation: Go To Definition,
//! the navigation history, Find All References and the Error List's filters (brief 0014).
//!
//! Keys and menu items both produce [`RunCommand`]; this view's action handler is the one place the UI turns that
//! into a command-bus invocation. The file and editor commands are applied here, on the UI thread, whoever invokes
//! them (see `target`).

mod documents;
pub mod error_list;
pub mod explorer;
pub mod intellisense;
#[cfg(test)]
mod intellisense_tests;
pub mod navigation;
#[cfg(test)]
mod navigation_tests;
pub mod references;
pub mod session;
pub mod target;
#[cfg(test)]
mod tests;
pub mod workspace_edit;
#[cfg(test)]
mod workspace_edit_tests;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eludite_commands::diagnostics::{self, Diagnostic as ListedDiagnostic, Severity};
use eludite_commands::workspace::{self, WorkspaceRequest};
use eludite_commands::{CommandError, CommandRegistry, builtins};
use eludite_docking::{DockController, DockHost, DocumentTab, Persistence, Probe, ids};
use eludite_editor::EditorView;
use eludite_editor::syntax::LanguageRegistry;
use eludite_lsp::host::{
    HostDiagnostic, HostDiagnosticSeverity, LanguageServerState, LoadPhase, SolutionState,
};
use eludite_lsp::lsp;
use eludite_ui::{
    MenuBar, RunCommand, SHELL_CONTEXT, SlotAlign, StatusBar, Theme, menu_bar_with, slots,
    vs_keymap,
};
use eludite_workspace::explorer::SolutionModel;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, PathPromptOptions, PromptLevel, Render, StyleRefinement, Styled,
    Task, Window, div,
};
use serde_json::{Value, json};

use self::documents::{Document, uri_to_path};
use self::error_list::{ErrorList, ErrorRow};
use self::explorer::{Placeholder, SolutionExplorer};
use self::navigation::Navigation;
use self::references::{References, ReferencesEvent, ReferencesWindow};
use self::session::{HostLaunch, HostSession, SessionEvent};
use self::target::{ShellTarget, UiJob};

type AfterPresent = Box<dyn FnOnce(&mut Window, &mut App)>;

/// A 1-based line and column.
type Caret = (u32, u32);

/// Status bar slot: the solution's load state (left, after `state`).
pub const SOLUTION_SLOT: &str = "solution";
/// Status bar slot: the host and the language server (right).
pub const LANGUAGE_SERVER_SLOT: &str = "language_server";

/// The id of the placeholder document tab of the default layout.
const WELCOME: &str = "welcome";

/// What the shell needs from the command bus side: wired by [`register_workspace`].
pub struct Services {
    pub session: HostSession,
    pub events: UnboundedReceiver<SessionEvent>,
    pub jobs: UnboundedReceiver<UiJob>,
    /// The Error List rows `diagnostics.list` reads, on any thread.
    pub published: Arc<Mutex<Vec<ListedDiagnostic>>>,
}

/// Start the host session and register the workspace commands and `diagnostics.list` on `commands`. Call on the UI
/// thread.
pub fn register_workspace(commands: &mut CommandRegistry, launch: HostLaunch) -> Services {
    let (session, events) = HostSession::spawn(launch);
    let (jobs_tx, jobs) = unbounded();
    workspace::register(
        commands,
        Arc::new(ShellTarget {
            session: session.clone(),
            ui_thread: std::thread::current().id(),
            jobs: jobs_tx,
        }),
    );
    let published: Arc<Mutex<Vec<ListedDiagnostic>>> = Arc::default();
    let source = published.clone();
    diagnostics::register(
        commands,
        Arc::new(move || source.lock().unwrap_or_else(|e| e.into_inner()).clone()),
    )
    .expect("diagnostics.list registers once");
    Services {
        session,
        events,
        jobs,
        published,
    }
}

/// When the steps of opening a solution happened (the `--timings-out` harness and the brief 0012 report).
#[derive(Debug, Default, Clone)]
pub struct Timings {
    /// `eludite.solution.open` invoked.
    pub open: Option<Instant>,
    /// The first opened document's editor was created.
    pub editable: Option<Instant>,
    pub tree: Option<Instant>,
    /// First `publishDiagnostics` for an open document.
    pub first_diagnostics: Option<Instant>,
    /// First non-empty one.
    pub first_nonempty_diagnostics: Option<Instant>,
    pub loaded: Option<Instant>,
    /// `publishDiagnostics` notifications applied so far.
    pub diagnostics_events: usize,
}

pub struct Shell {
    theme: Theme,
    commands: Arc<CommandRegistry>,
    controller: DockController,
    menu: Entity<MenuBar>,
    dock: Entity<DockHost>,
    explorer: Entity<SolutionExplorer>,
    error_list: Entity<ErrorList>,
    references_window: Entity<ReferencesWindow>,
    status: StatusBar,
    focus: FocusHandle,
    on_first_render: Option<AfterPresent>,
    session: HostSession,
    published: Arc<Mutex<Vec<ListedDiagnostic>>>,
    languages: Arc<LanguageRegistry>,
    /// Open documents by tab id (the absolute path).
    documents: HashMap<String, Document>,
    /// What the document area draws for each tab id.
    views: Rc<RefCell<HashMap<String, Entity<EditorView>>>>,
    /// Files being read, with the caret position to apply when they open.
    loading: HashMap<String, (Task<()>, Option<Caret>)>,
    /// Agents' requests waiting for a file to finish loading.
    load_waiters: HashMap<String, Vec<futures::channel::oneshot::Sender<()>>>,
    solution: Option<PathBuf>,
    generation: u64,
    /// Diagnostics by document URI, current generation only.
    diagnostics: BTreeMap<String, Vec<lsp::Diagnostic>>,
    /// The solution's load diagnostics (`eludite/solution/status`).
    host_diagnostics: Vec<HostDiagnostic>,
    /// The language server's state and the solution's load state for the current generation: whether IntelliSense
    /// asks the server, the syntax fallback, or both (brief 0013).
    ls_state: Option<LanguageServerState>,
    solution_state: Option<SolutionState>,
    features: intellisense::ServerFeatures,
    completion_timings: Vec<intellisense::CompletionTiming>,
    /// Agents waiting for an IntelliSense answer: woken whenever an editor changes.
    intellisense_waiters: Vec<futures::channel::oneshot::Sender<()>>,
    /// Go To Definition, the picker and the history (brief 0014).
    navigation: Navigation,
    /// Find All References in flight (brief 0014).
    references: References,
    timings: Timings,
    _tasks: Vec<Task<()>>,
}

fn tool_body(
    explorer: Entity<SolutionExplorer>,
    error_list: Entity<ErrorList>,
    references: Entity<ReferencesWindow>,
) -> impl Fn(&str, &Theme) -> AnyElement {
    // Cached: they re-render when they change, not on every keystroke frame of the editor.
    move |id, _| match id {
        ids::SOLUTION_EXPLORER => explorer
            .clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element(),
        ids::ERROR_LIST => error_list
            .clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element(),
        ids::FIND_ALL_REFERENCES => references
            .clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element(),
        // Titled empty panels until later briefs fill them.
        _ => div().into_any_element(),
    }
}

fn document_body(
    views: Rc<RefCell<HashMap<String, Entity<EditorView>>>>,
) -> impl Fn(&DocumentTab, &Theme) -> AnyElement {
    move |tab, theme| {
        if let Some(view) = views.borrow().get(&tab.id) {
            return view.clone().into_any_element();
        }
        let text = if tab.id == WELCOME {
            "Open a solution with File > Open > Project/Solution (Ctrl+Shift+O)."
        } else {
            "Loading\u{2026}"
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .child(div().text_size(gpui::px(20.)).child(tab.title.clone()))
            .child(div().text_color(theme.text_muted).child(text))
            .into_any_element()
    }
}

impl Shell {
    pub fn new(
        commands: Arc<CommandRegistry>,
        controller: DockController,
        theme: Theme,
        persistence: Option<Persistence>,
        services: Services,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let registry = commands.clone();
        let menu = cx.new(|_| {
            menu_bar_with(theme, vs_keymap(), move |cmd| {
                registry.lookup(cmd).is_some()
            })
        });
        // Document tabs saved in a layout have no editor behind them after a restart.
        controller.retain_documents(|id| id == WELCOME);
        let explorer = cx.new(|_| SolutionExplorer::new(theme));
        let error_list = cx.new(|cx| ErrorList::new(theme, cx));
        let references_window = cx.new(|_| ReferencesWindow::new(theme));
        let views: Rc<RefCell<HashMap<String, Entity<EditorView>>>> = Rc::default();
        let dock = cx.new(|cx| {
            DockHost::new(
                controller.clone(),
                commands.clone(),
                theme,
                Rc::new(tool_body(
                    explorer.clone(),
                    error_list.clone(),
                    references_window.clone(),
                )),
                Rc::new(document_body(views.clone())),
                persistence,
                cx,
            )
        });
        cx.observe(&dock, |_, _, cx| cx.notify()).detach();
        cx.observe(&explorer, |_, _, cx| cx.notify()).detach();
        cx.observe(&error_list, |_, _, cx| cx.notify()).detach();
        cx.observe(&references_window, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe_in(
            &references_window,
            window,
            |shell, _, event: &ReferencesEvent, window, cx| match event {
                ReferencesEvent::Navigate(ix) => shell.navigate_to_reference(*ix, window, cx),
            },
        )
        .detach();
        let mut status = StatusBar::vs_default();
        status.add_slot(SOLUTION_SLOT, SlotAlign::Left);
        status.add_slot(LANGUAGE_SERVER_SLOT, SlotAlign::Right);
        // The status bar reads the version through the command bus, like an agent would.
        let version = commands
            .invoke(builtins::ABOUT, json!({}))
            .ok()
            .and_then(|v| v["version"].as_str().map(str::to_owned))
            .unwrap_or_else(|| builtins::VERSION.to_owned());
        status.set(slots::VERSION, format!("Eludite {version}"));

        let Services {
            session,
            mut events,
            mut jobs,
            published,
        } = services;
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.next().await {
                if this
                    .update_in(cx, |shell, window, cx| {
                        shell.on_session_event(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let job_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                let UiJob { request, reply } = job;
                // An agent may name a file it opened a moment ago: wait until it is loaded.
                if let Ok(Some(loaded)) = this.update(cx, |shell, _| shell.wait_for_load(&request))
                {
                    let _ = loaded.await;
                }
                let mut outcome = this
                    .update_in(cx, |shell, window, cx| {
                        shell.apply(request.clone(), window, cx)
                    })
                    .unwrap_or_else(|_| Err(CommandError::Failed("the window is closed".into())));
                // An agent asking for IntelliSense gets the answer, not the request: wait for it (up to 5 s), checking
                // again whenever an editor changes.
                let mut deadline = cx.background_executor().timer(intellisense::AGENT_WAIT);
                while outcome.as_ref().is_ok_and(|o| o.is_loading()) {
                    let Ok(changed) = this.update(cx, |shell, _| shell.intellisense_waiter())
                    else {
                        break;
                    };
                    if let futures::future::Either::Right(_) =
                        futures::future::select(changed, &mut deadline).await
                    {
                        break;
                    }
                    match this.update(cx, |shell, cx| shell.intellisense_state(&request, cx)) {
                        Ok(Some(o)) => outcome = o,
                        _ => break,
                    }
                }
                let _ = reply.send(outcome);
            }
        });
        Self {
            theme,
            commands,
            controller,
            menu,
            dock,
            explorer,
            error_list,
            references_window,
            status,
            focus: cx.focus_handle(),
            on_first_render: None,
            session,
            published,
            languages: Arc::new(LanguageRegistry::with_builtins()),
            documents: HashMap::new(),
            views,
            loading: HashMap::new(),
            load_waiters: HashMap::new(),
            solution: None,
            generation: 0,
            diagnostics: BTreeMap::new(),
            host_diagnostics: Vec::new(),
            ls_state: None,
            solution_state: None,
            features: Default::default(),
            completion_timings: Vec::new(),
            intellisense_waiters: Vec::new(),
            navigation: Navigation::default(),
            references: References::default(),
            timings: Timings::default(),
            _tasks: vec![event_task, job_task],
        }
    }

    pub fn dock(&self) -> &Entity<DockHost> {
        &self.dock
    }

    pub fn timings(&self) -> &Timings {
        &self.timings
    }

    /// The active document tab's id (its path).
    pub fn active_document(&self) -> Option<String> {
        self.controller.active_document()
    }

    /// When the steps of each completion happened (oldest first, at most 4096).
    pub fn completion_timings(&self) -> &[intellisense::CompletionTiming] {
        &self.completion_timings
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn explorer(&self) -> &Entity<SolutionExplorer> {
        &self.explorer
    }

    pub fn error_list(&self) -> &Entity<ErrorList> {
        &self.error_list
    }

    pub fn references_window(&self) -> &Entity<ReferencesWindow> {
        &self.references_window
    }

    /// The Go To Definition picker, while it is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn definition_picker(&self) -> Option<&Entity<navigation::DefinitionPicker>> {
        self.navigation.picker.as_ref()
    }

    /// The editor of the open document at `path`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn editor(&self, path: &Path) -> Option<Entity<EditorView>> {
        let id = documents::normalize_path(path);
        self.views
            .borrow()
            .get(id.to_string_lossy().as_ref())
            .cloned()
    }

    #[cfg(test)]
    pub fn menu(&self) -> &Entity<MenuBar> {
        &self.menu
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn status(&self) -> &StatusBar {
        &self.status
    }

    pub fn set_probe(&mut self, probe: Option<Probe>, cx: &mut Context<Self>) {
        self.dock.update(cx, |d, _| d.set_probe(probe));
    }

    /// Run `f` once, after the first frame is presented.
    pub fn after_first_present(&mut self, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.on_first_render = Some(Box::new(f));
    }

    fn run_command(&mut self, action: &RunCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.run(&action.command, action.args.clone(), window, cx);
    }

    /// Invoke a command from the UI: the File > Open Project/Solution dialog and the unsaved-changes question come
    /// first, then the bus. The result goes to the status bar.
    pub fn run(&mut self, command: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        if command == workspace::SOLUTION_OPEN && args.get("path").is_none() {
            self.prompt_open_solution(window, cx);
            return;
        }
        if command == workspace::FILE_CLOSE
            && args.get("save").is_none()
            && let Some(path) = args.get("path").and_then(Value::as_str)
            && self.documents.get(path).is_some_and(|d| d.dirty)
        {
            self.prompt_close(path.to_owned(), window, cx);
            return;
        }
        let result = self.invoke(command, args, window, cx);
        let text = match (&result, command) {
            (Ok(v), builtins::ABOUT) => format!(
                "{} {}",
                v["name"].as_str().unwrap_or("Eludite"),
                v["version"].as_str().unwrap_or_default()
            ),
            (Ok(_), _) => "Ready".to_owned(),
            (Err(e), _) => {
                eprintln!("eludite: {command}: {e}");
                e.to_string()
            }
        };
        self.status.set(slots::STATE, text);
        cx.notify();
    }

    /// Invoke `command` on the bus from the UI thread. File and editor commands are applied here first and their
    /// result staged for the bus handler (see `target`).
    pub fn invoke(
        &mut self,
        command: &str,
        args: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Value, CommandError> {
        let ui_bound = workspace::ALL.contains(&command)
            && command != workspace::SOLUTION_OPEN
            && command != workspace::SOLUTION_CLOSE;
        if ui_bound && let Ok(request) = workspace::parse(command, args.clone()) {
            let outcome = self.apply(request, window, cx);
            target::stage(outcome);
        }
        if command == workspace::SOLUTION_OPEN {
            self.timings = Timings {
                open: Some(Instant::now()),
                ..Timings::default()
            };
        }
        self.commands.invoke(command, args)
    }

    fn prompt_open_solution(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let chosen = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) | Err(_) => None,
                Ok(Err(e)) => {
                    let _ = this.update(cx, |shell, cx| {
                        shell
                            .status
                            .set(slots::STATE, format!("No file dialog: {e}"));
                        cx.notify();
                    });
                    None
                }
            };
            if let Some(path) = chosen {
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.run(
                        workspace::SOLUTION_OPEN,
                        json!({ "path": path.to_string_lossy() }),
                        window,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    fn prompt_close(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Save changes to {name}?"),
            None,
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
                    workspace::FILE_CLOSE,
                    json!({ "path": path, "save": save }),
                    window,
                    cx,
                )
            });
        })
        .detach();
    }

    /// Apply a file or editor command (the UI-thread half of `target::ShellTarget`).
    fn apply(
        &mut self,
        request: WorkspaceRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<workspace::WorkspaceOutput, CommandError> {
        match request {
            WorkspaceRequest::FileOpen { path, line, column } => {
                self.open_file(&path, line.map(|l| (l, column.unwrap_or(1))), window, cx)
            }
            WorkspaceRequest::FileClose { path, save } => self.close_file(&path, save, cx),
            WorkspaceRequest::Save { path } => self.save(path.as_deref(), cx),
            WorkspaceRequest::Undo { path } => self.history(path.as_deref(), true, cx),
            WorkspaceRequest::Redo { path } => self.history(path.as_deref(), false, cx),
            WorkspaceRequest::Find {
                path,
                query,
                case_sensitive,
            } => self.find(path.as_deref(), query, case_sensitive, window, cx),
            WorkspaceRequest::Complete {
                path,
                line,
                column,
                trigger,
            } => self.complete_command(
                path.as_deref(),
                line.map(|l| (l, column.unwrap_or(1))),
                trigger,
                cx,
            ),
            WorkspaceRequest::AcceptCompletion { path, label } => {
                self.accept_completion_command(path.as_deref(), label.as_deref(), cx)
            }
            WorkspaceRequest::Hover { path, line, column } => {
                self.hover_command(path.as_deref(), line.map(|l| (l, column.unwrap_or(1))), cx)
            }
            WorkspaceRequest::SignatureHelp {
                path,
                line,
                column,
                trigger,
            } => self.signature_help_command(
                path.as_deref(),
                line.map(|l| (l, column.unwrap_or(1))),
                trigger,
                cx,
            ),
            WorkspaceRequest::GoToDefinition {
                path,
                line,
                column,
                target,
            } => self.go_to_definition_command(
                path.as_deref(),
                line.map(|l| (l, column.unwrap_or(1))),
                target,
                window,
                cx,
            ),
            WorkspaceRequest::FindReferences { path, line, column } => self
                .find_references_command(
                    path.as_deref(),
                    line.map(|l| (l, column.unwrap_or(1))),
                    window,
                    cx,
                ),
            WorkspaceRequest::NavigateBack => self.navigate_history(true, window, cx),
            WorkspaceRequest::NavigateForward => self.navigate_history(false, window, cx),
            WorkspaceRequest::ErrorListFilter(input) => Ok(
                workspace::WorkspaceOutput::ErrorListFilter(self.error_list.update(cx, |e, cx| {
                    e.set_filter(&input, cx);
                    e.filter_output()
                })),
            ),
            WorkspaceRequest::SolutionOpen { .. } | WorkspaceRequest::SolutionClose => Err(
                CommandError::Failed("solution commands are not applied on the UI thread".into()),
            ),
        }
    }

    fn solution_name(&self) -> String {
        self.solution
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn on_session_event(
        &mut self,
        event: SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionEvent::Opening { path } => {
                if self.solution.as_ref() != Some(&path) {
                    self.diagnostics.clear();
                    self.host_diagnostics.clear();
                    for doc in self.documents.values() {
                        doc.clear_diagnostics(cx);
                    }
                }
                if self.timings.open.is_none() {
                    self.timings.open = Some(Instant::now());
                }
                self.solution = Some(path.clone());
                let name = self.solution_name();
                window.set_window_title(&format!(
                    "{} - Eludite",
                    path.file_stem()
                        .map_or("Solution".into(), |s| s.to_string_lossy())
                ));
                self.explorer.update(cx, |e, cx| {
                    e.set_placeholder(Placeholder::Loading(name.clone()), cx)
                });
                self.status
                    .set(SOLUTION_SLOT, format!("Opening {name}\u{2026}"));
            }
            SessionEvent::HostStarted { version } => {
                self.status
                    .set(LANGUAGE_SERVER_SLOT, format!("eludite-host {version}"));
            }
            SessionEvent::HostFailed { reason } => {
                self.status.set(SOLUTION_SLOT, reason.clone());
                self.explorer.update(cx, |e, cx| {
                    e.set_placeholder(Placeholder::Failed(reason), cx)
                });
            }
            SessionEvent::HostRestarting => {
                self.status.set(
                    LANGUAGE_SERVER_SLOT,
                    "eludite-host exited; restarting\u{2026}",
                );
            }
            SessionEvent::LanguageServer(s) => {
                self.ls_state = Some(s.state);
                if let Some(caps) = &s.capabilities {
                    self.features = intellisense::ServerFeatures::from_capabilities(caps);
                }
                let text = match s.state {
                    LanguageServerState::Starting => "C#: starting\u{2026}".to_owned(),
                    LanguageServerState::Running => format!(
                        "C#: running{}",
                        s.server_info
                            .and_then(|i| i.version)
                            .map(|v| format!(" ({v})"))
                            .unwrap_or_default()
                    ),
                    LanguageServerState::Restarting => "C#: restarting\u{2026}".to_owned(),
                    LanguageServerState::Unavailable => "C#: unavailable".to_owned(),
                    LanguageServerState::Exited => format!(
                        "C#: exited{}",
                        s.message.map(|m| format!(" ({m})")).unwrap_or_default()
                    ),
                };
                self.status.set(LANGUAGE_SERVER_SLOT, text);
            }
            SessionEvent::Solution(status) => {
                if status.generation < self.generation {
                    return;
                }
                if status.generation > self.generation {
                    // A new generation: everything computed under the old one is stale (CLAUDE.md invariant 12).
                    self.generation = status.generation;
                    self.diagnostics.clear();
                    for doc in self.documents.values_mut() {
                        doc.clear_diagnostics(cx);
                        doc.intellisense.cancel_all();
                    }
                    self.navigation.cancel();
                    if self.references.cancel() {
                        // Never show results computed for the old solution.
                        self.references_window
                            .update(cx, |w, cx| w.fail(navigation::OUTDATED.to_owned(), cx));
                    }
                }
                let was_loaded = self.solution_state == Some(SolutionState::Loaded);
                self.solution_state =
                    (status.state != SolutionState::Closed).then_some(status.state);
                if status.state == SolutionState::Loaded && !was_loaded {
                    self.refresh_fallback_lists(cx);
                }
                let name = Path::new(&status.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let text = match status.state {
                    SolutionState::Loading => match status.phase {
                        Some(LoadPhase::LegacyEvaluation) => {
                            format!("{name}: evaluating legacy projects\u{2026}")
                        }
                        _ => format!("{name}: loading projects\u{2026}"),
                    },
                    SolutionState::Loaded => {
                        self.timings.loaded.get_or_insert_with(Instant::now);
                        let n = status.counts.map_or(0, |c| c.projects);
                        format!(
                            "{name}: {n} project{} loaded{}",
                            if n == 1 { "" } else { "s" },
                            status
                                .elapsed_ms
                                .map(|ms| format!(" in {:.1} s", ms / 1000.))
                                .unwrap_or_default()
                        )
                    }
                    SolutionState::Failed => format!(
                        "{name} did not load: {}",
                        status
                            .diagnostics
                            .first()
                            .map_or("see the Error List", |d| d.message.as_str())
                    ),
                    SolutionState::Closed => String::new(),
                };
                documents::trace(format_args!(
                    "solution {:?} generation {}: {text}",
                    status.state, status.generation
                ));
                self.host_diagnostics = status.diagnostics.clone();
                self.status.set(SOLUTION_SLOT, text);
                self.update_error_list(cx);
            }
            SessionEvent::Tree(tree) => {
                if tree.generation < self.generation {
                    return;
                }
                self.timings.tree.get_or_insert_with(Instant::now);
                documents::trace(format_args!(
                    "tree generation {}: {} projects",
                    tree.generation,
                    tree.projects.len()
                ));
                match SolutionModel::from_tree(&tree) {
                    Some(model) => {
                        // Show where the active document is (Visual Studio's Track Active Item).
                        let active = self
                            .controller
                            .active_document()
                            .filter(|id| self.documents.contains_key(id));
                        self.explorer.update(cx, |e, cx| {
                            e.set_model(model, cx);
                            if let Some(id) = active {
                                e.reveal(Path::new(&id), cx);
                            }
                        })
                    }
                    None => self.explorer.update(cx, |e, cx| e.clear(cx)),
                }
                self.update_error_list(cx);
            }
            SessionEvent::Diagnostics(params) => self.on_diagnostics(params, cx),
            SessionEvent::ApplyEdit {
                id,
                generation,
                params,
            } => self.on_host_apply_edit(id, generation, params, window, cx),
            SessionEvent::Closed => {
                self.solution = None;
                self.solution_state = None;
                self.diagnostics.clear();
                self.host_diagnostics.clear();
                for doc in self.documents.values() {
                    doc.clear_diagnostics(cx);
                }
                self.explorer.update(cx, |e, cx| e.clear(cx));
                self.status.set(SOLUTION_SLOT, "");
                window.set_window_title("Eludite");
                self.update_error_list(cx);
            }
        }
        cx.notify();
    }

    fn on_diagnostics(&mut self, params: lsp::PublishDiagnosticsParams, cx: &mut Context<Self>) {
        self.timings.diagnostics_events += 1;
        documents::trace(format_args!(
            "publishDiagnostics {} version {:?}: {} ({} errors)",
            params.uri,
            params.version,
            params.diagnostics.len(),
            params
                .diagnostics
                .iter()
                .filter(|d| d.severity.is_none_or(|s| s == 1))
                .count()
        ));
        // Match by URI, not by a path string: the document id is whatever path string the tree or
        // dialog produced (on Windows possibly with mixed separators), while every document and the
        // host agree on the URI form (the Windows CI failure after brief 0012).
        if let Some(doc) = self.documents.values().find(|d| d.uri == params.uri) {
            // A result for an older version than the editor last sent is stale; a newer one is coming.
            if params.version.is_some_and(|v| v < doc.lsp_version) {
                return;
            }
            doc.show_diagnostics(&params.diagnostics, cx);
            self.timings
                .first_diagnostics
                .get_or_insert_with(Instant::now);
            if !params.diagnostics.is_empty() {
                self.timings
                    .first_nonempty_diagnostics
                    .get_or_insert_with(Instant::now);
            }
        }
        if params.diagnostics.is_empty() {
            self.diagnostics.remove(&params.uri);
        } else {
            self.diagnostics.insert(params.uri, params.diagnostics);
        }
        self.update_error_list(cx);
    }

    /// Rebuild the Error List rows and what `diagnostics.list` returns.
    fn update_error_list(&mut self, cx: &mut Context<Self>) {
        let model = self.explorer.read(cx).model().cloned();
        let root = self
            .solution
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let mut rows = Vec::new();
        for (uri, diags) in &self.diagnostics {
            let Some(path) = uri_to_path(uri) else {
                continue;
            };
            let project = model
                .as_ref()
                .and_then(|m| m.project_of(&path))
                .map(str::to_owned);
            for d in diags {
                let severity = match d.severity {
                    Some(2) => Severity::Warning,
                    Some(3) => Severity::Message,
                    // Hints are not listed (Visual Studio hides them too).
                    Some(4) => continue,
                    _ => Severity::Error,
                };
                rows.push(ErrorRow {
                    severity,
                    code: d.code.as_ref().map(code_text).unwrap_or_default(),
                    message: d.message.clone(),
                    project: project.clone(),
                    file: file_name(&path),
                    path: path.clone(),
                    line: d.range.start.line + 1,
                    column: d.range.start.character + 1,
                });
            }
        }
        for d in &self.host_diagnostics {
            let path = d
                .project
                .as_deref()
                .map(PathBuf::from)
                .or_else(|| self.solution.clone())
                .unwrap_or_default();
            rows.push(ErrorRow {
                severity: match d.severity {
                    HostDiagnosticSeverity::Error => Severity::Error,
                    HostDiagnosticSeverity::Warning => Severity::Warning,
                    HostDiagnosticSeverity::Info => Severity::Message,
                },
                code: d.code.clone(),
                message: d.message.clone(),
                project: d.project.as_deref().map(|p| {
                    Path::new(p)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                }),
                file: file_name(&path),
                path,
                line: 1,
                column: 1,
            });
        }
        let rank = |s: Severity| match s {
            Severity::Error => 0,
            Severity::Warning => 1,
            Severity::Message => 2,
        };
        rows.sort_by(|a, b| {
            rank(a.severity)
                .cmp(&rank(b.severity))
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| (a.line, a.column).cmp(&(b.line, b.column)))
        });
        let listed: Vec<ListedDiagnostic> = rows
            .iter()
            .map(|r| ListedDiagnostic {
                path: relative_path(&r.path, root.as_deref()),
                line: r.line,
                column: r.column,
                severity: r.severity,
                code: r.code.clone(),
                message: r.message.clone(),
                project: r.project.clone(),
            })
            .collect();
        *self.published.lock().unwrap_or_else(|e| e.into_inner()) = listed;
        self.error_list.update(cx, |e, cx| e.set_rows(rows, cx));
    }
}

fn code_text(code: &Value) -> String {
    match code {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `path` relative to `root` with `/` separators (the `diagnostics.list` schema), else the whole path with `/`.
fn relative_path(path: &Path, root: Option<&Path>) -> String {
    let rel = root.and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(f) = self.on_first_render.take() {
            // Deferred effects run after this frame is presented.
            let handle = window.window_handle();
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| f(window, cx));
            });
        }
        let t = self.theme;
        div()
            .id("shell")
            .key_context(SHELL_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::run_command))
            .flex()
            .flex_col()
            .size_full()
            .bg(t.chrome)
            .text_color(t.text)
            .child(self.menu.clone())
            .child(self.dock.clone())
            .child(self.status.render(&t))
            .children(self.navigation.picker.clone())
    }
}

#[cfg(test)]
mod path_tests {
    use super::relative_path;
    use std::path::Path;

    #[test]
    fn relative_paths_use_forward_slashes() {
        assert_eq!(
            relative_path(Path::new("/s/src/Host/Rpc/A.cs"), Some(Path::new("/s"))),
            "src/Host/Rpc/A.cs"
        );
        assert_eq!(
            relative_path(Path::new("/elsewhere/A.cs"), Some(Path::new("/s"))),
            "/elsewhere/A.cs"
        );
        assert_eq!(relative_path(Path::new("/a/B.cs"), None), "/a/B.cs");
    }
}
