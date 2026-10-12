//! The Quick Find and Quick Replace bar (brief 0068): Visual Studio's find box at the editor's top right, with a
//! find box and, for Quick Replace, a replace box (both [`TextInput`]s), Match Case (Alt+C), Match Whole Word
//! (Alt+W), Use Regular Expressions (Alt+E), Find in Selection (Alt+I), Find Previous and Next, Replace Next (Alt+R)
//! and Replace All (Alt+A), "3 of 17", and the find box's history on Up and Down.
//!
//! A child module of `view`: the bar is part of the view's element tree and its state is in the view's
//! [`super::view_editing_state`].

use gpui::{
    Context, Entity, Focusable, IntoElement, MouseButton, ParentElement, SharedString, Styled,
    Window, div, prelude::*, px,
};

use super::EditorView;
use super::editing::FIND_BAR_CONTEXT;
use crate::editor::FindQuery;
use crate::input::{TextInput, TextInputEvent};

/// How many queries the find box remembers.
const HISTORY: usize = 20;

impl EditorView {
    /// Ctrl+F (Quick Find) or Ctrl+H (Quick Replace): open the bar with a single-line selection as the query (a
    /// multi-line selection turns Find in Selection on), focus the find box and select its text.
    pub(super) fn open_find_bar(
        &mut self,
        replace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = self.editor.primary_selection();
        if !sel.is_empty() {
            let text = self.editor.buffer().text_for_range(sel.range());
            if text.contains('\n') {
                self.editor.set_find_in_selection(true);
            } else {
                let q = FindQuery {
                    text,
                    case_sensitive: self.editor.find_query().case_sensitive,
                };
                self.editor.set_find_query(q);
            }
        }
        self.find_bar_open = true;
        self.editing.replace_mode = replace;
        let find = self.ensure_find_inputs(window, cx);
        let query = self.editor.find_query().text.clone();
        find.update(cx, |input, cx| {
            if input.text() != query {
                input.set_text(&query, cx);
            }
            input.select_all(cx);
        });
        let focus = find.focus_handle(cx);
        window.focus(&focus, cx);
        self.recount_find_matches();
        cx.notify();
    }

