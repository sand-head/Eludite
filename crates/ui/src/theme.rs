use gpui::{Pixels, Rgba, px, rgb};

/// Color tokens. Field names describe roles, not widgets. Dark and Light are Eludite's own palette, taken from the
/// mark: green-tinted neutrals from the crystal's shell, teal for focus, selection and the status bar, amber for the
/// active-tab marker and the current item. Classic approximates Visual Studio 2022's Blue (PLAN.md 8).
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
    /// Focus, selection and the status bar.
    pub accent: Rgba,
    /// The active document or tool window tab, and its text.
    pub tab_active: Rgba,
    pub tab_active_text: Rgba,
    /// The rule under the document tab strip.
    pub tab_strip_rule: Rgba,
    /// The current item set apart from the selection: the active tab's marker.
    pub highlight: Rgba,
    /// The active tool window tab: its fill, its text and the rule along its bottom.
    pub tool_tab_active: Rgba,
    pub tool_tab_active_text: Rgba,
    pub tool_tab_marker: Rgba,
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
    /// The icon set's tints (brief 0062, [`crate::icons`]): each reads at 3:1 on `panel`. Folders and solution
    /// folders.
    pub icon_folder: Rgba,
    /// C# files, Razor and `.cshtml` pages.
    pub icon_csharp_file: Rgba,
    /// C# projects and project references.
    pub icon_csharp_project: Rgba,
    /// The Cargo workspace, packages, targets and `.rs` files.
    pub icon_rust: Rgba,
    /// JSON, JavaScript and YAML files.
    pub icon_json: Rgba,
    /// Markdown files.
    pub icon_markdown: Rgba,
    /// MSBuild, XML and HTML files.
    pub icon_xml: Rgba,
    /// Web projects, TypeScript, CSS and Web Forms pages.
    pub icon_web: Rgba,
    /// NuGet packages and the Packages folder.
    pub icon_package: Rgba,
    /// The Dependencies node and its frameworks.
    pub icon_dependencies: Rgba,
    /// The solution and solution files.
    pub icon_solution: Rgba,
    /// Every other file, the Targets folder and the search box's glyphs.
    pub icon_muted: Rgba,
    /// The editor's caret, its selection, line numbers (and the current line's), the current line's outline and find
    /// matches.
    pub editor_caret: Rgba,
    pub editor_selection: Rgba,
    pub editor_line_number: Rgba,
    pub editor_line_number_active: Rgba,
    pub editor_current_line: Rgba,
    pub editor_find_match: Rgba,
    pub typography: Typography,
}

