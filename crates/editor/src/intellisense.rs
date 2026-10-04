//! IntelliSense display layers and trigger logic (brief 0013): the completion list, Quick Info (hover) and Parameter
//! Info (signature help).
//!
//! The editor knows nothing about LSP. It decides *when* a feature is wanted (typing an identifier character, a
//! trigger character, `(` or `,`, the mouse resting over a word) and tells its owner with an [`EditorEvent`]; the owner
//! asks a language server (or anything else) and hands the answer back through `EditorView`'s `open_*` and `set_*`
//! methods, quoting the request id `open_*` returned so a superseded answer is ignored. While no server can answer,
//! [`identifiers`] lists the identifiers of the buffer's tree-sitter tree as a fallback list.
//!
//! The pure parts live here (types, word boundaries, snippet text, the active-parameter heuristic, filtering order);
//! the state and drawing live in `view.rs`.

use std::ops::Range;
use std::sync::Arc;

use eludite_ui::CompletionKind;
use eludite_ui::markdown::{self, Block};
use fuzzy::{StringMatch, StringMatchCandidate};
use text::Anchor;

use crate::syntax::Language;

pub mod emmet;

/// How long the mouse must rest over an identifier before Quick Info is requested.
pub const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

/// Characters that open the completion list when typed, besides identifier characters (the brief 0013 contract).
pub const DEFAULT_COMPLETION_TRIGGERS: [char; 3] = ['.', '(', '<'];

/// Why the completion list is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTrigger {
    /// Explicitly (Ctrl+Space).
    Invoked,
    /// An identifier character typed at the start of a word.
    Typing(char),
    /// A trigger character (`.`, `(`, `<`).
    Character(char),
    /// The text typed since the list opened needs fresh items: the list is incomplete, or the previous request is
    /// still in flight. Each such keystroke supersedes the previous request.
    Refresh,
}

/// Why Parameter Info is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureTrigger {
    /// Explicitly (Ctrl+Shift+Space).
    Invoked,
    /// `(` or `,` typed.
    Character(char),
    /// The caret moved or the text changed while Parameter Info is open: refresh the active parameter.
    Retrigger,
}

/// What an [`crate::EditorView`] asks its owner for. The owner answers through the view's `open_*` / `set_*` methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEvent {
    CompletionTriggered(CompletionTrigger),
    /// Item `index` of completion list `id` is selected and has no documentation yet: resolve it lazily.
    ResolveCompletion {
        id: u64,
        index: usize,
    },
    /// The completion list closed; a request still in flight can be canceled.
    CompletionClosed,
    /// The mouse rested over the identifier at `offset` for [`HOVER_DELAY`].
    HoverTriggered {
        offset: usize,
    },
    HoverClosed,
    SignatureHelpTriggered(SignatureTrigger),
    SignatureHelpClosed,
    /// Ctrl+click on the text at `offset` (Visual Studio's Go To Definition gesture, brief 0014). The caret has
    /// already moved there; the owner asks a language server and navigates.
    GoToDefinition {
        offset: usize,
    },
    /// The light bulb in the margin of `row` was clicked (brief 0015): the owner opens its menu.
    LightbulbClicked {
        row: u32,
    },
    /// The breakpoint margin of `row` (0-based) was clicked (brief 0018): the owner toggles a breakpoint there.
    BreakpointMarginClicked {
        row: u32,
    },
    /// The document's lenses are wanted (brief 0052): it was opened, edited [`CODE_LENS_DEBOUNCE`] ago, or refreshed.
    /// The owner asks its server(s) and answers with [`crate::EditorView::set_code_lenses`] quoting `id`; an answer
    /// for another id, or computed on text the person has changed since, is dropped.
    CodeLensRequested {
        id: u64,
    },
    /// These unresolved lenses came within [`CODE_LENS_MARGIN`] lines of the visible range: resolve them and update
    /// them with [`crate::EditorView::update_code_lens`].
    CodeLensResolve {
        ids: Vec<u64>,
    },
    /// Lens `id` was clicked, or chosen from the keyboard (Ctrl+K, Ctrl+Q): the owner does what it says (the References
    /// popup, Run Test, Debug Test). Its bounds are [`crate::EditorView::code_lens_bounds`].
    CodeLensActivated {
        id: u64,
        keyboard: bool,
    },
}

