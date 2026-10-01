//! Dialog and menu pieces (brief 0015): the frame of a modal dialog (Rename), push buttons, and the rows of the light
//! bulb menu (Quick Actions and Refactorings). Stateless: the caller owns the state, the focus and the handlers.

use gpui::{
    Div, FontWeight, InteractiveElement, ParentElement, Rgba, SharedString, Stateful, Styled, div,
    px, rgb,
};

use crate::Theme;

/// Height of a light bulb menu row.
pub const MENU_ROW_HEIGHT: f32 = 22.;

/// The frame of a dialog: a bordered, shadowed panel with a title bar.
pub fn dialog_panel(theme: &Theme, title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .bg(theme.panel)
        .border_1()
        .border_color(theme.accent)
        .shadow_lg()
        .text_color(theme.text)
        .text_size(theme.typography.ui)
        .child(
            div()
                .flex()
                .items_center()
                .h(px(28.))
                .px_3()
                .bg(theme.chrome)
                .text_color(theme.chrome_text)
                .child(title.into()),
        )
}

/// A push button. The default button (Enter) has the accent border; a disabled one is muted and takes no clicks
/// (the caller skips the handler).
pub fn push_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    default: bool,
    enabled: bool,
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
        .justify_center()
        .min_w(px(75.))
        .h(px(23.))
        .px_3()
        .border_1()
        .bg(theme.chrome)
        .text_size(theme.typography.ui);
    if !enabled {
        return el
            .border_color(theme.border)
            .text_color(theme.text_disabled)
            .child(label.into());
    }
    el.border_color(if default { theme.accent } else { theme.border })
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(theme.menu_hover))
        .child(label.into())
}

/// What a light bulb shows: Visual Studio's yellow bulb when there are fixes, its blue-gray screwdriver bulb when
/// there are only refactorings. The editor draws the bulb in the margin in this color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightbulbKind {
    Fix,
    Refactoring,
}

impl LightbulbKind {
    /// The bulb's color.
    pub fn color(self) -> Rgba {
        match self {
            LightbulbKind::Fix => rgb(0xFFCC00),
            LightbulbKind::Refactoring => rgb(0x9CDCFE),
        }
    }
}

/// One row of the light bulb menu: `indent` levels in (nested actions), a submenu arrow when `has_children`.
pub fn menu_row(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    indent: usize,
    selected: bool,
    has_children: bool,
    disabled: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    let row = div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(MENU_ROW_HEIGHT))
        .pl(px(8. + 16. * indent as f32))
        .pr_2()
        .whitespace_nowrap()
        .cursor_pointer()
        .child(div().flex_1().child(label.into()))
        .children(has_children.then(|| div().text_color(theme.text_muted).child("\u{25B8}")));
    let row = if disabled {
        row.text_color(theme.text_disabled)
    } else {
        row
    };
    if selected {
        row.bg(theme.accent).text_color(theme.text_on_accent)
    } else {
        row.hover(|s| s.bg(theme.menu_hover))
    }
}

/// A group heading in the light bulb menu or a dialog ("Preview changes").
pub fn section_heading(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .px_2()
        .pt_1()
        .text_size(theme.typography.small)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_muted)
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulbs_differ_for_fixes_and_refactorings() {
        assert_ne!(
            LightbulbKind::Fix.color(),
            LightbulbKind::Refactoring.color()
        );
    }
}
