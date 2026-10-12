//! QuickWatch (brief 0077; Shift+F9, Debug > QuickWatch...): Visual Studio's modal dialog on one expression: the
//! Expression box with its history (Up and Down), Reevaluate, Add Watch and Close, and the value as a variables tree
//! (kind [`super::windows::VarsKind::QuickWatch`]: expanding lazily, values editable in place). Reevaluate is
//! `eludite.debug.quickwatch` and Add Watch `eludite.debug.watch`, the commands agents call.

use eludite_commands::debug as cmds;
use eludite_commands::debug_inspect::QUICKWATCH;
use eludite_ui::{RunCommand, Theme, text_box, toggle_button};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px,
};
use serde_json::json;

use super::windows::{InputKey, LineInput, ROW_HEIGHT, VarsWindow};

pub const DIALOG: &str = "quickwatch-dialog";
pub const REEVALUATE: &str = "quickwatch-reevaluate";
pub const ADD_WATCH: &str = "quickwatch-add-watch";
pub const CLOSE: &str = "quickwatch-close";
/// History entries kept.
const HISTORY: usize = 25;

/// The dialog asks to be closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseQuickWatch;

pub struct QuickWatchDialog {
    theme: Theme,
    input: LineInput,
    /// The expression the tree shows.
    shown: String,
    history: Vec<String>,
    at: Option<usize>,
    pub tree: Entity<VarsWindow>,
}

impl EventEmitter<CloseQuickWatch> for QuickWatchDialog {}

impl gpui::Focusable for QuickWatchDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.input.focus.clone()
    }
}

impl QuickWatchDialog {
    pub fn new(theme: Theme, tree: Entity<VarsWindow>, cx: &mut App) -> Self {
        Self {
            theme,
            input: LineInput::new(cx),
            shown: String::new(),
            history: Vec::new(),
            at: None,
            tree,
        }
    }

    /// The dialog now shows `expression` (opened or reevaluated).
    pub fn show(&mut self, expression: &str, cx: &mut Context<Self>) {
        self.input.text = expression.to_owned();
        self.shown = expression.to_owned();
        self.at = None;
        if self.history.last().map(String::as_str) != Some(expression) {
            if self.history.len() == HISTORY {
                self.history.remove(0);
            }
            self.history.push(expression.to_owned());
        }
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn expression(&self) -> &str {
        &self.input.text
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn history(&self) -> &[String] {
        &self.history
    }

    fn command(window: &mut Window, cx: &mut App, id: &str, args: serde_json::Value) {
        window.dispatch_action(Box::new(RunCommand::new(id.to_owned(), args)), cx);
    }

    pub fn reevaluate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let e = self.input.text.trim().to_owned();
        if !e.is_empty() {
            Self::command(window, cx, QUICKWATCH, json!({ "expression": e }));
        }
    }

    pub fn add_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let e = self.input.text.trim().to_owned();
        if !e.is_empty() {
            Self::command(window, cx, cmds::WATCH, json!({ "add": e }));
        }
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => cx.emit(CloseQuickWatch),
            "enter" => self.reevaluate(window, cx),
            "up" | "down" if !self.history.is_empty() => {
                let last = self.history.len() - 1;
                self.at = Some(match (self.at, e.keystroke.key.as_str()) {
                    (None, _) => last,
                    (Some(i), "up") => i.saturating_sub(1),
                    (Some(i), _) => (i + 1).min(last),
                });
                self.input.text = self.history[self.at.unwrap_or(last)].clone();
            }
            _ => {
                if matches!(self.input.key(e), InputKey::Ignored) {
                    return;
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for QuickWatchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.input.focus.is_focused(window);
        let panel =
            div()
                .id(DIALOG)
                .debug_selector(|| DIALOG.to_owned())
                .occlude()
                .w(px(640.))
                .h(px(420.))
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .bg(t.panel)
                .border_1()
                .border_color(t.border)
                .rounded_md()
                .shadow_lg()
                .text_size(t.typography.ui)
                .text_color(t.text)
                .child(div().font_weight(FontWeight::SEMIBOLD).child("QuickWatch"))
                .child("Expression:")
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .items_center()
                        .child(
                            text_box(
                                "quickwatch-expression",
                                &self.input.text,
                                "Expression",
                                focused,
                                &t,
                            )
                            .flex_1()
                            .track_focus(&self.input.focus)
                            .key_context("DebugQuickWatch")
                            .on_key_down(cx.listener(Self::key))
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.input.focus.focus(window, cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(toggle_button(REEVALUATE, "Reevaluate", false, &t).on_click(
                            cx.listener(|this, _, window, cx| this.reevaluate(window, cx)),
                        ))
                        .child(toggle_button(ADD_WATCH, "Add Watch", false, &t).on_click(
                            cx.listener(|this, _, window, cx| this.add_watch(window, cx)),
                        )),
                )
                .child("Value:")
                .child(
                    div()
                        .flex_1()
                        .min_h(px(ROW_HEIGHT * 4.))
                        .border_1()
                        .border_color(t.border)
                        .bg(t.background)
                        .child(self.tree.clone()),
                )
                .child(
                    div().flex().flex_row().justify_end().child(
                        toggle_button(CLOSE, "Close", false, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseQuickWatch))),
                    ),
                );
        let viewport = window.viewport_size();
        let at = gpui::point(
            ((viewport.width - px(640.)) / 2.).max(px(0.)),
            (viewport.height / 8.).max(px(0.)),
        );
        gpui::deferred(gpui::anchored().position(at).child(panel)).with_priority(5)
    }
}
