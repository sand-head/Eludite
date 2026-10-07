//! The Workspace window (PLAN.md 4.2, brief 0012; Visual Studio's Solution Explorer, renamed so Cargo and npm
//! workspaces fit the same window later): the tree from `eludite-workspace`'s [`SolutionModel`], drawn as
//! virtualized rows. Click selects; the triangle or a double-click expands and collapses; double-clicking a file
//! opens it through `eludite.file.open`. The startup project is drawn bold (brief 0020), every one of the multiple
//! startup projects too (brief 0028).
//!
//! A right-click on a project (or a Cargo package) opens its context menu (brief 0020), in Visual Studio's order:
//! Build, Rebuild and Clean (`eludite.build.project` with the project and a target), Set as Startup Project
//! (`eludite.workspace.set_startup_project`, .NET projects and Cargo packages: brief 0029 debugs Cargo packages with
//! lldb-dap) and Open Containing Folder (`eludite.workspace.open_containing_folder`). Every item runs its command
//! through the bus.
//!
//! In a Git repository (brief 0040) each file carries Visual Studio's source control glyph and color from the
//! repository's status, a file's context menu has Compare with Unmodified, Undo Changes, Stage, Unstage and Blame,
//! and Ctrl+D on the selected file is Compare with Unmodified.
//!
//! A project's or folder's context menu ends with Open in Terminal (brief 0041), a terminal in its folder.
//!
//! A .NET project has Visual Studio's Dependencies node (brief 0048): Frameworks, Packages (each package with the ones it
//! brings in under it) and Projects. A package with a known vulnerability (NuGet Audit, or the sources' data once the
//! Manage NuGet Packages window or an agent asked) or a deprecation carries a yellow warning glyph. The project's
//! context menu has Manage NuGet Packages..., the solution's Manage NuGet Packages for Solution..., the Packages node's
//! Manage NuGet Packages..., and a package's Update and Remove.
//!
//! Brief 0062 gives the window Visual Studio's look: every row leads with an icon of Eludite's own set
//! ([`eludite_ui::icons`], chosen by [`row_icon`]: the node's kind, a folder's expanded state, a project's test
//! framework, web and load state, a file's type), the change glyphs stay small beside it, and the box
//! "Search Workspace (Ctrl+;)" under the title filters the tree as the person types (150 ms after the last key): the
//! rows whose name holds every word of the query (case-insensitive) and their ancestors, expanded, the matched
//! characters bold. Escape clears it, restores the expanded set from before the search and returns to the tree. The
//! search runs on the UI thread under [`SEARCH_INLINE_ROWS`] nodes, else on a background task whose stale results are
//! dropped. `eludite.workspace.search` ([`register_search`]) is the same search for agents and for Ctrl+;
//! ([`bind_keys`]).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use eludite_commands::workspace::{
    self, WORKSPACE_SEARCH, WorkspaceSearchInput, WorkspaceSearchOutput, WorkspaceSearchRow,
    WorkspaceSearchTarget,
};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_editor::{EditorStyle, TextInput, TextInputEvent};
use eludite_git::GlyphIndex;
use eludite_ui::{
    Icon, RunCommand, SHELL_CONTEXT, TREE_ROW_HEIGHT, Theme, TreeRowStyle, WORKSPACE_GIT_ITEMS,
    WORKSPACE_TERMINAL_ITEM, menu_row, tree_row_with_icon,
};
use eludite_workspace::cargo::TargetKind;
use eludite_workspace::explorer::{DependencyGroup, FileType, NodeKind, Row, SolutionModel};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{
    AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable as _, InteractiveElement,
    IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window,
    anchored, deferred, div, px, uniform_list,
};
use serde_json::{Value, json};

/// The project context menu's items: (selector suffix, label).
pub const CONTEXT_ITEMS: [(&str, &str); 6] = [
    ("build", "Build"),
    ("rebuild", "Rebuild"),
    ("clean", "Clean"),
    ("startup", "Set as Startup Project"),
    ("folder", "Open Containing Folder"),
    // Brief 0049: the project property pages (Alt+Enter).
    ("properties", "Properties"),
];

/// Debug selector of a context menu item (`build`, `rebuild`, `clean`, `startup`, `folder`).
pub fn context_item_selector(item: &str) -> String {
    format!("se-menu-{item}")
}

/// The command and arguments a context menu item runs for project file (or `Cargo.toml`) `path`, or, for the git
/// items ([`WORKSPACE_GIT_ITEMS`]), for the file at `path`.
pub fn context_command(item: &str, path: &Path) -> Option<(&'static str, Value)> {
    let p = path.to_string_lossy();
    // Open in Terminal: the terminal resolves a project file to its folder.
    if item == WORKSPACE_TERMINAL_ITEM.0 {
        return Some((WORKSPACE_TERMINAL_ITEM.2, json!({ "cwd": p })));
    }
    if let Some((_, _, command)) = WORKSPACE_GIT_ITEMS.iter().find(|(i, _, _)| *i == item) {
        let args = match item {
            "compare" | "blame" => json!({ "path": p }),
            _ => json!({ "paths": [p] }),
        };
        return Some((command, args));
    }
    Some(match item {
        "build" => (eludite_commands::build::PROJECT, json!({ "project": p })),
        "rebuild" => (
            eludite_commands::build::PROJECT,
            json!({ "project": p, "target": "rebuild" }),
        ),
        "clean" => (
            eludite_commands::build::PROJECT,
            json!({ "project": p, "target": "clean" }),
        ),
        "startup" => (
            eludite_commands::project::SET_STARTUP_PROJECT,
            json!({ "project": p }),
        ),
        "folder" => (
            eludite_commands::project::OPEN_CONTAINING_FOLDER,
            json!({ "path": p }),
        ),
        "properties" => (
            eludite_commands::project::properties::PROPERTIES,
            json!({ "project": p, "open": true }),
        ),
        _ => return None,
    })
}

/// The NuGet items of the context menus (brief 0048): (selector suffix, label).
pub const NUGET_PROJECT_ITEM: (&str, &str) = ("nuget", "Manage NuGet Packages...");
pub const NUGET_SOLUTION_ITEM: (&str, &str) =
    ("nuget_solution", "Manage NuGet Packages for Solution...");
