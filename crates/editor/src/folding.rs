//! Outlining (brief 0067): Visual Studio's collapsible regions.
//!
//! - **Ranges.** A fold is a row range `start..=end` with a [`FoldKind`]. The owner sets the language server's
//!   `textDocument/foldingRange` answer with [`EditorView::set_fold_ranges`] (asked for with
//!   [`DisplayEvent::FoldRangesRequested`] when the owner turns server folds on and 300 ms after the last edit); until
//!   then, and for languages without a server, the ranges come from the language's `folds.scm` query run on the
//!   [`SyntaxThread`] plus the `#region` markers found in the text ([`syntax_fold_ranges`]). Server ranges borrow
//!   their kind (namespace, type, member) from a syntax fold with the same header row, so Collapse to Definitions and
//!   sticky scroll work the same either way.
//! - **State.** Folds are anchored, so they follow edits; a new set keeps the collapsed state of folds with the same
//!   header row. Collapsing hides rows `start + 1..=end` in the display map; the header row stays and shows the
//!   adornment (`...`, or the region's name) after its text. The caret, a selection head, an edit or the execution
//!   point inside hidden rows expands the folds hiding it ([`EditorView::reveal_hidden_carets`]).
//! - **Commands.** Edit > Outlining's operations are methods here, run by the shell's `eludite.editor.*` commands.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{Context, Task};
use text::{Anchor, BufferSnapshot};

use crate::syntax::{Language, SyntaxThread};
use crate::view::EditorView;

/// How long after the last edit fold ranges are recomputed (and asked of the server).
pub const FOLD_DEBOUNCE: Duration = Duration::from_millis(300);

/// Lines of hidden text the collapsed adornment's tooltip shows.
pub const TOOLTIP_LINES: usize = 25;

/// What a fold covers; decides what Collapse to Definitions collapses and what sticky scroll pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FoldKind {
    /// `#region` ... `#endregion` (and the other languages' markers).
    Region,
    /// A multi-line comment or a run of line comments (documentation comments included).
    Comment,
    /// A run of `using`, `Imports`, `open`, `use` or `import` lines.
    Imports,
    Namespace,
    /// A class, struct, interface, enum, record, module, trait or impl.
    Type,
    /// A method, constructor, property, accessor, function or other member.
    Member,
    /// Any other block (statements, objects, arrays, elements, rules).
    Block,
    /// A region the person made with Hide Selection (Ctrl+M, Ctrl+H).
    Manual,
}

impl FoldKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FoldKind::Region => "region",
            FoldKind::Comment => "comment",
            FoldKind::Imports => "imports",
            FoldKind::Namespace => "namespace",
            FoldKind::Type => "type",
            FoldKind::Member => "member",
            FoldKind::Block => "block",
            FoldKind::Manual => "manual",
        }
    }

    /// The kind of a fold capture name (`fold`, `fold.member`, ...).
    pub fn from_capture(name: &str) -> Option<Self> {
        Some(match name {
            "fold" | "fold.block" => FoldKind::Block,
            "fold.region" => FoldKind::Region,
            "fold.comment" => FoldKind::Comment,
            "fold.imports" => FoldKind::Imports,
            "fold.namespace" => FoldKind::Namespace,
            "fold.type" => FoldKind::Type,
            "fold.member" => FoldKind::Member,
            _ => return None,
        })
    }

    /// The kind of an LSP `FoldingRangeKind` (`comment`, `imports`, `region`; anything else is a block).
    pub fn from_lsp(kind: Option<&str>) -> Self {
        match kind {
            Some("comment") => FoldKind::Comment,
            Some("imports") => FoldKind::Imports,
            Some("region") => FoldKind::Region,
            _ => FoldKind::Block,
        }
    }

    /// Whether sticky scroll pins this kind's header (a scope, not a comment, imports or region).
    pub fn is_scope(self) -> bool {
        matches!(
            self,
            FoldKind::Namespace | FoldKind::Type | FoldKind::Member | FoldKind::Block
        )
    }
}

/// One fold range as a source gives it: 0-based rows, `end_row > start_row`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldRange {
    pub start_row: u32,
    pub end_row: u32,
    pub kind: FoldKind,
    /// What the collapsed adornment reads (a region's name, the server's `collapsedText`); `None` is `...`.
    pub label: Option<String>,
}

impl FoldRange {
    pub fn new(start_row: u32, end_row: u32, kind: FoldKind) -> Self {
        Self {
            start_row,
            end_row,
            kind,
            label: None,
        }
    }
}

/// Where the shown fold ranges came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldSource {
    /// The language's tree-sitter fold query and the `#region` markers.
    Syntax,
    /// The language server's `textDocument/foldingRange`.
    Server,
}

impl FoldSource {
    pub fn as_str(self) -> &'static str {
        match self {
            FoldSource::Syntax => "syntax",
            FoldSource::Server => "server",
        }
    }
}

