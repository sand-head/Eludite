use gpui::{Pixels, Rgba, px, rgb};

/// Color tokens. Field names describe roles, not widgets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Window and document background.
    pub background: Rgba,
    /// Tool window, menu bar and tab strip background.
    pub panel: Rgba,
    /// Active tab, focus and the status bar.
    pub accent: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    /// Text drawn on `accent`.
    pub text_on_accent: Rgba,
    pub border: Rgba,
    /// Background of a tool window's title header.
    pub panel_header: Rgba,
    pub typography: Typography,
}

impl Theme {
    /// Defaults inspired by Visual Studio's Dark theme (PLAN.md 8).
    pub fn vs_dark() -> Self {
        Self {
            background: rgb(0x1E1E1E),
            panel: rgb(0x252526),
            accent: rgb(0x007ACC),
            text: rgb(0xD4D4D4),
            text_muted: rgb(0x9D9D9D),
            text_on_accent: rgb(0xFFFFFF),
            border: rgb(0x3F3F46),
            panel_header: rgb(0x2D2D30),
            typography: Typography::default(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::vs_dark()
    }
}

/// Text sizes and fixed chrome metrics, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Typography {
    /// Menus, tabs, tool window bodies.
    pub ui: Pixels,
    /// Status bar and secondary text.
    pub small: Pixels,
    /// Document body text until the editor has its own settings.
    pub body: Pixels,
    pub menu_bar_height: Pixels,
    pub tab_height: Pixels,
    pub panel_header_height: Pixels,
    pub status_bar_height: Pixels,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            ui: px(12.),
            small: px(11.),
            body: px(14.),
            menu_bar_height: px(24.),
            tab_height: px(24.),
            panel_header_height: px(22.),
            status_bar_height: px(22.),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vs_dark_tokens() {
        let t = Theme::vs_dark();
        assert_eq!(t.background, rgb(0x1E1E1E));
        assert_eq!(t.panel, rgb(0x252526));
        assert_eq!(t.accent, rgb(0x007ACC));
        assert_eq!(t.text, rgb(0xD4D4D4));
        assert_eq!(t.border, rgb(0x3F3F46));
        assert_eq!(Theme::default(), t);
    }

    #[test]
    fn typography_is_ordered() {
        let ty = Typography::default();
        assert!(ty.small < ty.ui && ty.ui < ty.body);
    }
}