pub const NUGET_UPDATE_ITEM: (&str, &str) = ("nuget_update", "Update");
pub const NUGET_REMOVE_ITEM: (&str, &str) = ("nuget_remove", "Remove");

/// The yellow warning glyph of a package with a known vulnerability or a deprecation.
pub const PACKAGE_WARNING: &str = "\u{26A0}";

/// What the explorer shows when there is no tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placeholder {
    NoSolution,
    Loading(String),
    Failed(String),
}

pub struct SolutionExplorer {
    theme: Theme,
    model: Option<SolutionModel>,
    expanded: HashSet<String>,
    selected: Option<String>,
    rows: Vec<Row>,
    placeholder: Placeholder,
    /// The startup projects' files, drawn bold (the first is the startup project; brief 0028 has several).
    startup: Vec<PathBuf>,
    /// The open context menu: the row it is for and where the pointer was.
    menu: Option<(usize, Point<Pixels>)>,
    /// The repository's root and its files' glyphs (brief 0040).
    git: Option<(PathBuf, Rc<GlyphIndex>)>,
    /// `id/version` (id lowercase) of packages the sources say are vulnerable or deprecated (brief 0048).
    package_warnings: HashSet<String>,
    /// Where the rows were drawn, while `--bounds-out` probes (brief 0048's Xvfb run clicks them).
    probe: Option<eludite_ui::BoundsMap>,
    focus: FocusHandle,
    /// The box "Search Workspace (Ctrl+;)" (brief 0062).
    input: Entity<TextInput>,
    /// The search the tree shows, while the box has a query.
    search: Option<SearchState>,
    /// The expanded set from before the search, restored when it ends.
    saved_expanded: Option<HashSet<String>>,
    /// Every node in tree order, which the search and `eludite.workspace.search` run over.
    index: Arc<RwLock<SearchIndex>>,
    /// Bumped by every search, so a background search's late result is dropped.
    search_seq: u64,
    /// The query of the last search asked for (shown or still running).
    requested: String,
    /// The debounce timer or the background search in flight.
    pending_search: Option<Task<()>>,
    /// Ids of .NET projects that reference a test framework (their icon has the flask).
    test_projects: HashSet<String>,
    _input_events: Subscription,
    /// `eludite.workspace.search`'s requests for the box ([`SolutionExplorer::link_search`]).
    _search_jobs: Option<Task<()>>,
}

/// The text of the search box while it is empty.
pub const SEARCH_PLACEHOLDER: &str = "Search Workspace (Ctrl+;)";

/// Debug selector of the search box, and of its clear button.
pub const SEARCH_BOX: &str = "se-search";
pub const SEARCH_CLEAR: &str = "se-search-clear";

/// How long the box waits after the last keystroke before it searches.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// Trees with fewer nodes are searched on the UI thread; larger ones on a background task.
pub const SEARCH_INLINE_ROWS: usize = 20_000;

/// The packages that make a .NET project a test project (Visual Studio's test icon), lowercase.
const TEST_PACKAGES: [&str; 9] = [
    "microsoft.net.test.sdk",
    "xunit",
    "xunit.v3",
    "mstest",
    "mstest.testframework",
    "nunit",
    "tunit",
    "microsoft.testing.platform",
    "xunit.core",
];

/// What the search shows.
#[derive(Debug, Clone)]
struct SearchState {
    hits: SearchHits,
    /// Each match's ranges by row id, for drawing.
    by_id: HashMap<String, Vec<Range<usize>>>,
    /// Its own expanded set: the ancestors of every match at first.
    expanded: HashSet<String>,
}

/// Every node of the tree in order with its parent: what the search runs over, on any thread.
#[derive(Debug, Default, Clone)]
pub struct SearchIndex {
    pub all: Arc<Vec<Row>>,
    pub parents: Arc<Vec<Option<usize>>>,
    /// The box's query (empty when it is clear).
    pub query: String,
}

impl SearchIndex {
    fn of(model: Option<&SolutionModel>) -> (Arc<Vec<Row>>, Arc<Vec<Option<usize>>>) {
        let Some(m) = model else {
            return Default::default();
        };
        fn ids(n: &eludite_workspace::explorer::Node, out: &mut HashSet<String>) {
            out.insert(n.id.clone());
            n.children.iter().for_each(|c| ids(c, out));
        }
        let mut every = HashSet::new();
        ids(&m.root, &mut every);
        let all = m.visible_rows(&every);
        let mut parents = Vec::with_capacity(all.len());
        let mut stack: Vec<usize> = Vec::new();
        for (i, r) in all.iter().enumerate() {
            stack.truncate(r.depth);
            parents.push(stack.last().copied());
            stack.push(i);
        }
        (Arc::new(all), Arc::new(parents))
    }
}

/// Which rows a query matches ([`search_rows`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchHits {
    /// Index into [`SearchIndex::all`] of each match, with the label's matched byte ranges.
    pub matched: Vec<(usize, Vec<Range<usize>>)>,
    /// Matches and their ancestors.
    pub kept: Vec<bool>,
    /// Nodes with a kept node below them.
    pub leads_to_match: Vec<bool>,
}

/// The query's words, lowercase.
fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// The byte ranges of `label` where each of `words` (lowercase) first occurs, case-insensitively, sorted and merged;
/// `None` unless every word occurs.
pub fn match_ranges(label: &str, words: &[String]) -> Option<Vec<Range<usize>>> {
    if words.is_empty() {
        return None;
    }
    // Most names are ASCII: compare bytes, no allocation.
    if label.is_ascii() && words.iter().all(|w| w.is_ascii()) {
        let bytes = label.as_bytes();
        let mut ranges: Vec<Range<usize>> = Vec::with_capacity(words.len());
        for w in words {
            let w = w.as_bytes();
            let at = bytes
                .windows(w.len())
                .position(|win| win.eq_ignore_ascii_case(w))?;
            ranges.push(at..at + w.len());
        }
        return Some(merge(ranges));
    }
    // The lowercase label, and for each of its bytes the byte of `label` its character came from.
    let mut lower = String::with_capacity(label.len());
    let mut from = Vec::with_capacity(label.len() + 1);
    for (i, c) in label.char_indices() {
        for l in c.to_lowercase() {
            let start = lower.len();
            lower.push(l);
            from.extend(std::iter::repeat_n(i, lower.len() - start));
        }
    }
    from.push(label.len());
    let mut ranges: Vec<Range<usize>> = Vec::with_capacity(words.len());
    for w in words {
        let at = lower.find(w.as_str())?;
        let end = from[at + w.len()];
        let end = if end <= from[at] { label.len() } else { end };
        ranges.push(from[at]..end);
    }
    Some(merge(ranges))
}

