//! The Welcome document: Eludite's mark, which plays its open once (it enters spinning and settles on the
//! isometric pose), the wordmark, the line under it, and how to open a workspace or a solution.

use eludite_ui::{Crystal, Theme};
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window,
    div, px,
};

/// The mark's edge on the Welcome page.
const MARK_EDGE: f32 = 130.;

/// The line under the wordmark.
pub const TAGLINE: &str = "An IDE for .NET, the web stack and Rust, shared by you and your agents";

/// How to start, under the tagline.
pub const HINT: &str = "Open a workspace with File > Open > Workspace... (Ctrl+Shift+Alt+O), or a .NET solution file with Ctrl+Shift+O.";

pub struct WelcomePage {
    theme: Theme,
    mark: Crystal,
}

impl WelcomePage {
    /// The open plays the first time the page is drawn.
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            mark: Crystal::opening(px(MARK_EDGE)),
        }
    }
}

impl Render for WelcomePage {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .debug_selector(|| "welcome".into())
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(28.))
            .p_4()
            .bg(t.background)
            .child(self.mark.render(false, window))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .debug_selector(|| "welcome-wordmark".into())
                            .text_size(px(64.))
                            .line_height(px(64.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child("Eludite"),
                    )
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_color(t.text_muted)
                            .child(TAGLINE),
                    ),
            )
            .child(
                div()
                    .max_w(px(560.))
                    .text_size(t.typography.ui)
                    .text_color(t.text_muted)
                    .text_center()
                    .child(HINT),
            )
    }
}
