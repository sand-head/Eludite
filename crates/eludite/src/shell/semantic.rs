//! Semantic highlighting (brief 0066): the language servers' classifications painted over the tree-sitter colors,
//! and unnecessary code faded.
//!
//! - **Requests.** The editor says when a document's tokens are wanted (turned on, 150 ms after the last edit, a
//!   refresh). The shell sends, through the document's server session after a pending `didChange` (one request in
//!   flight per document; a newer one cancels the older): `full/delta` quoting the last `resultId` when the server
//!   offers deltas and the document has an answer from the same server generation; for a document over
//!   [`LARGE_DOCUMENT`] lines with nothing shown yet, `range` for the visible rows (and 100 lines around them) first,
//!   then `full`; else `full` (or `range` over the whole document for a server that only serves ranges). A server
//!   without `semanticTokensProvider` is never asked.
//! - **Decoding** (the five-integer encoding to rows of colored byte ranges, UTF-16 columns converted on the snapshot
//!   the request was made on) runs on a background thread. An answer is dropped when its server generation or the
//!   document's version moved since (CLAUDE.md invariant 12), and the editor drops one for text changed since.
//! - **Colors** come from the theme's [`eludite_ui::SemanticColors`] by [`TokenClass`]; a class with no color leaves the
//!   syntax color. A deprecated symbol is struck through.
//! - **Refresh.** `workspace/semanticTokens/refresh` (relayed by the host as `eludite/semanticTokens/refresh`), a new
//!   generation, a server's capabilities arriving and the solution finishing its load ask again.
//! - **Settings** `editor.semanticHighlighting` and `editor.languages.<id>.semanticHighlighting` (`default`, `on`,
//!   `off`) turn it on or off per language; off removes the server's colors at once.
//! - **Unnecessary code**: a diagnostic tagged `Unnecessary` (an unused `using`, an unread assignment) fades its range
//!   to [`UNNECESSARY_OPACITY`] whatever its severity ([`unnecessary_decorations`], layer [`UNNECESSARY_LAYER`]).

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::time::Instant;

use eludite_editor::text::{self, PointUtf16, Unclipped};
use eludite_editor::{Decoration, DecorationStyle, SemanticSpan, SemanticUpdate};
use eludite_lsp::lsp;
use eludite_lsp::semantic::{Classifier, Modifiers, Provider, TokenClass, apply_edits, decode};
use eludite_protocol::RequestType as _;
use eludite_ui::SemanticColors;
use gpui::{Context, Rgba, Task};

use super::Shell;
use super::documents::trace;
use super::intellisense::Provider as Source;
use super::servers::ServerKey;
use super::session::{Reply, RequestError, RequestHandle};

/// Languages with an `editor.languages.<id>.semanticHighlighting` override (TSX and JSX files follow TypeScript and
/// JavaScript; F# signature files and scripts follow F#).
pub const LANGUAGES: [&str; 6] = ["csharp", "vb", "fsharp", "rust", "typescript", "javascript"];

/// Above this many lines the visible rows are asked for first, when the server serves ranges.
pub const LARGE_DOCUMENT: u32 = 2000;

/// Lines around the visible ones a first range request covers.
pub const RANGE_MARGIN: u32 = 100;

/// The decoration layer of unnecessary code.
pub const UNNECESSARY_LAYER: &str = "unnecessary";

/// The opacity of unnecessary code (Visual Studio fades it to about half).
pub const UNNECESSARY_OPACITY: f32 = 0.55;

/// The LSP language a setting is read for (`typescriptreact` follows `typescript`).
pub fn setting_language(language_id: &str) -> &str {
    match language_id {
        "typescriptreact" => "typescript",
        "javascriptreact" => "javascript",
        "fsharp-signature" => "fsharp",
        other => other,
    }
}

/// An `on` setting with per-language `default` / `on` / `off` overrides.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LanguageSwitch {
    pub enabled: bool,
    pub languages: BTreeMap<String, String>,
}

impl LanguageSwitch {
    pub fn on(&self, language_id: &str) -> bool {
        match self
            .languages
            .get(setting_language(language_id))
            .map(String::as_str)
        {
            Some("on") => true,
            Some("off") => false,
            _ => self.enabled,
        }
    }
}

