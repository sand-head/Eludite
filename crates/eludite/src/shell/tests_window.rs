//! The Test Explorer window (brief 0035; Visual Studio's Test > Test Explorer, Ctrl+E, T): the toolbar (Run All, Run,
//! Debug, Run Failed Tests, Repeat Last Run, Cancel, the search box and the last run's counts), the tree (project, then
//! namespace and class for .NET, module path for Rust, then test, each with its outcome glyph and duration), and the
//! detail pane of the selected test (its outcome, duration, failure message, stack trace and output).
//!
//! The window holds a copy of the shell's model ([`TreeData`]) and what is its own: which nodes are collapsed, the
//! selection and the search text. Every button and the search box run commands (`eludite.test.*`), as the Test menu
//! and agents do; a double-click on a test opens its source (`eludite.file.open`). The search box takes Visual Studio's
//! filters: `FullName:<text>`, `Outcome:<Passed|Failed|Skipped|Not Run>`, `Trait:<Name=Value>` and plain text (a
//! substring of the name or full name), all of them holding.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use eludite_commands::test::{Outcome, RunSummary};
use eludite_ui::{RunCommand, TestGlyph, Theme, TreeRowStyle, test_row, text_box, toolbar_button};
use gpui::{
    App, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use serde_json::{Value, json};

use super::test_runs::{NodeState, Phase, ProjectNode, TestNode};

/// Debug selectors.
pub const RUN_ALL: &str = "test-explorer-run-all";
pub const RUN: &str = "test-explorer-run";
pub const DEBUG: &str = "test-explorer-debug";
pub const RUN_FAILED: &str = "test-explorer-run-failed";
pub const REPEAT: &str = "test-explorer-repeat";
pub const CANCEL: &str = "test-explorer-cancel";
pub const SEARCH_BOX: &str = "test-explorer-search";
pub const SUMMARY: &str = "test-explorer-summary";
pub const DETAILS: &str = "test-explorer-details";

/// Debug selector of visible row `ix`.
pub fn row_selector(ix: usize) -> String {
    format!("test-explorer-row-{ix}")
}

/// What the shell hands the window.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeData {
    pub projects: Vec<ProjectNode>,
    pub tests: Vec<TestNode>,
    pub phase: Phase,
    pub message: Option<String>,
    /// The last finished run's summary line and counts.
    pub summary: Option<(String, RunSummary)>,
    pub running: bool,
}

impl Default for TreeData {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            tests: Vec::new(),
            phase: Phase::Idle,
            message: None,
            summary: None,
            running: false,
        }
    }
}

/// What a row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Project(String),
    Group,
    Test(String),
}

/// A visible row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Stable across refreshes: `p:<project>`, `g:<project>/<path>`, `t:<test id>`.
    pub key: String,
    pub depth: usize,
    pub label: String,
    pub glyph: TestGlyph,
    pub detail: Option<String>,
    pub kind: RowKind,
    /// `None` for a test, else whether expanded.
    pub expanded: Option<bool>,
    /// The tests under it (itself for a test).
    pub tests: Vec<String>,
}

pub fn glyph_of(o: Outcome) -> TestGlyph {
    match o {
        Outcome::Passed => TestGlyph::Passed,
        Outcome::Failed => TestGlyph::Failed,
        Outcome::Skipped => TestGlyph::Skipped,
        Outcome::NotRun => TestGlyph::NotRun,
        Outcome::Running => TestGlyph::Running,
    }
}

/// A duration as the tree shows it: `< 1 ms`, `12 ms`, `1.2 sec`.
pub fn duration_text(ms: f64) -> String {
    if ms < 1. {
        "< 1 ms".into()
    } else if ms < 1000. {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} sec", ms / 1000.)
    }
}

/// The search box's filter (see the module docs).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchFilter {
    full_name: Vec<String>,
    outcome: Vec<Outcome>,
    traits: Vec<(String, String)>,
    text: Vec<String>,
}

