use gpui::{Pixels, Rgba, px, rgb};

/// Color tokens. Field names describe roles, not widgets. Values approximate
/// Visual Studio 2022's Dark, Light and Blue themes (PLAN.md 8); they are
/// Eludite's own token sets, not copied theme files.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// Document area background.
    pub background: Rgba,
    /// Environment background: behind docks, tab strips, auto-hide strips.
    pub chrome: Rgba,
    /// Text drawn on `chrome` (inactive document tabs, strip labels).
    pub chrome_text: Rgba,
    /// Tool window body background.
    pub panel: Rgba,
    /// Inactive tool window title bar.
    pub panel_header: Rgba,
    pub panel_header_text: Rgba,
    /// Active tool window title bar.
    pub panel_header_active: Rgba,
    pub panel_header_active_text: Rgba,
    /// Focus, selection and the active document tab.
    pub accent: Rgba,
    /// Body text on `panel` and `background`.
    pub text: Rgba,
    pub text_muted: Rgba,
    /// Text drawn on `accent`.
    pub text_on_accent: Rgba,
    /// Disabled menu items.
    pub text_disabled: Rgba,
    pub border: Rgba,
    pub menu_background: Rgba,
    pub menu_text: Rgba,
    /// Hovered menu title or item.
    pub menu_hover: Rgba,
    /// Drop-down menus and fly-outs.
    pub popup_background: Rgba,
    pub popup_border: Rgba,
    /// Docking guides and drop highlights.
    pub guide: Rgba,
    pub status_bar: Rgba,
    pub status_bar_text: Rgba,
    /// CodeLens indicators above members in the editor (brief 0052): a quiet grey, as Visual Studio draws them.
    pub code_lens: Rgba,
    /// Something that needs attention but is not an error (brief 0059): the usage strip's context bar from 80
    /// percent, a permission or review a tool call waits for, the logged-out state. Readable as text on `panel`.
    pub warning: Rgba,
    /// Something that went well (brief 0059): a completed tool call, the agent's Ready state. Readable as text on
    /// `panel`.
    pub success: Rgba,
    /// A block set off from `panel` (brief 0059): the person's prompts in the Agents transcript.
    pub panel_raised: Rgba,
    pub typography: Typography,
}

impl Theme {
    /// Visual Studio Dark. The default.
    pub fn vs_dark() -> Self {
        Self {
            name: "dark",
            background: rgb(0x1E1E1E),
            chrome: rgb(0x2D2D30),
            chrome_text: rgb(0xD0D0D0),
            panel: rgb(0x252526),
            panel_header: rgb(0x2D2D30),
            panel_header_text: rgb(0xD0D0D0),
            panel_header_active: rgb(0x007ACC),
            panel_header_active_text: rgb(0xFFFFFF),
            accent: rgb(0x007ACC),
            text: rgb(0xD4D4D4),
            text_muted: rgb(0x9D9D9D),
            text_on_accent: rgb(0xFFFFFF),
            text_disabled: rgb(0x656565),
            border: rgb(0x3F3F46),
            menu_background: rgb(0x2D2D30),
            menu_text: rgb(0xF1F1F1),
            menu_hover: rgb(0x3E3E40),
            popup_background: rgb(0x1B1B1C),
            popup_border: rgb(0x333337),
            guide: rgb(0x1C97EA),
            status_bar: rgb(0x007ACC),
            status_bar_text: rgb(0xFFFFFF),
            code_lens: rgb(0x999999),
            warning: rgb(0xCCA700),
            success: rgb(0x89D185),
            panel_raised: rgb(0x2D2D30),
            typography: Typography::default(),
        }
    }

    /// Visual Studio Light.
    pub fn vs_light() -> Self {
        Self {
            name: "light",
            background: rgb(0xFFFFFF),
            chrome: rgb(0xEEEEF2),
            chrome_text: rgb(0x1E1E1E),
            panel: rgb(0xF5F5F5),
            panel_header: rgb(0xEEEEF2),
            panel_header_text: rgb(0x444444),
            panel_header_active: rgb(0x007ACC),
            panel_header_active_text: rgb(0xFFFFFF),
            accent: rgb(0x007ACC),
            text: rgb(0x1E1E1E),
            text_muted: rgb(0x717171),
            text_on_accent: rgb(0xFFFFFF),
            text_disabled: rgb(0xA2A4A5),
            border: rgb(0xCCCEDB),
            menu_background: rgb(0xEEEEF2),
            menu_text: rgb(0x1E1E1E),
            menu_hover: rgb(0xC9DEF5),
            popup_background: rgb(0xF6F6F6),
            popup_border: rgb(0xCCCEDB),
            guide: rgb(0x007ACC),
            status_bar: rgb(0x007ACC),
            status_bar_text: rgb(0xFFFFFF),
            code_lens: rgb(0x6D6D6D),
            warning: rgb(0x8F6200),
            success: rgb(0x2B7A2F),
            panel_raised: rgb(0xE7E8EC),
            typography: Typography::default(),
        }
    }