/// How long after the last edit the lenses are asked for again (brief 0052).
pub const CODE_LENS_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// Lines beyond the visible range, on each side, whose lenses are resolved (brief 0052).
pub const CODE_LENS_MARGIN: u32 = 50;

/// One CodeLens indicator as the owner hands it to the editor (brief 0052).
#[derive(Debug, Clone, PartialEq)]
pub struct CodeLens {
    /// The owner's id, quoted back in [`EditorEvent::CodeLensResolve`] and [`EditorEvent::CodeLensActivated`].
    pub id: u64,
    /// Where the member starts: the lens shows on the lens row above its line, and follows it as the text changes.
    pub offset: usize,
    /// What the indicator reads (`3 references`, `Run Test`).
    pub title: String,
    /// False until resolved: the editor asks for it when its line comes near the visible range.
    pub resolved: bool,
    /// A test's last outcome, drawn before the title in its color.
    pub glyph: Option<eludite_ui::TestGlyph>,
}

/// A lens as the editor shows it (for tests and commands).
#[derive(Debug, Clone, PartialEq)]
pub struct CodeLensSnapshot {
    pub id: u64,
    /// The buffer row the lens row is above (0-based).
    pub row: u32,
    pub title: String,
    pub resolved: bool,
    pub glyph: Option<eludite_ui::TestGlyph>,
}

/// The rows whose lenses are resolved: `visible` and [`CODE_LENS_MARGIN`] lines on each side, within the document.
pub fn code_lens_window(visible: Range<u32>, line_count: u32) -> Range<u32> {
    visible.start.saturating_sub(CODE_LENS_MARGIN)
        ..visible
            .end
            .saturating_add(CODE_LENS_MARGIN)
            .min(line_count.max(1))
}

/// The editor's side of the lens requests (brief 0052): when the document's lenses are wanted, whether an answer is
/// current, and which lenses to resolve. Time is the view's: it starts a [`CODE_LENS_DEBOUNCE`] timer on every edit
/// and asks [`CodeLensPipeline::debounced`] when it fires.
#[derive(Debug, Default)]
pub struct CodeLensPipeline {
    enabled: bool,
    next_id: u64,
    /// The request in flight: its id and the buffer version it was made on.
    pending: Option<(u64, clock::Global)>,
    /// The text changed since the last request.
    stale: bool,
    /// Lenses whose resolve was asked for.
    asked: std::collections::HashSet<u64>,
}

