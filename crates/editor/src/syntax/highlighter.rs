//! Incremental parsing and highlighting of one buffer.

use std::ops::{ControlFlow, Range};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use text::{BufferSnapshot, Point};
use tree_sitter::{InputEdit, Parser, QueryCursor, StreamingIterator, Tree};

use super::highlights::{PointEdit, edits_between};
use super::{HighlightKind, Language, LineHighlights, Span};

/// Rows highlighted per [`Highlighter::step`] beyond the priority range, so a
/// large file's highlights stream in instead of arriving all at once.
pub const ROWS_PER_STEP: u32 = 8_000;

/// The result of one [`Highlighter::step`].
#[derive(Clone)]
pub struct HighlightUpdate {
    /// The buffer version these highlights describe.
    pub version: clock::Global,
    /// The snapshot that version corresponds to; lets the receiver
    /// interpolate the highlights to a newer version.
    pub snapshot: BufferSnapshot,
    pub highlights: LineHighlights,
    /// No dirty rows remain for this version.
    pub complete: bool,
    pub stats: HighlightStats,
}

impl std::fmt::Debug for HighlightUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HighlightUpdate")
            .field("complete", &self.complete)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

/// Timings for one step, for benchmarks and logs.
#[derive(Clone, Copy, Debug, Default)]
pub struct HighlightStats {
    pub parse: Duration,
    pub highlight: Duration,
    pub rows_highlighted: u32,
    /// The step parsed the whole buffer (no previous tree).
    pub full_parse: bool,
}

/// Owns the parser, the last tree and the highlights for one buffer.
///
/// It is `Send` and does all its work in [`Highlighter::step`], which the
/// view runs on a background thread. Cancellation: setting the flag from
/// [`Highlighter::cancel_flag`] makes a running parse return early and
/// `step` return `None`; the highlighter then re-parses from its previous
/// tree on the next call.
pub struct Highlighter {
    language: Arc<Language>,
    parser: Parser,
    tree: Option<Tree>,
    snapshot: Option<BufferSnapshot>,
    highlights: LineHighlights,
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter")
            .field("language", &self.language.id())
            .finish_non_exhaustive()
    }
}

