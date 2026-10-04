//! CodeLens (brief 0052): Visual Studio's indicators above members, from the document's language server.
//!
//! - **Requests.** The editor says when a document's lenses are wanted (opened, 150 ms after the last edit, a refresh)
//!   and which unresolved ones came near the visible range. The shell sends `textDocument/codeLens` through the
//!   document's server session after a pending `didChange` (one in flight per document; a newer one cancels the
//!   older), and `codeLens/resolve` for each lens the editor asks for, at most [`RESOLVE_CONCURRENCY`] at a time. An
//!   answer is dropped when its server generation or the document's version moved since (CLAUDE.md invariant 12); a
//!   resolve is dropped when the document's lenses were replaced since. A server without `codeLensProvider` is never
//!   asked and contributes nothing.
//! - **What each lens does** comes from its command ([`eludite_lsp::codelens::classify`]): the References popup (the
//!   locations as given, or `textDocument/references` without the declaration at the symbol), or Run Test and Debug
//!   Test through the Test Explorer's model (`eludite.test.run` and `eludite.test.debug` with `ids`). A lens with a
//!   command the shell does not know is not shown. A test lens shows the last outcome's glyph and duration; a test not
//!   discovered yet is discovered first, the lens reading "Discovering…".
//! - **Refresh.** `workspace/codeLens/refresh` (relayed by the host as `eludite/codeLens/refresh`), a new generation,
//!   a server's capabilities arriving and the solution finishing its load ask for the open documents' lenses again.
//! - **Settings** (`editor.codeLens`, `editor.codeLens.references`, `editor.codeLens.tests`,
//!   `editor.languages.<id>.codeLens`) choose which indicators show per language; a change asks again, and turning them
//!   off removes the rows in one reflow.
//!
//! The References popup ([`ReferencesPopup`]) lists the locations grouped by file with a preview line (the rows of
//! brief 0014's Find All References, read off the UI thread), virtualized; Up, Down and Enter navigate, Escape closes.
//! Agents get the same answers through `eludite.editor.find_references` and `eludite.test.*`: nothing here is new to
//! them.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use eludite_commands::test as test_cmds;
use eludite_editor::{CodeLens, text};
use eludite_lsp::codelens::{self, LensCommand, LensKind, LensTarget};
use eludite_lsp::lsp;
use eludite_ui::{TREE_ROW_HEIGHT, TestGlyph, Theme, TreeRowStyle, highlighted_code, tree_row};
use futures::channel::oneshot;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Point, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Task,
    UniformListScrollHandle, Window, anchored, deferred, div, point, px, uniform_list,
};
use serde_json::json;

use super::Shell;
use super::documents::{normalize_path, trace};
use super::intellisense::{Provider, lsp_position, offset_in};
use super::navigation::document_title;
use super::references::{Reference, build_references};
use super::servers::ServerKey;
use super::session::{Reply, RequestError, RequestHandle};
use super::test_runs::{Phase, TestNode, TestRuns};
use super::tests_window::glyph_of;

/// Languages with an `editor.languages.<id>.codeLens` override (TSX and JSX files follow TypeScript and JavaScript).
pub const LANGUAGES: [&str; 4] = ["csharp", "rust", "typescript", "javascript"];

/// Resolves in flight per document at most; the rest wait their turn.
pub const RESOLVE_CONCURRENCY: usize = 8;

/// What a test lens reads while its run waits for the Test Explorer's discovery.
pub const DISCOVERING: &str = "Discovering\u{2026}";

/// What a references lens reads until it is resolved (the pinned Roslyn's own placeholder).
pub const UNRESOLVED: &str = "- references";

/// Which indicators show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LensFilter {
    pub references: bool,
    pub tests: bool,
}

impl LensFilter {
    pub const NONE: Self = Self {
        references: false,
        tests: false,
    };

    pub fn any(self) -> bool {
        self.references || self.tests
    }

    pub fn shows(self, kind: LensKind) -> bool {
        if kind.is_test() {
            self.tests
        } else {
            self.references
        }
    }
}

/// The CodeLens settings.
#[derive(Debug, Clone, PartialEq)]
pub struct LensSettings {
    pub enabled: bool,
    pub references: bool,
    pub tests: bool,
    /// `editor.languages.<id>.codeLens`: `default`, `on`, `references`, `tests` or `off`.
    pub languages: BTreeMap<String, String>,
}

impl Default for LensSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            references: true,
            tests: true,
            languages: BTreeMap::new(),
        }
    }
}

impl LensSettings {
    /// The indicators a document of LSP language `language_id` shows.
    pub fn filter(&self, language_id: &str) -> LensFilter {
        let language = match language_id {
            "typescriptreact" => "typescript",
            "javascriptreact" => "javascript",
            other => other,
        };
        let (references, tests) = match self.languages.get(language).map(String::as_str) {
            Some("on") => (true, true),
            Some("references") => (true, false),
            Some("tests") => (false, true),
            Some("off") => (false, false),
            _ if self.enabled => (self.references, self.tests),
            _ => (false, false),
        };
        LensFilter { references, tests }
    }
}

/// One lens of a document, as the server sent it and as the shell understands it.
#[derive(Debug, Clone)]
pub struct LensEntry {
    /// As received (sent back unchanged in `codeLens/resolve`).
    pub lens: lsp::CodeLens,
    pub command: Option<LensCommand>,
    pub kind: LensKind,
    /// A run from this lens waits for the Test Explorer's discovery.
    pub discovering: bool,
}

struct Pending {
    handle: RequestHandle,
    /// The editor's request id it answers (the lens id for a resolve).
    ticket: u64,
    _task: Task<()>,
}

/// One document's lenses.
#[derive(Default)]
pub struct DocLenses {
    request: Option<Pending>,
    /// By the editor's lens id.
    pub entries: BTreeMap<u64, LensEntry>,
    next_id: u64,
    /// The server generation and document version the entries were computed on.
    answered: Option<(u64, i32)>,
    resolving: HashMap<u64, Pending>,
    queue: VecDeque<u64>,
}

