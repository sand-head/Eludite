//! The exception helper (brief 0077): Visual Studio's popup at the execution point of an exception stop, anchored
//! under the statement's line in the editor ([`eludite_editor::EditorView::set_anchored_popup`]). It shows "Exception
//! Thrown" or "Exception User-Unhandled", the exception's type and message and its inner exceptions from DAP's
//! `exceptionInfo` details, the links View Details (QuickWatch on `$exception`), Copy Details and Open Exception
//! Settings, and the box "Break when this exception type is thrown", which writes the type through
//! `eludite.debug.exception_settings`. Every link but Copy Details is a command.

use eludite_commands::debug::{self as cmds, ExceptionSettingsRow};
use eludite_dap::types::{ExceptionDetails, ExceptionInfoResponse};
use eludite_ui::{RunCommand, Theme};
use gpui::{
    Context, EventEmitter, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use serde_json::json;

/// The editor popup key.
pub const POPUP: &str = "exception-helper";

/// What the helper shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HelperContent {
    pub title: String,
    pub type_name: String,
    pub message: String,
    /// Inner exceptions, outermost first: (depth from 1, type, message).
    pub inner: Vec<(usize, String, String)>,
    /// "Break when this exception type is thrown".
    pub break_when_thrown: bool,
    /// Whether the type has its own row in the exception settings.
    pub has_row: bool,
    pub break_when_user_unhandled: bool,
    /// The stack trace from the details, for Copy Details.
    pub stack_trace: Option<String>,
}

impl HelperContent {
    /// The helper for an `exceptionInfo` answer under the current exception settings.
    pub fn new(e: &ExceptionInfoResponse, settings: &ExceptionSettingsRow) -> Self {
        let title = match e.break_mode.as_str() {
            "userUnhandled" | "unhandled" => "Exception User-Unhandled",
            _ => "Exception Thrown",
        };
        let details = e.details.as_ref();
        let type_name = details
            .and_then(|d| d.full_type_name.clone().or(d.type_name.clone()))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| e.exception_id.clone());
        let message = details
            .and_then(|d| d.message.clone())
            .or_else(|| e.description.clone())
            .unwrap_or_default();
        let mut inner = Vec::new();
        fn walk(d: &ExceptionDetails, depth: usize, out: &mut Vec<(usize, String, String)>) {
            for i in &d.inner_exception {
                if out.len() >= cmds::MAX_INNER_EXCEPTIONS {
                    return;
                }
                out.push((
                    depth,
                    i.full_type_name
                        .clone()
                        .or(i.type_name.clone())
                        .unwrap_or_default(),
                    i.message.clone().unwrap_or_default(),
                ));
                walk(i, depth + 1, out);
            }
        }
        if let Some(d) = details {
            walk(d, 1, &mut inner);
        }
        let row = settings.types.iter().find(|t| t.type_name == type_name);
        Self {
            title: title.into(),
            break_when_thrown: row.map_or(settings.break_when_thrown, |r| r.break_when_thrown),
            break_when_user_unhandled: row.map_or(settings.break_when_user_unhandled, |r| {
                r.break_when_user_unhandled
            }),
            has_row: row.is_some(),
            type_name,
            message,
            inner,
            stack_trace: details.and_then(|d| d.stack_trace.clone()),
        }
    }

    /// Copy Details: Visual Studio's text.
    pub fn details_text(&self) -> String {
        let mut s = format!("{}\n  Message={}\n", self.type_name, self.message);
        if let Some(t) = &self.stack_trace {
            s.push_str(&format!("  StackTrace:\n{t}\n"));
        }
        for (depth, t, m) in &self.inner {
            s.push_str(&format!(
                "{}Inner Exception {depth}:\n{}{t}: {m}\n",
                "  ".repeat(*depth),
                "  ".repeat(*depth)
            ));
        }
        s
    }

    /// The box's command: the type's row with Thrown toggled.
    pub fn toggle_args(&self) -> serde_json::Value {
        json!({"types": [{
            "type": self.type_name,
            "break_when_thrown": !self.break_when_thrown,
            "break_when_user_unhandled": self.break_when_user_unhandled,
        }]})
    }
}

/// The helper asks to be closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseHelper;

pub struct ExceptionHelperView {
    theme: Theme,
    content: HelperContent,
}

impl EventEmitter<CloseHelper> for ExceptionHelperView {}

