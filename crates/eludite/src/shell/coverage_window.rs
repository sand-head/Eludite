//! The Code Coverage Results window (brief 0076; Visual Studio's Test > Windows > Code Coverage Results): the last
//! coverage as assembly > namespace > class > method with Covered (Lines), Not Covered (Lines) and Covered (% Lines),
//! a toolbar with Analyze Code Coverage for All Tests and Show Code Coverage Coloring, and the totals. A double-click
//! on a class or method opens its source (`eludite.file.open`); every button runs a command.

use std::collections::HashSet;
use std::ops::Range;

use eludite_commands::test::{CoverageRow, CoverageSummary};
use eludite_ui::{RunCommand, Theme, TreeRowStyle, toolbar_button, tree_row};
use gpui::{
    App, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, UniformListScrollHandle, Window, div, px, uniform_list,
};
use serde_json::{Value, json};

/// Debug selectors.
pub const ANALYZE: &str = "coverage-analyze";
pub const COLORING: &str = "coverage-coloring";
pub const TOTALS: &str = "coverage-totals";

pub fn row_selector(ix: usize) -> String {
    format!("coverage-row-{ix}")
}

/// A visible row: the row and its key (the names along its path).
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub key: String,
    pub row: CoverageRow,
    pub expanded: Option<bool>,
}

pub struct CoverageWindow {
    theme: Theme,
    rows: Vec<CoverageRow>,
    summary: Option<CoverageSummary>,
    show: bool,
    stale: bool,
    collapsed: HashSet<String>,
    shown: Vec<Shown>,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
}

