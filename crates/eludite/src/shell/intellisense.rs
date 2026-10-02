//! IntelliSense through `eludite-host` (brief 0013): the editor's completion list, Quick Info and Parameter Info
//! fed by `textDocument/completion`, `completionItem/resolve`, `textDocument/hover` and `textDocument/signatureHelp`.
//!
//! - **Requests** go through the [`HostSession`](super::session::HostSession) worker after a pending `didChange` is
//!   flushed, so the host sees the text the user sees; the reply arrives on a oneshot channel the UI awaits.
//! - **Superseding**: each document has at most one request of each kind in flight. A new one (the next keystroke)
//!   cancels the previous one with `$/cancelRequest`; closing the popup cancels it too.
//! - **Stale answers are dropped** (CLAUDE.md invariant 12): an answer is applied only when it belongs to the newest
//!   request, its solution generation is current and the document's LSP version is the one it was computed for.
//! - **Fallback**: while the language server is starting or the solution is loading (or for a file no server
//!   handles), the list comes from the identifiers of the buffer's tree-sitter tree, marked as such; the server's
//!   items replace it as soon as they arrive.
//! - **Resolve**: completion documentation is fetched lazily, for the selected item only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::workspace::{
    self, AcceptCompletionOutput, CompleteOutput, CompletionRow, HoverOutput, MAX_COMPLETION_ROWS,
    PopupState, SignatureHelpOutput, SignatureRow, WorkspaceOutput, WorkspaceRequest,
};

use eludite_editor::intellisense::snippet_to_plain;
use eludite_editor::text::{self, PointUtf16, Unclipped};
use eludite_editor::{
    CompletionEdit, CompletionItem, CompletionKind, CompletionSource, CompletionTrigger,
    EditorEvent, SignatureHelpData, SignatureInfo, SignatureTrigger,
};
use eludite_lsp::host::{LanguageServerState, SolutionState};
use eludite_lsp::lsp;
use gpui::{Context, Entity, Task, Window};
use serde_json::{Value, json};

use super::documents::{Document, move_caret, offset_of, trace};
use super::{Caret, Shell};
use eludite_editor::EditorView;

/// How long an agent's `eludite.editor.complete`, `hover` or `signature_help` waits for the answer.
pub const AGENT_WAIT: Duration = Duration::from_secs(5);
use super::session::{Reply, RequestError, RequestHandle};

/// A request in flight for one document.
pub struct Pending {
    handle: RequestHandle,
    /// The editor's request id it answers.
    editor_id: u64,
    _task: Task<()>,
}

/// Each document's IntelliSense requests in flight.
#[derive(Default)]
pub struct DocIntellisense {
    completion: Option<Pending>,
    resolve: Option<Pending>,
    hover: Option<Pending>,
    signature: Option<Pending>,
    /// The server's items of the shown list (editor list id, items), for `completionItem/resolve`.
    raw: Option<(u64, Arc<Vec<lsp::CompletionItem>>)>,
    /// The text the shown list was computed on: its items' `additionalTextEdits` are positions in it.
    raw_base: Option<text::BufferSnapshot>,
    /// `additionalTextEdits` from `completionItem/resolve`, by item index of the shown list, with the text the server
    /// had when it resolved (an empty list: resolved, none).
    resolved: std::collections::HashMap<usize, (Vec<lsp::TextEdit>, text::BufferSnapshot)>,
    /// A resolve sent when an unresolved item was committed (brief 0015).
    accept_resolve: Option<Pending>,
    /// The last Parameter Info answer, sent back as `activeSignatureHelp` on a retrigger.
    last_signature: Option<lsp::SignatureHelp>,
}

fn cancel(slot: &mut Option<Pending>) {
    if let Some(p) = slot.take() {
        p.handle.cancel();
    }
}

impl DocIntellisense {
    /// Cancel everything in flight (a new solution generation, the document closing).
    pub fn cancel_all(&mut self) {
        cancel(&mut self.completion);
        cancel(&mut self.resolve);
        cancel(&mut self.hover);
        cancel(&mut self.signature);
        cancel(&mut self.accept_resolve);
        self.raw = None;
        self.raw_base = None;
        self.resolved.clear();
        self.last_signature = None;
    }
}

/// What the language server supports, from its capabilities in `eludite/languageServer/status`.
#[derive(Debug, Clone, Default)]
pub struct ServerFeatures {
    completion_triggers: Vec<String>,
    resolve: bool,
    signature_triggers: Vec<String>,
    /// `codeActionProvider.resolveProvider` (brief 0015).
    pub code_action_resolve: bool,
}

impl ServerFeatures {
    pub fn from_capabilities(caps: &Value) -> Self {
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let sig = &caps["signatureHelpProvider"];
        let mut signature_triggers = strings(&sig["triggerCharacters"]);
        signature_triggers.extend(strings(&sig["retriggerCharacters"]));
        Self {
            completion_triggers: strings(&caps["completionProvider"]["triggerCharacters"]),
            resolve: caps["completionProvider"]["resolveProvider"].as_bool() == Some(true),
            signature_triggers,
            code_action_resolve: caps["codeActionProvider"]["resolveProvider"].as_bool()
                == Some(true),
        }
    }
}

/// Where completion items come from for a document right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// The language server is running and the solution loaded.
    Server,
    /// The server is starting or the solution loading: the syntax fallback at once, the server's items when they
    /// arrive.
    ServerAndSyntax,
    /// No server for this document (none started, it failed, or the file is not one it handles).
    Syntax,
}