impl CodeLensPipeline {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Turn lenses on or off; returns whether it changed. Off forgets the request in flight.
    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        if self.enabled == enabled {
            return false;
        }
        self.enabled = enabled;
        if !enabled {
            self.pending = None;
            self.asked.clear();
        }
        true
    }

    /// Ask for the lenses now (the document opened, a refresh): the id to quote, or `None` while lenses are off. A
    /// newer request supersedes the one in flight.
    pub fn request(&mut self, version: &clock::Global) -> Option<u64> {
        if !self.enabled {
            return None;
        }
        self.next_id += 1;
        self.pending = Some((self.next_id, version.clone()));
        self.stale = false;
        Some(self.next_id)
    }

    /// The text changed: the lenses are asked for again [`CODE_LENS_DEBOUNCE`] after the last edit.
    pub fn edited(&mut self) {
        if self.enabled {
            self.stale = true;
        }
    }

    /// The debounce after an edit ran out without another edit: the request to make, if any.
    pub fn debounced(&mut self, version: &clock::Global) -> Option<u64> {
        if self.stale {
            self.request(version)
        } else {
            None
        }
    }

    /// Whether the answer to request `id` is current: it is the newest request's, and the text is the one it was
    /// made on (`version`). A stale answer is dropped (CLAUDE.md invariant 12); the debounce asks again.
    pub fn accept(&mut self, id: u64, version: &clock::Global) -> bool {
        match &self.pending {
            Some((pending, made_on)) if *pending == id && made_on == version => {
                self.pending = None;
                self.asked.clear();
                true
            }
            _ => false,
        }
    }

    /// The unresolved lenses (`(id, row, resolved)`) inside `window` not asked for yet; they are marked asked.
    pub fn to_resolve(
        &mut self,
        lenses: impl IntoIterator<Item = (u64, u32, bool)>,
        window: Range<u32>,
    ) -> Vec<u64> {
        if !self.enabled {
            return Vec::new();
        }
        lenses
            .into_iter()
            .filter(|(id, row, resolved)| {
                !resolved && window.contains(row) && self.asked.insert(*id)
            })
            .map(|(id, _, _)| id)
            .collect()
    }

    /// Ask for lens `id` again next time it is in the window (its resolve failed).
    pub fn retry(&mut self, id: u64) {
        self.asked.remove(&id);
    }
}

/// Where the shown completion items come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionSource {
    /// A language server.
    LanguageServer,
    /// The identifiers of the buffer's syntax tree, while no language server can answer.
    Syntax,
}

impl CompletionSource {
    /// `languageServer` or `syntax` (the `eludite.editor.complete` output).
    pub fn name(self) -> &'static str {
        match self {
            CompletionSource::LanguageServer => "languageServer",
            CompletionSource::Syntax => "syntax",
        }
    }
}

/// Replace `range` with `new_text` when the item is committed.
#[derive(Debug, Clone)]
pub struct CompletionEdit {
    pub range: Range<Anchor>,
    pub new_text: String,
}

/// One completion item, already in editor terms.
#[derive(Debug, Clone, Default)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    /// The signature or type, shown beside the list for the selected item.
    pub detail: Option<String>,
    /// Markdown documentation; `None` until resolved.
    pub documentation: Option<String>,
    /// What the typed text is matched against; the label when `None`.
    pub filter_text: Option<String>,
    /// Order with an empty filter; the label when `None`.
    pub sort_text: Option<String>,
    /// The edit to apply; without one the typed word is replaced by `insert_text` (or the label).
    pub edit: Option<CompletionEdit>,
    pub insert_text: Option<String>,
    /// True once documentation was fetched (or there is nothing to fetch).
    pub resolved: bool,
}

impl CompletionItem {
    fn filter_string(&self) -> &str {
        self.filter_text.as_deref().unwrap_or(&self.label)
    }
}

/// The completion list as shown, for commands and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionSnapshot {
    /// The request id the list belongs to.
    pub id: u64,
    /// True while waiting for items.
    pub loading: bool,
    /// True when the list is drawn (it has matching items).
    pub visible: bool,
    pub source: Option<CompletionSource>,
    /// Text typed since the start of the word.
    pub filter: String,
    /// Matching items in display order: (label, kind, detail).
    pub items: Vec<(String, CompletionKind, Option<String>)>,
    pub selected: Option<usize>,
    /// Offset of the start of the word being completed.
    pub word_start: usize,
}

/// What [`crate::EditorView::open_completion`] returns: the request to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionRequest {
    pub id: u64,
    /// The caret (where the request is made).
    pub offset: usize,
    /// The start of the word being completed.
    pub word_start: usize,
}

/// The item a commit applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedCompletion {
    pub label: String,
    /// The text that replaced the typed word.
    pub text: String,
    /// The request id of the items the committed one came from, and its index among them (as in
    /// [`EditorEvent::ResolveCompletion`]), so the owner can find the server's item (its additional edits).
    pub list: u64,
    pub index: usize,
}

