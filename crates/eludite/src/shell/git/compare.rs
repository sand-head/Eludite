//! Compare with Unmodified (the Workspace window's context menu and Ctrl+D; `eludite.git.diff` from the UI; a
//! double-click in Git Changes): a read-only document of two panes, the unmodified text (the index, HEAD or a
//! revision) on the left and the working tree's on the right, aligned row by row from brief 0016's line diff and
//! scrolled together (one list draws both). Removed and added lines carry the diff colors and signs; F8 and Shift+F8
//! go to the next and previous difference, as Visual Studio's diff viewer does. And Blame: each line with the commit
//! that last changed it.

use std::ops::Range;

use eludite_commands::git::BlameOutput;
use eludite_ui::Theme;
use eludite_ui::diff::{DIFF_ROW_HEIGHT, DiffKind, DiffLine, diff_colors};
use gpui::{
    Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

/// Document tab ids of Compare with Unmodified start with this.
pub const COMPARE_PREFIX: &str = "git-diff:";
/// And Blame's with this.
pub const BLAME_PREFIX: &str = "git-blame:";

/// The tab id for comparing `path` against `against` (`staged` for the index's side).
pub fn compare_tab(path: &str, against: &str, staged: bool) -> String {
    format!(
        "{COMPARE_PREFIX}{path}|{against}{}",
        if staged { "|staged" } else { "" }
    )
}

pub fn blame_tab(path: &str) -> String {
    format!("{BLAME_PREFIX}{path}")
}

/// One side of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub line: u32,
    pub text: String,
    pub kind: DiffKind,
}

/// A row: the old side and the new side (either missing beside an added or removed line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideRow {
    pub left: Option<Cell>,
    pub right: Option<Cell>,
}

/// Rows of the two panes from the line diff, and the rows where each difference starts.
pub fn side_by_side(lines: &[DiffLine]) -> (Vec<SideRow>, Vec<usize>) {
    let mut rows = Vec::with_capacity(lines.len());
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if l.kind == DiffKind::Same {
            rows.push(SideRow {
                left: l.old.map(|n| Cell {
                    line: n,
                    text: l.text.clone(),
                    kind: DiffKind::Same,
                }),
                right: l.new.map(|n| Cell {
                    line: n,
                    text: l.text.clone(),
                    kind: DiffKind::Same,
                }),
            });
            i += 1;
            continue;
        }
        // A difference: its removed lines on the left, its added lines on the right, side by side.
        let start = i;
        while i < lines.len() && lines[i].kind != DiffKind::Same {
            i += 1;
        }
        let removed: Vec<&DiffLine> = lines[start..i]
            .iter()
            .filter(|l| l.kind == DiffKind::Removed)
            .collect();
        let added: Vec<&DiffLine> = lines[start..i]
            .iter()
            .filter(|l| l.kind == DiffKind::Added)
            .collect();
        hunks.push(rows.len());
        for k in 0..removed.len().max(added.len()) {
            rows.push(SideRow {
                left: removed.get(k).map(|l| Cell {
                    line: l.old.unwrap_or_default(),
                    text: l.text.clone(),
                    kind: DiffKind::Removed,
                }),
                right: added.get(k).map(|l| Cell {
                    line: l.new.unwrap_or_default(),
                    text: l.text.clone(),
                    kind: DiffKind::Added,
                }),
            });
        }
    }
    (rows, hunks)
}

pub struct CompareView {
    theme: Theme,
    pub path: String,
    pub old_label: String,
    pub new_label: String,
    pub binary: bool,
    rows: Vec<SideRow>,
    hunks: Vec<usize>,
    current: Option<usize>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    mono: SharedString,
}