/// `ranges` sorted, the overlapping and touching ones merged.
fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// The rows of `all` (with `parents`) whose label holds every word of `query`, and their ancestors.
pub fn search_rows(all: &[Row], parents: &[Option<usize>], query: &str) -> SearchHits {
    let words = words(query);
    let mut hits = SearchHits {
        matched: Vec::new(),
        kept: vec![false; all.len()],
        leads_to_match: vec![false; all.len()],
    };
    if words.is_empty() {
        return hits;
    }
    for (i, row) in all.iter().enumerate() {
        let Some(ranges) = match_ranges(&row.label, &words) else {
            continue;
        };
        hits.matched.push((i, ranges));
        hits.kept[i] = true;
        let mut at = parents.get(i).copied().flatten();
        while let Some(p) = at {
            hits.leads_to_match[p] = true;
            if hits.kept[p] {
                break;
            }
            hits.kept[p] = true;
            at = parents[p];
        }
    }
    hits
}

/// The name `eludite.workspace.search` gives a row's kind.
pub fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::FolderRoot => "workspace",
        NodeKind::Solution => "solution",
        NodeKind::Project { .. } => "project",
        NodeKind::Folder => "folder",
        NodeKind::File { .. } => "file",
        NodeKind::CargoWorkspace => "cargo_workspace",
        NodeKind::CargoPackage { .. } => "cargo_package",
        NodeKind::CargoTargets => "cargo_targets",
        NodeKind::CargoTarget { .. } => "cargo_target",
        NodeKind::Dependencies => "dependencies",
        NodeKind::DependencyGroup { .. } => "dependency_group",
        NodeKind::Package { .. } => "package",
        NodeKind::Framework => "framework",
        NodeKind::ProjectReference => "project_reference",
    }
}

/// A request of `eludite.workspace.search` for the box, on the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchJob {
    pub query: Option<String>,
    pub focus: bool,
}

/// `eludite.workspace.search`'s target: it answers from the shared index on the invoking thread and hands the box's
/// change to the UI thread without waiting.
struct SearchBus {
    index: Arc<RwLock<SearchIndex>>,
    jobs: UnboundedSender<SearchJob>,
}

impl WorkspaceSearchTarget for SearchBus {
    fn search(&self, input: WorkspaceSearchInput) -> Result<WorkspaceSearchOutput, CommandError> {
        let (all, parents, current) = {
            let i = self.index.read().unwrap_or_else(|e| e.into_inner());
            (i.all.clone(), i.parents.clone(), i.query.clone())
        };
        let query = input.query.clone().unwrap_or(current);
        let hits = search_rows(&all, &parents, &query);
        let total = hits.matched.len();
        let rows = hits
            .matched
            .iter()
            .take(workspace::MAX_SEARCH_ROWS)
            .map(|(i, _)| {
                let r = &all[*i];
                WorkspaceSearchRow {
                    path: r.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    name: r.label.clone(),
                    kind: kind_name(&r.kind).into(),
                }
            })
            .collect::<Vec<_>>();
        if input.query.is_some() || input.focus {
            self.jobs
                .unbounded_send(SearchJob {
                    query: input.query,
                    focus: input.focus,
                })
                .map_err(|_| CommandError::Failed("the Workspace window is closed".into()))?;
        }
        Ok(WorkspaceSearchOutput {
            query,
            truncated: total > rows.len(),
            rows,
            total,
        })
    }
}

/// What [`register_search`] hands the Workspace window ([`SolutionExplorer::link_search`]).
pub struct SearchLink {
    index: Arc<RwLock<SearchIndex>>,
    jobs: UnboundedReceiver<SearchJob>,
}

/// Register `eludite.workspace.search` on `commands`; link the Workspace window to it with
/// [`SolutionExplorer::link_search`].
pub fn register_search(commands: &CommandRegistry) -> SearchLink {
    let index: Arc<RwLock<SearchIndex>> = Arc::default();
    let (jobs, rx) = unbounded();
    workspace::register_search(
        commands,
        Arc::new(SearchBus {
            index: index.clone(),
            jobs,
        }),
    )
    .expect("eludite.workspace.search registers once");
    SearchLink { index, jobs: rx }
}

/// Ctrl+; (Visual Studio's key for the Solution Explorer's search) from anywhere in the shell:
/// `eludite.workspace.search` with `focus`.
pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([KeyBinding::new(
        "ctrl-;",
        RunCommand::new(WORKSPACE_SEARCH, json!({ "focus": true })),
        Some(SHELL_CONTEXT),
    )]);
}