impl DocLenses {
    fn cancel(&mut self) {
        if let Some(p) = self.request.take() {
            p.handle.cancel();
        }
        self.cancel_resolves();
    }

    fn cancel_resolves(&mut self) {
        for (_, p) in self.resolving.drain() {
            p.handle.cancel();
        }
        self.queue.clear();
    }
}

/// When the steps of one lens request happened (the report's budget numbers).
#[derive(Debug, Clone, Default)]
// Read by the tests and the report's measurements.
#[allow(dead_code)]
pub struct LensTiming {
    /// The editor asked (after the debounce, or at once on open and refresh).
    pub requested: Option<Instant>,
    /// Written to the server, and its answer read.
    pub sent: Option<Instant>,
    pub received: Option<Instant>,
    /// Handed to the editor.
    pub applied: Option<Instant>,
    pub lenses: usize,
}

/// When the References popup opened and filled.
#[derive(Debug, Clone, Default)]
// Read by the tests and the report's measurements.
#[allow(dead_code)]
pub struct PopupTiming {
    pub activated: Option<Instant>,
    /// The popup exists, showing its header (searching, or the given locations).
    pub opened: Option<Instant>,
    /// Its rows are in.
    pub filled: Option<Instant>,
    pub rows: usize,
}

struct WaitingRun {
    doc: String,
    tx: oneshot::Sender<()>,
}

/// The shell's CodeLens state.
#[derive(Default)]
pub struct CodeLensState {
    pub settings: LensSettings,
    pub docs: HashMap<String, DocLenses>,
    pub popup: Option<Entity<ReferencesPopup>>,
    /// The document the popup belongs to.
    popup_doc: Option<String>,
    popup_request: Option<Pending>,
    popup_ticket: u64,
    waiting: Vec<WaitingRun>,
    pub timings: Vec<LensTiming>,
    pub popup_timings: Vec<PopupTiming>,
}

const MAX_TIMINGS: usize = 4096;

fn push_capped<T>(v: &mut Vec<T>, item: T) {
    if v.len() >= MAX_TIMINGS {
        v.remove(0);
    }
    v.push(item);
}

/// The order of indicators on a row: references, implementations, Run Test, Debug Test.
fn order(kind: LensKind) -> u8 {
    match kind {
        LensKind::References => 0,
        LensKind::Implementations => 1,
        LensKind::RunTest => 2,
        LensKind::DebugTest => 3,
    }
}

/// A command title in Visual Studio's words: rust-analyzer's `▶︎ Run Test` and `Debug` become `Run Test` and
/// `Debug Test`.
fn display_title(c: &LensCommand) -> String {
    let t = c
        .title
        .trim_start_matches(|ch: char| !ch.is_alphanumeric() && ch != '-')
        .trim();
    match (c.kind, &c.target) {
        (LensKind::DebugTest, LensTarget::CargoTest { exact: true, .. }) if t == "Debug" => {
            "Debug Test".into()
        }
        (LensKind::DebugTest, LensTarget::CargoTest { exact: false, .. }) if t == "Debug" => {
            "Debug Tests".into()
        }
        _ => t.to_owned(),
    }
}

