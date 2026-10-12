//! The navigation set's LSP answers (brief 0071), read from raw JSON: the requests go out untyped through the host
//! (host-rpc.md, "Forwarded LSP methods, untyped") and to generic servers alike, so the shell parses the answers here
//! into small typed values.
//!
//! - [`locations`]: `textDocument/implementation` and `typeDefinition` (a `Location`, `Location[]` or
//!   `LocationLink[]`, or null).
//! - [`highlights`]: `textDocument/documentHighlight`, with LSP's kinds (1 text, 2 read, 3 write).
//! - [`symbols`]: `textDocument/documentSymbol`, nested `DocumentSymbol[]` or flat `SymbolInformation[]` (nested here
//!   by containment).
//! - [`workspace_symbols`]: `workspace/symbol`, `SymbolInformation[]` or `WorkspaceSymbol[]`.
//! - [`HierarchyItem`]: call and type hierarchy items, kept whole so `data` goes back unchanged; [`calls`] reads
//!   `callHierarchy/incomingCalls` and `outgoingCalls`.
//! - [`kind_name`]: an LSP symbol kind in Visual Studio's words; [`is_source_generated`] and [`generated_title`] for
//!   Roslyn's `roslyn-source-generated://` documents.

use eludite_protocol::lsp::{Location, Position, Range};
use serde_json::{Value, json};

/// The scheme of Roslyn's source-generated documents.
pub const SOURCE_GENERATED_SCHEME: &str = "roslyn-source-generated";

/// LSP 3.18's request for a document's text by URI (Roslyn serves source-generated documents through it).
pub const TEXT_DOCUMENT_CONTENT: &str = "workspace/textDocumentContent";

fn range(v: &Value) -> Option<Range> {
    serde_json::from_value(v.clone()).ok()
}

fn location(v: &Value) -> Option<Location> {
    if let (Some(uri), Some(r)) = (v["targetUri"].as_str(), v.get("targetSelectionRange")) {
        return Some(Location {
            uri: uri.to_owned(),
            range: range(r)?,
        });
    }
    Some(Location {
        uri: v["uri"].as_str()?.to_owned(),
        range: range(&v["range"])?,
    })
}

/// The locations of a definition-like answer, duplicates dropped, in the server's order.
pub fn locations(answer: &Value) -> Vec<Location> {
    let items: Vec<&Value> = match answer {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![answer],
        _ => Vec::new(),
    };
    let mut out: Vec<Location> = Vec::new();
    for l in items.into_iter().filter_map(location) {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// A highlight's kind (LSP `DocumentHighlightKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Text,
    Read,
    Write,
}

/// One `DocumentHighlight`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    pub range: Range,
    pub kind: HighlightKind,
}

/// `textDocument/documentHighlight`'s answer, sorted by position (a missing kind is text, as LSP says).
pub fn highlights(answer: &Value) -> Vec<Highlight> {
    let mut out: Vec<Highlight> = answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            Some(Highlight {
                range: range(&h["range"])?,
                kind: match h["kind"].as_u64() {
                    Some(2) => HighlightKind::Read,
                    Some(3) => HighlightKind::Write,
                    _ => HighlightKind::Text,
                },
            })
        })
        .collect();
    out.sort_by_key(|h| (h.range.start.line, h.range.start.character));
    out.dedup_by(|a, b| a.range == b.range);
    out
}

/// A document symbol, nested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: u32,
    pub detail: Option<String>,
    /// The whole symbol (its body included).
    pub range: Range,
    /// The symbol's name.
    pub selection: Range,
    pub children: Vec<Symbol>,
}

fn before_or_at(a: Position, b: Position) -> bool {
    (a.line, a.character) <= (b.line, b.character)
}

/// Whether `outer` contains `inner`.
pub fn contains(outer: &Range, inner: &Range) -> bool {
    before_or_at(outer.start, inner.start) && before_or_at(inner.end, outer.end)
}

/// Whether `range` contains position `p` (its end included, as a caret just after a name is on it).
pub fn contains_position(range: &Range, p: Position) -> bool {
    before_or_at(range.start, p) && before_or_at(p, range.end)
}