impl SearchFilter {
    pub fn parse(text: &str) -> Self {
        let mut f = SearchFilter::default();
        for token in text.split_whitespace() {
            let lower = token.to_lowercase();
            if let Some(v) = lower.strip_prefix("fullname:") {
                f.full_name.push(v.to_owned());
            } else if let Some(v) = lower.strip_prefix("outcome:") {
                let o = match v.replace(['-', '_', ' '], "").as_str() {
                    "passed" => Some(Outcome::Passed),
                    "failed" => Some(Outcome::Failed),
                    "skipped" => Some(Outcome::Skipped),
                    "notrun" => Some(Outcome::NotRun),
                    "running" => Some(Outcome::Running),
                    _ => None,
                };
                f.outcome.extend(o);
            } else if let Some(v) = lower.strip_prefix("trait:") {
                let (n, val) = v.split_once('=').unwrap_or(("", v));
                f.traits.push((n.to_owned(), val.to_owned()));
            } else {
                f.text.push(lower);
            }
        }
        f
    }

    pub fn is_empty(&self) -> bool {
        self.full_name.is_empty()
            && self.outcome.is_empty()
            && self.traits.is_empty()
            && self.text.is_empty()
    }

    pub fn matches(&self, t: &TestNode) -> bool {
        let full = t.full_name.to_lowercase();
        let name = t.name.to_lowercase();
        self.full_name.iter().all(|f| full.contains(f))
            && (self.outcome.is_empty() || self.outcome.contains(&t.outcome()))
            && self.traits.iter().all(|(n, v)| {
                t.traits.iter().any(|(tn, tv)| {
                    (n.is_empty() || tn.to_lowercase() == *n) && tv.to_lowercase() == *v
                })
            })
            && self
                .text
                .iter()
                .all(|f| full.contains(f) || name.contains(f))
    }
}

pub struct TestExplorer {
    theme: Theme,
    data: TreeData,
    rows: Vec<Row>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    filter: String,
    search_focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl TestExplorer {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            data: TreeData::default(),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            selected: None,
            filter: String::new(),
            search_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn set_data(&mut self, data: TreeData, cx: &mut Context<Self>) {
        if data != self.data {
            self.data = data;
            self.rebuild();
            cx.notify();
        }
    }

    pub fn set_filter(&mut self, filter: String, cx: &mut Context<Self>) {
        if filter != self.filter {
            self.filter = filter;
            self.rebuild();
            cx.notify();
        }
    }

    /// The visible rows, in order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The tests under the selected row (the Run and Debug buttons, Ctrl+R, T).
    pub fn selected_tests(&self) -> Vec<String> {
        self.selected
            .as_ref()
            .and_then(|k| self.rows.iter().find(|r| &r.key == k))
            .map(|r| r.tests.clone())
            .unwrap_or_default()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn select(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        self.selected = key;
        cx.notify();
    }

    pub fn toggle(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_owned());
        }
        self.rebuild();
        cx.notify();
    }