impl Highlighter {
    pub fn new(language: Arc<Language>) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(language.grammar())
            .expect("grammar checked by Language::new");
        Self {
            language,
            parser,
            tree: None,
            snapshot: None,
            highlights: LineHighlights::default(),
            cancel: Arc::default(),
        }
    }

    pub fn language(&self) -> &Arc<Language> {
        &self.language
    }

    /// Set to `true` to abandon the current and later steps.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// The highlights as of the last completed step.
    pub fn highlights(&self) -> &LineHighlights {
        &self.highlights
    }

    /// The version of the last completed step.
    pub fn version(&self) -> Option<&clock::Global> {
        self.snapshot.as_ref().map(|s| s.version())
    }

    /// True when the last step's version has no dirty rows left.
    pub fn is_complete(&self) -> bool {
        self.snapshot.is_some() && self.highlights.dirty_row_count() == 0
    }

    /// Bring the tree up to `snapshot` (incrementally when there is a previous
    /// tree), then highlight dirty rows: first those in `priority`, then up to
    /// [`ROWS_PER_STEP`] more in buffer order.
    pub fn step(
        &mut self,
        snapshot: &BufferSnapshot,
        priority: Range<u32>,
    ) -> Option<HighlightUpdate> {
        let mut stats = HighlightStats::default();
        let parse_started = Instant::now();
        let needs_parse = self.tree.is_none()
            || self
                .snapshot
                .as_ref()
                .is_none_or(|s| s.version() != snapshot.version());
        if needs_parse {
            let (old_tree, edits) = match (&self.tree, &self.snapshot) {
                (Some(tree), Some(old)) => {
                    let edits = edits_between(old, snapshot);
                    let mut tree = tree.clone();
                    for edit in edits.iter().rev() {
                        tree.edit(&input_edit(old, snapshot, edit));
                    }
                    (Some(tree), Some(edits))
                }
                _ => (None, None),
            };
            stats.full_parse = old_tree.is_none();
            let new_tree = self.parse(snapshot, old_tree.as_ref())?;
            match edits {
                Some(edits) => {
                    self.highlights.apply_edits(&edits);
                    let old_tree = old_tree.expect("edits imply a tree");
                    for range in old_tree.changed_ranges(&new_tree) {
                        self.highlights.mark_dirty(
                            range.start_point.row as u32..range.end_point.row as u32 + 1,
                        );
                    }
                }
                None => {
                    self.highlights = LineHighlights::dirty(snapshot.max_point().row + 1);
                }
            }
            self.tree = Some(new_tree);
            self.snapshot = Some(snapshot.clone());
        }
        stats.parse = parse_started.elapsed();

        let highlight_started = Instant::now();
        let tree = self.tree.clone().expect("parsed above");
        let dirty = self.highlights.dirty_ranges();
        let mut todo: Vec<Range<u32>> = dirty
            .iter()
            .filter_map(|r| intersect(r, &priority))
            .collect();
        let mut budget = ROWS_PER_STEP;
        for r in &dirty {
            if budget == 0 {
                break;
            }
            for piece in subtract(r, &priority) {
                let take = (piece.end - piece.start).min(budget);
                if take > 0 {
                    todo.push(piece.start..piece.start + take);
                    budget -= take;
                }
            }
        }
        for rows in todo {
            if self.cancel.load(Ordering::Relaxed) {
                return None;
            }
            stats.rows_highlighted += rows.end - rows.start;
            self.highlight_rows(&tree, snapshot, rows);
        }
        stats.highlight = highlight_started.elapsed();

        Some(HighlightUpdate {
            version: snapshot.version().clone(),
            snapshot: snapshot.clone(),
            highlights: self.highlights.clone(),
            complete: self.highlights.dirty_row_count() == 0,
            stats,
        })
    }

    fn parse(&mut self, snapshot: &BufferSnapshot, old_tree: Option<&Tree>) -> Option<Tree> {
        let rope = snapshot.as_rope();
        let len = rope.len();
        let cancel = self.cancel.clone();
        let mut progress = |_: &tree_sitter::ParseState| {
            if cancel.load(Ordering::Relaxed) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = tree_sitter::ParseOptions::new().progress_callback(&mut progress);
        let mut read = |byte: usize, _: tree_sitter::Point| -> &[u8] {
            if byte >= len {
                return &[];
            }
            rope.chunks_in_range(byte..len)
                .next()
                .map_or(&[][..], str::as_bytes)
        };
        self.parser
            .parse_with_options(&mut read, old_tree, Some(options))
    }

    /// Re-highlight `rows` from `tree` and store the result.
    fn highlight_rows(&mut self, tree: &Tree, snapshot: &BufferSnapshot, rows: Range<u32>) {
        let max_row = snapshot.max_point().row;
        let rows = rows.start..rows.end.min(max_row + 1);
        if rows.is_empty() {
            return;
        }
        let start = snapshot.point_to_offset(Point::new(rows.start, 0));
        let end = if rows.end > max_row {
            snapshot.len()
        } else {
            snapshot.point_to_offset(Point::new(rows.end, 0))
        };

        // Collect captures: (byte range, pattern index, kind).
        let rope = snapshot.as_rope();
        let query = self.language.query();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start..end);
        let mut captures: Vec<(usize, usize, usize, HighlightKind)> = Vec::new();
        let text = |node: tree_sitter::Node| {
            rope.chunks_in_range(node.byte_range())
                .map(|chunk| chunk.as_bytes())
        };
        let mut matches = cursor.matches(query, tree.root_node(), text);
        while let Some(m) = matches.next() {
            for capture in m.captures() {
                if let Some(kind) = self.language.capture_kind(capture.index) {
                    let r = capture.node.byte_range();
                    if r.end > start && r.start < end && r.start < r.end {
                        captures.push((r.start, r.end, m.pattern_index, kind));
                    }
                }
            }
        }
        // Outer nodes before inner ones; for the same node, earlier patterns first.
        captures.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));

        // Paint kinds over the byte range: inner captures overwrite outer ones,
        // a repeated capture of the same range keeps the first pattern's kind.
        let mut paint: Vec<u8> = vec![0; end - start];
        let mut last: Option<(usize, usize)> = None;
        for (s, e, _, kind) in captures {
            if last == Some((s, e)) {
                continue;
            }
            last = Some((s, e));
            let (s, e) = (s.max(start) - start, e.min(end) - start);
            paint[s..e].fill(kind as u8 + 1);
        }

        // Split into rows and run-length encode.
        let mut row_start = 0usize;
        for row in rows.clone() {
            let row_len = snapshot.line_len(row) as usize;
            let row_end = (row_start + row_len).min(paint.len());
            let mut spans = Vec::new();
            let mut col = row_start;
            while col < row_end {
                let k = paint[col];
                let run_start = col;
                while col < row_end && paint[col] == k {
                    col += 1;
                }
                if k != 0 {
                    spans.push(Span {
                        start: (run_start - row_start) as u32,
                        end: (col - row_start) as u32,
                        kind: kind_from_u8(k - 1),
                    });
                }
            }
            self.highlights.set_row(row, spans);
            row_start = row_end + 1;
        }
    }
}