impl Theme {
    /// Eludite Dark. The default.
    pub fn dark() -> Self {
        Self {
            name: "dark",
            background: rgb(0x0F1514),
            chrome: rgb(0x161E1C),
            chrome_text: rgb(0x8FA39B),
            panel: rgb(0x131A18),
            panel_header: rgb(0x161E1C),
            panel_header_text: rgb(0x8FA39B),
            panel_header_active: rgb(0x0E7F64),
            panel_header_active_text: rgb(0xFFFFFF),
            accent: rgb(0x0E7F64),
            tab_active: rgb(0x0F1514),
            tab_active_text: rgb(0xD6DFDB),
            tab_strip_rule: rgb(0x25312E),
            highlight: rgb(0xE0A52B),
            tool_tab_active: rgb(0x131A18),
            tool_tab_active_text: rgb(0xD6DFDB),
            tool_tab_marker: rgb(0x10A57E),
            text: rgb(0xD6DFDB),
            text_muted: rgb(0x8FA39B),
            text_on_accent: rgb(0xFFFFFF),
            text_disabled: rgb(0x55655F),
            border: rgb(0x25312E),
            menu_background: rgb(0x161E1C),
            menu_text: rgb(0xD6DFDB),
            menu_hover: rgb(0x202B28),
            popup_background: rgb(0x1A2321),
            popup_border: rgb(0x2E3D39),
            guide: rgb(0x10A57E),
            status_bar: rgb(0x0E7F64),
            status_bar_text: rgb(0xFFFFFF),
            code_lens: rgb(0x8FA39B),
            warning: rgb(0xE0A52B),
            success: rgb(0x6CC59A),
            panel_raised: rgb(0x1A2321),
            // Visual Studio's dark palette as the eye reads it (brief 0062).
            icon_folder: rgb(0xDCB67A),
            icon_csharp_file: rgb(0xA179DC),
            icon_csharp_project: rgb(0x3BA25A),
            icon_rust: rgb(0xDE7A4A),
            icon_json: rgb(0xCBCB41),
            icon_markdown: rgb(0x519ABA),
            icon_xml: rgb(0xE37933),
            icon_web: rgb(0x4FA6E0),
            icon_package: rgb(0x6DAEE1),
            icon_dependencies: rgb(0x519ABA),
            icon_solution: rgb(0xA179DC),
            icon_muted: rgb(0x9D9D9D),
            editor_caret: rgb(0xE0A52B),
            editor_selection: rgb(0x0F3F34),
            editor_line_number: rgb(0x5B6C65),
            editor_line_number_active: rgb(0xD6DFDB),
            editor_current_line: rgb(0x25312E),
            editor_find_match: rgb(0x5A4314),
            typography: Typography::default(),
        }
    }

    /// Eludite Light.
    pub fn light() -> Self {
        Self {
            name: "light",
            background: rgb(0xFCFBF8),
            chrome: rgb(0xEDECE5),
            chrome_text: rgb(0x56665F),
            panel: rgb(0xF5F4EF),
            panel_header: rgb(0xEDECE5),
            panel_header_text: rgb(0x56665F),
            panel_header_active: rgb(0x0E7F64),
            panel_header_active_text: rgb(0xFFFFFF),
            accent: rgb(0x0E7F64),
            tab_active: rgb(0xFCFBF8),
            tab_active_text: rgb(0x15201C),
            tab_strip_rule: rgb(0xD6D5CB),
            highlight: rgb(0xB57F18),
            tool_tab_active: rgb(0xF5F4EF),
            tool_tab_active_text: rgb(0x15201C),
            tool_tab_marker: rgb(0x0E7F64),
            text: rgb(0x15201C),
            text_muted: rgb(0x56665F),
            text_on_accent: rgb(0xFFFFFF),
            text_disabled: rgb(0x9AA59F),
            border: rgb(0xD6D5CB),
            menu_background: rgb(0xEDECE5),
            menu_text: rgb(0x15201C),
            menu_hover: rgb(0xE2E1D8),
            popup_background: rgb(0xFFFFFF),
            popup_border: rgb(0xCFCEC4),
            guide: rgb(0x0E7F64),
            status_bar: rgb(0x0E7F64),
            status_bar_text: rgb(0xFFFFFF),
            code_lens: rgb(0x56665F),
            warning: rgb(0x8A5E0B),
            success: rgb(0x1F7A45),
            panel_raised: rgb(0xEAE9E1),
            // The same hues, darker, so each reads on a light panel (brief 0062).
            icon_folder: rgb(0xA87B1E),
            icon_csharp_file: rgb(0x7B4DB3),
            icon_csharp_project: rgb(0x2E7D32),
            icon_rust: rgb(0xB5451B),
            icon_json: rgb(0x8A7A00),
            icon_markdown: rgb(0x2F6F9F),
            icon_xml: rgb(0xB4530C),
            icon_web: rgb(0x1F6FB2),
            icon_package: rgb(0x1C6EA4),
            icon_dependencies: rgb(0x2F6F9F),
            icon_solution: rgb(0x6F42B0),
            icon_muted: rgb(0x6E6E6E),
            editor_caret: rgb(0xB57F18),
            editor_selection: rgb(0xC0DCD3),
            editor_line_number: rgb(0x86928C),
            editor_line_number_active: rgb(0x15201C),
            editor_current_line: rgb(0xE2E1D8),
            editor_find_match: rgb(0xF3D9A4),
            typography: Typography::default(),
        }
    }