/// When the steps of one completion happened (the `--bench-complete` harness and the report).
#[derive(Debug, Clone, Default)]
pub struct CompletionTiming {
    /// The request was written to the host.
    pub sent: Option<Instant>,
    /// The reply was read.
    pub received: Option<Instant>,
    /// The items were handed to the editor.
    pub applied: Option<Instant>,
    pub items: usize,
    /// The answer was dropped (stale or superseded).
    pub dropped: bool,
}

/// Completion timings kept at most (the harness reads them as it goes).
const MAX_TIMINGS: usize = 4096;

fn markup_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// Hover `contents` (`MarkupContent`, `MarkedString` or `MarkedString[]`) as Markdown.
fn hover_markdown(contents: &Value) -> Option<String> {
    let marked = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => {
                let value = o.get("value")?.as_str()?;
                match (o.get("kind"), o.get("language").and_then(Value::as_str)) {
                    (Some(_), _) => Some(value.to_owned()),
                    (None, Some(lang)) => Some(format!("```{lang}\n{value}\n```")),
                    (None, None) => Some(value.to_owned()),
                }
            }
            _ => None,
        }
    };
    match contents {
        Value::Array(parts) => {
            let parts: Vec<String> = parts.iter().filter_map(marked).collect();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        other => marked(other),
    }
}

pub(super) fn offset_in(snapshot: &text::BufferSnapshot, p: lsp::Position) -> usize {
    let point = snapshot.clip_point_utf16(
        Unclipped(PointUtf16::new(p.line, p.character)),
        text::Bias::Left,
    );
    snapshot.point_utf16_to_offset(point)
}

pub fn lsp_position(snapshot: &text::BufferSnapshot, offset: usize) -> lsp::Position {
    let p = snapshot.offset_to_point_utf16(offset.min(snapshot.len()));
    lsp::Position {
        line: p.row,
        character: p.column,
    }
}

/// The edit range of a `TextEdit` or `InsertReplaceEdit` (its insert range), or of an `itemDefaults.editRange`.
fn edit_range(v: &Value) -> Option<lsp::Range> {
    let range = v.get("range").or_else(|| v.get("insert")).unwrap_or(v);
    serde_json::from_value(range.clone()).ok()
}

/// The server's items in editor terms. Edits are anchored in `snapshot`, the text the request was made on.
pub fn completion_items(
    raw: &mut [lsp::CompletionItem],
    defaults: Option<&Value>,
    snapshot: &text::BufferSnapshot,
    resolvable: bool,
) -> Vec<CompletionItem> {
    let default_format = defaults
        .and_then(|d| d.get("insertTextFormat"))
        .and_then(Value::as_u64);
    let default_range = defaults
        .and_then(|d| d.get("editRange"))
        .and_then(edit_range);
    let default_data = defaults.and_then(|d| d.get("data")).cloned();
    let anchored = |r: lsp::Range| {
        let start = offset_in(snapshot, r.start);
        let end = offset_in(snapshot, r.end).max(start);
        snapshot.anchor_before(start)..snapshot.anchor_after(end)
    };
    raw.iter_mut()
        .map(|item| {
            if item.data.is_none() {
                item.data.clone_from(&default_data);
            }
            let snippet = item
                .extra
                .get("insertTextFormat")
                .and_then(Value::as_u64)
                .or(default_format)
                == Some(2);
            let plain = |t: &str| {
                if snippet {
                    snippet_to_plain(t)
                } else {
                    t.to_owned()
                }
            };
            let edit = match &item.text_edit {
                Some(te) => edit_range(te).map(|r| CompletionEdit {
                    range: anchored(r),
                    new_text: plain(
                        te.get("newText")
                            .and_then(Value::as_str)
                            .unwrap_or(&item.label),
                    ),
                }),
                None => default_range.map(|r| CompletionEdit {
                    range: anchored(r),
                    new_text: plain(
                        item.extra
                            .get("textEditText")
                            .and_then(Value::as_str)
                            .or(item.insert_text.as_deref())
                            .unwrap_or(&item.label),
                    ),
                }),
            };
            let documentation = item.documentation.as_ref().and_then(markup_text);
            CompletionItem {
                label: item.label.clone(),
                kind: CompletionKind::from_lsp(item.kind.unwrap_or(1)),
                detail: item.detail.clone(),
                resolved: documentation.is_some() || !resolvable,
                documentation,
                filter_text: item.filter_text.clone(),
                sort_text: item.sort_text.clone(),
                insert_text: item.insert_text.as_deref().map(plain),
                edit,
            }
        })
        .collect()
}

/// Parameter Info in editor terms.
pub fn signature_data(help: &lsp::SignatureHelp) -> SignatureHelpData {
    let signatures = help
        .signatures
        .iter()
        .map(|s| SignatureInfo {
            label: s.label.clone(),
            documentation: s.documentation.as_ref().and_then(markup_text),
            parameters: (0..s.parameters.as_ref().map_or(0, Vec::len))
                .map(|i| s.parameter_range(i).unwrap_or(0..0))
                .collect(),
            active_parameter: s.active_parameter.map(|p| p as usize),
        })
        .collect::<Vec<_>>();
    let active_signature =
        (help.active_signature.unwrap_or(0) as usize).min(signatures.len().saturating_sub(1));
    SignatureHelpData {
        signatures,
        active_signature,
        active_parameter: help.active_parameter.map(|p| p as usize),
    }
}

