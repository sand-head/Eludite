//! The Test Explorer's elements (brief 0035): Visual Studio's outcome glyphs (green check, red cross, blue skip, grey
//! not run, and a running mark), tree rows that carry one with a duration column, and toolbar buttons that can be
//! disabled. The window itself (the tree, the toolbar, the detail pane) is the shell's.

use gpui::{
    App, ClickEvent, Div, FontWeight, InteractiveElement, ParentElement, Rgba, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};

use crate::{TREE_ROW_HEIGHT, Theme, TreeRowStyle};

/// Indentation per level, as the tree rows of Workspace.
const INDENT: f32 = 14.;

/// A test's (or a node's) state as the Test Explorer draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestGlyph {
    Passed,
    Failed,
    Skipped,
    NotRun,
    Running,
}

impl TestGlyph {
    pub fn glyph(self) -> &'static str {
        match self {
            TestGlyph::Passed => "\u{2714}",
            TestGlyph::Failed => "\u{2716}",
            TestGlyph::Skipped => "\u{26A0}",
            TestGlyph::NotRun => "\u{25CB}",
            TestGlyph::Running => "\u{21BB}",
        }
    }

    /// Visual Studio's colors: green, red, blue, grey; running in the accent's blue.
    pub fn color(self) -> Rgba {
        match self {
            TestGlyph::Passed => rgb(0x1BA01B),
            TestGlyph::Failed => rgb(0xE51400),
            TestGlyph::Skipped => rgb(0x1E90FF),
            TestGlyph::NotRun => rgb(0x8A8A8A),
            TestGlyph::Running => rgb(0x007ACC),
        }
    }

    /// A node's glyph from its tests': failed beats running beats not run beats skipped beats passed.
    pub fn aggregate(glyphs: impl IntoIterator<Item = TestGlyph>) -> TestGlyph {
        let mut any = false;
        let (mut failed, mut running, mut not_run, mut skipped) = (false, false, false, false);
        for g in glyphs {
            any = true;
            match g {
                TestGlyph::Failed => failed = true,
                TestGlyph::Running => running = true,
                TestGlyph::NotRun => not_run = true,
                TestGlyph::Skipped => skipped = true,
                TestGlyph::Passed => {}
            }
        }
        if !any || (not_run && !failed && !running) {
            TestGlyph::NotRun
        } else if failed {
            TestGlyph::Failed
        } else if running {
            TestGlyph::Running
        } else if skipped {
            TestGlyph::Skipped
        } else {
            TestGlyph::Passed
        }
    }
}

/// One Test Explorer row: `[indent][triangle][glyph] label ... detail`, with element id `id`; `detail` (a duration, a
/// count) is right-aligned and muted. Clicking the triangle calls `on_toggle` only.
pub fn test_row(
    theme: &Theme,
    id: impl Into<SharedString>,
    glyph: TestGlyph,
    label: impl Into<SharedString>,
    detail: Option<String>,
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
        .w_full()
        .items_center()
        .h(px(TREE_ROW_HEIGHT))
        .pl(px(4. + INDENT * style.depth as f32))
        .pr_2()
        .gap_1()
        .text_size(theme.typography.ui)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(toggle)
        .child(
            div()
                .flex_none()
                .w(px(16.))
                .text_size(theme.typography.small)
                .text_color(glyph.color())
                .child(glyph.glyph()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(label.into()),
        )
        .children(detail.map(|d| {
            div()
                .flex_none()
                .pl_2()
                .text_size(theme.typography.small)
                .text_color(if style.selected {
                    theme.text_on_accent
                } else {
                    theme.text_muted
                })
                .child(d)
        }));
    let row = if style.bold {
        row.font_weight(FontWeight::BOLD)
    } else {
        row
    };
    if style.selected {
        row.bg(theme.accent).text_color(theme.text_on_accent)
    } else {
        row.hover(|s| s.bg(theme.menu_hover))
            .text_color(if style.muted {
                theme.text_muted
            } else {
                theme.text
            })
    }
}

/// A toolbar button (Run All, Run, Debug, Cancel): drawn muted and without hover while `enabled` is false. The caller
/// adds the click handler (and should not when disabled).
pub fn toolbar_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
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
        .h(px(20.))
        .px_2()
        .text_size(theme.typography.ui)
        .child(label.into());
    if enabled {
        el.cursor_pointer()
            .text_color(theme.text)
            .hover(|s| s.bg(theme.menu_hover))
    } else {
        el.text_color(theme.text_disabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodes_aggregate_their_tests() {
        use TestGlyph::*;
        assert_eq!(TestGlyph::aggregate([]), NotRun);
        assert_eq!(TestGlyph::aggregate([Passed, Passed]), Passed);
        assert_eq!(TestGlyph::aggregate([Passed, Skipped]), Skipped);
        assert_eq!(TestGlyph::aggregate([Passed, Failed, Running]), Failed);
        assert_eq!(TestGlyph::aggregate([Passed, Running]), Running);
        assert_eq!(TestGlyph::aggregate([Passed, NotRun]), NotRun);
        assert_ne!(Passed.color(), Failed.color());
        assert_eq!(Failed.glyph(), "\u{2716}");
    }
}
