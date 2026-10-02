//! The root view: menu bar, docking area and status bar, plus the workspace: the host session, Workspace,
//! the open documents and the Error List (brief 0012), IntelliSense (brief 0013), and navigation: Go To Definition,
//! the navigation history, Find All References and the Error List's filters (brief 0014), and rename, code actions
//! and the workspace-edit applier (brief 0015), the Agents window (brief 0016), builds with the Output window and
//! the build's rows in the Error List (brief 0017), run and debug (brief 0018), and File > Open Folder
//! with the Cargo workspace and generic language servers beside the host (brief 0019, `folder` and `servers`).
//!
//! Keys and menu items both produce [`RunCommand`]; this view's action handler is the one place the UI turns that
//! into a command-bus invocation. The file and editor commands are applied here, on the UI thread, whoever invokes
//! them (see `target`).

pub mod agents;
pub mod build;
#[cfg(test)]
mod build_tests;
pub mod cargo_build;
pub mod code_actions;
pub mod debug;
pub mod documents;
pub mod error_list;
pub mod explorer;
pub mod folder;
pub mod intellisense;
#[cfg(test)]
mod intellisense_tests;
pub mod navigation;
#[cfg(test)]
mod navigation_tests;
pub mod options;
pub mod output;
#[cfg(test)]
mod refactor_tests;
pub mod references;
pub mod rename;
#[cfg(test)]
mod rust_tests;
pub mod servers;
pub mod session;
pub mod settings;
#[cfg(test)]
mod settings_tests;
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

use eludite_commands::diagnostics::{self, Diagnostic as ListedDiagnostic, RowSource, Severity};
use eludite_commands::workspace::{self, WorkspaceRequest};
use eludite_commands::{CommandError, CommandRegistry, builtins};
use eludite_docking::{DockController, DockHost, DocumentTab, Persistence, Probe, ids};
use eludite_editor::EditorView;
use eludite_editor::syntax::LanguageRegistry;
use eludite_lsp::host::{
    BuildDiagnosticSeverity, HostDiagnostic, HostDiagnosticSeverity, LanguageServerState,
    LoadPhase, SolutionState,
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

use self::build::{BuildBus, BuildJob, BuildShared, Builds};
use self::documents::{Document, uri_to_path};
use self::error_list::{ErrorList, ErrorRow};
use self::explorer::{Placeholder, SolutionExplorer};
use self::navigation::Navigation;
use self::output::OutputWindow;
use self::references::{References, ReferencesEvent, ReferencesWindow};
use self::servers::{GenericServer, ServerKey, ServerLaunches};
use self::session::{HostLaunch, ServerSession, SessionEvent};
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
    pub session: ServerSession,
    pub events: UnboundedReceiver<SessionEvent>,
    pub jobs: UnboundedReceiver<UiJob>,
    /// The Error List rows `diagnostics.list` reads, on any thread.
    pub published: Arc<Mutex<Vec<ListedDiagnostic>>>,
    /// Where the Agents window's agents come from.
    pub agents: agents::AgentsSetup,
    /// `eludite.agents.*` from other threads, for the UI thread to apply.
    pub agent_jobs: UnboundedReceiver<agents::AgentsJob>,
    /// What `eludite.solution.tree` reads, on any thread.
    pub tree: Arc<Mutex<eludite_commands::solution::SolutionTreeOutput>>,
    /// `eludite.build.*` and `eludite.output.*` from other threads, for the UI thread to apply.
    pub build_jobs: UnboundedReceiver<BuildJob>,
    /// Where waiting build commands read finished builds.
    pub build_shared: Arc<BuildShared>,
    /// `eludite.debug.*` from other threads, for the UI thread to apply (brief 0018).
    pub debug_jobs: UnboundedReceiver<debug::DebugJob>,
    /// How debugging sessions reach their adapter.
    pub debug: debug::DebugSetup,
    /// What `eludite.workspace.tree` reads, on any thread (brief 0019).
    pub workspace_tree: Arc<Mutex<eludite_commands::workspace_tree::WorkspaceTreeOutput>>,
    /// The language-server registrations and, in tests, servers in this process (brief 0019).
    pub launches: ServerLaunches,
    /// The settings store (brief 0020) and when it changed.
    pub settings: crate::settings::Settings,
    pub settings_changed: UnboundedReceiver<Instant>,
    /// `eludite.tools.options` from other threads.
    pub options_jobs: UnboundedReceiver<self::settings::OptionsJob>,
}