fn nested(v: &Value) -> Option<Symbol> {
    let range = range(&v["range"])?;
    let mut children: Vec<Symbol> = v["children"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(nested)
        .collect();
    sort(&mut children);
    Some(Symbol {
        name: v["name"].as_str()?.to_owned(),
        kind: v["kind"].as_u64().unwrap_or(0) as u32,
        detail: v["detail"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(str::to_owned),
        selection: v
            .get("selectionRange")
            .and_then(self::range)
            .unwrap_or(range),
        range,
        children,
    })
}

fn sort(symbols: &mut [Symbol]) {
    symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));
}

/// Insert `s` under the innermost symbol of `into` that contains it.
fn insert(into: &mut Vec<Symbol>, s: Symbol) {
    if let Some(parent) = into
        .iter_mut()
        .rev()
        .find(|p| p.range != s.range && contains(&p.range, &s.range))
    {
        insert(&mut parent.children, s);
    } else {
        into.push(s);
    }
}

/// `textDocument/documentSymbol`'s answer as a tree in document order: nested answers as given, flat ones nested by
/// containment (outer symbols first).
pub fn symbols(answer: &Value) -> Vec<Symbol> {
    let Some(items) = answer.as_array() else {
        return Vec::new();
    };
    let flat = items.iter().any(|i| i.get("location").is_some());
    if !flat {
        let mut out: Vec<Symbol> = items.iter().filter_map(nested).collect();
        sort(&mut out);
        return out;
    }
    let mut all: Vec<Symbol> = items
        .iter()
        .filter_map(|i| {
            let range = range(&i["location"]["range"])?;
            Some(Symbol {
                name: i["name"].as_str()?.to_owned(),
                kind: i["kind"].as_u64().unwrap_or(0) as u32,
                detail: None,
                range,
                selection: range,
                children: Vec::new(),
            })
        })
        .collect();
    // Outer symbols first, so each lands under the symbol holding it.
    all.sort_by(|a, b| {
        (a.range.start.line, a.range.start.character)
            .cmp(&(b.range.start.line, b.range.start.character))
            .then(
                (b.range.end.line, b.range.end.character)
                    .cmp(&(a.range.end.line, a.range.end.character)),
            )
    });
    let mut out = Vec::new();
    for s in all {
        insert(&mut out, s);
    }
    out
}

/// The path of symbols (outermost first) holding `p`, as indices into each level's children.
pub fn path_at(symbols: &[Symbol], p: Position) -> Vec<usize> {
    let mut path = Vec::new();
    let mut level = symbols;
    while let Some(ix) = level.iter().rposition(|s| contains_position(&s.range, p)) {
        path.push(ix);
        level = &level[ix].children;
    }
    path
}

/// A `workspace/symbol` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: u32,
    pub container: Option<String>,
    pub uri: String,
    /// `None` for a 3.17 `WorkspaceSymbol` whose location is only `{ uri }` (resolved later; the file opens at its
    /// top).
    pub range: Option<Range>,
}

/// `workspace/symbol`'s answer.
pub fn workspace_symbols(answer: &Value) -> Vec<WorkspaceSymbol> {
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(WorkspaceSymbol {
                name: s["name"].as_str()?.to_owned(),
                kind: s["kind"].as_u64().unwrap_or(0) as u32,
                container: s["containerName"]
                    .as_str()
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned),
                uri: s["location"]["uri"].as_str()?.to_owned(),
                range: s["location"].get("range").and_then(range),
            })
        })
        .collect()
}

/// A call or type hierarchy item, with the whole item kept so it goes back to the server unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct HierarchyItem {
    pub name: String,
    pub kind: u32,
    pub detail: Option<String>,
    pub uri: String,
    pub range: Range,
    pub selection: Range,
    pub raw: Value,
}

impl HierarchyItem {
    pub fn parse(v: &Value) -> Option<Self> {
        let range = range(&v["range"])?;
        Some(Self {
            name: v["name"].as_str()?.to_owned(),
            kind: v["kind"].as_u64().unwrap_or(0) as u32,
            detail: v["detail"]
                .as_str()
                .filter(|d| !d.is_empty())
                .map(str::to_owned),
            uri: v["uri"].as_str()?.to_owned(),
            selection: v
                .get("selectionRange")
                .and_then(self::range)
                .unwrap_or(range),
            range,
            raw: v.clone(),
        })
    }

