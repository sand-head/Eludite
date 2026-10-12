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
//!
//! Brief 0076: the tree groups by the setting test.groupBy (the toolbar's Group By menu: project, namespace, class,
//! outcome, duration, traits, target framework); a method with data rows is a node of its own; rows have a context menu
//! (Run, Debug, Analyze Code Coverage, Go To Test, Copy) and the keyboard (Up, Down, Left, Right, Home, End, Enter,
//! F12, Ctrl+C); the details pane's stack frames open their file, Copy copies it, and its top border resizes it; the
//! search box has a caret, paste and quoted values (`Outcome:"Not Run"`); Sort By Duration and Run Tests After Build
//! (test.runAfterBuild) are toolbar toggles.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use eludite_commands::test::{Outcome, RunSummary};
use eludite_ui::{
    RunCommand, TestGlyph, Theme, TreeRowStyle, menu_row, test_row, text_box, toolbar_button,
};
use gpui::{
    App, ClipboardItem, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point,
    Render, SharedString, StatefulInteractiveElement, Styled, UniformListScrollHandle, Window,
    anchored, deferred, div, px, uniform_list,
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
/// Brief 0076.
pub const TREE: &str = "test-explorer-tree";
pub const GROUP_BY: &str = "test-explorer-group-by";
pub const COVERAGE: &str = "test-explorer-coverage";
pub const RUN_AFTER_BUILD: &str = "test-explorer-run-after-build";
pub const SORT_DURATION: &str = "test-explorer-sort-duration";
pub const COPY_DETAILS: &str = "test-explorer-copy-details";
pub const DETAILS_RESIZE: &str = "test-explorer-details-resize";
pub const CONTEXT_MENU: &str = "test-explorer-context-menu";

/// The context menu's items (brief 0076): id and label.
pub const MENU_ITEMS: [(&str, &str); 5] = [
    ("run", "Run"),
    ("debug", "Debug"),
    ("coverage", "Analyze Code Coverage"),
    ("go", "Go To Test\tF12"),
    ("copy", "Copy\tCtrl+C"),
];

/// Debug selector of visible row `ix`.
pub fn row_selector(ix: usize) -> String {
    format!("test-explorer-row-{ix}")
}

/// Debug selector of the Group By menu's item `mode`.
pub fn group_selector(mode: GroupBy) -> String {
    format!("test-explorer-group-{}", mode.as_str())
}

/// Debug selector of the context menu's item `id`.
pub fn menu_selector(id: &str) -> String {
    format!("test-explorer-menu-{id}")
}

/// Debug selector of line `ix` of the details pane.
pub fn detail_selector(ix: usize) -> String {
    format!("test-explorer-detail-{ix}")
}

/// How the tree groups (the setting test.groupBy, brief 0076).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    #[default]
    Project,
    Namespace,
    Class,
    Outcome,
    Duration,
    Traits,
    TargetFramework,
}

impl GroupBy {
    pub const ALL: [GroupBy; 7] = [
        GroupBy::Project,
        GroupBy::Namespace,
        GroupBy::Class,
        GroupBy::Outcome,
        GroupBy::Duration,
        GroupBy::Traits,
        GroupBy::TargetFramework,
    ];

    pub fn parse(s: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|g| g.as_str() == s)
            .unwrap_or_default()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GroupBy::Project => "project",
            GroupBy::Namespace => "namespace",
            GroupBy::Class => "class",
            GroupBy::Outcome => "outcome",
            GroupBy::Duration => "duration",
            GroupBy::Traits => "traits",
            GroupBy::TargetFramework => "target_framework",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::Project => "Project",
            GroupBy::Namespace => "Namespace",
            GroupBy::Class => "Class",
            GroupBy::Outcome => "Outcome",
            GroupBy::Duration => "Duration",
            GroupBy::Traits => "Traits",
            GroupBy::TargetFramework => "Target Framework",
        }
    }
}

/// What the shell hands the window.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeData {
    /// Brief 0076: the grouping, and whether the tree is from before the last build, generation or session. First, so
    /// a regroup compares unequal before the tests are compared.
    pub group_by: GroupBy,
    pub stale: bool,
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
            group_by: GroupBy::Project,
            stale: false,
        }
    }
}

/// What a row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Project(String),
    Group,
    /// Brief 0076: a method with data rows.
    Method,
    Test(String),
}

/// A visible row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Stable across refreshes: `p:<project>`, `g:<segment ids joined by />`, `t:<test id>` (a test listed under
    /// several traits: `t:<trait>|<test id>`).
    pub key: String,
    pub depth: usize,
    pub label: String,
    pub glyph: TestGlyph,
    pub detail: Option<String>,
    pub kind: RowKind,
    /// `None` for a test, else whether expanded.
    pub expanded: Option<bool>,
    /// The tests under it (itself for a test), as indices into the window's tests ([`TestExplorer::ids`]).
    pub tests: Vec<usize>,
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

/// Brief 0076: the search text's tokens: white space separates them except inside double quotes
/// (`Outcome:"Not Run"`), and `Outcome:Not Run` joins `Run` to the outcome before it.
fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    let mut joined: Vec<String> = Vec::new();
    for t in out {
        if t.eq_ignore_ascii_case("run")
            && joined
                .last()
                .is_some_and(|l| l.to_lowercase() == "outcome:not")
        {
            joined.last_mut().unwrap().push_str(" Run");
        } else {
            joined.push(t);
        }
    }
    joined
}