/// The icon a row leads with (brief 0062). `test_project`: a .NET project that references a test framework.
pub fn row_icon(row: &Row, test_project: bool) -> Icon {
    match &row.kind {
        NodeKind::Solution => Icon::Solution,
        NodeKind::FolderRoot => Icon::Workspace,
        NodeKind::CargoWorkspace => Icon::CargoWorkspace,
        NodeKind::CargoPackage { .. } => Icon::CargoPackage,
        NodeKind::Project { error: Some(_), .. } => Icon::ProjectUnavailable,
        NodeKind::Project { web: true, .. } => Icon::ProjectWeb,
        NodeKind::Project { .. } if test_project => Icon::ProjectCSharpTest,
        NodeKind::Project { .. } => Icon::ProjectCSharp,
        NodeKind::Folder if row.expanded => Icon::FolderOpen,
        NodeKind::Folder => Icon::Folder,
        NodeKind::Dependencies => Icon::Dependencies,
        NodeKind::DependencyGroup { group } => match group {
            DependencyGroup::Frameworks => Icon::Frameworks,
            DependencyGroup::Packages => Icon::Packages,
            DependencyGroup::Projects => Icon::ProjectReference,
        },
        NodeKind::Package {
            transitive: true, ..
        } => Icon::PackageTransitive,
        NodeKind::Package { .. } => Icon::Package,
        NodeKind::Framework => Icon::Framework,
        NodeKind::ProjectReference => Icon::ProjectCSharp,
        NodeKind::CargoTargets => Icon::CargoTargets,
        NodeKind::CargoTarget { kind } => match kind {
            TargetKind::Bin | TargetKind::CustomBuild => Icon::CargoTargetBin,
            TargetKind::Lib | TargetKind::ProcMacro => Icon::CargoTargetLib,
            TargetKind::Test => Icon::CargoTargetTest,
            TargetKind::Example => Icon::CargoTargetExample,
            TargetKind::Bench => Icon::CargoTargetBench,
        },
        NodeKind::File { .. } => file_icon(row.file_type().unwrap_or(FileType::Other)),
    }
}

/// The icon of a file type.
pub fn file_icon(t: FileType) -> Icon {
    match t {
        FileType::CSharp => Icon::FileCs,
        FileType::Rust => Icon::FileRs,
        FileType::Json => Icon::FileJson,
        FileType::Markdown => Icon::FileMd,
        FileType::Xml => Icon::FileXml,
        FileType::Solution => Icon::FileSln,
        FileType::Toml => Icon::FileToml,
        FileType::TypeScript => Icon::FileTs,
        FileType::JavaScript => Icon::FileJs,
        FileType::Html => Icon::FileHtml,
        FileType::Css => Icon::FileCss,
        FileType::Razor => Icon::FileRazor,
        FileType::Cshtml => Icon::FileCshtml,
        FileType::Aspx => Icon::FileAspx,
        FileType::Image => Icon::FileImage,
        FileType::Text => Icon::FileText,
        FileType::Shell => Icon::FileShell,
        FileType::Yaml => Icon::FileYaml,
        FileType::Lock => Icon::FileLock,
        FileType::Other => Icon::File,
    }
}

/// Ids of the model's .NET projects whose Dependencies node lists a test framework package.
fn test_projects(model: &SolutionModel) -> HashSet<String> {
    use eludite_workspace::explorer::Node;
    fn walk(n: &Node, out: &mut HashSet<String>) {
        if matches!(n.kind, NodeKind::Project { .. }) {
            let tests = n
                .children
                .iter()
                .filter(|c| c.kind == NodeKind::Dependencies)
                .flat_map(|d| &d.children)
                .flat_map(|g| &g.children)
                .any(|p| match &p.kind {
                    NodeKind::Package {
                        id,
                        transitive: false,
                        ..
                    } => TEST_PACKAGES.contains(&id.to_lowercase().as_str()),
                    _ => false,
                });
            if tests {
                out.insert(n.id.clone());
            }
            return;
        }
        n.children.iter().for_each(|c| walk(c, out));
    }
    let mut out = HashSet::new();
    walk(&model.root, &mut out);
    out
}

impl SolutionExplorer {
    pub fn set_probe(&mut self, probe: Option<eludite_ui::BoundsMap>) {
        self.probe = probe;
    }
}

/// Debug selector of the row for node `id` (tests and the real-input driver).
pub fn row_selector(id: &str) -> String {
    format!("se-{id}")
}

