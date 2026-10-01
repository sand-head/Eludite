use gpui::{AnyElement, Div, IntoElement, ParentElement, Styled, div, px};
use niello_ui::{Theme, panel, tab_strip};

use crate::{DockArea, DockLayout, Presentation, ToolWindow};

/// Draw `layout`: left dock | (document area over bottom dock) | right dock.
///
/// `tool_body` supplies each tool window's content and `document_body` the
/// active document's, so the docking crate stays ignorant of what they show.
pub fn render_layout(
    layout: &DockLayout,
    theme: &Theme,
    tool_body: &dyn Fn(&ToolWindow) -> AnyElement,
    document_body: AnyElement,
) -> Div {
    let documents = div()
        .flex()
        .flex_col()
        .flex_1()
        .bg(theme.background)
        .child(tab_strip(
            theme,
            layout.documents.tabs.iter().map(|t| t.title.clone()),
            layout.documents.active,
        ))
        .child(
            div()
                .flex_1()
                .p_4()
                .text_color(theme.text)
                .text_size(theme.typography.body)
                .child(document_body),
        );

    let mut center = div().flex().flex_col().flex_1().child(documents);
    if !layout.bottom.is_empty() {
        center = center.child(
            render_dock(&layout.bottom, theme, tool_body)
                .h(px(layout.bottom.size))
                .border_t_1()
                .border_color(theme.border),
        );
    }

    let mut row = div().flex().flex_row().flex_1().overflow_hidden();
    if !layout.left.is_empty() {
        row = row.child(render_dock(&layout.left, theme, tool_body).w(px(layout.left.size)));
    }
    row = row.child(center);
    if !layout.right.is_empty() {
        row = row.child(render_dock(&layout.right, theme, tool_body).w(px(layout.right.size)));
    }
    row
}

fn render_dock(
    dock: &DockArea,
    theme: &Theme,
    tool_body: &dyn Fn(&ToolWindow) -> AnyElement,
) -> Div {
    let base = div().flex().flex_col().flex_none().bg(theme.panel);
    match dock.presentation {
        Presentation::Stacked => base.children(
            dock.windows
                .iter()
                .map(|w| panel(theme, w.title.clone(), tool_body(w)).flex_1()),
        ),
        Presentation::Tabbed => {
            let body = dock.active_window().map(|w| {
                div()
                    .flex_1()
                    .p_2()
                    .text_color(theme.text)
                    .text_size(theme.typography.ui)
                    .child(tool_body(w))
                    .into_any_element()
            });
            base.child(tab_strip(
                theme,
                dock.windows.iter().map(|w| w.title.clone()),
                Some(dock.active),
            ))
            .children(body)
        }
    }
}
