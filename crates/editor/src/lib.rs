//! Eludite's editor core (PLAN.md 4.1): text buffer, editing model, syntax
//! highlighting and the GPUI editor view.
//!
//! # Public API
//!
//! Embedding an editor takes three calls:
//!
//! ```ignore
//! cx.bind_keys(eludite_editor::key_bindings());         // once per app
//! let registry = LanguageRegistry::with_builtins();       // once per app
//! let view = cx.new(|cx| EditorView::new(Buffer::load(path)?, registry.for_path(path), cx));
//! ```
//!
//! - [`Buffer`]: the text. Wraps Zed's `text::Buffer` (anchors,
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
//!   ([`syntax::LanguageConfig`]); C#, Rust, the web languages (TypeScript,
//!   TSX, JavaScript, HTML, CSS, JSON, JSON with comments), Razor
//!   (`.razor`, `.cshtml`), Visual Basic (`.vb`) and F# (`.fs`, `.fsx`,
//!   `.fsscript`, and `.fsi` signatures) are built in ([`syntax::BUILTINS`]). Parsing and
//!   highlighting run on the [`syntax::SyntaxThread`], never on the UI
//!   thread; until a result arrives the view shows the previous highlights,
//!   moved through the edits since ([`syntax::LineHighlights::interpolate`]).
//!
//! # Hooks for LSP features (next brief)
//!
//! [`EditorView::set_decorations`] (read back with [`EditorView::decorations`]) takes named layers of [`Decoration`]s
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
//! # IntelliSense (brief 0013)
//!
//! [`intellisense`] holds the completion list, Quick Info and Parameter Info
//! layers. The view decides when a feature is wanted (typing an identifier
//! character at the start of a word, `.`, `(`, `<`, `,`, the keys
//! Ctrl+Space, Ctrl+Shift+Space and Ctrl+K, Ctrl+I, the mouse resting over a
//! word for [`intellisense::HOVER_DELAY`]) and emits an [`EditorEvent`]; the
//! owner fetches the answer off the UI thread and hands it back with
//! [`EditorView::open_completion`] / [`EditorView::set_completions`],
//! [`EditorView::open_hover`] / [`EditorView::set_hover`] and
//! [`EditorView::open_signature_help`] / [`EditorView::set_signature_help`],
//! quoting the id `open_*` returned, so superseded answers are ignored. The
//! list is filtered with Zed's `fuzzy` crate as the user types, and
//! [`EditorView::complete_from_syntax`] fills it from the identifiers of the
//! buffer's tree-sitter tree while no server can answer. Nothing here is
//! specific to a language or to LSP.
//!
//! # Code actions (brief 0015)
//!
//! [`EditorView::set_lightbulb`] shows Visual Studio's light bulb in the margin at the left of the line numbers
//! (yellow for fixes, blue-gray for refactorings only); clicking it emits [`EditorEvent::LightbulbClicked`]. The owner
//! decides when code actions exist. [`Editor::apply_edits`] applies a workspace edit's ranges as one undo step that
//! never merges with typing, and [`Editor::merge_transactions`] folds a later step into an earlier one (a
//! completion's additional edits into its commit).
//!
//! # Debugging (brief 0018)
//!
//! [`EditorView::set_breakpoint_glyphs`] draws breakpoints in the margin at the far left (filled, hollow,
//! conditional, unbound; [`BreakpointGlyph`]); a click there emits [`EditorEvent::BreakpointMarginClicked`] and the
//! owner toggles the breakpoint. [`EditorView::set_execution_point`] draws the debugger's arrow and highlights the
//! statement ([`ExecutionKind`]: yellow where execution stopped, green for a caller's frame).
//! [`EditorView::expression_at`] gives the member-access expression under the mouse for data tips, which the owner
//! shows with [`EditorView::open_data_tip`] and [`EditorView::set_hover`].
//!
//! # CodeLens (brief 0052)
//!
//! [`EditorView::set_code_lenses`] shows Visual Studio's CodeLens indicators on a display-only row above each member
//! (`3 references | Run Test | Debug Test`): the row takes layout height ([`VerticalLayout`]) but holds no buffer
//! text, so the caret, selections, line numbers and gutter never move because of it. The view says when lenses are
//! wanted ([`EditorEvent::CodeLensRequested`]: on [`EditorView::set_code_lens_enabled`], 150 ms after the last edit,
//! on [`EditorView::refresh_code_lenses`]) and which to resolve ([`EditorEvent::CodeLensResolve`]: those within 50
//! lines of the visible range); a click or Ctrl+K, Ctrl+Q emits [`EditorEvent::CodeLensActivated`]. Overlays drawn
//! beside the text place themselves with [`EditorView::row_top`].
//!
//! # Text input (brief 0057)
//!
//! [`TextInput`] is a text box over the same [`Editor`] (a prompt, a search box): its own element with no gutter,
//! highlighting or popups, text that wraps at the box's width with the caret, clicks and Up and Down mapped through
//! the wrapped rows, `min_rows` to `max_rows` tall then scrolling, selection, the clipboard, undo, IME composition, and
//! [`TextInputEvent`]s for Enter, Escape, edits, and Up and Down past the first and last rows. Its keys are bound by
//! [`key_bindings`] in the context [`INPUT_KEY_CONTEXT`].
//!
//! # Known gaps
//!
//! In the editor view: no IME composition (composed text is inserted as typed), no soft wrap,
//! folding, minimap or sticky scroll, no right-to-left text, grapheme
//! clusters are not respected by cursor movement (characters are), very long
//! lines are shaped whole, find is ASCII case-insensitive only, and files
//! must be UTF-8.

