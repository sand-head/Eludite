//! Small stateless elements shared by the docking view and the shell.

use gpui::{Div, InteractiveElement, ParentElement, SharedString, Stateful, Styled, div, px};

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