/// Events for the owner about the display (brief 0067), beside [`crate::EditorEvent`].
#[derive(Debug, Clone, PartialEq)]
pub enum DisplayEvent {
    /// The text changed (or server folds were turned on): ask the server for `textDocument/foldingRange` and answer
    /// with [`EditorView::set_fold_ranges`] quoting `id`.
    FoldRangesRequested { id: u64 },
    /// The zoom level changed (Ctrl+wheel, Zoom In, Zoom Out), in percent.
    ZoomChanged { level: u32 },
}

/// A fold as shown, resolved to rows for the current text.
#[derive(Debug, Clone, PartialEq)]
pub struct Fold {
    pub start_row: u32,
    pub end_row: u32,
    pub kind: FoldKind,
    pub collapsed: bool,
    pub label: Option<String>,
}

impl Fold {
    /// The rows a collapsed fold hides.
    pub fn hidden_rows(&self) -> Range<u32> {
        self.start_row + 1..self.end_row + 1
    }

    /// What the adornment reads.
    pub fn adornment(&self) -> &str {
        self.label.as_deref().unwrap_or("...")
    }
}

#[derive(Debug, Clone)]
struct AnchoredFold {
    /// The start of the header row.
    start: Anchor,
    /// The end of the last row.
    end: Anchor,
    kind: FoldKind,
    collapsed: bool,
    label: Option<String>,
}

/// The view's outlining state.
pub(crate) struct Folds {
    /// Outlining on (Stop Outlining turns it off; Start Automatic Outlining back on).
    pub enabled: bool,
    folds: Vec<AnchoredFold>,
    /// Hide Selection's regions: always collapsed until Stop Hiding Current.
    manual: Vec<AnchoredFold>,
    pub source: FoldSource,
    /// The owner answers `textDocument/foldingRange` (set with [`EditorView::set_server_folds`]).
    pub server: bool,
    /// The last syntax ranges (kinds for server ranges, scopes for sticky scroll).
    pub syntax_ranges: Vec<FoldRange>,
    /// Collapse `#region`s when the first set of folds arrives.
    pub collapse_regions_on_open: bool,
    first_set_applied: bool,
    request_id: u64,
    pending: Option<u64>,
    debounce: Option<Task<()>>,
    syntax_task: Option<Task<()>>,
    /// The version the hidden rows were last checked against edits.
    revealed_at: Option<clock::Global>,
    /// One more on every change of the shown folds or their collapsed state.
    pub epoch: u64,
    cache: RefCell<Option<(clock::Global, u64, Arc<Vec<Fold>>)>>,
}

impl Default for Folds {
    fn default() -> Self {
        Self {
            enabled: true,
            folds: Vec::new(),
            manual: Vec::new(),
            source: FoldSource::Syntax,
            server: false,
            syntax_ranges: Vec::new(),
            collapse_regions_on_open: false,
            first_set_applied: false,
            request_id: 0,
            pending: None,
            debounce: None,
            syntax_task: None,
            revealed_at: None,
            epoch: 0,
            cache: RefCell::new(None),
        }
    }
}

/// The folds sorted by header row, outermost first, deduplicated, at least two rows each.
pub fn normalize(mut ranges: Vec<FoldRange>) -> Vec<FoldRange> {
    ranges.retain(|r| r.end_row > r.start_row);
    ranges.sort_by(|a, b| {
        a.start_row
            .cmp(&b.start_row)
            .then(b.end_row.cmp(&a.end_row))
    });
    ranges.dedup_by(|b, a| a.start_row == b.start_row && a.end_row == b.end_row);
    // Two folds with one header row: the outer one wins (one expander per line, as Visual Studio draws it).
    ranges.dedup_by(|b, a| a.start_row == b.start_row);
    ranges
}

// ----- tree-sitter fold queries -----

/// The fold query of a language id (empty for none).
pub fn folds_query(language_id: &str) -> &'static str {
    match language_id {
        "csharp" => include_str!("../queries/csharp/folds.scm"),
        "rust" => include_str!("../queries/rust/folds.scm"),
        "typescript" | "tsx" => concat!(
            include_str!("../queries/typescript/folds.scm"),
            include_str!("../queries/javascript/folds.scm")
        ),
        "javascript" => include_str!("../queries/javascript/folds.scm"),
        "html" => include_str!("../queries/html/folds.scm"),
        "razor" => include_str!("../queries/razor/folds.scm"),
        "css" => include_str!("../queries/css/folds.scm"),
        "json" | "jsonc" => include_str!("../queries/json/folds.scm"),
        "vb" => include_str!("../queries/vb/folds.scm"),
        "fsharp" | "fsharp-signature" => include_str!("../queries/fsharp/folds.scm"),
        _ => "",
    }
}

/// The `#region` markers of a language: (start, end), matched case-insensitively at the start of a trimmed line.
fn region_markers(language_id: &str) -> &'static [(&'static str, &'static str)] {
    match language_id {
        "csharp" | "razor" | "fsharp" | "fsharp-signature" => &[("#region", "#endregion")],
        "vb" => &[("#region", "#end region")],
        "typescript" | "tsx" | "javascript" => &[
            ("//#region", "//#endregion"),
            ("// #region", "// #endregion"),
        ],
        "rust" => &[("// region:", "// endregion"), ("//region:", "//endregion")],
        "css" => &[
            ("/* #region", "/* #endregion"),
            ("/*#region", "/*#endregion"),
        ],
        "html" => &[
            ("<!-- #region", "<!-- #endregion"),
            ("<!--#region", "<!--#endregion"),
        ],
        _ => &[],
    }
}

