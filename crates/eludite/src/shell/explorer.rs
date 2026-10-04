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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use eludite_commands::workspace;
use eludite_git::GlyphIndex;
use eludite_ui::{
    RunCommand, TREE_ROW_HEIGHT, Theme, TreeRowStyle, WORKSPACE_GIT_ITEMS, WORKSPACE_TERMINAL_ITEM,
    menu_row, tree_row_with_badge,
};
use eludite_workspace::explorer::{DependencyGroup, NodeKind, Row, SolutionModel};
use gpui::{
    ClickEvent, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, anchored, deferred, div, px, uniform_list,
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

fn glyph(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Solution => "\u{25A3}",
        NodeKind::Project { .. } => "C#",
        NodeKind::Folder | NodeKind::CargoTargets => "\u{25A1}",
        NodeKind::File { .. } => "\u{2261}",
        NodeKind::FolderRoot => "\u{25A0}",
        NodeKind::CargoWorkspace | NodeKind::CargoPackage { .. } => "Rs",
        NodeKind::CargoTarget { .. } => "\u{25B8}",
        // Brief 0048: the Dependencies node.
        NodeKind::Dependencies | NodeKind::DependencyGroup { .. } => "\u{25A6}",
        NodeKind::Package { .. } => "\u{25C8}",
        NodeKind::Framework => "\u{25A4}",
        NodeKind::ProjectReference => "C#",
    }
}

impl SolutionExplorer {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
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
        }
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
        self.model = Some(model);
        self.refresh_rows();
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.model = None;
        self.rows.clear();
        self.expanded.clear();
        self.selected = None;
        self.placeholder = Placeholder::NoSolution;
        cx.notify();
    }

    pub fn toggle(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_owned());
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
        self.rows = self
            .model
            .as_ref()
            .map(|m| m.visible_rows(&self.expanded))
            .unwrap_or_default();
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

impl Render for SolutionExplorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        if self.model.is_none() {
            let text = match &self.placeholder {
                Placeholder::NoSolution => {
                    "No workspace is open. File > Open > Workspace... (Ctrl+Shift+Alt+O) opens a folder; File > Open > Solution \
                     or Project File... (Ctrl+Shift+O) opens a .NET solution.".to_owned()
                }
                Placeholder::Loading(name) => format!("Loading {name}\u{2026}"),
                Placeholder::Failed(why) => why.clone(),
            };
            return div()
                .id("solution-explorer")
                .debug_selector(|| "solution-explorer".into())
                .size_full()
                .p_2()
                .text_size(t.typography.ui)
                .text_color(t.text_muted)
                .child(text)
                .into_any_element();
        }
        let count = self.rows.len();
        div()
            .id("solution-explorer")
            .debug_selector(|| "solution-explorer".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus.focus(window, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
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
                                Some(
                                    tree_row_with_badge(
                                        &t,
                                        SharedString::from(row_selector(&row.id)),
                                        Some(glyph(&row.kind)),
                                        badge,
                                        row.label.clone(),
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
