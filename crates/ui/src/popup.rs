//! IntelliSense popups (brief 0013): the completion list's rows with Visual Studio's kind icons, and the tooltip
//! panel Quick Info, Parameter Info and the completion documentation use. Stateless: the editor owns the state and
//! positions the popups; these draw them from the theme's tokens (`popup_background`, `popup_border`, `accent`).

use std::ops::Range;

use gpui::{
    Div, FontWeight, HighlightStyle, InteractiveElement, ParentElement, Rgba, SharedString,
    Stateful, Styled, StyledText, div, px, rgb,
};

use crate::Theme;

/// Height of a completion row; the list shows [`COMPLETION_ROWS`] of them.
pub const COMPLETION_ROW_HEIGHT: f32 = 20.;
/// Rows the completion list shows at once (Visual Studio shows nine).
pub const COMPLETION_ROWS: usize = 9;

/// What a completion item is: LSP 3.17's `CompletionItemKind`, which Visual Studio's icons distinguish too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CompletionKind {
    #[default]
    Text,
    Method,
    Function,
    Constructor,
    Field,
    Variable,
    Class,
    Interface,
    Module,
    Property,
    Unit,
    Value,
    Enum,
    Keyword,
    Snippet,
    Color,
    File,
    Reference,
    Folder,
    EnumMember,
    Constant,
    Struct,
    Event,
    Operator,
    TypeParameter,
}

const KINDS: [(CompletionKind, &str); 25] = [
    (CompletionKind::Text, "text"),
    (CompletionKind::Method, "method"),
    (CompletionKind::Function, "function"),
    (CompletionKind::Constructor, "constructor"),
    (CompletionKind::Field, "field"),
    (CompletionKind::Variable, "variable"),
    (CompletionKind::Class, "class"),
    (CompletionKind::Interface, "interface"),
    (CompletionKind::Module, "module"),
    (CompletionKind::Property, "property"),
    (CompletionKind::Unit, "unit"),
    (CompletionKind::Value, "value"),
    (CompletionKind::Enum, "enum"),
    (CompletionKind::Keyword, "keyword"),
    (CompletionKind::Snippet, "snippet"),
    (CompletionKind::Color, "color"),
    (CompletionKind::File, "file"),
    (CompletionKind::Reference, "reference"),
    (CompletionKind::Folder, "folder"),
    (CompletionKind::EnumMember, "enum_member"),
    (CompletionKind::Constant, "constant"),
    (CompletionKind::Struct, "struct"),
    (CompletionKind::Event, "event"),
    (CompletionKind::Operator, "operator"),
    (CompletionKind::TypeParameter, "type_parameter"),
];

impl CompletionKind {
    /// From LSP's `CompletionItemKind` number (1 is `Text`); unknown numbers are `Text`.
    pub fn from_lsp(kind: u32) -> Self {
        KINDS
            .get((kind as usize).wrapping_sub(1))
            .map_or(CompletionKind::Text, |(k, _)| *k)
    }

    /// Snake-case name, as `eludite.editor.complete` reports it.
    pub fn name(self) -> &'static str {
        KINDS
            .iter()
            .find(|(k, _)| *k == self)
            .map_or("text", |(_, n)| n)
    }

    /// The glyph and color of Visual Studio's icon for this kind: purple cubes for methods, blue for fields and
    /// locals, orange for classes and events, and so on.
    pub fn icon(self) -> (&'static str, Rgba) {
        use CompletionKind::*;
        const PURPLE: u32 = 0xB180D7;
        const BLUE: u32 = 0x75BEFF;
        const ORANGE: u32 = 0xEE9D28;
        const GREY: u32 = 0xC5C5C5;
        let (glyph, color) = match self {
            Method | Function | Constructor => ("\u{25C6}", PURPLE),
            Field => ("\u{25AC}", BLUE),
            Variable | Value | Reference => ("\u{25A3}", BLUE),
            Class => ("\u{25C8}", ORANGE),
            Interface => ("\u{25CB}", BLUE),
            Module | Folder | File => ("{}", GREY),
            Property => ("\u{25A4}", GREY),
            Enum => ("\u{25A6}", ORANGE),
            EnumMember => ("\u{25AA}", BLUE),
            Constant => ("\u{25A0}", GREY),
            Struct => ("\u{25A7}", BLUE),
            Event => ("\u{03DF}", ORANGE),
            Operator => ("\u{00B1}", GREY),
            TypeParameter => ("T", BLUE),
            Keyword => ("\u{25AD}", GREY),
            Snippet => ("\u{29C9}", GREY),
            Unit | Color | Text => ("abc", GREY),
        };
        (glyph, rgb(color))
    }
}

/// The 16 px kind icon.
pub fn kind_icon(kind: CompletionKind) -> Div {
    let (glyph, color) = kind.icon();
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(16.))
        .h(px(16.))
        .text_size(px(if glyph.len() > 2 { 8. } else { 12. }))
        .text_color(color)
        .child(glyph)
}

/// The frame of every IntelliSense popup.
pub fn popup_panel(theme: &Theme) -> Div {
    div()
        .rounded(px(6.))
        .bg(theme.popup_background)
        .border_1()
        .border_color(theme.popup_border)
        .shadow_md()
        .text_color(theme.text)
        .text_size(theme.typography.ui)
}

/// One completion row: icon and label, the characters that matched the typed text in bold and the guide color.
/// `matched` are byte ranges of `label`.
pub fn completion_row(
    theme: &Theme,
    id: impl Into<SharedString>,
    kind: CompletionKind,
    label: impl Into<SharedString>,
    matched: &[Range<usize>],
    selected: bool,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    let highlight = HighlightStyle {
        color: Some(theme.guide.into()),
        font_weight: Some(FontWeight::BOLD),
        ..Default::default()
    };
    let highlights: Vec<(Range<usize>, HighlightStyle)> = matched
        .iter()
        .filter(|r| {
            r.end <= label.len() && label.is_char_boundary(r.start) && label.is_char_boundary(r.end)
        })
        .map(|r| (r.clone(), highlight))
        .collect();
    let row = div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(COMPLETION_ROW_HEIGHT))
        .px(px(4.))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(kind_icon(kind))
        .child(StyledText::new(label).with_highlights(highlights));
    if selected {
        row.bg(theme.accent).text_color(theme.text_on_accent)
    } else {
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_from_lsp_numbers() {
        assert_eq!(CompletionKind::from_lsp(1), CompletionKind::Text);
        assert_eq!(CompletionKind::from_lsp(2), CompletionKind::Method);
        assert_eq!(CompletionKind::from_lsp(10), CompletionKind::Property);
        assert_eq!(CompletionKind::from_lsp(25), CompletionKind::TypeParameter);
        assert_eq!(CompletionKind::from_lsp(0), CompletionKind::Text);
        assert_eq!(CompletionKind::from_lsp(99), CompletionKind::Text);
        assert_eq!(CompletionKind::EnumMember.name(), "enum_member");
        for (i, (k, _)) in KINDS.iter().enumerate() {
            assert_eq!(CompletionKind::from_lsp(i as u32 + 1), *k);
        }
        // Methods and fields look different, as in Visual Studio.
        assert_ne!(CompletionKind::Method.icon(), CompletionKind::Field.icon());
        assert_ne!(
            CompletionKind::Class.icon(),
            CompletionKind::Interface.icon()
        );
    }
}