/// The region folds of `text` by the language's markers, nested; an unmatched marker makes none.
pub fn region_folds(language_id: &str, text: &str) -> Vec<FoldRange> {
    let markers = region_markers(language_id);
    if markers.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut open: Vec<(u32, Option<String>)> = Vec::new();
    for (row, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with(['#', '/', '<']) {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        for (start, end) in markers {
            if lower.starts_with(end) {
                if let Some((s, label)) = open.pop() {
                    let mut f = FoldRange::new(s, row as u32, FoldKind::Region);
                    f.label = label;
                    out.push(f);
                }
                break;
            }
            if lower.starts_with(start) {
                let name = trimmed[start.len()..]
                    .trim()
                    .trim_end_matches("-->")
                    .trim_end_matches("*/")
                    .trim();
                // Visual Studio shows the region's name; a nameless region shows its marker line.
                let label = if name.is_empty() {
                    trimmed.trim_end().to_owned()
                } else {
                    name.to_owned()
                };
                open.push((row as u32, Some(label)));
                break;
            }
        }
    }
    out
}

fn compiled_query(language: &Language) -> Option<Arc<tree_sitter::Query>> {
    static QUERIES: OnceLock<Mutex<HashMap<&'static str, Option<Arc<tree_sitter::Query>>>>> =
        OnceLock::new();
    let source = folds_query(language.id());
    if source.is_empty() {
        return None;
    }
    let mut map = QUERIES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.entry(language.id())
        .or_insert_with(
            || match tree_sitter::Query::new(language.grammar(), source) {
                Ok(q) => Some(Arc::new(q)),
                Err(e) => {
                    eprintln!("eludite-editor: {} folds query: {e}", language.id());
                    None
                }
            },
        )
        .clone()
}

/// Whether `language` has a fold query that compiles (for tests).
pub fn has_fold_query(language: &Language) -> bool {
    compiled_query(language).is_some()
}

/// The fold ranges of `text` in `language`: its fold query's captures (consecutive one-line comments and imports
/// merged; a node with a `name` field folds from the name's row) plus its `#region` markers. Parses the text, so run
/// it off the UI thread.
pub fn syntax_fold_ranges(language: &Language, text: &str) -> Vec<FoldRange> {
    use tree_sitter::StreamingIterator as _;
    let mut out = region_folds(language.id(), text);
    let Some(query) = compiled_query(language) else {
        return normalize(out);
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(language.grammar()).is_err() {
        return normalize(out);
    }
    let Some(tree) = parser.parse(text, None) else {
        return normalize(out);
    };
    let kinds: Vec<Option<FoldKind>> = query
        .capture_names()
        .iter()
        .map(|n| FoldKind::from_capture(n))
        .collect();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), text.as_bytes());
    // Runs of one-line comments and of imports, merged as they come (captures arrive in document order).
    let mut run: Option<(FoldKind, u32, u32)> = None;
    let flush = |run: &mut Option<(FoldKind, u32, u32)>, out: &mut Vec<FoldRange>| {
        if let Some((kind, s, e)) = run.take()
            && e > s
        {
            out.push(FoldRange::new(s, e, kind));
        }
    };
    while let Some(m) = matches.next() {
        for c in m.captures() {
            let Some(kind) = kinds.get(c.index as usize).copied().flatten() else {
                continue;
            };
            let node = c.node;
            let mut start = node.start_position().row as u32;
            let mut end = node.end_position().row as u32;
            // A node ending at column 0 ends on the previous row (a trailing newline belongs to it).
            if node.end_position().column == 0 && end > start {
                end -= 1;
            }
            if matches!(kind, FoldKind::Comment | FoldKind::Imports) {
                // A comment after code on its line is not a fold of its own, nor part of a run.
                let line_start = text[..node.start_byte()].rfind('\n').map_or(0, |i| i + 1);
                if !text[line_start..node.start_byte()].trim().is_empty() {
                    continue;
                }
                match &mut run {
                    Some((k, _, e)) if *k == kind && start <= *e + 1 => {
                        *e = (*e).max(end);
                        continue;
                    }
                    _ => {
                        flush(&mut run, &mut out);
                        run = Some((kind, start, end));
                        continue;
                    }
                }
            }
            if let Some(name) = node.child_by_field_name("name") {
                start = start.max(name.start_position().row as u32);
            }
            if end > start {
                out.push(FoldRange::new(start, end, kind));
            }
        }
    }
    flush(&mut run, &mut out);
    normalize(out)
}

// ----- the view's outlining -----

