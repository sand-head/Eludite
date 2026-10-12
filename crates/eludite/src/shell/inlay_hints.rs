//! Inlay hints (brief 0066): the language servers' parameter-name and type hints, drawn in the line as chips that are
//! not text, and `eludite.editor.toggle_inlay_hints` (Edit > IntelliSense > Toggle Inline Hints, Alt+F1).
//!
//! - **Requests.** The editor says when hints are wanted and for which rows (the visible ones and 100 lines around
//!   them: turned on, 150 ms after the last edit, scrolled out of the covered rows, a refresh). The shell sends
//!   `textDocument/inlayHint` for those rows through the document's server session after a pending `didChange` (one in
//!   flight per document; a newer one cancels the older). An answer is dropped when its server generation or the
//!   document's version moved since (CLAUDE.md invariant 12); the editor drops one for text changed since. A server
//!   without `inlayHintProvider` is never asked.
//! - **Kinds.** `editor.inlayHints.parameterNames` shows parameter hints (LSP kind 2), `editor.inlayHints.types` type
//!   hints (kind 1); a hint with no kind shows when either does. The host turns every Roslyn hint on; the filter is
//!   here, so a setting change needs no server round trip.
//! - **Tooltips.** The mouse resting on a hint shows its tooltip in the Quick Info popup, after `inlayHint/resolve`
//!   when the hint has none and the server resolves hints.
//! - **Refresh.** `workspace/inlayHint/refresh` (relayed by the host as `eludite/inlayHint/refresh`), a new generation,
//!   a server's capabilities arriving and the solution finishing its load ask again.
//! - **Toggle.** Alt+F1 shows or hides every editor's hints for the session (Visual Studio shows them while Alt+F1 is
//!   held; GPUI's key-up reaches only the focused element, so here it toggles); the settings are unchanged.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::mpsc;
use std::thread::ThreadId;

use eludite_commands::inlay_hints::{self as cmd, InlayHintsTarget, ToggleRequest, ToggleResult};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_editor::{InlayHint, InlayKind};
use eludite_lsp::inlay::{self as hints, HintKind};
use eludite_lsp::lsp;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{Context, Task, Window};
use std::cell::RefCell;

use super::Shell;
use super::documents::trace;
use super::intellisense::{Provider as Source, offset_in};
use super::semantic::{LANGUAGES, LanguageSwitch};
use super::servers::ServerKey;
use super::session::{RequestError, RequestHandle};

/// The hint settings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HintSettings {
    pub switch: LanguageSwitch,
    pub parameters: bool,
    pub types: bool,
}

impl HintSettings {
    /// Whether a hint of `kind` shows.
    pub fn shows(&self, kind: HintKind) -> bool {
        match kind {
            HintKind::Parameter => self.parameters,
            HintKind::Type => self.types,
            HintKind::Other => self.parameters || self.types,
        }
    }
}

struct Pending {
    handle: RequestHandle,
    ticket: u64,
    _task: Task<()>,
}

/// One document's hints.
#[derive(Default)]
pub struct DocHints {
    request: Option<Pending>,
    resolve: Option<Pending>,
    /// The hints shown, as the server sent them, by the editor's hint id.
    pub hints: HashMap<u64, lsp::InlayHint>,
    next_id: u64,
}

/// A toggle from another thread (an agent), applied on the UI thread.
pub struct InlayJob {
    pub request: ToggleRequest,
    pub reply: mpsc::SyncSender<Result<ToggleResult, CommandError>>,
}

/// The shell's [`InlayHintsTarget`].
pub struct InlayBus {
    ui_thread: ThreadId,
    jobs: UnboundedSender<InlayJob>,
}

thread_local! {
    static STAGED: RefCell<Option<Result<ToggleResult, CommandError>>> = const { RefCell::new(None) };
}

/// Stage the UI thread's outcome for the handler (as the other shell commands do).
pub fn stage(outcome: Result<ToggleResult, CommandError>) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

impl InlayHintsTarget for InlayBus {
    fn toggle(&self, request: ToggleRequest) -> Result<ToggleResult, CommandError> {
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    cmd::TOGGLE
                )))
            });
        }
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(InlayJob { request, reply })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|_| CommandError::Failed("the editor did not answer in time".into()))?
    }
}

