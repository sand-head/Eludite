//! Open documents (brief 0012): one document tab per file with an [`EditorView`], a dirty marker, saving, and the
//! LSP document notifications.
//!
//! - **Open** reads the file off the UI thread, then creates the editor and sends `textDocument/didOpen` to the
//!   document's language server: the registration that matches its file name (`eludite-lsp`'s `servers.json`)
//!   decides whether that is `eludite-host` or a generic server such as rust-analyzer (brief 0019, `servers`).
//! - **Edits** mark the tab dirty at once and send `textDocument/didChange` [`DIDCHANGE_DEBOUNCE`] after the last
//!   edit: the whole text goes to the session worker, which diffs it against what it last sent and sends one
//!   incremental change. The host then pulls diagnostics 150 ms later (its own debounce, host-rpc.md).
//! - **Diagnostics** become wavy underlines in the `diagnostics` decoration layer, anchored in the text as it was
//!   when its version was sent, so they follow later edits.
//! - **Close** sends `textDocument/didClose`; a dirty document needs an answer (save or discard) first.
//! - **Metadata as source** (brief 0014): a decompiled type from a referenced assembly (a file under
//!   `<temp>/MetadataAsSource/`, host-rpc.md) opens read-only, titled `Type [from metadata]`, and sends no document
//!   notifications; the language server answers requests in it from its own metadata workspace.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::workspace::{
    CloseSave, FileCloseOutput, FileOpenOutput, FindOutput, HistoryOutput, SaveOutput,
    WorkspaceOutput,
};
use eludite_editor::text::{self, PointUtf16, Unclipped};
use eludite_editor::{Buffer, Decoration, DecorationStyle, EditorView, FindQuery};
use eludite_lsp::lsp;
use gpui::{AppContext as _, Context, Entity, Focusable as _, Subscription, Task, Window, rgb};

use super::Shell;
use super::intellisense::{DocIntellisense, Provider};
use super::servers::ServerKey;
use super::session::ServerSession;

/// Quiet time after the last edit before `textDocument/didChange` is sent.
pub const DIDCHANGE_DEBOUNCE: Duration = Duration::from_millis(50);

/// The decoration layer diagnostics use.
pub const DIAGNOSTICS_LAYER: &str = "diagnostics";

pub struct Document {
    pub path: PathBuf,
    pub uri: String,
    /// The LSP language id (`csharp`, `rust`) for files a language server handles; others get no LSP traffic.
    pub language_id: Option<String>,
    /// The server the document belongs to (brief 0019): its primary one when several serve it (brief 0050).
    pub server: ServerKey,
    /// Every server that serves it, the primary first (brief 0050: TypeScript and ESLint).
    pub servers: Vec<ServerKey>,
    /// Its servers' session (fanned out when there are several): every LSP message about the document goes through
    /// it.
    pub session: ServerSession,
    /// Metadata as source: read-only, and no `didOpen`, `didChange`, `didSave` or `didClose`.
    pub read_only: bool,
    pub view: Entity<EditorView>,
    /// The LSP version last sent (`didOpen` is 1).
    pub lsp_version: i32,
    /// The text as of `lsp_version`, to anchor diagnostics computed on it.
    pub(super) sent: text::BufferSnapshot,
    saved: clock::Global,
    seen: clock::Global,
    pub dirty: bool,
    debounce: Option<Task<()>>,
    /// IntelliSense requests in flight (brief 0013).
    pub(super) intellisense: DocIntellisense,
    _observe: Subscription,
    _events: Subscription,
}

impl Document {
    pub fn clear_diagnostics(&self, cx: &mut gpui::App) {
        self.view
            .update(cx, |v, cx| v.clear_decorations(DIAGNOSTICS_LAYER, cx));
    }

    /// Draw `diagnostics` (computed on the text of `lsp_version`) as squiggles.
    pub fn show_diagnostics(&self, diagnostics: &[lsp::Diagnostic], cx: &mut gpui::App) {
        let decorations = decorations(&self.sent, diagnostics);
        self.view.update(cx, |v, cx| {
            v.set_decorations(DIAGNOSTICS_LAYER, decorations, cx)
        });
    }
}

