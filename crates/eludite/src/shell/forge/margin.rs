//! Review threads in the editor's margin: when the pull request checked out (its head branch, or the `pr/<n>` or
//! `mr/<n>` branch Check Out made) has review threads on a file open in the editor, a glyph marks each thread's line
//! at the editor's right edge (resolved ones dimmed). Clicking it opens the thread over the editor with a reply box;
//! the reply posts through `eludite.forge.pull_comment` with `reply_to`. A "+" at the caret's line starts a new
//! comment on that line. Either can post at once (Comment) or join the pending review (Add to Review: `pending`),
//! which the pull request document submits as one.

use eludite_editor::EditorView;
use eludite_forge::{ItemRef, Thread};
use eludite_ui::{RunCommand, Theme, push_button, text_box};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use serde_json::json;

use super::item_args;
use super::widgets::{self, Edit};

pub const POPOVER: &str = "forge-thread-popover";
pub const REPLY_BOX: &str = "forge-thread-reply";
pub const REPLY: &str = "forge-thread-reply-post";
pub const ADD_TO_REVIEW: &str = "forge-thread-add-to-review";
pub const NEW_COMMENT: &str = "forge-thread-new";

/// What the popover shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    /// A thread, by index in `marks`.
    Thread(usize),
    /// A new comment on a 1-based line.
    New(u64),
}

pub fn mark_selector(ix: usize) -> String {
    format!("forge-thread-mark-{ix}")
}

/// A thread at a 1-based line of the file, and the pull request it belongs to.
pub type Mark = (u64, Thread, ItemRef);

pub struct ThreadMarks {
    theme: Theme,
    view: Entity<EditorView>,
    pub marks: Vec<Mark>,
    /// The pull request checked out and this file's path in the repository (new comments go there).
    pub target: Option<(ItemRef, String)>,
    pub open: Option<Open>,
    pub reply: String,
    /// Where the marks were drawn, while `--bounds-out` probes.
    pub probe: Option<eludite_ui::BoundsMap>,
    focus: FocusHandle,
    _observe: gpui::Subscription,
}

impl Focusable for ThreadMarks {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ThreadMarks {
    pub fn new(theme: Theme, view: Entity<EditorView>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        Self {
            theme,
            view,
            marks: Vec::new(),
            target: None,
            open: None,
            reply: String::new(),
            probe: None,
            focus: cx.focus_handle(),
            _observe: observe,
        }
    }

    pub fn set(
        &mut self,
        mut marks: Vec<Mark>,
        target: Option<(ItemRef, String)>,
        cx: &mut Context<Self>,
    ) {
        marks.sort_by_key(|m| m.0);
        if marks == self.marks && target == self.target {
            return;
        }
        let open_id = match self.open {
            Some(Open::Thread(i)) => self.marks.get(i).map(|m| m.1.id.clone()),
            _ => None,
        };
        self.marks = marks;
        self.target = target;
        self.open = match self.open {
            Some(Open::Thread(_)) => open_id
                .and_then(|id| self.marks.iter().position(|m| m.1.id == id))
                .map(Open::Thread),
            other if self.target.is_some() => other,
            _ => None,
        };
        cx.notify();
    }

    /// The caret's 1-based line.
    fn caret_line(&self, cx: &App) -> u64 {
        let e = self.view.read(cx).editor();
        let head = e.primary_selection().head;
        e.buffer().offset_to_point(head).row as u64 + 1
    }