impl Folds {
    fn resolve(&self, snapshot: &BufferSnapshot) -> Arc<Vec<Fold>> {
        if let Some((v, e, f)) = self.cache.borrow().as_ref()
            && v == snapshot.version()
            && *e == self.epoch
        {
            return f.clone();
        }
        let row = |a: &Anchor| snapshot.offset_to_point(snapshot.offset_for_anchor(a)).row;
        let mut folds: Vec<Fold> = self
            .folds
            .iter()
            .chain(self.manual.iter())
            .map(|f| Fold {
                start_row: row(&f.start),
                end_row: row(&f.end),
                kind: f.kind,
                collapsed: f.collapsed,
                label: f.label.clone(),
            })
            .filter(|f| f.end_row > f.start_row)
            .collect();
        folds.sort_by(|a, b| {
            a.start_row
                .cmp(&b.start_row)
                .then(b.end_row.cmp(&a.end_row))
        });
        let folds = Arc::new(folds);
        *self.cache.borrow_mut() = Some((snapshot.version().clone(), self.epoch, folds.clone()));
        folds
    }

    fn anchored(snapshot: &BufferSnapshot, r: &FoldRange, collapsed: bool) -> AnchoredFold {
        let max = snapshot.max_point().row;
        let (s, e) = (r.start_row.min(max), r.end_row.min(max));
        let end_col = snapshot.line_len(e);
        AnchoredFold {
            start: snapshot.anchor_before(snapshot.point_to_offset(text::Point::new(s, 0))),
            end: snapshot.anchor_after(snapshot.point_to_offset(text::Point::new(e, end_col))),
            kind: r.kind,
            collapsed,
            label: r.label.clone(),
        }
    }

    /// Replace the folds with `ranges`, keeping the collapsed state of those whose header row was collapsed.
    fn replace(&mut self, snapshot: &BufferSnapshot, ranges: Vec<FoldRange>, source: FoldSource) {
        let old = self.resolve(snapshot);
        let first = !self.first_set_applied && !ranges.is_empty();
        let regions = first && self.collapse_regions_on_open;
        self.folds = normalize(ranges)
            .iter()
            .map(|r| {
                let was = old
                    .iter()
                    .find(|f| f.start_row == r.start_row && f.kind != FoldKind::Manual)
                    .is_some_and(|f| f.collapsed);
                Self::anchored(snapshot, r, was || (regions && r.kind == FoldKind::Region))
            })
            .collect();
        if first {
            self.first_set_applied = true;
        }
        self.source = source;
        self.epoch += 1;
    }

    fn set_collapsed_where(
        &mut self,
        snapshot: &BufferSnapshot,
        mut f: impl FnMut(&Fold) -> Option<bool>,
    ) -> bool {
        let resolved = self.resolve(snapshot);
        let mut changed = false;
        let row = |a: &Anchor| snapshot.offset_to_point(snapshot.offset_for_anchor(a)).row;
        for af in self.folds.iter_mut() {
            let (s, e) = (row(&af.start), row(&af.end));
            let Some(fold) = resolved
                .iter()
                .find(|x| x.start_row == s && x.end_row == e && x.kind == af.kind)
            else {
                continue;
            };
            if let Some(c) = f(fold)
                && c != af.collapsed
            {
                af.collapsed = c;
                changed = true;
            }
        }
        if changed {
            self.epoch += 1;
        }
        changed
    }
}

impl gpui::EventEmitter<DisplayEvent> for EditorView {}

impl EditorView {
    /// The folds as shown (empty while outlining is off), sorted by header row, outermost first.
    pub fn folds(&self) -> Arc<Vec<Fold>> {
        let d = &self.display.folds;
        if !d.enabled {
            return Arc::new(Vec::new());
        }
        d.resolve(self.editor.buffer().snapshot())
    }

    /// Where the shown folds came from.
    pub fn fold_source(&self) -> FoldSource {
        self.display.folds.source
    }

    /// Whether outlining is on.
    pub fn outlining_enabled(&self) -> bool {
        self.display.folds.enabled
    }

    /// The hidden row ranges of the collapsed folds (input to the display map).
    pub(crate) fn hidden_rows(&self) -> Vec<Range<u32>> {
        self.folds()
            .iter()
            .filter(|f| f.collapsed)
            .map(Fold::hidden_rows)
            .collect()
    }

    /// The syntax fold ranges last computed (scopes for sticky scroll).
    pub fn syntax_fold_ranges(&self) -> &[FoldRange] {
        &self.display.folds.syntax_ranges
    }

    /// Collapse `#region`s when the first set of folds arrives (`editor.outlining.collapseRegionsOnOpen`).
    pub fn set_collapse_regions_on_open(&mut self, on: bool) {
        self.display.folds.collapse_regions_on_open = on;
    }