fn kind_from_u8(v: u8) -> HighlightKind {
    // `HighlightKind` is `repr(u8)` with contiguous discriminants.
    const ALL: [HighlightKind; 22] = [
        HighlightKind::Keyword,
        HighlightKind::Type,
        HighlightKind::TypeBuiltin,
        HighlightKind::Function,
        HighlightKind::Macro,
        HighlightKind::String,
        HighlightKind::Escape,
        HighlightKind::Number,
        HighlightKind::Constant,
        HighlightKind::ConstantBuiltin,
        HighlightKind::Comment,
        HighlightKind::DocComment,
        HighlightKind::Variable,
        HighlightKind::Parameter,
        HighlightKind::VariableBuiltin,
        HighlightKind::Property,
        HighlightKind::Attribute,
        HighlightKind::Namespace,
        HighlightKind::Label,
        HighlightKind::Operator,
        HighlightKind::Punctuation,
        HighlightKind::Preprocessor,
    ];
    ALL[v as usize]
}

fn ts_point(p: Point) -> tree_sitter::Point {
    tree_sitter::Point::new(p.row as usize, p.column as usize)
}

/// The tree-sitter edit for `edit`, expressed so it can be applied after all
/// later edits (edits are applied last to first).
fn input_edit(old: &BufferSnapshot, new: &BufferSnapshot, edit: &PointEdit) -> InputEdit {
    let start_byte = old.point_to_offset(edit.old.start);
    let old_end_byte = old.point_to_offset(edit.old.end);
    let new_len = new.point_to_offset(edit.new.end) - new.point_to_offset(edit.new.start);
    let rows = edit.new.end.row - edit.new.start.row;
    let new_end = if rows == 0 {
        Point::new(
            edit.old.start.row,
            edit.old.start.column + edit.new.end.column - edit.new.start.column,
        )
    } else {
        Point::new(edit.old.start.row + rows, edit.new.end.column)
    };
    InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte: start_byte + new_len,
        start_position: ts_point(edit.old.start),
        old_end_position: ts_point(edit.old.end),
        new_end_position: ts_point(new_end),
    }
}

fn intersect(a: &Range<u32>, b: &Range<u32>) -> Option<Range<u32>> {
    let r = a.start.max(b.start)..a.end.min(b.end);
    (!r.is_empty()).then_some(r)
}

