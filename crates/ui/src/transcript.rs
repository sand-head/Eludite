//! The Agents window's transcript pieces (brief 0016): the user's prompt, a line of agent text, the collapsed
//! thinking block, a tool call card with its status, arguments and result, a plan, notices, and the status badge.
//! Stateless: the shell owns the transcript, the expansion state and the click handlers.

use gpui::{
    Div, FontWeight, InteractiveElement, ParentElement, Rgba, SharedString, Stateful, Styled, div,
    px, rgb,
};

use crate::Theme;

/// A tool call's state as the transcript shows it (brief 0016 Contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Announced, not started.
    Pending,
    Running,
    /// Permission granted by policy, without asking.
    AllowedWithoutPrompt,
    /// Waiting for the user's answer.
    AwaitingPermission,
    /// Waiting for the user's review of its edits.
    AwaitingReview,
    Denied,
    Failed,
    Completed,
}

impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            ToolStatus::Pending => "pending",
            ToolStatus::Running => "running",
            ToolStatus::AllowedWithoutPrompt => "allowed without prompt",
            ToolStatus::AwaitingPermission => "awaiting permission",
            ToolStatus::AwaitingReview => "awaiting review",
            ToolStatus::Denied => "denied",
            ToolStatus::Failed => "failed",
            ToolStatus::Completed => "completed",
        }
    }

    pub fn color(self) -> Rgba {
        match self {
            ToolStatus::Completed | ToolStatus::AllowedWithoutPrompt => rgb(0x89D185),
            ToolStatus::Denied | ToolStatus::Failed => rgb(0xF48771),
            ToolStatus::AwaitingPermission | ToolStatus::AwaitingReview => rgb(0xCCA700),
            ToolStatus::Pending | ToolStatus::Running => rgb(0x3794FF),
        }
    }
}

/// A small colored label.
pub fn status_badge(status: ToolStatus, theme: &Theme) -> Div {
    div()
        .flex_none()
        .px_1()
        .border_1()
        .border_color(status.color())
        .text_color(status.color())
        .text_size(theme.typography.small)
        .child(status.label())
}

/// The user's prompt.
pub fn user_prompt(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div().w_full().px_2().py_1().child(
        div()
            .px_2()
            .py_1()
            .bg(theme.menu_hover)
            .border_l_2()
            .border_color(theme.accent)
            .text_color(theme.text)
            .child(text.into()),
    )
}

/// One line of the agent's message (an empty line keeps its height).
pub fn agent_line(text: impl Into<SharedString>, theme: &Theme) -> Div {
    let text: SharedString = text.into();
    let line = div().w_full().px_3().text_color(theme.text);
    if text.is_empty() {
        line.h(px(8.))
    } else {
        line.child(text)
    }
}

/// The agent's thinking: one muted line when collapsed (the default), the whole text when expanded. The caller adds
/// the click that toggles it.
pub fn thought_block(
    id: impl Into<SharedString>,
    text: &str,
    expanded: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    let first = text.lines().next().unwrap_or_default();
    let body: SharedString = if expanded {
        text.to_owned().into()
    } else {
        let short: String = first.chars().take(80).collect();
        if short.len() < text.len() {
            format!("{short}\u{2026}").into()
        } else {
            short.into()
        }
    };
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .w_full()
        .px_3()
        .py_px()
        .cursor_pointer()
        .italic()
        .text_color(theme.text_muted)
        .child(
            div()
                .flex()
                .gap_1()
                .child(if expanded { "\u{25BE}" } else { "\u{25B8}" })
                .child("Thinking")
                .child(div().flex_1().overflow_hidden().child(body)),
        )
}

/// A tool call card: name, kind and status on the first line, then the arguments, the result, and an optional link
/// line (the audit record and the change it made). The caller adds handlers to the link through its id.
pub struct ToolCard<'a> {
    pub id: SharedString,
    pub name: &'a str,
    pub kind: &'a str,
    pub status: ToolStatus,
    pub arguments: &'a str,
    pub result: &'a str,
    pub note: Option<&'a str>,
}

pub fn tool_call_card(card: ToolCard<'_>, theme: &Theme, mono: SharedString) -> Stateful<Div> {
    let selector = card.id.clone();
    let mut body = div()
        .flex()
        .flex_col()
        .gap_px()
        .p_1()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.accent)
                        .child("Tool"),
                )
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .font_family(mono.clone())
                        .child(SharedString::from(card.name.to_owned())),
                )
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .text_size(theme.typography.small)
                        .child(SharedString::from(card.kind.to_owned())),
                )
                .child(status_badge(card.status, theme)),
        );
    if !card.arguments.is_empty() {
        body = body.child(
            div()
                .font_family(mono.clone())
                .text_size(theme.typography.small)
                .text_color(theme.text_muted)
                .child(SharedString::from(card.arguments.to_owned())),
        );
    }
    if !card.result.is_empty() {
        body = body.child(
            div()
                .font_family(mono)
                .text_size(theme.typography.small)
                .text_color(theme.text)
                .child(SharedString::from(card.result.to_owned())),
        );
    }
    if let Some(note) = card.note {
        body = body.child(
            div()
                .text_size(theme.typography.small)
                .text_color(theme.accent)
                .child(SharedString::from(note.to_owned())),
        );
    }
    div()
        .id(card.id)
        .debug_selector(move || selector.to_string())
        .w_full()
        .px_2()
        .py_px()
        .child(body)
}

/// A plan update: one line per entry with its status mark.
pub fn plan_card(entries: &[(String, String)], theme: &Theme) -> Div {
    let mut col = div()
        .flex()
        .flex_col()
        .p_1()
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child("Plan"),
        );
    for (content, status) in entries {
        let mark = match status.as_str() {
            "completed" => "\u{2611}",
            "in_progress" => "\u{25B6}",
            _ => "\u{2610}",
        };
        col = col.child(
            div()
                .flex()
                .gap_1()
                .text_color(if status == "completed" {
                    theme.text_muted
                } else {
                    theme.text
                })
                .child(mark)
                .child(SharedString::from(content.clone())),
        );
    }
    div().w_full().px_2().py_px().child(col)
}

/// A muted line: turn ends, state changes, login instructions.
pub fn notice(text: impl Into<SharedString>, error: bool, theme: &Theme) -> Div {
    div()
        .w_full()
        .px_3()
        .py_px()
        .text_size(theme.typography.small)
        .text_color(if error {
            rgb(0xF48771)
        } else {
            theme.text_muted
        })
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_have_labels_and_distinct_alarm_colors() {
        use ToolStatus::*;
        let all = [
            Pending,
            Running,
            AllowedWithoutPrompt,
            AwaitingPermission,
            AwaitingReview,
            Denied,
            Failed,
            Completed,
        ];
        let mut labels: Vec<_> = all.iter().map(|s| s.label()).collect();
        labels.dedup();
        assert_eq!(labels.len(), all.len());
        assert_eq!(AllowedWithoutPrompt.label(), "allowed without prompt");
        assert_ne!(Denied.color(), Completed.color());
        assert_ne!(AwaitingPermission.color(), Running.color());
        assert_eq!(Denied.color(), Failed.color());
    }
}
