//! The document lifecycle (brief 0070): `eludite.file.*`, `eludite.window.*`, `eludite.bookmarks.*` and
//! `eludite.editor.go_to_last_edit` on the UI thread.
//!
//! - **Unsaved changes.** Closing the window, File > Exit, Close All and Close Workspace ask Visual Studio's "Save
//!   changes to the following items?" (Save, Don't Save, Cancel) over every tab marked dirty: text documents, `.resx`
//!   grids and project property pages alike. An agent's call never sees the question: it passes `save`, or fails
//!   with the list.
//! - **Save All** saves every dirty tab through `eludite.editor.save`'s path (format on save included); **save before
//!   build** (`build.saveBeforeBuild`) writes every dirty text document synchronously before a build, F5, Ctrl+F5 or
//!   a test run reads the files.
//! - **Open File, Save As, New File** use the platform's dialogs from the UI; agents pass the paths. An untitled
//!   document (`untitled:<name>`) is saved through Save As, which retargets its tab
//!   ([`Shell::rename_open_document`]).
//! - **The tab well's commands**: Pin Tab, Move Tab, the next document, the IDE Navigator (Ctrl+Tab, drawn by the
//!   docking crate), Reopen Closed Tab, Copy Full Path, Open Containing Folder.
//!
//! Commands from other threads arrive as [`DocumentsJob`]s; the UI's own invocations are applied here first and their
//! outcome staged for the bus handler, as `resx` does.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::ThreadId;
use std::time::Duration;

use eludite_commands::documents::{
    self as cmd, DocumentsRequest, DocumentsTarget, LineEndingChoice,
};
use eludite_commands::workspace::{self, CloseSave, WorkspaceRequest};
use eludite_commands::{CommandError, CommandRegistry, view};
use eludite_editor::{Buffer, LineEnding, TextEncoding};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{
    AppContext as _, ClipboardItem, Context, Focusable as _, PathPromptOptions, PromptLevel, Window,
};
use serde_json::{Value, json};

use super::Shell;
use super::documents::UNTITLED_PREFIX;

type Outcome = Result<Value, CommandError>;
/// Where a deferred answer goes (an agent's reply, or nothing for the UI).
pub type Reply = Box<dyn FnOnce(Outcome)>;

/// How long an agent's call waits for the UI.
const AGENT_TIMEOUT: Duration = Duration::from_secs(120);

/// Visual Studio's question.
pub const SAVE_CHANGES: &str = "Save changes to the following items?";
pub const SAVE_ANSWERS: [&str; 3] = ["Save", "Don't Save", "Cancel"];

/// File > New File's languages: (button, language id, extension).
pub const NEW_FILE_LANGUAGES: [(&str, &str, &str); 8] = [
    ("C#", "csharp", "cs"),
    ("Visual Basic", "vb", "vb"),
    ("F#", "fsharp", "fs"),
    ("JSON", "json", "json"),
    ("XML", "xml", "xml"),
    ("TypeScript", "typescript", "ts"),
    ("Rust", "rust", "rs"),
    ("Text", "text", "txt"),
];

/// Advanced Save Options' encodings: (label, encoding, byte-order mark); the last is Cancel.
pub const SAVE_ENCODINGS: [(&str, &str, bool); 6] = [
    ("Unicode (UTF-8 with signature)", "utf-8", true),
    ("Unicode (UTF-8 without signature)", "utf-8", false),
    ("Unicode (UTF-16 LE)", "utf-16le", true),
    ("Unicode (UTF-16 BE)", "utf-16be", true),
    ("Western European (Windows-1252)", "windows-1252", false),
    ("Cancel", "", false),
];

/// How many closed tabs Reopen Closed Tab remembers.
pub const CLOSED_TABS: usize = 20;

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The answer the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// A request from another thread, for the UI thread.
pub struct DocumentsJob {
    pub request: DocumentsRequest,
    pub reply: mpsc::SyncSender<Outcome>,
}

/// The shell's [`DocumentsTarget`].
pub struct DocumentsBus {
    pub ui_thread: ThreadId,
    pub jobs: UnboundedSender<DocumentsJob>,
}

impl DocumentsTarget for DocumentsBus {
    fn apply(&self, request: DocumentsRequest) -> Outcome {
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(DocumentsJob { request, reply })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(AGENT_TIMEOUT)
            .map_err(|_| CommandError::Failed("the editor did not answer in time".into()))?
    }
}

/// Register the commands; returns what the shell drains on the UI thread.
pub fn register(commands: &CommandRegistry) -> UnboundedReceiver<DocumentsJob> {
    let (tx, rx) = unbounded();
    cmd::register(
        commands,
        std::sync::Arc::new(DocumentsBus {
            ui_thread: std::thread::current().id(),
            jobs: tx,
        }),
    );
    rx
}

/// A closed tab, for Reopen Closed Tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedTab {
    pub path: String,
    pub line: u32,
    pub column: u32,
}

/// What waits for the answer to "Save changes to the following items?".
#[derive(Debug, Clone, PartialEq)]
pub enum AfterAnswer {
    Exit,
    CloseAll(Value),
    CloseWorkspace,
}

/// The document half of the shell (brief 0070).
pub struct DocsUi {
    /// Reopen Closed Tab's stack, the last closed last.
    pub closed: Vec<ClosedTab>,
    /// The last untitled document's number.
    pub untitled: u32,
    /// The person answered the unsaved-changes question for quitting: the window may close.
    pub quitting: bool,
    /// The question is on screen.
    pub asking: bool,
    /// How many times the shell quit (tests read it; the real quit happens only outside tests).
    pub quits: u32,
    /// Where the last edit was: Go To Last Edit Location.
    pub last_edit: Option<(String, eludite_editor::text::Anchor)>,
    /// The last text Copy Full Path put on the clipboard (tests read it).
    pub copied: Option<String>,
    /// The workspace state folders' root (`<config>/eludite/workspaces`, brief 0047); `None`: nothing persists.
    pub state_root: Option<PathBuf>,
}

