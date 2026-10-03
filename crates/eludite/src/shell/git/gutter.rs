//! The editor's change margin: the buffer's lines that differ from the index (added green, modified blue, a red
//! wedge where lines were deleted), from the same bounded line diff the review view uses, recomputed off the UI thread
//! when the editor has been idle after an edit and when the index changes (a stage clears them). A file with conflict
//! markers shows its sides instead: "ours" (between `<<<<<<<` and `=======`) and "theirs" (up to `>>>>>>>`).

use eludite_editor::EditorView;
use eludite_ui::diff::{DiffKind, diff_lines};
use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div,
    px, rgb,
};

/// How long the editor must be idle after an edit before the margin is recomputed (the brief's budget is 200 ms
/// from the edit to the marks).
pub const IDLE: std::time::Duration = std::time::Duration::from_millis(120);

/// What a mark says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkKind {
    Added,
    Modified,
    /// Lines were deleted before this row.
    Deleted,
    /// A conflict's sides.
    Ours,
    Theirs,
}

impl MarkKind {
    /// Visual Studio's change margin colors.
    pub fn color(self) -> u32 {
        match self {
            MarkKind::Added => 0x48_8C_48,
            MarkKind::Modified => 0x1B_81_A8,
            MarkKind::Deleted => 0xC8_3C_3C,
            MarkKind::Ours => 0x40_A6_FF,
            MarkKind::Theirs => 0xE0_8A_2E,
        }
    }
}

/// A run of marked rows (0-based); a deletion has no rows and sits before `row`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mark {
    pub row: u32,
    pub len: u32,
    pub kind: MarkKind,
}

/// The marks of `buffer` against `base` (the index's text), or its conflict sides when it has markers.
pub fn marks(base: &str, buffer: &str) -> Vec<Mark> {
    if let Some(sides) = conflict_sides(buffer) {
        return sides;
    }
    let lines = diff_lines(base, buffer);
    let mut out = Vec::new();
    let mut new_row = 0u32;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind == DiffKind::Same {
            new_row += 1;
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].kind != DiffKind::Same {
            i += 1;
        }
        let removed = lines[start..i]
            .iter()
            .filter(|l| l.kind == DiffKind::Removed)
            .count() as u32;
        let added = lines[start..i]
            .iter()
            .filter(|l| l.kind == DiffKind::Added)
            .count() as u32;
        if added == 0 {
            out.push(Mark {
                row: new_row,
                len: 0,
                kind: MarkKind::Deleted,
            });
        } else {
            let modified = removed.min(added);
            if modified > 0 {
                out.push(Mark {
                    row: new_row,
                    len: modified,
                    kind: MarkKind::Modified,
                });
            }
            if added > modified {
                out.push(Mark {
                    row: new_row + modified,
                    len: added - modified,
                    kind: MarkKind::Added,
                });
            }
            new_row += added;
        }
    }
    out
}

/// The sides of the conflicts in `text`, or `None` when it has no markers.
pub fn conflict_sides(text: &str) -> Option<Vec<Mark>> {
    let mut out = Vec::new();
    let mut side: Option<(MarkKind, u32)> = None;
    for (row, line) in text.lines().enumerate() {
        let row = row as u32;
        if line.starts_with("<<<<<<<") {
            side = Some((MarkKind::Ours, row + 1));
        } else if line.starts_with("=======") && matches!(side, Some((MarkKind::Ours, _))) {
            if let Some((k, start)) = side {
                out.push(Mark {
                    row: start,
                    len: row - start,
                    kind: k,
                });
            }
            side = Some((MarkKind::Theirs, row + 1));
        } else if line.starts_with(">>>>>>>")
            && let Some((MarkKind::Theirs, start)) = side
        {
            side = None;
            out.push(Mark {
                row: start,
                len: row - start,
                kind: MarkKind::Theirs,
            });
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The margin over an editor (drawn by the document area above the view, as the pending-change marks are).
pub struct ChangeMarks {
    view: Entity<EditorView>,
    pub marks: Vec<Mark>,
    _observe: gpui::Subscription,
}

impl ChangeMarks {
    pub fn new(view: Entity<EditorView>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        Self {
            view,
            marks: Vec::new(),
            _observe: observe,
        }
    }

    pub fn set(&mut self, marks: Vec<Mark>, cx: &mut Context<Self>) {
        if marks != self.marks {
            self.marks = marks;
            cx.notify();
        }
    }
}

impl Render for ChangeMarks {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.view.read(cx);
        let lh = v.line_height();
        let top = v.scroll_position().y;
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(self.marks.iter().enumerate().map(|(i, m)| {
                let sel = format!("git-mark-{i}");
                let el = div()
                    .debug_selector(move || sel)
                    .absolute()
                    .left(px(1.))
                    .bg(rgb(m.kind.color()));
                if m.kind == MarkKind::Deleted {
                    el.top(lh * m.row as f32 - top - px(2.)).w(px(6.)).h(px(4.))
                } else {
                    el.top(lh * m.row as f32 - top)
                        .w(px(3.))
                        .h(lh * m.len as f32)
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_modified_and_deleted_rows() {
        let base = "a\nb\nc\nd\ne\n";
        assert!(marks(base, base).is_empty());
        let m = marks(base, "a\nB\nc\nnew\nd\n");
        assert_eq!(
            m,
            [
                Mark {
                    row: 1,
                    len: 1,
                    kind: MarkKind::Modified
                },
                Mark {
                    row: 3,
                    len: 1,
                    kind: MarkKind::Added
                },
                Mark {
                    row: 5,
                    len: 0,
                    kind: MarkKind::Deleted
                },
            ]
        );
        // A new file is all added.
        assert_eq!(
            marks("", "x\ny\n"),
            [Mark {
                row: 0,
                len: 2,
                kind: MarkKind::Added
            }]
        );
    }

    #[test]
    fn conflict_markers_show_their_sides() {
        let text = "one\n<<<<<<< HEAD\nours\n=======\ntheirs 1\ntheirs 2\n>>>>>>> branch\nend\n";
        assert_eq!(
            marks("one\nend\n", text),
            [
                Mark {
                    row: 2,
                    len: 1,
                    kind: MarkKind::Ours
                },
                Mark {
                    row: 4,
                    len: 2,
                    kind: MarkKind::Theirs
                },
            ]
        );
        assert_eq!(conflict_sides("no markers\n"), None);
    }
}