/// One overload in Parameter Info.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignatureInfo {
    pub label: String,
    /// Markdown.
    pub documentation: Option<String>,
    /// Byte ranges of the parameters in `label`.
    pub parameters: Vec<Range<usize>>,
    /// Overrides [`SignatureHelpData::active_parameter`] for this overload.
    pub active_parameter: Option<usize>,
}

/// An answer for Parameter Info.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignatureHelpData {
    pub signatures: Vec<SignatureInfo>,
    pub active_signature: usize,
    pub active_parameter: Option<usize>,
}

/// Parameter Info as shown, for commands and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureSnapshot {
    pub id: u64,
    pub loading: bool,
    pub visible: bool,
    pub data: Option<SignatureHelpData>,
    /// The overload shown and the parameter highlighted in it.
    pub active_signature: usize,
    pub active_parameter: Option<usize>,
}

/// Quick Info as shown, for commands and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverSnapshot {
    pub id: u64,
    pub loading: bool,
    pub visible: bool,
    /// The position described.
    pub offset: usize,
    /// The tooltip as plain text.
    pub text: Option<String>,
}

/// Identifier characters of the C-family and Rust grammars (letters, digits, `_`).
pub fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Start of the identifier that ends at `offset` in `text_before` (the text up to `offset`).
pub fn word_start_in(text_before: &str, offset: usize) -> usize {
    let mut start = offset;
    for (i, c) in text_before.char_indices().rev() {
        if !is_identifier_char(c) {
            break;
        }
        start = i;
    }
    start.min(offset)
}