impl DocsUi {
    pub fn new(state_root: Option<PathBuf>) -> Self {
        Self {
            closed: Vec::new(),
            untitled: 0,
            quitting: false,
            asking: false,
            quits: 0,
            last_edit: None,
            copied: None,
            state_root,
        }
    }
}

fn failed(message: impl Into<String>) -> CommandError {
    CommandError::Failed(message.into())
}

fn line_ending_name(b: &Buffer) -> &'static str {
    if b.has_mixed_line_endings() {
        return "mixed";
    }
    match b.line_ending() {
        LineEnding::CrLf => "crlf",
        LineEnding::Lf => "lf",
        LineEnding::Cr => "cr",
    }
}

/// The extension and language id File > New File uses for `language` (a language id or an extension).
pub fn new_file_extension(language: &str) -> (String, String) {
    let l = language.trim().trim_start_matches('.').to_ascii_lowercase();
    match NEW_FILE_LANGUAGES
        .iter()
        .find(|(_, id, ext)| *id == l || *ext == l)
    {
        Some((_, id, ext)) => ((*ext).to_owned(), (*id).to_owned()),
        None => {
            let ext = match l.as_str() {
                "javascript" => "js",
                "html" => "html",
                "css" => "css",
                "razor" => "razor",
                "python" => "py",
                "markdown" => "md",
                "sql" => "sql",
                "yaml" => "yaml",
                other => other,
            };
            (ext.to_owned(), l.clone())
        }
    }
}