    /// The selected test's detail text (the detail pane), or a node's count.
    pub fn details(&self) -> Vec<String> {
        let Some(row) = self
            .selected
            .as_ref()
            .and_then(|k| self.rows.iter().find(|r| &r.key == k))
        else {
            return Vec::new();
        };
        let RowKind::Test(id) = &row.kind else {
            return vec![format!("{} ({} tests)", row.label, row.tests.len())];
        };
        let Some(t) = self.data.tests.iter().find(|t| &t.id == id) else {
            return Vec::new();
        };
        let mut out = vec![t.full_name.clone()];
        match &t.result {
            None => out.push("Not Run".into()),
            Some(r) => {
                let what = match r.outcome {
                    Outcome::Passed => "Passed",
                    Outcome::Failed => "Failed",
                    Outcome::Skipped => "Skipped",
                    Outcome::NotRun => "Not Run",
                    Outcome::Running => "Running",
                };
                out.push(match r.duration_ms {
                    Some(ms) => format!("{what} ({})", duration_text(ms)),
                    None => what.into(),
                });
                if let Some(m) = &r.message {
                    out.push(String::new());
                    out.push(if r.outcome == Outcome::Skipped {
                        "Skip reason:".into()
                    } else {
                        "Message:".into()
                    });
                    out.extend(m.lines().map(|l| format!("  {l}")));
                }
                if let Some(s) = &r.stack_trace {
                    out.push(String::new());
                    out.push("Stack Trace:".into());
                    out.extend(s.lines().map(|l| format!("  {}", l.trim_start())));
                }
                if let Some(o) = &r.output {
                    out.push(String::new());
                    out.push("Standard Output:".into());
                    out.extend(o.lines().map(|l| format!("  {l}")));
                }
            }
        }
        if let (Some(s), Some(l)) = (&t.source, t.line) {
            out.push(String::new());
            out.push(format!("Source: {s}, line {l}"));
        }
        out
    }

    fn rebuild(&mut self) {
        let filter = SearchFilter::parse(&self.filter);
        let mut rows = Vec::new();
        for p in &self.data.projects {
            let tests: Vec<&TestNode> = self
                .data
                .tests
                .iter()
                .filter(|t| t.project == p.key && (filter.is_empty() || filter.matches(t)))
                .collect();
            if !filter.is_empty() && tests.is_empty() {
                continue;
            }
            let key = format!("p:{}", p.key);
            let expanded = !self.collapsed.contains(&key);
            let glyph = TestGlyph::aggregate(tests.iter().map(|t| glyph_of(t.outcome())));
            let detail = match p.state {
                NodeState::Discovering => Some("discovering\u{2026}".into()),
                NodeState::Failed => Some(p.message.clone().unwrap_or_else(|| "failed".into())),
                NodeState::Ready => Some(format!("{} tests", tests.len())),
            };
            rows.push(Row {
                key: key.clone(),
                depth: 0,
                label: p.name.clone(),
                glyph,
                detail,
                kind: RowKind::Project(p.key.clone()),
                expanded: Some(expanded),
                tests: tests.iter().map(|t| t.id.clone()).collect(),
            });
            if !expanded {
                continue;
            }
            // The groups: namespace then class (.NET), module path (Rust); a tree of them by path.
            let mut by_group: BTreeMap<Vec<String>, Vec<&TestNode>> = BTreeMap::new();
            for t in &tests {
                by_group.entry(t.group.clone()).or_default().push(t);
            }
            self.emit_groups(&mut rows, &key, &[], 1, &by_group);
        }
        // The selection survives a refresh while its row is shown.
        if let Some(sel) = &self.selected
            && !rows.iter().any(|r| &r.key == sel)
        {
            self.selected = None;
        }
        self.rows = rows;
    }

