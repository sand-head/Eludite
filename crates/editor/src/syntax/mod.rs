//! Syntax highlighting with tree-sitter.
//!
//! - [`language`]: languages as data ([`LanguageConfig`]) and a
//!   [`LanguageRegistry`].
//! - [`LineHighlights`]: highlight spans per buffer row, which can be
//!   interpolated through edits so stale highlights stay aligned with the text
//!   until fresh ones arrive.
//! - [`Highlighter`]: owns the parser and tree; [`Highlighter::step`] does an
//!   incremental re-parse and highlights dirty rows. `EditorView` runs it on
//!   the [`SyntaxThread`], never on the UI thread. Buffers over
//!   [`TREE_RETAIN_LIMIT`] drop their tree after each highlight pass and
//!   parse from scratch on the next edit.
//! - [`alloc`]: tree-sitter's C allocations go through Rust's global
//!   allocator, and [`alloc::live_bytes`] reports how much they hold.
//! - [`SyntaxTheme`]: maps [`HighlightKind`] to colors.
//!
//! The tree-sitter glue is written for Eludite, following the approach of
//! Zed's `language/src/syntax_map.rs` (incremental re-parse from buffer edits,
//! `changed_ranges` to find rows to re-highlight) without porting its
//! injection layers, which C# and Rust do not need yet.

pub mod alloc;
mod highlighter;
mod highlights;
pub mod language;
mod theme;
mod worker;

pub use highlighter::{HighlightStats, HighlightUpdate, Highlighter, TREE_RETAIN_LIMIT};
pub use highlights::{LineHighlights, Span};
pub use language::{Language, LanguageConfig, LanguageError, LanguageRegistry};
pub use theme::SyntaxTheme;
pub use worker::SyntaxThread;

/// What a span of source text is, independent of color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum HighlightKind {
    Keyword,
    Type,
    TypeBuiltin,
    Function,
    Macro,
    String,
    Escape,
    Number,
    Constant,
    ConstantBuiltin,
    Comment,
    DocComment,
    Variable,
    Parameter,
    VariableBuiltin,
    Property,
    Attribute,
    Namespace,
    Label,
    Operator,
    Punctuation,
    Preprocessor,
}

/// Capture name to kind. Lookup tries the full name, then drops trailing
/// `.segment`s, so `function.method` falls back to `function`.
const CAPTURE_KINDS: &[(&str, HighlightKind)] = &[
    ("keyword", HighlightKind::Keyword),
    ("type.builtin", HighlightKind::TypeBuiltin),
    ("type", HighlightKind::Type),
    ("constructor", HighlightKind::Type),
    ("function.macro", HighlightKind::Macro),
    ("function", HighlightKind::Function),
    ("string.escape", HighlightKind::Escape),
    ("escape", HighlightKind::Escape),
    ("string", HighlightKind::String),
    ("number", HighlightKind::Number),
    ("constant.builtin", HighlightKind::ConstantBuiltin),
    ("constant", HighlightKind::Constant),
    ("comment.documentation", HighlightKind::DocComment),
    ("comment", HighlightKind::Comment),
    ("variable.parameter", HighlightKind::Parameter),
    ("variable.builtin", HighlightKind::VariableBuiltin),
    ("variable", HighlightKind::Variable),
    ("property", HighlightKind::Property),
    ("attribute", HighlightKind::Attribute),
    ("module", HighlightKind::Namespace),
    ("namespace", HighlightKind::Namespace),
    ("label", HighlightKind::Label),
    ("operator", HighlightKind::Operator),
    ("punctuation", HighlightKind::Punctuation),
    ("preprocessor", HighlightKind::Preprocessor),
];

impl HighlightKind {
    /// Map a tree-sitter capture name (without the `@`) to a kind.
    pub fn from_capture_name(name: &str) -> Option<Self> {
        let mut name = name;
        loop {
            if let Some((_, kind)) = CAPTURE_KINDS.iter().find(|(n, _)| *n == name) {
                return Some(*kind);
            }
            name = &name[..name.rfind('.')?];
        }
    }
}