/// The color and strike of a token, or `None` when it leaves the syntax color untouched.
pub fn style_for(
    class: Option<TokenClass>,
    modifiers: Modifiers,
    colors: &SemanticColors,
) -> Option<(Option<Rgba>, bool)> {
    use TokenClass::*;
    let color = class.and_then(|c| match c {
        Class | Record | Delegate | Module | Type | Attribute => colors.class,
        Interface => colors.interface,
        Enum => colors.enumeration,
        Struct | RecordStruct => colors.structure,
        TypeParameter => colors.type_parameter,
        Method | ExtensionMethod | Function => colors.method,
        OverloadedOperator => colors.overloaded_operator,
        Local | Parameter => colors.local,
        Property | Field | Event | EnumMember | Constant | Namespace | Label => colors.identifier,
        Keyword | BuiltinType | SelfKeyword => colors.keyword,
        ControlKeyword => colors.control_keyword,
        String | JsonString => colors.string,
        VerbatimString => colors.verbatim_string,
        StringEscape => colors.string_escape,
        Number | JsonNumber => colors.number,
        Comment | JsonComment | RegexComment => {
            if c == RegexComment {
                colors.regex_comment
            } else {
                colors.comment
            }
        }
        ExcludedCode => colors.excluded_code,
        RegexCharacterClass => colors.regex_character_class,
        RegexAnchor | RegexQuantifier => colors.regex_anchor,
        RegexGrouping | RegexAlternation => colors.regex_grouping,
        RegexText | RegexSelfEscapedCharacter => colors.regex_text,
        RegexOtherEscape => colors.regex_other_escape,
        JsonKeyword => colors.keyword,
        JsonPropertyName => colors.json_property,
        XmlDocComment => colors.xml_doc_comment,
        XmlDocCommentText => colors.xml_doc_comment_text,
        // The syntax colors already tell an operator, a lifetime, a macro and JSON punctuation apart.
        Operator | Lifetime | Macro | JsonPunctuation => None,
    });
    let strike = modifiers.contains(Modifiers::DEPRECATED);
    (color.is_some() || strike).then_some((color, strike))
}

/// Decode `data` (in the server's legend) into rows of spans on `snapshot`, the text the request was made on.
/// `rows` is the range the answer covers. Runs off the UI thread.
pub fn build_update(
    snapshot: &text::BufferSnapshot,
    data: &[u32],
    classifier: &Classifier,
    colors: &SemanticColors,
    rows: Range<u32>,
) -> SemanticUpdate {
    let mut out: Vec<(u32, Vec<SemanticSpan>)> = Vec::new();
    let max_row = snapshot.max_point().row;
    for token in decode(data) {
        if token.line > max_row || token.length == 0 {
            continue;
        }
        let (class, modifiers) = classifier.classify(&token);
        let Some((color, strikethrough)) = style_for(class, modifiers, colors) else {
            continue;
        };
        let start = snapshot.clip_point_utf16(
            Unclipped(PointUtf16::new(token.line, token.start)),
            text::Bias::Left,
        );
        let end = snapshot.clip_point_utf16(
            Unclipped(PointUtf16::new(token.line, token.start + token.length)),
            text::Bias::Right,
        );
        let (start, end) = (
            snapshot.point_utf16_to_point(start),
            snapshot.point_utf16_to_point(end),
        );
        if start.row != token.line || start.column >= end.column {
            continue;
        }
        let span = SemanticSpan {
            start: start.column,
            end: end.column,
            color,
            strikethrough,
        };
        match out.last_mut() {
            Some((r, spans)) if *r == token.line => spans.push(span),
            _ => out.push((token.line, vec![span])),
        }
    }
    SemanticUpdate { rows, spans: out }
}

/// Faded decorations for the diagnostics tagged `Unnecessary` (tag 1), anchored in `snapshot`.
pub fn unnecessary_decorations(
    snapshot: &text::BufferSnapshot,
    diagnostics: &[lsp::Diagnostic],
) -> Vec<Decoration> {
    let offset = |p: lsp::Position| {
        let point = snapshot.clip_point_utf16(
            Unclipped(PointUtf16::new(p.line, p.character)),
            text::Bias::Left,
        );
        snapshot.point_utf16_to_offset(point)
    };
    diagnostics
        .iter()
        .filter(|d| {
            d.extra
                .get("tags")
                .and_then(|t| t.as_array())
                .is_some_and(|t| t.iter().any(|v| v.as_u64() == Some(1)))
        })
        .filter_map(|d| {
            let (start, end) = (offset(d.range.start), offset(d.range.end));
            (start < end).then(|| Decoration {
                range: snapshot.anchor_before(start)..snapshot.anchor_after(end),
                style: DecorationStyle::Faded(UNNECESSARY_OPACITY),
            })
        })
        .collect()
}