fn subtract(a: &Range<u32>, b: &Range<u32>) -> Vec<Range<u32>> {
    let mut out = Vec::new();
    if a.start < b.start.min(a.end) {
        out.push(a.start..b.start.min(a.end));
    }
    if b.end.max(a.start) < a.end {
        out.push(b.end.max(a.start)..a.end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::LanguageRegistry;
    use text::{Buffer as TextBuffer, BufferId, ReplicaId};

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), text)
    }

    fn highlight_all(h: &mut Highlighter, snapshot: &BufferSnapshot) -> HighlightUpdate {
        loop {
            let update = h.step(snapshot, 0..0).expect("not cancelled");
            if update.complete {
                return update;
            }
        }
    }

    /// Kind at the first occurrence of `needle` in the update's text.
    fn kind_of(update: &HighlightUpdate, needle: &str) -> Option<HighlightKind> {
        let text = update.snapshot.text();
        let offset = text
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in text"));
        update
            .highlights
            .kind_at(update.snapshot.offset_to_point(offset))
    }

    const CSHARP_SOURCE: &str = r#"using System.Text;

namespace Demo.App
{
    // A greeter.
    [Serializable]
    public class Greeter : IGreeter
    {
        private const int Count = 42;
        public string Name { get; set; }

        public string Greet(string who)
        {
            var sb = new StringBuilder();
            sb.Append("Hello, \n");
            return who == null ? "nobody" : sb.ToString();
        }
    }
}
"#;

    const RUST_SOURCE: &str = r#"use std::fmt;

/// Doc comment.
#[derive(Debug)]
pub struct Point { x: i32 }

const MAX_LEN: usize = 10;