    /// The params of `callHierarchy/*Calls` and `typeHierarchy/*types` for this item.
    pub fn params(&self) -> Value {
        json!({ "item": self.raw })
    }
}

/// `textDocument/prepareCallHierarchy` or `prepareTypeHierarchy`'s answer.
pub fn hierarchy_items(answer: &Value) -> Vec<HierarchyItem> {
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(HierarchyItem::parse)
        .collect()
}

/// One incoming or outgoing call: the other item and the call sites (`fromRanges`).
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub item: HierarchyItem,
    pub sites: Vec<Range>,
}

/// `callHierarchy/incomingCalls` (`incoming`: items under `from`) or `outgoingCalls` (under `to`).
pub fn calls(answer: &Value, incoming: bool) -> Vec<Call> {
    let key = if incoming { "from" } else { "to" };
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(Call {
                item: HierarchyItem::parse(&c[key])?,
                sites: c["fromRanges"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(range)
                    .collect(),
            })
        })
        .collect()
}

/// An LSP symbol kind in Visual Studio's words (the outputs' `kind`).
pub fn kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum_member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type_parameter",
        _ => "symbol",
    }
}

/// Whether kind `kind` is a type (class, enum, interface, struct) for the navigation bar's types dropdown.
pub fn is_type_kind(kind: u32) -> bool {
    matches!(kind, 5 | 10 | 11 | 23)
}

/// Whether kind `kind` only contains types (a namespace, module or package), flattened in the navigation bar.
pub fn is_container_kind(kind: u32) -> bool {
    matches!(kind, 2..=4)
}

/// Whether `uri` is one of Roslyn's source-generated documents.
pub fn is_source_generated(uri: &str) -> bool {
    uri.split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(SOURCE_GENERATED_SCHEME))
}

/// The tab title of a source-generated document: its hint name and `[generated]`
/// (`roslyn-source-generated://<project>/Foo.g.cs?documentId=...` is `Foo.g.cs [generated]`).
pub fn generated_title(uri: &str) -> String {
    let rest = uri.split_once("://").map_or(uri, |(_, r)| r);
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    let name = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path);
    let name = percent_decode(name);
    format!("{name} [generated]")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `workspace/textDocumentContent`'s params for `uri`.
pub fn content_params(uri: &str) -> Value {
    json!({ "uri": uri })
}