impl CoverageWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            summary: None,
            show: true,
            stale: false,
            collapsed: HashSet::new(),
            shown: Vec::new(),
            selected: None,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn set_data(
        &mut self,
        rows: Vec<CoverageRow>,
        summary: Option<CoverageSummary>,
        show: bool,
        stale: bool,
        cx: &mut Context<Self>,
    ) {
        if rows != self.rows || summary != self.summary || show != self.show || stale != self.stale
        {
            self.rows = rows;
            self.summary = summary;
            self.show = show;
            self.stale = stale;
            self.rebuild();
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn shown(&self) -> &[Shown] {
        &self.shown
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn totals(&self) -> String {
        self.totals_text()
    }

    pub fn toggle(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_owned());
        }
        self.rebuild();
        cx.notify();
    }

    fn rebuild(&mut self) {
        let mut out = Vec::new();
        let mut path: Vec<String> = Vec::new();
        let mut hidden_below: Option<u32> = None;
        for (i, r) in self.rows.iter().enumerate() {
            path.truncate(r.depth as usize);
            path.push(r.name.clone());
            let key = path.join("/");
            if let Some(d) = hidden_below {
                if r.depth > d {
                    continue;
                }
                hidden_below = None;
            }
            let has_children = self.rows.get(i + 1).is_some_and(|n| n.depth > r.depth);
            let expanded = has_children.then(|| !self.collapsed.contains(&key));
            if expanded == Some(false) {
                hidden_below = Some(r.depth);
            }
            out.push(Shown {
                key,
                row: r.clone(),
                expanded,
            });
        }
        self.shown = out;
    }

    fn totals_text(&self) -> String {
        match &self.summary {
            None => "No code coverage yet: Test > Analyze Code Coverage for All Tests.".into(),
            Some(s) => {
                let mut t = format!(
                    "{:.1}% covered: {} of {} lines",
                    s.percent, s.lines_covered, s.lines_valid
                );
                if let (Some(c), Some(v)) = (s.branches_covered, s.branches_valid) {
                    t.push_str(&format!(", {c} of {v} branches"));
                }
                if self.stale {
                    t.push_str(" (documents changed since)");
                }
                if !s.missing.is_empty() {
                    t.push_str(&format!(". No coverage: {}", s.missing.join("; ")));
                }
                t
            }
        }
    }

    fn run(&self, command: &str, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
    }

    fn click(&mut self, ix: usize, count: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        cx.notify();
        let Some(s) = self.shown.get(ix).cloned() else {
            return;
        };
        if count < 2 {
            return;
        }
        match (&s.row.file, s.expanded) {
            (Some(file), _) if s.row.kind == "method" || s.row.kind == "class" => {
                let mut args = json!({ "path": file });
                if let Some(l) = s.row.line {
                    args["line"] = json!(l);
                }
                self.run(eludite_commands::workspace::FILE_OPEN, args, window, cx);
            }
            (_, Some(_)) => self.toggle(&s.key, cx),
            _ => {}
        }
    }

    fn render_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        range
            .filter_map(|ix| {
                let s = self.shown.get(ix)?;
                let key = s.key.clone();
                let style = TreeRowStyle {
                    depth: s.row.depth as usize,
                    disclosure: s.expanded,
                    selected: self.selected == Some(ix),
                    muted: self.stale,
                    bold: s.row.depth == 0,
                };
                let cols = div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap_2()
                    .text_size(t.typography.small)
                    .child(div().w(px(60.)).child(format!("{}", s.row.covered)))
                    .child(div().w(px(60.)).child(format!("{}", s.row.not_covered)))
                    .child(div().w(px(60.)).child(format!("{:.1}%", s.row.percent)));
                Some(
                    div()
                        .id(gpui::SharedString::from(row_selector(ix)))
                        .debug_selector(move || row_selector(ix))
                        .flex()
                        .flex_row()
                        .w_full()
                        .items_center()
                        .child(div().flex_1().min_w_0().child(tree_row(
                            &t,
                            format!("{}-label", row_selector(ix)),
                            None,
                            s.row.name.clone(),
                            style,
                            {
                                let entity = cx.entity();
                                move |_, _, cx| {
                                    entity.update(cx, |w, cx| w.toggle(&key, cx));
                                }
                            },
                        )))
                        .child(cols)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                            this.click(ix, e.click_count(), window, cx)
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for CoverageWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let show = self.show;
        let toolbar = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .flex_none()
            .items_center()
            .gap_1()
            .min_h(px(26.))
            .px_1()
            .border_b_1()
            .border_color(t.border)
            .child(
                toolbar_button(ANALYZE, "Analyze Code Coverage for All Tests", true, &t).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.run(eludite_commands::test::COVERAGE, json!({}), window, cx)
                    }),
                ),
            )
            .child(
                toolbar_button(
                    COLORING,
                    if show {
                        "\u{2713} Show Code Coverage Coloring"
                    } else {
                        "Show Code Coverage Coloring"
                    },
                    true,
                    &t,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run(
                        eludite_commands::test::COVERAGE_COLORING,
                        json!({ "show": !show }),
                        window,
                        cx,
                    )
                })),
            );
        let header = div()
            .flex()
            .flex_row()
            .flex_none()
            .px_2()
            .gap_2()
            .text_size(t.typography.small)
            .text_color(t.text_muted)
            .border_b_1()
            .border_color(t.border)
            .child(div().flex_1().child("Hierarchy"))
            .child(div().w(px(60.)).child("Covered"))
            .child(div().w(px(60.)).child("Not Covered"))
            .child(div().w(px(60.)).child("Covered %"));
        div()
            .id("code-coverage-results")
            .debug_selector(|| "code-coverage-results".into())
            .size_full()
            .flex()
            .flex_col()
            .text_color(t.text)
            .child(toolbar)
            .child(
                div()
                    .id(TOTALS)
                    .debug_selector(|| TOTALS.into())
                    .flex_none()
                    .px_2()
                    .py_1()
                    .text_size(t.typography.small)
                    .child(self.totals_text()),
            )
            .child(header)
            .child(
                uniform_list(
                    "coverage-rows",
                    self.shown.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
    }
}
