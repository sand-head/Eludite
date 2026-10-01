//! The Error List (PLAN.md 5.4, brief 0012): diagnostics from the host's `textDocument/publishDiagnostics` and the
//! solution's load diagnostics, one row each (code, description, project, file, line, column). The tab label stays
//! "Error List". Double-clicking a row (Visual Studio's gesture) opens the file at the location through
//! `eludite.file.open`. Rows come from the live analysis and from the last build (brief 0017); the Source column says
//! which (Build, IntelliSense, or Build + IntelliSense for a build diagnostic that matched a live one).
//!
//! The toolbar (brief 0014) is Visual Studio's: the Errors, Warnings and Messages toggle buttons with their counts, a
//! project dropdown and a search box over code and description. Every change goes through
//! `eludite.error_list.filter`; filtering is local and instant and never changes what `diagnostics.list` returns.
//! The counts are those of the rows that pass the project and search filters, whether or not their severity is
//! shown, as in Visual Studio.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use eludite_commands::diagnostics::{RowSource, Severity};
use eludite_commands::workspace::{
    self, ErrorListFilterInput, ErrorListFilterOutput, FilterCounts,
};
use eludite_ui::{RunCommand, Theme, text_box, toggle_button};
use gpui::{
    App, Bounds, ClickEvent, Context, Div, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Render, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, anchored, canvas, deferred, div, px, uniform_list,
};
use serde_json::{Value, json};

/// One Error List row. Line and column are 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRow {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub project: Option<String>,
    pub path: PathBuf,
    /// The file name, as the File column shows it.
    pub file: String,
    pub line: u32,
    pub column: u32,
    /// The last build, the live analysis, or both (brief 0017).
    pub source: RowSource,
}

/// The Source column's text, in Visual Studio's terms (the live analysis is IntelliSense).
pub fn source_label(source: RowSource) -> &'static str {
    match source {
        RowSource::Build => "Build",
        RowSource::Live => "IntelliSense",
        RowSource::Both => "Build + IntelliSense",
    }
}

/// Counts for the header: errors, warnings, messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
    pub messages: usize,
}

/// The toolbar's filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorFilter {
    pub errors: bool,
    pub warnings: bool,
    pub messages: bool,
    /// `None`: every project.
    pub project: Option<String>,
    /// Matched case-insensitively against the code and the description.
    pub text: String,
}

impl Default for ErrorFilter {
    fn default() -> Self {
        Self {
            errors: true,
            warnings: true,
            messages: true,
            project: None,
            text: String::new(),
        }
    }
}

impl ErrorFilter {
    /// Whether `row` passes the project and search filters (the counts use these).
    fn scope(&self, row: &ErrorRow, needle: &str) -> bool {
        self.project
            .as_ref()
            .is_none_or(|p| row.project.as_ref() == Some(p))
            && (needle.is_empty()
                || row.code.to_lowercase().contains(needle)
                || row.message.to_lowercase().contains(needle))
    }

    fn severity(&self, s: Severity) -> bool {
        match s {
            Severity::Error => self.errors,
            Severity::Warning => self.warnings,
            Severity::Message => self.messages,
        }
    }

    /// Apply the members `input` sets.
    pub fn update(&mut self, input: &ErrorListFilterInput) {
        if let Some(v) = input.errors {
            self.errors = v;
        }
        if let Some(v) = input.warnings {
            self.warnings = v;
        }
        if let Some(v) = input.messages {
            self.messages = v;
        }
        if let Some(p) = &input.project {
            self.project = p.clone();
        }
        if let Some(t) = &input.text {
            self.text = t.clone();
        }
    }
}

pub struct ErrorList {
    theme: Theme,
    rows: Vec<ErrorRow>,
    filter: ErrorFilter,
    /// Indices into `rows` of the rows shown.
    visible: Vec<usize>,
    counts: Counts,
    /// A visible row index.
    selected: Option<usize>,
    project_menu: bool,
    search_focus: FocusHandle,
    /// Where the toolbar's controls were last drawn (window coordinates), for `--bounds-out` and real-input runs.
    painted: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
}

const ROW_HEIGHT: f32 = 20.;

/// Debug selector of row `ix`.
pub fn row_selector(ix: usize) -> String {
    format!("error-row-{ix}")
}