    /// Visual Studio Blue.
    pub fn vs_blue() -> Self {
        Self {
            name: "blue",
            background: rgb(0xFFFFFF),
            chrome: rgb(0x293955),
            chrome_text: rgb(0xFFFFFF),
            panel: rgb(0xFFFFFF),
            panel_header: rgb(0x4D6082),
            panel_header_text: rgb(0xFFFFFF),
            panel_header_active: rgb(0xFFF29D),
            panel_header_active_text: rgb(0x1E1E1E),
            accent: rgb(0xFFF29D),
            text: rgb(0x1E1E1E),
            text_muted: rgb(0x595959),
            text_on_accent: rgb(0x1E1E1E),
            text_disabled: rgb(0x9A9A9A),
            border: rgb(0x8E9BBC),
            menu_background: rgb(0xD6DBE9),
            menu_text: rgb(0x1E1E1E),
            menu_hover: rgb(0xFDF4BF),
            popup_background: rgb(0xEAF0FF),
            popup_border: rgb(0x9BA7B7),
            guide: rgb(0x3399FF),
            status_bar: rgb(0x007ACC),
            status_bar_text: rgb(0xFFFFFF),
            code_lens: rgb(0x6D6D6D),
            warning: rgb(0x8A5A00),
            success: rgb(0x26722A),
            panel_raised: rgb(0xE6EBF5),
            typography: Typography::default(),
        }
    }

    /// `dark`, `light` or `blue`.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::vs_dark()),
            "light" => Some(Self::vs_light()),
            "blue" => Some(Self::vs_blue()),
            _ => None,
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::vs_dark(), Self::vs_light(), Self::vs_blue()]
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

    /// WCAG relative luminance contrast between two colors.
    fn contrast(a: Rgba, b: Rgba) -> f32 {
        fn lum(c: Rgba) -> f32 {
            let f = |v: f32| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
        }
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn vs_dark_is_default() {
        let t = Theme::vs_dark();
        assert_eq!(t.background, rgb(0x1E1E1E));
        assert_eq!(t.panel, rgb(0x252526));
        assert_eq!(t.accent, rgb(0x007ACC));
        assert_eq!(Theme::default(), t);
    }

    #[test]
    fn three_distinct_token_sets() {
        let [d, l, b] = Theme::all();
        assert_ne!(d, l);
        assert_ne!(l, b);
        assert_ne!(d, b);
        for name in ["dark", "light", "blue"] {
            assert_eq!(Theme::by_name(name).unwrap().name, name);
        }
        assert!(Theme::by_name("solarized").is_none());
    }

    #[test]
    fn text_is_readable_in_every_theme() {
        for t in Theme::all() {
            let pairs = [
                ("text/panel", t.text, t.panel),
                ("text/background", t.text, t.background),
                ("chrome_text/chrome", t.chrome_text, t.chrome),
                ("menu_text/menu", t.menu_text, t.menu_background),
                ("menu_text/popup", t.menu_text, t.popup_background),
                ("header", t.panel_header_text, t.panel_header),
                (
                    "active header",
                    t.panel_header_active_text,
                    t.panel_header_active,
                ),
                ("on accent", t.text_on_accent, t.accent),
                ("status", t.status_bar_text, t.status_bar),
                ("code lens/background", t.code_lens, t.background),
                // Brief 0059: the Agents window's status colors and its prompt blocks.
                ("warning/panel", t.warning, t.panel),
                ("success/panel", t.success, t.panel),
                ("text/panel_raised", t.text, t.panel_raised),
            ];
            for (what, fg, bg) in pairs {
                assert!(
                    contrast(fg, bg) >= 4.0,
                    "{} {what}: {}",
                    t.name,
                    contrast(fg, bg)
                );
            }
            // Disabled text must look different from enabled text.
            assert!(contrast(t.text_disabled, t.menu_text) > 1.5, "{}", t.name);
            // A raised block is visibly set off from the panel it sits on, and the context bar's warning from its
            // normal fill.
            assert!(contrast(t.panel_raised, t.panel) > 1.05, "{}", t.name);
            assert_ne!(t.warning, t.accent, "{}", t.name);
        }
    }

    #[test]
    fn typography_is_ordered() {
        let ty = Typography::default();
        assert!(ty.small < ty.ui && ty.ui < ty.body);
    }
}