    fn ensure_find_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(find) = &self.editing.find_input {
            return find.clone();
        }
        let style = self.style.clone();
        let find = cx.new(|cx| {
            let mut input = TextInput::new(false, cx);
            input.set_placeholder("Find...", cx);
            input.set_style(style.clone(), cx);
            input
        });
        let replace = cx.new(|cx| {
            let mut input = TextInput::new(false, cx);
            input.set_placeholder("Replace...", cx);
            input.set_style(style, cx);
            input
        });
        let s1 = cx.subscribe_in(
            &find,
            window,
            |this, input, event, window, cx| match event {
                TextInputEvent::Changed => {
                    let text = input.read(cx).text();
                    if text != this.editor.find_query().text {
                        let q = FindQuery {
                            text,
                            case_sensitive: this.editor.find_query().case_sensitive,
                        };
                        this.set_find_text(q, cx);
                    }
                }
                TextInputEvent::Submit => {
                    this.remember_query();
                    this.movement(cx, |e| {
                        e.find_next();
                    });
                }
                TextInputEvent::Escape => this.close_find_bar(window, cx),
                TextInputEvent::Up => this.recall_history(1, input.clone(), cx),
                TextInputEvent::Down => this.recall_history(-1, input.clone(), cx),
            },
        );
        let s2 = cx.subscribe_in(&replace, window, |this, _, event, window, cx| match event {
            TextInputEvent::Submit => this.replace_next(cx),
            TextInputEvent::Escape => this.close_find_bar(window, cx),
            _ => {}
        });
        self.editing.subscriptions.extend([s1, s2]);
        self.editing.find_input = Some(find.clone());
        self.editing.replace_input = Some(replace);
        find
    }

    pub(super) fn close_find_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_bar_open = false;
        self.editor.set_find_in_selection(false);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn remember_query(&mut self) {
        let q = self.editor.find_query().text.clone();
        if q.is_empty() {
            return;
        }
        let h = &mut self.editing.history;
        h.retain(|t| *t != q);
        h.insert(0, q);
        h.truncate(HISTORY);
        self.editing.history_index = None;
    }

    /// Up (older) and Down (newer) through the find box's history.
    fn recall_history(&mut self, by: isize, input: Entity<TextInput>, cx: &mut Context<Self>) {
        let h = &self.editing.history;
        if h.is_empty() {
            return;
        }
        let next = match self.editing.history_index {
            None if by > 0 => 0,
            None => return,
            Some(i) => i as isize + by,
        };
        if next < 0 || next as usize >= h.len() {
            return;
        }
        self.editing.history_index = Some(next as usize);
        let text = h[next as usize].clone();
        input.update(cx, |i, cx| {
            i.set_text(&text, cx);
            i.select_all(cx);
        });
    }

    /// The text the replace box holds.
    fn replacement(&self, cx: &gpui::App) -> String {
        self.editing
            .replace_input
            .as_ref()
            .map(|i| i.read(cx).text())
            .unwrap_or_default()
    }

    pub(super) fn replace_next(&mut self, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let replacement = self.replacement(cx);
        self.remember_query();
        self.editor.replace_next(&replacement);
        self.changed(cx);
        self.after_other_change(cx);
    }

    /// Replace All: one undo step; the count shows in the bar until the query changes.
    pub(super) fn replace_all(&mut self, cx: &mut Context<Self>) -> usize {
        if self.read_only {
            return 0;
        }
        let replacement = self.replacement(cx);
        self.remember_query();
        let n = self.editor.replace_all(&replacement);
        self.editing.replaced = Some(n);
        self.changed(cx);
        self.after_other_change(cx);
        n
    }

    pub(super) fn toggle_find_option(
        &mut self,
        f: impl FnOnce(&mut crate::find::SearchOptions),
        cx: &mut Context<Self>,
    ) {
        let mut o = self.editor.find_options();
        f(&mut o);
        self.editor.set_find_options(o);
        let q = self.editor.find_query().clone();
        self.set_find_text(q, cx);
    }

    pub(super) fn toggle_find_in_selection(&mut self, cx: &mut Context<Self>) {
        let on = !self.editor.find_in_selection();
        self.editor.set_find_in_selection(on);
        self.recount_find_matches();
        cx.notify();
    }

    /// Ctrl+F3 and Ctrl+Shift+F3: find the selected text (the word at a caret) without the bar.
    pub(super) fn find_selected(&mut self, forward: bool, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.editor.primary_selection();
        let range = if sel.is_empty() {
            self.editor.word_at(sel.head)
        } else {
            sel.range()
        };
        if range.is_empty() {
            return;
        }
        let text = self.editor.buffer().text_for_range(range.clone());
        if text.contains('\n') {
            return;
        }
        self.editor.set_find_query(FindQuery {
            text,
            case_sensitive: self.editor.find_query().case_sensitive,
        });
        let mut o = self.editor.find_options();
        o.regex = false;
        self.editor.set_find_options(o);
        self.editor.set_selections(
            vec![crate::editor::SelectionRange {
                tail: range.start,
                head: range.end,
            }],
            0,
        );
        self.movement(cx, |e| {
            if forward {
                e.find_next();
            } else {
                e.find_previous();
            }
        });
    }

    /// "3 of 17", "No results", or the pattern's error.
    pub fn find_status(&self) -> String {
        if let Some(e) = self.editor.find_error() {
            return e;
        }
        if let Some(n) = self.editing.replaced {
            return format!("{n} replaced");
        }
        let n = self.editor.find_match_count();
        if self.editor.find_query().text.is_empty() {
            return String::new();
        }
        if n == 0 {
            return "No results".into();
        }
        match self.editor.find_match_index() {
            Some(i) => format!("{} of {n}", i + 1),
            None => format!("{n} matches"),
        }
    }

    /// The replace box is shown (Quick Replace).
    pub fn is_quick_replace(&self) -> bool {
        self.find_bar_open && self.editing.replace_mode
    }

    /// The find box, for tests and the shell (focus, text).
    pub fn find_input(&self) -> Option<&Entity<TextInput>> {
        self.editing.find_input.as_ref()
    }

    /// The replace box.
    pub fn replace_input(&self) -> Option<&Entity<TextInput>> {
        self.editing.replace_input.as_ref()
    }

    pub(super) fn render_find_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = self.style.theme;
        let options = self.editor.find_options();
        let in_selection = self.editor.find_in_selection();
        let status = self.find_status();
        let replace_mode = self.editing.replace_mode;
        let toggle = |id: &'static str, label: &'static str, on: bool, _tip: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .when(on, |d| d.bg(theme.accent).text_color(theme.panel))
                .child(SharedString::from(label))
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .hover(|d| d.bg(theme.panel_raised))
                .child(SharedString::from(label))
        };
        let find = self.editing.find_input.clone();
        let replace = self.editing.replace_input.clone();
        let find_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                button("find-bar-expand", if replace_mode { "v" } else { ">" }).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.editing.replace_mode = !this.editing.replace_mode;
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .w(px(220.))
                    .border_1()
                    .border_color(theme.border)
                    .children(find),
            )
            .child(
                toggle(
                    "find-bar-case",
                    "Aa",
                    self.editor.find_query().case_sensitive,
                    "Match case (Alt+C)",
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, w, cx| {
                        this.toggle_case(&super::ToggleFindCaseSensitive, w, cx)
                    }),
                ),
            )
            .child(
                toggle(
                    "find-bar-word",
                    "W",
                    options.whole_word,
                    "Match whole word (Alt+W)",
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_find_option(|o| o.whole_word = !o.whole_word, cx)
                    }),
                ),
            )
            .child(
                toggle(
                    "find-bar-regex",
                    ".*",
                    options.regex,
                    "Use regular expressions (Alt+E)",
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_find_option(|o| o.regex = !o.regex, cx)
                    }),
                ),
            )
            .child(
                toggle(
                    "find-bar-selection",
                    "Sel",
                    in_selection,
                    "Find in selection (Alt+I)",
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.toggle_find_in_selection(cx)),
                ),
            )
            .child(button("find-bar-previous", "<").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.movement(cx, |e| {
                        e.find_previous();
                    })
                }),
            ))
            .child(button("find-bar-next", ">").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.movement(cx, |e| {
                        e.find_next();
                    })
                }),
            ))
            .child(div().min_w(px(80.)).child(SharedString::from(status)))
            .child(button("find-bar-close", "x").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, w, cx| this.close_find_bar(w, cx)),
            ));
        let replace_row = replace_mode.then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .pl(px(18.))
                .child(
                    div()
                        .w(px(220.))
                        .border_1()
                        .border_color(theme.border)
                        .children(replace),
                )
                .child(
                    button("find-bar-replace-next", "Replace Next").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.replace_next(cx)),
                    ),
                )
                .child(button("find-bar-replace-all", "Replace All").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.replace_all(cx);
                    }),
                ))
        });
        div()
            .id("find-bar")
            .key_context(FIND_BAR_CONTEXT)
            .absolute()
            .top_0()
            .right(px(16.))
            .p_1()
            .flex()
            .flex_col()
            .gap_1()
            .bg(theme.panel)
            .border_1()
            .border_color(theme.accent)
            .text_color(theme.text)
            .text_size(theme.typography.ui)
            .cursor_default()
            .child(find_row)
            .children(replace_row)
    }
}