/// Start the host session and the settings store, and register the workspace, settings and other shell commands on
/// `commands`. Call on the UI thread.
pub fn register_workspace(
    commands: &mut CommandRegistry,
    launch: HostLaunch,
    settings: crate::settings::SettingsSetup,
) -> Services {
    let (session, events) = ServerSession::spawn(launch);
    let schema = Arc::new(eludite_commands::settings::SettingsSchema::builtin());
    let (settings_tx, settings_changed) = unbounded();
    let settings = crate::settings::Settings::start(schema.clone(), settings, settings_tx);
    let (options_tx, options_jobs) = unbounded();
    eludite_commands::settings::register(
        commands,
        schema,
        Arc::new(self::settings::SettingsBus {
            settings: settings.clone(),
            ui_thread: std::thread::current().id(),
            jobs: options_tx,
        }),
        true,
    );
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
    let tree: Arc<Mutex<eludite_commands::solution::SolutionTreeOutput>> = Arc::default();
    let tree_source = tree.clone();
    eludite_commands::solution::register(
        commands,
        Arc::new(move || {
            tree_source
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }),
    )
    .expect("eludite.solution.tree registers once");
    let workspace_tree: Arc<Mutex<eludite_commands::workspace_tree::WorkspaceTreeOutput>> =
        Arc::default();
    let workspace_tree_source = workspace_tree.clone();
    eludite_commands::workspace_tree::register(
        commands,
        Arc::new(move || {
            workspace_tree_source
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }),
    )
    .expect("eludite.workspace.tree registers once");
    let (agent_jobs_tx, agent_jobs) = unbounded();
    eludite_commands::agents::register(
        commands,
        Arc::new(agents::AgentsBus {
            ui_thread: std::thread::current().id(),
            jobs: agent_jobs_tx,
        }),
    );
    let (build_jobs_tx, build_jobs) = unbounded();
    let build_shared = Arc::new(BuildShared::default());
    eludite_commands::build::register(
        commands,
        Arc::new(BuildBus {
            ui_thread: std::thread::current().id(),
            jobs: build_jobs_tx,
            shared: build_shared.clone(),
        }),
    );
    let debug_jobs = debug::register(commands);
    Services {
        session,
        events,
        jobs,
        published,
        agents: agents::AgentsSetup::from_env(),
        agent_jobs,
        tree,
        build_jobs,
        build_shared,
        debug_jobs,
        debug: debug::DebugSetup::from_env(),
        workspace_tree,
        launches: ServerLaunches::default(),
        settings,
        settings_changed,
        options_jobs,
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
    /// The Output window (brief 0017).
    output: Entity<OutputWindow>,
    status: StatusBar,
    focus: FocusHandle,
    on_first_render: Option<AfterPresent>,
    session: ServerSession,
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
    /// Rename and its dialog (brief 0015).
    rename: rename::Rename,
    /// The light bulb and its menu (brief 0015).
    code_actions: code_actions::CodeActions,
    /// The last `eludite.workspace.apply_edit` (state, summary).
    apply_edit: Option<(workspace::ApplyEditState, workspace_edit::ApplySummary)>,
    /// The Agents window and its sessions (brief 0016).
    agents: agents::Agents,
    /// An agent's edit command is running: the applier's next edit is held as pending changes for review.
    capture_next: Option<eludite_commands::Caller>,
    /// What `eludite.solution.tree` returns.
    tree: Arc<Mutex<eludite_commands::solution::SolutionTreeOutput>>,
    /// The build (brief 0017).
    builds: Builds,
    /// Run and debug (brief 0018).
    debug: debug::Debugger,
    /// Where the shell's own Cargo builds report, as the host reports MSBuild's (brief 0019).
    build_events: futures::channel::mpsc::UnboundedSender<SessionEvent>,
    /// Language-server registrations and how to launch them (brief 0019).
    launches: ServerLaunches,
    /// Generic language servers by `<registration id>|<root>` (brief 0019).
    generic: std::collections::BTreeMap<String, GenericServer>,
    /// File > Open Folder (brief 0019).
    folder: Option<folder::OpenFolder>,
    /// What `eludite.workspace.tree` returns.
    workspace_tree: Arc<Mutex<eludite_commands::workspace_tree::WorkspaceTreeOutput>>,
    /// The host's last tree, for `eludite.workspace.tree` without a folder.
    last_tree: Option<eludite_lsp::host::SolutionTree>,
    /// The settings store (brief 0020), what was last applied from it, and how long each change took to apply.
    settings: crate::settings::Settings,
    applied_settings: Option<self::settings::Applied>,
    settings_applied: Vec<std::time::Duration>,
    /// Tools > Options, while open.
    options: Option<Entity<options::OptionsDialog>>,
    timings: Timings,
    _tasks: Vec<Task<()>>,
}

fn tool_body(
    explorer: Entity<SolutionExplorer>,
    error_list: Entity<ErrorList>,
    references: Entity<ReferencesWindow>,
    agents: Entity<agents::window::AgentsWindow>,
    output: Entity<OutputWindow>,
    debug: debug::windows::DebugWindows,
) -> impl Fn(&str, &Theme) -> AnyElement {
    // Cached: they re-render when they change, not on every keystroke frame of the editor.
    move |id, _| match id {
        ids::WORKSPACE => explorer
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
        ids::AGENTS => agents
            .clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element(),
        ids::OUTPUT => output
            .clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element(),
        // The debugger's windows (brief 0018); titled empty panels for the rest until later briefs fill them.
        _ => debug.body(id).unwrap_or_else(|| div().into_any_element()),
    }
}

fn document_body(
    views: Rc<RefCell<HashMap<String, Entity<EditorView>>>>,
    reviews: agents::Reviews,
    gutters: agents::Gutters,
) -> impl Fn(&DocumentTab, &Theme) -> AnyElement {
    move |tab, theme| {
        if let Some(view) = views.borrow().get(&tab.id) {
            // An agent's pending change marks the lines it touches in the gutter (brief 0016).
            if let Some(marks) = gutters.borrow().get(&tab.id) {
                return div()
                    .relative()
                    .size_full()
                    .child(view.clone())
                    .child(marks.clone())
                    .into_any_element();
            }
            return view.clone().into_any_element();
        }
        if let Some(review) = reviews.borrow().get(&tab.id) {
            return review.clone().into_any_element();
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
        // The Build menu's start items are disabled while a build runs, Cancel only then (brief 0017).
        let building = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let menu_building = building.clone();
        let menu = cx.new(|_| {
            menu_bar_with(theme, vs_keymap(), move |cmd| {
                registry.lookup(cmd).is_some()
                    && build::menu_enabled(
                        cmd,
                        menu_building.load(std::sync::atomic::Ordering::SeqCst),
                    )
            })
        });
        // Document tabs saved in a layout have no editor behind them after a restart.
        controller.retain_documents(|id| id == WELCOME);
        let explorer = cx.new(|_| SolutionExplorer::new(theme));
        let error_list = cx.new(|cx| ErrorList::new(theme, cx));
        let references_window = cx.new(|_| ReferencesWindow::new(theme));
        let output = cx.new(|_| OutputWindow::new(theme));
        let views: Rc<RefCell<HashMap<String, Entity<EditorView>>>> = Rc::default();
        let Services {
            session,
            mut events,
            mut jobs,
            published,
            agents: agents_setup,
            mut agent_jobs,
            tree,
            mut build_jobs,
            build_shared,
            debug_jobs,
            debug: debug_setup,
            workspace_tree,
            launches,
            settings,
            mut settings_changed,
            mut options_jobs,
        } = services;
        let (agents, mut agent_msgs) = agents::Agents::new(agents_setup, theme, cx);
        let (debugger, debug_msgs) = debug::Debugger::new(debug_setup, theme, cx);
        let dock = cx.new(|cx| {
            DockHost::new(
                controller.clone(),
                commands.clone(),
                theme,
                Rc::new(tool_body(
                    explorer.clone(),
                    error_list.clone(),
                    references_window.clone(),
                    agents.window.clone(),
                    output.clone(),
                    debugger.windows.clone(),
                )),
                Rc::new(document_body(
                    views.clone(),
                    agents.reviews.clone(),
                    agents.gutters.clone(),
                )),
                persistence,
                cx,
            )
        });
        cx.observe(&dock, |_, _, cx| cx.notify()).detach();
        cx.observe(&explorer, |_, _, cx| cx.notify()).detach();
        cx.observe(&error_list, |_, _, cx| cx.notify()).detach();
        cx.observe(&references_window, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&output, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(
            &references_window,
            window,
            |shell, _, event: &ReferencesEvent, window, cx| match event {
                ReferencesEvent::Navigate(ix) => shell.navigate_to_reference(*ix, window, cx),
            },
        )
        .detach();
        cx.subscribe_in(&agents.window, window, Self::on_agents_window_event)
            .detach();
        for (vars, watch) in [
            (debugger.windows.locals.clone(), false),
            (debugger.windows.watch.clone(), true),
        ] {
            cx.subscribe(
                &vars,
                move |shell, _, e: &debug::windows::ToggleVariable, cx| {
                    shell.debug_toggle_variable(watch, &e.0, cx)
                },
            )
            .detach();
        }
        let mut status = StatusBar::vs_default();
        status.add_slot(SOLUTION_SLOT, SlotAlign::Left);
        status.add_slot(build::BUILD_SLOT, SlotAlign::Left);
        status.add_slot(debug::DEBUG_SLOT, SlotAlign::Left);
        status.add_slot(LANGUAGE_SERVER_SLOT, SlotAlign::Right);
        status.add_slot(agents::AGENTS_SLOT, SlotAlign::Right);
        // The status bar reads the version through the command bus, like an agent would.
        let version = commands
            .invoke(builtins::ABOUT, json!({}))
            .ok()
            .and_then(|v| v["version"].as_str().map(str::to_owned))
            .unwrap_or_else(|| builtins::VERSION.to_owned());
        status.set(slots::VERSION, format!("Eludite {version}"));

        // The shell's own Cargo builds report through the same handlers as the host's builds (brief 0019).
        let (build_events, mut cargo_events) = unbounded();
        let cargo_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = cargo_events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = cargo_events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        for event in batch {
                            shell.on_session_event(event, window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        // Everything queued is applied in one update, so a burst of build output costs one frame.
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        for event in batch {
                            shell.on_session_event(event, window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        // `eludite.build.*` and `eludite.output.*` from other threads (an agent).
        let build_job_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = build_jobs.next().await {
                let BuildJob { request, reply } = job;
                let outcome = this
                    .update_in(cx, |shell, window, cx| {
                        shell.apply_build(request, window, cx)
                    })
                    .unwrap_or_else(|_| {
                        (
                            Err(CommandError::Failed("the window is closed".into())),
                            None,
                        )
                    });
                let _ = reply.send(outcome);
            }
        });
        let job_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                let UiJob {
                    request,
                    reply,
                    caller,
                } = job;
                // An agent's edit through the applier is held for review (brief 0016).
                let _ = this.update(cx, |shell, _| {
                    shell.capture_next = shell.reviews_edit(&request, &caller).then_some(caller);
                });
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
                let _ = this.update(cx, |shell, _| shell.capture_next = None);
                let _ = reply.send(outcome);
            }
        });
        // `eludite.agents.*` from other threads (an outer agent).
        let agent_job_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = agent_jobs.next().await {
                let agents::AgentsJob { request, reply } = job;
                let outcome = this
                    .update_in(cx, |shell, window, cx| {
                        shell.apply_agents(request, window, cx)
                    })
                    .unwrap_or_else(|_| Err(CommandError::Failed("the window is closed".into())));
                let _ = reply.send(outcome);
            }
        });
        // Agent events arrive from the agents' threads; everything queued is applied as one batch, so a burst of
        // streamed chunks costs one frame.
        let agent_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = agent_msgs.next().await {
                let mut batch = vec![first];
                while let Ok(more) = agent_msgs.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        shell.on_agent_batch(batch, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let debug_task = Self::debug_tasks(debug_msgs, debug_jobs, window, cx);
        // Settings changes (a file edited on disk, or eludite.settings.set): applied in one update per burst.
        let settings_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = settings_changed.next().await {
                let mut seen = first;
                while let Ok(more) = settings_changed.try_recv() {
                    seen = seen.min(more);
                }
                if this
                    .update(cx, |shell, cx| shell.apply_settings(Some(seen), cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let options_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = options_jobs.next().await {
                let self::settings::OptionsJob { section, reply } = job;
                let outcome = this
                    .update_in(cx, |shell, window, cx| {
                        shell.open_options(section, window, cx)
                    })
                    .unwrap_or_else(|_| Err(CommandError::Failed("the window is closed".into())));
                let _ = reply.send(outcome);
            }
        });
        let mut this = Self {
            theme,
            commands,
            controller,
            menu,
            dock,
            explorer,
            error_list,
            references_window,
            output,
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
            rename: rename::Rename::default(),
            code_actions: code_actions::CodeActions::default(),
            apply_edit: None,
            agents,
            capture_next: None,
            tree,
            builds: Builds::new(building, build_shared),
            debug: debugger,
            build_events,
            launches,
            generic: Default::default(),
            folder: None,
            workspace_tree,
            last_tree: None,
            settings,
            applied_settings: None,
            settings_applied: Vec::new(),
            options: None,
            timings: Timings::default(),
            _tasks: vec![
                event_task,
                job_task,
                agent_task,
                agent_job_task,
                build_job_task,
                debug_task.0,
                debug_task.1,
                cargo_task,
                settings_task,
                options_task,
            ],
        };
        this.apply_settings(None, cx);
        this
    }

    pub fn dock(&self) -> &Entity<DockHost> {
        &self.dock
    }

    pub fn timings(&self) -> &Timings {
        &self.timings
    }

    /// The folder of the open solution.
    pub fn solution_dir(&self) -> Option<PathBuf> {
        self.solution
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
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
        // F5 is Start Debugging, and Continue while the debuggee is in break mode (Visual Studio's Debug.Start).
        if command == eludite_commands::debug::START
            && args.get("debug") != Some(&Value::Bool(false))
            && self.debug.model.mode == debug::state::Mode::Break
        {
            self.run(eludite_commands::debug::CONTINUE, json!({}), window, cx);
            return;
        }
        if command == workspace::SOLUTION_OPEN && args.get("path").is_none() {
            self.prompt_open_solution(window, cx);
            return;
        }
        if command == workspace::WORKSPACE_OPEN_FOLDER && args.get("path").is_none() {
            self.prompt_open_folder(window, cx);
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
        if eludite_commands::agents::ALL.contains(&command)
            && let Ok(request) = eludite_commands::agents::parse(command, args.clone())
        {
            let outcome = self.apply_agents(request, window, cx);
            agents::stage(outcome);
        }
        if eludite_commands::build::ALL.contains(&command)
            && let Ok(request) = eludite_commands::build::parse(command, args.clone())
        {
            let (outcome, _) = self.apply_build(request, window, cx);
            build::stage(outcome);
        }
        if eludite_commands::debug::ALL.contains(&command)
            && let Ok(request) = eludite_commands::debug::parse(command, args.clone())
        {
            let outcome = self
                .apply_debug(request, &eludite_commands::Caller::User, false, window, cx)
                .map(|(out, _)| out);
            debug::stage(outcome);
        }
        if command == eludite_commands::settings::OPTIONS {
            let schema = self.settings.lock().schema().clone();
            if let Ok(eludite_commands::settings::SettingsRequest::Options { section }) =
                eludite_commands::settings::parse(command, args.clone(), &schema)
            {
                let outcome = self.open_options(section, window, cx);
                self::settings::stage(outcome);
            }
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
            WorkspaceRequest::Save { path } => {
                let saved = self.save(path.as_deref(), cx);
                if let Ok(workspace::WorkspaceOutput::Save(out)) = &saved {
                    // Build on save (off by default), after this command's own bus call.
                    let path = out.path.clone();
                    cx.defer_in(window, move |shell, window, cx| {
                        shell.build_after_save(&path, window, cx)
                    });
                }
                saved
            }
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
            WorkspaceRequest::Rename {
                path,
                line,
                column,
                new_name,
                apply,
            } => self.rename_command(
                path.as_deref(),
                line.map(|l| (l, column.unwrap_or(1))),
                new_name,
                apply,
                window,
                cx,
            ),
            WorkspaceRequest::CodeActions { path, line, column } => self.code_actions_command(
                path.as_deref(),
                line.map(|l| (l, column.unwrap_or(1))),
                window,
                cx,
            ),
            WorkspaceRequest::ApplyCodeAction { index, title } => {
                self.apply_code_action_command(index, title.as_deref(), window, cx)
            }
            WorkspaceRequest::ApplyEdit { edit, label } => {
                self.apply_edit_command(edit, label, window, cx)
            }
            WorkspaceRequest::OpenFolder { path } => self.open_folder(&path, window, cx),
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
                    self.clear_host_diagnostics(cx);
                    self.host_diagnostics.clear();
                    self.builds.diagnostics.clear();
                    self.load_platforms(path.clone(), window, cx);
                }
                if self.timings.open.is_none() {
                    self.timings.open = Some(Instant::now());
                }
                self.solution = Some(path.clone());
                self.update_settings_dir();
                self.debug_solution_opened(&path, cx);
                let name = self.solution_name();
                // An open folder keeps its title and its tree, where the solution shows as loading.
                if self.folder.is_none() {
                    window.set_window_title(&format!(
                        "{} - Eludite",
                        path.file_stem()
                            .map_or("Solution".into(), |s| s.to_string_lossy())
                    ));
                    self.explorer.update(cx, |e, cx| {
                        e.set_placeholder(Placeholder::Loading(name.clone()), cx)
                    });
                }
                self.status
                    .set(SOLUTION_SLOT, format!("Opening {name}\u{2026}"));
            }
            SessionEvent::HostStarted { version } => {
                self.status
                    .set(LANGUAGE_SERVER_SLOT, format!("eludite-host {version}"));
            }
            SessionEvent::HostFailed { reason } => {
                // The host gave up restarting while its build waited for it.
                if self.builds.awaiting_status {
                    self.on_build_lost(&reason, window, cx);
                }
                self.status.set(SOLUTION_SLOT, reason.clone());
                if let Some(f) = self.folder.as_mut() {
                    if f.solution.is_some() {
                        f.solution_failed = Some(reason);
                        self.recompose(cx);
                    }
                } else {
                    self.explorer.update(cx, |e, cx| {
                        e.set_placeholder(Placeholder::Failed(reason), cx)
                    });
                }
            }
            SessionEvent::HostRestarting => {
                self.status.set(
                    LANGUAGE_SERVER_SLOT,
                    "eludite-host exited; restarting\u{2026}",
                );
                self.on_host_restarting(cx);
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
                    // Generic servers' documents and diagnostics are not the solution's.
                    self.generation = status.generation;
                    self.clear_host_diagnostics(cx);
                    self.builds.diagnostics.clear();
                    for doc in self
                        .documents
                        .values_mut()
                        .filter(|d| d.server == ServerKey::Host)
                    {
                        doc.intellisense.cancel_all();
                    }
                    self.navigation.cancel();
                    self.rename.cancel();
                    self.close_rename_dialog(window, cx);
                    self.code_actions.cancel();
                    self.close_code_action_menu(window, cx);
                    for doc in self.documents.values() {
                        doc.view.update(cx, |v, cx| v.set_lightbulb(None, cx));
                    }
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
                if status.state == SolutionState::Failed
                    && let Some(f) = self.folder.as_mut()
                    && f.tree.is_none()
                {
                    f.solution_failed = Some(
                        status
                            .diagnostics
                            .first()
                            .map_or("load failed".into(), |d| d.message.clone()),
                    );
                    self.recompose(cx);
                }
                self.update_error_list(cx);
            }
            SessionEvent::Tree(tree) => {
                if tree.generation < self.generation {
                    return;
                }
                self.timings.tree.get_or_insert_with(Instant::now);
                self.publish_tree(&tree);
                documents::trace(format_args!(
                    "tree generation {}: {} projects",
                    tree.generation,
                    tree.projects.len()
                ));
                if let Some(f) = self.folder.as_mut() {
                    // The solution's node of the open folder (brief 0019).
                    f.tree = Some(tree);
                    f.solution_failed = None;
                    self.recompose(cx);
                    cx.notify();
                    return;
                }
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
            } => self.on_server_apply_edit(ServerKey::Host, id, generation, params, window, cx),
            SessionEvent::BuildStarted { ticket, result } => {
                self.on_build_started(ticket, result, cx)
            }
            SessionEvent::BuildRefused { ticket, message } => {
                self.on_build_refused(ticket, message, window, cx)
            }
            SessionEvent::BuildOutput(o) => self.on_build_output(o.build_id, o.seq, &o.text, cx),
            SessionEvent::BuildStatus(status) => self.on_build_status(status, window, cx),
            SessionEvent::BuildProgress(p) => self.on_build_progress(p, cx),
            SessionEvent::BuildFinished { finished, received } => {
                self.on_build_finished(*finished, received, window, cx)
            }
            SessionEvent::HostLog(line) => self.output.update(cx, |o, cx| {
                o.append(
                    eludite_commands::build::OutputSource::Host,
                    &format!("{line}\n"),
                    cx,
                )
            }),
            SessionEvent::Closed => {
                *self.tree.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
                self.solution = None;
                self.update_settings_dir();
                self.solution_state = None;
                self.last_tree = None;
                self.clear_host_diagnostics(cx);
                self.builds.diagnostics.clear();
                self.host_diagnostics.clear();
                self.status.set(SOLUTION_SLOT, "");
                if let Some(f) = self.folder.as_mut() {
                    f.tree = None;
                    if f.solution.is_some() && self.session.shared().solution.is_none() {
                        f.solution = None;
                    }
                    self.recompose(cx);
                } else {
                    self.explorer.update(cx, |e, cx| e.clear(cx));
                    window.set_window_title("Eludite");
                    self.publish_workspace_tree();
                }
                self.update_error_list(cx);
            }
            // Generic servers' events arrive through their own sessions (`servers`).
            SessionEvent::Progress(_)
            | SessionEvent::ServerStatus(_)
            | SessionEvent::ServerGeneration(_) => {}
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

    /// What `eludite.solution.tree` returns from now on.
    fn publish_tree(&mut self, tree: &eludite_lsp::host::SolutionTree) {
        use eludite_commands::solution::{SolutionTreeOutput, TreeProject};
        let state = match self.solution_state {
            Some(SolutionState::Failed) => "failed",
            Some(SolutionState::Loaded) => "loaded",
            _ => "loading",
        };
        let out = SolutionTreeOutput {
            path: tree.path.clone(),
            state: if tree.path.is_some() { state } else { "none" }.into(),
            projects: tree
                .projects
                .iter()
                .map(|p| TreeProject {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    kind: match p.kind {
                        eludite_lsp::host::TreeProjectKind::Sdk => "sdk",
                        eludite_lsp::host::TreeProjectKind::Legacy => "legacy",
                    }
                    .into(),
                    target_frameworks: p.target_frameworks.clone(),
                    files: p.files.iter().map(|f| f.path.clone()).collect(),
                    error: p.error.clone(),
                })
                .collect(),
        };
        *self.tree.lock().unwrap_or_else(|e| e.into_inner()) = out;
        self.last_tree = Some(tree.clone());
        self.publish_workspace_tree();
    }

    /// Forget the diagnostics of the host's documents (a new solution generation); generic servers' stay.
    fn clear_host_diagnostics(&mut self, cx: &mut Context<Self>) {
        let host: Vec<String> = self
            .diagnostics
            .keys()
            .filter(|uri| self.uri_server(uri) == ServerKey::Host)
            .cloned()
            .collect();
        for uri in host {
            self.diagnostics.remove(&uri);
        }
        for doc in self
            .documents
            .values()
            .filter(|d| d.server == ServerKey::Host)
        {
            doc.clear_diagnostics(cx);
        }
    }

    /// Rebuild the Error List rows and what `diagnostics.list` returns: the live diagnostics (the language server's
    /// and the solution load's) and the last build's. A build diagnostic with the same file, position and code as a
    /// live one is shown once, as both (brief 0017); build diagnostics never replace live ones.
    fn update_error_list(&mut self, cx: &mut Context<Self>) {
        let model = self.explorer.read(cx).model().cloned();
        let root = self.workspace_root();
        let project_name = |p: &str| self.project_display(p);
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
                    source: RowSource::Live,
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
                project: d.project.as_deref().map(project_name),
                file: file_name(&path),
                path,
                line: 1,
                column: 1,
                source: RowSource::Live,
            });
        }
        // The last build's, deduplicated against the live rows by file, position and code.
        let key = |path: &Path, line: u32, column: u32, code: &str| {
            (
                documents::normalize_path(path),
                line,
                column,
                code.to_owned(),
            )
        };
        let mut live: HashMap<_, usize> = HashMap::new();
        for (i, r) in rows.iter().enumerate() {
            live.entry(key(&r.path, r.line, r.column, &r.code))
                .or_insert(i);
        }
        let mut seen = std::collections::HashSet::new();
        for d in &self.builds.diagnostics {
            let path = d
                .file
                .as_deref()
                .or(d.project.as_deref())
                .map(PathBuf::from)
                .or_else(|| self.solution.clone())
                .unwrap_or_default();
            let line = d.line.filter(|l| *l > 0).unwrap_or(1);
            let column = d.column.filter(|c| *c > 0).unwrap_or(1);
            let k = key(&path, line, column, &d.code);
            if let Some(&i) = live.get(&k) {
                rows[i].source = RowSource::Both;
                continue;
            }
            if !seen.insert(k) {
                continue;
            }
            rows.push(ErrorRow {
                severity: match d.severity {
                    BuildDiagnosticSeverity::Error => Severity::Error,
                    BuildDiagnosticSeverity::Warning => Severity::Warning,
                    BuildDiagnosticSeverity::Message => Severity::Message,
                },
                code: d.code.clone(),
                message: d.message.clone(),
                project: d.project.as_deref().map(project_name),
                file: file_name(&path),
                path,
                line,
                column,
                source: RowSource::Build,
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
                source: Some(r.source),
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
            // The solution configuration and platform dropdowns sit at the right of the menu bar's row, so the
            // docking area keeps its height.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .bg(t.menu_background)
                    .child(div().flex_1().min_w_0().child(self.menu.clone()))
                    .child(self.build_toolbar(cx)),
            )
            .child(self.dock.clone())
            .child(self.status.render(&t))
            .children(self.navigation.picker.clone())
            .children(self.rename.dialog.clone())
            .children(self.options.clone())
            .children(self.code_actions.menu.clone())
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
