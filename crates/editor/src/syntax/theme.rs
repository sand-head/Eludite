//! Colors for highlight kinds.

use eludite_ui::Theme;
use gpui::{Rgba, rgb};

use super::HighlightKind;

/// Maps each [`HighlightKind`] to a color.
///
/// Built from an `eludite_ui::Theme`: text with no syntax color, punctuation and
/// variables use the theme's `text` token. `crates/ui` has no syntax tokens
/// yet, so the syntax colors themselves are defined here, matching Visual
/// Studio's Dark and Light themes; they move into `eludite_ui::Theme` when that
/// crate grows syntax tokens (a later brief).
#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxTheme {
    pub default: Rgba,
    colors: [Option<Rgba>; HighlightKind::ALL.len()],
}

impl SyntaxTheme {
    /// Visual Studio Dark's colors on a dark theme, Visual Studio Light's on a light one.
    pub fn for_theme(theme: &Theme) -> Self {
        if theme.is_dark() {
            Self::vs_dark(theme)
        } else {
            Self::vs_light(theme)
        }
    }

    /// Visual Studio Light syntax colors on top of `theme`.
    pub fn vs_light(theme: &Theme) -> Self {
        use HighlightKind::*;
        let mut colors = [None; HighlightKind::ALL.len()];
        let mut set = |k: HighlightKind, c: u32| colors[k as usize] = Some(rgb(c));
        set(Keyword, 0x0000FF);
        set(TypeBuiltin, 0x0000FF);
        set(ConstantBuiltin, 0x0000FF);
        set(VariableBuiltin, 0x0000FF);
        set(Type, 0x2B91AF);
        set(Attribute, 0x2B91AF);
        set(Function, 0x74531F);
        set(Macro, 0x8F08C4);
        set(String, 0xA31515);
        set(Escape, 0xB5200D);
        set(Number, 0x098658);
        set(Constant, 0x1F377F);
        set(Comment, 0x008000);
        set(DocComment, 0x008000);
        set(Parameter, 0x1F377F);
        set(Operator, 0x000000);
        set(Preprocessor, 0x808080);
        set(Label, 0x000000);
        set(Tag, 0x800000);
        set(AttributeName, 0xE50000);
        set(PropertyName, 0x0451A5);
        set(Selector, 0x800000);
        Self {
            default: theme.text,
            colors,
        }
    }

    /// Visual Studio Dark syntax colors on top of `theme`.
    pub fn vs_dark(theme: &Theme) -> Self {
        use HighlightKind::*;
        let mut colors = [None; HighlightKind::ALL.len()];
        let mut set = |k: HighlightKind, c: u32| colors[k as usize] = Some(rgb(c));
        set(Keyword, 0x569CD6);
        set(TypeBuiltin, 0x569CD6);
        set(ConstantBuiltin, 0x569CD6);
        set(VariableBuiltin, 0x569CD6);
        set(Type, 0x4EC9B0);
        set(Attribute, 0x4EC9B0);
        set(Function, 0xDCDCAA);
        set(Macro, 0xBD63C5);
        set(String, 0xD69D85);
        set(Escape, 0xFFD68F);
        set(Number, 0xB5CEA8);
        set(Constant, 0xB8D7A3);
        set(Comment, 0x57A64A);
        set(DocComment, 0x608B4E);
        set(Parameter, 0x9CDCFE);
        set(Operator, 0xB4B4B4);
        set(Preprocessor, 0x9B9B9B);
        set(Label, 0xC8C8C8);
        // Visual Studio Dark's HTML, CSS and JSON colors (brief 0050).
        set(Tag, 0x569CD6);
        set(AttributeName, 0x9CDCFE);
        set(PropertyName, 0x9CDCFE);
        set(Selector, 0xD7BA7D);
        Self {
            default: theme.text,
            colors,
        }
    }

    /// The color for `kind`, or the default text color.
    pub fn color(&self, kind: HighlightKind) -> Rgba {
        self.colors[kind as usize].unwrap_or(self.default)
    }

    /// Override one kind's color.
    pub fn set(&mut self, kind: HighlightKind, color: Rgba) {
        self.colors[kind as usize] = Some(color);
    }
}

impl Default for SyntaxTheme {
    fn default() -> Self {
        Self::for_theme(&Theme::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstyled_kinds_use_the_ui_text_token() {
        let ui = Theme::dark();
        let t = SyntaxTheme::vs_dark(&ui);
        assert_eq!(t.color(HighlightKind::Variable), ui.text);
        assert_eq!(t.color(HighlightKind::Punctuation), ui.text);
        assert_eq!(t.color(HighlightKind::Keyword), rgb(0x569CD6));
        assert_ne!(t.color(HighlightKind::Comment), ui.text);
    }

    #[test]
    fn light_themes_get_light_syntax_colors() {
        assert_eq!(
            SyntaxTheme::for_theme(&Theme::dark()),
            SyntaxTheme::vs_dark(&Theme::dark())
        );
        for ui in [Theme::light(), Theme::classic()] {
            let t = SyntaxTheme::for_theme(&ui);
            assert_eq!(t.color(HighlightKind::Keyword), rgb(0x0000FF));
            assert_eq!(t.color(HighlightKind::Variable), ui.text);
        }
    }
}