/// Squiggle color by LSP severity, as Visual Studio draws them: red errors, green warnings, grey messages.
fn squiggle(severity: Option<u8>) -> Option<gpui::Rgba> {
    match severity {
        Some(2) => Some(rgb(0x4EC94E)),
        Some(3) => Some(rgb(0x9D9D9D)),
        Some(4) => None,
        _ => Some(rgb(0xF14C4C)),
    }
}

/// Wavy underlines for `diagnostics`, anchored in `snapshot`. An empty range covers the next character (or the
/// previous one at the end of a line), so it stays visible.
pub fn decorations(
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
        .filter_map(|d| {
            let color = squiggle(d.severity)?;
            let mut start = offset(d.range.start);
            let mut end = offset(d.range.end).max(start);
            if start == end {
                let len = snapshot.len();
                let next = snapshot.chars_at(start).next();
                match next {
                    Some(c) if c != '\n' => end = start + c.len_utf8(),
                    _ if start > 0 => {
                        let prev = snapshot
                            .reversed_chars_at(start)
                            .next()
                            .map_or(1, char::len_utf8);
                        start -= prev;
                    }
                    _ if len > 0 => end = snapshot.chars_at(0).next().map_or(1, char::len_utf8),
                    _ => {}
                }
            }
            Some(Decoration {
                range: snapshot.anchor_before(start)..snapshot.anchor_after(end),
                style: DecorationStyle::Underline { color, wavy: true },
            })
        })
        .collect()
}

/// The path form document ids use: components re-joined with the native separator, so on Windows
/// `C:\\a/b.cs` (a tree path built with `/`) and `C:\\a\\b.cs` (the same file back from a URI or a dialog)
/// name one document. Without this, click-through from the Error List opened a second tab for an
/// already open file on Windows CI (brief 0012).
pub fn normalize_path(path: &Path) -> PathBuf {
    path.components().collect()
}

/// `file:///abs/path` with reserved and non-ASCII bytes percent-encoded.
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The path of a `file://` URI.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    // file:///C:/x on Windows.
    let s = if s.len() > 2 && s.as_bytes()[2] == b':' {
        s[1..].to_owned()
    } else {
        s
    };
    // Documents are keyed by the path string the tree or dialog produced, which uses the
    // native separator; a URI always uses `/`, so restore `\` on Windows or the lookup
    // in `Shell::on_diagnostics` misses and squiggles never appear.
    #[cfg(windows)]
    let s = s.replace('/', "\\");
    Some(PathBuf::from(s))
}

/// `ELUDITE_TRACE_LSP=1` logs document notifications and diagnostics to stderr with wall-clock milliseconds (the
/// manual timing runs).
pub fn trace(what: std::fmt::Arguments<'_>) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("ELUDITE_TRACE_LSP").is_some_and(|v| v != "0")) {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        eprintln!("[lsp] {ms} {what}");
    }
}

fn title(path: &Path) -> String {
    super::navigation::document_title(path)
}

/// Byte offset of a 1-based line and character column, clipped to the text.
pub(super) fn offset_of(buffer: &Buffer, line: u32, column: u32) -> usize {
    let row = line
        .saturating_sub(1)
        .min(buffer.line_count().saturating_sub(1));
    let text = buffer.line(row);
    let byte = text
        .char_indices()
        .nth(column.saturating_sub(1) as usize)
        .map_or(text.len(), |(i, _)| i);
    buffer.point_to_offset(text::Point::new(row, byte as u32))
}

impl Shell {
    /// When `request` names a document that is still being read (or no path while the active one is), a receiver
    /// that fires once it is open (or failed to open).
    pub(super) fn wait_for_load(
        &mut self,
        request: &eludite_commands::workspace::WorkspaceRequest,
    ) -> Option<futures::channel::oneshot::Receiver<()>> {
        use eludite_commands::workspace::WorkspaceRequest::*;
        let path = match request {
            SolutionOpen { .. } | SolutionClose | FileOpen { .. } => return None,
            other => other.path().map(str::to_owned),
        };
        let id = match path {
            Some(p) => self.resolve_file(&p).to_string_lossy().into_owned(),
            None => self.controller.active_document()?,
        };
        if !self.loading.contains_key(&id) {
            return None;
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        self.load_waiters.entry(id).or_default().push(tx);
        Some(rx)
    }

    pub(super) fn resolve_file(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace_root()
                .map(|d| d.join(p))
                .or_else(|| std::path::absolute(p).ok())
                .unwrap_or_else(|| p.to_path_buf())
        };
        normalize_path(&resolved)
    }