/// Debug selectors of the toolbar.
pub const ERRORS_BUTTON: &str = "error-list-errors";
pub const WARNINGS_BUTTON: &str = "error-list-warnings";
pub const MESSAGES_BUTTON: &str = "error-list-messages";
pub const PROJECT_BUTTON: &str = "error-list-project";
pub const SEARCH_BOX: &str = "error-list-search";

/// Debug selector of entry `ix` of the project dropdown (0 is "All Projects").
pub fn project_item_selector(ix: usize) -> String {
    format!("error-list-project-{ix}")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl ErrorList {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            filter: ErrorFilter::default(),
            visible: Vec::new(),
            counts: Counts::default(),
            selected: None,
            project_menu: false,
            search_focus: cx.focus_handle(),
            painted: Rc::default(),
        }
    }

    /// Where the toolbar's controls were last drawn, by debug selector.
    pub fn painted_bounds(&self) -> Vec<(&'static str, Bounds<Pixels>)> {
        self.painted
            .borrow()
            .iter()
            .map(|(k, b)| (*k, *b))
            .collect()
    }

    fn tracked(&self, id: &'static str, el: Stateful<Div>) -> Stateful<Div> {
        let painted = self.painted.clone();
        el.relative().child(
            canvas(
                move |b, _, _| {
                    painted.borrow_mut().insert(id, b);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    /// Every row, before filtering.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[ErrorRow] {
        &self.rows
    }

    /// The rows shown, in order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn visible_rows(&self) -> impl Iterator<Item = &ErrorRow> {
        self.visible.iter().map(|&i| &self.rows[i])
    }

    /// The counts the toggle buttons show.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// The projects the dropdown lists: those of the rows, sorted.
    pub fn projects(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|r| r.project.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The counts as the toolbar's buttons word them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn header(&self) -> String {
        let c = self.counts;
        format!(
            "\u{2716} {}   \u{26A0} {}   \u{24D8} {}",
            plural(c.errors, "Error", "Errors"),
            plural(c.warnings, "Warning", "Warnings"),
            plural(c.messages, "Message", "Messages")
        )
    }

    pub fn set_rows(&mut self, rows: Vec<ErrorRow>, cx: &mut Context<Self>) {
        if rows != self.rows {
            self.rows = rows;
            self.selected = None;
            self.refilter();
            cx.notify();
        }
    }

    /// Change the filters (`eludite.error_list.filter`).
    pub fn set_filter(&mut self, input: &ErrorListFilterInput, cx: &mut Context<Self>) {
        let before = self.filter.clone();
        self.filter.update(input);
        if self.filter != before {
            self.selected = None;
            self.refilter();
            cx.notify();
        }
    }

    pub fn filter_output(&self) -> ErrorListFilterOutput {
        let f = &self.filter;
        ErrorListFilterOutput {
            errors: f.errors,
            warnings: f.warnings,
            messages: f.messages,
            project: f.project.clone(),
            text: f.text.clone(),
            counts: FilterCounts {
                errors: self.counts.errors as u64,
                warnings: self.counts.warnings as u64,
                messages: self.counts.messages as u64,
            },
            shown: self.visible.len() as u64,
            total: self.rows.len() as u64,
        }
    }

    fn refilter(&mut self) {
        let needle = self.filter.text.to_lowercase();
        let mut counts = Counts::default();
        self.visible.clear();
        for (i, r) in self.rows.iter().enumerate() {
            if !self.filter.scope(r, &needle) {
                continue;
            }
            match r.severity {
                Severity::Error => counts.errors += 1,
                Severity::Warning => counts.warnings += 1,
                Severity::Message => counts.messages += 1,
            }
            if self.filter.severity(r.severity) {
                self.visible.push(i);
            }
        }
        self.counts = counts;
    }

    /// Run `eludite.error_list.filter` with `args`, as the toolbar does.
    fn run_filter(&self, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(
            Box::new(RunCommand::new(workspace::ERROR_LIST_FILTER, args)),
            cx,
        );
    }

    fn search_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        let mut text = self.filter.text.clone();
        match k.key.as_str() {
            "backspace" => {
                text.pop();
            }
            "escape" => text.clear(),
            "space" => text.push(' '),
            _ => {
                // Platforms report the typed text in `key_char`; a bare one-character key (tests) is the text too.
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
        self.run_filter(json!({ "text": text }), window, cx);
    }

    fn click(&mut self, ix: usize, e: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        cx.notify();
        if e.click_count() < 2 {
            return;
        }
        let Some(row) = self.visible.get(ix).and_then(|&i| self.rows.get(i)) else {
            return;
        };
        window.dispatch_action(
            Box::new(RunCommand::new(
                workspace::FILE_OPEN,
                json!({"path": row.path.to_string_lossy(), "line": row.line, "column": row.column}),
            )),
            cx,
        );
    }
}

fn cell(text: impl Into<SharedString>, width: Option<f32>) -> gpui::Div {
    let c = div()
        .px_1()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(text.into());
    match width {
        Some(w) => c.flex_none().w(px(w)),
        None => c.flex_1().min_w_0(),
    }
}

impl Render for ErrorList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let c = self.counts;
        let f = &self.filter;
        let toggle = |id: &'static str, label: String, on: bool, member: &'static str| {
            self.tracked(
                id,
                toggle_button(id, label, on, &t).on_click(cx.listener(
                    move |this, _, window, cx| this.run_filter(json!({ member: !on }), window, cx),
                )),
            )
        };
        let project_label = format!(
            "{} \u{25BE}",
            f.project.as_deref().unwrap_or("All Projects")
        );
        let project_button = self.tracked(
            PROJECT_BUTTON,
            toggle_button(PROJECT_BUTTON, project_label, false, &t).on_click(cx.listener(
                |this, _, _, cx| {
                    this.project_menu = !this.project_menu;
                    cx.notify();
                },
            )),
        );
        let project_menu = self.project_menu.then(|| {
            let mut entries = vec![(None, "All Projects".to_owned())];
            entries.extend(self.projects().into_iter().map(|p| (Some(p.clone()), p)));
            let items = entries.into_iter().enumerate().map(|(ix, (value, label))| {
                let sel = project_item_selector(ix);
                div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .px_2()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.menu_hover))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.project_menu = false;
                        this.run_filter(json!({ "project": value }), window, cx);
                    }))
            });
            deferred(
                anchored().child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("error-list-project-menu")
                        .occlude()
                        .min_w(px(180.))
                        .py_1()
                        .mt(px(22.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.project_menu = false;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1)
        });
        let focused = self.search_focus.is_focused(window);
        let search = self
            .tracked(
                SEARCH_BOX,
                text_box(SEARCH_BOX, &f.text, "Search Error List", focused, &t),
            )
            .track_focus(&self.search_focus)
            .key_context("ErrorListSearch")
            .on_key_down(cx.listener(Self::search_key))
            .on_click(cx.listener(|this, _, window, cx| {
                this.search_focus.focus(window, cx);
                cx.notify();
            }));
        let header = div()
            .id("error-list-header")
            .debug_selector(|| "error-list-header".into())
            .flex()
            .flex_row()
            .flex_none()
            .gap_1()
            .h(px(ROW_HEIGHT + 6.))
            .items_center()
            .px_2()
            .text_size(t.typography.ui)
            .child(
                div()
                    .relative()
                    .child(project_button)
                    .children(project_menu),
            )
            .child(toggle(
                ERRORS_BUTTON,
                format!("\u{2716} {}", plural(c.errors, "Error", "Errors")),
                f.errors,
                "errors",
            ))
            .child(toggle(
                WARNINGS_BUTTON,
                format!("\u{26A0} {}", plural(c.warnings, "Warning", "Warnings")),
                f.warnings,
                "warnings",
            ))
            .child(toggle(
                MESSAGES_BUTTON,
                format!("\u{24D8} {}", plural(c.messages, "Message", "Messages")),
                f.messages,
                "messages",
            ))
            .child(div().flex_1())
            .child(search);
        let columns = div()
            .w_full()
            .flex()
            .flex_row()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .text_size(t.typography.small)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.text_muted)
            .child(cell("", Some(22.)))
            .child(cell("Code", Some(70.)))
            .child(cell("Description", None))
            .child(cell("Project", Some(130.)))
            .child(cell("File", Some(150.)))
            .child(cell("Line", Some(44.)))
            .child(cell("Col", Some(36.)))
            .child(cell("Source", Some(130.)));
        let count = self.visible.len();
        div()
            .id("error-list")
            .debug_selector(|| "error-list".into())
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(columns)
            .child(
                uniform_list(
                    "error-list-rows",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        let t = this.theme;
                        range
                            .filter_map(|ix| {
                                let r = this.rows.get(*this.visible.get(ix)?)?;
                                let (icon, color) = match r.severity {
                                    Severity::Error => ("\u{2716}", gpui::rgb(0xF14C4C)),
                                    Severity::Warning => ("\u{26A0}", gpui::rgb(0xCCA700)),
                                    Severity::Message => ("\u{24D8}", gpui::rgb(0x3794FF)),
                                };
                                let selected = this.selected == Some(ix);
                                let sel = row_selector(ix);
                                let row = div()
                                    .id(SharedString::from(sel.clone()))
                                    .debug_selector(move || sel)
                                    .w_full()
                                    .overflow_hidden()
                                    .flex()
                                    .flex_row()
                                    .h(px(ROW_HEIGHT))
                                    .items_center()
                                    .text_size(t.typography.ui)
                                    .cursor_pointer()
                                    .child(cell(icon, Some(22.)).text_color(color))
                                    .child(cell(r.code.clone(), Some(70.)))
                                    .child(cell(r.message.clone(), None))
                                    .child(cell(r.project.clone().unwrap_or_default(), Some(130.)))
                                    .child(cell(r.file.clone(), Some(150.)))
                                    .child(cell(r.line.to_string(), Some(44.)))
                                    .child(cell(r.column.to_string(), Some(36.)))
                                    .child(cell(source_label(r.source), Some(130.)))
                                    .on_click(cx.listener(move |this, e, window, cx| {
                                        this.click(ix, e, window, cx)
                                    }));
                                Some(if selected {
                                    row.bg(t.accent).text_color(t.text_on_accent)
                                } else {
                                    row.text_color(t.text).hover(|s| s.bg(t.menu_hover))
                                })
                            })
                            .collect()
                    }),
                )
                .flex_1(),
            )
    }
}