/// `12 ms`, `1.2 s`.
pub fn duration_text(ms: f64) -> String {
    if ms < 1.0 {
        "< 1 ms".into()
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// The method of a fully qualified test name (`Ns.Class.Method(Int32)` gives `Method`).
fn method_name(full: &str) -> &str {
    let full = full.split('(').next().unwrap_or(full);
    full.rsplit('.').next().unwrap_or(full)
}

/// The class of a fully qualified test name.
fn class_name(full: &str) -> &str {
    let full = full.split('(').next().unwrap_or(full);
    let mut parts = full.rsplit('.');
    parts.next();
    parts.next().unwrap_or_default()
}

/// The discovered tests a test lens of the document at `path` stands for, by the Test Explorer's model: a .NET
/// lens's member is a test method (all its data rows) or, for Run All Tests, a class, in that file; a Cargo lens's
/// libtest name is one test or a module, in the package holding the file.
pub fn lens_test_ids(tests: &TestRuns, path: &Path, target: &LensTarget) -> Vec<String> {
    let path = normalize_path(path);
    match target {
        LensTarget::Test { member, .. } => {
            let in_file = |t: &TestNode| {
                t.source
                    .as_deref()
                    .is_some_and(|s| normalize_path(Path::new(s)) == path)
            };
            let method = |t: &TestNode| method_name(&t.full_name) == member;
            let class = |t: &TestNode| {
                class_name(&t.full_name) == member || t.group.last().is_some_and(|g| g == member)
            };
            let pick = |f: &dyn Fn(&TestNode) -> bool| -> Vec<String> {
                tests
                    .tests
                    .iter()
                    .filter(|t| f(t))
                    .map(|t| t.id.clone())
                    .collect()
            };
            let ids = pick(&|t| in_file(t) && method(t));
            if !ids.is_empty() {
                return ids;
            }
            let ids = pick(&|t| in_file(t) && class(t));
            if !ids.is_empty() {
                return ids;
            }
            // Some VSTest adapters report no source location: the method by name alone.
            pick(&|t| t.source.is_none() && method(t))
        }
        LensTarget::CargoTest {
            name,
            exact,
            package,
        } => tests
            .tests
            .iter()
            .filter(|t| {
                let Some((_, libtest)) = &t.cargo else {
                    return false;
                };
                let named =
                    libtest == name || (!exact && libtest.starts_with(&format!("{name}::")));
                let Some(p) = tests.projects.iter().find(|p| p.key == t.project) else {
                    return false;
                };
                let in_package = package
                    .as_ref()
                    .is_none_or(|n| p.cargo.as_ref().is_some_and(|(pn, _)| pn == n));
                let holds_file = Path::new(&p.path)
                    .parent()
                    .is_some_and(|d| path.starts_with(normalize_path(d)));
                named && in_package && holds_file
            })
            .map(|t| t.id.clone())
            .collect(),
        LensTarget::References { .. } => Vec::new(),
    }
}

/// What a lens shows: its title, and for Run Test the last outcome's glyph and duration (from the Test Explorer's
/// model).
fn decorate(tests: &TestRuns, path: &Path, entry: &LensEntry) -> (String, Option<TestGlyph>) {
    let Some(c) = &entry.command else {
        return (UNRESOLVED.into(), None);
    };
    if entry.discovering {
        return (DISCOVERING.into(), None);
    }
    let title = display_title(c);
    if c.kind != LensKind::RunTest {
        return (title, None);
    }
    let ids = lens_test_ids(tests, path, &c.target);
    let results: Vec<_> = ids
        .iter()
        .filter_map(|id| tests.test(id)?.result.as_ref())
        .collect();
    if results.is_empty() {
        return (title, None);
    }
    let glyph = TestGlyph::aggregate(ids.iter().map(|id| {
        glyph_of(
            tests
                .test(id)
                .map_or(test_cmds::Outcome::NotRun, |t| t.outcome()),
        )
    }));
    let durations: Vec<f64> = results.iter().filter_map(|r| r.duration_ms).collect();
    let title = if durations.is_empty() {
        title
    } else {
        format!("{title} ({})", duration_text(durations.iter().sum()))
    };
    (title, Some(glyph))
}

impl Shell {
    /// The CodeLens state (tests and the timing harness).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn code_lens(&self) -> &CodeLensState {
        &self.code_lens
    }

    /// The indicators document `id` shows (none for a read-only document or a file no server handles).
    fn lens_filter(&self, id: &str) -> LensFilter {
        match self.documents.get(id) {
            Some(d) if !d.read_only => d
                .language_id
                .as_deref()
                .map_or(LensFilter::NONE, |l| self.code_lens.settings.filter(l)),
            _ => LensFilter::NONE,
        }
    }

    /// A document opened: its editor shows lens rows when the settings show any for its language.
    pub(super) fn code_lens_document_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        let on = self.lens_filter(id).any();
        if let Some(view) = self.documents.get(id).map(|d| d.view.clone()) {
            view.update(cx, |v, cx| v.set_code_lens_enabled(on, cx));
        }
    }

    pub(super) fn code_lens_document_closed(&mut self, id: &str) {
        if let Some(mut d) = self.code_lens.docs.remove(id) {
            d.cancel();
        }
        self.code_lens.waiting.retain(|w| w.doc != id);
        if self.code_lens.popup_doc.as_deref() == Some(id) {
            self.code_lens.popup = None;
            self.code_lens.popup_doc = None;
        }
    }

    /// Apply the CodeLens settings: documents turn their rows on or off, and the ones that show lenses ask again.
    pub(super) fn code_lens_apply_settings(&mut self, cx: &mut Context<Self>) {
        let next = {
            let s = self.settings.lock();
            LensSettings {
                enabled: s.bool("editor.codeLens"),
                references: s.bool("editor.codeLens.references"),
                tests: s.bool("editor.codeLens.tests"),
                languages: LANGUAGES
                    .iter()
                    .map(|l| {
                        (
                            (*l).to_owned(),
                            s.string(&format!("editor.languages.{l}.codeLens")),
                        )
                    })
                    .collect(),
            }
        };
        if next == self.code_lens.settings {
            return;
        }
        self.code_lens.settings = next;
        let ids: Vec<String> = self.documents.keys().cloned().collect();
        for id in ids {
            let on = self.lens_filter(&id).any();
            let view = self.documents[&id].view.clone();
            view.update(cx, |v, cx| {
                if v.code_lens_enabled() != on {
                    v.set_code_lens_enabled(on, cx);
                } else if on {
                    v.refresh_code_lenses(cx);
                }
            });
            if !on && let Some(d) = self.code_lens.docs.get_mut(&id) {
                d.cancel();
                d.entries.clear();
            }
        }
    }

    /// The server of some documents changed (a refresh, a new generation, its capabilities, its load): their lenses
    /// are asked for again.
    pub(super) fn code_lens_server_changed(&mut self, server: &ServerKey, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .documents
            .iter()
            .filter(|(_, d)| &d.server == server)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(d) = self.code_lens.docs.get_mut(&id) {
                d.cancel();
            }
            let view = self.documents[&id].view.clone();
            view.update(cx, |v, cx| v.refresh_code_lenses(cx));
        }
    }

    /// [`eludite_editor::EditorEvent::CodeLensRequested`]: ask the document's server for its lenses.
    pub(super) fn request_code_lenses(&mut self, id: &str, request: u64, cx: &mut Context<Self>) {
        let requested = Instant::now();
        let features = self.doc_features(id);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let view = doc.view.clone();
        let usable = doc.language_id.is_some()
            && !doc.read_only
            && features.code_lens
            && self.provider(doc) != Provider::Syntax;
        let state = self.code_lens.docs.entry(id.to_owned()).or_default();
        if let Some(p) = state.request.take() {
            p.handle.cancel();
        }
        if !usable {
            // A server without lenses (or none at all) contributes nothing; nothing else degrades.
            state.cancel_resolves();
            state.entries.clear();
            state.answered = None;
            view.update(cx, |v, cx| v.set_code_lenses(request, Vec::new(), cx));
            return;
        }
        self.flush_change(id, cx);
        let generation = self.doc_generation(id);
        let doc = &self.documents[id];
        let snapshot = doc.sent.clone();
        let version = doc.lsp_version;
        let params = lsp::CodeLensParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
        };
        trace(format_args!(
            "codeLens request {request} for {} version {version}",
            doc.uri
        ));
        let (handle, rx) = doc.session.request::<lsp::CodeLensRequest>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                shell.on_code_lens_reply(
                    &doc_id, request, generation, version, snapshot, reply, requested, cx,
                )
            });
        });
        self.code_lens
            .docs
            .entry(id.to_owned())
            .or_default()
            .request = Some(Pending {
            handle,
            ticket: request,
            _task: task,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn on_code_lens_reply(
        &mut self,
        id: &str,
        request: u64,
        generation: u64,
        version: i32,
        snapshot: text::BufferSnapshot,
        reply: Reply<Option<Vec<lsp::CodeLens>>>,
        requested: Instant,
        cx: &mut Context<Self>,
    ) {
        let current = self.doc_generation(id);
        let filter = self.lens_filter(id);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let (view, path, doc_version) = (doc.view.clone(), doc.path.clone(), doc.lsp_version);
        let Some(state) = self.code_lens.docs.get_mut(id) else {
            return;
        };
        if state.request.as_ref().is_none_or(|p| p.ticket != request) {
            return; // Superseded.
        }
        state.request = None;
        if generation != current || version != doc_version {
            trace(format_args!(
                "codeLens reply {request} dropped (stale: generation {generation}/{current}, version {version}/{doc_version})"
            ));
            return;
        }
        let lenses = match reply.result {
            Ok(l) => l.unwrap_or_default(),
            Err(RequestError::Canceled | RequestError::Stale) => return,
            Err(e) => {
                trace(format_args!("codeLens reply {request} failed: {e:?}"));
                Vec::new()
            }
        };
        // A references lens shows its last count until it is resolved again, rather than a placeholder.
        let mut before: HashMap<(u32, LensKind), String> = HashMap::new();
        for shown in view.read(cx).code_lenses() {
            if shown.resolved
                && let Some(e) = state.entries.get(&shown.id)
                && !e.kind.is_test()
            {
                before.insert((shown.row, e.kind), shown.title);
            }
        }
        state.cancel_resolves();
        state.entries.clear();
        let mut rows: Vec<(u32, u8, u64, usize)> = Vec::new();
        for lens in lenses {
            let command = lens.command.as_ref().and_then(codelens::classify);
            if lens.command.is_some() && command.is_none() {
                continue; // A command the shell does not know.
            }
            let kind = command
                .as_ref()
                .map_or(codelens::UNRESOLVED_KIND, |c| c.kind);
            if !filter.shows(kind) {
                continue;
            }
            let offset = offset_in(&snapshot, lens.range.start);
            let row = snapshot.offset_to_point(offset).row;
            state.next_id += 1;
            let lens_id = state.next_id;
            rows.push((row, order(kind), lens_id, offset));
            state.entries.insert(
                lens_id,
                LensEntry {
                    lens,
                    command,
                    kind,
                    discovering: false,
                },
            );
        }
        rows.sort();
        state.answered = Some((generation, version));
        let items: Vec<CodeLens> = rows
            .into_iter()
            .map(|(row, _, lens_id, offset)| {
                let entry = &state.entries[&lens_id];
                let resolved = entry.command.is_some();
                let (title, glyph) = if resolved {
                    decorate(&self.tests, &path, entry)
                } else {
                    (
                        before
                            .get(&(row, entry.kind))
                            .cloned()
                            .unwrap_or_else(|| UNRESOLVED.into()),
                        None,
                    )
                };
                CodeLens {
                    id: lens_id,
                    offset,
                    title,
                    resolved,
                    glyph,
                }
            })
            .collect();
        let count = items.len();
        let applied = view.update(cx, |v, cx| v.set_code_lenses(request, items, cx));
        trace(format_args!(
            "codeLens reply {request}: {count} lenses{} (server {:.1} ms)",
            if applied {
                ""
            } else {
                ", dropped by the editor"
            },
            reply
                .sent
                .map_or(0., |s| (reply.received - s).as_secs_f64() * 1e3)
        ));
        push_capped(
            &mut self.code_lens.timings,
            LensTiming {
                requested: Some(requested),
                sent: reply.sent,
                received: Some(reply.received),
                applied: Some(Instant::now()),
                lenses: count,
            },
        );
        self.wake_intellisense_waiters();
    }

    /// [`eludite_editor::EditorEvent::CodeLensResolve`]: resolve these lenses, a few at a time.
    pub(super) fn resolve_code_lenses(&mut self, id: &str, ids: &[u64], cx: &mut Context<Self>) {
        if !self.doc_features(id).code_lens_resolve {
            return;
        }
        let Some(state) = self.code_lens.docs.get_mut(id) else {
            return;
        };
        for lens in ids {
            if state.entries.get(lens).is_some_and(|e| e.command.is_none())
                && !state.resolving.contains_key(lens)
                && !state.queue.contains(lens)
            {
                state.queue.push_back(*lens);
            }
        }
        self.pump_lens_resolves(id, cx);
    }

    fn pump_lens_resolves(&mut self, id: &str, cx: &mut Context<Self>) {
        let generation = self.doc_generation(id);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let session = doc.session.clone();
        let Some(state) = self.code_lens.docs.get_mut(id) else {
            return;
        };
        let Some(answered) = state.answered else {
            return;
        };
        while state.resolving.len() < RESOLVE_CONCURRENCY {
            let Some(lens_id) = state.queue.pop_front() else {
                break;
            };
            let Some(entry) = state.entries.get(&lens_id) else {
                continue;
            };
            let (handle, rx) = session.request::<lsp::ResolveCodeLens>(entry.lens.clone());
            let doc_id = id.to_owned();
            let task = cx.spawn(async move |this, cx| {
                let Ok(reply) = rx.await else {
                    return;
                };
                let _ = this.update(cx, |shell, cx| {
                    shell.on_lens_resolved(&doc_id, lens_id, answered, generation, reply, cx)
                });
            });
            state.resolving.insert(
                lens_id,
                Pending {
                    handle,
                    ticket: lens_id,
                    _task: task,
                },
            );
        }
    }

    fn on_lens_resolved(
        &mut self,
        id: &str,
        lens_id: u64,
        answered: (u64, i32),
        generation: u64,
        reply: Reply<lsp::CodeLens>,
        cx: &mut Context<Self>,
    ) {
        let current = self.doc_generation(id);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let (view, path) = (doc.view.clone(), doc.path.clone());
        let Some(state) = self.code_lens.docs.get_mut(id) else {
            return;
        };
        if state.resolving.remove(&lens_id).is_none() {
            return; // Canceled with the lenses it belonged to.
        }
        let current_lenses = state.answered == Some(answered) && generation == current;
        if current_lenses && let Some(entry) = state.entries.get_mut(&lens_id) {
            match reply.result {
                Ok(lens) => {
                    entry.command = lens.command.as_ref().and_then(codelens::classify);
                    entry.lens = lens;
                    if let Some(c) = &entry.command {
                        entry.kind = c.kind;
                    }
                    let (title, glyph) = decorate(&self.tests, &path, entry);
                    view.update(cx, |v, cx| {
                        v.update_code_lens(lens_id, title, true, glyph, cx)
                    });
                }
                // The text changed under it (Roslyn's ContentModified): asked again when next in the window.
                Err(RequestError::Stale) => {
                    view.update(cx, |v, _| v.code_lens_resolve_failed(lens_id))
                }
                Err(e) => trace(format_args!("codeLens/resolve {lens_id} failed: {e:?}")),
            }
        }
        self.pump_lens_resolves(id, cx);
        self.wake_intellisense_waiters();
    }

    /// [`eludite_editor::EditorEvent::CodeLensActivated`]: the References popup, Run Test or Debug Test.
    pub(super) fn activate_code_lens(
        &mut self,
        id: &str,
        lens_id: u64,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let activated = Instant::now();
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let uri = doc.uri.clone();
        let Some(entry) = self
            .code_lens
            .docs
            .get(id)
            .and_then(|d| d.entries.get(&lens_id))
            .cloned()
        else {
            return;
        };
        trace(format_args!(
            "code lens {lens_id} activated ({})",
            if keyboard { "keyboard" } else { "mouse" }
        ));
        // An unresolved references lens opens at its symbol.
        let target =
            entry
                .command
                .as_ref()
                .map(|c| c.target.clone())
                .unwrap_or(LensTarget::References {
                    uri,
                    position: entry.lens.range.start,
                    locations: None,
                });
        match target {
            LensTarget::References {
                position,
                locations,
                ..
            } => self.open_lens_references(id, lens_id, position, locations, activated, window, cx),
            LensTarget::Test { .. } | LensTarget::CargoTest { .. } => {
                self.run_lens_tests(id, lens_id, entry.kind == LensKind::DebugTest, window, cx)
            }
        }
    }

    /// Run Test or Debug Test from a lens, through the Test Explorer's commands (so the build gate, the streamed
    /// results and the Error List rows are the Test Explorer's). A test not discovered yet is discovered first.
    fn run_lens_tests(
        &mut self,
        id: &str,
        lens_id: u64,
        debug: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.documents.get(id).map(|d| d.path.clone()) else {
            return;
        };
        let Some(target) = self
            .code_lens
            .docs
            .get(id)
            .and_then(|d| d.entries.get(&lens_id))
            .and_then(|e| e.command.as_ref())
            .map(|c| c.target.clone())
        else {
            return;
        };
        let ids = lens_test_ids(&self.tests, &path, &target);
        if !ids.is_empty() {
            self.invoke_lens_run(ids, debug, window, cx);
            return;
        }
        // Not discovered yet: discover, the lens reading "Discovering…", then run.
        self.set_lens_discovering(id, lens_id, true, cx);
        if let Err(e) = self.invoke(test_cmds::DISCOVER, json!({}), window, cx) {
            self.set_lens_discovering(id, lens_id, false, cx);
            self.status
                .set(eludite_ui::slots::STATE, format!("Run Test: {e}"));
            return;
        }
        let (tx, rx) = oneshot::channel();
        self.code_lens.waiting.push(WaitingRun {
            doc: id.to_owned(),
            tx,
        });
        let doc = id.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            if rx.await.is_err() {
                return;
            }
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.set_lens_discovering(&doc, lens_id, false, cx);
                let ids = lens_test_ids(&shell.tests, &path, &target);
                if ids.is_empty() {
                    let name = match &target {
                        LensTarget::Test { member, .. } => member.clone(),
                        LensTarget::CargoTest { name, .. } => name.clone(),
                        LensTarget::References { .. } => String::new(),
                    };
                    shell.status.set(
                        eludite_ui::slots::STATE,
                        format!("Run Test: the Test Explorer found no test named {name}"),
                    );
                    cx.notify();
                    return;
                }
                shell.invoke_lens_run(ids, debug, window, cx);
            });
        })
        .detach();
        // The discovery may already be over (nothing to build, or it could not start).
        self.code_lens_tests_changed(cx);
    }

    fn invoke_lens_run(
        &mut self,
        ids: Vec<String>,
        debug: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = if debug {
            test_cmds::DEBUG
        } else {
            test_cmds::RUN
        };
        if let Err(e) = self.invoke(command, json!({ "ids": ids }), window, cx) {
            self.status.set(
                eludite_ui::slots::STATE,
                format!("{}: {e}", if debug { "Debug Test" } else { "Run Test" }),
            );
            cx.notify();
        }
    }

    fn set_lens_discovering(&mut self, id: &str, lens_id: u64, on: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let (view, path) = (doc.view.clone(), doc.path.clone());
        let Some(entry) = self
            .code_lens
            .docs
            .get_mut(id)
            .and_then(|d| d.entries.get_mut(&lens_id))
        else {
            return;
        };
        entry.discovering = on;
        let (title, glyph) = decorate(&self.tests, &path, entry);
        view.update(cx, |v, cx| {
            v.update_code_lens(lens_id, title, true, glyph, cx)
        });
    }

    /// The Test Explorer's model changed: the test lenses show the latest outcomes, and runs waiting for a discovery
    /// go once it is over.
    pub(super) fn code_lens_tests_changed(&mut self, cx: &mut Context<Self>) {
        let discovering = matches!(self.tests.phase, Phase::Building | Phase::Discovering);
        if !discovering {
            for w in self.code_lens.waiting.drain(..) {
                let _ = w.tx.send(());
            }
        }
        for (doc_id, state) in &self.code_lens.docs {
            let Some(doc) = self.documents.get(doc_id) else {
                continue;
            };
            let updates: Vec<(u64, String, Option<TestGlyph>)> = state
                .entries
                .iter()
                .filter(|(_, e)| e.kind == LensKind::RunTest && e.command.is_some())
                .map(|(lens_id, e)| {
                    let (title, glyph) = decorate(&self.tests, &doc.path, e);
                    (*lens_id, title, glyph)
                })
                .collect();
            if updates.is_empty() {
                continue;
            }
            doc.view.update(cx, |v, cx| {
                for (lens_id, title, glyph) in updates {
                    v.update_code_lens(lens_id, title, true, glyph, cx);
                }
            });
        }
    }

    /// Open the References popup at lens `lens_id`: the locations given, or `textDocument/references` (without the
    /// declaration, as the lens counts) at the symbol.
    #[allow(clippy::too_many_arguments)]
    fn open_lens_references(
        &mut self,
        id: &str,
        lens_id: u64,
        position: lsp::Position,
        locations: Option<Vec<lsp::Location>>,
        activated: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.code_lens.popup_request.take() {
            p.handle.cancel();
        }
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let view = doc.view.clone();
        let offset = offset_in(&doc.sent, position);
        let symbol = {
            let v = view.read(cx);
            let b = v.editor().buffer();
            let o = offset.min(b.len());
            let line = b.line(b.offset_to_point(o).row);
            let col = b.offset_to_point(o).column as usize;
            line.get(col..)
                .map(|rest| {
                    rest.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                })
                .unwrap_or_default()
        };
        let anchor = {
            let v = view.read(cx);
            v.code_lens_bounds(lens_id)
                .map(|b| b.bottom_left())
                .or_else(|| {
                    let caret = v.editor().primary_selection().head;
                    v.pixel_position_for_offset(caret)
                        .map(|p| point(p.x, p.y + v.line_height()))
                })
                .unwrap_or_default()
        };
        let theme = self.theme;
        let popup = cx.new(|cx| ReferencesPopup::new(theme, symbol, anchor, cx));
        cx.subscribe_in(&popup, window, |shell, _, event, window, cx| match event {
            LensPopupEvent::Navigate(r) => shell.navigate_from_lens_popup(*r, window, cx),
            LensPopupEvent::Dismissed => shell.close_lens_popup(window, cx),
        })
        .detach();
        popup.focus_handle(cx).focus(window, cx);
        self.code_lens.popup = Some(popup.clone());
        self.code_lens.popup_doc = Some(id.to_owned());
        self.code_lens.popup_ticket += 1;
        let ticket = self.code_lens.popup_ticket;
        push_capped(
            &mut self.code_lens.popup_timings,
            PopupTiming {
                activated: Some(activated),
                opened: Some(Instant::now()),
                ..Default::default()
            },
        );
        cx.notify();
        if let Some(locations) = locations {
            self.fill_lens_popup(ticket, locations, cx);
            return;
        }
        self.flush_change(id, cx);
        let generation = self.doc_generation(id);
        let doc = &self.documents[id];
        let version = doc.lsp_version;
        let params = lsp::ReferenceParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position: lsp_position(&doc.sent, offset_in(&doc.sent, position)),
            context: lsp::ReferenceContext {
                include_declaration: false,
            },
        };
        let (handle, rx) = doc.session.request::<lsp::References>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                let current = shell.code_lens.popup_ticket == ticket;
                let stale = shell.doc_generation(&doc_id) != generation
                    || shell.documents.get(&doc_id).map(|d| d.lsp_version) != Some(version);
                if !current {
                    return;
                }
                shell.code_lens.popup_request = None;
                let Some(popup) = shell.code_lens.popup.clone() else {
                    return;
                };
                match reply.result {
                    Ok(_) | Err(RequestError::Stale) if stale => popup.update(cx, |p, cx| {
                        p.fail(super::navigation::OUTDATED.to_owned(), cx)
                    }),
                    Ok(l) => shell.fill_lens_popup(ticket, l.unwrap_or_default(), cx),
                    Err(RequestError::Canceled) => {}
                    Err(e) => popup.update(cx, |p, cx| {
                        p.fail(
                            match e {
                                RequestError::NoHost => "no language server is running".into(),
                                RequestError::Failed(m) => m,
                                _ => super::navigation::OUTDATED.to_owned(),
                            },
                            cx,
                        )
                    }),
                }
            });
        });
        self.code_lens.popup_request = Some(Pending {
            handle,
            ticket,
            _task: task,
        });
    }

    /// Read the locations' lines off the UI thread, then fill the popup (if it is still the one asked for).
    fn fill_lens_popup(
        &mut self,
        ticket: u64,
        locations: Vec<lsp::Location>,
        cx: &mut Context<Self>,
    ) {
        let (inputs, sources) = self.reference_inputs(locations, cx);
        cx.spawn(async move |this, cx| {
            let refs = cx
                .background_spawn(async move { build_references(inputs, sources) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if shell.code_lens.popup_ticket != ticket {
                    return;
                }
                let Some(popup) = shell.code_lens.popup.clone() else {
                    return;
                };
                let rows = refs.len();
                popup.update(cx, |p, cx| p.finish(refs, cx));
                if let Some(t) = shell.code_lens.popup_timings.last_mut() {
                    t.filled = Some(Instant::now());
                    t.rows = rows;
                }
                shell.wake_intellisense_waiters();
            });
        })
        .detach();
    }

    fn close_lens_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.code_lens.popup_request.take() {
            p.handle.cancel();
        }
        self.code_lens.popup = None;
        if let Some(doc) = self.code_lens.popup_doc.take()
            && let Some(view) = self.documents.get(&doc).map(|d| d.view.clone())
        {
            view.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Enter (or a double-click) on a reference: push where the caret is, then open the reference.
    fn navigate_from_lens_popup(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self
            .code_lens
            .popup
            .as_ref()
            .and_then(|p| p.read(cx).references().get(index).cloned())
        else {
            return;
        };
        self.close_lens_popup(window, cx);
        if let Some(here) = self.caret_entry(cx) {
            self.navigation.history.push(here);
        }
        if let Err(e) = self.open_at(&r.path.to_string_lossy(), r.line, r.column, window, cx) {
            self.status.set(eludite_ui::slots::STATE, e.to_string());
        }
    }
}