/// Snippet syntax reduced to plain text for this brief (no tab stops): `$1`, `${1}`, `$0` and `$name` vanish,
/// `${1:text}` and `${1|a,b|}` become their text (`a` for a choice), `\$`, `\}` and `\\` are unescaped.
pub fn snippet_to_plain(snippet: &str) -> String {
    let mut out = String::with_capacity(snippet.len());
    let chars: Vec<char> = snippet.chars().collect();
    let mut i = 0;
    // Depth of `${` groups whose closing `}` is dropped.
    let mut open = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && matches!(chars[i + 1], '$' | '}' | '\\' | ',' | '|') => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '$' if i + 1 < chars.len() && chars[i + 1] == '{' => {
                // `${N`, `${N:`, `${N|a,b|}` or `${name:`.
                let mut j = i + 2;
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    j += 1;
                }
                match chars.get(j) {
                    Some(':') => {
                        open += 1;
                        i = j + 1;
                    }
                    Some('|') => {
                        let end = chars[j + 1..]
                            .iter()
                            .position(|&c| c == '|')
                            .map(|p| j + 1 + p);
                        let choices: String = match end {
                            Some(e) => chars[j + 1..e].iter().collect(),
                            None => String::new(),
                        };
                        out.push_str(choices.split(',').next().unwrap_or_default());
                        i = end.map_or(chars.len(), |e| e + 2);
                    }
                    Some('}') => i = j + 1,
                    _ => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            '$' if i + 1 < chars.len()
                && (chars[i + 1].is_alphanumeric() || chars[i + 1] == '_') =>
            {
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
            }
            '}' if open > 0 => {
                open -= 1;
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The unmatched `(` before the end of `text_before` and the number of top-level commas after it: the call the caret
/// is in and its argument index. Looks back at most 8 KB; stops at `;`, `{` or `}` outside parentheses.
pub fn enclosing_call(text_before: &str) -> Option<(usize, usize)> {
    let floor = text_before.len().saturating_sub(8192);
    let mut depth = 0usize;
    let mut commas = 0usize;
    for (i, c) in text_before.char_indices().rev() {
        if i < floor {
            break;
        }
        match c {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            '(' => return Some((i, commas)),
            '[' => return None,
            ',' if depth == 0 => commas += 1,
            ';' | '{' | '}' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// Distinct identifiers in `text`, parsed with `language`'s grammar: every leaf node whose kind ends in
/// `identifier`, which is how tree-sitter grammars name them (`identifier`, `type_identifier`, `field_identifier`).
/// Sorted. Run it off the UI thread; the tree is dropped before returning.
pub fn identifiers(language: &Language, text: &str) -> Vec<String> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(language.grammar()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        if node.child_count() == 0 && node.kind().ends_with("identifier") {
            let name = &text[node.byte_range()];
            if !name.is_empty() && name.len() <= 128 {
                seen.insert(name);
            }
        }
        if cursor.goto_first_child() || cursor.goto_next_sibling() {
            continue;
        }
        loop {
            if !cursor.goto_parent() {
                break 'walk;
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
    seen.into_iter().map(str::to_owned).collect()
}

/// Completion items for the fallback list: the identifiers except `typing` itself.
pub fn identifier_items(names: &[String], typing: &str) -> Vec<CompletionItem> {
    names
        .iter()
        .filter(|n| n.as_str() != typing)
        .map(|n| CompletionItem {
            label: n.clone(),
            kind: CompletionKind::Text,
            resolved: true,
            ..Default::default()
        })
        .collect()
}

/// The list's candidates for the fuzzy matcher, in empty-filter order (by sort text, then label). Returns the
/// candidates and, for each, its item index.
pub(crate) fn candidates(items: &[CompletionItem]) -> Vec<StringMatchCandidate> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| {
        let key = |i: usize| items[i].sort_text.as_deref().unwrap_or(&items[i].label);
        key(a)
            .cmp(key(b))
            .then_with(|| items[a].label.cmp(&items[b].label))
    });
    order
        .into_iter()
        .map(|i| StringMatchCandidate::new(i, items[i].filter_string()))
        .collect()
}

/// One row of the filtered list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListMatch {
    pub item: usize,
    /// Byte ranges of the label to highlight.
    pub highlights: Vec<Range<usize>>,
}

/// Order fuzzy matches for display: items whose filter text starts with the query (ignoring case) first, then by
/// score, then in empty-filter order. With an empty query, every candidate in order.
pub(crate) fn rank(
    items: &[CompletionItem],
    candidates: &[StringMatchCandidate],
    query: &str,
    mut matches: Vec<StringMatch>,
) -> Vec<ListMatch> {
    if query.is_empty() {
        return candidates
            .iter()
            .map(|c| ListMatch {
                item: c.id,
                highlights: Vec::new(),
            })
            .collect();
    }
    // Position in empty-filter order, for ties.
    let mut position = vec![0usize; items.len()];
    for (pos, c) in candidates.iter().enumerate() {
        position[c.id] = pos;
    }
    let lower = query.to_lowercase();
    let prefix = |m: &StringMatch| m.string.to_lowercase().starts_with(&lower);
    matches.sort_by(|a, b| {
        prefix(b)
            .cmp(&prefix(a))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| position[a.candidate_id].cmp(&position[b.candidate_id]))
    });
    matches
        .into_iter()
        .map(|m| {
            let item = &items[m.candidate_id];
            let highlights = if item.filter_text.as_deref().is_none_or(|f| f == item.label) {
                m.ranges().collect()
            } else {
                Vec::new()
            };
            ListMatch {
                item: m.candidate_id,
                highlights,
            }
        })
        .collect()
}

/// Parsed Markdown and its plain text, for tooltips.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Rendered {
    pub blocks: Arc<Vec<Block>>,
    pub plain: String,
}

impl Rendered {
    pub fn new(markdown_text: &str) -> Self {
        let blocks = markdown::parse(markdown_text);
        let plain = markdown::plain_text(&blocks);
        Self {
            blocks: Arc::new(blocks),
            plain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lens_window_and_pipeline() {
        assert_eq!(code_lens_window(100..140, 1000), 50..190);
        assert_eq!(code_lens_window(10..40, 60), 0..60);
        let mut p = CodeLensPipeline::default();
        let v1 = clock::Global::new();
        assert_eq!(p.request(&v1), None, "off asks for nothing");
        assert!(p.set_enabled(true));
        assert!(!p.set_enabled(true));
        let a = p.request(&v1).unwrap();
        assert!(p.accept(a, &v1));
        assert!(!p.accept(a, &v1), "an answer is applied once");
        assert_eq!(p.debounced(&v1), None, "no edit, no request");
        p.edited();
        let b = p.debounced(&v1).unwrap();
        let mut clock = clock::Lamport::new(clock::ReplicaId::new(1));
        let v2: clock::Global = [clock.tick()].into_iter().collect();
        assert!(!p.accept(b, &v2), "the text changed since the request");
        let rows = [(1, 5, false), (2, 60, false), (3, 7, true)];
        assert_eq!(p.to_resolve(rows, 0..50), [1]);
        assert!(p.to_resolve(rows, 0..50).is_empty());
        p.retry(1);
        assert_eq!(p.to_resolve(rows, 0..100), [1, 2]);
        assert!(p.set_enabled(false));
        assert!(p.to_resolve(rows, 0..100).is_empty());
    }

    #[test]
    fn word_starts() {
        assert_eq!(word_start_in("foo.Bar", 7), 4);
        assert_eq!(word_start_in("foo.", 4), 4);
        assert_eq!(word_start_in("x _ab1", 6), 2);
        assert_eq!(word_start_in("", 0), 0);
        assert_eq!(word_start_in("é", 2), 0);
    }

    #[test]
    fn snippets_become_plain_text() {
        assert_eq!(snippet_to_plain("Foo($1)$0"), "Foo()");
        assert_eq!(
            snippet_to_plain("for (${1:int} i = 0; i < ${2:length}; i++)"),
            "for (int i = 0; i < length; i++)"
        );
        assert_eq!(snippet_to_plain("${1|public,private|} void"), "public void");
        assert_eq!(snippet_to_plain("cost \\$5 ${1:a {b\\}}"), "cost $5 a {b}");
        assert_eq!(snippet_to_plain("${TM_SELECTED_TEXT}x$name"), "x");
        assert_eq!(snippet_to_plain("plain {}"), "plain {}");
    }

    #[test]
    fn enclosing_calls() {
        assert_eq!(enclosing_call("Foo("), Some((3, 0)));
        assert_eq!(enclosing_call("Foo(a, b"), Some((3, 1)));
        assert_eq!(enclosing_call("Foo(a, Bar(x, y), "), Some((3, 2)));
        assert_eq!(enclosing_call("Foo(a, Bar(x, y"), Some((10, 1)));
        assert_eq!(enclosing_call("Foo(a);"), None);
        assert_eq!(enclosing_call("{ x"), None);
        assert_eq!(enclosing_call("M(arr[1, 2], "), Some((1, 1)));
    }

    #[test]
    fn ranking_prefers_prefixes_then_score() {
        let items: Vec<CompletionItem> = ["WriteLine", "Write", "ReadLine", "OverwriteLine"]
            .iter()
            .map(|l| CompletionItem {
                label: (*l).into(),
                ..Default::default()
            })
            .collect();
        let candidates = candidates(&items);
        // Empty filter: sort text (the label here) order.
        let all = rank(&items, &candidates, "", Vec::new());
        let labels = |m: &[ListMatch]| {
            m.iter()
                .map(|m| items[m.item].label.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            labels(&all),
            ["OverwriteLine", "ReadLine", "Write", "WriteLine"]
        );
        let fake = |id: usize, score: f64| StringMatch {
            candidate_id: id,
            score,
            positions: vec![0],
            string: items[id].label.clone(),
        };
        let ranked = rank(
            &items,
            &candidates,
            "w",
            vec![fake(3, 0.9), fake(0, 0.5), fake(1, 0.7)],
        );
        assert_eq!(labels(&ranked), ["Write", "WriteLine", "OverwriteLine"]);
        assert_eq!(ranked[0].highlights, vec![0..1]);
    }
}