struct Pending {
    handle: RequestHandle,
    ticket: u64,
    _task: Task<()>,
}

/// One document's tokens.
#[derive(Default)]
pub struct DocTokens {
    request: Option<Pending>,
    /// The last whole-document answer, for `full/delta`: its server generation, `resultId` and integers.
    previous: Option<(u64, String, Vec<u32>)>,
    /// The methods asked, in order (tests and traces).
    pub methods: Vec<&'static str>,
}

/// When the steps of one answer happened (the report's numbers).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SemanticTiming {
    pub method: &'static str,
    pub tokens: usize,
    pub received: Instant,
    /// Decoded off the UI thread.
    pub decoded: Instant,
    pub applied: Instant,
    /// Time on the UI thread to install the rows.
    pub ui_micros: u128,
}

/// The shell's semantic highlighting state.
#[derive(Default)]
pub struct SemanticState {
    pub settings: LanguageSwitch,
    pub docs: HashMap<String, DocTokens>,
    /// Roslyn's provider, from `eludite/languageServer/status`.
    host_provider: Option<Provider>,
    pub timings: Vec<SemanticTiming>,
}

/// What a request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    Full,
    Delta,
    /// The visible rows first; `then_full` asks for the document after.
    Range {
        then_full: bool,
    },
}

impl Shell {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn semantic(&self) -> &SemanticState {
        &self.semantic
    }

    /// The semantic token provider of document `id`'s (primary) server.
    fn semantic_provider(&self, id: &str) -> Option<Provider> {
        let doc = self.documents.get(id)?;
        match &doc.server {
            ServerKey::Host => self.semantic.host_provider.clone(),
            _ => doc
                .session
                .shared()
                .capabilities
                .as_ref()
                .and_then(Provider::from_capabilities),
        }
    }

    fn semantic_wanted(&self, id: &str) -> bool {
        match self.documents.get(id) {
            Some(d) if !d.read_only => d
                .language_id
                .as_deref()
                .is_some_and(|l| self.semantic.settings.on(l)),
            _ => false,
        }
    }

    /// Roslyn's capabilities arrived (brief 0066): its legend.
    pub(super) fn semantic_host_capabilities(&mut self, caps: &serde_json::Value) {
        self.semantic.host_provider = Provider::from_capabilities(caps);
    }