    /// The rows under group path `prefix` (at `depth`): its subgroups, then its tests.
    fn emit_groups(
        &self,
        rows: &mut Vec<Row>,
        parent: &str,
        prefix: &[String],
        depth: usize,
        by_group: &BTreeMap<Vec<String>, Vec<&TestNode>>,
    ) {
        // Subgroups: the next segment of every group under the prefix.
        let mut children: Vec<String> = by_group
            .keys()
            .filter(|g| g.len() > prefix.len() && g.starts_with(prefix))
            .map(|g| g[prefix.len()].clone())
            .collect();
        children.dedup();
        for child in children {
            let mut path = prefix.to_vec();
            path.push(child.clone());
            let under: Vec<&TestNode> = by_group
                .iter()
                .filter(|(g, _)| g.starts_with(&path))
                .flat_map(|(_, ts)| ts.iter().copied())
                .collect();
            let key = format!("g:{parent}/{}", path.join("/"));
            let expanded = !self.collapsed.contains(&key);
            rows.push(Row {
                key: key.clone(),
                depth,
                label: child,
                glyph: TestGlyph::aggregate(under.iter().map(|t| glyph_of(t.outcome()))),
                detail: Some(format!("{}", under.len())),
                kind: RowKind::Group,
                expanded: Some(expanded),
                tests: under.iter().map(|t| t.id.clone()).collect(),
            });
            if expanded {
                self.emit_groups(rows, parent, &path, depth + 1, by_group);
            }
        }
        if let Some(tests) = by_group.get(prefix) {
            let mut tests = tests.clone();
            tests.sort_by(|a, b| a.name.cmp(&b.name));
            for t in tests {
                rows.push(Row {
                    key: format!("t:{}", t.id),
                    depth,
                    label: t.name.clone(),
                    glyph: glyph_of(t.outcome()),
                    detail: t
                        .result
                        .as_ref()
                        .and_then(|r| r.duration_ms)
                        .map(duration_text),
                    kind: RowKind::Test(t.id.clone()),
                    expanded: None,
                    tests: vec![t.id.clone()],
                });
            }
        }
    }

    fn run(&self, command: &str, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
    }