impl SearchFilter {
    pub fn parse(text: &str) -> Self {
        let mut f = SearchFilter::default();
        for token in tokens(text) {
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

/// A frame of a details line: `at ... in <file>:line <n>` (.NET) or `<file>:<line>:<col>` (Rust panics).
pub fn frame_of(line: &str) -> Option<(String, u32)> {
    if let Some((_, rest)) = line.split_once(" in ")
        && let Some((file, n)) = rest.rsplit_once(":line ")
    {
        return Some((file.trim().to_owned(), n.trim().parse().ok()?));
    }
    // `src/lib.rs:25:9` (libtest's panic location), possibly after `panicked at `.
    let t = line
        .trim()
        .trim_start_matches("panicked at ")
        .trim_end_matches(':');
    let mut parts = t.rsplitn(3, ':');
    let col = parts.next()?;
    let row = parts.next()?;
    let file = parts.next()?;
    if col.parse::<u32>().is_ok() && file.ends_with(".rs") && !file.contains(' ') {
        return Some((file.to_owned(), row.parse().ok()?));
    }
    None
}

/// What a segment of a test's path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegKind {
    Project,
    Group,
    Method,
}

/// One segment of a test's path in the tree.
#[derive(Clone)]
struct Seg<'a> {
    /// Unique among its siblings, part of the row key.
    id: Cow<'a, str>,
    label: Cow<'a, str>,
    /// Siblings sort by it.
    order: Cow<'a, str>,
    kind: SegKind,
    /// A project segment's project key.
    project: Option<&'a str>,
}

impl<'a> Seg<'a> {
    fn group(g: &'a str) -> Self {
        Seg {
            id: Cow::Borrowed(g),
            label: Cow::Borrowed(g),
            order: Cow::Borrowed(g),
            kind: SegKind::Group,
            project: None,
        }
    }
}

/// A node of the tree being built.
struct TNode<'a> {
    label: String,
    kind: SegKind,
    project: Option<&'a str>,
    key: String,
    order: String,
    children: Vec<usize>,
    child_index: HashMap<String, usize>,
    /// Every test under it, in insertion order.
    tests: Vec<usize>,
    /// Its own tests (leaves).
    leaves: Vec<usize>,
}

impl<'a> TNode<'a> {
    fn root() -> Self {
        TNode {
            label: String::new(),
            kind: SegKind::Group,
            project: None,
            key: String::new(),
            order: String::new(),
            children: Vec::new(),
            child_index: HashMap::new(),
            tests: Vec::new(),
            leaves: Vec::new(),
        }
    }

    /// The child of `parent` for `seg`, made when missing.
    fn child(arena: &mut Vec<TNode<'a>>, parent: usize, seg: &Seg<'a>) -> usize {
        if let Some(&c) = arena[parent].child_index.get(seg.id.as_ref()) {
            return c;
        }
        let key = if parent == 0 {
            match seg.kind {
                SegKind::Project => seg.id.to_string(),
                _ => format!("g:{}", seg.id),
            }
        } else {
            let base = &arena[parent].key;
            match base.strip_prefix("p:") {
                Some(k) => format!("g:p:{k}/{}", seg.id),
                None => format!("{base}/{}", seg.id),
            }
        };
        let id = arena.len();
        arena.push(TNode {
            label: seg.label.to_string(),
            kind: seg.kind,
            project: seg.project,
            key,
            order: seg.order.to_string(),
            children: Vec::new(),
            child_index: HashMap::new(),
            tests: Vec::new(),
            leaves: Vec::new(),
        });
        arena[parent].children.push(id);
        arena[parent].child_index.insert(seg.id.to_string(), id);
        id
    }
}

