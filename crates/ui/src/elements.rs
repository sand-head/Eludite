use gpui::{Div, FontWeight, IntoElement, ParentElement, SharedString, Styled, div};

use crate::Theme;

/// A horizontal menu bar with one entry per label (File, Edit, ...).
pub fn menu_bar<L>(theme: &Theme, labels: impl IntoIterator<Item = L>) -> Div
where
    L: Into<SharedString>,
{
    let ty = theme.typography;
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h(ty.menu_bar_height)
        .px_1()
        .bg(theme.panel)
        .border_b_1()
        .border_color(theme.border)
        .text_size(ty.ui)
        .text_color(theme.text)
        .children(
            labels
                .into_iter()
                .map(|label| div().px_2().child(label.into())),
        )
}

/// A row of tabs; `active` is highlighted with the accent color.
pub fn tab_strip<L>(theme: &Theme, tabs: impl IntoIterator<Item = L>, active: Option<usize>) -> Div
where
    L: Into<SharedString>,
{
    let ty = theme.typography;
    div()
        .flex()
        .flex_row()
        .flex_none()
        .h(ty.tab_height)
        .bg(theme.panel)
        .border_b_1()
        .border_color(theme.border)
        .text_size(ty.ui)
        .children(tabs.into_iter().enumerate().map(|(ix, title)| {
            let tab = div()
                .flex()
                .items_center()
                .h_full()
                .px_3()
                .border_r_1()
                .border_color(theme.border)
                .child(title.into());
            if Some(ix) == active {
                tab.bg(theme.accent).text_color(theme.text_on_accent)
            } else {
                tab.text_color(theme.text_muted)
            }
        }))
}

/// A tool window: a title header above a body that fills the remaining space.
pub fn panel(theme: &Theme, title: impl Into<SharedString>, body: impl IntoElement) -> Div {
    let ty = theme.typography;
    div()
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(theme.panel)
        .border_1()
        .border_color(theme.border)
        .text_color(theme.text)
        .text_size(ty.ui)
        .child(
            div()
                .flex()
                .items_center()
                .flex_none()
                .h(ty.panel_header_height)
                .px_2()
                .bg(theme.panel_header)
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .child(div().flex_1().p_2().overflow_hidden().child(body))
}

/// The status bar: left-aligned state text and right-aligned details.
pub fn status_bar(
    theme: &Theme,
    left: impl Into<SharedString>,
    right: impl Into<SharedString>,
) -> Div {
    let ty = theme.typography;
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .h(ty.status_bar_height)
        .px_2()
        .bg(theme.accent)
        .text_color(theme.text_on_accent)
        .text_size(ty.small)
        .child(div().child(left.into()))
        .child(div().child(right.into()))
}
