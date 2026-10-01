//! Eludite's editor core (PLAN.md 4.1): text buffer, editing model, syntax
//! highlighting and the GPUI editor view.
//!
//! # Public API
//!
//! Embedding an editor takes three calls:
//!
//! ```ignore
//! cx.bind_keys(eludite_editor::key_bindings());          // once per app
//! let registry = LanguageRegistry::with_builtins();       // once per app
//! let view = cx.new(|cx| EditorView::new(Buffer::load(path)?, registry.for_path(path), cx));
//! ```
//!
//! - [`Buffer`]: the text. Wraps Zed's vendored `text::Buffer` (anchors,
//!   transactions, undo and redo) and adds what files need: UTF-8 BOM and
//!   line endings (CRLF, LF, CR, mixed) preserved byte for byte on save, and
//!   [`LARGE_FILE_THRESHOLD`] (32 MiB), above which a file opens without
//!   highlighting. Offsets are UTF-8 bytes in `\n`-normalized text; rows and
//!   columns are [`text::Point`]s.
//! - [`Editor`]: a buffer plus multiple selections and every editing
//!   operation (typing, deleting, movement, multi-caret, find, undo/redo,
//!   clipboard text). No GPUI; unit-testable.
//! - [`EditorView`]: the GPUI view. Draws line numbers, syntax colors,
//!   selections, carets, find matches and decorations; scrolls by pixel;
//!   takes text through GPUI's input handler, keys through the actions in
//!   [`key_bindings`] (context [`KEY_CONTEXT`], Visual Studio defaults), and
//!   the mouse (click, drag, Shift+click, Ctrl+Alt+click, double and triple
//!   click). [`EditorView::update_editor`] is the entry point for
//!   programmatic edits.
//! - [`syntax`]: tree-sitter highlighting. Languages are data
//!   ([`syntax::LanguageConfig`]); C# and Rust are built in. Parsing and
//!   highlighting run on the [`syntax::SyntaxThread`], never on the UI
//!   thread; until a result arrives the view shows the previous highlights,
//!   moved through the edits since ([`syntax::LineHighlights::interpolate`]).
//!
//! # Hooks for LSP features (next brief)
//!
//! [`EditorView::set_decorations`] takes named layers of [`Decoration`]s
//! (anchor ranges styled as background, underline or wavy underline, or
//! foreground color). Diagnostics map to wavy underlines, semantic tokens to
//! foreground colors, document highlights to backgrounds. Anchors keep them
//! attached to the text while the user types; the producer replaces a layer
//! when fresh results arrive and drops results for stale buffer versions
//! ([`Buffer::version`]). [`EditorView::pixel_position_for_offset`] places
//! popups (completion, hover) at a text position. Inlay hints need text
//! that is not in the buffer, which this view does not model yet (no display
//! map); see `docs/briefs/0009-report.md`.
//!
//! # Known gaps
//!
//! No IME composition (composed text is inserted as typed), no soft wrap,
//! folding, minimap or sticky scroll, no right-to-left text, grapheme
//! clusters are not respected by cursor movement (characters are), very long
//! lines are shaped whole, find is ASCII case-insensitive only, and files
//! must be UTF-8.

mod buffer;
pub mod display;
mod editor;
pub mod syntax;
mod view;

pub use buffer::{Buffer, LARGE_FILE_THRESHOLD, LineEnding, LoadError};
pub use editor::{ClickKind, Editor, FindQuery, Selection, SelectionRange};
pub use text;
pub use view::{
    Decoration, DecorationStyle, EditorStyle, EditorView, KEY_CONTEXT, default_font_family,
    key_bindings,
};
/// Editor actions, for binding keys and dispatching from commands.
pub mod actions {
    pub use crate::view::{
        AddCaretAbove, AddCaretBelow, Backspace, Cancel, Copy, Cut, Delete, DeleteWordLeft,
        DeleteWordRight, Find, FindNext, FindPrevious, MoveDown, MoveLeft, MoveRight, MoveToEnd,
        MoveToLineEnd, MoveToLineStart, MoveToStart, MoveUp, MoveWordLeft, MoveWordRight, Newline,
        PageDown, PageUp, Paste, Redo, SelectAll, SelectDown, SelectLeft, SelectNextOccurrence,
        SelectPageDown, SelectPageUp, SelectRight, SelectToEnd, SelectToLineEnd, SelectToLineStart,
        SelectToStart, SelectUp, SelectWordLeft, SelectWordRight, Tab, ToggleFindCaseSensitive,
        Undo,
    };
}