impl SolutionExplorer {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(false, cx);
            input.set_placeholder(SEARCH_PLACEHOLDER, cx);
            input.set_style(
                EditorStyle {
                    theme,
                    font_size: theme.typography.ui,
                    line_height: px(16.),
                    ..EditorStyle::default()
                },
                cx,
            );
            // After the input's own update: the window reads the box.
            input.on_escape(move |window, cx| {
                let weak = weak.clone();
                window.defer(cx, move |window, cx| {
                    let _ = weak.update(cx, |e, cx| e.escape(window, cx));
                });
            });
            input
        });
        let _input_events = cx.subscribe(&input, |this, _, event: &TextInputEvent, cx| {
            if *event == TextInputEvent::Changed {
                this.schedule_search(cx);
            }
        });
        Self {
            theme,
            model: None,
            expanded: HashSet::new(),
            selected: None,
            rows: Vec::new(),
            placeholder: Placeholder::NoSolution,
            startup: Vec::new(),
            menu: None,
            git: None,
            package_warnings: HashSet::new(),
            probe: None,
            focus: cx.focus_handle(),
            input,
            search: None,
            saved_expanded: None,
            index: Arc::default(),
            search_seq: 0,
            requested: String::new(),
            pending_search: None,
            test_projects: HashSet::new(),
            _input_events,
            _search_jobs: None,
        }
    }

    /// Serve `eludite.workspace.search` ([`register_search`]): its index is this window's, and its requests reach the
    /// box.
    pub fn link_search(&mut self, link: SearchLink, window: &mut Window, cx: &mut Context<Self>) {
        let SearchLink { index, mut jobs } = link;
        {
            let mine = self.index.read().unwrap_or_else(|e| e.into_inner()).clone();
            *index.write().unwrap_or_else(|e| e.into_inner()) = mine;
        }
        self.index = index;
        self._search_jobs = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                if this
                    .update_in(cx, |e, window, cx| e.apply_search_job(job, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// A request of `eludite.workspace.search`: the query typed into the box and searched at once, and Ctrl+;'s
    /// focus (the Workspace window shown first).
    pub fn apply_search_job(
        &mut self,
        job: SearchJob,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(query) = job.query {
            self.set_query(&query, cx);
        }
        if job.focus {
            window.dispatch_action(
                Box::new(RunCommand::new(
                    eludite_commands::view::SHOW,
                    json!({ "id": "workspace" }),
                )),
                cx,
            );
            self.input.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Type `query` into the box and search now (no debounce); empty ends the search.
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.input.read(cx).text() != query {
            self.input.update(cx, |i, cx| i.set_text(query, cx));
        }
        self.pending_search = None;
        self.run_search(query.to_owned(), cx);
    }

    /// The box's text.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn query(&self, cx: &gpui::App) -> String {
        self.input.read(cx).text()
    }

    /// The search box.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn search_input(&self) -> &Entity<TextInput> {
        &self.input
    }

    /// Whether the tree shows a search's rows.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn searching(&self) -> bool {
        self.search.is_some()
    }

    /// The matched byte ranges of row `id`'s label in the search the tree shows.
    pub fn search_matches(&self, id: &str) -> Option<&[Range<usize>]> {
        self.search.as_ref()?.by_id.get(id).map(Vec::as_slice)
    }

    /// The expanded set outside a search.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn expanded(&self) -> &HashSet<String> {
        &self.expanded
    }

    /// The box's text changed: search once typing pauses.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text();
        if query == self.requested {
            return;
        }
        self.pending_search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |e, cx| {
                let query = e.input.read(cx).text();
                e.run_search(query, cx);
            });
        }));
    }

    /// Search for `query` (on a background task for a large tree), or end the search when it is blank.
    fn run_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_seq += 1;
        self.requested = query.clone();
        self.index.write().unwrap_or_else(|e| e.into_inner()).query = query.clone();
        if query.trim().is_empty() {
            self.pending_search = None;
            self.end_search(cx);
            return;
        }
        let (all, parents) = {
            let i = self.index.read().unwrap_or_else(|e| e.into_inner());
            (i.all.clone(), i.parents.clone())
        };
        if all.len() < SEARCH_INLINE_ROWS {
            let hits = search_rows(&all, &parents, &query);
            self.show_hits(hits, cx);
            return;
        }
        let seq = self.search_seq;
        self.pending_search = Some(cx.spawn(async move |this, cx| {
            let rows = all.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { search_rows(&rows, &parents, &query) })
                .await;
            let _ = this.update(cx, |e, cx| {
                // Principle 12: a later keystroke or a new tree makes this result stale.
                let current =
                    Arc::ptr_eq(&e.index.read().unwrap_or_else(|e| e.into_inner()).all, &all);
                if e.search_seq == seq && current {
                    e.show_hits(hits, cx);
                }
            });
        }));
    }

    fn show_hits(&mut self, hits: SearchHits, cx: &mut Context<Self>) {
        if self.saved_expanded.is_none() {
            self.saved_expanded = Some(self.expanded.clone());
        }
        let (expanded, by_id) = {
            let index = self.index.read().unwrap_or_else(|e| e.into_inner());
            let expanded = index
                .all
                .iter()
                .zip(&hits.leads_to_match)
                .filter(|(_, lead)| **lead)
                .map(|(r, _)| r.id.clone())
                .collect();
            let by_id = hits
                .matched
                .iter()
                .filter_map(|(i, r)| Some((index.all.get(*i)?.id.clone(), r.clone())))
                .collect();
            (expanded, by_id)
        };
        self.search = Some(SearchState {
            hits,
            by_id,
            expanded,
        });
        self.refresh_rows();
        cx.notify();
    }

    /// Back to the tree as it was before the search.
    fn end_search(&mut self, cx: &mut Context<Self>) {
        if self.search.take().is_some()
            && let Some(saved) = self.saved_expanded.take()
        {
            self.expanded = saved;
        }
        self.saved_expanded = None;
        self.refresh_rows();
        cx.notify();
    }

    /// Escape in the box: clear it, restore the tree and return to it.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_query("", cx);
        self.focus.focus(window, cx);
    }

    /// Packages the sources say are vulnerable or deprecated (`id/version`, id lowercase), for the warning glyph.
    pub fn set_package_warnings(&mut self, warnings: HashSet<String>, cx: &mut Context<Self>) {
        if warnings != self.package_warnings {
            self.package_warnings = warnings;
            cx.notify();
        }
    }

    /// Whether a row is a package with the yellow warning glyph.
    pub fn package_warning(&self, row: &Row) -> bool {
        match &row.kind {
            NodeKind::Package {
                id,
                version,
                warning,
                ..
            } => {
                *warning
                    || version.as_ref().is_some_and(|v| {
                        self.package_warnings
                            .contains(&format!("{}/{v}", id.to_lowercase()))
                    })
            }
            _ => false,
        }
    }

    /// The repository's root and its files' glyphs, or none (brief 0040).
    pub fn set_git(&mut self, git: Option<(PathBuf, Rc<GlyphIndex>)>, cx: &mut Context<Self>) {
        self.git = git.map(|(root, g)| (super::documents::normalize_path(&root), g));
        cx.notify();
    }

    /// `path`'s repository-relative name, when it is in the repository.
    fn git_relative(&self, path: &Path) -> Option<String> {
        let (root, _) = self.git.as_ref()?;
        let p = super::documents::normalize_path(path);
        let rel = p.strip_prefix(root).ok()?;
        Some(
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        )
    }

    /// The glyph a row's file shows.
    pub fn git_glyph(&self, row: &Row) -> Option<eludite_git::FileGlyph> {
        if !matches!(
            row.kind,
            NodeKind::File { .. } | NodeKind::CargoTarget { .. }
        ) {
            return None;
        }
        let rel = self.git_relative(row.path.as_deref()?)?;
        self.git.as_ref()?.1.get(&rel)
    }

    /// Ctrl+D on the selected file: Compare with Unmodified. Alt+Enter on a project: its property pages (brief 0049).
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed in the search box are the box's.
        if self.input.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let k = &event.keystroke;
        if k.modifiers.alt
            && !k.modifiers.control
            && k.key == "enter"
            && let Some(path) = self
                .selected
                .as_ref()
                .and_then(|id| self.rows.iter().find(|r| &r.id == id))
                .filter(|r| matches!(r.kind, NodeKind::Project { .. }))
                .and_then(|r| r.path.clone())
        {
            cx.stop_propagation();
            open_properties(path, window, cx);
            return;
        }
        if !(k.modifiers.control && k.key == "d" && !k.modifiers.shift && !k.modifiers.alt) {
            return;
        }
        let Some(path) = self
            .selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|r| &r.id == id))
            .filter(|r| r.kind.opens_file())
            .and_then(|r| r.path.clone())
        else {
            return;
        };
        if self.git_relative(&path).is_none() {
            return;
        }
        cx.stop_propagation();
        window.dispatch_action(
            Box::new(RunCommand::new(
                eludite_commands::git::DIFF,
                json!({ "path": path.to_string_lossy() }),
            )),
            cx,
        );
    }

    /// Every startup project (brief 0028: Visual Studio's multiple startup projects), each drawn bold.
    pub fn set_startups(&mut self, startups: Vec<PathBuf>, cx: &mut Context<Self>) {
        let startups: Vec<PathBuf> = startups
            .iter()
            .map(|p| super::documents::normalize_path(p))
            .collect();
        if startups != self.startup {
            self.startup = startups;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn startup(&self) -> Option<&Path> {
        self.startup.first().map(PathBuf::as_path)
    }

    /// Every project drawn bold.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn startups(&self) -> &[PathBuf] {
        &self.startup
    }

    fn is_startup(&self, row: &Row) -> bool {
        matches!(
            row.kind,
            NodeKind::Project { .. } | NodeKind::CargoPackage { .. }
        ) && row
            .path
            .as_deref()
            .map(super::documents::normalize_path)
            .is_some_and(|p| self.startup.contains(&p))
    }

    fn open_menu(&mut self, ix: usize, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        self.selected = Some(row.id.clone());
        let project = matches!(
            row.kind,
            NodeKind::Project { .. } | NodeKind::CargoPackage { .. }
        );
        let git_file = row.kind.opens_file()
            && row
                .path
                .as_deref()
                .is_some_and(|p| self.git_relative(p).is_some());
        // Brief 0048: the solution, the Packages node and a top-level package have NuGet items.
        let nuget = matches!(
            row.kind,
            NodeKind::Solution
                | NodeKind::DependencyGroup {
                    group: DependencyGroup::Packages
                }
                | NodeKind::Package {
                    transitive: false,
                    ..
                }
        );
        self.menu =
            ((project || git_file || nuget) && row.path.is_some()).then_some((ix, event.position));
        cx.notify();
    }

    fn run_item(&mut self, item: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self
            .menu
            .take()
            .and_then(|(ix, _)| self.rows.get(ix))
            .cloned()
        else {
            return;
        };
        let Some(path) = row.path.clone() else {
            return;
        };
        if let Some((command, args)) = nuget_command(item, &row) {
            window.dispatch_action(Box::new(RunCommand::new(command, args)), cx);
            cx.notify();
            return;
        }
        if let Some((command, args)) = context_command(item, &path) {
            window.dispatch_action(Box::new(RunCommand::new(command, args)), cx);
        }
        cx.notify();
    }

    fn context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (ix, at) = self.menu?;
        let row = self.rows.get(ix)?;
        let t = self.theme;
        let mut items = Vec::new();
        let nuget_entries: Option<Vec<(&'static str, &'static str)>> = match &row.kind {
            NodeKind::Solution => Some(vec![NUGET_SOLUTION_ITEM]),
            NodeKind::DependencyGroup { .. } => Some(vec![NUGET_PROJECT_ITEM]),
            NodeKind::Package { .. } => Some(vec![
                NUGET_UPDATE_ITEM,
                NUGET_REMOVE_ITEM,
                NUGET_PROJECT_ITEM,
            ]),
            _ => None,
        };
        let entries: Vec<(&'static str, &'static str)> = if let Some(e) = nuget_entries {
            e
        } else if row.kind.opens_file() {
            WORKSPACE_GIT_ITEMS
                .iter()
                .map(|(i, l, _)| (*i, *l))
                .collect()
        } else {
            let mut items = CONTEXT_ITEMS.to_vec();
            items.push((WORKSPACE_TERMINAL_ITEM.0, WORKSPACE_TERMINAL_ITEM.1));
            // Brief 0048: a .NET project's Manage NuGet Packages..., after Build, Rebuild and Clean.
            if matches!(row.kind, NodeKind::Project { .. }) {
                items.insert(3, NUGET_PROJECT_ITEM);
            }
            items
        };
        let file = row.kind.opens_file();
        let dotnet_project = matches!(row.kind, NodeKind::Project { .. });
        let nuget_menu = row.kind.is_dependency() || row.kind == NodeKind::Solution;
        for (i, (item, label)) in entries.into_iter().enumerate() {
            let separator = if nuget_menu {
                false
            } else if file {
                i == 2 || i == 4
            } else if dotnet_project {
                // Brief 0048's Manage NuGet Packages... at 3 moves brief 0049's Properties to 6.
                i == 3 || i == 4 || i == 5 || i == 6
            } else {
                i == 3 || i == 4 || i == 5
            };
            if separator {
                items.push(
                    div()
                        .h(px(1.))
                        .mx_1()
                        .my_1()
                        .bg(t.border)
                        .into_any_element(),
                );
            }
            // Every item applies to .NET projects and Cargo packages alike (brief 0029 starts Cargo packages).
            let el = menu_row(
                context_item_selector(item),
                label,
                0,
                false,
                false,
                false,
                &t,
            )
            .min_w(px(220.));
            items.push(
                el.on_click(
                    cx.listener(move |this, _, window, cx| this.run_item(item, window, cx)),
                )
                .into_any_element(),
            );
        }
        Some(
            deferred(
                anchored().position(at).child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("se-context-menu")
                        .debug_selector(|| "se-context-menu".into())
                        .occlude()
                        .py_1()
                        .text_size(t.typography.ui)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(3),
        )
    }

    pub fn model(&self) -> Option<&SolutionModel> {
        self.model.as_ref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn set_placeholder(&mut self, placeholder: Placeholder, cx: &mut Context<Self>) {
        if self.model.is_none() || placeholder == Placeholder::NoSolution {
            self.model = None;
            self.rows.clear();
            self.reindex();
        }
        self.placeholder = placeholder;
        cx.notify();
    }

    /// A new tree. Expanded nodes that still exist stay expanded (a refresh of the same solution).
    pub fn set_model(&mut self, model: SolutionModel, cx: &mut Context<Self>) {
        let same_solution = self.model.as_ref().is_some_and(|m| m.path == model.path);
        if !same_solution {
            self.expanded = model.default_expanded();
            self.selected = None;
        }
        self.test_projects = test_projects(&model);
        self.model = Some(model);
        self.reindex();
        if !same_solution {
            self.search = None;
            self.saved_expanded = None;
        }
        // A refreshed (or another) tree keeps the search the box holds.
        let query = self.input.read(cx).text();
        if query.trim().is_empty() {
            self.refresh_rows();
        } else {
            self.run_search(query, cx);
        }
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.model = None;
        self.rows.clear();
        self.expanded.clear();
        self.selected = None;
        self.search = None;
        self.saved_expanded = None;
        self.test_projects.clear();
        self.reindex();
        self.placeholder = Placeholder::NoSolution;
        cx.notify();
    }

    /// Rebuild the index the search runs over from the model.
    fn reindex(&mut self) {
        let (all, parents) = SearchIndex::of(self.model.as_ref());
        let mut index = self.index.write().unwrap_or_else(|e| e.into_inner());
        index.all = all;
        index.parents = parents;
    }

    pub fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        let expanded = match &mut self.search {
            Some(s) => &mut s.expanded,
            None => &mut self.expanded,
        };
        if !expanded.remove(id) {
            expanded.insert(id.to_owned());
        }
        self.refresh_rows();
        cx.notify();
    }

    /// Expand the folders above `path` and select its node (Track Active Item).
    pub fn reveal(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(model) = &self.model else { return };
        let Some(trail) = model.ancestors_of_file(path) else {
            return;
        };
        self.expanded.extend(trail);
        self.refresh_rows();
        self.selected = self
            .rows
            .iter()
            .find(|r| r.path.as_deref() == Some(path))
            .map(|r| r.id.clone());
        cx.notify();
    }

    /// Expand the folders down to the folder `dir` and select its node (a terminal's link to a directory, brief 0041):
    /// a project or package whose file is in `dir`, else the deepest folder node named like `dir` whose files all lie
    /// under it. Nothing changes when the tree has no node for it.
    pub fn reveal_folder(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let Some(model) = &self.model else { return };
        let Some(trail) = folder_trail(&model.root, dir) else {
            return;
        };
        let (id, above) = trail.split_last().expect("a trail names its node");
        self.expanded.extend(above.iter().cloned());
        self.expanded.insert(id.clone());
        self.selected = Some(id.clone());
        self.refresh_rows();
        cx.notify();
    }

    /// The selected node's id (brief 0049: Project > Properties opens the selected project's pages).
    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    fn refresh_rows(&mut self) {
        if let Some(s) = &self.search {
            self.rows = self.search_visible_rows(s);
            return;
        }
        self.rows = self
            .model
            .as_ref()
            .map(|m| m.visible_rows(&self.expanded))
            .unwrap_or_default();
    }

    /// The rows a search shows: the matches and their ancestors, a node's children filtered while it leads to a match
    /// and all of them under a match the person expands.
    fn search_visible_rows(&self, s: &SearchState) -> Vec<Row> {
        let index = self.index.read().unwrap_or_else(|e| e.into_inner());
        let (all, parents) = (&index.all, &index.parents);
        let hits = &s.hits;
        if hits.kept.len() != all.len() {
            return Vec::new();
        }
        let mut visible = vec![false; all.len()];
        let mut out = Vec::new();
        for (i, row) in all.iter().enumerate() {
            let shown = match parents[i] {
                None => hits.kept[i],
                Some(p) => {
                    visible[p]
                        && s.expanded.contains(&all[p].id)
                        && (hits.kept[i] || !hits.leads_to_match[p])
                }
            };
            if !shown {
                continue;
            }
            visible[i] = true;
            let mut row = row.clone();
            row.expanded = row.has_children && s.expanded.contains(&row.id);
            out.push(row);
        }
        out
    }

    fn click(
        &mut self,
        ix: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        self.selected = Some(row.id.clone());
        if event.click_count() >= 2 {
            match (&row.kind, &row.path) {
                (kind, Some(path)) if kind.opens_file() => {
                    open_file(path.clone(), window, cx);
                }
                // A .NET project's property pages (brief 0049); the triangle still expands it.
                (NodeKind::Project { .. }, Some(path)) => open_properties(path.clone(), window, cx),
                _ if row.has_children => self.toggle(&row.id, cx),
                _ => {}
            }
        }
        cx.notify();
    }
}