/// Register `eludite.editor.toggle_inlay_hints`; returns what the shell drains on the UI thread.
pub fn register(commands: &CommandRegistry) -> UnboundedReceiver<InlayJob> {
    let (jobs, rx) = unbounded();
    cmd::register(
        commands,
        std::sync::Arc::new(InlayBus {
            ui_thread: std::thread::current().id(),
            jobs,
        }),
    );
    rx
}

/// The shell's inlay hint state.
pub struct InlayHintsState {
    pub settings: HintSettings,
    /// Alt+F1's session switch: hints show only while it is on.
    pub shown: bool,
    pub docs: HashMap<String, DocHints>,
    /// Roslyn's `inlayHintProvider`: offered, and resolves.
    host: Option<bool>,
}

impl Default for InlayHintsState {
    fn default() -> Self {
        Self {
            settings: HintSettings::default(),
            shown: true,
            docs: HashMap::new(),
            host: None,
        }
    }
}

/// `inlayHintProvider` in `caps`: `None` when absent, else whether it resolves.
fn provider(caps: &serde_json::Value) -> Option<bool> {
    let p = &caps["inlayHintProvider"];
    if p.is_null() || p == &serde_json::Value::Bool(false) {
        return None;
    }
    Some(p["resolveProvider"].as_bool() == Some(true))
}

impl Shell {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn inlay_hint_state(&self) -> &InlayHintsState {
        &self.inlay_hints
    }

    /// Document `id`'s server's hint provider: `Some(resolves)` when it offers hints.
    fn hint_provider(&self, id: &str) -> Option<bool> {
        let doc = self.documents.get(id)?;
        match &doc.server {
            ServerKey::Host => self.inlay_hints.host,
            _ => doc
                .session
                .shared()
                .capabilities
                .as_ref()
                .and_then(provider),
        }
    }

    fn hints_wanted(&self, id: &str) -> bool {
        let s = &self.inlay_hints;
        match self.documents.get(id) {
            Some(d) if !d.read_only && s.shown && (s.settings.parameters || s.settings.types) => d
                .language_id
                .as_deref()
                .is_some_and(|l| s.settings.switch.on(l)),
            _ => false,
        }
    }

    /// Roslyn's capabilities arrived (brief 0066).
    pub(super) fn inlay_host_capabilities(&mut self, caps: &serde_json::Value) {
        self.inlay_hints.host = provider(caps);
    }