mod buffer;
mod codelens;
mod debugging;
pub mod display;
mod editor;
mod input;
pub mod intellisense;
mod popups;
pub mod syntax;
mod view;

pub use buffer::{Buffer, LARGE_FILE_THRESHOLD, LineEnding, LoadError};
pub use debugging::{BreakpointGlyph, ExecutionKind};
pub use display::{RowHit, VerticalLayout};
pub use editor::{ClickKind, Editor, FindQuery, Selection, SelectionRange};
pub use eludite_ui::CompletionKind;
pub use input::{INPUT_KEY_CONTEXT, InputLayout, TextInput, TextInputEvent};
pub use intellisense::{
    AcceptedCompletion, CodeLens, CodeLensSnapshot, CompletionEdit, CompletionItem,
    CompletionRequest, CompletionSnapshot, CompletionSource, CompletionTrigger, EditorEvent,
    HoverSnapshot, SignatureHelpData, SignatureInfo, SignatureSnapshot, SignatureTrigger,
};
pub use text;
pub use view::{
    COMPLETION_CONTEXT, COMPLETION_SELECTED_CONTEXT, Decoration, DecorationStyle, EditorStyle,
    EditorView, KEY_CONTEXT, LightbulbKind, SIGNATURES_CONTEXT, default_font_family, key_bindings,
};
/// The text input's actions (context [`INPUT_KEY_CONTEXT`]).
pub mod input_actions {
    pub use crate::input::{
        Backspace, Copy, Cut, Delete, DeleteWordLeft, DeleteWordRight, Escape, MoveDown, MoveEnd,
        MoveHome, MoveLeft, MoveRight, MoveToEnd, MoveToStart, MoveUp, MoveWordLeft, MoveWordRight,
        Newline, Paste, Redo, SelectAll, SelectDown, SelectEnd, SelectHome, SelectLeft,
        SelectRight, SelectToEnd, SelectToStart, SelectUp, SelectWordLeft, SelectWordRight, Submit,
        Tab, Undo,
    };
}
/// Editor actions, for binding keys and dispatching from commands.
pub mod actions {
    pub use crate::view::{
        AcceptCompletion, CompletionPageDown, CompletionPageUp, NextSignature, PreviousSignature,
        SelectNextCompletion, SelectPreviousCompletion, ShowCodeLensMenu, ShowCompletions,
        ShowHover, ShowSignatureHelp,
    };
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