/// The command a NuGet context menu item runs for `row` (brief 0048).
pub fn nuget_command(item: &str, row: &Row) -> Option<(&'static str, Value)> {
    let path = row.path.as_ref()?.to_string_lossy().into_owned();
    match (item, &row.kind) {
        (i, _) if i == NUGET_SOLUTION_ITEM.0 => {
            Some((eludite_commands::nuget::MANAGE, json!({ "solution": true })))
        }
        (i, _)
            if i == NUGET_PROJECT_ITEM.0 && !matches!(row.kind, NodeKind::CargoPackage { .. }) =>
        {
            Some((eludite_commands::nuget::MANAGE, json!({ "project": path })))
        }
        (i, NodeKind::Package { id, .. }) if i == NUGET_UPDATE_ITEM.0 => Some((
            eludite_commands::nuget::UPDATE,
            json!({ "package": id, "project": path }),
        )),
        (i, NodeKind::Package { id, .. }) if i == NUGET_REMOVE_ITEM.0 => Some((
            eludite_commands::nuget::UNINSTALL,
            json!({ "package": id, "project": path }),
        )),
        _ => None,
    }
}

fn open_properties(path: PathBuf, window: &mut Window, cx: &mut Context<SolutionExplorer>) {
    window.dispatch_action(
        Box::new(RunCommand::new(
            eludite_commands::project::properties::PROPERTIES,
            json!({ "project": path.to_string_lossy(), "open": true }),
        )),
        cx,
    );
}

