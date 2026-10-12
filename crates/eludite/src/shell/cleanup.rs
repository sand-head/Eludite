//! IntelliSense editing depth (brief 0069), the shell's side of everything that is not completion itself:
//!
//! - **Commands** (`eludite_commands::intellisense`): Insert Snippet, Surround With, the snippet list, Format
//!   Selection, Remove and Sort Usings and Code Cleanup, run from the keys, the menus and agents alike. An agent's
//!   call that needs the language server is answered when the server's edit is applied (or dropped).
//! - **Typing**: format on type (`textDocument/onTypeFormatting` after a character the server registers, Roslyn's `}`,
//!   `;` and Enter) and `///` doc comments (Roslyn's `textDocument/_vs_onAutoInsert` on the third `/`, the third `'`
//!   in Visual Basic, and Enter on a doc comment line), and format on paste (`textDocument/rangeFormatting`).
//! - **Override completion**: Roslyn's `roslyn.client.completionComplexEdit` command, run after the commit.
//! - **Roslyn options**: the settings `RoslynOptions` (the host) maps are sent with `workspace/didChangeConfiguration`
//!   when they change and when the solution's documents open; the host answers Roslyn's `workspace/configuration`.
//! - **Full-solution analysis**: with `dotnet.backgroundAnalysis.scope` `fullSolution`, one `workspace/diagnostic`
//!   is kept in flight (Roslyn answers when something changed) and closed files' diagnostics go to the Error List.
//!
//! Every server answer is applied only when the document is still at the version it was computed for and the
//! solution generation has not moved (CLAUDE.md invariant 12); otherwise it is dropped.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::ThreadId;
use std::time::Duration;

use eludite_commands::intellisense::{self as cmd, IntellisenseRequest, IntellisenseTarget};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_editor::EditorView;
use eludite_editor::text::{self, Point};
use eludite_lsp::lsp;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{AppContext as _, Context, Entity, Focusable as _, Task, Window, point, px};
use serde_json::{Value, json};

use super::Shell;
use super::code_actions::FIX_ALL_COMMAND;
use super::documents::trace;
use super::intellisense::{lsp_position, offset_in};
use super::servers::ServerKey;
use super::session::RequestError;
use super::snippets::{Library, PickerEvent, SnippetKind, SnippetPicker, language_of_path};

type Outcome = Result<Value, CommandError>;

/// How long an agent's call waits for the editor and the language server.
const AGENT_TIMEOUT: Duration = Duration::from_secs(60);

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The answer the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// A request from another thread (an agent), for the UI thread.
pub struct DepthJob {
    pub request: IntellisenseRequest,
    pub reply: mpsc::SyncSender<Outcome>,
}

/// The shell's [`IntellisenseTarget`].
pub struct DepthBus {
    pub ui_thread: ThreadId,
    pub jobs: UnboundedSender<DepthJob>,
}

impl IntellisenseTarget for DepthBus {
    fn apply(&self, request: IntellisenseRequest) -> Outcome {
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
            .unbounded_send(DepthJob { request, reply })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(AGENT_TIMEOUT)
            .map_err(|_| CommandError::Failed("the editor did not answer in time".into()))?
    }
}

/// Register the commands; returns what the shell drains on the UI thread.
pub fn register(commands: &CommandRegistry) -> UnboundedReceiver<DepthJob> {
    let (tx, rx) = unbounded();
    cmd::register(
        commands,
        std::sync::Arc::new(DepthBus {
            ui_thread: std::thread::current().id(),
            jobs: tx,
        }),
    );
    rx
}

/// The settings this module applies.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DepthSettings {
    pub format_on_type: bool,
    pub format_on_paste: bool,
    pub auto_insert: bool,
    pub snippet_paths: String,
    pub profiles: [String; 2],
    pub cleanup_on_save: bool,
    pub full_solution: bool,
    /// The settings sent to the host for Roslyn (`RoslynOptions`' keys present in the schema).
    pub roslyn: serde_json::Map<String, Value>,
}

/// The Eludite settings the host maps to Roslyn options (`dotnet/src/Eludite.Host/Lsp/RoslynOptions.cs`); the ones
/// the schema does not define (yet) are not sent.
pub const ROSLYN_SETTINGS: &[&str] = &[
    "dotnet.completion.showItemsFromUnimportedNamespaces",
    "dotnet.completion.showNameSuggestions",
    "dotnet.backgroundAnalysis.scope",
    "dotnet.quickInfo.showRemarks",
    "dotnet.autoInsert",
    "editor.codeLens.references",
    "editor.codeLens.tests",
    // Brief 0066's inlay hint settings.
    "editor.inlayHints.parameters",
    "editor.inlayHints.literalParameters",
    "editor.inlayHints.indexerParameters",
    "editor.inlayHints.objectCreationParameters",
    "editor.inlayHints.otherParameters",
    "editor.inlayHints.suppressForParametersThatDifferOnlyBySuffix",
    "editor.inlayHints.suppressForParametersThatMatchMethodIntent",
    "editor.inlayHints.suppressForParametersThatMatchArgumentName",
    "editor.inlayHints.types",
    "editor.inlayHints.implicitVariableTypes",
    "editor.inlayHints.lambdaParameterTypes",
    "editor.inlayHints.implicitObjectCreation",
    "editor.inlayHints.collectionExpressions",
];

/// One step of a running Code Cleanup.
struct Cleanup {
    doc: String,
    steps: VecDeque<String>,
    results: Vec<Value>,
    reply: Option<mpsc::SyncSender<Outcome>>,
    /// Run on save: write the file when done.
    then_save: bool,
}