impl Focusable for ErrorList {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.search_focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_combine_and_counts_follow_project_and_text_only() {
        let row = |severity, code: &str, message: &str, project: &str| ErrorRow {
            severity,
            code: code.into(),
            message: message.into(),
            project: Some(project.into()),
            path: PathBuf::from("/a.cs"),
            file: "a.cs".into(),
            line: 1,
            column: 1,
            source: RowSource::Live,
        };
        let rows = [
            row(
                Severity::Error,
                "CS0103",
                "The name 'x' does not exist",
                "Host",
            ),
            row(
                Severity::Warning,
                "CS0168",
                "The variable 'e' is declared",
                "Host",
            ),
            row(
                Severity::Message,
                "IDE0290",
                "Use primary constructor",
                "Tests",
            ),
        ];
        let mut f = ErrorFilter::default();
        let pass = |f: &ErrorFilter| {
            let needle = f.text.to_lowercase();
            rows.iter()
                .filter(|r| f.scope(r, &needle) && f.severity(r.severity))
                .map(|r| r.code.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(pass(&f), ["CS0103", "CS0168", "IDE0290"]);
        f.update(&ErrorListFilterInput {
            errors: Some(false),
            messages: Some(false),
            ..Default::default()
        });
        assert_eq!(pass(&f), ["CS0168"]);
        f.update(&ErrorListFilterInput {
            errors: Some(true),
            project: Some(Some("Host".into())),
            ..Default::default()
        });
        assert_eq!(pass(&f), ["CS0103", "CS0168"]);
        f.update(&ErrorListFilterInput {
            text: Some("NAME".into()),
            ..Default::default()
        });
        assert_eq!(
            pass(&f),
            ["CS0103"],
            "case-insensitive, over the description"
        );
        f.update(&ErrorListFilterInput {
            text: Some("ide0".into()),
            project: Some(None),
            messages: Some(true),
            ..Default::default()
        });
        assert_eq!(pass(&f), ["IDE0290"], "over the code, every project again");
    }
}