    /// Classic: Visual Studio Blue.
    pub fn classic() -> Self {
        Self {
            name: "classic",
            background: rgb(0xFFFFFF),
            chrome: rgb(0x293955),
            chrome_text: rgb(0xFFFFFF),
            panel: rgb(0xFFFFFF),
            panel_header: rgb(0x4D6082),
            panel_header_text: rgb(0xFFFFFF),
            panel_header_active: rgb(0xFFF29D),
            panel_header_active_text: rgb(0x1E1E1E),
            accent: rgb(0xFFF29D),
            tab_active: rgb(0xFFF29D),
            tab_active_text: rgb(0x1E1E1E),
            tab_strip_rule: rgb(0xFFF29D),
            highlight: rgb(0xFFF29D),
            tool_tab_active: rgb(0xFFF29D),
            tool_tab_active_text: rgb(0x1E1E1E),
            tool_tab_marker: rgb(0xFFF29D),
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
            // The same hues, darker, so each reads on a light panel (brief 0062).
            icon_folder: rgb(0xA87B1E),
            icon_csharp_file: rgb(0x7B4DB3),
            icon_csharp_project: rgb(0x2E7D32),
            icon_rust: rgb(0xB5451B),
            icon_json: rgb(0x8A7A00),
            icon_markdown: rgb(0x2F6F9F),
            icon_xml: rgb(0xB4530C),
            icon_web: rgb(0x1F6FB2),
            icon_package: rgb(0x1C6EA4),
            icon_dependencies: rgb(0x2F6F9F),
            icon_solution: rgb(0x6F42B0),
            icon_muted: rgb(0x6E6E6E),
            // Visual Studio Light's editor (Blue shares it).
            editor_caret: rgb(0x1E1E1E),
            editor_selection: rgb(0xADD6FF),
            editor_line_number: rgb(0x2B91AF),
            editor_line_number_active: rgb(0x1E1E1E),
            editor_current_line: rgb(0xEAEAF2),
            editor_find_match: rgb(0xF5CC84),
            typography: Typography::default(),
        }
    }

