//! Tree rows (Workspace and later tree views): indentation by depth, a disclosure triangle for nodes with
//! children, an optional glyph or icon and the label, at a fixed height so a virtualized list can lay them out. The
//! caller keeps the expanded set and adds ids and handlers.
//!
//! [`tree_row_with_icon`] (brief 0062) leads with an icon of the set ([`crate::icons`]) and can draw the characters a
//! search matched in bold and the guide color, as the completion list does.

use std::ops::Range;

use gpui::{
    AnyElement, App, ClickEvent, Div, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, Rgba, SharedString, Stateful, StatefulInteractiveElement, Styled, StyledText,
    Window, div, px,
};

use crate::Theme;
use crate::icons::{Icon, icon};

/// Height of one row; lists of rows can be virtualized with it.
pub const TREE_ROW_HEIGHT: f32 = 20.;

/// Indentation per level, as in Visual Studio's Workspace.
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
    /// Drawn bold (the startup project, as Visual Studio shows it).
    pub bold: bool,
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
    tree_row_with_badge(theme, id, glyph, None, label, style, on_toggle)
}

/// [`tree_row`] with a colored badge between the glyph and the label (brief 0040: Visual Studio's source control
/// glyph on a Workspace file: `(glyph, 0xRRGGBB)`).
pub fn tree_row_with_badge(
    theme: &Theme,
    id: impl Into<SharedString>,
    glyph: Option<&'static str>,
    badge: Option<(&'static str, u32)>,
    label: impl Into<SharedString>,
    style: TreeRowStyle,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let lead = glyph.map(|g| {
        div()
            .flex_none()
            .w(px(16.))
            .text_size(theme.typography.small)
            .text_color(theme.text_muted)
            .child(g)
            .into_any_element()
    });
    let label: SharedString = label.into();
    row(
        theme,
        id.into(),
        lead,
        badge,
        label.into_any_element(),
        style,
        on_toggle,
    )
}

/// A tree row led by `icon` (an icon of the set and its tint, [`Icon::tint`]), with the colored `badge` between the
/// icon and the label (brief 0040's source control glyph, brief 0048's warning), and the label's `matched` byte ranges
/// drawn bold in the guide color (brief 0062's search; bold only on the selected row). The icon slot has the debug
/// selector `<id>-icon`.
#[allow(clippy::too_many_arguments)]
pub fn tree_row_with_icon(
    theme: &Theme,
    id: impl Into<SharedString>,
    lead: Option<(Icon, Rgba)>,
    badge: Option<(&'static str, u32)>,
    label: impl Into<SharedString>,
    matched: &[Range<usize>],
    style: TreeRowStyle,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let icon_id = format!("{id}-icon");
    let lead = lead.map(|(i, color)| {
        div()
            .debug_selector(move || icon_id)
            .flex_none()
            .flex()
            .items_center()
            .w(px(16.))
            .h(px(16.))
            .child(icon(i, color))
            .into_any_element()
    });
    let label: SharedString = label.into();
    let label = if matched.is_empty() {
        label.into_any_element()
    } else {
        let highlight = HighlightStyle {
            color: (!style.selected).then(|| theme.guide.into()),
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let highlights: Vec<(Range<usize>, HighlightStyle)> = matched
            .iter()
            .filter(|r| {
                r.start < r.end
                    && r.end <= label.len()
                    && label.is_char_boundary(r.start)
                    && label.is_char_boundary(r.end)
            })
            .map(|r| (r.clone(), highlight))
            .collect();
        StyledText::new(label)
            .with_highlights(highlights)
            .into_any_element()
    };
    row(theme, id, lead, badge, label, style, on_toggle)
}

fn row(
    theme: &Theme,
    id: SharedString,
    lead: Option<AnyElement>,
    badge: Option<(&'static str, u32)>,
    label: AnyElement,
    style: TreeRowStyle,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let badge_id = format!("{id}-badge");
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
        .children(lead)
        .children(badge.map(|(b, color)| {
            div()
                .debug_selector(move || badge_id)
                .flex_none()
                .w(px(12.))
                .text_size(theme.typography.small)
                .text_color(gpui::rgb(color))
                .child(b)
        }))
        .child(label);
    let row = if style.bold {
        row.font_weight(FontWeight::BOLD)
    } else {
        row
    };
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