impl ExceptionHelperView {
    pub fn new(theme: Theme, content: HelperContent) -> Self {
        Self { theme, content }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn content(&self) -> &HelperContent {
        &self.content
    }

    pub fn set_content(&mut self, content: HelperContent, cx: &mut Context<Self>) {
        if self.content != content {
            self.content = content;
            cx.notify();
        }
    }

    /// The box was clicked: write the type's row.
    pub fn toggle_break(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let args = self.content.toggle_args();
        self.content.break_when_thrown = !self.content.break_when_thrown;
        self.content.has_row = true;
        window.dispatch_action(
            Box::new(RunCommand::new(cmds::EXCEPTION_SETTINGS.to_owned(), args)),
            cx,
        );
        cx.notify();
    }
}

fn link(t: &Theme, id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .text_color(t.accent)
        .cursor_pointer()
        .hover(|s| s.underline())
        .child(label)
}

impl Render for ExceptionHelperView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let c = &self.content;
        let inner = c.inner.iter().map(|(depth, ty, m)| {
            div()
                .pl(px(12. * *depth as f32))
                .text_color(t.text_muted)
                .child(format!("Inner Exception {depth}: {ty}: {m}"))
        });
        let check = if c.break_when_thrown {
            "\u{2611}"
        } else {
            "\u{2610}"
        };
        eludite_ui::popup::popup_panel(&t)
            .id(POPUP)
            .debug_selector(|| POPUP.to_owned())
            .occlude()
            .w(px(480.))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .text_size(t.typography.ui)
            .border_l_4()
            .border_color(gpui::rgb(0xE51400))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(c.title.clone()),
                    )
                    .child(
                        div()
                            .id("exception-helper-close")
                            .debug_selector(|| "exception-helper-close".to_owned())
                            .cursor_pointer()
                            .text_color(t.text_muted)
                            .child("\u{2715}")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseHelper))),
                    ),
            )
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(c.type_name.clone()),
            )
            .child(div().child(SharedString::from(c.message.clone())))
            .children(inner)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .pt_1()
                    .child(
                        link(&t, "exception-helper-details", "View Details").on_click(cx.listener(
                            |_, _, window, cx| {
                                window.dispatch_action(
                                    Box::new(RunCommand::new(
                                        eludite_commands::debug_inspect::QUICKWATCH.to_owned(),
                                        json!({"expression": "$exception"}),
                                    )),
                                    cx,
                                )
                            },
                        )),
                    )
                    .child(
                        link(&t, "exception-helper-copy", "Copy Details").on_click(cx.listener(
                            |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    this.content.details_text(),
                                ))
                            },
                        )),
                    )
                    .child(
                        link(&t, "exception-helper-settings", "Open Exception Settings").on_click(
                            cx.listener(|_, _, window, cx| {
                                window.dispatch_action(
                                    Box::new(RunCommand::new(
                                        "eludite.view.show".to_owned(),
                                        json!({"id": eludite_docking::ids::EXCEPTION_SETTINGS}),
                                    )),
                                    cx,
                                )
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(t.border)
                    .pt_1()
                    .child(div().text_color(t.text_muted).child("Exception Settings"))
                    .child(
                        div()
                            .id("exception-helper-break")
                            .debug_selector(|| "exception-helper-break".to_owned())
                            .flex()
                            .flex_row()
                            .gap_1()
                            .cursor_pointer()
                            .child(check)
                            .child("Break when this exception type is thrown")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_break(window, cx)),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(mode: &str) -> ExceptionInfoResponse {
        serde_json::from_value(json!({
            "exceptionId": "System.InvalidOperationException", "description": "boom", "breakMode": mode,
            "details": {"message": "boom", "typeName": "InvalidOperationException",
                        "fullTypeName": "System.InvalidOperationException", "stackTrace": "   at Program.Main()",
                        "innerException": [{"message": "inner", "typeName": "FormatException",
                                            "innerException": [{"message": "deepest", "typeName": "IOException"}]}]}
        }))
        .unwrap()
    }

    #[test]
    fn content_follows_the_break_mode_the_details_and_the_settings() {
        let mut settings = ExceptionSettingsRow::default();
        let c = HelperContent::new(&info("always"), &settings);
        assert_eq!(c.title, "Exception Thrown");
        assert_eq!(c.type_name, "System.InvalidOperationException");
        assert_eq!(c.message, "boom");
        assert_eq!(
            c.inner,
            [
                (1, "FormatException".into(), "inner".into()),
                (2, "IOException".into(), "deepest".into())
            ]
        );
        assert!(!c.break_when_thrown && !c.has_row);
        assert!(c.details_text().contains("at Program.Main()"));
        assert_eq!(
            c.toggle_args()["types"][0]["break_when_thrown"],
            json!(true)
        );
        assert_eq!(
            HelperContent::new(&info("userUnhandled"), &settings).title,
            "Exception User-Unhandled"
        );
        settings.types.push(cmds::ExceptionTypeRow {
            type_name: "System.InvalidOperationException".into(),
            break_when_thrown: true,
            break_when_user_unhandled: false,
        });
        let c = HelperContent::new(&info("always"), &settings);
        assert!(c.break_when_thrown && c.has_row && !c.break_when_user_unhandled);
    }
}
