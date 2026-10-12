//! Popups the owner supplies as GPUI views, anchored under a buffer position (brief 0077): the debugger's exception
//! helper, DataTips and the inline Breakpoint Settings panel. The view only places them (under the line of their
//! position, following scrolling and edits through an anchor) and drops them by key; what they show and do is the
//! owner's. Each key holds at most one popup.

use gpui::{
    AnyElement, AnyView, Context, IntoElement, ParentElement, Pixels, anchored, deferred, point, px,
};
use text::Anchor;

use crate::view::EditorView;

/// One owner popup.
pub(crate) struct AnchoredPopup {
    key: &'static str,
    anchor: Anchor,
    view: AnyView,
}

impl EditorView {
    /// Show `view` under the line of byte offset `at` as popup `key` (replacing that key's popup), or remove it with
    /// `None`.
    pub fn set_anchored_popup(
        &mut self,
        key: &'static str,
        at: Option<(usize, AnyView)>,
        cx: &mut Context<Self>,
    ) {
        let had = self.popups.anchored.iter().any(|p| p.key == key);
        self.popups.anchored.retain(|p| p.key != key);
        if let Some((offset, view)) = at {
            let b = self.editor.buffer();
            let anchor = b.anchor_before(offset.min(b.len()));
            self.popups
                .anchored
                .push(AnchoredPopup { key, anchor, view });
        } else if !had {
            return;
        }
        cx.notify();
    }

    /// The keys of the popups shown, in the order they were opened.
    pub fn anchored_popups(&self) -> Vec<&'static str> {
        self.popups.anchored.iter().map(|p| p.key).collect()
    }

    /// The row (0-based) popup `key` is anchored on.
    pub fn anchored_popup_row(&self, key: &str) -> Option<u32> {
        let p = self.popups.anchored.iter().find(|p| p.key == key)?;
        let b = self.editor.buffer();
        Some(b.offset_to_point(b.offset_for_anchor(&p.anchor)).row)
    }

    /// The popups as elements, each under its line when the line is in view.
    pub(crate) fn render_anchored(&self, line_height: Pixels) -> Vec<AnyElement> {
        let b = self.editor.buffer();
        self.popups
            .anchored
            .iter()
            .filter_map(|p| {
                let origin = self.popup_origin(b.offset_for_anchor(&p.anchor))?;
                Some(
                    deferred(
                        anchored()
                            .position(point(origin.x, origin.y + line_height + px(2.)))
                            .snap_to_window_with_margin(px(4.))
                            .child(p.view.clone()),
                    )
                    .with_priority(4)
                    .into_any_element(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, Render, TestAppContext, Window, div};

    use super::*;
    use crate::Buffer;

    struct Panel;

    impl Render for Panel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child("panel")
        }
    }

    #[gpui::test]
    fn popups_follow_their_line_and_go_by_key(cx: &mut TestAppContext) {
        let view = cx.new(|cx| EditorView::new(Buffer::new("a\nbb\nccc\n"), None, cx));
        let panel = cx.new(|_| Panel);
        view.update(cx, |v, cx| {
            v.set_anchored_popup("helper", Some((3, panel.clone().into())), cx);
            v.set_anchored_popup("tip", Some((0, panel.clone().into())), cx);
            assert_eq!(v.anchored_popups(), ["helper", "tip"]);
            assert_eq!(v.anchored_popup_row("helper"), Some(1));
            // Text inserted above moves it with its line.
            v.update_editor(cx, |e| {
                e.set_caret(0);
                e.insert("x\n");
            });
            assert_eq!(v.anchored_popup_row("helper"), Some(2));
            v.set_anchored_popup("helper", None, cx);
            assert_eq!(v.anchored_popups(), ["tip"]);
        });
    }
}