impl Shell {
    /// Where completion items come from for `doc` now.
    pub(super) fn provider(&self, doc: &Document) -> Provider {
        if doc.language_id.is_none() {
            return Provider::Syntax;
        }
        match (self.ls_state, self.solution_state) {
            (None, _) => Provider::Syntax,
            (Some(LanguageServerState::Unavailable | LanguageServerState::Exited), _)
            | (_, Some(SolutionState::Failed)) => Provider::Syntax,
            (Some(LanguageServerState::Running), Some(SolutionState::Loaded) | None) => {
                Provider::Server
            }
            _ => Provider::ServerAndSyntax,
        }
    }

    /// Ask for completion in document `id` at its caret; the answer fills the editor's list when it arrives.
    pub(super) fn request_completion(
        &mut self,
        id: &str,
        trigger: Option<CompletionTrigger>,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        cancel(&mut doc.intellisense.completion);
        cancel(&mut doc.intellisense.resolve);
        let view = doc.view.clone();
        let request = view.update(cx, |v, cx| v.open_completion(trigger, cx));
        let provider = self.provider(&self.documents[id]);
        if provider != Provider::Server {
            view.update(cx, |v, cx| v.complete_from_syntax(request.id, cx));
        }
        if provider == Provider::Syntax {
            return;
        }
        self.flush_change(id, cx);
        let generation = self.generation;
        let doc = self.documents.get_mut(id).expect("checked above");
        let snapshot = doc.sent.clone();
        let version = doc.lsp_version;
        let (kind, character) = match trigger {
            Some(CompletionTrigger::Character(c))
                if self
                    .features
                    .completion_triggers
                    .iter()
                    .any(|t| t.chars().eq(std::iter::once(c))) =>
            {
                (2, Some(c.to_string()))
            }
            _ => (1, None),
        };
        let params = lsp::CompletionParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position: lsp_position(&snapshot, request.offset),
            context: Some(lsp::CompletionContext {
                trigger_kind: kind,
                trigger_character: character,
            }),
        };
        trace(format_args!(
            "completion request {} at {}:{} version {version}",
            request.id, params.position.line, params.position.character
        ));
        let (handle, rx) = self.session.request::<lsp::Completion>(params);
        let doc_id = id.to_owned();
        let editor_id = request.id;
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                shell.on_completion_reply(
                    &doc_id, editor_id, version, generation, snapshot, reply, cx,
                )
            });
        });
        doc.intellisense.completion = Some(Pending {
            handle,
            editor_id,
            _task: task,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn on_completion_reply(
        &mut self,
        id: &str,
        editor_id: u64,
        version: i32,
        generation: u64,
        snapshot: text::BufferSnapshot,
        reply: Reply<Option<lsp::CompletionResponse>>,
        cx: &mut Context<Self>,
    ) {
        let mut timing = CompletionTiming {
            sent: reply.sent,
            received: Some(reply.received),
            ..Default::default()
        };
        let current_generation = self.generation;
        let features_resolve = self.features.resolve;
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        let newest = doc
            .intellisense
            .completion
            .as_ref()
            .is_some_and(|p| p.editor_id == editor_id);
        let stale = !newest || generation != current_generation || version != doc.lsp_version;
        if stale {
            trace(format_args!(
                "completion reply {editor_id} dropped (stale: newest {newest}, generation {generation}/{current_generation}, version {version}/{})",
                doc.lsp_version
            ));
            timing.dropped = true;
            self.push_timing(timing);
            return;
        }
        doc.intellisense.completion = None;
        let view = doc.view.clone();
        let (items, incomplete) = match reply.result {
            Ok(Some(response)) => {
                let (mut raw, incomplete, defaults) = match response {
                    lsp::CompletionResponse::List(l) => {
                        let defaults = l.extra.get("itemDefaults").cloned();
                        (l.items, l.is_incomplete, defaults)
                    }
                    lsp::CompletionResponse::Items(items) => (items, false, None),
                };
                let items =
                    completion_items(&mut raw, defaults.as_ref(), &snapshot, features_resolve);
                doc.intellisense.raw = Some((editor_id, Arc::new(raw)));
                doc.intellisense.raw_base = Some(snapshot.clone());
                doc.intellisense.resolved.clear();
                (items, incomplete)
            }
            Ok(None) => (Vec::new(), false),
            Err(RequestError::Canceled | RequestError::Stale) => {
                timing.dropped = true;
                self.push_timing(timing);
                return;
            }
            Err(e) => {
                trace(format_args!("completion reply {editor_id} failed: {e:?}"));
                (Vec::new(), false)
            }
        };
        timing.items = items.len();
        trace(format_args!(
            "completion reply {editor_id}: {} items (host {:.1} ms)",
            items.len(),
            reply
                .sent
                .map_or(0., |s| (reply.received - s).as_secs_f64() * 1e3)
        ));
        view.update(cx, |v, cx| {
            v.set_completions(
                editor_id,
                items,
                CompletionSource::LanguageServer,
                incomplete,
                cx,
            )
        });
        timing.applied = Some(Instant::now());
        self.push_timing(timing);
    }

    fn push_timing(&mut self, timing: CompletionTiming) {
        if self.completion_timings.len() >= MAX_TIMINGS {
            self.completion_timings.remove(0);
        }
        self.completion_timings.push(timing);
    }

    /// Fetch the documentation of item `index` of list `list` (the selected item).
    fn resolve_completion(&mut self, id: &str, list: u64, index: usize, cx: &mut Context<Self>) {
        if !self.features.resolve {
            return;
        }
        let generation = self.generation;
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        cancel(&mut doc.intellisense.resolve);
        let Some(item) = doc
            .intellisense
            .raw
            .as_ref()
            .filter(|(l, _)| *l == list)
            .and_then(|(_, items)| items.get(index).cloned())
        else {
            return;
        };
        // The server resolves on the text it has, which is the text last sent (a pending change goes after this).
        let base = doc.sent.clone();
        let (handle, rx) = self.session.request::<lsp::ResolveCompletionItem>(item);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                if shell.generation != generation {
                    return;
                }
                let Some(doc) = shell.documents.get_mut(&doc_id) else {
                    return;
                };
                if !doc
                    .intellisense
                    .resolve
                    .as_ref()
                    .is_some_and(|p| p.editor_id == list)
                {
                    return;
                }
                doc.intellisense.resolve = None;
                if let Ok(resolved) = reply.result {
                    if doc
                        .intellisense
                        .raw
                        .as_ref()
                        .is_some_and(|(l, _)| *l == list)
                    {
                        doc.intellisense
                            .resolved
                            .insert(index, (additional_edits(&resolved), base));
                    }
                    let documentation = resolved.documentation.as_ref().and_then(markup_text);
                    doc.view.update(cx, |v, cx| {
                        v.set_completion_resolved(list, index, resolved.detail, documentation, cx)
                    });
                }
            });
        });
        doc.intellisense.resolve = Some(Pending {
            handle,
            editor_id: list,
            _task: task,
        });
    }

    /// Ask for Quick Info at `offset` of document `id`.
    pub(super) fn request_hover(&mut self, id: &str, offset: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        cancel(&mut doc.intellisense.hover);
        let view = doc.view.clone();
        let editor_id = view.update(cx, |v, cx| v.open_hover(offset, cx));
        if self.provider(&self.documents[id]) == Provider::Syntax {
            view.update(cx, |v, cx| v.set_hover(editor_id, None, None, cx));
            return;
        }
        self.flush_change(id, cx);
        let generation = self.generation;
        let doc = self.documents.get_mut(id).expect("checked above");
        let snapshot = doc.sent.clone();
        let version = doc.lsp_version;
        let params = lsp::TextDocumentPositionParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position: lsp_position(&snapshot, offset),
        };
        let (handle, rx) = self.session.request::<lsp::HoverRequest>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                let current = shell.generation;
                let Some(doc) = shell.documents.get_mut(&doc_id) else {
                    return;
                };
                let newest = doc
                    .intellisense
                    .hover
                    .as_ref()
                    .is_some_and(|p| p.editor_id == editor_id);
                if !newest || generation != current || version != doc.lsp_version {
                    trace(format_args!("hover reply {editor_id} dropped (stale)"));
                    return;
                }
                doc.intellisense.hover = None;
                let (text, range) = match reply.result {
                    Ok(Some(h)) => (
                        hover_markdown(&h.contents),
                        h.range
                            .map(|r| offset_in(&snapshot, r.start)..offset_in(&snapshot, r.end)),
                    ),
                    Ok(None) => (None, None),
                    Err(RequestError::Canceled | RequestError::Stale) => return,
                    Err(_) => (None, None),
                };
                trace(format_args!(
                    "hover reply {editor_id}: {} chars",
                    text.as_ref().map_or(0, String::len)
                ));
                doc.view.update(cx, |v, cx| {
                    v.set_hover(editor_id, text.as_deref(), range, cx)
                });
            });
        });
        doc.intellisense.hover = Some(Pending {
            handle,
            editor_id,
            _task: task,
        });
    }

    /// Ask for Parameter Info at the caret of document `id`.
    pub(super) fn request_signature_help(
        &mut self,
        id: &str,
        trigger: Option<SignatureTrigger>,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        cancel(&mut doc.intellisense.signature);
        let view = doc.view.clone();
        let retrigger = view.read(cx).signature_help().is_some_and(|s| s.visible);
        let editor_id = view.update(cx, |v, cx| v.open_signature_help(cx));
        if self.provider(&self.documents[id]) == Provider::Syntax {
            view.update(cx, |v, cx| v.set_signature_help(editor_id, None, cx));
            return;
        }
        self.flush_change(id, cx);
        let generation = self.generation;
        let triggers = self.features.signature_triggers.clone();
        let doc = self.documents.get_mut(id).expect("checked above");
        let snapshot = doc.sent.clone();
        let version = doc.lsp_version;
        let caret = view.read(cx).editor().primary_selection().head;
        let (kind, character) = match trigger {
            Some(SignatureTrigger::Character(c))
                if triggers.iter().any(|t| t.chars().eq(std::iter::once(c))) =>
            {
                (2, Some(c.to_string()))
            }
            Some(SignatureTrigger::Retrigger) => (3, None),
            _ => (1, None),
        };
        let params = lsp::SignatureHelpParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position: lsp_position(&snapshot, caret),
            context: Some(lsp::SignatureHelpContext {
                trigger_kind: kind,
                trigger_character: character,
                is_retrigger: retrigger,
                active_signature_help: retrigger
                    .then(|| doc.intellisense.last_signature.clone())
                    .flatten(),
            }),
        };
        let (handle, rx) = self.session.request::<lsp::SignatureHelpRequest>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                let current = shell.generation;
                let Some(doc) = shell.documents.get_mut(&doc_id) else {
                    return;
                };
                let newest = doc
                    .intellisense
                    .signature
                    .as_ref()
                    .is_some_and(|p| p.editor_id == editor_id);
                if !newest || generation != current || version != doc.lsp_version {
                    trace(format_args!(
                        "signature help reply {editor_id} dropped (stale)"
                    ));
                    return;
                }
                doc.intellisense.signature = None;
                let help = match reply.result {
                    Ok(help) => help,
                    Err(RequestError::Canceled | RequestError::Stale) => return,
                    Err(_) => None,
                };
                let data = help.as_ref().map(signature_data);
                trace(format_args!(
                    "signature help reply {editor_id}: {} signatures, active parameter {:?}",
                    data.as_ref().map_or(0, |d| d.signatures.len()),
                    data.as_ref().and_then(|d| d.active_parameter)
                ));
                doc.intellisense.last_signature = help;
                doc.view
                    .update(cx, |v, cx| v.set_signature_help(editor_id, data, cx));
            });
        });
        doc.intellisense.signature = Some(Pending {
            handle,
            editor_id,
            _task: task,
        });
    }

    /// What an editor asks for. Triggers run the IntelliSense commands, so typing, the keys, the mouse and agents
    /// go through the same commands (and the audit log); closing a popup cancels its request.
    pub(super) fn on_editor_event(
        &mut self,
        id: &str,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let run = |shell: &mut Self,
                   command: &str,
                   args: Value,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            if let Err(e) = shell.invoke(command, args, window, cx) {
                eprintln!("eludite: {command}: {e}");
            }
        };
        match event {
            EditorEvent::CompletionTriggered(trigger) => {
                let mut args = json!({ "path": id });
                if let CompletionTrigger::Typing(c) | CompletionTrigger::Character(c) = trigger {
                    args["trigger"] = json!(c.to_string());
                }
                run(self, workspace::EDITOR_COMPLETE, args, window, cx);
            }
            EditorEvent::ResolveCompletion { id: list, index } => {
                self.resolve_completion(id, *list, *index, cx)
            }
            EditorEvent::CompletionClosed => {
                if let Some(doc) = self.documents.get_mut(id) {
                    cancel(&mut doc.intellisense.completion);
                    cancel(&mut doc.intellisense.resolve);
                }
            }
            EditorEvent::HoverTriggered { offset } => {
                let Some(doc) = self.documents.get(id) else {
                    return;
                };
                let (line, column) = line_column(&doc.view, *offset, cx);
                run(
                    self,
                    workspace::EDITOR_HOVER,
                    json!({ "path": id, "line": line, "column": column }),
                    window,
                    cx,
                );
            }
            EditorEvent::HoverClosed => {
                if let Some(doc) = self.documents.get_mut(id) {
                    cancel(&mut doc.intellisense.hover);
                }
            }
            EditorEvent::SignatureHelpTriggered(trigger) => {
                let mut args = json!({ "path": id });
                if let SignatureTrigger::Character(c) = trigger {
                    args["trigger"] = json!(c.to_string());
                }
                run(self, workspace::EDITOR_SIGNATURE_HELP, args, window, cx);
            }
            EditorEvent::SignatureHelpClosed => {
                if let Some(doc) = self.documents.get_mut(id) {
                    cancel(&mut doc.intellisense.signature);
                    doc.intellisense.last_signature = None;
                }
            }
            // A click on the light bulb opens its menu, as Ctrl+. does.
            // A click on the light bulb opens its menu, as Ctrl+. does (the bulb is on the caret's line).
            EditorEvent::LightbulbClicked { .. } => run(
                self,
                workspace::EDITOR_CODE_ACTIONS,
                json!({ "path": id }),
                window,
                cx,
            ),
            // The debugger's (brief 0018).
            EditorEvent::BreakpointMarginClicked { .. } => {}
            // Ctrl+click: Go To Definition, through the same command as F12.
            EditorEvent::GoToDefinition { offset } => {
                let Some(doc) = self.documents.get(id) else {
                    return;
                };
                let (line, column) = line_column(&doc.view, *offset, cx);
                run(
                    self,
                    workspace::EDITOR_GO_TO_DEFINITION,
                    json!({ "path": id, "line": line, "column": column }),
                    window,
                    cx,
                );
            }
        }
    }

    // ----- the commands (brief 0013) -----

    /// Fires the next time an editor changes (an answer arrived, a list was filtered).
    pub(super) fn intellisense_waiter(&mut self) -> futures::channel::oneshot::Receiver<()> {
        let (tx, rx) = futures::channel::oneshot::channel();
        self.intellisense_waiters.push(tx);
        rx
    }

    pub(super) fn wake_intellisense_waiters(&mut self) {
        for w in self.intellisense_waiters.drain(..) {
            let _ = w.send(());
        }
    }

    /// `eludite.editor.complete`.
    pub(super) fn complete_command(
        &mut self,
        path: Option<&str>,
        caret: Option<Caret>,
        trigger: Option<char>,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        if let Some((line, column)) = caret {
            move_caret(&self.documents[&id].view, line, column, cx);
        }
        let trigger = trigger.map(|c| {
            if eludite_editor::intellisense::is_identifier_char(c) {
                CompletionTrigger::Typing(c)
            } else {
                CompletionTrigger::Character(c)
            }
        });
        self.request_completion(&id, trigger, cx);
        Ok(WorkspaceOutput::Complete(self.completion_output(&id, cx)))
    }

    /// `eludite.editor.accept_completion`.
    pub(super) fn accept_completion_command(
        &mut self,
        path: Option<&str>,
        label: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        let view = self.documents[&id].view.clone();
        let accepted = view.update(cx, |v, cx| v.accept_completion(label, cx));
        if let Some(a) = &accepted {
            trace(format_args!("accept completion {:?}", a.label));
            self.apply_additional_edits(&id, a.list, a.index, cx);
        }
        let caret = view.read(cx).editor().primary_selection().head;
        let (line, column) = line_column(&view, caret, cx);
        Ok(WorkspaceOutput::AcceptCompletion(AcceptCompletionOutput {
            path: id,
            accepted: accepted.is_some(),
            label: accepted.as_ref().map(|a| a.label.clone()),
            text: accepted.map(|a| a.text),
            line,
            column,
        }))
    }

    /// The committed item's `additionalTextEdits` (a `using` for an unimported type, brief 0015), through the
    /// workspace-edit applier: at once when the item or its lazy resolve carried them, else after a
    /// `completionItem/resolve`. They join the commit's undo step when nothing was typed in between.
    fn apply_additional_edits(
        &mut self,
        id: &str,
        list: u64,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let generation = self.generation;
        let resolvable = self.features.resolve;
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        cancel(&mut doc.intellisense.accept_resolve);
        let Some((raw_list, items)) = &doc.intellisense.raw else {
            return;
        };
        if *raw_list != list {
            return;
        }
        let Some(item) = items.get(index).cloned() else {
            return;
        };
        let commit = doc.view.read(cx).editor().last_transaction();
        let inline = additional_edits(&item);
        let known = if !inline.is_empty() {
            doc.intellisense.raw_base.clone().map(|b| (inline, b))
        } else {
            doc.intellisense.resolved.get(&index).cloned()
        };
        if let Some((edits, base)) = known {
            if !edits.is_empty() {
                self.apply_completion_edits(id, &edits, &base, commit, cx);
            }
            return;
        }
        if !resolvable || item.data.is_none() {
            return;
        }
        // Resolve now: the server has the text last sent (the commit's change is queued after this request).
        let base = doc.sent.clone();
        let label = item.label.clone();
        let (handle, rx) = self.session.request::<lsp::ResolveCompletionItem>(item);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                if shell.generation != generation {
                    return;
                }
                let Some(doc) = shell.documents.get_mut(&doc_id) else {
                    return;
                };
                if !doc
                    .intellisense
                    .accept_resolve
                    .as_ref()
                    .is_some_and(|p| p.editor_id == list)
                {
                    return;
                }
                doc.intellisense.accept_resolve = None;
                let Ok(resolved) = reply.result else {
                    return;
                };
                let edits = additional_edits(&resolved);
                trace(format_args!(
                    "accept resolve {label:?}: {} additional edits",
                    edits.len()
                ));
                if !edits.is_empty() {
                    shell.apply_completion_edits(&doc_id, &edits, &base, commit, cx);
                }
                shell.wake_intellisense_waiters();
            });
        });
        if let Some(doc) = self.documents.get_mut(id) {
            doc.intellisense.accept_resolve = Some(Pending {
                handle,
                editor_id: list,
                _task: task,
            });
        }
    }

    fn apply_completion_edits(
        &mut self,
        id: &str,
        edits: &[lsp::TextEdit],
        base: &text::BufferSnapshot,
        commit: Option<eludite_editor::text::TransactionId>,
        cx: &mut Context<Self>,
    ) {
        let still_last = self
            .documents
            .get(id)
            .map(|d| d.view.read(cx).editor().last_transaction())
            == Some(commit);
        match self.apply_document_edits(id, edits, base, cx) {
            Ok((n, Some(tx))) => {
                if still_last
                    && let Some(commit) = commit
                    && let Some(doc) = self.documents.get(id)
                {
                    doc.view.update(cx, |v, cx| {
                        v.update_editor(cx, |e| e.merge_transactions(tx, commit))
                    });
                }
                trace(format_args!("completion: {n} additional edits applied"));
            }
            Ok((_, None)) => {}
            Err(e) => trace(format_args!(
                "completion: additional edits not applied: {e}"
            )),
        }
    }

    /// `eludite.editor.hover`.
    pub(super) fn hover_command(
        &mut self,
        path: Option<&str>,
        at: Option<Caret>,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        let view = self.documents[&id].view.clone();
        let offset = match at {
            Some((line, column)) => offset_of(view.read(cx).editor().buffer(), line, column),
            None => view.read(cx).editor().primary_selection().head,
        };
        self.request_hover(&id, offset, cx);
        Ok(WorkspaceOutput::Hover(self.hover_output(&id, cx)))
    }

    /// `eludite.editor.signature_help`.
    pub(super) fn signature_help_command(
        &mut self,
        path: Option<&str>,
        caret: Option<Caret>,
        trigger: Option<char>,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        let view = self.documents[&id].view.clone();
        if let Some((line, column)) = caret {
            move_caret(&view, line, column, cx);
        }
        let open = view.read(cx).signature_help().is_some_and(|s| s.visible);
        let trigger = match trigger {
            Some(c) => SignatureTrigger::Character(c),
            None if open => SignatureTrigger::Retrigger,
            None => SignatureTrigger::Invoked,
        };
        self.request_signature_help(&id, Some(trigger), cx);
        Ok(WorkspaceOutput::SignatureHelp(
            self.signature_output(&id, cx),
        ))
    }

    /// The current state of the popup an IntelliSense command shows, for an agent waiting for the answer.
    pub(super) fn intellisense_state(
        &self,
        request: &WorkspaceRequest,
        cx: &Context<Self>,
    ) -> Option<Result<WorkspaceOutput, CommandError>> {
        if matches!(
            request,
            WorkspaceRequest::ApplyCodeAction { .. } | WorkspaceRequest::ApplyEdit { .. }
        ) {
            return Some(Ok(match request {
                WorkspaceRequest::ApplyCodeAction { .. } => {
                    WorkspaceOutput::ApplyCodeAction(self.apply_code_action_output())
                }
                _ => WorkspaceOutput::ApplyEdit(self.apply_edit_output()),
            }));
        }
        let id = match self.document_id(request.path()) {
            Ok(id) => id,
            Err(e) => return Some(Err(e)),
        };
        Some(Ok(match request {
            WorkspaceRequest::Complete { .. } => {
                WorkspaceOutput::Complete(self.completion_output(&id, cx))
            }
            WorkspaceRequest::Hover { .. } => WorkspaceOutput::Hover(self.hover_output(&id, cx)),
            WorkspaceRequest::SignatureHelp { .. } => {
                WorkspaceOutput::SignatureHelp(self.signature_output(&id, cx))
            }
            WorkspaceRequest::GoToDefinition { .. } => {
                WorkspaceOutput::GoToDefinition(self.definition_output())
            }
            WorkspaceRequest::FindReferences { .. } => {
                WorkspaceOutput::FindReferences(self.references_output(cx))
            }
            WorkspaceRequest::Rename { .. } => WorkspaceOutput::Rename(self.rename_output()),
            WorkspaceRequest::CodeActions { .. } => {
                WorkspaceOutput::CodeActions(self.code_actions_output())
            }
            WorkspaceRequest::ApplyCodeAction { .. } => {
                WorkspaceOutput::ApplyCodeAction(self.apply_code_action_output())
            }
            WorkspaceRequest::ApplyEdit { .. } => {
                WorkspaceOutput::ApplyEdit(self.apply_edit_output())
            }
            _ => return None,
        }))
    }

    fn completion_output(&self, id: &str, cx: &gpui::App) -> CompleteOutput {
        let view = &self.documents[id].view;
        let caret = view.read(cx).editor().primary_selection().head;
        let (line, column) = line_column(view, caret, cx);
        let snapshot = view.read(cx).completion();
        let state = match &snapshot {
            Some(s) if s.visible => PopupState::Open,
            Some(s) if s.loading => PopupState::Loading,
            _ => PopupState::Closed,
        };
        let snapshot = snapshot.filter(|s| s.visible);
        CompleteOutput {
            path: id.to_owned(),
            line,
            column,
            state,
            source: snapshot
                .as_ref()
                .and_then(|s| s.source)
                .map(|s| s.name().to_owned()),
            filter: snapshot
                .as_ref()
                .map(|s| s.filter.clone())
                .unwrap_or_default(),
            total: snapshot.as_ref().map_or(0, |s| s.items.len() as u64),
            selected: snapshot
                .as_ref()
                .and_then(|s| s.selected.and_then(|i| s.items.get(i)))
                .map(|i| i.0.clone()),
            items: snapshot
                .map(|s| {
                    s.items
                        .into_iter()
                        .take(MAX_COMPLETION_ROWS)
                        .map(|(label, kind, detail)| CompletionRow {
                            label,
                            kind: kind.name().to_owned(),
                            detail,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn hover_output(&self, id: &str, cx: &gpui::App) -> HoverOutput {
        let view = &self.documents[id].view;
        let hover = view.read(cx).hover();
        let offset = hover.as_ref().map_or_else(
            || view.read(cx).editor().primary_selection().head,
            |h| h.offset,
        );
        let (line, column) = line_column(view, offset, cx);
        HoverOutput {
            path: id.to_owned(),
            line,
            column,
            state: match &hover {
                Some(h) if h.visible => PopupState::Open,
                Some(h) if h.loading => PopupState::Loading,
                _ => PopupState::Closed,
            },
            text: hover.and_then(|h| h.text),
        }
    }

    fn signature_output(&self, id: &str, cx: &gpui::App) -> SignatureHelpOutput {
        let view = &self.documents[id].view;
        let caret = view.read(cx).editor().primary_selection().head;
        let (line, column) = line_column(view, caret, cx);
        let help = view.read(cx).signature_help();
        let state = match &help {
            Some(h) if h.visible => PopupState::Open,
            Some(h) if h.loading => PopupState::Loading,
            _ => PopupState::Closed,
        };
        let visible = help.filter(|h| h.visible);
        SignatureHelpOutput {
            path: id.to_owned(),
            line,
            column,
            state,
            signatures: visible
                .as_ref()
                .and_then(|h| h.data.as_ref())
                .map(|d| {
                    d.signatures
                        .iter()
                        .map(|s| SignatureRow {
                            label: s.label.clone(),
                            documentation: s.documentation.clone(),
                            parameters: s
                                .parameters
                                .iter()
                                .map(|r| s.label.get(r.clone()).unwrap_or_default().to_owned())
                                .collect(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            active_signature: visible.as_ref().map(|h| h.active_signature as u32),
            active_parameter: visible
                .as_ref()
                .and_then(|h| h.active_parameter)
                .map(|p| p as u32),
        }
    }

    /// The solution finished loading: lists still showing the syntax fallback ask the server again.
    pub(super) fn refresh_fallback_lists(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .documents
            .iter()
            .filter(|(_, d)| {
                d.view
                    .read(cx)
                    .completion()
                    .is_some_and(|c| c.source == Some(CompletionSource::Syntax))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.request_completion(&id, None, cx);
        }
    }
}

/// A completion item's `additionalTextEdits` (none when absent or malformed).
fn additional_edits(item: &lsp::CompletionItem) -> Vec<lsp::TextEdit> {
    item.extra
        .get("additionalTextEdits")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// 1-based line and character column of `offset`.
pub(super) fn line_column(view: &Entity<EditorView>, offset: usize, cx: &gpui::App) -> (u32, u32) {
    let buffer = view.read(cx).editor().buffer();
    let p = buffer.offset_to_point(offset.min(buffer.len()));
    let line = buffer.line(p.row);
    let column = line[..(p.column as usize).min(line.len())].chars().count() as u32 + 1;
    (p.row + 1, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_editor::Buffer;
    use serde_json::json;

    #[test]
    fn hover_contents_become_markdown() {
        assert_eq!(
            hover_markdown(&json!({"kind": "markdown", "value": "**x**"})).as_deref(),
            Some("**x**")
        );
        assert_eq!(
            hover_markdown(&json!([{"language": "csharp", "value": "int x"}, "doc"])).as_deref(),
            Some("```csharp\nint x\n```\n\ndoc")
        );
        assert_eq!(hover_markdown(&json!([])), None);
    }

    #[test]
    fn items_use_text_edits_defaults_and_plain_snippets() {
        let buffer = Buffer::new("Console.Wr\n");
        let snapshot = buffer.snapshot().clone();
        let range =
            json!({"start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 10}});
        let mut raw: Vec<lsp::CompletionItem> = serde_json::from_value(json!([
            {"label": "WriteLine", "kind": 2, "textEdit": {"range": range, "newText": "WriteLine"}},
            {"label": "Write", "kind": 2, "insertText": "Write($1)$0", "insertTextFormat": 2},
            {"label": "Out", "kind": 10, "documentation": {"kind": "markdown", "value": "The output."}},
            {"label": "Replace", "textEdit": {"insert": range, "replace": range, "newText": "Replace"}}
        ]))
        .unwrap();
        let defaults = json!({"editRange": range, "data": {"id": 7}});
        let items = completion_items(&mut raw, Some(&defaults), &snapshot, true);
        let span = |e: &CompletionEdit| {
            buffer.offset_for_anchor(&e.range.start)..buffer.offset_for_anchor(&e.range.end)
        };
        assert_eq!(items[0].kind, CompletionKind::Method);
        assert_eq!(span(items[0].edit.as_ref().unwrap()), 8..10);
        // No textEdit: the default edit range, with the snippet as plain text.
        assert_eq!(items[1].insert_text.as_deref(), Some("Write()"));
        assert_eq!(items[1].edit.as_ref().unwrap().new_text, "Write()");
        assert_eq!(items[2].kind, CompletionKind::Property);
        assert!(items[2].resolved && !items[0].resolved);
        assert_eq!(items[2].documentation.as_deref(), Some("The output."));
        assert_eq!(span(items[3].edit.as_ref().unwrap()), 8..10);
        // The default data is filled in for completionItem/resolve.
        assert_eq!(raw[0].data, Some(json!({"id": 7})));
        // Without resolve support nothing is fetched lazily.
        assert!(completion_items(&mut raw, None, &snapshot, false)[0].resolved);
    }

    #[test]
    fn signatures_find_their_parameters() {
        let help: lsp::SignatureHelp = serde_json::from_value(json!({"signatures": [
            {"label": "void Console.WriteLine(string format, object? arg0)",
             "documentation": {"kind": "markdown", "value": "Writes."},
             "parameters": [{"label": "string format"}, {"label": "object? arg0"}]}],
            "activeSignature": 3, "activeParameter": 1}))
        .unwrap();
        let data = signature_data(&help);
        let s = &data.signatures[0];
        assert_eq!(&s.label[s.parameters[1].clone()], "object? arg0");
        assert_eq!(data.active_signature, 0, "clamped");
        assert_eq!(data.active_parameter, Some(1));
        assert_eq!(s.documentation.as_deref(), Some("Writes."));
    }

    #[test]
    fn features_from_roslyn_capabilities() {
        let f = ServerFeatures::from_capabilities(&json!({
            "completionProvider": {"triggerCharacters": [".", "<"], "resolveProvider": true},
            "signatureHelpProvider": {"triggerCharacters": ["(", ","], "retriggerCharacters": [")"]}}));
        assert_eq!(f.completion_triggers, [".", "<"]);
        assert!(f.resolve);
        assert_eq!(f.signature_triggers, ["(", ",", ")"]);
        assert!(!ServerFeatures::from_capabilities(&json!({})).resolve);
    }
}