impl Shell {
    /// Wire the jobs from other threads, the disk watcher and the workspace's restored documents (call once from
    /// `new`).
    pub(super) fn documents_install(
        &mut self,
        mut jobs: UnboundedReceiver<DocumentsJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                let DocumentsJob { request, reply } = job;
                let r = reply.clone();
                let sent = this.update_in(cx, |shell, window, cx| {
                    let r2 = r.clone();
                    let deferred: Reply = Box::new(move |o| {
                        let _ = r2.send(o);
                    });
                    if let Some(out) = shell.apply_documents(request, Some(deferred), window, cx) {
                        let _ = r.send(out);
                    }
                });
                if sent.is_err() {
                    let _ = reply.send(Err(failed("the window is closed")));
                }
            }
        });
        self._tasks.push(task);
        self.reload_install(window, cx);
        self.session_install(window, cx);
    }

    /// Apply a command. `reply` is for answers that come later (a reload reads the file first); when this returns
    /// `None`, `reply` was kept and will be called.
    pub(super) fn apply_documents(
        &mut self,
        request: DocumentsRequest,
        reply: Option<Reply>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        use DocumentsRequest::*;
        let out = match request {
            SaveAll => self.save_all(window, cx),
            SaveAs {
                path,
                new_path,
                overwrite,
            } => self.save_as(path.as_deref(), new_path.as_deref(), overwrite, window, cx),
            New { language } => match language {
                Some(l) => self.new_file(&l, window, cx),
                None => Err(failed(
                    "pass `language` (csharp, vb, fsharp, json, xml, typescript, rust, text, or an extension)",
                )),
            },
            OpenFileDialog => Ok(json!({ "shown": false })),
            Documents { dirty_only } => Ok(self.documents_output(dirty_only, cx)),
            CloseAll {
                except,
                keep_pinned,
                save,
            } => self.close_all(except.as_deref(), keep_pinned, save, window, cx),
            Reload { path, discard } => {
                return match reply {
                    Some(reply) => {
                        self.reload_command(path.as_deref(), discard, reply, window, cx);
                        None
                    }
                    None => {
                        // The UI's own call: answer at once, the reload follows.
                        let path_out = self
                            .document_id(path.as_deref())
                            .unwrap_or_else(|_| path.clone().unwrap_or_default());
                        self.reload_command(path.as_deref(), discard, Box::new(|_| {}), window, cx);
                        Some(Ok(json!({
                            "path": path_out, "reloaded": true, "dirty": false, "deleted": false
                        })))
                    }
                };
            }
            SetEncoding {
                path,
                encoding,
                bom,
                line_ending,
                reopen,
            } => self.set_encoding(
                path.as_deref(),
                encoding.as_deref(),
                bom,
                line_ending,
                reopen,
                window,
                cx,
            ),
            Exit { save } => self.exit(save, window, cx),
            PinTab { path, pinned } => self.pin_tab(path.as_deref(), pinned),
            MoveTab { path, index } => self.move_tab(&path, index),
            NextDocument { backward } => self.next_document(backward, window, cx),
            Navigator { backward, activate } => self.navigator(backward, activate, window, cx),
            ReopenClosed => self.reopen_closed(window, cx),
            CopyFullPath { path } => self.copy_full_path(path.as_deref(), cx),
            OpenContainingFolder { path } => self.open_containing_folder_of(path.as_deref()),
            BookmarkToggle { path, line } => self.bookmark_toggle(path.as_deref(), line, cx),
            BookmarkNext => self.bookmark_step(true, window, cx),
            BookmarkPrevious => self.bookmark_step(false, window, cx),
            BookmarkClear { path } => self.bookmark_clear(path.as_deref(), cx),
            BookmarkList => Ok(self.bookmark_list(cx)),
            BookmarkUpdate { id, name, enabled } => self.bookmark_update(id, name, enabled, cx),
            BookmarkGoTo { id } => self.bookmark_go_to(id, window, cx),
            GoToLastEdit => self.go_to_last_edit(window, cx),
        };
        Some(out)
    }

    /// The UI's side: Visual Studio's dialogs and questions first, then the bus. Returns true when it handled
    /// `command`.
    pub(super) fn run_documents(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let no = |k: &str| args.get(k).is_none_or(Value::is_null);
        match command {
            cmd::OPEN_FILE_DIALOG => {
                self.prompt_open_files(window, cx);
                true
            }
            workspace::FILE_OPEN if no("path") => {
                self.prompt_open_files(window, cx);
                true
            }
            cmd::NEW if no("language") => {
                self.prompt_new_file(window, cx);
                true
            }
            cmd::SAVE_AS if no("new_path") => {
                let path = args.get("path").and_then(Value::as_str).map(str::to_owned);
                self.prompt_save_as(path, window, cx);
                true
            }
            // Ctrl+S on an untitled document is Save As; on a file changed on disk it asks before overwriting.
            workspace::EDITOR_SAVE => {
                let path = args.get("path").and_then(Value::as_str).map(str::to_owned);
                let Ok(id) = self.document_id(path.as_deref()) else {
                    return false;
                };
                if self.documents[&id].untitled {
                    self.prompt_save_as(Some(id), window, cx);
                    return true;
                }
                if self.check_disk_conflict(&id).is_err() {
                    self.prompt_overwrite(id, window, cx);
                    return true;
                }
                false
            }
            cmd::EXIT if no("save") => {
                if self.dirty_tabs().is_empty() {
                    args["save"] = json!("discard");
                    return false;
                }
                self.ask_save_changes(AfterAnswer::Exit, window, cx);
                true
            }
            cmd::CLOSE_ALL if no("save") => {
                let except = args
                    .get("except")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let keep_pinned = args.get("keep_pinned") == Some(&Value::Bool(true));
                let dirty = self.closing_dirty(except.as_deref(), keep_pinned);
                if dirty.is_empty() {
                    return false;
                }
                self.ask_save_changes(AfterAnswer::CloseAll(args.clone()), window, cx);
                true
            }
            workspace::WORKSPACE_CLOSE if !self.dirty_tabs().is_empty() => {
                self.ask_save_changes(AfterAnswer::CloseWorkspace, window, cx);
                true
            }
            // Ctrl+F4 and File > Close: the active tab.
            workspace::FILE_CLOSE if no("path") => {
                let Some(active) = self.controller.active_document() else {
                    return true;
                };
                args["path"] = json!(active);
                self.run(workspace::FILE_CLOSE, args.clone(), window, cx);
                true
            }
            // Ctrl+Tab: the IDE Navigator while Ctrl is held; again moves its selection.
            cmd::NAVIGATOR if no("activate") => {
                let backward = args.get("backward") == Some(&Value::Bool(true));
                let dock = self.dock.clone();
                if dock.update(cx, |d, cx| d.navigator_step(backward, cx)) {
                    return true;
                }
                let items: Vec<(String, String, bool)> = self
                    .navigator_items()
                    .into_iter()
                    .map(|(id, title, kind)| (id, title, kind == "document"))
                    .collect();
                let selected = if items.len() < 2 {
                    0
                } else if backward {
                    items.len() - 1
                } else {
                    1
                };
                dock.update(cx, |d, cx| d.show_navigator(items, selected, cx));
                true
            }
            // File > Advanced Save Options...: the encoding to save in.
            cmd::SET_ENCODING
                if ["encoding", "bom", "line_ending", "reopen"]
                    .iter()
                    .all(|k| no(k)) =>
            {
                let path = args.get("path").and_then(Value::as_str).map(str::to_owned);
                self.prompt_save_options(path, window, cx);
                true
            }
            _ => false,
        }
    }

    /// The tabs marked dirty, in tab order: (id, title).
    pub(super) fn dirty_tabs(&self) -> Vec<(String, String)> {
        let snap = self.controller.snapshot();
        snap.layout
            .documents
            .tabs
            .iter()
            .filter(|t| snap.dirty.contains(&t.id))
            .map(|t| (t.id.clone(), t.title.clone()))
            .collect()
    }

    /// The dirty tabs Close All with these options would close.
    fn closing_dirty(&self, except: Option<&str>, keep_pinned: bool) -> Vec<String> {
        let snap = self.controller.snapshot();
        snap.layout
            .documents
            .tabs
            .iter()
            .filter(|t| Some(t.id.as_str()) != except && !(keep_pinned && t.pinned))
            .filter(|t| snap.dirty.contains(&t.id))
            .map(|t| t.id.clone())
            .collect()
    }

    /// "Save changes to the following items?" and what follows the answer.
    pub(super) fn ask_save_changes(
        &mut self,
        then: AfterAnswer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.docs.asking {
            return;
        }
        let items: Vec<String> = self
            .dirty_tabs()
            .into_iter()
            .map(|(id, title)| {
                if Path::new(&id).is_absolute() {
                    format!("{title}  ({id})")
                } else {
                    title
                }
            })
            .collect();
        self.docs.asking = true;
        let detail = items.join("\n");
        let answer = window.prompt(
            PromptLevel::Warning,
            SAVE_CHANGES,
            Some(&detail),
            &SAVE_ANSWERS,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.ok();
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.docs.asking = false;
                let save = match answer {
                    Some(0) => "save",
                    Some(1) => "discard",
                    _ => return,
                };
                match then {
                    AfterAnswer::Exit => {
                        shell.run(cmd::EXIT, json!({ "save": save }), window, cx);
                    }
                    AfterAnswer::CloseAll(mut args) => {
                        args["save"] = json!(save);
                        shell.run(cmd::CLOSE_ALL, args, window, cx);
                    }
                    AfterAnswer::CloseWorkspace => {
                        // Visual Studio closes the documents with the solution.
                        shell.session_save_now(cx);
                        shell.run(cmd::CLOSE_ALL, json!({ "save": save }), window, cx);
                        if shell.dirty_tabs().is_empty() {
                            shell.run(workspace::WORKSPACE_CLOSE, json!({}), window, cx);
                        }
                    }
                }
            });
        })
        .detach();
    }

    /// The main window's close button (and the platform's close): ask about unsaved changes first. Returns whether
    /// the window may close now.
    pub fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.docs.quitting || self.dirty_tabs().is_empty() {
            self.quit(cx);
            return true;
        }
        self.ask_save_changes(AfterAnswer::Exit, window, cx);
        false
    }

    /// Quit Eludite: the session state is written first.
    fn quit(&mut self, cx: &mut Context<Self>) {
        self.docs.quitting = true;
        self.docs.quits += 1;
        self.session_save_now(cx);
        if cfg!(test) {
            return;
        }
        cx.quit();
    }

    /// Save a dirty tab now, whatever kind it is: a text document is written synchronously (no format on save),
    /// the others through `eludite.editor.save`'s path.
    fn save_tab_now(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        if let Some(doc) = self.documents.get(id) {
            if doc.untitled {
                return Err(failed(format!(
                    "{id} has no file yet; save it with Save As first"
                )));
            }
            return self.save(Some(id), cx).map(|_| ());
        }
        self.apply(
            WorkspaceRequest::Save {
                path: Some(id.to_owned()),
            },
            window,
            cx,
        )
        .map(|_| ())
    }

    /// `eludite.file.save_all`.
    fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let mut saved = Vec::new();
        let mut failed_rows = Vec::new();
        let mut skipped = Vec::new();
        for (id, _) in self.dirty_tabs() {
            if self.documents.get(&id).is_some_and(|d| d.untitled) {
                skipped.push(id);
                continue;
            }
            let result = self.apply(
                WorkspaceRequest::Save {
                    path: Some(id.clone()),
                },
                window,
                cx,
            );
            match result {
                Ok(_) => saved.push(id),
                Err(e) => failed_rows.push(json!({ "path": id, "message": e.to_string() })),
            }
        }
        Ok(json!({ "saved": saved, "failed": failed_rows, "skipped": skipped }))
    }

    /// Brief 0070's save before build (`build.saveBeforeBuild`): every dirty text document with a file is written
    /// now, before a build, a run or a test run reads the files. Failures go to the status bar; the build goes on.
    pub(super) fn save_before_build(&mut self, cx: &mut Context<Self>) {
        if !self.settings.lock().bool("build.saveBeforeBuild") {
            return;
        }
        let ids: Vec<String> = self
            .dirty_tabs()
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| {
                self.documents
                    .get(id)
                    .is_some_and(|d| !d.untitled && !d.read_only)
            })
            .collect();
        for id in ids {
            if let Err(e) = self.save(Some(&id), cx) {
                self.status.set(eludite_ui::slots::STATE, e.to_string());
            }
        }
    }

    /// `eludite.file.close_all`.
    fn close_all(
        &mut self,
        except: Option<&str>,
        keep_pinned: bool,
        save: Option<CloseSave>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let dirty = self.closing_dirty(except, keep_pinned);
        if !dirty.is_empty() && save.is_none() {
            return Err(failed(format!(
                "unsaved changes in {}; pass \"save\": \"save\" or \"discard\"",
                dirty.join(", ")
            )));
        }
        let except = except.map(|e| {
            if self.controller.layout().documents.get(e).is_some() {
                e.to_owned()
            } else {
                self.resolve_file(e).to_string_lossy().into_owned()
            }
        });
        let targets: Vec<(String, bool)> = {
            let snap = self.controller.snapshot();
            snap.layout
                .documents
                .tabs
                .iter()
                .filter(|t| Some(&t.id) != except.as_ref() && !(keep_pinned && t.pinned))
                .map(|t| (t.id.clone(), snap.dirty.contains(&t.id)))
                .collect()
        };
        let mut closed = Vec::new();
        let mut saved = Vec::new();
        for (id, dirty) in targets {
            if dirty && save == Some(CloseSave::Save) {
                // An untitled document cannot be saved here: it stays open.
                if let Err(e) = self.save_tab_now(&id, window, cx) {
                    self.status.set(eludite_ui::slots::STATE, e.to_string());
                    continue;
                }
                saved.push(id.clone());
            }
            let save_arg = dirty.then_some(CloseSave::Discard);
            let out = self.apply(
                WorkspaceRequest::FileClose {
                    path: id.clone(),
                    save: save_arg,
                },
                window,
                cx,
            );
            if out.is_ok() {
                closed.push(id);
            }
        }
        cx.notify();
        Ok(json!({ "closed": closed, "saved": saved }))
    }

    /// `eludite.file.exit`.
    fn exit(
        &mut self,
        save: Option<CloseSave>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let dirty: Vec<String> = self.dirty_tabs().into_iter().map(|(id, _)| id).collect();
        let mut saved = Vec::new();
        if !dirty.is_empty() {
            match save {
                None => {
                    return Ok(json!({ "exiting": false, "saved": [], "dirty": dirty }));
                }
                Some(CloseSave::Save) => {
                    for id in &dirty {
                        self.save_tab_now(id, window, cx)
                            .map_err(|e| failed(format!("not exiting: {e}")))?;
                        saved.push(id.clone());
                    }
                }
                Some(CloseSave::Discard) => {}
            }
        }
        // After this command's answer is on its way.
        cx.defer_in(window, |shell, _, cx| shell.quit(cx));
        Ok(json!({ "exiting": true, "saved": saved }))
    }

    /// `eludite.file.documents`.
    fn documents_output(&self, dirty_only: bool, cx: &gpui::App) -> Value {
        let snap = self.controller.snapshot();
        let active = snap.layout.documents.active.clone();
        let rows: Vec<Value> = snap
            .layout
            .documents
            .tabs
            .iter()
            .filter(|t| !dirty_only || snap.dirty.contains(&t.id))
            .map(|t| {
                let kind = if self.documents.contains_key(&t.id) {
                    "text"
                } else if self.resx_tab(Some(&t.id)).is_some() {
                    "resx"
                } else if self.pages_tab(Some(&t.id)).is_some() {
                    "properties"
                } else {
                    "other"
                };
                let mut row = json!({
                    "path": t.id,
                    "title": t.title,
                    "kind": kind,
                    "dirty": snap.dirty.contains(&t.id),
                    "pinned": t.pinned,
                    "preview": t.preview,
                    "deleted": snap.deleted.contains(&t.id),
                    "active": active.as_deref() == Some(t.id.as_str()),
                });
                if let Some(doc) = self.documents.get(&t.id) {
                    let e = doc.view.read(cx).editor();
                    let b = e.buffer();
                    row["untitled"] = json!(doc.untitled);
                    row["encoding"] = json!(b.encoding().label());
                    row["bom"] = json!(b.has_bom());
                    row["line_ending"] = json!(line_ending_name(b));
                    let (line, column) =
                        super::intellisense::line_column(&doc.view, e.primary_selection().head, cx);
                    row["line"] = json!(line);
                    row["column"] = json!(column);
                }
                row
            })
            .collect();
        json!({ "documents": rows })
    }

    /// `eludite.file.save_as` with a target.
    fn save_as(
        &mut self,
        path: Option<&str>,
        new_path: Option<&str>,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let id = self.document_id(path)?;
        let Some(new_path) = new_path else {
            return Err(failed("pass `new_path`: where to write the document"));
        };
        let target = self.resolve_file(new_path);
        let same = self.documents[&id].path == target && !self.documents[&id].untitled;
        if !same && target.exists() && !overwrite {
            return Err(failed(format!(
                "{} exists; pass \"overwrite\": true to replace it",
                target.display()
            )));
        }
        if !same
            && self
                .documents
                .contains_key(target.to_string_lossy().as_ref())
        {
            return Err(failed(format!(
                "{} is open in another tab",
                target.display()
            )));
        }
        let bytes = self.write_document_to(&id, &target, cx)?;
        if !same {
            self.rename_open_document(Path::new(&id), &target, window, cx);
            let new_id = target.to_string_lossy().into_owned();
            self.mark_saved(&new_id, cx);
        }
        Ok(json!({
            "path": id,
            "new_path": target.to_string_lossy(),
            "bytes": bytes,
        }))
    }

    /// `eludite.file.new` with a language.
    fn new_file(&mut self, language: &str, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let (ext, language) = new_file_extension(language);
        self.docs.untitled += 1;
        let name = format!("Untitled-{}.{ext}", self.docs.untitled);
        let id = format!("{UNTITLED_PREFIX}{name}");
        let mut buffer = Buffer::new("");
        buffer.set_line_ending(LineEnding::platform_default());
        // Visual Studio writes its .NET source files as UTF-8 with a signature.
        buffer.set_bom(matches!(
            ext.as_str(),
            "cs" | "vb" | "fs" | "xml" | "csproj"
        ));
        self.controller.open_document(&id, &name);
        self.loading
            .insert(id.clone(), (gpui::Task::ready(()), None));
        self.finish_open(id.clone(), Ok(buffer), None, window, cx);
        Ok(json!({ "path": id, "title": name, "language": language }))
    }

    /// `eludite.file.set_encoding`.
    #[allow(clippy::too_many_arguments)]
    fn set_encoding(
        &mut self,
        path: Option<&str>,
        encoding: Option<&str>,
        bom: Option<bool>,
        line_ending: Option<LineEndingChoice>,
        reopen: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let id = self.document_id(path)?;
        let encoding = encoding
            .map(|label| {
                TextEncoding::from_label(label)
                    .ok_or_else(|| failed(format!("unknown encoding `{label}`")))
            })
            .transpose()?;
        let doc = &self.documents[&id];
        if doc.read_only {
            return Err(failed(format!("{id} is read-only")));
        }
        if reopen {
            if doc.dirty {
                return Err(failed(format!(
                    "{id} has unsaved changes; save or reload it first"
                )));
            }
            let encoding = encoding.expect("parse requires it");
            let path = doc.path.clone();
            let task_id = id.clone();
            cx.spawn_in(window, async move |this, cx| {
                let read = cx
                    .background_spawn(async move {
                        let stamp = super::reload::DiskStamp::of(&path);
                        std::fs::read(&path)
                            .map(|bytes| (Buffer::from_bytes_as(&bytes, encoding), stamp))
                    })
                    .await;
                let _ = this.update(cx, |shell, cx| match read {
                    Ok((buffer, stamp)) => {
                        shell.apply_reload(&task_id, buffer, stamp, cx);
                    }
                    Err(e) => {
                        shell
                            .status
                            .set(eludite_ui::slots::STATE, format!("{task_id}: {e}"));
                    }
                });
            })
            .detach();
            return Ok(json!({
                "path": id,
                "encoding": encoding.label(),
                "bom": encoding.is_unicode() && doc.view.read(cx).editor().buffer().has_bom(),
                "line_ending": line_ending_name(doc.view.read(cx).editor().buffer()),
                "dirty": false,
            }));
        }
        if bom == Some(true) {
            let target = encoding.unwrap_or(doc.view.read(cx).editor().buffer().encoding());
            if !target.is_unicode() {
                return Err(failed(format!(
                    "{} has no byte-order mark",
                    target.display_name()
                )));
            }
        }
        let view = doc.view.clone();
        let changed = view.update(cx, |v, cx| {
            v.update_editor(cx, |e| {
                let b = e.buffer_mut();
                let before = (
                    b.encoding(),
                    b.has_bom(),
                    b.line_ending(),
                    b.has_mixed_line_endings(),
                );
                if let Some(enc) = encoding {
                    b.set_encoding(enc);
                    // A switch to a Unicode form keeps the signature choice; UTF-16 and UTF-32 always carry one.
                    if matches!(
                        enc,
                        TextEncoding::Utf16Le
                            | TextEncoding::Utf16Be
                            | TextEncoding::Utf32Le
                            | TextEncoding::Utf32Be
                    ) && bom.is_none()
                    {
                        b.set_bom(true);
                    }
                }
                if let Some(bom) = bom {
                    b.set_bom(bom);
                }
                if let Some(le) = line_ending {
                    b.set_line_ending(match le {
                        LineEndingChoice::CrLf => LineEnding::CrLf,
                        LineEndingChoice::Lf => LineEnding::Lf,
                        LineEndingChoice::Cr => LineEnding::Cr,
                    });
                }
                before
                    != (
                        b.encoding(),
                        b.has_bom(),
                        b.line_ending(),
                        b.has_mixed_line_endings(),
                    )
            })
        });
        if changed {
            self.mark_dirty(&id);
        }
        self.update_document_status(cx);
        let b = view.read(cx).editor().buffer();
        Ok(json!({
            "path": id,
            "encoding": b.encoding().label(),
            "bom": b.has_bom(),
            "line_ending": line_ending_name(b),
            "dirty": self.documents.get(&id).is_some_and(|d| d.dirty),
        }))
    }

    /// `eludite.window.pin_tab`.
    fn pin_tab(&mut self, path: Option<&str>, pinned: Option<bool>) -> Outcome {
        let id = self.tab_id(path)?;
        let current = self
            .controller
            .layout()
            .documents
            .get(&id)
            .is_some_and(|t| t.pinned);
        let pinned = pinned.unwrap_or(!current);
        self.controller.set_document_pinned(&id, pinned);
        Ok(json!({ "path": id, "pinned": pinned }))
    }

    /// `eludite.window.move_tab`.
    fn move_tab(&mut self, path: &str, index: usize) -> Outcome {
        let id = self.tab_id(Some(path))?;
        self.controller.move_document(&id, index);
        let layout = self.controller.layout();
        let tabs: Vec<String> = layout.documents.tabs.iter().map(|t| t.id.clone()).collect();
        let index = tabs.iter().position(|t| *t == id).unwrap_or(0);
        Ok(json!({ "path": id, "index": index, "tabs": tabs }))
    }

    /// A document tab's id: `path` as an open tab's id, or resolved; the active one when `None`.
    pub(super) fn tab_id(&self, path: Option<&str>) -> Result<String, CommandError> {
        let layout = self.controller.layout();
        let id = match path {
            Some(p) if layout.documents.get(p).is_some() => p.to_owned(),
            Some(p) => self.resolve_file(p).to_string_lossy().into_owned(),
            None => layout
                .documents
                .active
                .clone()
                .ok_or_else(|| failed("no document is active"))?,
        };
        if layout.documents.get(&id).is_some() {
            Ok(id)
        } else {
            Err(failed(format!("{id} is not an open document tab")))
        }
    }

    /// Activate document tab `id`, focusing its editor, and remember where the caret was.
    pub(super) fn activate_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.controller.active_document().as_deref() != Some(id)
            && let Some(here) = self.caret_entry(cx)
        {
            self.navigation.history.push(here);
        }
        self.controller.activate_document(id);
        if let Some(doc) = self.documents.get(id) {
            doc.view.focus_handle(cx).focus(window, cx);
        }
        self.session_layout_changed(window, cx);
        cx.notify();
    }

    /// `eludite.window.next_document`.
    fn next_document(
        &mut self,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let layout = self.controller.layout();
        let tabs = &layout.documents.tabs;
        if tabs.is_empty() {
            return Ok(json!({}));
        }
        let ix = layout
            .documents
            .active
            .as_deref()
            .and_then(|a| tabs.iter().position(|t| t.id == a))
            .unwrap_or(0);
        let next = if backward {
            (ix + tabs.len() - 1) % tabs.len()
        } else {
            (ix + 1) % tabs.len()
        };
        let id = tabs[next].id.clone();
        self.activate_tab(&id, window, cx);
        Ok(json!({ "path": id }))
    }

    /// `eludite.window.navigator`: the items in MRU order; from the UI, the popup.
    fn navigator(
        &mut self,
        backward: bool,
        activate: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        let items = self.navigator_items();
        if let Some(ix) = activate {
            let (id, _, kind) = items
                .get(ix)
                .cloned()
                .ok_or_else(|| failed(format!("no item {ix}: there are {}", items.len())))?;
            self.navigator_activate(&id, kind == "document", window, cx);
            return Ok(json!({
                "items": items_json(&items), "selected": ix, "activated": id
            }));
        }
        let selected = if items.len() < 2 {
            0
        } else if backward {
            items.len() - 1
        } else {
            1
        };
        Ok(json!({ "items": items_json(&items), "selected": selected }))
    }

    /// The IDE Navigator's items: (id, title, kind), most recent first.
    pub(super) fn navigator_items(&self) -> Vec<(String, String, &'static str)> {
        let snap = self.controller.snapshot();
        let mut items: Vec<(String, String, &'static str)> = snap
            .mru
            .iter()
            .filter_map(|id| {
                if let Some(t) = snap.layout.documents.get(id) {
                    Some((id.clone(), t.title.clone(), "document"))
                } else if snap.registry.contains(id) {
                    Some((
                        id.clone(),
                        snap.registry.title(id).to_owned(),
                        "tool_window",
                    ))
                } else {
                    None
                }
            })
            .collect();
        // Open documents never activated yet come after, in tab order.
        for t in &snap.layout.documents.tabs {
            if !items.iter().any(|(id, _, _)| *id == t.id) {
                items.push((t.id.clone(), t.title.clone(), "document"));
            }
        }
        items
    }

    /// Activate a navigator item.
    pub(super) fn navigator_activate(
        &mut self,
        id: &str,
        document: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if document {
            self.activate_tab(id, window, cx);
        } else {
            let _ = self.commands.invoke(view::SHOW, json!({ "id": id }));
        }
    }

    /// `eludite.window.reopen_closed`.
    fn reopen_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        while let Some(tab) = self.docs.closed.pop() {
            if self.documents.contains_key(&tab.path) || !Path::new(&tab.path).is_file() {
                continue;
            }
            self.open_file(&tab.path, Some((tab.line, tab.column)), window, cx)?;
            return Ok(json!({ "path": tab.path }));
        }
        Ok(json!({}))
    }

    /// `eludite.window.copy_full_path`.
    fn copy_full_path(&mut self, path: Option<&str>, cx: &mut Context<Self>) -> Outcome {
        let id = self.tab_id(path)?;
        let full = self
            .documents
            .get(&id)
            .filter(|d| !d.untitled)
            .map(|d| d.path.to_string_lossy().into_owned())
            .or_else(|| Path::new(&id).is_absolute().then(|| id.clone()))
            .ok_or_else(|| failed(format!("{id} has no file")))?;
        cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
        self.docs.copied = Some(full.clone());
        Ok(json!({ "path": full }))
    }

    /// `eludite.window.open_containing_folder`.
    fn open_containing_folder_of(&mut self, path: Option<&str>) -> Outcome {
        let id = self.tab_id(path)?;
        let file = self
            .documents
            .get(&id)
            .filter(|d| !d.untitled)
            .map(|d| d.path.clone())
            .or_else(|| Path::new(&id).is_absolute().then(|| PathBuf::from(&id)))
            .ok_or_else(|| failed(format!("{id} has no file")))?;
        let folder = file
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| failed(format!("{} has no folder", file.display())))?;
        (self.folder_opener)(&folder).map_err(|e| failed(format!("{}: {e}", folder.display())))?;
        Ok(json!({ "folder": folder.to_string_lossy() }))
    }

    /// `eludite.editor.go_to_last_edit`.
    fn go_to_last_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let Some((id, anchor)) = self.docs.last_edit.clone() else {
            return Ok(json!({}));
        };
        let Some(doc) = self.documents.get(&id) else {
            return Ok(json!({}));
        };
        let offset = doc
            .view
            .read(cx)
            .editor()
            .buffer()
            .offset_for_anchor(&anchor);
        let (line, column) = super::intellisense::line_column(&doc.view, offset, cx);
        if let Some(here) = self.caret_entry(cx) {
            self.navigation.history.push(here);
        }
        self.open_file(&id, Some((line, column)), window, cx)?;
        Ok(json!({ "path": id, "line": line, "column": column }))
    }

    // ---- hooks the documents call ----

    /// A document's editor exists (after it was read).
    pub(super) fn documents_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        self.bookmarks_document_opened(id, cx);
        self.update_document_status(cx);
    }

    /// A document's text changed.
    pub(super) fn documents_edited(&mut self, id: &str, cx: &mut Context<Self>) {
        // Visual Studio keeps the preview tab once it is edited.
        self.controller.promote_document(id);
        if let Some(doc) = self.documents.get(id) {
            let e = doc.view.read(cx).editor();
            let anchor = e.buffer().anchor_before(e.primary_selection().head);
            self.docs.last_edit = Some((id.to_owned(), anchor));
        }
        self.autosave_after_edit(id, cx);
    }

    /// A document is about to close.
    pub(super) fn documents_closing(&mut self, id: &str, cx: &mut Context<Self>) {
        self.bookmarks_document_closing(id, cx);
        if let Some(doc) = self.documents.get(id)
            && !doc.untitled
        {
            let head = doc.view.read(cx).editor().primary_selection().head;
            let (line, column) = super::intellisense::line_column(&doc.view, head, cx);
            self.docs.closed.retain(|c| c.path != id);
            self.docs.closed.push(ClosedTab {
                path: id.to_owned(),
                line,
                column,
            });
            if self.docs.closed.len() > CLOSED_TABS {
                self.docs.closed.remove(0);
            }
        }
        if self.docs.last_edit.as_ref().is_some_and(|(e, _)| e == id) {
            self.docs.last_edit = None;
        }
    }

    /// A document moved from `old` to `new` (Save As, a rename).
    pub(super) fn documents_renamed(&mut self, old: &str, new: &str, cx: &mut Context<Self>) {
        self.bookmarks_document_renamed(old, new, cx);
        if let Some((e, _)) = self.docs.last_edit.as_mut()
            && e == old
        {
            *e = new.to_owned();
        }
        self.update_document_status(cx);
    }

    /// The status bar's document items: Ln, Col, Ch and the selection, the encoding, the line endings.
    pub(super) fn update_document_status(&mut self, cx: &mut Context<Self>) {
        use eludite_ui::slots;
        let active = self.controller.active_document();
        let Some(doc) = active.as_deref().and_then(|id| self.documents.get(id)) else {
            for slot in [slots::LINE_COLUMN, slots::ENCODING, slots::LINE_ENDINGS] {
                self.status.set(slot, "");
            }
            return;
        };
        let e = doc.view.read(cx).editor();
        let b = e.buffer();
        let sel = e.primary_selection();
        let point = b.offset_to_point(sel.head);
        let line_text = b.line(point.row);
        let upto = &line_text[..(point.column as usize).min(line_text.len())];
        let ch = upto.chars().count() + 1;
        // Visual Studio's Col counts a tab as its width (4).
        let col = upto
            .chars()
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum::<usize>()
            + 1;
        let selected = sel.end() - sel.start();
        let mut text = format!("Ln: {}  Col: {col}  Ch: {ch}", point.row + 1);
        if selected > 0 {
            let chars = b.text_for_range(sel.start()..sel.end()).chars().count();
            text.push_str(&format!("  Sel: {chars}"));
        }
        let encoding = {
            let name = b.encoding().display_name();
            if b.has_bom() && b.encoding() == TextEncoding::Utf8 {
                format!("{name} with BOM")
            } else {
                name
            }
        };
        let ending = match line_ending_name(b) {
            "crlf" => "CRLF",
            "lf" => "LF",
            "cr" => "CR",
            _ => "Mixed",
        };
        self.status.set(slots::LINE_COLUMN, text);
        self.status.set(slots::ENCODING, encoding);
        self.status.set(slots::LINE_ENDINGS, ending);
        // A click on the encoding opens Advanced Save Options' choice; on the line endings, it switches them.
        let next_ending = match ending {
            "CRLF" => "lf",
            _ => "crlf",
        };
        let enc_next = if b.encoding() == TextEncoding::Utf8 && !b.has_bom() {
            json!({ "bom": true })
        } else if b.encoding() == TextEncoding::Utf8 {
            json!({ "bom": false })
        } else {
            json!({ "encoding": "utf-8", "bom": true })
        };
        self.status
            .set_action(slots::ENCODING, cmd::SET_ENCODING, enc_next);
        self.status.set_action(
            slots::LINE_ENDINGS,
            cmd::SET_ENCODING,
            json!({ "line_ending": next_ending }),
        );
    }

    // ---- the dialogs ----

    fn prompt_open_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                for p in paths {
                    shell.run(
                        workspace::FILE_OPEN,
                        json!({ "path": p.to_string_lossy() }),
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn prompt_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut buttons: Vec<&str> = NEW_FILE_LANGUAGES.iter().map(|(b, _, _)| *b).collect();
        buttons.push("Cancel");
        let answer = window.prompt(
            PromptLevel::Info,
            "New File",
            Some("Choose the new file's language."),
            &buttons,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(ix) = answer.await else {
                return;
            };
            let Some((_, language, _)) = NEW_FILE_LANGUAGES.get(ix) else {
                return;
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.run(cmd::NEW, json!({ "language": language }), window, cx);
            });
        })
        .detach();
    }

    fn prompt_save_as(
        &mut self,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(id) = self.document_id(path.as_deref()) else {
            self.status.set(
                eludite_ui::slots::STATE,
                "Save As: no text document is active",
            );
            return;
        };
        let doc = &self.documents[&id];
        let (dir, name) = if doc.untitled {
            (
                self.workspace_root()
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_default(),
                id.trim_start_matches(UNTITLED_PREFIX).to_owned(),
            )
        } else {
            (
                doc.path.parent().map(Path::to_path_buf).unwrap_or_default(),
                doc.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        };
        let chosen = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                // The platform's dialog asked about replacing an existing file.
                shell.run(
                    cmd::SAVE_AS,
                    json!({ "path": id, "new_path": target.to_string_lossy(), "overwrite": true }),
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    fn prompt_save_options(
        &mut self,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(id) = self.document_id(path.as_deref()) else {
            self.status.set(
                eludite_ui::slots::STATE,
                "Advanced Save Options: no text document is active",
            );
            return;
        };
        let current = {
            let b = self.documents[&id].view.read(cx).editor().buffer();
            let mut name = b.encoding().display_name();
            if b.has_bom() && b.encoding() == TextEncoding::Utf8 {
                name.push_str(" with signature");
            }
            name
        };
        let detail = format!("{id}\n\nNow: {current}. Save the file in:");
        let answer = window.prompt(
            PromptLevel::Info,
            "Advanced Save Options",
            Some(&detail),
            &SAVE_ENCODINGS.map(|(label, _, _)| label),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(ix) = answer.await else {
                return;
            };
            let Some((_, encoding, bom)) = SAVE_ENCODINGS.get(ix).filter(|(_, e, _)| !e.is_empty())
            else {
                return;
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.run(
                    cmd::SET_ENCODING,
                    json!({ "path": id, "encoding": encoding, "bom": bom }),
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    fn prompt_overwrite(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let detail = format!(
            "{id}\n\nThe file has been changed outside the editor since it was opened. Overwrite it with the \
             editor's version?"
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "Save",
            Some(&detail),
            &["Overwrite", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.run(
                    cmd::SAVE_AS,
                    json!({ "path": id, "new_path": id, "overwrite": true }),
                    window,
                    cx,
                );
            });
        })
        .detach();
    }
}

fn items_json(items: &[(String, String, &'static str)]) -> Vec<Value> {
    items
        .iter()
        .map(|(id, title, kind)| json!({ "id": id, "title": title, "kind": kind }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_file_languages_map_to_extensions() {
        assert_eq!(new_file_extension("csharp"), ("cs".into(), "csharp".into()));
        assert_eq!(new_file_extension(".cs"), ("cs".into(), "csharp".into()));
        assert_eq!(new_file_extension("VB"), ("vb".into(), "vb".into()));
        assert_eq!(new_file_extension("javascript").0, "js");
        assert_eq!(new_file_extension("sql").0, "sql");
    }
}
