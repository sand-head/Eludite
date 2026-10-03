//! The status bar: named slots, left-aligned state and right-aligned details,
//! as in Visual Studio (PLAN.md 8: build/debug state, line/column, encoding,
//! line endings, branch, host memory).

use gpui::{
    AnyElement, Div, InteractiveElement, IntoElement, ParentElement, SharedString, Stateful,
    Styled, div,
};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotAlign {
    Left,
    Right,
}

/// Well-known slot ids.
pub mod slots {
    /// "Ready", build and debug state, command results.
    pub const STATE: &str = "state";
    pub const LINE_COLUMN: &str = "line_column";
    pub const ENCODING: &str = "encoding";
    pub const LINE_ENDINGS: &str = "line_endings";
    pub const BRANCH: &str = "branch";
    pub const HOST_MEMORY: &str = "host_memory";
    pub const VERSION: &str = "version";
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusSlot {
    pub id: SharedString,
    pub align: SlotAlign,
    pub text: SharedString,
}

/// Status bar contents. Empty slots are not drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusBar {
    slots: Vec<StatusSlot>,
}

impl StatusBar {
    /// Visual Studio's slots, in display order, all empty but "Ready".
    pub fn vs_default() -> Self {
        let slot = |id: &'static str, align| StatusSlot {
            id: id.into(),
            align,
            text: SharedString::default(),
        };
        let mut bar = Self {
            slots: vec![
                slot(slots::STATE, SlotAlign::Left),
                slot(slots::LINE_COLUMN, SlotAlign::Right),
                slot(slots::ENCODING, SlotAlign::Right),
                slot(slots::LINE_ENDINGS, SlotAlign::Right),
                slot(slots::BRANCH, SlotAlign::Right),
                slot(slots::HOST_MEMORY, SlotAlign::Right),
                slot(slots::VERSION, SlotAlign::Right),
            ],
        };
        bar.set(slots::STATE, "Ready");
        bar
    }

    /// Set a slot's text. Returns false for an unknown slot.
    pub fn set(&mut self, id: &str, text: impl Into<SharedString>) -> bool {
        match self.slots.iter_mut().find(|s| s.id.as_ref() == id) {
            Some(s) => {
                s.text = text.into();
                true
            }
            None => false,
        }
    }

    /// Add a slot (later briefs: debugger state, test results; one per language server, brief 0019).
    pub fn add_slot(&mut self, id: impl Into<SharedString>, align: SlotAlign) {
        let id = id.into();
        if !self.has_slot(&id) {
            self.slots.push(StatusSlot {
                id,
                align,
                text: SharedString::default(),
            });
        }
    }

    /// Whether slot `id` exists (empty or not).
    pub fn has_slot(&self, id: &str) -> bool {
        self.slots.iter().any(|s| s.id.as_ref() == id)
    }

    pub fn get(&self, id: &str) -> Option<&str> {
        self.slots
            .iter()
            .find(|s| s.id.as_ref() == id)
            .map(|s| s.text.as_ref())
    }

    /// Non-empty slots on one side, in order.
    pub fn visible(&self, align: SlotAlign) -> impl Iterator<Item = &StatusSlot> {
        self.slots
            .iter()
            .filter(move |s| s.align == align && !s.text.is_empty())
    }

    pub fn render(&self, theme: &Theme) -> Div {
        self.render_with(theme, Vec::new())
    }

    /// [`StatusBar::render`] with controls after the left slots (the debugger's Allow Agents to Drive toggle).
    pub fn render_with(&self, theme: &Theme, left_controls: Vec<AnyElement>) -> Div {
        let ty = theme.typography;
        let slot = |s: &StatusSlot| div().px_2().child(s.text.clone()).into_any_element();
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_between()
            .h(ty.status_bar_height)
            .px_1()
            .bg(theme.status_bar)
            .text_color(theme.status_bar_text)
            .text_size(ty.small)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .children(self.visible(SlotAlign::Left).map(slot))
                    .children(left_controls),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .children(self.visible(SlotAlign::Right).map(slot)),
            )
    }
}

/// A two-state control in the status bar (brief 0027: Allow Agents to Drive while debugging): its label with a box
/// that is checked while `on`. The caller adds the click handler.
pub fn status_toggle(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    on: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let selector = id.clone();
    div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .cursor_pointer()
        .hover(|s| s.bg(theme.menu_hover))
        .child(if on { "\u{2611}" } else { "\u{2610}" })
        .child(label.into())
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::vs_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_set_and_show() {
        let mut bar = StatusBar::vs_default();
        assert_eq!(bar.get(slots::STATE), Some("Ready"));
        assert_eq!(bar.visible(SlotAlign::Right).count(), 0);
        assert!(bar.set(slots::BRANCH, "main"));
        assert!(bar.set(slots::VERSION, "Eludite 0.1.0"));
        assert!(!bar.set("nope", "x"));
        let right: Vec<_> = bar
            .visible(SlotAlign::Right)
            .map(|s| s.id.to_string())
            .collect();
        assert_eq!(right, [slots::BRANCH, slots::VERSION]);
        bar.add_slot("debug_state", SlotAlign::Left);
        bar.add_slot("debug_state", SlotAlign::Left);
        // Slot ids made at run time (one per language server, brief 0019).
        bar.add_slot(
            format!("language_server:{}", "rust-analyzer"),
            SlotAlign::Right,
        );
        assert!(bar.has_slot("language_server:rust-analyzer"));
        assert!(!bar.has_slot("language_server:other"));
        assert!(bar.set("debug_state", "Running"));
        let left: Vec<_> = bar
            .visible(SlotAlign::Left)
            .map(|s| s.text.to_string())
            .collect();
        assert_eq!(left, ["Ready", "Running"]);
    }
}