    /// The document `path` names, or the active one.
    pub(super) fn document_id(&self, path: Option<&str>) -> Result<String, CommandError> {
        let id = match path {
            Some(p) => self.resolve_file(p).to_string_lossy().into_owned(),
            None => self
                .controller
                .active_document()
                .ok_or_else(|| CommandError::Failed("no document is active".into()))?,
        };
        if self.documents.contains_key(&id) {
            Ok(id)
        } else {
            Err(CommandError::Failed(format!(
                "{id} is not open in an editor"
            )))
        }
    }

    pub(super) fn open_file(
        &mut self,
        path: &str,
        caret: Option<(u32, u32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let path = self.resolve_file(path);
        let id = path.to_string_lossy().into_owned();
        let output = |already_open| {
            Ok(WorkspaceOutput::FileOpen(FileOpenOutput {
                path: id.clone(),
                already_open,
            }))
        };
        if let Some(doc) = self.documents.get(&id) {
            self.controller.open_document(&id, &title(&path));
            let view = doc.view.clone();
            if let Some((line, column)) = caret {
                move_caret(&view, line, column, cx);
            }
            view.focus_handle(cx).focus(window, cx);
            return output(true);
        }
        if let Some((_, pending)) = self.loading.get_mut(&id) {
            if caret.is_some() {
                *pending = caret;
            }
            self.controller.open_document(&id, &title(&path));
            return output(true);
        }
        if !path.is_file() {
            return Err(CommandError::InvalidInput(format!(
                "{} is not a file",
                path.display()
            )));
        }
        self.controller.open_document(&id, &title(&path));
        let load_path = path.clone();
        let task_id = id.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { Buffer::load(&load_path) })
                .await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.finish_open(task_id, loaded, window, cx)
            });
        });
        self.loading.insert(id.clone(), (task, caret));
        output(false)
    }

    fn finish_open(
        &mut self,
        id: String,
        loaded: Result<Buffer, eludite_editor::LoadError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, caret)) = self.loading.remove(&id) else {
            return; // Closed while loading.
        };
        // Wake waiting agents after this update, when the document exists.
        let waiters = self.load_waiters.remove(&id).unwrap_or_default();
        cx.defer(move |_| {
            for w in waiters {
                let _ = w.send(());
            }
        });
        let path = PathBuf::from(&id);
        let buffer = match loaded {
            Ok(b) => b,
            Err(e) => {
                self.controller.close_document(&id);
                self.status.set(
                    eludite_ui::slots::STATE,
                    format!("Cannot open {}: {e}", path.display()),
                );
                cx.notify();
                return;
            }
        };
        let language = self.languages.for_path(&path);
        let view = cx.new(|cx| EditorView::new(buffer, language, cx));
        // Emmet on Tab in HTML and CSS (brief 0050's `editor.emmet`; the language says whether it has Emmet).
        let emmet = self.launches.emmet;
        view.update(cx, |v, _| v.set_emmet(emmet));
        let (text, sent, version) = {
            let b = view.read(cx).editor().buffer();
            (b.text(), b.snapshot().clone(), b.version())
        };
        let uri = super::documents::path_to_uri(&path);
        let read_only = super::navigation::is_metadata_path(&path);
        let (servers, session, language_id) = match self.server_for_path(&path, window, cx) {
            Some((servers, session, language)) => (servers, session, Some(language)),
            None => (vec![ServerKey::Host], self.session.clone(), None),
        };
        let server = servers.first().cloned().unwrap_or_default();
        if read_only {
            view.update(cx, |v, cx| v.set_read_only(true, cx));
        } else if let Some(lang) = &language_id {
            trace(format_args!("didOpen {uri} version 1"));
            session.did_open(uri.clone(), lang, 1, text);
        }
        let observe_id = id.clone();
        let observe = cx.observe(&view, move |shell, _, cx| {
            shell.on_editor_changed(&observe_id, cx)
        });
        let events_id = id.clone();
        let events = cx.subscribe_in(&view, window, move |shell, _, event, window, cx| {
            shell.on_editor_event(&events_id, event, window, cx)
        });
        self.views.borrow_mut().insert(id.clone(), view.clone());
        self.documents.insert(
            id.clone(),
            Document {
                path: path.clone(),
                uri: uri.clone(),
                language_id,
                server,
                servers,
                session,
                read_only,
                view: view.clone(),
                lsp_version: 1,
                sent,
                saved: version.clone(),
                seen: version,
                dirty: false,
                debounce: None,
                intellisense: DocIntellisense::default(),
                _observe: observe,
                _events: events,
            },
        );
        // Read the fallback identifiers now if the language server cannot answer yet.
        if self
            .documents
            .get(&id)
            .is_some_and(|d| self.provider(d) != Provider::Server)
        {
            view.update(cx, |v, cx| v.prefetch_identifiers(cx));
        }
        // Diagnostics that arrived before the editor existed.
        if let Some(diags) = self.diagnostics.get(&uri)
            && let Some(doc) = self.documents.get(&id)
        {
            doc.show_diagnostics(diags, cx);
        }
        if let Some((line, column)) = caret {
            move_caret(&view, line, column, cx);
        }
        if self.controller.active_document().as_deref() == Some(id.as_str()) {
            view.focus_handle(cx).focus(window, cx);
        }
        self.explorer.update(cx, |e, cx| e.reveal(&path, cx));
        // Breakpoints and the debugger's execution point (brief 0018).
        self.debug_document_opened(cx);
        // The tab's source control glyph and the change margin (brief 0040).
        self.git_document_opened(&id, cx);
        // CodeLens rows, when the settings show any for the document's language (brief 0052).
        self.code_lens_document_opened(&id, cx);
        if self.timings.editable.is_none() {
            // Editable once the frame that shows the editor has been drawn.
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |shell, _| {
                    shell.timings.editable.get_or_insert_with(Instant::now);
                });
            });
        }
        self.dock.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    fn on_editor_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        self.wake_intellisense_waiters();
        // Breakpoints follow their lines through edits (brief 0018).
        if self
            .documents
            .get(id)
            .is_some_and(|d| d.view.read(cx).editor().buffer().version() != d.seen)
        {
            self.debug_sync_lines(id, cx);
        }
        // The light bulb follows the caret (brief 0015).
        self.probe_lightbulb(id, cx);
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        let version = doc.view.read(cx).editor().buffer().version();
        if version == doc.seen {
            return; // A repaint, a caret move, a scroll.
        }
        doc.seen = version.clone();
        let dirty = version != doc.saved;
        if dirty != doc.dirty {
            doc.dirty = dirty;
            self.controller.set_document_dirty(id, dirty);
        }
        // The change margin, once the editor is idle (brief 0040).
        self.git_editor_changed(id, cx);
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        if doc.language_id.is_none() || doc.read_only {
            return;
        }
        let task_id = id.to_owned();
        doc.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DIDCHANGE_DEBOUNCE).await;
            let _ = this.update(cx, |shell, cx| shell.flush_change(&task_id, cx));
        }));
    }

    /// Send the document's text now (the debounce expired, or a save or a request needs the host up to date).
    pub(super) fn flush_change(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get_mut(id) else {
            return;
        };
        doc.debounce = None;
        let buffer = doc.view.read(cx).editor().buffer();
        if doc.read_only || buffer.snapshot().version() == doc.sent.version() {
            return;
        }
        let text = buffer.text();
        doc.sent = buffer.snapshot().clone();
        doc.lsp_version += 1;
        trace(format_args!(
            "didChange {} version {}",
            doc.uri, doc.lsp_version
        ));
        doc.session
            .did_change(doc.uri.clone(), doc.lsp_version, text);
    }

    pub(super) fn save(
        &mut self,
        path: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        if self.documents[&id].read_only {
            return Err(CommandError::Failed(format!("{id} is read-only")));
        }
        if self.documents[&id].debounce.is_some() {
            self.flush_change(&id, cx);
        }
        let doc = self.documents.get_mut(&id).expect("checked above");
        let (bytes, version) = {
            let b = doc.view.read(cx).editor().buffer();
            (b.to_file_bytes(), b.version())
        };
        std::fs::write(&doc.path, &bytes)
            .map_err(|e| CommandError::Failed(format!("{}: {e}", doc.path.display())))?;
        doc.saved = version;
        if doc.dirty {
            doc.dirty = false;
            self.controller.set_document_dirty(&id, false);
        }
        if doc.language_id.is_some() {
            doc.session.did_save(doc.uri.clone());
        }
        // The repository's status follows the saved file (brief 0040).
        self.git_saved();
        Ok(WorkspaceOutput::Save(SaveOutput {
            path: id,
            bytes: bytes.len() as u64,
            pending: false,
        }))
    }

    pub(super) fn history(
        &mut self,
        path: Option<&str>,
        undo: bool,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        let view = self.documents[&id].view.clone();
        if self.documents[&id].read_only {
            return Ok(WorkspaceOutput::History(HistoryOutput {
                path: id,
                applied: false,
                dirty: false,
            }));
        }
        let applied = view.update(cx, |v, cx| {
            v.update_editor(cx, |e| if undo { e.undo() } else { e.redo() })
        });
        self.on_editor_changed(&id, cx);
        Ok(WorkspaceOutput::History(HistoryOutput {
            dirty: self.documents[&id].dirty,
            path: id,
            applied,
        }))
    }

    pub(super) fn find(
        &mut self,
        path: Option<&str>,
        query: Option<String>,
        case_sensitive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        let view = self.documents[&id].view.clone();
        let Some(text) = query else {
            // Open the editor's find bar, as Ctrl+F does.
            view.focus_handle(cx).focus(window, cx);
            window.dispatch_action(Box::new(eludite_editor::actions::Find), cx);
            return Ok(WorkspaceOutput::Find(FindOutput {
                path: id,
                found: false,
                line: None,
                column: None,
                find_bar_open: Some(true),
            }));
        };
        let found = view.update(cx, |v, cx| {
            v.update_editor(cx, |e| {
                e.set_find_query(FindQuery {
                    text,
                    case_sensitive,
                });
                e.find_next()
            })
        });
        let (line, column) = {
            let e = view.read(cx).editor();
            let start = e.primary_selection().start();
            let point = e.buffer().offset_to_point(start);
            let line_text = e.buffer().line(point.row);
            let column = line_text[..point.column as usize].chars().count() as u32 + 1;
            (point.row + 1, column)
        };
        Ok(WorkspaceOutput::Find(FindOutput {
            path: id,
            found,
            line: found.then_some(line),
            column: found.then_some(column),
            find_bar_open: None,
        }))
    }

    pub(super) fn close_file(
        &mut self,
        path: &str,
        save: Option<CloseSave>,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = if self.controller.layout().documents.get(path).is_some() {
            path.to_owned()
        } else {
            self.resolve_file(path).to_string_lossy().into_owned()
        };
        let mut saved = false;
        if self.documents.get(&id).is_some_and(|d| d.dirty) {
            match save {
                None => {
                    return Err(CommandError::Failed(format!(
                        "{id} has unsaved changes; pass \"save\": \"save\" or \"discard\""
                    )));
                }
                Some(CloseSave::Save) => {
                    self.save(Some(&id), cx)?;
                    saved = true;
                }
                Some(CloseSave::Discard) => {}
            }
        }
        self.loading.remove(&id);
        self.load_waiters.remove(&id);
        let had = match self.documents.remove(&id) {
            Some(mut doc) => {
                doc.intellisense.cancel_all();
                self.views.borrow_mut().remove(&id);
                if doc.language_id.is_some() && !doc.read_only {
                    doc.session.did_close(doc.uri.clone());
                }
                self.diagnostics.remove(&doc.uri);
                true
            }
            None => false,
        };
        let closed = self.controller.close_document(&id) || had;
        self.git_document_closed(&id);
        self.code_lens_document_closed(&id);
        self.update_error_list(cx);
        cx.notify();
        Ok(WorkspaceOutput::FileClose(FileCloseOutput {
            path: id,
            closed,
            saved,
        }))
    }
}