/// What the popup reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LensPopupEvent {
    /// Enter or a double-click on reference `index`.
    Navigate(usize),
    Dismissed,
}

/// The popup's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupState {
    Loading,
    Done,
    Failed(String),
}

/// A row of the popup: a file, or a reference in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupRow {
    File { title: String, count: usize },
    Reference(usize),
}

/// The rows for references sorted by file: each file's header, then its references.
pub fn popup_rows(refs: &[Reference]) -> Vec<PopupRow> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < refs.len() {
        let path = &refs[i].path;
        let end = refs[i..]
            .iter()
            .position(|r| &r.path != path)
            .map_or(refs.len(), |n| i + n);
        rows.push(PopupRow::File {
            title: document_title(path),
            count: end - i,
        });
        rows.extend((i..end).map(PopupRow::Reference));
        i = end;
    }
    rows
}

/// Rows the popup shows at once; the list scrolls past them.
pub const POPUP_ROWS: usize = 12;

/// Debug selector of popup row `ix`.
pub fn popup_row_selector(ix: usize) -> String {
    format!("code-lens-popup-row-{ix}")
}

/// The CodeLens References popup: the references grouped by file with a preview line, virtualized; Up, Down, Enter,
/// Escape, double-click.
pub struct ReferencesPopup {
    theme: Theme,
    symbol: String,
    state: PopupState,
    refs: Vec<Reference>,
    rows: Vec<PopupRow>,
    /// A reference row.
    selected: Option<usize>,
    anchor: Point<Pixels>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl ReferencesPopup {
    pub fn new(
        theme: Theme,
        symbol: String,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme,
            symbol,
            state: PopupState::Loading,
            refs: Vec::new(),
            rows: Vec::new(),
            selected: None,
            anchor,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn references(&self) -> &[Reference] {
        &self.refs
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[PopupRow] {
        &self.rows
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn state(&self) -> &PopupState {
        &self.state
    }

    /// The selected reference's index into [`ReferencesPopup::references`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_reference(&self) -> Option<usize> {
        match self.rows.get(self.selected?) {
            Some(PopupRow::Reference(r)) => Some(*r),
            _ => None,
        }
    }

    pub fn header(&self) -> String {
        match &self.state {
            PopupState::Loading => format!("'{}' references: searching\u{2026}", self.symbol),
            PopupState::Failed(m) => format!("'{}' references: {m}", self.symbol),
            PopupState::Done => {
                let n = self.refs.len();
                format!(
                    "'{}': {n} reference{}",
                    self.symbol,
                    if n == 1 { "" } else { "s" }
                )
            }
        }
    }

    pub fn finish(&mut self, refs: Vec<Reference>, cx: &mut Context<Self>) {
        self.rows = popup_rows(&refs);
        self.refs = refs;
        self.state = PopupState::Done;
        self.selected = self
            .rows
            .iter()
            .position(|r| matches!(r, PopupRow::Reference(_)));
        cx.notify();
    }

    pub fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.state = PopupState::Failed(message);
        cx.notify();
    }

    /// Move the selection by `delta` reference rows (file rows are skipped).
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let refs: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, PopupRow::Reference(_)))
            .map(|(ix, _)| ix)
            .collect();
        if refs.is_empty() {
            return;
        }
        let at = self
            .selected
            .and_then(|s| refs.iter().position(|&r| r == s))
            .unwrap_or(0) as isize;
        let next = (at + delta).clamp(0, refs.len() as isize - 1) as usize;
        self.selected = Some(refs[next]);
        self.scroll
            .scroll_to_item(refs[next], ScrollStrategy::Nearest);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "up" => self.step(-1, cx),
            "down" => self.step(1, cx),
            "pageup" => self.step(-(POPUP_ROWS as isize - 1), cx),
            "pagedown" => self.step(POPUP_ROWS as isize - 1, cx),
            "enter" => {
                if let Some(r) = self.selected_reference() {
                    cx.emit(LensPopupEvent::Navigate(r));
                }
            }
            "escape" => cx.emit(LensPopupEvent::Dismissed),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn click(&mut self, ix: usize, e: &ClickEvent, cx: &mut Context<Self>) {
        if !matches!(self.rows.get(ix), Some(PopupRow::Reference(_))) {
            return;
        }
        self.selected = Some(ix);
        cx.notify();
        if e.click_count() >= 2
            && let Some(r) = self.selected_reference()
        {
            cx.emit(LensPopupEvent::Navigate(r));
        }
    }
}