fn open_file(path: PathBuf, window: &mut Window, cx: &mut Context<SolutionExplorer>) {
    window.dispatch_action(
        Box::new(RunCommand::new(
            workspace::FILE_OPEN,
            json!({ "path": path.to_string_lossy() }),
        )),
        cx,
    );
}

impl SolutionExplorer {
    /// The box "Search Workspace (Ctrl+;)": the input, then the magnifier, or the clear button while it has text.
    fn search_box(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = self.theme;
        let input = self.input.read(cx);
        let has_text = !input.is_empty();
        let focused = input.focus_handle(cx).is_focused(window);
        let end = if has_text {
            div()
                .id(SEARCH_CLEAR)
                .debug_selector(|| SEARCH_CLEAR.into())
                .flex_none()
                .cursor_pointer()
                .child(eludite_ui::icon(Icon::Clear, Icon::Clear.tint(&t)))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_query("", cx);
                    this.input.read(cx).focus_handle(cx).focus(window, cx);
                }))
                .into_any_element()
        } else {
            div()
                .flex_none()
                .child(eludite_ui::icon(Icon::Search, Icon::Search.tint(&t)))
                .into_any_element()
        };
        div()
            .id(SEARCH_BOX)
            .debug_selector(|| SEARCH_BOX.into())
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.))
            .mx_1()
            .my_1()
            .pl_1()
            .pr(px(3.))
            .gap_1()
            .bg(t.background)
            .border_1()
            .border_color(if focused { t.accent } else { t.border })
            .text_size(t.typography.ui)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.input.read(cx).focus_handle(cx).focus(window, cx);
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.input.clone()),
            )
            .child(end)
    }
}