pub struct TestExplorer {
    theme: Theme,
    data: TreeData,
    rows: Vec<Row>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    filter: String,
    /// Brief 0076: the search box's caret (a char index), the tree's focus, the open menus, the details pane.
    caret: usize,
    search_focus: FocusHandle,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    menu: Option<(usize, Point<Pixels>)>,
    group_menu: bool,
    sort_duration: bool,
    run_after_build: bool,
    details_height: f32,
    resizing: Option<(f32, f32)>,
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
            caret: 0,
            search_focus: cx.focus_handle(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            menu: None,
            group_menu: false,
            sort_duration: false,
            run_after_build: false,
            details_height: 160.,
            resizing: None,
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
            // A filter set from elsewhere (an agent, the menu) puts the caret at its end.
            if self.caret > filter.chars().count() || !filter.starts_with(&self.filter) {
                self.caret = filter.chars().count();
            }
            self.filter = filter;
            self.rebuild();
            cx.notify();
        }
    }

    /// Brief 0076: the setting test.runAfterBuild as the toolbar's toggle shows it.
    pub fn set_run_after_build(&mut self, on: bool, cx: &mut Context<Self>) {
        if on != self.run_after_build {
            self.run_after_build = on;
            cx.notify();
        }
    }

    /// Whether the window (its tree, toolbar, search box or details) has focus.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.search_focus.is_focused(window)
    }

    /// The visible rows, in order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn details_height(&self) -> f32 {
        self.details_height
    }

    /// The tests under the selected row (the Run and Debug buttons, Ctrl+R, T).
    pub fn selected_tests(&self) -> Vec<String> {
        self.selected
            .as_ref()
            .and_then(|k| self.rows.iter().find(|r| &r.key == k))
            .map(|r| self.ids(r))
            .unwrap_or_default()
    }

    /// The ids of a row's tests.
    pub fn ids(&self, row: &Row) -> Vec<String> {
        row.tests
            .iter()
            .filter_map(|&i| self.data.tests.get(i).map(|t| t.id.clone()))
            .collect()
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

    /// The path of `t` in the tree for the grouping (`projects`: each project's segment, made once per rebuild).
    fn path_of<'a>(
        &self,
        t: &'a TestNode,
        projects: &HashMap<&str, Seg<'a>>,
        trait_: Option<&'a str>,
    ) -> Vec<Seg<'a>> {
        let project = || {
            projects.get(t.project.as_str()).cloned().unwrap_or(Seg {
                id: Cow::Owned(format!("p:{}", t.project)),
                label: Cow::Borrowed(&t.project),
                order: Cow::Borrowed(""),
                kind: SegKind::Project,
                project: Some(&t.project),
            })
        };
        let groups = || t.group.iter().map(|g| Seg::group(g));
        let top = |label: &'a str, order: &'a str| Seg {
            id: Cow::Borrowed(label),
            label: Cow::Borrowed(label),
            order: Cow::Borrowed(order),
            kind: SegKind::Group,
            project: None,
        };
        let mut path = Vec::with_capacity(t.group.len() + 3);
        match self.data.group_by {
            GroupBy::Project => {}
            GroupBy::Namespace => {
                path.extend(groups());
                return path;
            }
            GroupBy::Class => {
                match t.group.last() {
                    Some(c) => path.push(Seg::group(c)),
                    None => path.push(project()),
                }
                return path;
            }
            GroupBy::Outcome => {
                let (label, order) = match t.outcome() {
                    Outcome::Failed => ("Failed Tests", "0"),
                    Outcome::Running => ("Running Tests", "1"),
                    Outcome::NotRun => ("Not Run Tests", "2"),
                    Outcome::Skipped => ("Skipped Tests", "3"),
                    Outcome::Passed => ("Passed Tests", "4"),
                };
                path.push(top(label, order));
            }
            GroupBy::Duration => {
                let (label, order) = match t.result.as_ref().and_then(|r| r.duration_ms) {
                    Some(ms) if ms >= 1000. => ("Slow (> 1 sec)", "0"),
                    Some(ms) if ms >= 100. => ("Medium (> 100 ms)", "1"),
                    Some(_) => ("Fast (< 100 ms)", "2"),
                    None => ("Not Run", "3"),
                };
                path.push(top(label, order));
            }
            GroupBy::Traits => match trait_ {
                Some(tr) => path.push(Seg {
                    order: Cow::Owned(format!("0{tr}")),
                    ..top(tr, "")
                }),
                None => path.push(top("No Traits", "1")),
            },
            GroupBy::TargetFramework => {
                let tfm = projects
                    .get(t.project.as_str())
                    .and_then(|p| p.project)
                    .and_then(|k| self.data.projects.iter().find(|p| p.key == k))
                    .and_then(|p| p.target_framework.as_deref())
                    .unwrap_or("No Target Framework");
                path.push(Seg {
                    id: Cow::Owned(tfm.to_owned()),
                    label: Cow::Owned(tfm.to_owned()),
                    order: Cow::Owned(tfm.to_owned()),
                    kind: SegKind::Group,
                    project: None,
                });
            }
        }
        path.push(project());
        path.extend(groups());
        path
    }

    fn rebuild(&mut self) {
        let filter = SearchFilter::parse(&self.filter);
        let data = &self.data;
        let names: Vec<(String, String)> = data
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (format!("p:{}", p.key), format!("{i:08}")))
            .collect();
        let projects: HashMap<&str, Seg> = data
            .projects
            .iter()
            .zip(&names)
            .map(|(p, (id, order))| {
                (
                    p.key.as_str(),
                    Seg {
                        id: Cow::Borrowed(id.as_str()),
                        label: Cow::Borrowed(p.name.as_str()),
                        order: Cow::Borrowed(order.as_str()),
                        kind: SegKind::Project,
                        project: Some(p.key.as_str()),
                    },
                )
            })
            .collect();
        let mut arena: Vec<TNode> = vec![TNode::root()];
        // Project grouping lists every project, discovered or not, while no filter hides them.
        if data.group_by == GroupBy::Project && filter.is_empty() {
            for p in &data.projects {
                TNode::child(&mut arena, 0, &projects[p.key.as_str()]);
            }
        }
        // Methods with data rows: several tests share the full name, or a name carries arguments.
        let mut per_method: HashMap<&str, u32> = HashMap::with_capacity(data.tests.len());
        for t in &data.tests {
            *per_method.entry(t.full_name.as_str()).or_default() += 1;
        }
        let traits_mode = data.group_by == GroupBy::Traits;
        let mut trait_labels: Vec<String> = Vec::new();
        for t in data.tests.iter().filter(|_| traits_mode) {
            for (n, v) in &t.traits {
                trait_labels.push(if v.is_empty() {
                    n.clone()
                } else {
                    format!("{n}={v}")
                });
            }
        }
        let mut trait_at = 0;
        let mut last: Vec<(Cow<str>, usize)> = Vec::new();
        for (ti, t) in data.tests.iter().enumerate() {
            let first_trait = trait_at;
            if traits_mode {
                trait_at += t.traits.len();
            }
            if !(filter.is_empty() || filter.matches(t)) {
                continue;
            }
            let listed = &trait_labels[first_trait..trait_at];
            let traits = listed
                .iter()
                .map(|s| Some(s.as_str()))
                .chain(listed.is_empty().then_some(None));
            for tr in traits {
                let mut path = self.path_of(t, &projects, tr);
                let rows = per_method[t.full_name.as_str()];
                if t.cargo.is_none() && (rows > 1 || t.name.contains('(')) {
                    let method = t.full_name.rsplit('.').next().unwrap_or(&t.full_name);
                    path.push(Seg {
                        id: Cow::Owned(format!("m:{method}")),
                        label: Cow::Borrowed(method),
                        order: Cow::Borrowed(method),
                        kind: SegKind::Method,
                        project: None,
                    });
                }
                // Consecutive tests mostly share their path: reuse the nodes of the last one while the ids match.
                let mut at = 0;
                for (depth, seg) in path.iter().enumerate() {
                    at = match last.get(depth) {
                        Some((id, node)) if id.as_ref() == seg.id.as_ref() => *node,
                        _ => {
                            last.truncate(depth);
                            let n = TNode::child(&mut arena, at, seg);
                            last.push((seg.id.clone(), n));
                            n
                        }
                    };
                    arena[at].tests.push(ti);
                }
                arena[at].leaves.push(ti);
            }
        }
        let mut rows = Vec::with_capacity(data.tests.len() + arena.len());
        self.emit(&arena, 0, 0, &mut rows);
        // The selection survives a refresh while its row is shown.
        if let Some(sel) = &self.selected
            && !rows.iter().any(|r| &r.key == sel)
        {
            self.selected = None;
        }
        self.rows = rows;
    }

    /// The rows under node `parent` of `arena`, at `depth`.
    fn emit(&self, arena: &[TNode], parent: usize, depth: usize, rows: &mut Vec<Row>) {
        let nodes = &arena[parent].children;
        let data = &self.data;
        let traits_mode = data.group_by == GroupBy::Traits;
        let duration = |i: usize| data.tests[i].result.as_ref().and_then(|r| r.duration_ms);
        // Groups first; then methods and tests together, by name (or by duration, longest first).
        let mut groups: Vec<usize> = Vec::new();
        let mut items: Vec<(Option<usize>, Option<usize>)> = Vec::new();
        for &n in nodes.iter() {
            if arena[n].kind == SegKind::Method {
                items.push((Some(n), None));
            } else {
                groups.push(n);
            }
        }
        groups.sort_by(|a, b| arena[*a].order.cmp(&arena[*b].order));
        let emit_node = |n: usize, rows: &mut Vec<Row>| {
            let node = &arena[n];
            let mut tests: Vec<usize> = node.tests.clone();
            if traits_mode {
                let mut seen = HashSet::new();
                tests.retain(|i| seen.insert(*i));
            }
            let expanded = !self.collapsed.contains(&node.key);
            let glyph =
                TestGlyph::aggregate(tests.iter().map(|&i| glyph_of(data.tests[i].outcome())));
            let mut any = false;
            let sum: f64 = tests
                .iter()
                .filter_map(|&i| duration(i))
                .inspect(|_| any = true)
                .sum();
            let count = tests.len();
            let detail = match node.project.filter(|_| depth == 0) {
                Some(key) => {
                    let p = data.projects.iter().find(|p| p.key == key);
                    match p.map(|p| p.state) {
                        Some(NodeState::Discovering) => Some("discovering\u{2026}".into()),
                        Some(NodeState::Failed) => Some(
                            p.and_then(|p| p.message.clone())
                                .unwrap_or_else(|| "failed".into()),
                        ),
                        _ if any => Some(format!("{count} tests \u{00B7} {}", duration_text(sum))),
                        _ => Some(format!("{count} tests")),
                    }
                }
                None if any => Some(format!("{count} \u{00B7} {}", duration_text(sum))),
                None => Some(format!("{count}")),
            };
            rows.push(Row {
                key: node.key.clone(),
                depth,
                label: node.label.clone(),
                glyph,
                detail,
                kind: match (node.kind, node.project) {
                    (SegKind::Project, Some(k)) => RowKind::Project(k.to_owned()),
                    (SegKind::Method, _) => RowKind::Method,
                    _ => RowKind::Group,
                },
                expanded: Some(expanded),
                tests,
            });
            if expanded {
                self.emit(arena, n, depth + 1, rows);
            }
        };
        for n in groups {
            emit_node(n, rows);
        }
        for &i in &arena[parent].leaves {
            items.push((None, Some(i)));
        }
        let name = |it: &(Option<usize>, Option<usize>)| match it {
            (Some(n), _) => arena[*n].label.as_str(),
            (_, Some(i)) => data.tests[*i].name.as_str(),
            _ => "",
        };
        let total = |it: &(Option<usize>, Option<usize>)| match it {
            (Some(n), _) => arena[*n].tests.iter().filter_map(|&i| duration(i)).sum(),
            (_, Some(i)) => duration(*i).unwrap_or(-1.),
            _ => -1.,
        };
        if self.sort_duration {
            items.sort_by(|a, b| total(b).total_cmp(&total(a)));
        } else {
            items.sort_by(|a, b| name(a).cmp(name(b)));
        }
        let trait_prefix = traits_mode.then(|| arena[parent].key.clone());
        for it in items {
            match it {
                (Some(n), _) => emit_node(n, rows),
                (_, Some(i)) => {
                    let t = &data.tests[i];
                    rows.push(Row {
                        key: match &trait_prefix {
                            Some(p) => format!("t:{p}|{}", t.id),
                            None => format!("t:{}", t.id),
                        },
                        depth,
                        label: t.name.clone(),
                        glyph: glyph_of(t.outcome()),
                        detail: duration(i).map(duration_text),
                        kind: RowKind::Test(t.id.clone()),
                        expanded: None,
                        tests: vec![i],
                    });
                }
                _ => {}
            }
        }
    }

    fn run(&self, command: &str, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
    }

    fn set_setting(&self, key: &str, value: Value, window: &mut Window, cx: &mut App) {
        self.run(
            eludite_commands::settings::SET,
            json!({ "key": key, "value": value }),
            window,
            cx,
        );
    }

    /// Brief 0076: the search box edits at its caret; Ctrl+V pastes.
    fn search_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let mut chars: Vec<char> = self.filter.chars().collect();
        let mut caret = self.caret.min(chars.len());
        let paste = (k.modifiers.control || k.modifiers.platform) && k.key == "v";
        if !paste && (k.modifiers.control || k.modifiers.alt || k.modifiers.platform) {
            return;
        }
        match k.key.as_str() {
            _ if paste => {
                let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
                    return;
                };
                let text = text.replace(['\n', '\r'], " ");
                for (i, c) in text.chars().enumerate() {
                    chars.insert(caret + i, c);
                }
                caret += text.chars().count();
            }
            "backspace" => {
                if caret > 0 {
                    chars.remove(caret - 1);
                    caret -= 1;
                }
            }
            "delete" => {
                if caret < chars.len() {
                    chars.remove(caret);
                }
            }
            "left" => caret = caret.saturating_sub(1),
            "right" => caret = (caret + 1).min(chars.len()),
            "home" => caret = 0,
            "end" => caret = chars.len(),
            "escape" => {
                chars.clear();
                caret = 0;
            }
            "space" => {
                chars.insert(caret, ' ');
                caret += 1;
            }
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
                        for (i, ch) in c.chars().enumerate() {
                            chars.insert(caret + i, ch);
                        }
                        caret += c.chars().count();
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        self.caret = caret;
        let text: String = chars.into_iter().collect();
        cx.notify();
        if text != self.filter {
            self.run(
                eludite_commands::test::EXPLORER,
                json!({ "filter": text }),
                window,
                cx,
            );
        }
    }

    fn row_index(&self) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        self.rows.iter().position(|r| &r.key == sel)
    }

    fn select_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.rows.get(ix) {
            self.selected = Some(r.key.clone());
            self.scroll.scroll_to_item(ix, gpui::ScrollStrategy::Top);
            cx.notify();
        }
    }

    /// Brief 0076: the tree's keys.
    fn tree_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let ctrl = k.modifiers.control || k.modifiers.platform;
        let ix = self.row_index();
        let last = self.rows.len().saturating_sub(1);
        match k.key.as_str() {
            "down" => self.select_index(ix.map_or(0, |i| (i + 1).min(last)), cx),
            "up" => self.select_index(ix.map_or(0, |i| i.saturating_sub(1)), cx),
            "home" => self.select_index(0, cx),
            "end" => self.select_index(last, cx),
            "left" => {
                let Some(i) = ix else { return };
                let row = self.rows[i].clone();
                if row.expanded == Some(true) {
                    self.toggle(&row.key, cx);
                } else if let Some(parent) = (0..i).rev().find(|&j| self.rows[j].depth < row.depth)
                {
                    self.select_index(parent, cx);
                }
            }
            "right" => {
                let Some(i) = ix else { return };
                let row = self.rows[i].clone();
                match row.expanded {
                    Some(false) => self.toggle(&row.key, cx),
                    Some(true) if i < last => self.select_index(i + 1, cx),
                    _ => {}
                }
            }
            "enter" if ctrl => self.menu_action("go", window, cx),
            "enter" => self.menu_action("run", window, cx),
            "f12" => self.menu_action("go", window, cx),
            "c" if ctrl => self.menu_action("copy", window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The text Copy puts on the clipboard: a test's full name, or the names of a node's tests.
    pub fn copy_text(&self) -> Option<String> {
        let row = self.rows.get(self.row_index()?)?;
        let names: Vec<String> = row
            .tests
            .iter()
            .filter_map(|&i| self.data.tests.get(i))
            .map(|t| {
                if t.name.contains('(') {
                    let prefix = t.full_name.rsplit_once('.').map_or("", |(p, _)| p);
                    format!("{prefix}.{}", t.name)
                } else {
                    t.full_name.clone()
                }
            })
            .collect();
        (!names.is_empty()).then(|| names.join("\n"))
    }

    /// Run a context menu item (or its key) on the selected row.
    pub fn menu_action(&mut self, item: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
        let Some(row) = self.row_index().and_then(|i| self.rows.get(i)).cloned() else {
            return;
        };
        let ids = json!(self.ids(&row));
        match item {
            "run" if !row.tests.is_empty() => self.run(
                eludite_commands::test::RUN,
                json!({ "ids": ids }),
                window,
                cx,
            ),
            "debug" if !row.tests.is_empty() => self.run(
                eludite_commands::test::DEBUG,
                json!({ "ids": ids }),
                window,
                cx,
            ),
            "coverage" if !row.tests.is_empty() => self.run(
                eludite_commands::test::COVERAGE,
                json!({ "ids": ids }),
                window,
                cx,
            ),
            "go" => {
                let t = row
                    .tests
                    .iter()
                    .filter_map(|&i| self.data.tests.get(i))
                    .find(|t| t.source.is_some());
                if let Some(t) = t
                    && let Some(source) = &t.source
                {
                    let mut args = json!({ "path": source });
                    if let Some(l) = t.line {
                        args["line"] = json!(l);
                    }
                    self.run(eludite_commands::workspace::FILE_OPEN, args, window, cx);
                }
            }
            "copy" => {
                if let Some(text) = self.copy_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, ix: usize, count: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        self.selected = Some(row.key.clone());
        self.focus.focus(window, cx);
        cx.notify();
        if count < 2 {
            return;
        }
        match &row.kind {
            RowKind::Test(_) => self.menu_action("go", window, cx),
            _ => self.toggle(&row.key, cx),
        }
    }

    /// Brief 0076: a right-click selects the row and opens its context menu.
    pub fn open_menu(&mut self, ix: usize, at: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(r) = self.rows.get(ix) {
            self.selected = Some(r.key.clone());
            self.menu = Some((ix, at));
            cx.notify();
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
                    muted: self.data.stale,
                    bold: matches!(r.kind, RowKind::Project(_)) && r.depth == 0,
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
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            this.focus.focus(window, cx);
                            this.open_menu(ix, e.position, cx);
                        }),
                    )
                    .into_any_element(),
                )
            })
            .collect()
    }

    fn context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (_, at) = self.menu?;
        let t = self.theme;
        let items: Vec<gpui::AnyElement> = MENU_ITEMS
            .iter()
            .map(|(id, label)| {
                let id = *id;
                menu_row(menu_selector(id), *label, 0, false, false, false, &t)
                    .min_w(px(200.))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.menu_action(id, window, cx)),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            deferred(
                anchored().position(at).child(
                    eludite_ui::popup::popup_panel(&t)
                        .id(CONTEXT_MENU)
                        .debug_selector(|| CONTEXT_MENU.into())
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

    fn group_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.group_menu {
            return None;
        }
        let t = self.theme;
        let current = self.data.group_by;
        let items: Vec<gpui::AnyElement> = GroupBy::ALL
            .into_iter()
            .map(|g| {
                let label = if g == current {
                    format!("\u{2713} {}", g.label())
                } else {
                    format!("   {}", g.label())
                };
                menu_row(group_selector(g), label, 0, false, false, false, &t)
                    .min_w(px(180.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.group_menu = false;
                        cx.notify();
                        this.set_setting("test.groupBy", json!(g.as_str()), window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        Some(
            deferred(
                anchored().child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("test-explorer-group-menu")
                        .occlude()
                        .mt(px(22.))
                        .py_1()
                        .text_size(t.typography.ui)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.group_menu = false;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(3),
        )
    }

    /// The details pane's copy (its lines joined).
    pub fn details_text(&self) -> String {
        self.details().join("\n")
    }

    /// Brief 0076: set the details pane's height (the drag on its top border), kept between 80 and 600 px.
    pub fn set_details_height(&mut self, h: f32, cx: &mut Context<Self>) {
        self.details_height = h.clamp(80., 600.);
        cx.notify();
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
            // Brief 0076: coverage of the selection (else every test), Group By, Sort By Duration, Run Tests After
            // Build.
            .child(button(
                COVERAGE,
                "Analyze Code Coverage",
                !running,
                eludite_commands::test::COVERAGE,
                if has_selection {
                    json!({ "selection": true })
                } else {
                    json!({})
                },
            ))
            .child(
                toolbar_button(
                    GROUP_BY,
                    format!("Group By: {} \u{25BE}", self.data.group_by.label()),
                    true,
                    &t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.group_menu = !this.group_menu;
                    cx.notify();
                })),
            )
            .children(self.group_menu(cx))
            .child(
                toolbar_button(
                    SORT_DURATION,
                    if self.sort_duration {
                        "\u{2713} Sort By Duration"
                    } else {
                        "Sort By Duration"
                    },
                    true,
                    &t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sort_duration = !this.sort_duration;
                    this.rebuild();
                    cx.notify();
                })),
            )
            .child({
                let on = self.run_after_build;
                toolbar_button(
                    RUN_AFTER_BUILD,
                    if on {
                        "\u{2713} Run Tests After Build"
                    } else {
                        "Run Tests After Build"
                    },
                    true,
                    &t,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_setting("test.runAfterBuild", json!(!on), window, cx)
                }))
            })
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
        // Brief 0076: the caret where it is, not only at the end.
        let shown = if focused {
            let chars: Vec<char> = self.filter.chars().collect();
            let at = self.caret.min(chars.len());
            let (a, b) = chars.split_at(at);
            format!(
                "{}\u{2502}{}",
                a.iter().collect::<String>(),
                b.iter().collect::<String>()
            )
        } else {
            self.filter.clone()
        };
        let search = text_box(SEARCH_BOX, &shown, "Search Test Explorer", false, &t)
            .when(focused, |b| b.border_color(t.accent).text_color(t.text))
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
            _ if self.data.stale => Some(
                "These tests are from before the last build or session; they are rediscovered at the next run."
                    .into(),
            ),
            _ => self.data.message.clone(),
        };
        let summary_line = self.data.summary.as_ref().map(|(text, _)| text.clone());
        let count = self.rows.len();
        let details = self.details();
        let detail_lines: Vec<gpui::AnyElement> = details
            .into_iter()
            .enumerate()
            .map(|(ix, l)| {
                let el = div()
                    .id(SharedString::from(detail_selector(ix)))
                    .debug_selector(move || detail_selector(ix))
                    .whitespace_nowrap();
                match frame_of(&l) {
                    // Brief 0076: a stack frame opens its file at its line.
                    Some((file, line)) => el
                        .text_color(t.accent)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run(
                                eludite_commands::workspace::FILE_OPEN,
                                json!({ "path": file, "line": line }),
                                window,
                                cx,
                            )
                        }))
                        .child(SharedString::from(l))
                        .into_any_element(),
                    None => el.child(SharedString::from(l)).into_any_element(),
                }
            })
            .collect();
        div()
            .id("test-explorer")
            .debug_selector(|| "test-explorer".into())
            .size_full()
            .flex()
            .flex_col()
            .text_color(t.text)
            .track_focus(&self.focus)
            .key_context("TestExplorer")
            .on_key_down(cx.listener(Self::tree_key))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if let Some((y0, h0)) = this.resizing {
                    if e.pressed_button == Some(MouseButton::Left) {
                        let dy = f32::from(e.position.y) - y0;
                        this.set_details_height(h0 - dy, cx);
                    } else {
                        this.resizing = None;
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resizing = None),
            )
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
                    TREE,
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .child(
                div()
                    .id(DETAILS_RESIZE)
                    .debug_selector(|| DETAILS_RESIZE.into())
                    .flex_none()
                    .h(px(4.))
                    .w_full()
                    .cursor_row_resize()
                    .border_t_1()
                    .border_color(t.border)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, _, cx| {
                            this.resizing = Some((f32::from(e.position.y), this.details_height));
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .id(DETAILS)
                    .debug_selector(|| DETAILS.into())
                    .flex_none()
                    .h(px(self.details_height))
                    .overflow_y_scroll()
                    .p_1()
                    .text_size(t.typography.small)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .children(
                                summary_line
                                    .map(|s| div().font_weight(FontWeight::SEMIBOLD).child(s)),
                            )
                            .child(div().flex_1())
                            .child(
                                toolbar_button(COPY_DETAILS, "Copy", self.selected.is_some(), &t)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let text = this.details_text();
                                        if !text.is_empty() {
                                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        }
                                    })),
                            ),
                    )
                    .children(detail_lines),
            )
            .children(self.context_menu(cx))
    }
}