    /// `dark`, `light` or `classic`.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            "classic" => Some(Self::classic()),
            _ => None,
        }
    }

    /// A dark background: the editor uses Visual Studio Dark's syntax colors, else Visual Studio Light's.
    pub fn is_dark(&self) -> bool {
        let b = self.background;
        0.2126 * b.r + 0.7152 * b.g + 0.0722 * b.b < 0.5
    }

    pub fn all() -> [Self; 3] {
        [Self::dark(), Self::light(), Self::classic()]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Type families, text sizes and fixed chrome metrics, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Typography {
    /// The UI's family ([`crate::fonts`]).
    pub ui_font: &'static str,
    /// The family for code.
    pub mono_font: &'static str,
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
            ui_font: crate::fonts::UI_FONT,
            mono_font: crate::fonts::MONO_FONT,
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

/// WCAG relative luminance contrast between two colors.
#[cfg(test)]
pub(crate) fn contrast(a: Rgba, b: Rgba) -> f32 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eludite_dark_is_default() {
        let t = Theme::dark();
        assert_eq!(t.background, rgb(0x0F1514));
        assert_eq!(t.panel, rgb(0x131A18));
        assert_eq!(t.accent, rgb(0x0E7F64));
        assert_eq!(t.highlight, rgb(0xE0A52B));
        assert_eq!(Theme::default(), t);
        // Brief 0062's starting palette for VS Dark's icons.
        assert_eq!(t.icon_folder, rgb(0xDCB67A));
        assert_eq!(t.icon_csharp_file, rgb(0xA179DC));
        assert_eq!(t.icon_csharp_project, rgb(0x3BA25A));
        assert_eq!(t.icon_json, rgb(0xCBCB41));
        assert_eq!(t.icon_markdown, rgb(0x519ABA));
        assert_eq!(t.icon_xml, rgb(0xE37933));
        assert_eq!(t.icon_package, rgb(0x6DAEE1));
        assert_eq!(t.icon_dependencies, rgb(0x519ABA));
        assert_eq!(t.icon_solution, rgb(0xA179DC));
    }

    #[test]
    fn three_distinct_token_sets() {
        let [d, l, b] = Theme::all();
        assert_ne!(d, l);
        assert_ne!(l, b);
        assert_ne!(d, b);
        for name in ["dark", "light", "classic"] {
            assert_eq!(Theme::by_name(name).unwrap().name, name);
        }
        assert!(Theme::by_name("solarized").is_none());
        // Blue was renamed Classic.
        assert!(Theme::by_name("blue").is_none());
        // Dark and Light share the mark's teal accent.
        assert_eq!(d.accent, l.accent);
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
                ("active tab", t.tab_active_text, t.tab_active),
                ("active tool tab", t.tool_tab_active_text, t.tool_tab_active),
                ("status", t.status_bar_text, t.status_bar),
                ("code lens/background", t.code_lens, t.background),
                // Brief 0059: the Agents window's status colors and its prompt blocks.
                ("warning/panel", t.warning, t.panel),
                ("success/panel", t.success, t.panel),
                ("text/panel_raised", t.text, t.panel_raised),
                ("text/selection", t.text, t.editor_selection),
                (
                    "current line number",
                    t.editor_line_number_active,
                    t.background,
                ),
            ];
            for (what, fg, bg) in pairs {
                assert!(
                    contrast(fg, bg) >= 4.0,
                    "{} {what}: {}",
                    t.name,
                    contrast(fg, bg)
                );
            }
            // Brief 0062: every icon token reads on the panel the Workspace window draws on (WCAG's 3:1 for
            // graphics).
            let icons = [
                ("icon_folder", t.icon_folder),
                ("icon_csharp_file", t.icon_csharp_file),
                ("icon_csharp_project", t.icon_csharp_project),
                ("icon_rust", t.icon_rust),
                ("icon_json", t.icon_json),
                ("icon_markdown", t.icon_markdown),
                ("icon_xml", t.icon_xml),
                ("icon_web", t.icon_web),
                ("icon_package", t.icon_package),
                ("icon_dependencies", t.icon_dependencies),
                ("icon_solution", t.icon_solution),
                ("icon_muted", t.icon_muted),
            ];
            for (what, c) in icons {
                assert!(
                    contrast(c, t.panel) >= 3.0,
                    "{} {what}: {}",
                    t.name,
                    contrast(c, t.panel)
                );
            }
            // The caret and line numbers read on the editor (3:1, graphics and secondary text).
            for (what, c) in [
                ("caret", t.editor_caret),
                ("line number", t.editor_line_number),
            ] {
                assert!(
                    contrast(c, t.background) >= 3.0,
                    "{} {what}: {}",
                    t.name,
                    contrast(c, t.background)
                );
            }
            // The active tab's marker reads against the tab it marks, unless it is the tab's own fill (Classic).
            if t.highlight != t.tab_active {
                assert!(
                    contrast(t.highlight, t.tab_active) >= 3.0,
                    "{} highlight: {}",
                    t.name,
                    contrast(t.highlight, t.tab_active)
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
    fn only_dark_is_dark() {
        let [d, l, c] = Theme::all();
        assert!(d.is_dark() && !l.is_dark() && !c.is_dark());
    }

    #[test]
    fn typography_is_ordered() {
        let ty = Typography::default();
        assert!(ty.small < ty.ui && ty.ui < ty.body);
    }
}