    /// Open the new-comment popover at the caret's line.
    pub fn new_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.target.is_none() {
            return;
        }
        let line = self.caret_line(cx);
        self.open = Some(Open::New(line));
        self.reply.clear();
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn toggle(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ix = Open::Thread(ix);
        self.open = if self.open == Some(ix) {
            None
        } else {
            Some(ix)
        };
        self.reply.clear();
        if self.open.is_some() {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Type the reply (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_reply(&mut self, text: &str, cx: &mut Context<Self>) {
        self.reply = text.to_owned();
        cx.notify();
    }

    /// Post the text in the box: at once, or into the pending review (`pending`).
    pub fn post_reply(&mut self, pending: bool, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.reply.trim().to_owned();
        if body.is_empty() {
            return;
        }
        let mut args = match self.open {
            Some(Open::Thread(i)) => {
                let Some((_, thread, item)) = self.marks.get(i) else {
                    return;
                };
                let mut a = item_args(item);
                a["reply_to"] = json!(thread.id);
                a
            }
            Some(Open::New(line)) => {
                let Some((item, path)) = &self.target else {
                    return;
                };
                let mut a = item_args(item);
                a["path"] = json!(path);
                a["line"] = json!(line);
                a
            }
            None => return,
        };
        args["body"] = json!(body);
        if pending {
            args["pending"] = json!(true);
        }
        window.dispatch_action(
            Box::new(RunCommand::new(
                "eludite.forge.pull_comment".to_owned(),
                args,
            )),
            cx,
        );
        self.reply.clear();
        self.open = None;
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_none() {
            return;
        }
        match widgets::edit(&mut self.reply, e, true) {
            Edit::Enter => self.post_reply(false, window, cx),
            Edit::Escape => self.open = None,
            Edit::Changed => {}
            Edit::Tab | Edit::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for ThreadMarks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let v = self.view.read(cx);
        let lh = v.line_height();
        let top = v.scroll_position().y;
        // Placed by the editor's layout, which counts CodeLens rows (brief 0052).
        let y = |line: u64| v.row_top(line.saturating_sub(1) as u32) - top;
        let mut root = div().absolute().top_0().left_0().size_full();
        for (ix, (line, thread, _)) in self.marks.iter().enumerate() {
            let color = if thread.resolved {
                t.text_muted
            } else {
                rgb(0xD7_BA_7D)
            };
            root = root.child(
                div()
                    .id(SharedString::from(mark_selector(ix)))
                    .debug_selector(move || mark_selector(ix))
                    .children(eludite_ui::bounds_canvas(
                        self.probe.as_ref(),
                        mark_selector(ix),
                    ))
                    .absolute()
                    .right(px(14.))
                    .top(y(*line))
                    .h(lh)
                    .px_1()
                    .cursor_pointer()
                    .text_color(color)
                    .text_size(px(12.))
                    .child(format!("\u{1F4AC} {}", thread.comments.len()))
                    .on_click(cx.listener(move |this, _, window, cx| this.toggle(ix, window, cx))),
            );
        }
        if self.target.is_some() {
            let line = self.caret_line(cx);
            root = root.child(
                div()
                    .id(NEW_COMMENT)
                    .debug_selector(|| NEW_COMMENT.into())
                    .absolute()
                    .right(px(56.))
                    .top(y(line))
                    .h(lh)
                    .px_1()
                    .cursor_pointer()
                    .text_color(t.text_muted)
                    .text_size(px(12.))
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| this.new_comment(window, cx))),
            );
        }
        let shown = match self.open {
            Some(Open::Thread(i)) => self
                .marks
                .get(i)
                .map(|(line, thread, _)| (*line, Some(thread))),
            Some(Open::New(line)) => Some((line, None)),
            None => None,
        };
        if let Some((line, thread)) = shown {
            let focused = self.focus.is_focused(window);
            let path = match (thread, &self.target) {
                (Some(t), _) => t.path.clone().unwrap_or_default(),
                (None, Some((_, p))) => p.clone(),
                _ => String::new(),
            };
            let mut panel = div()
                .id(POPOVER)
                .debug_selector(|| POPOVER.into())
                .track_focus(&self.focus)
                .key_context("ForgeThread")
                .on_key_down(cx.listener(Self::key))
                .occlude()
                .absolute()
                .right(px(40.))
                .top(y(line) + lh)
                .w(px(420.))
                .max_h(px(360.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .bg(t.popup_background)
                .border_1()
                .border_color(t.border)
                .text_size(t.typography.ui)
                .text_color(t.text)
                .child(
                    div()
                        .text_size(t.typography.small)
                        .text_color(t.text_muted)
                        .child(format!(
                            "{path}:{line}{}",
                            match thread {
                                Some(t) if t.resolved => " \u{00B7} Resolved",
                                Some(_) => "",
                                None => " \u{00B7} New comment",
                            }
                        )),
                );
            for c in thread.iter().flat_map(|t| t.comments.iter()) {
                panel = panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .border_l_2()
                        .border_color(t.border)
                        .pl_1()
                        .child(
                            div()
                                .text_size(t.typography.small)
                                .text_color(t.text_muted)
                                .child(c.author.clone()),
                        )
                        .children(c.body.lines().map(|l| div().child(l.to_owned()))),
                );
            }
            panel = panel.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(
                        text_box(
                            REPLY_BOX,
                            &self.reply,
                            if thread.is_some() { "Reply" } else { "Comment" },
                            focused,
                            &t,
                        )
                        .flex_1(),
                    )
                    .child(push_button(REPLY, "Comment", true, true, &t).on_click(
                        cx.listener(|this, _, window, cx| this.post_reply(false, window, cx)),
                    ))
                    .child(
                        push_button(ADD_TO_REVIEW, "Add to Review", false, true, &t).on_click(
                            cx.listener(|this, _, window, cx| this.post_reply(true, window, cx)),
                        ),
                    ),
            );
            root = root.child(panel);
        }
        root
    }
}