    /// A document opened: its editor shows hints when the settings say so.
    pub(super) fn inlay_document_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        let on = self.hints_wanted(id);
        if let Some(view) = self.documents.get(id).map(|d| d.view.clone()) {
            view.update(cx, |v, cx| v.set_inlay_hints_enabled(on, cx));
        }
    }

    pub(super) fn inlay_document_closed(&mut self, id: &str) {
        if let Some(mut d) = self.inlay_hints.docs.remove(id) {
            for p in [d.request.take(), d.resolve.take()].into_iter().flatten() {
                p.handle.cancel();
            }
        }
    }

    /// Every open document's editor shows hints, or not, as the settings and the toggle now say; the ones that show
    /// them ask again (a kind filter changed).
    fn inlay_reapply(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.documents.keys().cloned().collect();
        for id in ids {
            let on = self.hints_wanted(&id);
            let view = self.documents[&id].view.clone();
            view.update(cx, |v, cx| {
                if v.inlay_hints_enabled() != on {
                    v.set_inlay_hints_enabled(on, cx);
                } else if on {
                    v.refresh_inlay_hints(cx);
                }
            });
            if !on {
                self.inlay_document_closed(&id);
            }
        }
    }

    /// `editor.inlayHints.*` and the per-language overrides.
    pub(super) fn inlay_apply_settings(&mut self, cx: &mut Context<Self>) {
        let next = {
            let s = self.settings.lock();
            HintSettings {
                switch: LanguageSwitch {
                    enabled: s.bool("editor.inlayHints.enabled"),
                    languages: LANGUAGES
                        .iter()
                        .map(|l| {
                            (
                                (*l).to_owned(),
                                s.string(&format!("editor.languages.{l}.inlayHints")),
                            )
                        })
                        .collect(),
                },
                parameters: s.bool("editor.inlayHints.parameterNames"),
                types: s.bool("editor.inlayHints.types"),
            }
        };
        if next == self.inlay_hints.settings {
            return;
        }
        self.inlay_hints.settings = next;
        self.inlay_reapply(cx);
    }

    /// The server of some documents changed: their hints are asked for again.
    pub(super) fn inlay_server_changed(&mut self, server: &ServerKey, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .documents
            .iter()
            .filter(|(_, d)| &d.server == server || d.servers.contains(server))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(d) = self.inlay_hints.docs.get_mut(&id)
                && let Some(p) = d.request.take()
            {
                p.handle.cancel();
            }
            let view = self.documents[&id].view.clone();
            view.update(cx, |v, cx| v.refresh_inlay_hints(cx));
        }
    }

    /// `eludite.editor.toggle_inlay_hints`: flip (or set) the session switch; the hints of the named or active
    /// document are counted.
    pub(super) fn toggle_inlay_hints(
        &mut self,
        request: ToggleRequest,
        cx: &mut Context<Self>,
    ) -> Result<ToggleResult, CommandError> {
        let path = match request.path {
            Some(p) => {
                let id = self.resolve_file(&p);
                let id = id.to_string_lossy().into_owned();
                if !self.documents.contains_key(&id) {
                    return Err(CommandError::Failed(format!("{p} is not open")));
                }
                Some(id)
            }
            None => self
                .controller
                .active_document()
                .filter(|d| self.documents.contains_key(d)),
        };
        self.inlay_hints.shown = request.enabled.unwrap_or(!self.inlay_hints.shown);
        self.inlay_reapply(cx);
        let hints = path
            .as_ref()
            .and_then(|p| self.documents.get(p))
            .map_or(0, |d| d.view.read(cx).inlay_hints().len());
        Ok(ToggleResult {
            path,
            enabled: self.inlay_hints.shown,
            hints,
        })
    }

    /// Apply the toggles agents send from other threads.
    pub(super) fn inlay_install(
        &mut self,
        mut jobs: UnboundedReceiver<InlayJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(InlayJob { request, reply }) = jobs.next().await {
                let outcome = this.update(cx, |shell, cx| shell.toggle_inlay_hints(request, cx));
                let _ =
                    reply.send(outcome.unwrap_or_else(|_| {
                        Err(CommandError::Failed("the window is closed".into()))
                    }));
            }
        });
        self._tasks.push(task);
    }

    /// [`eludite_editor::EditorEvent::InlayHintsRequested`]: ask the document's server for the hints of `rows`.
    pub(super) fn request_inlay_hints(
        &mut self,
        id: &str,
        request: u64,
        rows: Range<u32>,
        cx: &mut Context<Self>,
    ) {
        if self.hint_provider(id).is_none() || !self.hints_wanted(id) {
            return;
        }
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        if doc.language_id.is_none() || self.provider(doc) == Source::Syntax {
            return;
        }
        self.flush_change(id, cx);
        let generation = self.doc_generation(id);
        let doc = &self.documents[id];
        let snapshot = doc.sent.clone();
        let version = doc.lsp_version;
        let last = snapshot.max_point().row;
        let end = if rows.end > last {
            lsp::Position {
                line: last,
                character: snapshot.line_len(last),
            }
        } else {
            lsp::Position {
                line: rows.end,
                character: 0,
            }
        };
        let params = lsp::InlayHintParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            range: lsp::Range {
                start: lsp::Position {
                    line: rows.start.min(last),
                    character: 0,
                },
                end,
            },
        };
        trace(format_args!(
            "inlayHint request {request} for {} rows {rows:?} version {version}",
            doc.uri
        ));
        let (handle, rx) = doc.session.request::<lsp::InlayHintRequest>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                shell.on_inlay_hints(
                    &doc_id,
                    request,
                    generation,
                    version,
                    snapshot,
                    reply.result,
                    cx,
                )
            });
        });
        let state = self.inlay_hints.docs.entry(id.to_owned()).or_default();
        if let Some(p) = state.request.take() {
            p.handle.cancel();
        }
        state.request = Some(Pending {
            handle,
            ticket: request,
            _task: task,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn on_inlay_hints(
        &mut self,
        id: &str,
        request: u64,
        generation: u64,
        version: i32,
        snapshot: eludite_editor::text::BufferSnapshot,
        result: Result<Option<Vec<lsp::InlayHint>>, RequestError>,
        cx: &mut Context<Self>,
    ) {
        let current = self.doc_generation(id);
        let settings = self.inlay_hints.settings.clone();
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let (view, doc_version) = (doc.view.clone(), doc.lsp_version);
        let Some(state) = self.inlay_hints.docs.get_mut(id) else {
            return;
        };
        if state.request.as_ref().is_none_or(|p| p.ticket != request) {
            return; // Superseded.
        }
        state.request = None;
        if generation != current || version != doc_version {
            trace(format_args!(
                "inlayHint reply {request} dropped (stale: generation {generation}/{current}, version {version}/{doc_version})"
            ));
            return;
        }
        let list = match result {
            Ok(l) => l.unwrap_or_default(),
            Err(RequestError::Canceled | RequestError::Stale) => return,
            Err(e) => {
                trace(format_args!("inlayHint reply {request} failed: {e:?}"));
                Vec::new()
            }
        };
        state.hints.clear();
        let mut shown = Vec::with_capacity(list.len());
        for h in list {
            let kind = HintKind::of(&h);
            if !settings.shows(kind) {
                continue;
            }
            state.next_id += 1;
            let hint_id = state.next_id;
            shown.push(InlayHint {
                id: hint_id,
                offset: offset_in(&snapshot, h.position),
                label: hints::label(&h),
                kind: match kind {
                    HintKind::Parameter => InlayKind::Parameter,
                    HintKind::Type => InlayKind::Type,
                    HintKind::Other => InlayKind::Other,
                },
                padding_left: h.padding_left == Some(true),
                padding_right: h.padding_right == Some(true),
            });
            state.hints.insert(hint_id, h);
        }
        let count = shown.len();
        let applied = view.update(cx, |v, cx| v.set_inlay_hints(request, shown, cx));
        trace(format_args!(
            "inlayHint reply {request}: {count} hints{}",
            if applied {
                ""
            } else {
                ", dropped by the editor"
            }
        ));
    }

    /// [`eludite_editor::EditorEvent::InlayHintHovered`]: the hint's tooltip in the Quick Info popup, resolved first
    /// when it has none.
    pub(super) fn inlay_hint_hovered(
        &mut self,
        id: &str,
        hint_id: u64,
        offset: usize,
        cx: &mut Context<Self>,
    ) {
        let resolves = self.hint_provider(id) == Some(true);
        let generation = self.doc_generation(id);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let (view, session) = (doc.view.clone(), doc.session.clone());
        let Some(state) = self.inlay_hints.docs.get_mut(id) else {
            return;
        };
        let Some(hint) = state.hints.get(&hint_id).cloned() else {
            return;
        };
        if let Some(text) = hints::tooltip(&hint) {
            view.update(cx, |v, cx| {
                let popup = v.open_hover(offset, cx);
                v.set_hover(popup, Some(&text), None, cx);
            });
            return;
        }
        if !resolves {
            return;
        }
        if let Some(p) = state.resolve.take() {
            p.handle.cancel();
        }
        let (handle, rx) = session.request::<lsp::ResolveInlayHint>(hint);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                if shell.doc_generation(&doc_id) != generation {
                    return;
                }
                let Some(state) = shell.inlay_hints.docs.get_mut(&doc_id) else {
                    return;
                };
                state.resolve = None;
                let Ok(resolved) = reply.result else {
                    return;
                };
                if !state.hints.contains_key(&hint_id) {
                    return; // Replaced since.
                }
                let text = hints::tooltip(&resolved);
                state.hints.insert(hint_id, resolved);
                let Some(text) = text else {
                    return;
                };
                let Some(view) = shell.documents.get(&doc_id).map(|d| d.view.clone()) else {
                    return;
                };
                view.update(cx, |v, cx| {
                    let popup = v.open_hover(offset, cx);
                    v.set_hover(popup, Some(&text), None, cx);
                });
            });
        });
        self.inlay_hints
            .docs
            .entry(id.to_owned())
            .or_default()
            .resolve = Some(Pending {
            handle,
            ticket: hint_id,
            _task: task,
        });
    }
}