impl CompareView {
    pub fn new(
        theme: Theme,
        path: String,
        old_label: String,
        new_label: String,
        binary: bool,
        lines: &[DiffLine],
        cx: &mut Context<Self>,
    ) -> Self {
        let (rows, hunks) = side_by_side(lines);
        Self {
            theme,
            path,
            old_label,
            new_label,
            binary,
            rows,
            hunks,
            current: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            mono: eludite_editor::default_font_family(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[SideRow] {
        &self.rows
    }

    /// The rows where each difference starts.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn hunks(&self) -> &[usize] {
        &self.hunks
    }

    /// The difference F8 last went to.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// F8 (`forward`) or Shift+F8: the next or previous difference, wrapping, scrolled to the top.
    pub fn go_to_difference(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.hunks.is_empty() {
            return;
        }
        let n = self.hunks.len();
        let next = match (self.current, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(c), true) => (c + 1) % n,
            (Some(c), false) => (c + n - 1) % n,
        };
        self.current = Some(next);
        self.scroll
            .scroll_to_item(self.hunks[next], ScrollStrategy::Top);
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.key == "f8" && !k.modifiers.control && !k.modifiers.alt {
            cx.stop_propagation();
            self.go_to_difference(!k.modifiers.shift, cx);
        }
    }

    fn cell(&self, cell: &Option<Cell>, current: bool) -> gpui::AnyElement {
        let t = self.theme;
        let mono: SharedString = self.mono.clone();
        let (line, sign, text, kind) = match cell {
            Some(c) => (
                c.line.to_string(),
                match c.kind {
                    DiffKind::Added => "+",
                    DiffKind::Removed => "\u{2212}",
                    DiffKind::Same => "",
                },
                c.text.clone(),
                c.kind,
            ),
            None => (String::new(), "", String::new(), DiffKind::Same),
        };
        let el = div()
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .h(px(DIFF_ROW_HEIGHT))
            .overflow_hidden()
            .whitespace_nowrap()
            .font_family(mono)
            .child(
                div()
                    .w(px(44.))
                    .flex_none()
                    .pr_1()
                    .text_color(t.text_muted)
                    .child(line),
            )
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .text_color(t.text_muted)
                    .child(sign),
            )
            .child(div().flex_1().child(text));
        let el = match (cell.is_none(), diff_colors(kind)) {
            // The blank beside an added or removed line is hatched in Visual Studio; a flat grey here.
            (true, _) => el.bg(t.chrome),
            (false, Some(c)) => el.bg(c),
            (false, None) => el,
        };
        if current {
            el.border_t_1().border_color(t.accent).into_any_element()
        } else {
            el.into_any_element()
        }
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        let current_row = self.current.map(|c| self.hunks[c]);
        range
            .filter_map(|ix| {
                let r = self.rows.get(ix)?;
                let current = current_row == Some(ix);
                Some(
                    div()
                        .flex()
                        .flex_row()
                        .w_full()
                        .h(px(DIFF_ROW_HEIGHT))
                        .text_size(t.typography.body)
                        .child(self.cell(&r.left, current))
                        .child(div().w(px(1.)).h_full().bg(t.border))
                        .child(self.cell(&r.right, current))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Focusable for CompareView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CompareView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let header = |text: String| {
            div()
                .flex_1()
                .px_2()
                .py_1()
                .bg(t.chrome)
                .text_color(t.chrome_text)
                .whitespace_nowrap()
                .overflow_hidden()
                .child(text)
        };
        let count = self.rows.len();
        let body = if self.binary {
            div()
                .p_2()
                .text_color(t.text_muted)
                .child("Binary files differ.")
                .into_any_element()
        } else {
            uniform_list(
                "git-compare-rows",
                count,
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        div()
            .id("git-compare")
            .debug_selector(|| "git-compare".into())
            .track_focus(&self.focus)
            .key_context("GitCompare")
            .on_key_down(cx.listener(Self::key))
            .on_click(cx.listener(|this, _, window, cx| this.focus.focus(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(t.typography.ui)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .child(header(format!("{} ({})", self.path, self.old_label)))
                    .child(div().w(px(1.)).bg(t.border))
                    .child(header(format!("{} ({})", self.path, self.new_label)))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .bg(t.chrome)
                            .text_color(t.chrome_text)
                            .whitespace_nowrap()
                            .child(format!(
                                "{} difference{} (F8, Shift+F8)",
                                self.hunks.len(),
                                if self.hunks.len() == 1 { "" } else { "s" }
                            )),
                    ),
            )
            .child(body)
    }
}

/// Blame (Annotate): each line of the file as committed at HEAD with its commit, author and date.
pub struct BlameView {
    theme: Theme,
    pub blame: BlameOutput,
    lines: Vec<String>,
    scroll: UniformListScrollHandle,
}

impl BlameView {
    pub fn new(theme: Theme, blame: BlameOutput, text: String) -> Self {
        Self {
            theme,
            blame,
            lines: text.lines().map(str::to_owned).collect(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        range
            .filter_map(|ix| {
                let l = self.blame.lines.get(ix)?;
                let c = self.blame.commits.get(l.commit as usize)?;
                let first = ix == 0 || self.blame.lines[ix - 1].commit != l.commit;
                let note = if first {
                    format!(
                        "{} {} {}",
                        c.short,
                        c.author,
                        c.date
                            .as_deref()
                            .unwrap_or_default()
                            .get(..10)
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                };
                Some(
                    div()
                        .flex()
                        .flex_row()
                        .h(px(DIFF_ROW_HEIGHT))
                        .whitespace_nowrap()
                        .font_family(eludite_editor::default_font_family())
                        .child(
                            div()
                                .w(px(260.))
                                .flex_none()
                                .overflow_hidden()
                                .px_1()
                                .bg(t.chrome)
                                .text_color(t.text_muted)
                                .child(note),
                        )
                        .child(
                            div()
                                .w(px(44.))
                                .flex_none()
                                .pr_1()
                                .text_color(t.text_muted)
                                .child(l.line.to_string()),
                        )
                        .child(
                            div().child(
                                self.lines
                                    .get(l.line as usize - 1)
                                    .cloned()
                                    .unwrap_or_default(),
                            ),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for BlameView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let count = self.blame.lines.len();
        div()
            .id("git-blame")
            .debug_selector(|| "git-blame".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(t.typography.body)
            .child(
                uniform_list(
                    "git-blame-rows",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_ui::diff::diff_lines;

    #[test]
    fn rows_pair_removed_and_added_lines_and_mark_each_difference() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nx\ny\ne\n";
        let (rows, hunks) = side_by_side(&diff_lines(old, new));
        assert_eq!(hunks, [1, 4]);
        let r = &rows[1];
        assert_eq!(r.left.as_ref().unwrap().text, "b");
        assert_eq!(r.right.as_ref().unwrap().text, "B");
        // Two added lines beside nothing.
        assert_eq!(rows[4].left, None);
        assert_eq!(rows[4].right.as_ref().unwrap().text, "x");
        assert_eq!(rows[5].right.as_ref().unwrap().line, 6);
        assert_eq!(rows.len(), 7);
        assert_eq!(rows.last().unwrap().left.as_ref().unwrap().line, 5);
    }
}
