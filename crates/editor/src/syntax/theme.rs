//! Colors for highlight kinds.

use gpui::{Rgba, rgb};
use eludite_ui::Theme;

use super::HighlightKind;

/// Maps each [`HighlightKind`] to a color.
///
/// Built from a `eludite_ui::Theme`: text with no syntax color, punctuation and
/// variables use the theme's `text` token. `crates/ui` has no syntax tokens
/// yet, so the syntax colors themselves are defined here, matching Visual
/// Studio's Dark theme; they move into `eludite_ui::Theme` when that crate
/// grows syntax tokens (a later brief).
#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxTheme {
    pub default: Rgba,
    colors: [Option<Rgba>; 22],
}

impl SyntaxTheme {
    /// Visual Studio Dark syntax colors on top of `theme`.
    pub fn vs_dark(theme: &Theme) -> Self {
        use HighlightKind::*;
        let mut colors = [None; 22];
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
        Self::vs_dark(&Theme::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unstyled_kinds_use_the_ui_text_token() {
        let ui = Theme::vs_dark();
        let t = SyntaxTheme::vs_dark(&ui);
        assert_eq!(t.color(HighlightKind::Variable), ui.text);
        assert_eq!(t.color(HighlightKind::Punctuation), ui.text);
        assert_eq!(t.color(HighlightKind::Keyword), rgb(0x569CD6));
        assert_ne!(t.color(HighlightKind::Comment), ui.text);
    }
}
