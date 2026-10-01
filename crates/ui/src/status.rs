//! The status bar: named slots, left-aligned state and right-aligned details,
//! as in Visual Studio (PLAN.md 8: build/debug state, line/column, encoding,
//! line endings, branch, host memory).

use gpui::{Div, IntoElement, ParentElement, SharedString, Styled, div};

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
    pub id: &'static str,
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
        let slot = |id, align| StatusSlot {
            id,
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
        match self.slots.iter_mut().find(|s| s.id == id) {
            Some(s) => {
                s.text = text.into();
                true
            }
            None => false,
        }
    }

    /// Add a slot (later briefs: debugger state, test results).
    pub fn add_slot(&mut self, id: &'static str, align: SlotAlign) {
        if self.get(id).is_none() {
            self.slots.push(StatusSlot {
                id,
                align,
                text: SharedString::default(),
            });
        }
    }

    pub fn get(&self, id: &str) -> Option<&str> {
        self.slots
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.text.as_ref())
    }

    /// Non-empty slots on one side, in order.
    pub fn visible(&self, align: SlotAlign) -> impl Iterator<Item = &StatusSlot> {
        self.slots
            .iter()
            .filter(move |s| s.align == align && !s.text.is_empty())
    }

    pub fn render(&self, theme: &Theme) -> Div {
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
                    .children(self.visible(SlotAlign::Left).map(slot)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .children(self.visible(SlotAlign::Right).map(slot)),
            )
    }
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
        assert!(bar.set(slots::VERSION, "Niello 0.1.0"));
        assert!(!bar.set("nope", "x"));
        let right: Vec<_> = bar.visible(SlotAlign::Right).map(|s| s.id).collect();
        assert_eq!(right, [slots::BRANCH, slots::VERSION]);
        bar.add_slot("debug_state", SlotAlign::Left);
        bar.add_slot("debug_state", SlotAlign::Left);
        assert!(bar.set("debug_state", "Running"));
        let left: Vec<_> = bar
            .visible(SlotAlign::Left)
            .map(|s| s.text.to_string())
            .collect();
        assert_eq!(left, ["Ready", "Running"]);
    }
}
