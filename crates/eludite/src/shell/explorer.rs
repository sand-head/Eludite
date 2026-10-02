//! The Workspace window (PLAN.md 4.2, brief 0012; Visual Studio's Solution Explorer, renamed so Cargo and npm
//! workspaces fit the same window later): the tree from `eludite-workspace`'s [`SolutionModel`], drawn as
//! virtualized rows. Click selects; the triangle or a double-click expands and collapses; double-clicking a file
//! opens it through `eludite.file.open`. The startup project is drawn bold (brief 0020).
//!
//! A right-click on a project (or a Cargo package) opens its context menu (brief 0020), in Visual Studio's order:
//! Build, Rebuild and Clean (`eludite.build.project` with the project and a target), Set as Startup Project
//! (`eludite.workspace.set_startup_project`, .NET projects and Cargo packages: brief 0029 debugs Cargo packages with
//! lldb-dap) and Open Containing Folder (`eludite.workspace.open_containing_folder`). Every item runs its command
//! through the bus.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eludite_commands::workspace;
use eludite_ui::{RunCommand, TREE_ROW_HEIGHT, Theme, TreeRowStyle, menu_row, tree_row};
use eludite_workspace::explorer::{NodeKind, Row, SolutionModel};
use gpui::{
    ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    anchored, deferred, div, px, uniform_list,
};
use serde_json::{Value, json};

/// The project context menu's items: (selector suffix, label).
pub const CONTEXT_ITEMS: [(&str, &str); 5] = [
    ("build", "Build"),
    ("rebuild", "Rebuild"),
    ("clean", "Clean"),
    ("startup", "Set as Startup Project"),
    ("folder", "Open Containing Folder"),
];

/// Debug selector of a context menu item (`build`, `rebuild`, `clean`, `startup`, `folder`).
pub fn context_item_selector(item: &str) -> String {
    format!("se-menu-{item}")
}

/// The command and arguments a context menu item runs for project file (or `Cargo.toml`) `path`.
pub fn context_command(item: &str, path: &Path) -> Option<(&'static str, Value)> {
    let p = path.to_string_lossy();
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
        _ => return None,
    })
}

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
    /// The startup project's file, drawn bold.
    startup: Option<PathBuf>,
    /// The open context menu: the row it is for and where the pointer was.
    menu: Option<(usize, Point<Pixels>)>,
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
    }
}

impl SolutionExplorer {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            model: None,
            expanded: HashSet::new(),
            selected: None,
            rows: Vec::new(),
            placeholder: Placeholder::NoSolution,
            startup: None,
            menu: None,
        }
    }

    /// The startup project (Set as Startup Project's, or the first executable project).
    pub fn set_startup(&mut self, startup: Option<PathBuf>, cx: &mut Context<Self>) {
        let startup = startup.map(|p| super::documents::normalize_path(&p));
        if startup != self.startup {
            self.startup = startup;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn startup(&self) -> Option<&Path> {
        self.startup.as_deref()
    }

    fn is_startup(&self, row: &Row) -> bool {
        matches!(
            row.kind,
            NodeKind::Project { .. } | NodeKind::CargoPackage { .. }
        ) && self.startup.is_some()
            && row.path.as_deref().map(super::documents::normalize_path) == self.startup
    }

    fn open_menu(&mut self, ix: usize, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        self.selected = Some(row.id.clone());
        let project = matches!(
            row.kind,
            NodeKind::Project { .. } | NodeKind::CargoPackage { .. }
        );
        self.menu = (project && row.path.is_some()).then_some((ix, event.position));
        cx.notify();
    }

    fn run_item(&mut self, item: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .menu
            .take()
            .and_then(|(ix, _)| self.rows.get(ix))
            .and_then(|r| r.path.clone())
        else {
            return;
        };
        if let Some((command, args)) = context_command(item, &path) {
            window.dispatch_action(Box::new(RunCommand::new(command, args)), cx);
        }
        cx.notify();
    }

    fn context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (ix, at) = self.menu?;
        self.rows.get(ix)?;
        let t = self.theme;
        let mut items = Vec::new();
        for (i, (item, label)) in CONTEXT_ITEMS.into_iter().enumerate() {
            if i == 3 || i == 4 {
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
                _ if row.has_children => self.toggle(&row.id, cx),
                _ => {}
            }
        }
        cx.notify();
    }
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
                                Some(
                                    tree_row(
                                        &t,
                                        SharedString::from(row_selector(&row.id)),
                                        Some(glyph(&row.kind)),
                                        row.label.clone(),
                                        style,
                                        cx.listener(move |this, _, _, cx| this.toggle(&id, cx)),
                                    )
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