fn main() {
    let p = Point { x: 1 };
    println!("{:?}", p);
    helper(MAX_LEN);
}
"#;

    #[test]
    fn csharp_token_kinds() {
        let registry = LanguageRegistry::with_builtins();
        let mut h = Highlighter::new(registry.by_id("csharp").unwrap());
        let b = buffer(CSHARP_SOURCE);
        let u = highlight_all(&mut h, b.snapshot());
        use HighlightKind::*;
        assert_eq!(kind_of(&u, "using"), Some(Keyword));
        assert_eq!(kind_of(&u, "System.Text"), Some(Namespace));
        assert_eq!(kind_of(&u, "Demo.App"), Some(Namespace));
        assert_eq!(kind_of(&u, "// A greeter"), Some(Comment));
        assert_eq!(kind_of(&u, "Serializable"), Some(Attribute));
        assert_eq!(kind_of(&u, "public"), Some(Keyword));
        assert_eq!(kind_of(&u, "class"), Some(Keyword));
        assert_eq!(kind_of(&u, "Greeter "), Some(Type));
        assert_eq!(kind_of(&u, "IGreeter"), Some(Type));
        assert_eq!(kind_of(&u, "int"), Some(TypeBuiltin));
        assert_eq!(kind_of(&u, "42"), Some(Number));
        assert_eq!(kind_of(&u, "Name"), Some(Property));
        assert_eq!(kind_of(&u, "Greet("), Some(Function));
        assert_eq!(kind_of(&u, "who)"), Some(Parameter));
        assert_eq!(kind_of(&u, "var"), Some(Keyword));
        assert_eq!(kind_of(&u, "StringBuilder()"), Some(Type));
        assert_eq!(kind_of(&u, "Append"), Some(Function));
        assert_eq!(kind_of(&u, "\"Hello"), Some(String));
        assert_eq!(kind_of(&u, "\\n"), Some(Escape));
        assert_eq!(kind_of(&u, "null"), Some(ConstantBuiltin));
        assert_eq!(kind_of(&u, "== null"), Some(Operator));
        assert_eq!(
            kind_of(&u, "sb.Append"),
            None,
            "locals use the default color"
        );
    }

    #[test]
    fn rust_token_kinds() {
        let registry = LanguageRegistry::with_builtins();
        let mut h = Highlighter::new(registry.by_id("rust").unwrap());
        let b = buffer(RUST_SOURCE);
        let u = highlight_all(&mut h, b.snapshot());
        use HighlightKind::*;
        assert_eq!(kind_of(&u, "use"), Some(Keyword));
        assert_eq!(kind_of(&u, "/// Doc"), Some(DocComment));
        assert_eq!(kind_of(&u, "#[derive"), Some(Attribute));
        assert_eq!(kind_of(&u, "pub"), Some(Keyword));
        assert_eq!(kind_of(&u, "Point {"), Some(Type));
        assert_eq!(kind_of(&u, "i32"), Some(TypeBuiltin));
        assert_eq!(kind_of(&u, "MAX_LEN"), Some(Constant));
        assert_eq!(kind_of(&u, "main"), Some(Function));
        assert_eq!(kind_of(&u, "println"), Some(Macro));
        assert_eq!(kind_of(&u, "\"{:?}\""), Some(String));
        assert_eq!(kind_of(&u, "helper"), Some(Function));
        assert_eq!(kind_of(&u, "1 }"), Some(ConstantBuiltin));
    }

    #[test]
    fn incremental_reparse_matches_a_fresh_parse() {
        let registry = LanguageRegistry::with_builtins();
        let csharp = registry.by_id("csharp").unwrap();
        let mut h = Highlighter::new(csharp.clone());
        let mut b = buffer(CSHARP_SOURCE);
        let first = highlight_all(&mut h, b.snapshot());
        assert!(first.stats.full_parse);

        // Open a block comment that swallows several lines, then type in it.
        let at = CSHARP_SOURCE.find("private const").unwrap();
        b.edit([(at..at, "/* ")]);
        let end = b.snapshot().text().find("public string Greet").unwrap();
        b.edit([(end..end, "*/ ")]);
        let second = highlight_all(&mut h, b.snapshot());
        assert!(!second.stats.full_parse, "re-parse reuses the old tree");
        assert_eq!(kind_of(&second, "const int"), Some(HighlightKind::Comment));
        assert_eq!(kind_of(&second, "Name {"), Some(HighlightKind::Comment));

        let mut fresh = Highlighter::new(csharp);
        let expected = highlight_all(&mut fresh, b.snapshot());
        for row in 0..expected.highlights.row_count() {
            assert_eq!(
                second.highlights.spans(row),
                expected.highlights.spans(row),
                "row {row}"
            );
        }
    }

    #[test]
    fn priority_rows_are_highlighted_first() {
        let registry = LanguageRegistry::with_builtins();
        let mut h = Highlighter::new(registry.by_id("rust").unwrap());
        let mut text = String::new();
        for i in 0..(ROWS_PER_STEP * 2) {
            text.push_str(&format!("fn f{i}() {{}}\n"));
        }
        let b = buffer(&text);
        let far = ROWS_PER_STEP * 2 - 10;
        let u = h.step(b.snapshot(), far..far + 5).unwrap();
        assert!(!u.complete);
        assert_eq!(u.highlights.spans(far)[0].kind, HighlightKind::Keyword);
        assert_eq!(u.highlights.spans(0)[0].kind, HighlightKind::Keyword);
        assert!(u.highlights.is_dirty(ROWS_PER_STEP + 10));
        let u = h.step(b.snapshot(), 0..0).unwrap();
        assert!(u.complete);
    }

    #[test]
    fn cancellation_stops_a_step() {
        let registry = LanguageRegistry::with_builtins();
        let mut h = Highlighter::new(registry.by_id("rust").unwrap());
        h.cancel_flag().store(true, Ordering::Relaxed);
        let b = buffer("fn main() {}\n");
        assert!(h.step(b.snapshot(), 0..1).is_none());
    }

    #[test]
    fn registered_languages_resolve_by_path() {
        let registry = LanguageRegistry::with_builtins();
        let cs = registry
            .for_path(std::path::Path::new("Program.CS"))
            .unwrap();
        assert_eq!(cs.id(), "csharp");
        assert_eq!(
            registry
                .for_path(std::path::Path::new("lib.rs"))
                .unwrap()
                .name(),
            "Rust"
        );
        assert!(registry.for_path(std::path::Path::new("a.txt")).is_none());
    }
}
