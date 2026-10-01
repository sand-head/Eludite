//! The Error List (PLAN.md 5.4, brief 0012): diagnostics from the host's `textDocument/publishDiagnostics` and the
//! solution's load diagnostics, one row each (code, description, project, file, line, column). The tab label stays
//! "Error List"; the body header shows the error, warning and message counts, as in Visual Studio. Double-clicking a
//! row (Visual Studio's gesture) opens the file at the location through `eludite.file.open`.

use std::path::PathBuf;

use eludite_commands::diagnostics::Severity;
use eludite_commands::workspace;
use eludite_ui::{RunCommand, Theme};
use gpui::{
    ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, uniform_list,
};
use serde_json::json;

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
}

/// Counts for the header: errors, warnings, messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub errors: usize,
    pub warnings: usize,
    pub messages: usize,
}

pub struct ErrorList {
    theme: Theme,
    rows: Vec<ErrorRow>,
    selected: Option<usize>,
}

const ROW_HEIGHT: f32 = 20.;

/// Debug selector of row `ix`.
pub fn row_selector(ix: usize) -> String {
    format!("error-row-{ix}")
}

impl ErrorList {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            selected: None,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[ErrorRow] {
        &self.rows
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for r in &self.rows {
            match r.severity {
                Severity::Error => c.errors += 1,
                Severity::Warning => c.warnings += 1,
                Severity::Message => c.messages += 1,
            }
        }
        c
    }

    /// The header text, as Visual Studio words it.
    pub fn header(&self) -> String {
        let c = self.counts();
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
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
            cx.notify();
        }
    }

    fn click(&mut self, ix: usize, e: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        cx.notify();
        if e.click_count() < 2 {
            return;
        }
        let Some(row) = self.rows.get(ix) else { return };
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let header = div()
            .id("error-list-header")
            .debug_selector(|| "error-list-header".into())
            .flex()
            .flex_row()
            .flex_none()
            .h(px(ROW_HEIGHT + 4.))
            .items_center()
            .px_2()
            .text_size(t.typography.ui)
            .child(self.header());
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
            .child(cell("Col", Some(36.)));
        let count = self.rows.len();
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
                                let r = this.rows.get(ix)?;
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