/// The module's state in the shell.
#[derive(Default)]
pub struct Depth {
    pub settings: DepthSettings,
    pub library: Library,
    library_task: Option<Task<()>>,
    pub picker: Option<(Entity<SnippetPicker>, String)>,
    /// Requests in flight by document and purpose (`format`, `onType`, `autoInsert`, `paste`, `usings`), newest only.
    tasks: BTreeMap<(String, &'static str), Task<()>>,
    /// The last answer of each command, by command and document (what an agent sees).
    pub last: BTreeMap<(&'static str, String), Value>,
    cleanup: Option<Cleanup>,
    /// The Roslyn settings sent, and the generation they were sent in.
    sent: Option<(u64, serde_json::Map<String, Value>)>,
    /// Full-solution analysis: the request in flight, its generation, the result ids and the closed files listed.
    workspace: Option<(u64, Task<()>)>,
    result_ids: BTreeMap<String, String>,
    pub workspace_uris: std::collections::BTreeSet<String>,
    /// The libraries the open documents' Tab-Tab expanders read (replaced when the user's snippets are read again).
    libraries: Vec<std::rc::Weak<RefCell<Library>>>,
}

/// The text edits a workspace edit makes to `uri` (`changes` or `documentChanges`).
pub fn edits_for(edit: &Value, uri: &str) -> Vec<lsp::TextEdit> {
    let parse =
        |v: &Value| -> Vec<lsp::TextEdit> { serde_json::from_value(v.clone()).unwrap_or_default() };
    if let Some(e) = edit["changes"].get(uri) {
        return parse(e);
    }
    edit["documentChanges"]
        .as_array()
        .map(|changes| {
            changes
                .iter()
                .filter(|c| c["textDocument"]["uri"].as_str() == Some(uri))
                .flat_map(|c| parse(&c["edits"]))
                .collect()
        })
        .unwrap_or_default()
}

/// A C# or Visual Basic import directive's sort key, or `None` when `line` is not one: `System` namespaces first,
/// then the rest by name ignoring case; aliases and `static` ones after the plain ones (Visual Studio's order with
/// "Place System directives first").
fn directive_key(line: &str, vb: bool) -> Option<(u8, u8, String)> {
    let t = line.trim();
    let body = if vb {
        let lower = t.to_ascii_lowercase();
        lower
            .starts_with("imports ")
            .then(|| t["imports ".len()..].trim().to_owned())?
    } else {
        let t = t.strip_prefix("global ").unwrap_or(t).trim_start();
        t.strip_prefix("using ")?
            .trim()
            .strip_suffix(';')?
            .trim()
            .to_owned()
    };
    if body.is_empty() || body.starts_with('(') {
        return None;
    }
    let (kind, name) = if let Some(rest) = body.strip_prefix("static ") {
        (2, rest.trim().to_owned())
    } else if body.contains('=') {
        (1, body.clone())
    } else {
        (0, body.clone())
    };
    let system = !(name == "System" || name.starts_with("System."));
    Some((kind, system as u8, name.to_lowercase()))
}

/// The first run of import directives in `text` (lines, blank lines between them allowed) and the same lines sorted:
/// `(byte range of the run, sorted text)`, or `None` when there is no run or it is already sorted.
pub fn sorted_directives(text: &str, vb: bool) -> Option<(std::ops::Range<usize>, String)> {
    let mut offset = 0;
    let mut start = None;
    let mut end = 0;
    let mut lines: Vec<&str> = Vec::new();
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let trimmed = content.trim();
        if directive_key(content, vb).is_some() {
            start.get_or_insert(offset);
            lines.push(content);
            end = offset + line.len();
        } else if start.is_some() && !trimmed.is_empty() {
            break;
        } else if start.is_none()
            && !(trimmed.is_empty()
                || trimmed.starts_with("//")
                || trimmed.starts_with('\'')
                || trimmed.starts_with('#')
                || trimmed.to_ascii_lowercase().starts_with("option "))
        {
            return None;
        }
        offset += line.len();
    }
    let start = start?;
    let newline = if text[start..end].contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut sorted = lines.clone();
    sorted.sort_by_key(|l| directive_key(l, vb));
    sorted.dedup();
    if sorted == lines && !text[start..end].lines().any(|l| l.trim().is_empty()) {
        return None;
    }
    let mut out = sorted.join(newline);
    out.push_str(newline);
    Some((start..end, out))
}

fn output(path: &str, state: &str, applied: bool, message: Option<String>) -> Value {
    let mut v = json!({"path": path, "state": state, "applied": applied});
    if let Some(m) = message {
        v["message"] = json!(m);
    }
    v
}

fn describe(e: RequestError) -> String {
    match e {
        RequestError::Failed(m) => m,
        RequestError::NoHost => "no language server is running for this document".into(),
        RequestError::Stale => "the solution changed while the server answered".into(),
        RequestError::Canceled => "canceled".into(),
    }
}

/// What a request for text edits came to.
type EditsDone = Box<dyn FnOnce(&mut Shell, Result<usize, String>, &mut Context<Shell>)>;

impl Shell {
    /// Wire the agents' jobs (call once from `new`).
    pub(super) fn depth_install(
        &mut self,
        mut jobs: UnboundedReceiver<DepthJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(DepthJob { request, reply }) = jobs.next().await {
                let r = reply.clone();
                let sent = this.update_in(cx, |shell, window, cx| {
                    let outcome = shell.apply_depth(request, Some(r.clone()), window, cx);
                    // Answered now unless the command keeps the reply for later.
                    if let Some(outcome) = outcome {
                        let _ = r.send(outcome);
                    }
                });
                if sent.is_err() {
                    let _ = reply.send(Err(CommandError::Failed("the window is closed".into())));
                }
            }
        });
        self._tasks.push(task);
    }

    /// From `invoke` on the UI thread: run the command and stage its answer for the bus.
    pub(super) fn depth_invoke(
        &mut self,
        command: &str,
        args: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cmd::ALL.contains(&command)
            && let Ok(request) = cmd::parse(command, args.clone())
        {
            let outcome = self
                .apply_depth(request, None, window, cx)
                .unwrap_or_else(|| Ok(json!({})));
            stage(outcome);
        }
    }

    /// Apply a command. `reply`: an agent waits; a command that finishes later keeps it and returns `None`.
    pub(super) fn apply_depth(
        &mut self,
        request: IntellisenseRequest,
        reply: Option<mpsc::SyncSender<Outcome>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Outcome> {
        if let IntellisenseRequest::Snippets { language } = &request {
            let language = language.clone().or_else(|| {
                self.controller
                    .active_document()
                    .and_then(|id| self.documents.get(&id))
                    .and_then(|d| language_of_path(&d.path))
                    .map(str::to_owned)
            });
            let rows: Vec<Value> = self
                .depth
                .library
                .list(language.as_deref())
                .into_iter()
                .map(|s| {
                    let mut row = json!({"shortcut": s.shortcut, "title": s.title, "language": s.language,
                        "kinds": s.kinds.iter().map(|k| k.name()).collect::<Vec<_>>(), "source": s.source});
                    if !s.description.is_empty() {
                        row["description"] = json!(s.description);
                    }
                    row
                })
                .collect();
            return Some(Ok(json!({ "snippets": rows })));
        }
        let id = match self.document_id(request.path()) {
            Ok(id) => id,
            Err(e) => return Some(Err(e)),
        };
        if self.documents[&id].read_only {
            return Some(Err(CommandError::Failed(format!("{id} is read-only"))));
        }
        let command = request.command();
        let answer = |shell: &mut Shell, value: Value| {
            shell
                .depth
                .last
                .insert((command, id.clone()), value.clone());
            Some(Ok(value))
        };
        match request {
            IntellisenseRequest::InsertSnippet { shortcut, .. } => {
                let v = self.insert_snippet_command(&id, shortcut.as_deref(), false, window, cx);
                answer(self, v)
            }
            IntellisenseRequest::SurroundWith { shortcut, .. } => {
                let v = self.insert_snippet_command(&id, shortcut.as_deref(), true, window, cx);
                answer(self, v)
            }
            IntellisenseRequest::FormatSelection { .. } => {
                let (range, _) = self.selected_lines(&id, cx);
                self.depth
                    .last
                    .insert((command, id.clone()), output(&id, "running", false, None));
                let done_id = id.clone();
                let reply_slot = Rc::new(RefCell::new(reply));
                let r = reply_slot.clone();
                self.range_format(
                    &id,
                    range,
                    "format",
                    Box::new(move |shell, result, cx| {
                        let v = match result {
                            Ok(n) => {
                                let mut v = output(&done_id, "done", n > 0, None);
                                v["edits"] = json!(n);
                                v
                            }
                            Err(m) => output(&done_id, "failed", false, Some(m)),
                        };
                        shell.finish_depth(command, &done_id, v, r.borrow_mut().take(), cx);
                    }),
                    cx,
                );
                self.pending_answer(command, &id, reply_slot)
            }
            IntellisenseRequest::RemoveAndSortUsings { .. } => {
                let done_id = id.clone();
                let reply_slot = Rc::new(RefCell::new(reply));
                let r = reply_slot.clone();
                self.depth
                    .last
                    .insert((command, id.clone()), output(&id, "running", false, None));
                self.remove_and_sort_usings(
                    &id,
                    Box::new(move |shell, (sorted, removed, message), cx| {
                        let mut v = output(
                            &done_id,
                            if message.is_some() && !sorted && !removed {
                                "failed"
                            } else {
                                "done"
                            },
                            sorted || removed,
                            message,
                        );
                        v["sorted"] = json!(sorted);
                        v["removed"] = json!(removed);
                        shell.finish_depth(command, &done_id, v, r.borrow_mut().take(), cx);
                    }),
                    cx,
                );
                self.pending_answer(command, &id, reply_slot)
            }
            IntellisenseRequest::CodeCleanup { profile, .. } => {
                let steps: VecDeque<String> = self.depth.settings.profiles[profile as usize - 1]
                    .split(';')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect();
                if self.depth.cleanup.is_some() {
                    return Some(Ok(output(
                        &id,
                        "failed",
                        false,
                        Some("Code Cleanup is already running".into()),
                    )));
                }
                self.depth
                    .last
                    .insert((command, id.clone()), output(&id, "running", false, None));
                let had_reply = reply.is_some();
                self.depth.cleanup = Some(Cleanup {
                    doc: id.clone(),
                    steps,
                    results: Vec::new(),
                    reply,
                    then_save: false,
                });
                self.status
                    .set(eludite_ui::slots::STATE, "Running Code Cleanup...");
                self.next_cleanup_step(cx);
                // An agent's reply is sent when the cleanup finishes; the UI gets the state now.
                (!had_reply).then(|| Ok(self.depth.last[&(command, id.clone())].clone()))
            }
            IntellisenseRequest::Snippets { .. } => unreachable!("answered above"),
        }
    }

    /// The answer now for a command that finishes later: the agent's reply is kept (`None`), the UI gets the state.
    fn pending_answer(
        &mut self,
        command: &'static str,
        id: &str,
        reply: Rc<RefCell<Option<mpsc::SyncSender<Outcome>>>>,
    ) -> Option<Outcome> {
        if reply.borrow().is_some() {
            return None;
        }
        Some(Ok(self
            .depth
            .last
            .get(&(command, id.to_owned()))
            .cloned()
            .unwrap_or_else(|| output(id, "running", false, None))))
    }

    fn finish_depth(
        &mut self,
        command: &'static str,
        id: &str,
        value: Value,
        reply: Option<mpsc::SyncSender<Outcome>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = value["message"].as_str()
            && value["state"] == "failed"
        {
            self.status.set(eludite_ui::slots::STATE, m.to_owned());
        }
        self.depth
            .last
            .insert((command, id.to_owned()), value.clone());
        if let Some(r) = reply {
            let _ = r.send(Ok(value));
        }
        self.wake_intellisense_waiters();
        cx.notify();
    }

    /// The last answer of `command` for document `id` (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn depth_result(&self, command: &str, id: &str) -> Option<&Value> {
        self.depth
            .last
            .iter()
            .find(|((c, d), _)| *c == command && d == id)
            .map(|(_, v)| v)
    }

    // ----- snippets -----

    fn caret_output(&self, id: &str, state: &str, applied: bool, cx: &gpui::App) -> Value {
        let view = &self.documents[id].view;
        let caret = view.read(cx).editor().primary_selection().head;
        let (line, column) = super::intellisense::line_column(view, caret, cx);
        let mut v = output(id, state, applied, None);
        v["line"] = json!(line);
        v["column"] = json!(column);
        v
    }

    fn insert_snippet_command(
        &mut self,
        id: &str,
        shortcut: Option<&str>,
        surround: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Value {
        let doc = &self.documents[id];
        let Some(language) = language_of_path(&doc.path) else {
            return output(
                id,
                "failed",
                false,
                Some("Code snippets are available in C# and Visual Basic documents".into()),
            );
        };
        let kind = if surround {
            SnippetKind::SurroundsWith
        } else {
            SnippetKind::Expansion
        };
        let Some(shortcut) = shortcut else {
            self.open_snippet_picker(id, language, surround, window, cx);
            return output(id, "picker", false, None);
        };
        let Some(snippet) = self
            .depth
            .library
            .find(language, shortcut)
            .filter(|s| s.is(kind))
            .cloned()
        else {
            return output(
                id,
                "failed",
                false,
                Some(format!(
                    "There is no {} snippet '{shortcut}'",
                    if surround { "Surround With" } else { "" }.trim()
                )),
            );
        };
        let view = doc.view.clone();
        let file = doc
            .path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        view.update(cx, |v, cx| {
            let sel = v.editor().primary_selection();
            let range = sel.range();
            let b = v.editor().buffer();
            let row = b.offset_to_point(range.start).row;
            let from = b.point_to_offset(Point::new(row.saturating_sub(400), 0));
            let before = b.text_for_range(from..range.start);
            let body = snippet.to_lsp(&before);
            let base = std::path::Path::new(&file)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            v.insert_snippet(
                range,
                &body,
                &[("TM_FILENAME", file.clone()), ("TM_FILENAME_BASE", base)],
                cx,
            );
        });
        let mut v = self.caret_output(id, "done", true, cx);
        v["shortcut"] = json!(snippet.shortcut);
        v
    }

    fn open_snippet_picker(
        &mut self,
        id: &str,
        language: &str,
        surround: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_snippet_picker(window, cx);
        let kind = if surround {
            SnippetKind::SurroundsWith
        } else {
            SnippetKind::Expansion
        };
        let items: Vec<(String, String)> = self
            .depth
            .library
            .list(Some(language))
            .into_iter()
            .filter(|s| s.is(kind))
            .map(|s| (s.shortcut.clone(), s.description.clone()))
            .collect();
        let anchor = {
            let v = self.documents[id].view.read(cx);
            let caret = v.editor().primary_selection().head;
            v.pixel_position_for_offset(caret)
                .map(|p| point(p.x, p.y + v.line_height()))
                .unwrap_or_else(|| point(px(200.), px(120.)))
        };
        let theme = self.theme;
        let picker = cx.new(|cx| SnippetPicker::new(theme, items, surround, anchor, cx));
        let doc = id.to_owned();
        cx.subscribe_in(&picker, window, move |shell, p, event, window, cx| {
            let surround = p.read(cx).surround;
            shell.close_snippet_picker(window, cx);
            if let PickerEvent::Chosen(shortcut) = event {
                let command = if surround {
                    cmd::SURROUND_WITH
                } else {
                    cmd::INSERT_SNIPPET
                };
                shell.run(
                    command,
                    json!({"path": doc, "shortcut": shortcut}),
                    window,
                    cx,
                );
            }
        })
        .detach();
        picker.focus_handle(cx).focus(window, cx);
        self.depth.picker = Some((picker, id.to_owned()));
        cx.notify();
    }

    pub(super) fn close_snippet_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, id)) = self.depth.picker.take() {
            if let Some(doc) = self.documents.get(&id) {
                doc.view.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        }
    }

    /// The open snippet picker (tests and the shell's render).
    pub fn snippet_picker(&self) -> Option<&Entity<SnippetPicker>> {
        self.depth.picker.as_ref().map(|(p, _)| p)
    }

    /// The code snippets of document `id`'s language as completion items (not after a member access).
    pub(super) fn snippet_completion_items(
        &self,
        id: &str,
        view: &Entity<EditorView>,
        cx: &gpui::App,
    ) -> Vec<eludite_editor::CompletionItem> {
        let Some(language) = self
            .documents
            .get(id)
            .and_then(|d| language_of_path(&d.path))
        else {
            return Vec::new();
        };
        let Some(snap) = view.read(cx).completion() else {
            return Vec::new();
        };
        let b = view.read(cx).editor().buffer();
        let before = b.text_for_range(snap.word_start.saturating_sub(1)..snap.word_start);
        if before == "." {
            return Vec::new();
        }
        self.depth
            .library
            .list(Some(language))
            .into_iter()
            .filter(|s| s.is(SnippetKind::Expansion))
            .map(|s| eludite_editor::CompletionItem {
                label: s.shortcut.clone(),
                kind: eludite_editor::CompletionKind::Snippet,
                detail: Some(s.description.clone()).filter(|d| !d.is_empty()),
                documentation: Some(format!(
                    "{} (code snippet: press Tab twice to insert)",
                    s.title
                )),
                resolved: true,
                commit_characters: Some(std::sync::Arc::new(Vec::new())),
                ..Default::default()
            })
            .collect()
    }

    /// Tab-Tab and the typed-character reports for a newly opened C# or Visual Basic document.
    pub(super) fn depth_document_opened(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let Some(language) = language_of_path(&doc.path) else {
            return;
        };
        let view = doc.view.clone();
        let library = self.depth.library.clone();
        let library = Rc::new(RefCell::new(library));
        self.depth.libraries.push(Rc::downgrade(&library));
        let expander: eludite_editor::snippet_session::SnippetExpander =
            Rc::new(move |word: &str, before: &str| {
                library
                    .borrow()
                    .find(language, word)
                    .filter(|s| s.is(SnippetKind::Expansion))
                    .map(|s| s.to_lsp(before))
            });
        view.update(cx, |v, _| {
            v.set_snippet_expander(Some(expander));
            v.set_typed_character_reports(&['}', ';', '\n', '/', '\''], true);
        });
        if doc.server == ServerKey::Host {
            self.push_roslyn_settings(false);
            self.ensure_workspace_diagnostics(cx);
        }
    }

    // ----- settings -----

    /// The settings this module reads (called from `apply_settings`).
    pub(super) fn depth_apply_settings(&mut self, cx: &mut Context<Self>) {
        let next = {
            let s = self.settings.lock();
            let mut roslyn = serde_json::Map::new();
            for key in ROSLYN_SETTINGS {
                if let Some(spec) = s.schema().get(key) {
                    let value = match spec.kind {
                        eludite_commands::settings::SettingKind::Bool => json!(s.bool(key)),
                        _ => json!(s.string(key)),
                    };
                    roslyn.insert((*key).to_owned(), value);
                }
            }
            DepthSettings {
                format_on_type: s.bool("editor.formatOnType"),
                format_on_paste: s.bool("editor.formatOnPaste"),
                auto_insert: s.bool("dotnet.autoInsert"),
                snippet_paths: s.string("editor.snippets.paths"),
                profiles: [
                    s.string("editor.codeCleanup.profile1"),
                    s.string("editor.codeCleanup.profile2"),
                ],
                cleanup_on_save: s.bool("editor.codeCleanup.onSave"),
                full_solution: s.string("dotnet.backgroundAnalysis.scope") == "fullSolution",
                roslyn,
            }
        };
        let previous = std::mem::replace(&mut self.depth.settings, next.clone());
        if previous.snippet_paths != next.snippet_paths || self.depth.library_task.is_none() {
            let paths = next.snippet_paths.clone();
            let work =
                cx.background_spawn(async move { super::snippets::read_user_snippets(&paths) });
            self.depth.library_task = Some(cx.spawn(async move |this, cx| {
                let (user, problems) = work.await;
                let _ = this.update(cx, |shell, cx| {
                    for p in &problems {
                        trace(format_args!("snippets: {p}"));
                    }
                    shell.depth.library = Library::with_user(user);
                    shell.depth.libraries.retain(|l| {
                        l.upgrade()
                            .map(|l| *l.borrow_mut() = shell.depth.library.clone())
                            .is_some()
                    });
                    cx.notify();
                });
            }));
        }
        if previous.roslyn != next.roslyn {
            self.push_roslyn_settings(true);
        }
        if previous.full_solution != next.full_solution {
            if next.full_solution {
                self.ensure_workspace_diagnostics(cx);
            } else {
                self.stop_workspace_diagnostics(cx);
            }
        }
    }

    /// Send the Roslyn settings to the host (`workspace/didChangeConfiguration`) when they changed, or once per
    /// generation.
    fn push_roslyn_settings(&mut self, changed: bool) {
        let settings = self.depth.settings.roslyn.clone();
        if !changed
            && self
                .depth
                .sent
                .as_ref()
                .is_some_and(|(g, s)| *g == self.generation && *s == settings)
        {
            return;
        }
        if settings.is_empty() {
            return;
        }
        trace(format_args!(
            "workspace/didChangeConfiguration: {} settings",
            settings.len()
        ));
        self.session.notify_untyped(
            "workspace/didChangeConfiguration",
            json!({ "settings": settings }),
        );
        self.depth.sent = Some((self.generation, settings));
    }

    // ----- text edits from the server -----

    /// Send `method` for document `id` and apply the text edits it answers (`pick` finds them in the result) as one
    /// undo step, when the document and generation are still the ones it was computed on.
    fn request_edits(
        &mut self,
        id: &str,
        purpose: &'static str,
        method: &str,
        params: Value,
        pick: fn(&Value, &str) -> Vec<lsp::TextEdit>,
        done: EditsDone,
        cx: &mut Context<Self>,
    ) {
        self.flush_change(id, cx);
        let Some(doc) = self.documents.get(id) else {
            return done(self, Err("the document is closed".into()), cx);
        };
        let base = doc.sent.clone();
        let version = doc.lsp_version;
        let uri = doc.uri.clone();
        let generation = self.doc_generation(id);
        let (_handle, rx) = self.session_for(id).request_value(method, params);
        let doc_id = id.to_owned();
        let method = method.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                shell.depth.tasks.remove(&(doc_id.clone(), purpose));
                let current = shell.documents.get(&doc_id).map(|d| d.lsp_version);
                if current != Some(version) || shell.doc_generation(&doc_id) != generation {
                    trace(format_args!("{method} for {doc_id} dropped (stale)"));
                    return done(shell, Err("the document changed".into()), cx);
                }
                let edits = match reply.result {
                    Ok(v) => pick(&v, &uri),
                    Err(e) => return done(shell, Err(describe(e)), cx),
                };
                if edits.is_empty() {
                    return done(shell, Ok(0), cx);
                }
                match shell.apply_document_edits(&doc_id, &edits, &base, cx) {
                    Ok((n, _)) => {
                        trace(format_args!("{method}: {n} edits applied to {doc_id}"));
                        done(shell, Ok(n), cx)
                    }
                    Err(e) => done(shell, Err(e), cx),
                }
            });
        });
        self.depth.tasks.insert((id.to_owned(), purpose), task);
    }

    fn formatting_options() -> Value {
        json!({"tabSize": eludite_editor::display::TAB_SIZE, "insertSpaces": true})
    }

    /// The selection's whole lines (the caret's line when it is empty), and whether there was a selection.
    fn selected_lines(&self, id: &str, cx: &gpui::App) -> (std::ops::Range<usize>, bool) {
        let e = self.documents[id].view.read(cx).editor();
        let sel = e.primary_selection();
        let b = e.buffer();
        let (start, end) = (b.offset_to_point(sel.start()), b.offset_to_point(sel.end()));
        let last = if end.row > start.row && end.column == 0 {
            end.row - 1
        } else {
            end.row
        };
        let from = b.point_to_offset(Point::new(start.row, 0));
        let to = b.point_to_offset(Point::new(last, b.line(last).len() as u32));
        (from..to, !sel.is_empty())
    }

    /// `textDocument/rangeFormatting` over `range` (byte offsets in the current text).
    fn range_format(
        &mut self,
        id: &str,
        range: std::ops::Range<usize>,
        purpose: &'static str,
        done: EditsDone,
        cx: &mut Context<Self>,
    ) {
        if !self.doc_features(id).range_formatting {
            return done(
                self,
                Err("The language server does not format a selection".into()),
                cx,
            );
        }
        self.flush_change(id, cx);
        let doc = &self.documents[id];
        let params = json!({
            "textDocument": {"uri": doc.uri},
            "range": {"start": lsp_position(&doc.sent, range.start), "end": lsp_position(&doc.sent, range.end)},
            "options": Self::formatting_options(),
        });
        self.request_edits(
            id,
            purpose,
            "textDocument/rangeFormatting",
            params,
            |v, _| serde_json::from_value(v.clone()).unwrap_or_default(),
            done,
            cx,
        );
    }

    /// Format on type and `///` (the editor reported a character).
    pub(super) fn on_character_typed(
        &mut self,
        id: &str,
        ch: char,
        offset: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        if doc.read_only {
            return;
        }
        let features = self.doc_features(id);
        let (line_before, previous_line) = {
            let b = doc.view.read(cx).editor().buffer();
            let p = b.offset_to_point(offset.min(b.len()));
            let line = b.line(p.row);
            let before = line[..(p.column as usize).min(line.len())].to_owned();
            let previous = (p.row > 0).then(|| b.line(p.row - 1)).unwrap_or_default();
            (before, previous)
        };
        let vb = language_of_path(&doc.path) == Some("vb");
        let marker = if vb { "'''" } else { "///" };
        let doc_comment = match ch {
            '/' if !vb => line_before.trim_start() == "///",
            '\'' if vb => line_before.trim_start() == "'''",
            '\n' => previous_line.trim_start().starts_with(marker),
            _ => false,
        };
        let s = ch.to_string();
        if doc_comment
            && self.depth.settings.auto_insert
            && features.auto_insert_triggers.contains(&s)
        {
            self.flush_change(id, cx);
            let doc = &self.documents[id];
            let params = json!({
                "_vs_textDocument": {"uri": doc.uri},
                "_vs_position": lsp_position(&doc.sent, offset),
                "_vs_ch": s,
                "_vs_options": Self::formatting_options(),
            });
            self.auto_insert(id, params, cx);
            return;
        }
        if self.depth.settings.format_on_type && features.on_type_triggers.contains(&s) {
            self.flush_change(id, cx);
            let doc = &self.documents[id];
            let params = json!({
                "textDocument": {"uri": doc.uri},
                "position": lsp_position(&doc.sent, offset),
                "ch": s,
                "options": Self::formatting_options(),
            });
            self.request_edits(
                id,
                "onType",
                "textDocument/onTypeFormatting",
                params,
                |v, _| serde_json::from_value(v.clone()).unwrap_or_default(),
                Box::new(|_, _, _| {}),
                cx,
            );
        }
    }

    /// `textDocument/_vs_onAutoInsert`: the doc comment skeleton, as a snippet when the server says so.
    fn auto_insert(&mut self, id: &str, params: Value, cx: &mut Context<Self>) {
        let doc = &self.documents[id];
        let base = doc.sent.clone();
        let version = doc.lsp_version;
        let generation = self.doc_generation(id);
        let (_h, rx) = self
            .session_for(id)
            .request_value("textDocument/_vs_onAutoInsert", params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                shell.depth.tasks.remove(&(doc_id.clone(), "autoInsert"));
                let Some(doc) = shell.documents.get(&doc_id) else {
                    return;
                };
                if doc.lsp_version != version || shell.doc_generation(&doc_id) != generation {
                    trace(format_args!(
                        "_vs_onAutoInsert for {doc_id} dropped (stale)"
                    ));
                    return;
                }
                let Ok(v) = reply.result else {
                    return;
                };
                let Ok(edit) = serde_json::from_value::<lsp::TextEdit>(v["_vs_textEdit"].clone())
                else {
                    return;
                };
                let snippet = v["_vs_textEditFormat"].as_u64() == Some(2);
                let start = offset_in(&base, edit.range.start);
                let end = offset_in(&base, edit.range.end).max(start);
                let range = base.anchor_before(start)..base.anchor_after(end);
                let view = doc.view.clone();
                view.update(cx, |v, cx| {
                    let b = v.editor().buffer();
                    let r = b.offset_for_anchor(&range.start)..b.offset_for_anchor(&range.end);
                    if snippet {
                        v.insert_snippet(r, &edit.new_text, &[], cx);
                    } else {
                        v.update_editor(cx, |e| e.apply_edits(vec![(r, edit.new_text.clone())]));
                    }
                });
                trace(format_args!("_vs_onAutoInsert applied to {doc_id}"));
                shell.wake_intellisense_waiters();
            });
        });
        self.depth.tasks.insert((id.to_owned(), "autoInsert"), task);
    }

    /// Format on paste: the pasted lines with `textDocument/rangeFormatting`.
    pub(super) fn on_pasted(
        &mut self,
        id: &str,
        range: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.depth.settings.format_on_paste
            || !self.doc_features(id).range_formatting
            || self.documents.get(id).is_none_or(|d| d.read_only)
        {
            return;
        }
        self.range_format(id, range, "paste", Box::new(|_, _, _| {}), cx);
    }

    /// Roslyn's `roslyn.client.completionComplexEdit` (override and partial-method completion): replace the edit's
    /// range (in `base`, the text the server resolved on) with its text, as a snippet when the third argument says so,
    /// and put the caret at the fourth (an offset in the text after the edit; -1: none). Joins the commit's undo step.
    pub(super) fn apply_complex_edit(
        &mut self,
        id: &str,
        command: &Value,
        base: &text::BufferSnapshot,
        commit: Option<text::TransactionId>,
        cx: &mut Context<Self>,
    ) {
        let args = &command["arguments"];
        let Ok(edit) = serde_json::from_value::<lsp::TextEdit>(args[1].clone()) else {
            trace(format_args!("completionComplexEdit: no text edit"));
            return;
        };
        let snippet = args[2].as_bool() == Some(true);
        let new_position = args[3].as_i64().unwrap_or(-1);
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let start = offset_in(base, edit.range.start);
        let end = offset_in(base, edit.range.end).max(start);
        let start_utf16 = base.offset_to_offset_utf16(start).0 as i64;
        let anchors = base.anchor_before(start)..base.anchor_after(end);
        let view = doc.view.clone();
        view.update(cx, |v, cx| {
            let still_last = v.editor().last_transaction() == commit;
            let b = v.editor().buffer();
            let r = b.offset_for_anchor(&anchors.start)..b.offset_for_anchor(&anchors.end);
            if snippet {
                v.insert_snippet(r, &edit.new_text, &[], cx);
            } else {
                let at = r.start;
                v.update_editor(cx, |e| {
                    e.apply_edits(vec![(r, edit.new_text.clone())]);
                    // The caret: the new position's place in the inserted text.
                    let rel = new_position - start_utf16;
                    if new_position >= 0 && rel >= 0 {
                        let mut units = 0i64;
                        let mut byte = edit.new_text.len();
                        for (i, c) in edit.new_text.char_indices() {
                            if units >= rel {
                                byte = i;
                                break;
                            }
                            units += c.len_utf16() as i64;
                        }
                        e.set_caret(at + byte);
                    }
                });
            }
            if still_last && let (Some(commit), Some(tx)) = (commit, v.editor().last_transaction())
            {
                v.update_editor(cx, |e| e.merge_transactions(tx, commit));
            }
        });
        trace(format_args!("completionComplexEdit applied to {id}"));
        self.wake_intellisense_waiters();
    }

    // ----- Remove and Sort Usings -----

    /// Sort the import directives (one undo step), then apply the server's "Remove unnecessary usings" fix when it
    /// offers one. `done` gets (sorted, removed, message).
    fn remove_and_sort_usings(
        &mut self,
        id: &str,
        done: Box<dyn FnOnce(&mut Shell, (bool, bool, Option<String>), &mut Context<Shell>)>,
        cx: &mut Context<Self>,
    ) {
        let doc = &self.documents[id];
        let vb = language_of_path(&doc.path) == Some("vb");
        let view = doc.view.clone();
        let text = view.read(cx).editor().text();
        let sorted = match sorted_directives(&text, vb) {
            Some((range, replacement)) => {
                view.update(cx, |v, cx| {
                    v.update_editor(cx, |e| e.apply_edits(vec![(range, replacement)]))
                });
                true
            }
            None => false,
        };
        if language_of_path(&self.documents[id].path).is_none()
            || self.provider(&self.documents[id]) == super::intellisense::Provider::Syntax
        {
            return done(self, (sorted, false, None), cx);
        }
        // The server's fix over the directives (Roslyn offers it from its own diagnostics).
        self.flush_change(id, cx);
        let doc = &self.documents[id];
        let text = doc.sent.text();
        let end = sorted_directives_end(&text, vb);
        let params = json!({
            "textDocument": {"uri": doc.uri},
            "range": {"start": {"line": 0, "character": 0}, "end": lsp_position(&doc.sent, end)},
            "context": {"diagnostics": self.diagnostics.get(&doc.uri).map(|ds| {
                ds.iter().filter(|d| matches!(d.code.as_ref().and_then(Value::as_str), Some("IDE0005" | "CS8019" | "BC50001"))).cloned().collect::<Vec<_>>()
            }).unwrap_or_default(), "only": ["source.organizeImports", "quickfix"], "triggerKind": 1},
        });
        let (_h, rx) = self
            .session_for(id)
            .request_value("textDocument/codeAction", params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                let actions = reply
                    .result
                    .ok()
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default();
                let flat: Vec<Value> = actions
                    .iter()
                    .flat_map(|a| {
                        let nested = a["command"]["arguments"][0]["NestedCodeActions"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default();
                        std::iter::once(a.clone()).chain(nested)
                    })
                    .collect();
                let pick = flat.into_iter().find(|a| {
                    let title = a["title"].as_str().unwrap_or_default().to_lowercase();
                    a["kind"] == "source.organizeImports"
                        || title.starts_with("remove unnecessary using")
                        || title.starts_with("remove unnecessary import")
                });
                let Some(action) = pick else {
                    return done(shell, (sorted, false, None), cx);
                };
                shell.resolve_and_apply_action(
                    &doc_id,
                    "codeAction/resolve",
                    action,
                    "usings",
                    Box::new(move |shell, result, cx| match result {
                        Ok(n) => done(shell, (sorted, n > 0, None), cx),
                        Err(m) => done(shell, (sorted, false, Some(m)), cx),
                    }),
                    cx,
                );
            });
        });
        self.depth.tasks.insert((id.to_owned(), "usings"), task);
    }

    /// Resolve a code action (`codeAction/resolve`, or `codeAction/resolveFixAll` with its scope in `action`) and
    /// apply its edit to document `id`.
    fn resolve_and_apply_action(
        &mut self,
        id: &str,
        method: &str,
        action: Value,
        purpose: &'static str,
        done: EditsDone,
        cx: &mut Context<Self>,
    ) {
        if action.get("edit").is_some() {
            // Already resolved: apply against the text the server has.
            let edit = action["edit"].clone();
            self.flush_change(id, cx);
            let doc = &self.documents[id];
            let edits = edits_for(&edit, &doc.uri);
            let base = doc.sent.clone();
            let r = self
                .apply_document_edits(id, &edits, &base, cx)
                .map(|(n, _)| n);
            return done(self, r, cx);
        }
        self.request_edits(
            id,
            purpose,
            method,
            action,
            |v, uri| edits_for(&v["edit"], uri),
            done,
            cx,
        );
    }

    // ----- Code Cleanup -----

    fn next_cleanup_step(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.depth.cleanup.as_mut() else {
            return;
        };
        let doc = c.doc.clone();
        let Some(step) = c.steps.pop_front() else {
            let c = self.depth.cleanup.take().expect("checked");
            let applied = c.results.iter().any(|r| r["state"] == "applied");
            let mut v = output(&c.doc, "done", applied, None);
            v["steps"] = json!(c.results);
            self.status
                .set(eludite_ui::slots::STATE, "Code Cleanup completed");
            if c.then_save {
                let saved = self.save(Some(&c.doc), cx);
                if let Ok(eludite_commands::workspace::WorkspaceOutput::Save(out)) = saved {
                    self.formatting.saved.insert(c.doc.clone(), out);
                }
            }
            self.finish_depth(cmd::CODE_CLEANUP, &c.doc, v, c.reply, cx);
            return;
        };
        if !self.documents.contains_key(&doc) {
            self.depth.cleanup = None;
            return;
        }
        let name = step.clone();
        let finish = move |shell: &mut Shell,
                           state: &str,
                           message: Option<String>,
                           cx: &mut Context<Shell>| {
            if let Some(c) = shell.depth.cleanup.as_mut() {
                let mut r = json!({"step": name, "state": state});
                if let Some(m) = message {
                    r["message"] = json!(m);
                }
                c.results.push(r);
            }
            shell.next_cleanup_step(cx);
        };
        let edits_state = |r: &Result<usize, String>| match r {
            Ok(0) => "unchanged",
            Ok(_) => "applied",
            Err(_) => "failed",
        };
        match step.as_str() {
            "removeAndSortUsings" => self.remove_and_sort_usings(
                &doc,
                Box::new(move |shell, (sorted, removed, message), cx| {
                    let state = if sorted || removed {
                        "applied"
                    } else if message.is_some() {
                        "failed"
                    } else {
                        "unchanged"
                    };
                    finish(shell, state, message, cx)
                }),
                cx,
            ),
            "format" => {
                self.flush_change(&doc, cx);
                let params = json!({"textDocument": {"uri": self.documents[&doc].uri}, "options": Self::formatting_options()});
                self.request_edits(
                    &doc,
                    "format",
                    "textDocument/formatting",
                    params,
                    |v, _| serde_json::from_value(v.clone()).unwrap_or_default(),
                    Box::new(move |shell, r, cx| {
                        let state = edits_state(&r);
                        finish(shell, state, r.err(), cx)
                    }),
                    cx,
                );
            }
            s if s.starts_with("fixAll:") => {
                let id = s["fixAll:".len()..].trim().to_owned();
                self.fix_all_in_document(
                    &doc,
                    &id,
                    Box::new(move |shell, r, cx| match r {
                        Err(m) if m.starts_with("skipped: ") => finish(
                            shell,
                            "skipped",
                            Some(m["skipped: ".len()..].to_owned()),
                            cx,
                        ),
                        r => {
                            let state = edits_state(&r);
                            finish(shell, state, r.err(), cx)
                        }
                    }),
                    cx,
                );
            }
            other => {
                let message = format!("'{other}' is not a Code Cleanup step");
                finish(self, "skipped", Some(message), cx)
            }
        }
    }

    /// Roslyn's Fix All in the document for diagnostic `code`: the Fix All action of a fix for one of its
    /// diagnostics, resolved with `codeAction/resolveFixAll` and the scope `Document`. `Err("skipped: ...")` when the
    /// document has no such diagnostic or no Fix All for it.
    fn fix_all_in_document(
        &mut self,
        id: &str,
        code: &str,
        done: EditsDone,
        cx: &mut Context<Self>,
    ) {
        let doc = &self.documents[id];
        let Some(diagnostic) = self.diagnostics.get(&doc.uri).and_then(|ds| {
            ds.iter()
                .find(|d| d.code.as_ref().and_then(Value::as_str) == Some(code))
                .cloned()
        }) else {
            return done(
                self,
                Err(format!("skipped: no {code} diagnostics in the document")),
                cx,
            );
        };
        self.flush_change(id, cx);
        let doc = &self.documents[id];
        let params = json!({
            "textDocument": {"uri": doc.uri},
            "range": diagnostic.range,
            "context": {"diagnostics": [diagnostic], "triggerKind": 1},
        });
        let (_h, rx) = self
            .session_for(id)
            .request_value("textDocument/codeAction", params);
        let doc_id = id.to_owned();
        let code = code.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                let actions = match reply.result {
                    Ok(v) => v.as_array().cloned().unwrap_or_default(),
                    Err(e) => return done(shell, Err(describe(e)), cx),
                };
                let fix_all = actions.into_iter().find(|a| {
                    a["command"]["command"] == FIX_ALL_COMMAND
                        && a["diagnostics"].as_array().is_none_or(|ds| {
                            ds.is_empty() || ds.iter().any(|d| d["code"] == code.as_str())
                        })
                });
                let Some(action) = fix_all else {
                    return done(shell, Err(format!("skipped: no Fix All for {code}")), cx);
                };
                let data = action
                    .get("data")
                    .cloned()
                    .unwrap_or_else(|| action["command"]["arguments"][0].clone());
                let params = json!({"title": action["title"], "data": data, "scope": "Document"});
                shell.resolve_and_apply_action(
                    &doc_id,
                    "codeAction/resolveFixAll",
                    params,
                    "fixAll",
                    done,
                    cx,
                );
            });
        });
        self.depth.tasks.insert((id.to_owned(), "fixAll"), task);
    }

    /// Format on save's sibling (brief 0069): with `editor.codeCleanup.onSave`, saving a C# or Visual Basic document
    /// runs Code Cleanup profile 1 first and writes the file when it is done.
    pub(super) fn cleanup_on_save(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Option<Result<eludite_commands::workspace::WorkspaceOutput, CommandError>> {
        let doc = self.documents.get(id)?;
        if !self.depth.settings.cleanup_on_save
            || doc.read_only
            || language_of_path(&doc.path).is_none()
            || self.depth.cleanup.is_some()
        {
            return None;
        }
        self.formatting.saved.remove(id);
        self.depth.cleanup = Some(Cleanup {
            doc: id.to_owned(),
            steps: self.depth.settings.profiles[0]
                .split(';')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            results: Vec::new(),
            reply: None,
            then_save: true,
        });
        self.next_cleanup_step(cx);
        Some(Ok(eludite_commands::workspace::WorkspaceOutput::Save(
            eludite_commands::workspace::SaveOutput {
                path: id.to_owned(),
                bytes: 0,
                pending: true,
            },
        )))
    }

    // ----- full-solution analysis -----

    /// Keep one `workspace/diagnostic` in flight while the scope is the entire solution and a solution is open.
    pub(super) fn ensure_workspace_diagnostics(&mut self, cx: &mut Context<Self>) {
        if !self.depth.settings.full_solution
            || self.solution_state.is_none()
            || self
                .depth
                .workspace
                .as_ref()
                .is_some_and(|(g, _)| *g == self.generation)
        {
            return;
        }
        let generation = self.generation;
        if self
            .depth
            .workspace
            .as_ref()
            .is_some_and(|(g, _)| *g != generation)
        {
            self.depth.result_ids.clear();
        }
        let previous: Vec<Value> = self
            .depth
            .result_ids
            .iter()
            .map(|(uri, id)| json!({"uri": uri, "value": id}))
            .collect();
        trace(format_args!(
            "workspace/diagnostic ({} previous results)",
            previous.len()
        ));
        let (handle, rx) = self.session.request_value(
            "workspace/diagnostic",
            json!({"previousResultIds": previous}),
        );
        let task = cx.spawn(async move |this, cx| {
            let reply = rx.await;
            let _ = this.update(cx, |shell, cx| {
                drop(handle);
                if shell.generation != generation || !shell.depth.settings.full_solution {
                    shell.depth.workspace = None;
                    return;
                }
                shell.depth.workspace = None;
                let Ok(reply) = reply else {
                    return;
                };
                match reply.result {
                    Ok(v) => {
                        shell.on_workspace_diagnostics(&v, cx);
                        // Roslyn holds the next request until something changes.
                        shell.ensure_workspace_diagnostics(cx);
                    }
                    Err(e) => trace(format_args!("workspace/diagnostic failed: {}", describe(e))),
                }
            });
        });
        self.depth.workspace = Some((generation, task));
    }

    /// A `workspace/diagnostic` report: closed files' diagnostics go to the Error List (open files' come from the
    /// documents' own pulls).
    fn on_workspace_diagnostics(&mut self, report: &Value, cx: &mut Context<Self>) {
        let open: std::collections::BTreeSet<String> =
            self.documents.values().map(|d| d.uri.clone()).collect();
        for item in report["items"].as_array().into_iter().flatten() {
            let Some(uri) = item["uri"].as_str() else {
                continue;
            };
            if item["kind"] == "unchanged" {
                continue;
            }
            match item["resultId"].as_str() {
                Some(id) => {
                    self.depth.result_ids.insert(uri.to_owned(), id.to_owned());
                }
                None => {
                    self.depth.result_ids.remove(uri);
                }
            }
            if open.contains(uri) {
                continue;
            }
            let diagnostics: Vec<lsp::Diagnostic> =
                serde_json::from_value(item["items"].clone()).unwrap_or_default();
            if diagnostics.is_empty() {
                self.depth.workspace_uris.remove(uri);
            } else {
                self.depth.workspace_uris.insert(uri.to_owned());
            }
            self.on_diagnostics(
                lsp::PublishDiagnosticsParams {
                    uri: uri.to_owned(),
                    version: None,
                    diagnostics,
                },
                cx,
            );
        }
        self.wake_intellisense_waiters();
    }

    /// The scope went back to open files: the closed files' rows leave the Error List.
    fn stop_workspace_diagnostics(&mut self, cx: &mut Context<Self>) {
        self.depth.workspace = None;
        self.depth.result_ids.clear();
        let open: std::collections::BTreeSet<String> =
            self.documents.values().map(|d| d.uri.clone()).collect();
        for uri in std::mem::take(&mut self.depth.workspace_uris) {
            if !open.contains(&uri) {
                self.on_diagnostics(
                    lsp::PublishDiagnosticsParams {
                        uri,
                        version: None,
                        diagnostics: Vec::new(),
                    },
                    cx,
                );
            }
        }
    }
}