use gpui::prelude::FluentBuilder as _;

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

    fn project(key: &str, name: &str) -> ProjectNode {
        ProjectNode {
            key: key.into(),
            name: name.into(),
            path: format!("/s/{name}.csproj"),
            protocol: Proto::Mtp,
            target_framework: Some("net10.0".into()),
            runtime: None,
            program: None,
            state: NodeState::Ready,
            message: None,
            cargo: None,
        }
    }

    fn data() -> TreeData {
        TreeData {
            projects: vec![project("p", "Corpus")],
            tests: vec![
                test("t1", "A", "N.C.A", &["N", "C"], Some(Outcome::Passed)),
                test("t2", "B", "N.C.B", &["N", "C"], Some(Outcome::Failed)),
                test("t3", "M", "N.D.M", &["N", "D"], None),
            ],
            ..TreeData::default()
        }
    }

    fn labels(w: &TestExplorer) -> Vec<String> {
        w.rows()
            .iter()
            .map(|r| format!("{}{} {:?}", "  ".repeat(r.depth), r.label, r.glyph))
            .collect()
    }

    #[gpui::test]
    fn the_tree_groups_filters_and_details(cx: &mut gpui::TestAppContext) {
        let w = cx.new(|cx| TestExplorer::new(Theme::dark(), cx));
        w.update(cx, |w, cx| w.set_data(data(), cx));
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
            assert_eq!(w.rows()[2].detail.as_deref(), Some("2 \u{00B7} 24 ms"));
        });
        w.update(cx, |w, cx| w.set_filter("Outcome:Failed".into(), cx));
        w.read_with(cx, |w, _| assert_eq!(w.rows().last().unwrap().label, "B"));
        w.update(cx, |w, cx| w.set_filter("Trait:Category=Math".into(), cx));
        w.read_with(cx, |w, _| assert_eq!(w.rows().last().unwrap().label, "M"));
        w.update(cx, |w, cx| w.set_filter("FullName:N.C a".into(), cx));
        w.read_with(cx, |w, _| assert_eq!(w.rows().len(), 4));
        // Brief 0076: `Not Run` as two words, quoted or not.
        for f in ["Outcome:\"Not Run\"", "Outcome:Not Run", "outcome:notrun"] {
            w.update(cx, |w, cx| w.set_filter(f.into(), cx));
            w.read_with(cx, |w, _| {
                assert_eq!(w.rows().last().unwrap().label, "M", "{f}");
                assert_eq!(w.rows().len(), 4, "{f}");
            });
        }
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
            assert_eq!(w.copy_text().as_deref(), Some("N.C.B"));
        });
        w.update(cx, |w, cx| w.select(Some("g:p:p/N".into()), cx));
        w.read_with(cx, |w, _| {
            assert_eq!(w.selected_tests().len(), 3);
            assert_eq!(w.copy_text().as_deref(), Some("N.C.A\nN.C.B\nN.D.M"));
        });
        assert_eq!(duration_text(0.4), "< 1 ms");
        assert_eq!(duration_text(1500.), "1.5 sec");
    }

    #[test]
    fn stack_frames_are_found_in_details_lines() {
        assert_eq!(
            frame_of("  at N.C.B() in /s/C.cs:line 9"),
            Some(("/s/C.cs".into(), 9))
        );
        assert_eq!(
            frame_of("  src/lib.rs:25:9"),
            Some(("src/lib.rs".into(), 25))
        );
        assert_eq!(
            frame_of("panicked at src/lib.rs:25:9:"),
            Some(("src/lib.rs".into(), 25))
        );
        assert_eq!(frame_of("  boom"), None);
        assert_eq!(frame_of("Source: /s/C.cs, line 3"), None);
    }

    #[gpui::test]
    fn data_rows_sit_under_their_method_and_every_grouping_works(cx: &mut gpui::TestAppContext) {
        let w = cx.new(|cx| TestExplorer::new(Theme::dark(), cx));
        let mut d = data();
        d.projects.push(project("q", "Other"));
        d.tests.push(test(
            "r1",
            "P(a: 1)",
            "N.C.P",
            &["N", "C"],
            Some(Outcome::Passed),
        ));
        d.tests.push(test(
            "r2",
            "P(a: 2)",
            "N.C.P",
            &["N", "C"],
            Some(Outcome::Failed),
        ));
        let mut other = test("o1", "X", "O.E.X", &["O", "E"], Some(Outcome::Passed));
        other.project = "q".into();
        other.id = "q|o1".into();
        other.result.as_mut().unwrap().duration_ms = Some(1500.);
        d.tests.push(other);
        w.update(cx, |w, cx| w.set_data(d.clone(), cx));
        w.read_with(cx, |w, _| {
            assert_eq!(
                labels(w),
                [
                    "Corpus Failed",
                    "  N Failed",
                    "    C Failed",
                    "      A Passed",
                    "      B Failed",
                    "      P Failed",
                    "        P(a: 1) Passed",
                    "        P(a: 2) Failed",
                    "    D NotRun",
                    "      M NotRun",
                    "Other Passed",
                    "  O Passed",
                    "    E Passed",
                    "      X Passed"
                ]
            );
            assert_eq!(w.rows()[5].kind, RowKind::Method);
            assert_eq!(w.rows()[5].tests.len(), 2);
            assert_eq!(w.rows()[5].detail.as_deref(), Some("2 \u{00B7} 24 ms"));
        });
        let mut grouped = |by: GroupBy| {
            let mut d = d.clone();
            d.group_by = by;
            w.update(cx, |w, cx| w.set_data(d, cx));
            w.read_with(cx, |w, _| {
                w.rows()
                    .iter()
                    .filter(|r| r.depth == 0)
                    .map(|r| r.label.clone())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(
            grouped(GroupBy::Outcome),
            ["Failed Tests", "Not Run Tests", "Passed Tests"]
        );
        assert_eq!(grouped(GroupBy::Namespace), ["N", "O"]);
        assert_eq!(grouped(GroupBy::Class), ["C", "D", "E"]);
        assert_eq!(
            grouped(GroupBy::Duration),
            ["Slow (> 1 sec)", "Fast (< 100 ms)", "Not Run"]
        );
        assert_eq!(grouped(GroupBy::Traits), ["Category=Math", "No Traits"]);
        assert_eq!(grouped(GroupBy::TargetFramework), ["net10.0"]);
        assert_eq!(grouped(GroupBy::Project), ["Corpus", "Other"]);
        // Outcome grouping: project, namespace and class under each outcome.
        grouped(GroupBy::Outcome);
        w.read_with(cx, |w, _| {
            assert_eq!(
                &labels(w)[..4],
                [
                    "Failed Tests Failed",
                    "  Corpus Failed",
                    "    N Failed",
                    "      C Failed"
                ]
            );
        });
        assert_eq!(GroupBy::parse("target_framework"), GroupBy::TargetFramework);
        assert_eq!(GroupBy::parse("nonsense"), GroupBy::Project);
    }

    #[gpui::test]
    fn ten_thousand_tests_regroup_within_budget(cx: &mut gpui::TestAppContext) {
        let w = cx.new(|cx| TestExplorer::new(Theme::dark(), cx));
        let mut d = data();
        d.tests.clear();
        for i in 0..10_000 {
            let mut t = test(
                &format!("x{i}"),
                &format!("Test{i}"),
                &format!("N{}.C{}.Test{i}", i % 20, i % 200),
                &[&format!("N{}", i % 20), &format!("C{}", i % 200)],
                Some(if i % 7 == 0 {
                    Outcome::Failed
                } else {
                    Outcome::Passed
                }),
            );
            t.traits = vec![("Category".into(), format!("K{}", i % 5))];
            d.tests.push(t);
        }
        w.update(cx, |w, cx| w.set_data(d.clone(), cx));
        for by in [GroupBy::Outcome, GroupBy::Traits, GroupBy::Project] {
            let mut next = d.clone();
            next.group_by = by;
            let started = std::time::Instant::now();
            w.update(cx, |w, cx| w.set_data(next, cx));
            eludite_test_support::assert_budget(
                &format!("regroup 10,000 tests by {by:?}"),
                started.elapsed(),
                std::time::Duration::from_millis(50),
            );
        }
        w.read_with(cx, |w, _| assert!(w.rows().len() > 10_000));
    }
}
