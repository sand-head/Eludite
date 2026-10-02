//! Solution Explorer (PLAN.md 4.2, brief 0012): the tree from `eludite-workspace`'s [`SolutionModel`], drawn as
//! virtualized rows. Click selects; the triangle or a double-click expands and collapses; double-clicking a file
//! opens it through `eludite.file.open`. No context menus yet.

use std::collections::HashSet;
use std::path::PathBuf;

use eludite_commands::workspace;
use eludite_ui::{RunCommand, TREE_ROW_HEIGHT, Theme, TreeRowStyle, tree_row};
use eludite_workspace::explorer::{NodeKind, Row, SolutionModel};
use gpui::{
    ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, uniform_list,
};
use serde_json::json;

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
}

/// Debug selector of the row for node `id` (tests and the real-input driver).
pub fn row_selector(id: &str) -> String {
    format!("se-{id}")
}

fn glyph(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Solution => "\u{25A3}",
        NodeKind::Project { .. } => "C#",
        NodeKind::Folder => "\u{25A1}",
        NodeKind::File { .. } => "\u{2261}",
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
        }
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
                (NodeKind::File { .. }, Some(path)) => {
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
                    "No solution is open. File > Open > Project/Solution (Ctrl+Shift+O).".to_owned()
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
                                    .on_click(cx.listener(
                                        move |this, e, window, cx| this.click(ix, e, window, cx),
                                    )),
                                )
                            })
                            .collect()
                    }),
                )
                .flex_1()
                .min_h(px(TREE_ROW_HEIGHT)),
            )
            .into_any_element()
    }
}