    /// Say that the owner answers `textDocument/foldingRange` for this document: a [`DisplayEvent::FoldRangesRequested`]
    /// follows at once and after each edit; the syntax folds show until the first answer. Off returns to the syntax
    /// folds.
    pub fn set_server_folds(&mut self, on: bool, cx: &mut Context<Self>) {
        let d = &mut self.display.folds;
        if d.server == on {
            return;
        }
        d.server = on;
        if on {
            self.request_fold_ranges(cx);
        } else {
            d.pending = None;
            let ranges = d.syntax_ranges.clone();
            let snapshot = self.editor.buffer().snapshot().clone();
            self.display
                .folds
                .replace(&snapshot, ranges, FoldSource::Syntax);
            self.display_changed(cx);
        }
    }

    fn request_fold_ranges(&mut self, cx: &mut Context<Self>) {
        let d = &mut self.display.folds;
        d.request_id += 1;
        d.pending = Some(d.request_id);
        cx.emit(DisplayEvent::FoldRangesRequested { id: d.request_id });
    }

    /// The server's fold ranges for request `id` (dropped, returning false, unless it is the newest request). Kinds the
    /// server leaves out (members, types, namespaces) are taken from syntax folds with the same header row.
    pub fn set_fold_ranges(
        &mut self,
        id: u64,
        ranges: Vec<FoldRange>,
        cx: &mut Context<Self>,
    ) -> bool {
        let d = &mut self.display.folds;
        if !d.server || d.pending != Some(id) {
            return false;
        }
        d.pending = None;
        let ranges = ranges
            .into_iter()
            .map(|mut r| {
                if r.kind == FoldKind::Block
                    && let Some(s) = d.syntax_ranges.iter().find(|s| s.start_row == r.start_row)
                {
                    r.kind = s.kind;
                }
                r
            })
            .collect();
        let snapshot = self.editor.buffer().snapshot().clone();
        self.display
            .folds
            .replace(&snapshot, ranges, FoldSource::Server);
        self.display_changed(cx);
        true
    }

    /// Compute the syntax folds now (on the syntax thread); the result arrives with the next turn of the executor.
    pub fn refresh_syntax_folds(&mut self, cx: &mut Context<Self>) {
        let Some(language) = self.syntax_language() else {
            return;
        };
        let snapshot = self.editor.buffer().snapshot().clone();
        let version = snapshot.version().clone();
        let job = SyntaxThread::global().run(move || {
            let text = snapshot.text();
            syntax_fold_ranges(&language, &text)
        });
        self.display.folds.syntax_task = Some(cx.spawn(async move |this, cx| {
            let Some(ranges) = job.await else {
                return;
            };
            this.update(cx, |v, cx| {
                v.display.folds.syntax_task = None;
                let current = v.editor.buffer().snapshot().clone();
                // Rows from an older version are still close; the next refresh corrects them.
                let stale = current.version() != &version;
                v.display.folds.syntax_ranges = ranges.clone();
                if !v.display.folds.server || v.display.folds.source == FoldSource::Syntax {
                    v.display
                        .folds
                        .replace(&current, ranges, FoldSource::Syntax);
                    v.display_changed(cx);
                }
                if stale {
                    v.folds_edited(cx);
                }
            })
            .ok();
        }));
    }