/// The end of the import directives' run (or 0).
fn sorted_directives_end(text: &str, vb: bool) -> usize {
    let mut offset = 0;
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if directive_key(content, vb).is_some() {
            end = offset + content.len();
        } else if end > 0 && !content.trim().is_empty() {
            break;
        }
        offset += line.len();
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directives_sort_system_first_then_by_name() {
        let text = "// header\nusing Zeta;\nusing System.Text;\nusing static System.Math;\nusing Alpha;\nusing System;\n\nnamespace N;\n";
        let (range, sorted) = sorted_directives(text, false).unwrap();
        assert_eq!(
            &text[range.clone()],
            "using Zeta;\nusing System.Text;\nusing static System.Math;\nusing Alpha;\nusing System;\n"
        );
        assert_eq!(
            sorted,
            "using System;\nusing System.Text;\nusing Alpha;\nusing Zeta;\nusing static System.Math;\n"
        );
        assert!(sorted_directives("using System;\nusing A;\n\nclass C {}\n", false).is_none());
        assert!(sorted_directives("class C { void M() { using var x = y; } }", false).is_none());
        let vb = "Option Strict On\nImports System.Linq\nImports System\n";
        assert_eq!(
            sorted_directives(vb, true).unwrap().1,
            "Imports System\nImports System.Linq\n"
        );
        assert_eq!(
            sorted_directives_end("using B;\nusing A;\nclass C {}", false),
            17
        );
    }

    #[test]
    fn workspace_edits_give_a_documents_edits() {
        let e = json!({"changes": {"file:///a.cs": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "x"}]}});
        assert_eq!(edits_for(&e, "file:///a.cs").len(), 1);
        assert!(edits_for(&e, "file:///b.cs").is_empty());
        let d = json!({"documentChanges": [{"textDocument": {"uri": "file:///b.cs", "version": 3}, "edits": [
            {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 0}}, "newText": "y"}]}]});
        assert_eq!(edits_for(&d, "file:///b.cs")[0].new_text, "y");
    }
}