impl EventEmitter<LensPopupEvent> for ReferencesPopup {}

impl Focusable for ReferencesPopup {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReferencesPopup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let header = div()
            .id("code-lens-popup-header")
            .debug_selector(|| "code-lens-popup-header".into())
            .px_2()
            .py_1()
            .text_color(t.text_muted)
            .child(self.header());
        let shown = self.rows.len().clamp(1, POPUP_ROWS);
        let list = uniform_list(
            "code-lens-popup-rows",
            self.rows.len(),
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let t = this.theme;
                range
                    .filter_map(|ix| {
                        let selected = this.selected == Some(ix);
                        let id = popup_row_selector(ix);
                        let row = match this.rows.get(ix)? {
                            PopupRow::File { title, count } => tree_row(
                                &t,
                                id,
                                None,
                                format!("{title} ({count})"),
                                TreeRowStyle {
                                    depth: 0,
                                    disclosure: None,
                                    selected: false,
                                    muted: false,
                                    bold: true,
                                },
                                |_, _, _| {},
                            ),
                            PopupRow::Reference(r) => {
                                let r = this.refs.get(*r)?;
                                tree_row(
                                    &t,
                                    id,
                                    None,
                                    "",
                                    TreeRowStyle {
                                        depth: 1,
                                        disclosure: None,
                                        selected,
                                        muted: false,
                                        bold: false,
                                    },
                                    |_, _, _| {},
                                )
                                .child(div().flex_1().min_w_0().overflow_hidden().child(
                                    highlighted_code(r.text.clone(), r.highlight.clone(), &t),
                                ))
                                .child(
                                    div()
                                        .flex_none()
                                        .pl_2()
                                        .text_color(if selected {
                                            t.text_on_accent
                                        } else {
                                            t.text_muted
                                        })
                                        .child(SharedString::from(format!(
                                            "({}, {})",
                                            r.line, r.column
                                        ))),
                                )
                            }
                        };
                        Some(
                            row.w_full()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, e, _, cx| this.click(ix, e, cx))),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .h(px(TREE_ROW_HEIGHT * shown as f32));
        let panel = eludite_ui::popup::popup_panel(&t)
            .id("code-lens-popup")
            .debug_selector(|| "code-lens-popup".into())
            .track_focus(&self.focus)
            .key_context("CodeLensPopup")
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(LensPopupEvent::Dismissed)))
            .occlude()
            .flex()
            .flex_col()
            .w(px(560.))
            .py_1()
            .child(header)
            .child(list);
        deferred(
            anchored()
                .position(self.anchor)
                .snap_to_window_with_margin(px(4.))
                .child(panel),
        )
        .with_priority(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn settings_choose_the_indicators_per_language() {
        let mut s = LensSettings::default();
        assert_eq!(
            s.filter("csharp"),
            LensFilter {
                references: true,
                tests: true
            }
        );
        s.tests = false;
        assert!(!s.filter("rust").tests && s.filter("rust").references);
        s.languages.insert("rust".into(), "tests".into());
        assert_eq!(
            s.filter("rust"),
            LensFilter {
                references: false,
                tests: true
            }
        );
        s.enabled = false;
        assert!(!s.filter("csharp").any());
        assert!(
            s.filter("rust").tests,
            "a language override wins over the switch"
        );
        s.languages.insert("typescript".into(), "off".into());
        assert!(!s.filter("typescriptreact").any());
        s.languages.insert("csharp".into(), "default".into());
        assert!(!s.filter("csharp").any());
        assert!(!LensFilter::NONE.any());
        assert!(
            LensFilter {
                references: true,
                tests: false
            }
            .shows(LensKind::Implementations)
        );
        assert!(
            !LensFilter {
                references: true,
                tests: false
            }
            .shows(LensKind::DebugTest)
        );
    }

    #[test]
    fn names_titles_and_durations() {
        assert_eq!(
            method_name("Corpus.CalculatorTests.AddsPairs(System.Int32)"),
            "AddsPairs"
        );
        assert_eq!(class_name("Corpus.CalculatorTests.Adds"), "CalculatorTests");
        assert_eq!(class_name("Adds"), "");
        assert_eq!(duration_text(0.4), "< 1 ms");
        assert_eq!(duration_text(12.4), "12 ms");
        assert_eq!(duration_text(1234.0), "1.2 s");
        let ra = |title: &str, kind, exact| LensCommand {
            kind,
            title: title.into(),
            target: LensTarget::CargoTest {
                name: "tests::adds".into(),
                exact,
                package: None,
            },
        };
        assert_eq!(
            display_title(&ra("\u{25b6}\u{fe0e} Run Test", LensKind::RunTest, true)),
            "Run Test"
        );
        assert_eq!(
            display_title(&ra("Debug", LensKind::DebugTest, true)),
            "Debug Test"
        );
        assert_eq!(
            display_title(&ra("Debug", LensKind::DebugTest, false)),
            "Debug Tests"
        );
        assert_eq!(
            display_title(&LensCommand {
                kind: LensKind::References,
                title: "- references".into(),
                target: LensTarget::Test {
                    member: String::new(),
                    range: lsp::Range {
                        start: lsp::Position {
                            line: 0,
                            character: 0
                        },
                        end: lsp::Position {
                            line: 0,
                            character: 0
                        }
                    }
                }
            }),
            "- references"
        );
    }

    #[test]
    fn popup_rows_group_references_by_file() {
        let r = |path: &str, line: u32| Reference {
            project: None,
            path: PathBuf::from(path),
            line,
            column: 1,
            text: "x".into(),
            highlight: 0..1,
        };
        let refs = vec![r("/a/A.cs", 1), r("/a/A.cs", 9), r("/a/B.cs", 2)];
        assert_eq!(
            popup_rows(&refs),
            vec![
                PopupRow::File {
                    title: "A.cs".into(),
                    count: 2
                },
                PopupRow::Reference(0),
                PopupRow::Reference(1),
                PopupRow::File {
                    title: "B.cs".into(),
                    count: 1
                },
                PopupRow::Reference(2),
            ]
        );
        assert!(popup_rows(&[]).is_empty());
    }
}