    /// The text changed: recompute the syntax folds and ask for the server's after [`FOLD_DEBOUNCE`].
    pub(crate) fn folds_edited(&mut self, cx: &mut Context<Self>) {
        self.reveal_hidden_carets(cx);
        self.display.folds.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FOLD_DEBOUNCE).await;
            this.update(cx, |v, cx| {
                v.display.folds.debounce = None;
                v.refresh_syntax_folds(cx);
                if v.display.folds.server {
                    v.request_fold_ranges(cx);
                }
            })
            .ok();
        }));
    }

    /// Expand every collapsed fold hiding a selection head, the execution point or text edited since the last check.
    /// True when something expanded.
    pub fn reveal_hidden_carets(&mut self, cx: &mut Context<Self>) -> bool {
        let snapshot = self.editor.buffer().snapshot().clone();
        let mut rows: Vec<u32> = self
            .editor
            .selections()
            .iter()
            .map(|s| snapshot.offset_to_point(s.head).row)
            .collect();
        if let Some((r, _)) = &self.execution {
            rows.push(
                snapshot
                    .offset_to_point(snapshot.offset_for_anchor(&r.start))
                    .row,
            );
        }
        let mut edited: Vec<Range<u32>> = Vec::new();
        if let Some(since) = self.display.folds.revealed_at.as_ref()
            && since != snapshot.version()
        {
            edited = snapshot
                .edits_since::<text::Point>(since)
                .map(|e| e.new.start.row..e.new.end.row + 1)
                .collect();
        }
        self.display.folds.revealed_at = Some(snapshot.version().clone());
        let hit = |f: &Fold| {
            let h = f.hidden_rows();
            rows.iter().any(|r| h.contains(r))
                || edited.iter().any(|e| e.start < h.end && h.start < e.end)
        };
        let mut changed = self
            .display
            .folds
            .set_collapsed_where(&snapshot, |f| (f.collapsed && hit(f)).then_some(false));
        // Hide Selection's regions go when revealed (Visual Studio drops them on expansion).
        let before = self.display.folds.manual.len();
        let row = |a: &Anchor| snapshot.offset_to_point(snapshot.offset_for_anchor(a)).row;
        self.display.folds.manual.retain(|m| {
            let f = Fold {
                start_row: row(&m.start),
                end_row: row(&m.end),
                kind: FoldKind::Manual,
                collapsed: true,
                label: None,
            };
            !hit(&f)
        });
        if self.display.folds.manual.len() != before {
            self.display.folds.epoch += 1;
            changed = true;
        }
        if changed {
            self.display_changed(cx);
        }
        changed
    }

    fn caret_row(&self) -> u32 {
        self.editor.primary_head().row
    }

    /// The innermost fold whose rows hold `row`.
    fn innermost_at(&self, row: u32) -> Option<Fold> {
        self.folds()
            .iter()
            .filter(|f| f.start_row <= row && row <= f.end_row)
            .min_by_key(|f| f.end_row - f.start_row)
            .cloned()
    }

    fn set_fold(&mut self, target: &Fold, collapsed: bool, cx: &mut Context<Self>) -> bool {
        let snapshot = self.editor.buffer().snapshot().clone();
        if target.kind == FoldKind::Manual {
            return false;
        }
        let changed = self.display.folds.set_collapsed_where(&snapshot, |f| {
            (f.start_row == target.start_row && f.end_row == target.end_row).then_some(collapsed)
        });
        if changed {
            if collapsed {
                self.move_carets_out_of_hidden_rows();
            }
            self.display_changed(cx);
        }
        changed
    }

    /// A caret inside rows about to hide moves to its fold's header (Visual Studio keeps the caret visible).
    fn move_carets_out_of_hidden_rows(&mut self) {
        let folds = self.folds();
        let snapshot = self.editor.buffer().snapshot().clone();
        let header_end =
            |row: u32| snapshot.point_to_offset(text::Point::new(row, snapshot.line_len(row)));
        let mut moved = false;
        let ranges: Vec<crate::SelectionRange> = self
            .editor
            .selections()
            .into_iter()
            .map(|mut s| {
                let row = snapshot.offset_to_point(s.head).row;
                if let Some(f) = folds
                    .iter()
                    .filter(|f| f.collapsed && f.hidden_rows().contains(&row))
                    .min_by_key(|f| f.start_row)
                {
                    s.head = header_end(f.start_row);
                    s.tail = s.head;
                    moved = true;
                }
                s
            })
            .collect();
        if moved {
            let primary = self.primary_index();
            self.editor.set_selections(ranges, primary);
        }
        // The caret is outside hidden text now: do not reveal what was just collapsed.
        self.display.folds.revealed_at = Some(snapshot.version().clone());
    }

    pub(crate) fn primary_index(&self) -> usize {
        let p = self.editor.primary_selection();
        self.editor
            .selections()
            .iter()
            .position(|s| *s == p)
            .unwrap_or(0)
    }

    /// Toggle Outlining Expansion (Ctrl+M, Ctrl+M): the innermost fold at `row` (the caret's when `None`).
    pub fn toggle_outlining_expansion(&mut self, row: Option<u32>, cx: &mut Context<Self>) -> bool {
        let row = row.unwrap_or_else(|| self.caret_row());
        // A collapsed fold's header toggles that fold (the caret on it sees the adornment), else the innermost.
        let target = self
            .folds()
            .iter()
            .filter(|f| f.collapsed && f.start_row == row)
            .max_by_key(|f| f.end_row)
            .cloned()
            .or_else(|| self.innermost_at(row));
        match target {
            Some(f) => {
                let collapsed = !f.collapsed;
                self.set_fold(&f, collapsed, cx)
            }
            None => false,
        }
    }

    /// Toggle All Outlining (Ctrl+M, Ctrl+L): expand everything if anything is collapsed, else collapse everything.
    pub fn toggle_all_outlining(&mut self, cx: &mut Context<Self>) -> bool {
        let any = self.folds().iter().any(|f| f.collapsed);
        self.set_all(|_| Some(!any), cx)
    }

    fn set_all(&mut self, f: impl FnMut(&Fold) -> Option<bool>, cx: &mut Context<Self>) -> bool {
        if !self.display.folds.enabled {
            return false;
        }
        let snapshot = self.editor.buffer().snapshot().clone();
        let changed = self.display.folds.set_collapsed_where(&snapshot, f);
        if changed {
            self.move_carets_out_of_hidden_rows();
            self.display_changed(cx);
        }
        changed
    }

    /// Expand All Outlining (Ctrl+M, Ctrl+X).
    pub fn expand_all_outlining(&mut self, cx: &mut Context<Self>) -> bool {
        let manual = !self.display.folds.manual.is_empty();
        self.display.folds.manual.clear();
        if manual {
            self.display.folds.epoch += 1;
            self.display_changed(cx);
        }
        self.set_all(|_| Some(false), cx) || manual
    }

    /// Collapse All Outlining (Ctrl+M, Ctrl+A).
    pub fn collapse_all_outlining(&mut self, cx: &mut Context<Self>) -> bool {
        self.set_all(|_| Some(true), cx)
    }

    /// Collapse to Definitions (Ctrl+M, Ctrl+O): members and regions collapse, everything else expands.
    pub fn collapse_to_definitions(&mut self, cx: &mut Context<Self>) -> bool {
        self.set_all(
            |f| Some(matches!(f.kind, FoldKind::Member | FoldKind::Region)),
            cx,
        )
    }

    /// Collapse Current Region (Ctrl+M, Ctrl+S): the innermost expanded fold at the caret.
    pub fn collapse_current_region(&mut self, cx: &mut Context<Self>) -> bool {
        let row = self.caret_row();
        let target = self
            .folds()
            .iter()
            .filter(|f| !f.collapsed && f.start_row <= row && row <= f.end_row)
            .min_by_key(|f| f.end_row - f.start_row)
            .cloned();
        target.is_some_and(|f| self.set_fold(&f, true, cx))
    }

    /// Expand Current Region (Ctrl+M, Ctrl+E): the collapsed fold whose header holds the caret.
    pub fn expand_current_region(&mut self, cx: &mut Context<Self>) -> bool {
        let row = self.caret_row();
        let target = self
            .folds()
            .iter()
            .filter(|f| f.collapsed && f.start_row == row)
            .max_by_key(|f| f.end_row)
            .cloned();
        target.is_some_and(|f| self.set_fold(&f, false, cx))
    }

    /// Stop Outlining (Ctrl+M, Ctrl+P): every region expands and the margin goes until Start Automatic Outlining.
    pub fn stop_outlining(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.display.folds.enabled {
            return false;
        }
        let snapshot = self.editor.buffer().snapshot().clone();
        self.display
            .folds
            .set_collapsed_where(&snapshot, |_| Some(false));
        self.display.folds.manual.clear();
        self.display.folds.enabled = false;
        self.display.folds.epoch += 1;
        self.display_changed(cx);
        true
    }

    /// Start Automatic Outlining: outlining back on, every region expanded.
    pub fn start_automatic_outlining(&mut self, cx: &mut Context<Self>) -> bool {
        if self.display.folds.enabled {
            return false;
        }
        self.display.folds.enabled = true;
        self.display.folds.epoch += 1;
        self.refresh_syntax_folds(cx);
        self.display_changed(cx);
        true
    }

    /// Hide Selection (Ctrl+M, Ctrl+H): the selected rows collapse under the first one, until Stop Hiding Current or
    /// the caret enters them.
    pub fn hide_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let sel = self.editor.primary_selection();
        let snapshot = self.editor.buffer().snapshot().clone();
        let (s, e) = (
            snapshot.offset_to_point(sel.start()).row,
            snapshot.offset_to_point(sel.end()).row,
        );
        if !self.display.folds.enabled || e <= s {
            return false;
        }
        let manual = Folds::anchored(&snapshot, &FoldRange::new(s, e, FoldKind::Manual), true);
        self.display.folds.manual.push(manual);
        self.display.folds.epoch += 1;
        self.editor.set_caret(sel.start());
        self.move_carets_out_of_hidden_rows();
        self.display_changed(cx);
        true
    }

    /// Stop Hiding Current (Ctrl+M, Ctrl+U): remove the Hide Selection region at the caret, else the outlining of the
    /// innermost fold at the caret.
    pub fn stop_hiding_current(&mut self, cx: &mut Context<Self>) -> bool {
        let row = self.caret_row();
        let snapshot = self.editor.buffer().snapshot().clone();
        let r = |a: &Anchor| snapshot.offset_to_point(snapshot.offset_for_anchor(a)).row;
        let before = self.display.folds.manual.len();
        self.display
            .folds
            .manual
            .retain(|m| !(r(&m.start) <= row && row <= r(&m.end)));
        let changed = if self.display.folds.manual.len() != before {
            true
        } else if let Some(f) = self.innermost_at(row) {
            let len = self.display.folds.folds.len();
            self.display
                .folds
                .folds
                .retain(|a| !(r(&a.start) == f.start_row && r(&a.end) == f.end_row));
            len != self.display.folds.folds.len()
        } else {
            false
        };
        if changed {
            self.display.folds.epoch += 1;
            self.display_changed(cx);
        }
        changed
    }

    /// The hidden text of a collapsed fold, at most [`TOOLTIP_LINES`] lines (the adornment's tooltip).
    pub fn fold_tooltip(&self, fold: &Fold) -> String {
        let buffer = self.editor.buffer();
        let last = fold.end_row.min(fold.start_row + TOOLTIP_LINES as u32);
        let mut lines: Vec<String> = (fold.start_row..=last).map(|r| buffer.line(r)).collect();
        if last < fold.end_row {
            lines.push("...".into());
        }
        // Without the common indentation, as Visual Studio's tooltip shows it.
        let indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        lines
            .iter()
            .map(|l| l.get(indent..).unwrap_or(l.trim_start()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_nest_and_take_their_names() {
        let text = "class C\n{\n    #region Fields\n    int a;\n    #region Inner\n    int b;\n    #endregion\n    #endregion\n    #region\n    #endregion\n}\n";
        let r = region_folds("csharp", text);
        assert_eq!(
            r.iter()
                .map(|f| (f.start_row, f.end_row, f.label.clone().unwrap()))
                .collect::<Vec<_>>(),
            vec![
                (4, 6, "Inner".to_owned()),
                (2, 7, "Fields".to_owned()),
                (8, 9, "#region".to_owned())
            ]
        );
        let vb = region_folds("vb", "#Region \"Helpers\"\nSub A()\nEnd Sub\n#End Region\n");
        assert_eq!(vb.len(), 1);
        assert_eq!(vb[0].label.as_deref(), Some("\"Helpers\""));
        let html = region_folds(
            "html",
            "<!-- #region nav -->\n<nav></nav>\n<!-- #endregion -->\n",
        );
        assert_eq!(html[0].label.as_deref(), Some("nav"));
        assert!(region_folds("json", "#region\n#endregion\n").is_empty());
    }

    #[test]
    fn normalize_sorts_outermost_first_and_keeps_one_fold_per_header() {
        let n = normalize(vec![
            FoldRange::new(5, 9, FoldKind::Block),
            FoldRange::new(1, 20, FoldKind::Type),
            FoldRange::new(5, 7, FoldKind::Member),
            FoldRange::new(3, 3, FoldKind::Block),
            FoldRange::new(1, 20, FoldKind::Type),
        ]);
        assert_eq!(
            n.iter()
                .map(|f| (f.start_row, f.end_row))
                .collect::<Vec<_>>(),
            vec![(1, 20), (5, 9)]
        );
    }

    #[test]
    fn every_builtin_fold_query_compiles_and_finds_folds() {
        let registry = crate::syntax::LanguageRegistry::with_builtins();
        let cases: &[(&str, &str, usize)] = &[
            (
                "a.cs",
                "using A;\nusing B;\nnamespace N\n{\n    /// <summary>\n    /// Doc.\n    /// </summary>\n    class C\n    {\n        void M()\n        {\n        }\n    }\n}\n",
                5,
            ),
            (
                "a.rs",
                "use a;\nuse b;\nfn main() {\n    let x = 1;\n}\nimpl S {\n    fn f(&self) {\n    }\n}\n",
                4,
            ),
            (
                "a.ts",
                "import a from 'a';\nimport b from 'b';\nclass C {\n  m() {\n  }\n}\n",
                3,
            ),
            (
                "a.js",
                "function f() {\n  return {\n    a: 1,\n  };\n}\n",
                2,
            ),
            ("a.html", "<div>\n  <p>\n  </p>\n</div>\n", 2),
            ("a.css", ".a {\n  color: red;\n}\n", 1),
            ("a.json", "{\n  \"a\": [\n    1\n  ]\n}\n", 2),
            (
                "a.vb",
                "Imports A\nImports B\nModule M\n    Sub S()\n    End Sub\nEnd Module\n",
                3,
            ),
            (
                "a.fs",
                "module M\n\nlet f x =\n    x + 1\n\ntype T() =\n    member _.A = 1\n",
                2,
            ),
            (
                "a.razor",
                "<div>\n    <p>hi</p>\n</div>\n@code {\n    void M()\n    {\n    }\n}\n",
                2,
            ),
        ];
        for (path, text, at_least) in cases {
            let lang = registry.for_path(std::path::Path::new(path)).unwrap();
            assert!(has_fold_query(&lang), "{path}: the fold query compiles");
            let folds = syntax_fold_ranges(&lang, text);
            assert!(
                folds.len() >= *at_least,
                "{path}: {} folds, wanted {at_least}: {folds:?}",
                folds.len()
            );
        }
    }

    #[test]
    fn csharp_folds_have_kinds_and_start_at_the_name() {
        let registry = crate::syntax::LanguageRegistry::with_builtins();
        let lang = registry.for_path(std::path::Path::new("a.cs")).unwrap();
        let text = "using A;\nusing B;\nnamespace N\n{\n    [Attr]\n    class C\n    {\n        // one\n        // two\n        void M()\n        {\n        }\n    }\n}\n";
        let folds = syntax_fold_ranges(&lang, text);
        let find = |k: FoldKind| folds.iter().find(|f| f.kind == k).cloned();
        assert_eq!(
            find(FoldKind::Imports).map(|f| (f.start_row, f.end_row)),
            Some((0, 1))
        );
        assert_eq!(
            find(FoldKind::Namespace).map(|f| (f.start_row, f.end_row)),
            Some((2, 13))
        );
        // The class folds from its name's row, not its attribute's.
        assert_eq!(
            find(FoldKind::Type).map(|f| (f.start_row, f.end_row)),
            Some((5, 12))
        );
        assert_eq!(
            find(FoldKind::Comment).map(|f| (f.start_row, f.end_row)),
            Some((7, 8))
        );
        assert_eq!(
            find(FoldKind::Member).map(|f| (f.start_row, f.end_row)),
            Some((9, 11))
        );
    }
}
