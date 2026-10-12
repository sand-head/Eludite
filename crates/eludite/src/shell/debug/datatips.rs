//! DataTips (brief 0077): the value of the expression under the mouse in break mode as Visual Studio's expandable tree,
//! in an editor popup anchored under the expression. The tree is a variables window of kind
//! [`super::windows::VarsKind::DataTip`] (the Locals tree model: rows expand lazily through DAP `variables`, a double
//! click edits a value through `eludite.debug.set_variable`, the context menu copies and adds watches); this panel
//! frames it and turns transparent while Ctrl is held, as Visual Studio's does.

use eludite_ui::Theme;
use gpui::{
    Context, Entity, InteractiveElement, IntoElement, ModifiersChangedEvent, ParentElement, Render,
    Styled, Window, div, px,
};

use super::windows::{ROW_HEIGHT, VarsWindow};

/// The editor popup key.
pub const POPUP: &str = "datatip";
/// Rows shown before the tree scrolls.
const ROWS: usize = 12;

pub struct DataTipPanel {
    theme: Theme,
    pub tree: Entity<VarsWindow>,
    /// Ctrl is held: the tip is transparent.
    transparent: bool,
}

impl DataTipPanel {
    pub fn new(theme: Theme, tree: Entity<VarsWindow>) -> Self {
        Self {
            theme,
            tree,
            transparent: false,
        }
    }

    pub fn set_transparent(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.transparent != on {
            self.transparent = on;
            cx.notify();
        }
    }
}

impl Render for DataTipPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let rows = self.tree.read(cx).rows().len().clamp(1, ROWS);
        eludite_ui::popup::popup_panel(&t)
            .id(POPUP)
            .debug_selector(|| POPUP.to_owned())
            .occlude()
            .w(px(520.))
            .h(px(ROW_HEIGHT * (rows as f32 + 1.) + 4.))
            .opacity(if self.transparent { 0.2 } else { 1. })
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                this.set_transparent(e.modifiers.control, cx)
            }))
            .child(div().size_full().child(self.tree.clone()))
    }
}