pub(super) fn move_caret(view: &Entity<EditorView>, line: u32, column: u32, cx: &mut gpui::App) {
    view.update(cx, |v, cx| {
        let offset = v.update_editor(cx, |e| {
            let offset = offset_of(e.buffer(), line, column);
            e.set_caret(offset);
            offset
        });
        let row = v.editor().buffer().offset_to_point(offset).row;
        // Show the line with a little context above it.
        v.scroll_to_row(row.saturating_sub(5), cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        for p in ["/src/App/Program.cs", "/a b/C#/ü.cs", "/x/%41.cs"] {
            let uri = path_to_uri(Path::new(p));
            assert!(uri.starts_with("file:///"), "{uri}");
            assert!(!uri.contains(' '));
            assert_eq!(uri_to_path(&uri).unwrap(), PathBuf::from(p));
        }
        assert_eq!(path_to_uri(Path::new("/a b/c.cs")), "file:///a%20b/c.cs");
        assert!(uri_to_path("untitled:1").is_none());
    }

    /// The round trip must reproduce the native path *string*, not just an equal `Path`,
    /// because documents are keyed by that string (the Windows CI failure after brief 0012).
    #[test]
    fn normalize_path_joins_components_with_the_native_separator() {
        // Build from a real absolute base: a bare "/a" has no drive on Windows and
        // normalizes differently from how it was spelled.
        let base = std::env::temp_dir();
        let mixed = base.join("c/d.cs");
        let native = base.join("c").join("d.cs");
        assert_eq!(
            normalize_path(&mixed).to_string_lossy(),
            native.to_string_lossy()
        );
    }

    #[test]
    fn uri_round_trip_keeps_the_native_path_string() {
        let native = std::env::temp_dir().join("eludite uri").join("Program.cs");
        let back = uri_to_path(&path_to_uri(&native)).unwrap();
        assert_eq!(back.to_string_lossy(), native.to_string_lossy());
    }

    #[test]
    fn diagnostics_anchor_in_the_sent_text_and_follow_edits() {
        let mut buffer = Buffer::new("class A\n{\n    int x = \"s\";\n}\n");
        let sent = buffer.snapshot().clone();
        let diag = |line, start, end, severity| lsp::Diagnostic {
            range: lsp::Range {
                start: lsp::Position {
                    line,
                    character: start,
                },
                end: lsp::Position {
                    line,
                    character: end,
                },
            },
            severity: Some(severity),
            code: None,
            source: None,
            message: "m".into(),
            extra: Default::default(),
        };
        let d = decorations(
            &sent,
            &[diag(2, 12, 15, 1), diag(0, 7, 7, 2), diag(1, 0, 1, 4)],
        );
        assert_eq!(d.len(), 2, "hints are not drawn");
        let range = |d: &Decoration, b: &Buffer| {
            b.offset_for_anchor(&d.range.start)..b.offset_for_anchor(&d.range.end)
        };
        assert_eq!(buffer.text_for_range(range(&d[0], &buffer)), "\"s\"");
        assert!(matches!(
            d[0].style,
            DecorationStyle::Underline { wavy: true, .. }
        ));
        // An empty range at the end of a line covers the character before it.
        assert_eq!(buffer.text_for_range(range(&d[1], &buffer)), "A");
        // Typing before the error moves the squiggle with the text.
        buffer.edit([(0..0, "// x\n")]);
        assert_eq!(buffer.text_for_range(range(&d[0], &buffer)), "\"s\"");
    }

    #[test]
    fn caret_offsets_are_one_based_and_clipped() {
        let b = Buffer::new("ab\nçd\n");
        assert_eq!(offset_of(&b, 1, 1), 0);
        assert_eq!(offset_of(&b, 2, 2), 3 + 'ç'.len_utf8());
        assert_eq!(
            offset_of(&b, 2, 99),
            b.point_to_offset(text::Point::new(1, 3))
        );
        assert_eq!(
            offset_of(&b, 99, 1),
            b.point_to_offset(text::Point::new(2, 0))
        );
    }
}