impl Render for SolutionExplorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let search_box = self.search_box(window, cx);
        let root = div()
            .id("solution-explorer")
            .debug_selector(|| "solution-explorer".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .flex_col()
            .child(search_box);
        if self.model.is_none() {
            let text = match &self.placeholder {
                Placeholder::NoSolution => {
                    "No workspace is open. File > Open > Workspace... (Ctrl+Shift+Alt+O) opens a folder; File > Open > Solution \
                     or Project File... (Ctrl+Shift+O) opens a .NET solution.".to_owned()
                }
                Placeholder::Loading(name) => format!("Loading {name}\u{2026}"),
                Placeholder::Failed(why) => why.clone(),
            };
            return root
                .child(
                    div()
                        .flex_1()
                        .p_2()
                        .text_size(t.typography.ui)
                        .text_color(t.text_muted)
                        .child(text),
                )
                .into_any_element();
        }
        let count = self.rows.len();
        root.child(
            div()
                .flex_1()
                .min_h(px(TREE_ROW_HEIGHT))
                .flex()
                .flex_col()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
                )
                .child(
                    uniform_list(
                        "solution-explorer-rows",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            let t = this.theme;
                            range
                                .filter_map(|ix| {
                                    let row = this.rows.get(ix)?;
                                    let style = TreeRowStyle {
                                        depth: row.depth,
                                        disclosure: row.has_children.then_some(row.expanded),
                                        selected: this.selected.as_deref() == Some(row.id.as_str()),
                                        muted: matches!(
                                            row.kind,
                                            NodeKind::Project { error: Some(_), .. }
                                        ),
                                        bold: this.is_startup(row),
                                    };
                                    let id = row.id.clone();
                                    let badge = if this.package_warning(row) {
                                        Some((PACKAGE_WARNING, 0xFF_CC_00))
                                    } else {
                                        this.git_glyph(row).map(|g| (g.glyph(), g.color()))
                                    };
                                    let icon = row_icon(row, this.test_projects.contains(&row.id));
                                    let matched = this
                                        .search_matches(&row.id)
                                        .map(<[Range<usize>]>::to_vec)
                                        .unwrap_or_default();
                                    Some(
                                        tree_row_with_icon(
                                            &t,
                                            SharedString::from(row_selector(&row.id)),
                                            Some((icon, icon.tint(&t))),
                                            badge,
                                            row.label.clone(),
                                            &matched,
                                            style,
                                            cx.listener(move |this, _, _, cx| this.toggle(&id, cx)),
                                        )
                                        .relative()
                                        .children(eludite_ui::bounds_canvas(
                                            this.probe.as_ref(),
                                            row_selector(&row.id),
                                        ))
                                        .on_click(cx.listener(move |this, e, window, cx| {
                                            this.click(ix, e, window, cx)
                                        }))
                                        .on_mouse_down(
                                            MouseButton::Right,
                                            cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                                this.open_menu(ix, e, cx)
                                            }),
                                        ),
                                    )
                                })
                                .collect()
                        }),
                    )
                    .flex_1()
                    .min_h(px(TREE_ROW_HEIGHT)),
                ),
        )
        .children(self.context_menu(cx))
        .into_any_element()
    }
}

/// The ids from `root` down to the node that stands for the folder `dir` ([`SolutionExplorer::reveal_folder`]).
fn folder_trail(root: &eludite_workspace::explorer::Node, dir: &Path) -> Option<Vec<String>> {
    use eludite_workspace::explorer::Node;
    fn files<'a>(n: &'a Node, out: &mut Vec<&'a Path>) {
        if let (NodeKind::File { .. }, Some(p)) = (&n.kind, &n.path) {
            out.push(p);
        }
        for c in &n.children {
            files(c, out);
        }
    }
    fn stands_for(n: &Node, dir: &Path) -> bool {
        if n.kind.is_dependency() {
            return false;
        }
        match n.kind {
            NodeKind::File { .. } | NodeKind::CargoTarget { .. } => false,
            NodeKind::Folder => {
                let mut under = Vec::new();
                files(n, &mut under);
                Some(n.label.as_str()) == dir.file_name().and_then(|f| f.to_str())
                    && !under.is_empty()
                    && under.iter().all(|f| f.starts_with(dir))
            }
            _ => n
                .path
                .as_deref()
                .is_some_and(|p| p == dir || p.parent() == Some(dir)),
        }
    }
    fn walk(n: &Node, dir: &Path, trail: &mut Vec<String>) -> bool {
        trail.push(n.id.clone());
        // The deepest match wins: a package's folder before the workspace whose manifest sits beside it.
        if n.children.iter().any(|c| walk(c, dir, trail)) || stands_for(n, dir) {
            return true;
        }
        trail.pop();
        false
    }
    let mut trail = Vec::new();
    walk(root, dir, &mut trail).then_some(trail)
}

#[cfg(test)]
#[path = "explorer_tests.rs"]
mod tests;
