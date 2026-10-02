//! Tree rows (Solution Explorer and later tree views): indentation by depth, a disclosure triangle for nodes with
//! children, an optional glyph and the label, at a fixed height so a virtualized list can lay them out. The caller
//! keeps the expanded set and adds ids and handlers.

use gpui::{
    App, ClickEvent, Div, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::Theme;

/// Height of one row; lists of rows can be virtualized with it.
pub const TREE_ROW_HEIGHT: f32 = 20.;

/// Indentation per level, as in Visual Studio's Solution Explorer.
const INDENT: f32 = 14.;

/// How a row is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TreeRowStyle {
    pub depth: usize,
    /// `None` for a leaf, else whether the node is expanded.
    pub disclosure: Option<bool>,
    pub selected: bool,
    /// Drawn muted (an unavailable project).
    pub muted: bool,
}

/// One tree row: `[indent][triangle][glyph] label`, with element id `id`. Clicking the triangle calls `on_toggle`
/// (and does not reach the row's own click handler); the caller adds the row's handlers.
pub fn tree_row(
    theme: &Theme,
    id: impl Into<SharedString>,
    glyph: Option<&'static str>,
    label: impl Into<SharedString>,
    style: TreeRowStyle,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let triangle = match style.disclosure {
        Some(true) => "\u{25E2}",
        Some(false) => "\u{25B7}",
        None => "",
    };
    let toggle_id = SharedString::from(format!("{id}-toggle"));
    let toggle = div()
        .id(toggle_id.clone())
        .debug_selector(move || toggle_id.to_string())
        .flex_none()
        .w(px(12.))
        .text_size(theme.typography.small)
        .text_color(theme.text_muted)
        .child(triangle);
    let toggle = if style.disclosure.is_some() {
        toggle.cursor_pointer().on_click(move |e, window, cx| {
            cx.stop_propagation();
            on_toggle(e, window, cx);
        })
    } else {
        toggle
    };
    let row = div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .h(px(TREE_ROW_HEIGHT))
        .pl(px(4. + INDENT * style.depth as f32))
        .pr_2()
        .gap_1()
        .text_size(theme.typography.ui)
        .whitespace_nowrap()
        .child(toggle)
        .children(glyph.map(|g| {
            div()
                .flex_none()
                .w(px(16.))
                .text_size(theme.typography.small)
                .text_color(theme.text_muted)
                .child(g)
        }))
        .child(label.into());
    if style.selected {
        row.bg(theme.accent).text_color(theme.text_on_accent)
    } else {
        let row = row.hover(|s| s.bg(theme.menu_hover));
        if style.muted {
            row.text_color(theme.text_muted)
        } else {
            row.text_color(theme.text)
        }
    }
}