    /// A document opened: its editor shows the server's colors when the settings say so.
    pub(super) fn semantic_document_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        let on = self.semantic_wanted(id);
        if let Some(view) = self.documents.get(id).map(|d| d.view.clone()) {
            view.update(cx, |v, cx| v.set_semantic_highlighting_enabled(on, cx));
        }
    }

    pub(super) fn semantic_document_closed(&mut self, id: &str) {
        if let Some(mut d) = self.semantic.docs.remove(id)
            && let Some(p) = d.request.take()
        {
            p.handle.cancel();
        }
    }

    /// `editor.semanticHighlighting` and its per-language overrides.
    pub(super) fn semantic_apply_settings(&mut self, cx: &mut Context<Self>) {
        let next = {
            let s = self.settings.lock();
            LanguageSwitch {
                enabled: s.bool("editor.semanticHighlighting"),
                languages: LANGUAGES
                    .iter()
                    .map(|l| {
                        (
                            (*l).to_owned(),
                            s.string(&format!("editor.languages.{l}.semanticHighlighting")),
                        )
                    })
                    .collect(),
            }
        };
        if next == self.semantic.settings {
            return;
        }
        self.semantic.settings = next;
        let ids: Vec<String> = self.documents.keys().cloned().collect();
        for id in ids {
            let on = self.semantic_wanted(&id);
            let view = self.documents[&id].view.clone();
            view.update(cx, |v, cx| v.set_semantic_highlighting_enabled(on, cx));
            if !on {
                self.semantic_document_closed(&id);
            }
        }
    }

    /// The server of some documents changed (a refresh, a new generation, its capabilities, its load): their tokens
    /// are asked for again.
    pub(super) fn semantic_server_changed(&mut self, server: &ServerKey, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .documents
            .iter()
            .filter(|(_, d)| &d.server == server || d.servers.contains(server))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(d) = self.semantic.docs.get_mut(&id)
                && let Some(p) = d.request.take()
            {
                p.handle.cancel();
            }
            let view = self.documents[&id].view.clone();
            view.update(cx, |v, cx| v.refresh_semantic_tokens(cx));
        }
    }

    /// [`eludite_editor::EditorEvent::SemanticTokensRequested`]: ask the document's server for its tokens.
    pub(super) fn request_semantic_tokens(
        &mut self,
        id: &str,
        request: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(provider) = self.semantic_provider(id) else {
            return; // No provider: the syntax colors stay.
        };
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        if doc.language_id.is_none()
            || doc.read_only
            || self.provider(doc) == Source::Syntax
            || !self.semantic_wanted(id)
        {
            return;
        }
        let shown = doc.view.read(cx).semantic_rows().span_count();
        let generation = self.doc_generation(id);
        let lines = doc.view.read(cx).editor().buffer().line_count();
        let state = self.semantic.docs.entry(id.to_owned()).or_default();
        let ask = match &state.previous {
            Some((g, _, _)) if provider.delta && *g == generation => Ask::Delta,
            _ if provider.range && (!provider.full || (lines > LARGE_DOCUMENT && shown == 0)) => {
                Ask::Range {
                    then_full: provider.full,
                }
            }
            _ => Ask::Full,
        };
        self.send_semantic_request(id, request, ask, cx);
    }

    fn send_semantic_request(&mut self, id: &str, request: u64, ask: Ask, cx: &mut Context<Self>) {
        let Some(provider) = self.semantic_provider(id) else {
            return;
        };
        self.flush_change(id, cx);
        let generation = self.doc_generation(id);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let snapshot = doc.sent.clone();
        let version = doc.lsp_version;
        let text_document = lsp::TextDocumentIdentifier {
            uri: doc.uri.clone(),
        };
        let session = doc.session.clone();
        let line_count = snapshot.max_point().row + 1;
        let rows = match ask {
            Ask::Range { then_full: true } => {
                let visible = doc.view.read(cx).visible_rows();
                visible.start.saturating_sub(RANGE_MARGIN)
                    ..visible.end.saturating_add(RANGE_MARGIN).min(line_count)
            }
            _ => 0..line_count,
        };
        let state = self.semantic.docs.entry(id.to_owned()).or_default();
        if let Some(p) = state.request.take() {
            p.handle.cancel();
        }
        let (method, handle, rx): (&'static str, RequestHandle, AnswerFuture) = match ask {
            Ask::Full => {
                let (h, rx) =
                    session.request::<lsp::SemanticTokensFull>(lsp::SemanticTokensParams {
                        text_document,
                    });
                (
                    lsp::SemanticTokensFull::METHOD,
                    h,
                    Box::pin(async move {
                        rx.await
                            .ok()
                            .map(|r| map_reply(r, |t| t.map(Answer::Tokens)))
                    }) as AnswerFuture,
                )
            }
            Ask::Delta => {
                let previous = state
                    .previous
                    .as_ref()
                    .map(|(_, r, _)| r.clone())
                    .unwrap_or_default();
                let (h, rx) = session.request::<lsp::SemanticTokensFullDelta>(
                    lsp::SemanticTokensDeltaParams {
                        text_document,
                        previous_result_id: previous,
                    },
                );
                (
                    lsp::SemanticTokensFullDelta::METHOD,
                    h,
                    Box::pin(async move {
                        rx.await.ok().map(|r| {
                            map_reply(r, |d| {
                                d.map(|d| match d {
                                    lsp::SemanticTokensDeltaResult::Delta(d) => Answer::Delta(d),
                                    lsp::SemanticTokensDeltaResult::Tokens(t) => Answer::Tokens(t),
                                })
                            })
                        })
                    }) as AnswerFuture,
                )
            }
            Ask::Range { .. } => {
                let range = lsp::Range {
                    start: lsp::Position {
                        line: rows.start,
                        character: 0,
                    },
                    end: lsp::Position {
                        line: rows.end,
                        character: 0,
                    },
                };
                let (h, rx) =
                    session.request::<lsp::SemanticTokensRange>(lsp::SemanticTokensRangeParams {
                        text_document,
                        range,
                    });
                (
                    lsp::SemanticTokensRange::METHOD,
                    h,
                    Box::pin(async move {
                        rx.await
                            .ok()
                            .map(|r| map_reply(r, |t| t.map(Answer::Range)))
                    }) as AnswerFuture,
                )
            }
        };
        state.methods.push(method);
        trace(format_args!(
            "{method} request {request} for {} version {version}",
            session_uri(&self.documents[id].uri)
        ));
        let doc_id = id.to_owned();
        let classifier = Classifier::new(&provider.legend);
        let colors = self.theme.semantic;
        let task = cx.spawn(async move |this, cx| {
            let Some(reply) = rx.await else {
                return;
            };
            let received = Instant::now();
            // Check staleness and build the integers on the UI thread (cheap), decode on a background thread.
            let Ok(Some(data)) = this.update(cx, |shell, cx| {
                shell.semantic_reply_data(
                    &doc_id, request, generation, version, ask, rows, reply, cx,
                )
            }) else {
                return;
            };
            let (data, covered) = data;
            let tokens = data.len() / 5;
            let decoded = cx
                .background_executor()
                .spawn(async move {
                    let update = build_update(&snapshot, &data, &classifier, &colors, covered);
                    (update, Instant::now())
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.apply_semantic(
                    &doc_id, request, generation, version, method, ask, tokens, received, decoded,
                    cx,
                )
            });
        });
        self.semantic.docs.entry(id.to_owned()).or_default().request = Some(Pending {
            handle,
            ticket: request,
            _task: task,
        });
    }

    /// The integers an answer stands for (a delta applied to the kept ones) and the rows they cover, or `None` when
    /// it is stale, failed or empty.
    #[allow(clippy::too_many_arguments)]
    fn semantic_reply_data(
        &mut self,
        id: &str,
        request: u64,
        generation: u64,
        version: i32,
        ask: Ask,
        rows: Range<u32>,
        reply: Result<Option<Answer>, RequestError>,
        cx: &mut Context<Self>,
    ) -> Option<(Vec<u32>, Range<u32>)> {
        let current = self.doc_generation(id);
        let doc_version = self.documents.get(id)?.lsp_version;
        let line_count = self.documents.get(id)?.sent.max_point().row + 1;
        let state = self.semantic.docs.get_mut(id)?;
        if state.request.as_ref().is_none_or(|p| p.ticket != request) {
            return None; // Superseded.
        }
        if generation != current || version != doc_version {
            trace(format_args!(
                "semantic tokens {request} dropped (stale: generation {generation}/{current}, version {version}/{doc_version})"
            ));
            state.request = None;
            return None;
        }
        let answer = match reply {
            Ok(Some(a)) => a,
            Ok(None) => {
                state.request = None;
                return None;
            }
            Err(RequestError::Canceled | RequestError::Stale) => return None,
            Err(e) => {
                trace(format_args!("semantic tokens {request} failed: {e:?}"));
                state.request = None;
                if ask == Ask::Delta {
                    // Ask for the whole document instead.
                    state.previous = None;
                    self.send_semantic_request(id, request, Ask::Full, cx);
                }
                return None;
            }
        };
        match answer {
            Answer::Tokens(t) => {
                state.previous = t.result_id.map(|r| (generation, r, t.data.clone()));
                Some((t.data, 0..line_count))
            }
            Answer::Delta(d) => {
                let Some((g, _, mut data)) = state.previous.take() else {
                    self.send_semantic_request(id, request, Ask::Full, cx);
                    return None;
                };
                if g != generation || apply_edits(&mut data, &d.edits).is_err() {
                    trace(format_args!(
                        "semantic tokens {request}: the delta does not fit; asking for all"
                    ));
                    self.send_semantic_request(id, request, Ask::Full, cx);
                    return None;
                }
                state.previous = d.result_id.map(|r| (generation, r, data.clone()));
                Some((data, 0..line_count))
            }
            Answer::Range(t) => Some((t.data, rows)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_semantic(
        &mut self,
        id: &str,
        request: u64,
        generation: u64,
        version: i32,
        method: &'static str,
        ask: Ask,
        tokens: usize,
        received: Instant,
        (update, decoded): (SemanticUpdate, Instant),
        cx: &mut Context<Self>,
    ) {
        let current = self.doc_generation(id);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let (view, doc_version) = (doc.view.clone(), doc.lsp_version);
        let Some(state) = self.semantic.docs.get_mut(id) else {
            return;
        };
        if state.request.as_ref().is_none_or(|p| p.ticket != request) {
            return;
        }
        state.request = None;
        if generation != current || version != doc_version {
            return; // Stale: never painted.
        }
        let start = Instant::now();
        let applied = view.update(cx, |v, cx| v.set_semantic_tokens(request, update, cx));
        let ui_micros = start.elapsed().as_micros();
        trace(format_args!(
            "{method} reply {request}: {tokens} tokens{}",
            if applied {
                ""
            } else {
                ", dropped by the editor"
            }
        ));
        if applied {
            self.semantic.timings.push(SemanticTiming {
                method,
                tokens,
                received,
                decoded,
                applied: Instant::now(),
                ui_micros,
            });
            if self.semantic.timings.len() > 1024 {
                self.semantic.timings.remove(0);
            }
        }
        if applied && let Ask::Range { then_full: true } = ask {
            // The visible rows are in: now the whole document, for the same editor request.
            self.send_semantic_request(id, request, Ask::Full, cx);
        }
    }
}

/// An answer on its way: `None` when the session went away.
type AnswerFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Option<Result<Option<Answer>, RequestError>>>>,
>;

/// What an answer holds.
enum Answer {
    Tokens(lsp::SemanticTokens),
    Delta(lsp::SemanticTokensDelta),
    Range(lsp::SemanticTokens),
}

fn session_uri(uri: &str) -> &str {
    uri
}

fn map_reply<T>(
    reply: Reply<T>,
    f: impl FnOnce(T) -> Option<Answer>,
) -> Result<Option<Answer>, RequestError> {
    reply.result.map(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_lsp::semantic::Legend;
    use gpui::rgb;

    #[test]
    fn tokens_decode_to_byte_columns_and_theme_colors() {
        // A UTF-16 column after a non-ASCII character becomes a byte column.
        let text = "var é = new List();\nFoo(é);\n";
        let buffer = text::Buffer::new(
            text::ReplicaId::LOCAL,
            text::BufferId::new(1).unwrap(),
            text,
        );
        let snapshot = buffer.snapshot();
        let legend = Legend {
            token_types: vec![
                "variable".into(),
                "class".into(),
                "method".into(),
                "punctuation".into(),
            ],
            token_modifiers: vec!["static".into(), "deprecated".into()],
        };
        let classifier = Classifier::new(&legend);
        let colors = eludite_ui::Theme::dark().semantic;
        // `é` (local) at 4, `List` (class) at 12, then `Foo` (deprecated method) and `(` (punctuation) on row 1.
        let data = [0, 4, 1, 0, 0, 0, 8, 4, 1, 0, 1, 0, 3, 2, 2, 0, 3, 1, 3, 0];
        let update = build_update(snapshot, &data, &classifier, &colors, 0..3);
        assert_eq!(update.rows, 0..3);
        assert_eq!(
            update.spans,
            vec![
                (
                    0,
                    vec![
                        SemanticSpan {
                            start: 4,
                            end: 6,
                            color: Some(rgb(0x9CDCFE)),
                            strikethrough: false
                        },
                        SemanticSpan {
                            start: 13,
                            end: 17,
                            color: Some(rgb(0x4EC9B0)),
                            strikethrough: false
                        },
                    ]
                ),
                (
                    1,
                    vec![SemanticSpan {
                        start: 0,
                        end: 3,
                        color: Some(rgb(0xDCDCAA)),
                        strikethrough: true
                    }]
                ),
            ]
        );
    }

    #[test]
    fn per_language_switches() {
        let mut s = LanguageSwitch {
            enabled: true,
            languages: BTreeMap::new(),
        };
        assert!(s.on("csharp"));
        s.languages.insert("typescript".into(), "off".into());
        assert!(!s.on("typescriptreact"));
        s.enabled = false;
        s.languages.insert("rust".into(), "on".into());
        assert!(s.on("rust") && !s.on("csharp"));
    }

    #[test]
    fn unnecessary_diagnostics_fade_their_range() {
        let buffer = text::Buffer::new(
            text::ReplicaId::LOCAL,
            text::BufferId::new(1).unwrap(),
            "using System.Linq;\nclass A {}\n",
        );
        let snapshot = buffer.snapshot();
        let d = |tags: serde_json::Value| -> lsp::Diagnostic {
            serde_json::from_value(serde_json::json!({
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 18}},
                "severity": 4, "message": "Unnecessary using directive.", "tags": tags
            }))
            .unwrap()
        };
        let faded = unnecessary_decorations(
            snapshot,
            &[d(serde_json::json!([1])), d(serde_json::json!([2]))],
        );
        assert_eq!(faded.len(), 1);
        assert_eq!(faded[0].style, DecorationStyle::Faded(UNNECESSARY_OPACITY));
        assert_eq!(snapshot.offset_for_anchor(&faded[0].range.end), 18);
    }
}
