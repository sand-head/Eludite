//! The Immediate window (brief 0077; Debug > Windows > Immediate, Ctrl+Alt+I): Visual Studio's expression window, the
//! successor brief 0020 named for the retired Debug Console. A line is evaluated in the selected frame with DAP's
//! `repl` context through `eludite.debug.evaluate` (the command agents call too); `?expr` evaluates `expr`; `>`
//! command lines are refused as Visual Studio refuses unknown commands; Up and Down walk the history. The shell writes
//! each answer back with [`ImmediateWindow::push_result`].

use eludite_commands::debug as cmds;
use eludite_ui::{RunCommand, Theme, text_box};
use gpui::{
    App, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, uniform_list,
};
use serde_json::json;

use super::windows::{InputKey, LineInput, ROW_HEIGHT};

/// History entries kept.
pub const HISTORY: usize = 100;
/// Transcript lines kept.
pub const LINES: usize = 5_000;

/// What the shell answers a line with while the debuggee is not at a break.
pub const NOT_IN_BREAK: &str =
    "The expression cannot be evaluated while the debuggee is not stopped at a break.";

/// A transcript line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImmediateLine {
    /// What was typed.
    Input(String),
    Result(String),
    Error(String),
}

impl ImmediateLine {
    pub fn text(&self) -> &str {
        match self {
            ImmediateLine::Input(t) | ImmediateLine::Result(t) | ImmediateLine::Error(t) => t,
        }
    }
}

/// The expression a typed line evaluates: `?expr`, `? expr` and `expr` alike; `None` for a blank line. A `>` line is
/// a Visual Studio command: `Err` with the refusal.
pub fn expression_of(line: &str) -> Option<Result<String, String>> {
    let l = line.trim();
    if l.is_empty() {
        return None;
    }
    if l.starts_with('>') {
        return Some(Err(format!(
            "Command \"{}\" is not valid: the Immediate window evaluates expressions only.",
            l.trim_start_matches('>').trim()
        )));
    }
    let e = l.strip_prefix('?').unwrap_or(l).trim();
    (!e.is_empty()).then(|| Ok(e.to_owned()))
}

pub struct ImmediateWindow {
    theme: Theme,
    lines: Vec<ImmediateLine>,
    input: LineInput,
    history: Vec<String>,
    /// Where Up and Down are in the history (`None`: the line being typed).
    at: Option<usize>,
    /// The debuggee is at a break (the shell keeps it current).
    in_break: bool,
}

impl ImmediateWindow {
    pub fn new(theme: Theme, cx: &mut App) -> Self {
        Self {
            theme,
            lines: Vec::new(),
            input: LineInput::new(cx),
            history: Vec::new(),
            at: None,
            in_break: false,
        }
    }

    pub fn set_in_break(&mut self, on: bool) {
        self.in_break = on;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn lines(&self) -> &[ImmediateLine] {
        &self.lines
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn input_text(&self) -> &str {
        &self.input.text
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_input(&mut self, text: &str, cx: &mut Context<Self>) {
        self.input.text = text.to_owned();
        cx.notify();
    }

    fn push(&mut self, line: ImmediateLine) {
        if self.lines.len() == LINES {
            self.lines.remove(0);
        }
        self.lines.push(line);
    }

    /// An answer: the value (with its type) or the adapter's message.
    pub fn push_result(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.push(if error {
            ImmediateLine::Error(text)
        } else {
            ImmediateLine::Result(text)
        });
        cx.notify();
    }

    /// Enter: evaluate the line (or refuse it) and keep it in the history.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let line = std::mem::take(&mut self.input.text);
        self.at = None;
        let Some(e) = expression_of(&line) else {
            return;
        };
        let line = line.trim().to_owned();
        if self.history.last() != Some(&line) {
            if self.history.len() == HISTORY {
                self.history.remove(0);
            }
            self.history.push(line.clone());
        }
        self.push(ImmediateLine::Input(line));
        match e {
            Err(message) => self.push(ImmediateLine::Error(message)),
            Ok(_) if !self.in_break => self.push(ImmediateLine::Error(NOT_IN_BREAK.into())),
            Ok(expression) => window.dispatch_action(
                Box::new(RunCommand::new(
                    cmds::EVALUATE.to_owned(),
                    json!({"expression": expression, "context": "repl"}),
                )),
                cx,
            ),
        }
        cx.notify();
    }

    /// Up and Down: the previous or next history entry.
    pub fn recall(&mut self, up: bool, cx: &mut Context<Self>) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        self.at = match (self.at, up) {
            (None, true) => Some(last),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i >= last => None,
            (Some(i), false) => Some(i + 1),
        };
        self.input.text = self.at.map(|i| self.history[i].clone()).unwrap_or_default();
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "up" => self.recall(true, cx),
            "down" => self.recall(false, cx),
            "enter" => self.submit(window, cx),
            _ => match self.input.key(e) {
                InputKey::Ignored => return,
                InputKey::Changed | InputKey::Submit(_) => {}
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for ImmediateWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let count = self.lines.len();
        let list = uniform_list(
            "debug-immediate-lines",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                let t = this.theme;
                range
                    .filter_map(|ix| {
                        let l = this.lines.get(ix)?;
                        let color = match l {
                            ImmediateLine::Input(_) => t.text,
                            ImmediateLine::Result(_) => t.text,
                            ImmediateLine::Error(_) => gpui::rgb(0xF14C4C).into(),
                        };
                        let sel = format!("debug-immediate-line-{ix}");
                        Some(
                            div()
                                .id(SharedString::from(sel.clone()))
                                .debug_selector(move || sel)
                                .h(px(ROW_HEIGHT))
                                .px_2()
                                .flex_none()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_color(color)
                                .text_size(t.typography.ui)
                                .child(l.text().to_owned()),
                        )
                    })
                    .collect()
            }),
        )
        .flex_1();
        let focused = self.input.focus.is_focused(window);
        div()
            .id("debug-immediate")
            .size_full()
            .flex()
            .flex_col()
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(px(ROW_HEIGHT + 6.))
                    .items_center()
                    .px_2()
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        text_box(
                            "debug-immediate-input",
                            &self.input.text,
                            "?expression",
                            focused,
                            &t,
                        )
                        .track_focus(&self.input.focus)
                        .key_context("DebugImmediate")
                        .on_key_down(cx.listener(Self::key))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.input.focus.focus(window, cx);
                            cx.notify();
                        })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_become_expressions_as_visual_studio_reads_them() {
        assert_eq!(expression_of("?x"), Some(Ok("x".into())));
        assert_eq!(expression_of("? x + 1"), Some(Ok("x + 1".into())));
        assert_eq!(expression_of("order.Name"), Some(Ok("order.Name".into())));
        assert_eq!(expression_of("   "), None);
        assert_eq!(expression_of("?"), None);
        assert!(matches!(expression_of(">Debug.Start"), Some(Err(m)) if m.contains("Debug.Start")));
    }
}
