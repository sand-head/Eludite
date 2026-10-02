//! Small stateless elements shared by the docking view and the shell.

use std::ops::Range;

use gpui::{
    Div, FontWeight, HighlightStyle, InteractiveElement, ParentElement, SharedString, Stateful,
    Styled, StyledText, div, px,
};

use crate::Theme;

/// How a tab is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TabStyle {
    pub active: bool,
    /// Visual Studio's preview tab: italic, at the right end.
    pub preview: bool,
    pub pinned: bool,
}

/// A document or tool window tab. The caller adds the id and handlers.
pub fn tab(theme: &Theme, title: impl Into<SharedString>, style: TabStyle) -> Div {
    let title: SharedString = title.into();
    let label = if style.pinned {
        SharedString::from(format!("{title}  \u{2022}"))
    } else {
        title
    };
    let el = div()
        .flex()
        .flex_none()
        .items_center()
        .h_full()
        .px_3()
        .text_size(theme.typography.ui)
        .child(label);
    let el = if style.preview { el.italic() } else { el };
    if style.active {
        el.bg(theme.accent).text_color(theme.text_on_accent)
    } else {
        el.text_color(theme.chrome_text)
            .hover(|s| s.bg(theme.menu_hover))
    }
}

/// A small text button on a tool window title bar.
pub fn icon_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .min_w(px(16.))
        .h(px(16.))
        .px_1()
        .text_size(theme.typography.small)
        .cursor_pointer()
        .hover(|s| s.bg(theme.menu_hover))
        .child(label.into())
}

/// A toolbar toggle button (the Error List's Errors, Warnings and Messages filters): drawn pressed while `on`, with
/// element id `id`. The caller adds the click handler.
pub fn toggle_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    let el = div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px_2()
        .border_1()
        .text_size(theme.typography.ui)
        .cursor_pointer()
        .child(label.into());
    if on {
        el.bg(theme.menu_hover)
            .border_color(theme.accent)
            .text_color(theme.text)
    } else {
        el.border_color(gpui::transparent_black())
            .text_color(theme.text_muted)
            .hover(|s| s.bg(theme.menu_hover))
    }
}

/// A check box with its label (the Options dialog's switches, brief 0020): Visual Studio's square, checked or not,
/// with element id `id`. The caller adds the click handler.
pub fn check_box(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    checked: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    let square = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(13.))
        .h(px(13.))
        .border_1()
        .border_color(if checked { theme.accent } else { theme.border })
        .bg(theme.background)
        .text_size(theme.typography.small)
        .text_color(theme.text)
        .child(if checked { "\u{2713}" } else { "" });
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap_2()
        .h(px(20.))
        .text_size(theme.typography.ui)
        .text_color(theme.text)
        .cursor_pointer()
        .child(square)
        .child(label.into())
}

/// A one-line text box (the Error List's search box): `text`, or `placeholder` muted when empty, with a caret
/// while `focused`. The caller owns the text, the focus and the key handling.
pub fn text_box(
    id: impl Into<SharedString>,
    text: &str,
    placeholder: &str,
    focused: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    let shown: SharedString = if text.is_empty() && !focused {
        placeholder.to_owned().into()
    } else if focused {
        format!("{text}\u{2502}").into()
    } else {
        text.to_owned().into()
    };
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_none()
        .items_center()
        .w(px(220.))
        .h(px(20.))
        .px_1()
        .overflow_hidden()
        .whitespace_nowrap()
        .bg(theme.background)
        .border_1()
        .border_color(if focused { theme.accent } else { theme.border })
        .text_size(theme.typography.ui)
        .text_color(if text.is_empty() {
            theme.text_muted
        } else {
            theme.text
        })
        .cursor_text()
        .child(shown)
}

/// A line of code with `highlight` (a byte range of `text`) drawn bold on the highlight background, as Find All
/// References shows the symbol in each result. An invalid range is ignored.
pub fn highlighted_code(
    text: impl Into<SharedString>,
    highlight: Range<usize>,
    theme: &Theme,
) -> StyledText {
    let text: SharedString = text.into();
    let valid = highlight.start < highlight.end
        && highlight.end <= text.len()
        && text.is_char_boundary(highlight.start)
        && text.is_char_boundary(highlight.end);
    // The accent, translucent: readable on every theme's panel color.
    let mut background = theme.accent;
    background.a = 0.35;
    let style = HighlightStyle {
        background_color: Some(background.into()),
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let highlights = if valid {
        vec![(highlight, style)]
    } else {
        Vec::new()
    };
    StyledText::new(text).with_highlights(highlights)
}