    fn search_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        let mut text = self.filter.clone();
        match k.key.as_str() {
            "backspace" => {
                text.pop();
            }
            "escape" => text.clear(),
            "space" => text.push(' '),
            _ => {
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if k.modifiers.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                        text.push_str(&c)
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        self.run(
            eludite_commands::test::EXPLORER,
            json!({ "filter": text }),
            window,
            cx,
        );
    }

    fn click(&mut self, ix: usize, count: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        self.selected = Some(row.key.clone());
        cx.notify();
        if count < 2 {
            return;
        }
        match &row.kind {
            RowKind::Test(id) => {
                let Some(t) = self.data.tests.iter().find(|t| &t.id == id) else {
                    return;
                };
                if let Some(source) = &t.source {
                    let mut args = json!({ "path": source });
                    if let Some(l) = t.line {
                        args["line"] = json!(l);
                    }
                    self.run(eludite_commands::workspace::FILE_OPEN, args, window, cx);
                }
            }
            _ => self.toggle(&row.key, cx),
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
                let r = self.rows.get(ix)?;
                let key = r.key.clone();
                let toggle_key = key.clone();
                let entity = cx.entity();
                let style = TreeRowStyle {
                    depth: r.depth,
                    disclosure: r.expanded,
                    selected: self.selected.as_deref() == Some(key.as_str()),
                    muted: false,
                    bold: matches!(r.kind, RowKind::Project(_)),
                };
                Some(
                    test_row(
                        &t,
                        row_selector(ix),
                        r.glyph,
                        r.label.clone(),
                        r.detail.clone(),
                        style,
                        move |_, _, cx| {
                            entity.update(cx, |w, cx| w.toggle(&toggle_key, cx));
                        },
                    )
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

impl Render for TestExplorer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let running = self.data.running;
        let has_selection = !self.selected_tests().is_empty();
        let button = |id: &'static str,
                      label: &'static str,
                      enabled: bool,
                      command: &'static str,
                      args: Value| {
            let b = toolbar_button(id, label, enabled, &t);
            if enabled {
                b.on_click(cx.listener(move |this, _, window, cx| {
                    this.run(command, args.clone(), window, cx)
                }))
            } else {
                b
            }
        };
        let run = eludite_commands::test::RUN;
        // The buttons wrap when the window is narrow (it docks left, as Visual Studio's does).
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
            .child(button(
                RUN_ALL,
                "\u{25B6}\u{25B6} Run All",
                !running,
                run,
                json!({}),
            ))
            .child(button(
                RUN,
                "\u{25B6} Run",
                !running && has_selection,
                run,
                json!({ "selection": true }),
            ))
            .child(button(
                DEBUG,
                "Debug",
                !running && has_selection,
                eludite_commands::test::DEBUG,
                json!({ "selection": true }),
            ))
            .child(button(
                RUN_FAILED,
                "Run Failed Tests",
                !running,
                run,
                json!({ "failed_only": true }),
            ))
            .child(button(
                REPEAT,
                "Repeat Last Run",
                !running,
                run,
                json!({ "repeat_last": true }),
            ))
            .child(button(
                CANCEL,
                "Cancel",
                running,
                eludite_commands::test::CANCEL,
                json!({}),
            ))
            .child({
                let counts = self.data.summary.as_ref().map(|(_, s)| s);
                let (p, f, s) = counts.map_or((0, 0, 0), |c| (c.passed, c.failed, c.skipped));
                div()
                    .id(SUMMARY)
                    .debug_selector(|| SUMMARY.into())
                    .flex()
                    .flex_none()
                    .gap_2()
                    .text_size(t.typography.ui)
                    .child(
                        div()
                            .text_color(TestGlyph::Passed.color())
                            .child(format!("{} {p}", TestGlyph::Passed.glyph())),
                    )
                    .child(
                        div()
                            .text_color(TestGlyph::Failed.color())
                            .child(format!("{} {f}", TestGlyph::Failed.glyph())),
                    )
                    .child(
                        div()
                            .text_color(TestGlyph::Skipped.color())
                            .child(format!("{} {s}", TestGlyph::Skipped.glyph())),
                    )
            });
        let focused = self.search_focus.is_focused(window);
        let search = text_box(
            SEARCH_BOX,
            &self.filter,
            "Search Test Explorer",
            focused,
            &t,
        )
        .w_full()
        .track_focus(&self.search_focus)
        .key_context("TestExplorerSearch")
        .on_key_down(cx.listener(Self::search_key))
        .on_click(cx.listener(|this, _, window, cx| {
            this.search_focus.focus(window, cx);
            cx.notify();
        }));
        let status: Option<String> = match self.data.phase {
            Phase::Idle if self.data.projects.is_empty() => Some(
                "Build your solution to discover tests, or run them (Run All Tests, Ctrl+R, A)."
                    .into(),
            ),
            Phase::Building => Some("Building\u{2026}".into()),
            Phase::Discovering => Some("Discovering tests\u{2026}".into()),
            _ => self.data.message.clone(),
        };
        let summary_line = self.data.summary.as_ref().map(|(text, _)| text.clone());
        let count = self.rows.len();
        let details = self.details();
        div()
            .id("test-explorer")
            .debug_selector(|| "test-explorer".into())
            .size_full()
            .flex()
            .flex_col()
            .text_color(t.text)
            .child(toolbar)
            .child(div().flex_none().p_1().child(search))
            .children(status.map(|s| {
                div()
                    .flex_none()
                    .px_2()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(s)
            }))
            .child(
                uniform_list(
                    "test-explorer-rows",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .child(
                div()
                    .id(DETAILS)
                    .debug_selector(|| DETAILS.into())
                    .flex_none()
                    .h(px(160.))
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(t.border)
                    .p_1()
                    .text_size(t.typography.small)
                    .children(
                        summary_line.map(|s| div().font_weight(FontWeight::SEMIBOLD).child(s)),
                    )
                    .children(
                        details
                            .into_iter()
                            .map(|l| div().whitespace_nowrap().child(SharedString::from(l))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::test_runs::{Proto, ResultData};
    use gpui::AppContext as _;

    fn test(
        id: &str,
        name: &str,
        full: &str,
        group: &[&str],
        outcome: Option<Outcome>,
    ) -> TestNode {
        TestNode {
            id: format!("p|{id}"),
            runner_id: id.into(),
            project: "p".into(),
            name: name.into(),
            full_name: full.into(),
            group: group.iter().map(|s| (*s).to_owned()).collect(),
            source: Some("/s/C.cs".into()),
            line: Some(3),
            traits: if id == "t3" {
                vec![("Category".into(), "Math".into())]
            } else {
                vec![]
            },
            result: outcome.map(|o| ResultData {
                outcome: o,
                duration_ms: Some(12.),
                message: (o == Outcome::Failed).then(|| "boom\nsecond".into()),
                stack_trace: (o == Outcome::Failed)
                    .then(|| "   at N.C.B() in /s/C.cs:line 9".into()),
                output: None,
                failure: None,
            }),
            cargo: None,
        }
    }

    fn data() -> TreeData {
        TreeData {
            projects: vec![ProjectNode {
                key: "p".into(),
                name: "Corpus".into(),
                path: "/s/Corpus.csproj".into(),
                protocol: Proto::Mtp,
                target_framework: Some("net10.0".into()),
                runtime: None,
                program: None,
                state: NodeState::Ready,
                message: None,
                cargo: None,
            }],
            tests: vec![
                test("t1", "A", "N.C.A", &["N", "C"], Some(Outcome::Passed)),
                test("t2", "B", "N.C.B", &["N", "C"], Some(Outcome::Failed)),
                test("t3", "M", "N.D.M", &["N", "D"], None),
            ],
            phase: Phase::Ready,
            message: None,
            summary: None,
            running: false,
        }
    }

    #[gpui::test]
    fn the_tree_groups_filters_and_details(cx: &mut gpui::TestAppContext) {
        let w = cx.new(|cx| TestExplorer::new(Theme::dark(), cx));
        w.update(cx, |w, cx| w.set_data(data(), cx));
        let labels = |w: &TestExplorer| {
            w.rows()
                .iter()
                .map(|r| format!("{}{} {:?}", "  ".repeat(r.depth), r.label, r.glyph))
                .collect::<Vec<_>>()
        };
        w.read_with(cx, |w, _| {
            assert_eq!(
                labels(w),
                [
                    "Corpus Failed",
                    "  N Failed",
                    "    C Failed",
                    "      A Passed",
                    "      B Failed",
                    "    D NotRun",
                    "      M NotRun"
                ]
            );
            assert_eq!(w.rows()[3].detail.as_deref(), Some("12 ms"));
        });
        w.update(cx, |w, cx| w.set_filter("Outcome:Failed".into(), cx));
        w.read_with(cx, |w, _| assert_eq!(w.rows().last().unwrap().label, "B"));
        w.update(cx, |w, cx| w.set_filter("Trait:Category=Math".into(), cx));
        w.read_with(cx, |w, _| assert_eq!(w.rows().last().unwrap().label, "M"));
        w.update(cx, |w, cx| w.set_filter("FullName:N.C a".into(), cx));
        w.read_with(cx, |w, _| assert_eq!(w.rows().len(), 4));
        w.update(cx, |w, cx| {
            w.set_filter(String::new(), cx);
            w.toggle("g:p:p/N/C", cx);
        });
        w.read_with(cx, |w, _| assert_eq!(w.rows().len(), 5));
        w.update(cx, |w, cx| {
            w.toggle("g:p:p/N/C", cx);
            w.select(Some("t:p|t2".into()), cx);
        });
        w.read_with(cx, |w, _| {
            assert_eq!(w.selected_tests(), ["p|t2"]);
            let d = w.details();
            assert_eq!(d[0], "N.C.B");
            assert_eq!(d[1], "Failed (12 ms)");
            assert!(d.contains(&"  boom".to_owned()));
            assert!(d.contains(&"  at N.C.B() in /s/C.cs:line 9".to_owned()));
        });
        w.update(cx, |w, cx| w.select(Some("g:p:p/N".into()), cx));
        w.read_with(cx, |w, _| assert_eq!(w.selected_tests().len(), 3));
        assert_eq!(duration_text(0.4), "< 1 ms");
        assert_eq!(duration_text(1500.), "1.5 sec");
    }
}