/// `workspace/textDocumentContent`'s answer: the text.
pub fn content_text(answer: &Value) -> Option<String> {
    answer["text"].as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l0: u32, c0: u32, l1: u32, c1: u32) -> Value {
        json!({"start": {"line": l0, "character": c0}, "end": {"line": l1, "character": c1}})
    }

    #[test]
    fn locations_read_every_shape() {
        let loc = json!({"uri": "file:///a.cs", "range": r(1, 2, 1, 5)});
        assert_eq!(locations(&loc).len(), 1);
        assert_eq!(
            locations(&json!([loc.clone(), loc.clone()])).len(),
            1,
            "deduplicated"
        );
        let link = json!([{"targetUri": "file:///b.cs", "targetRange": r(0, 0, 9, 0), "targetSelectionRange": r(3, 4, 3, 9)}]);
        let l = locations(&link);
        assert_eq!(l[0].uri, "file:///b.cs");
        assert_eq!(
            l[0].range.start,
            Position {
                line: 3,
                character: 4
            }
        );
        assert!(locations(&Value::Null).is_empty());
    }

    #[test]
    fn highlights_have_kinds_and_are_sorted() {
        let h = highlights(&json!([
            {"range": r(5, 1, 5, 4), "kind": 3},
            {"range": r(1, 1, 1, 4), "kind": 2},
            {"range": r(3, 1, 3, 4)},
        ]));
        let kinds: Vec<_> = h.iter().map(|h| (h.range.start.line, h.kind)).collect();
        assert_eq!(
            kinds,
            [
                (1, HighlightKind::Read),
                (3, HighlightKind::Text),
                (5, HighlightKind::Write)
            ]
        );
    }

    #[test]
    fn flat_symbols_nest_by_containment_and_paths_find_the_caret() {
        let flat = json!([
            {"name": "Total", "kind": 7, "location": {"uri": "file:///a.cs", "range": r(2, 4, 2, 30)}, "containerName": "Order"},
            {"name": "App", "kind": 3, "location": {"uri": "file:///a.cs", "range": r(0, 0, 10, 1)}},
            {"name": "Order", "kind": 5, "location": {"uri": "file:///a.cs", "range": r(1, 0, 5, 1)}},
            {"name": "Line", "kind": 5, "location": {"uri": "file:///a.cs", "range": r(6, 0, 9, 1)}},
        ]);
        let s = symbols(&flat);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "App");
        let names: Vec<_> = s[0].children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Order", "Line"]);
        assert_eq!(s[0].children[0].children[0].name, "Total");
        assert_eq!(
            path_at(
                &s,
                Position {
                    line: 2,
                    character: 10
                }
            ),
            [0, 0, 0]
        );
        assert_eq!(
            path_at(
                &s,
                Position {
                    line: 7,
                    character: 0
                }
            ),
            [0, 1]
        );
        assert!(
            path_at(
                &s,
                Position {
                    line: 20,
                    character: 0
                }
            )
            .is_empty()
        );

        let nested = json!([{"name": "Order", "kind": 5, "range": r(0, 0, 4, 1), "selectionRange": r(0, 6, 0, 11),
            "children": [{"name": "B", "kind": 6, "range": r(3, 4, 3, 20), "selectionRange": r(3, 9, 3, 10)},
                         {"name": "A", "kind": 6, "range": r(1, 4, 1, 20), "selectionRange": r(1, 9, 1, 10), "detail": "void A()"}]}]);
        let s = symbols(&nested);
        let names: Vec<_> = s[0].children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "B"], "document order");
        assert_eq!(s[0].selection.start.character, 6);
        assert_eq!(s[0].children[0].detail.as_deref(), Some("void A()"));
    }

    #[test]
    fn workspace_symbols_and_hierarchy_items_parse() {
        let w = workspace_symbols(&json!([
            {"name": "Order", "kind": 5, "location": {"uri": "file:///o.cs", "range": r(0, 6, 0, 11)}, "containerName": "App.Models"},
            {"name": "Widget", "kind": 5, "location": {"uri": "file:///w.cs"}},
        ]));
        assert_eq!(w[0].container.as_deref(), Some("App.Models"));
        assert!(w[1].range.is_none());
        let item = json!({"name": "Run", "kind": 6, "uri": "file:///p.cs", "range": r(4, 4, 8, 5), "selectionRange": r(4, 16, 4, 19), "data": {"id": 7}});
        let items = hierarchy_items(&json!([item.clone()]));
        assert_eq!(items[0].params(), json!({"item": item}));
        let incoming = calls(
            &json!([{"from": {"name": "Main", "kind": 6, "uri": "file:///p.cs", "range": r(1, 0, 3, 1), "selectionRange": r(1, 16, 1, 20)},
                     "fromRanges": [r(2, 8, 2, 11), r(2, 20, 2, 23)]}]),
            true,
        );
        assert_eq!(incoming[0].item.name, "Main");
        assert_eq!(incoming[0].sites.len(), 2);
        let outgoing = calls(
            &json!([{"to": {"name": "Log", "kind": 6, "uri": "file:///p.cs", "range": r(9, 0, 9, 9)}, "fromRanges": []}]),
            false,
        );
        assert_eq!(outgoing[0].item.name, "Log");
        assert_eq!(kind_name(5), "class");
        assert_eq!(kind_name(99), "symbol");
    }

    #[test]
    fn source_generated_uris_are_recognized_and_titled() {
        let uri = "roslyn-source-generated://E7D5BCFA-E345-4029-9D12-3EDCD0FB0F6B/RegexGenerator.g.cs?documentId=8E4C&assembly=System.Text.RegularExpressions.Generator";
        assert!(is_source_generated(uri));
        assert!(!is_source_generated("file:///a.cs"));
        assert_eq!(generated_title(uri), "RegexGenerator.g.cs [generated]");
        assert_eq!(
            generated_title("roslyn-source-generated://p/My%20File.g.cs"),
            "My File.g.cs [generated]"
        );
        assert_eq!(
            content_text(&json!({"text": "class A { }"})).as_deref(),
            Some("class A { }")
        );
    }
}
