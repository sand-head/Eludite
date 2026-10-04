//! Format Document and format on save (brief 0050): Edit > Advanced > Format Document (Ctrl+K, Ctrl+D) and
//! `eludite.editor.format_document` run the formatter the setting `editor.formatter` chooses for the document
//! (`eludite-lsp`'s `ServerRegistry::pick_formatter` over `servers.json`'s formatters): the project's Prettier or
//! Biome run out of process on the document's text (stdin to stdout, in the file's folder so the project's
//! configuration applies), else the language server's `textDocument/formatting` (the first of the document's servers
//! that offers it). Nothing here knows a language: the formatters and their file kinds are data.
//!
//! - **Off the UI thread.** Choosing the formatter (a `node_modules` search, `--version` probes) and running it happen
//!   on a background thread; the server's formatting is a request like any other. The UI thread only applies the
//!   result.
//! - **One undo step, through the applier.** A formatter's output becomes one edit (the changed middle of the text)
//!   and the server's edits stay as they are; both go through the workspace-edit applier with the document's LSP
//!   version and generation at the request, so a document typed into meanwhile is left alone ("the document changed").
//! - **Failures** (a formatter that exits with an error, or is not found when chosen) write their message to the
//!   Output window's Language Servers source and leave the document.
//! - **Format on save** (`editor.formatOnSave.<registration id>` for the document's primary server: `typescript`,
//!   `html`, `css`, `json`): File > Save (Ctrl+S) and `eludite.editor.save` format first and write the file once the
//!   formatter is done (or failed); an agent's save is answered after the write.
//! - **Warm Prettier.** A formatter with a JavaScript API (`servers.json`'s `worker`: Prettier's `format` and
//!   `resolveConfig`) runs in a Node.js worker kept after its first use, one per Node.js and package, reading one JSON
//!   request per line on stdin; later runs skip Node's and Prettier's start (the 2,000-line budget). A worker that
//!   cannot start or stops answering is dropped and the formatter runs as a process.
//! - `node` runs only here, when Format Document or format on save asks; never at startup.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::build::OutputSource;
use eludite_commands::workspace::{FormatDocumentOutput, FormatState, SaveOutput, WorkspaceOutput};
use eludite_lsp::lsp;
use eludite_lsp::registry::{Environment, FormatterPick, ServerRegistry, substitute};
use gpui::{AppContext as _, Context, Task, Window};
use serde_json::{Value, json};

use super::Shell;
use super::documents::trace;
use super::session::RequestError;
use super::workspace_edit::ApplyOptions;

/// How long a formatter process may run.
pub const FORMATTER_TIMEOUT: Duration = Duration::from_secs(30);

/// The warm formatter worker's script (run with `node --input-type=module -e`): its arguments are the package's
/// `package.json`, the package's name and the API's format and resolve-config function names; each stdin line is
/// `{"id", "text", "file"}`, each stdout line `{"id", "text"}` or `{"id", "error"}`.
const WORKER_SCRIPT: &str = r#"
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { createInterface } from "node:readline";
const [packageJson, name, formatName, resolveName] = process.argv.slice(1);
const mod = await import(pathToFileURL(createRequire(packageJson).resolve(name)).href);
const api = mod.default ?? mod;
for await (const line of createInterface({ input: process.stdin })) {
  let id = null;
  let answer;
  try {
    const request = JSON.parse(line);
    id = request.id;
    const config = resolveName && api[resolveName] ? (await api[resolveName](request.file)) ?? {} : {};
    answer = { id, text: await api[formatName](request.text, { ...config, filepath: request.file }) };
  } catch (e) {
    answer = { id, error: String((e && e.message) || e).split(String.fromCharCode(10))[0] };
  }
  process.stdout.write(JSON.stringify(answer) + String.fromCharCode(10));
}
"#;

/// A warm formatter worker.
struct Worker {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: std::sync::mpsc::Receiver<String>,
    next: u64,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The warm workers, by (Node.js, package.json).
fn workers() -> &'static std::sync::Mutex<HashMap<(PathBuf, PathBuf), Worker>> {
    static WORKERS: std::sync::OnceLock<std::sync::Mutex<HashMap<(PathBuf, PathBuf), Worker>>> =
        std::sync::OnceLock::new();
    WORKERS.get_or_init(Default::default)
}

/// The `package.json` of npm package `name` that holds `script` (a file of it).
fn package_json_of(script: &Path, name: &str) -> Option<PathBuf> {
    script.ancestors().skip(1).find_map(|d| {
        let p = d.join("package.json");
        let text = std::fs::read_to_string(&p).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        (v["name"].as_str() == Some(name)).then_some(p)
    })
}

/// Format with a warm worker: `Err` when the worker cannot be used (the caller runs the process instead), else the
/// formatter's own answer.
fn worker_format(
    node: &Path,
    package_json: &Path,
    package: &str,
    api: &eludite_lsp::registry::WorkerSpec,
    file: &Path,
    text: &str,
) -> Result<Result<String, String>, String> {
    let mut all = workers().lock().unwrap_or_else(|e| e.into_inner());
    let key = (node.to_path_buf(), package_json.to_path_buf());
    if !all.contains_key(&key) {
        let mut child = Command::new(node)
            .args(["--input-type=module", "-e", WORKER_SCRIPT])
            .arg(package_json)
            .arg(package)
            .arg(&api.format)
            .arg(api.resolve_config.as_deref().unwrap_or(""))
            .current_dir(package_json.parent().unwrap_or(Path::new("/")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("the formatter worker did not start: {e}"))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("eludite-format-worker".into())
            .spawn(move || {
                use std::io::BufRead as _;
                for line in std::io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else { return };
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        all.insert(
            key.clone(),
            Worker {
                child,
                stdin,
                lines,
                next: 0,
            },
        );
    }
    let worker = all.get_mut(&key).expect("inserted above");
    worker.next += 1;
    let id = worker.next;
    let request = json!({"id": id, "text": text, "file": file.to_string_lossy()});
    let sent = writeln!(worker.stdin, "{request}").and_then(|_| worker.stdin.flush());
    let answer = sent.ok().and_then(|_| {
        let deadline = Instant::now() + FORMATTER_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = worker.lines.recv_timeout(left).ok()?;
            let v: Value = serde_json::from_str(&line).ok()?;
            if v["id"] == json!(id) {
                return Some(v);
            }
        }
    });
    match answer {
        Some(v) => Ok(match (v["text"].as_str(), v["error"].as_str()) {
            (Some(t), _) => Ok(t.to_owned()),
            (_, e) => Err(e.unwrap_or("the formatter failed").to_owned()),
        }),
        None => {
            all.remove(&key);
            Err("the formatter worker stopped answering".into())
        }
    }
}

/// Format Document's state in the shell.
#[derive(Default)]
pub struct Formatting {
    /// The last answer per document (the command's output, also while it runs).
    last: HashMap<String, FormatDocumentOutput>,
    running: HashMap<String, Task<()>>,
    /// The save that followed format on save, per document: what an agent's `eludite.editor.save` answers.
    saved: HashMap<String, SaveOutput>,
    /// How long each Format Document took, request to applied (tests and the harness).
    pub timings: Vec<Duration>,
}

/// What the formatter thread found.
enum Outcome {
    /// A formatter's output.
    Text {
        id: String,
        version: String,
        text: String,
    },
    /// The language server formats it.
    Server,
    Failed {
        formatter: String,
        message: String,
    },
}

/// Run `program args` in `dir` with `input` on stdin, killed after `timeout`: (exit success, stdout, stderr).
pub fn run_with_stdin(
    program: &Path,
    args: &[String],
    dir: &Path,
    input: &str,
    timeout: Duration,
) -> Result<(bool, String, String), String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} did not start: {e}", program.display()))?;
    let mut stdin = child.stdin.take().expect("piped");
    let input = input.to_owned();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} did not finish in {} s",
                    program.display(),
                    timeout.as_secs()
                ));
            }
        }
    };
    let _ = writer.join();
    Ok((
        status.success(),
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    ))
}

/// Choose and run the formatter for `file` (a background thread's work).
fn format_off_thread(
    registry: &ServerRegistry,
    file: &Path,
    text: &str,
    choice: &str,
    cache: Option<PathBuf>,
    node: Option<PathBuf>,
) -> Outcome {
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let node_found = || -> Result<PathBuf, String> {
        let user = eludite_lsp::node::WEB_SERVERS;
        let (path, _) = eludite_lsp::node::NodeSearch {
            configured: node.clone(),
            ..eludite_lsp::node::NodeSearch::from_env()
        }
        .find(&user)?;
        Ok(path)
    };
    let env = Environment {
        beside: None,
        var: &|k| std::env::var_os(k).filter(|v| !v.is_empty()),
        path_var: std::env::var_os("PATH"),
        probe: &eludite_lsp::registry::probe_version,
        rustup: &|_| None,
        project: Some(&dir),
        cache: cache.as_deref(),
        node: &node_found,
    };
    let picking = Instant::now();
    let picked = registry.pick_formatter(file, choice, &env);
    trace(format_args!(
        "format: formatter chosen in {:?}",
        picking.elapsed()
    ));
    let (spec, located) = match picked {
        Ok(FormatterPick::Server) => return Outcome::Server,
        Ok(FormatterPick::Formatter(spec, located)) => (spec, located),
        Err(message) => {
            return Outcome::Failed {
                formatter: choice.to_owned(),
                message,
            };
        }
    };
    // Warm: the package's API in a kept worker.
    if let (Some(api), Some(node), Some(package)) = (
        &spec.worker,
        &located.node,
        spec.command.npm_package.as_deref(),
    ) && let Some(package_json) = package_json_of(&located.path, package)
    {
        let formatting = Instant::now();
        let answer = worker_format(node, &package_json, package, api, file, text);
        trace(format_args!(
            "format: {} answered in {:?}",
            spec.name,
            formatting.elapsed()
        ));
        match answer {
            Ok(Ok(text)) => {
                return Outcome::Text {
                    id: spec.id.clone(),
                    version: located.version,
                    text,
                };
            }
            Ok(Err(message)) => {
                return Outcome::Failed {
                    formatter: spec.id.clone(),
                    message: format!("{} {} failed: {message}", spec.name, located.version),
                };
            }
            Err(why) => trace(format_args!(
                "{}: {why}; running it as a process",
                spec.name
            )),
        }
    }
    let file_arg = file.to_string_lossy().into_owned();
    let args: Vec<String> = spec
        .command
        .args
        .iter()
        .map(|a| {
            substitute(&Value::String(a.clone()), &|v| {
                (v == "file").then(|| file_arg.clone())
            })
            .as_str()
            .unwrap_or_default()
            .to_owned()
        })
        .collect();
    let (program, args) = match &located.node {
        Some(node) => {
            let mut all = vec![located.path.to_string_lossy().into_owned()];
            all.extend(args);
            (node.clone(), all)
        }
        None => (located.path.clone(), args),
    };
    match run_with_stdin(&program, &args, &dir, text, FORMATTER_TIMEOUT) {
        Ok((true, out, _)) => Outcome::Text {
            id: spec.id.clone(),
            version: located.version,
            text: out,
        },
        Ok((false, _, err)) => Outcome::Failed {
            formatter: spec.id.clone(),
            message: format!(
                "{} {} failed: {}",
                spec.name,
                located.version,
                err.lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("it exited with an error")
                    .trim()
            ),
        },
        Err(message) => Outcome::Failed {
            formatter: spec.id.clone(),
            message,
        },
    }
}

impl Shell {
    /// `eludite.editor.format_document`: start formatting document `path` (the active one) and answer at once with
    /// `formatting`; an agent waits for `done` through [`Shell::format_state`].
    pub(super) fn format_document_command(
        &mut self,
        path: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        if self.documents[&id].read_only {
            return Err(CommandError::Failed(format!("{id} is read-only")));
        }
        self.start_format(id.clone(), false, window, cx);
        Ok(WorkspaceOutput::FormatDocument(self.format_output(&id)))
    }

    /// The answer for document `id`: the last Format Document's.
    fn format_output(&self, id: &str) -> FormatDocumentOutput {
        self.formatting
            .last
            .get(id)
            .cloned()
            .unwrap_or(FormatDocumentOutput {
                path: id.to_owned(),
                state: FormatState::Formatting,
                formatter: "none".into(),
                server: None,
                version: None,
                applied: false,
                edits: 0,
                message: None,
            })
    }

    /// An agent's format or save, polled until it is done (see `WorkspaceOutput::is_loading`).
    pub(super) fn format_state(
        &self,
        request: &eludite_commands::workspace::WorkspaceRequest,
    ) -> Option<Result<WorkspaceOutput, CommandError>> {
        use eludite_commands::workspace::WorkspaceRequest;
        let id = match self.document_id(request.path()) {
            Ok(id) => id,
            Err(e) => return Some(Err(e)),
        };
        match request {
            WorkspaceRequest::FormatDocument { .. } => {
                Some(Ok(WorkspaceOutput::FormatDocument(self.format_output(&id))))
            }
            WorkspaceRequest::Save { .. } => Some(Ok(WorkspaceOutput::Save(
                self.formatting
                    .saved
                    .get(&id)
                    .cloned()
                    .unwrap_or(SaveOutput {
                        path: id,
                        bytes: 0,
                        pending: true,
                    }),
            ))),
            _ => None,
        }
    }

    /// Whether saving document `id` formats it first (`editor.formatOnSave.<its primary server's registration>`).
    fn format_on_save_due(&self, id: &str) -> bool {
        let Some(doc) = self.documents.get(id) else {
            return false;
        };
        if doc.read_only {
            return false;
        }
        let Some(reg) = self.launches.registry.for_path(&doc.path) else {
            return false;
        };
        self.launches
            .format_on_save
            .get(&reg.id)
            .copied()
            .unwrap_or(false)
    }

    /// `eludite.editor.save` with format on save: format first, then write. `None` when the save needs no format
    /// (the caller saves as before).
    pub(super) fn save_with_format(
        &mut self,
        path: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Result<WorkspaceOutput, CommandError>> {
        let id = self.document_id(path).ok()?;
        if !self.format_on_save_due(&id) {
            return None;
        }
        self.formatting.saved.remove(&id);
        self.start_format(id.clone(), true, window, cx);
        Some(Ok(WorkspaceOutput::Save(SaveOutput {
            path: id,
            bytes: 0,
            pending: true,
        })))
    }

    /// Format document `id` off the UI thread; with `then_save`, save it afterwards (format on save).
    fn start_format(
        &mut self,
        id: String,
        then_save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flush_change(&id, cx);
        let Some(doc) = self.documents.get(&id) else {
            return;
        };
        let started = Instant::now();
        let file = doc.path.clone();
        let text = doc.sent.text();
        let version = doc.lsp_version;
        let generation = self.doc_generation(&id);
        let key = self.doc_key(&id);
        let registry = self.launches.registry.clone();
        let choice = self.launches.formatter.clone();
        let cache = self.launches.cache();
        let node = self.launches.node.clone();
        self.formatting.last.insert(
            id.clone(),
            FormatDocumentOutput {
                path: id.clone(),
                state: FormatState::Formatting,
                formatter: "none".into(),
                server: None,
                version: None,
                applied: false,
                edits: 0,
                message: None,
            },
        );
        trace(format_args!(
            "format {id} with {choice} (version {version})"
        ));
        let base = text.clone();
        let work = cx.background_spawn(async move {
            format_off_thread(&registry, &file, &text, &choice, cache, node)
        });
        let task_id = id.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let outcome = work.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.formatted(
                    task_id, outcome, base, version, generation, key, then_save, started, window,
                    cx,
                )
            });
        });
        self.formatting.running.insert(id, task);
    }

    #[allow(clippy::too_many_arguments)]
    fn formatted(
        &mut self,
        id: String,
        outcome: Outcome,
        base: String,
        version: i32,
        generation: u64,
        key: super::servers::ServerKey,
        then_save: bool,
        started: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.formatting.running.remove(&id);
        let Some(uri) = self.documents.get(&id).map(|d| d.uri.clone()) else {
            return;
        };
        match outcome {
            Outcome::Failed { formatter, message } => {
                self.format_failed(&id, &formatter, None, message, then_save, window, cx);
            }
            Outcome::Text {
                id: f,
                version: v,
                text,
            } => {
                let edits: Vec<Value> = super::session::diff(&base, &text)
                    .and_then(|c| {
                        Some(json!({"range": serde_json::to_value(c.range?).ok()?, "newText": c.text}))
                    })
                    .into_iter()
                    .collect();
                self.apply_format(
                    id,
                    uri,
                    f,
                    None,
                    Some(v),
                    edits,
                    version,
                    generation,
                    key,
                    then_save,
                    started,
                    window,
                    cx,
                );
            }
            Outcome::Server => {
                let session = self.session_for(&id);
                let server = match &self.documents[&id].server {
                    super::servers::ServerKey::Generic(k) => {
                        self.generic.get(k).map(|g| g.registration.id.clone())
                    }
                    _ => self
                        .launches
                        .registry
                        .for_path(&self.documents[&id].path)
                        .map(|r| r.id.clone()),
                };
                let (_, rx) = session.request_value(
                    "textDocument/formatting",
                    json!({
                        "textDocument": {"uri": uri},
                        "options": {"tabSize": eludite_editor::display::TAB_SIZE, "insertSpaces": true}
                    }),
                );
                let task_id = id.clone();
                let task = cx.spawn_in(window, async move |this, cx| {
                    let Ok(reply) = rx.await else {
                        return;
                    };
                    let _ = this.update_in(cx, |shell, window, cx| {
                        shell.formatting.running.remove(&task_id);
                        match reply.result {
                            Ok(v) => {
                                let edits = v.as_array().cloned().unwrap_or_default();
                                shell.apply_format(
                                    task_id,
                                    uri,
                                    "server".into(),
                                    server,
                                    None,
                                    edits,
                                    version,
                                    generation,
                                    key,
                                    then_save,
                                    started,
                                    window,
                                    cx,
                                );
                            }
                            Err(e) => {
                                let message = match e {
                                    RequestError::Failed(m) => m,
                                    RequestError::NoHost => {
                                        "no language server formats this file".into()
                                    }
                                    other => format!("{other:?}"),
                                };
                                shell.format_failed(
                                    &task_id, "server", server, message, then_save, window, cx,
                                );
                            }
                        }
                    });
                });
                self.formatting.running.insert(id, task);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn format_failed(
        &mut self,
        id: &str,
        formatter: &str,
        server: Option<String>,
        message: String,
        then_save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        trace(format_args!("format {id} failed: {message}"));
        self.output.update(cx, |o, cx| {
            o.append(
                OutputSource::LanguageServers,
                &format!("Format Document: {id}: {message}\n"),
                cx,
            )
        });
        self.status.set(
            eludite_ui::slots::STATE,
            format!("Format Document failed: {message}"),
        );
        self.formatting.last.insert(
            id.to_owned(),
            FormatDocumentOutput {
                path: id.to_owned(),
                state: FormatState::Done,
                formatter: formatter.to_owned(),
                server,
                version: None,
                applied: false,
                edits: 0,
                message: Some(message),
            },
        );
        self.after_format(id, then_save, window, cx);
    }

    /// Apply a formatter's (or the server's) edits through the applier as one undo step.
    #[allow(clippy::too_many_arguments)]
    fn apply_format(
        &mut self,
        id: String,
        uri: String,
        formatter: String,
        server: Option<String>,
        formatter_version: Option<String>,
        edits: Vec<Value>,
        version: i32,
        generation: u64,
        key: super::servers::ServerKey,
        then_save: bool,
        started: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = edits.len() as u64;
        let done = |shell: &mut Shell, applied: bool, message: Option<String>| {
            shell.formatting.last.insert(
                id.clone(),
                FormatDocumentOutput {
                    path: id.clone(),
                    state: FormatState::Done,
                    formatter: formatter.clone(),
                    server: server.clone(),
                    version: formatter_version.clone(),
                    applied,
                    edits: if applied { count } else { 0 },
                    message,
                },
            );
        };
        if edits.is_empty() {
            done(self, false, None);
            self.status.set(
                eludite_ui::slots::STATE,
                "The document is already formatted",
            );
            self.after_format(&id, then_save, window, cx);
            return;
        }
        let edit: lsp::WorkspaceEdit = match serde_json::from_value(json!({
            "documentChanges": [{"textDocument": {"uri": uri, "version": version}, "edits": edits}]
        })) {
            Ok(e) => e,
            Err(e) => {
                let message = format!("the formatter's edits are not text edits: {e}");
                done(self, false, Some(message.clone()));
                self.format_failed(
                    &id,
                    &formatter,
                    server.clone(),
                    message,
                    then_save,
                    window,
                    cx,
                );
                return;
            }
        };
        let options = ApplyOptions {
            label: Some("Format Document".into()),
            generation: Some(generation),
            server: key,
            versions: [(id.clone(), version)].into_iter().collect(),
        };
        let (id2, formatter2, server2, fv2) = (
            id.clone(),
            formatter.clone(),
            server.clone(),
            formatter_version.clone(),
        );
        self.apply_workspace_edit(
            &edit,
            options,
            window,
            cx,
            Box::new(move |shell, summary, window, cx| {
                shell.formatting.timings.push(started.elapsed());
                shell.formatting.last.insert(
                    id2.clone(),
                    FormatDocumentOutput {
                        path: id2.clone(),
                        state: FormatState::Done,
                        formatter: formatter2,
                        server: server2,
                        version: fv2,
                        applied: summary.applied,
                        edits: if summary.applied { count } else { 0 },
                        message: summary.message.clone().filter(|_| !summary.applied),
                    },
                );
                if !summary.applied
                    && let Some(m) = &summary.message
                {
                    shell.output.update(cx, |o, cx| {
                        o.append(
                            OutputSource::LanguageServers,
                            &format!("Format Document: {id2}: {m}\n"),
                            cx,
                        )
                    });
                }
                shell.after_format(&id2, then_save, window, cx);
            }),
        );
    }

    /// After formatting: save when format on save asked, and wake an agent waiting for the answer.
    fn after_format(
        &mut self,
        id: &str,
        then_save: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if then_save {
            // Written now, as formatted (the save command's own path, which does not format again).
            let saved = self.save(Some(id), cx);
            if let Ok(WorkspaceOutput::Save(out)) = saved {
                let path = out.path.clone();
                self.formatting.saved.insert(id.to_owned(), out);
                cx.defer_in(window, move |shell, window, cx| {
                    shell.build_after_save(&path, window, cx)
                });
            }
        }
        self.wake_intellisense_waiters();
        cx.notify();
    }

    /// Format Document's per-document answers (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn format_result(&self, id: &str) -> Option<&FormatDocumentOutput> {
        self.formatting.last.get(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The warm worker loads a formatter package's API once and answers each request (with the package's own
    /// configuration lookup), reports the formatter's error as its answer, and is not used when Node.js cannot load
    /// the package (the caller then runs the formatter as a process). Skips without `node` on PATH.
    #[test]
    fn the_warm_worker_formats_reports_errors_and_gives_way() {
        let Some(node) = std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join(eludite_lsp::node::node_name()))
                .find(|n| n.is_file())
        }) else {
            eprintln!("skipped: no node on PATH");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("node_modules/fakefmt");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{"name": "fakefmt", "version": "1.0.0", "main": "index.js"}"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join("index.js"),
            "let loads = (globalThis.loads ?? 0) + 1; globalThis.loads = loads;\n\
             exports.resolveConfig = async (file) => ({ marker: file.endsWith('.ts') ? 'ts' : 'other' });\n\
             exports.format = async (text, o) => { if (text.includes('BROKEN')) throw new Error('Unexpected token (1:1)\\nmore'); \
             return `[${o.marker} ${loads}] ` + text.trim(); };\n",
        )
        .unwrap();
        let api = eludite_lsp::registry::WorkerSpec {
            format: "format".into(),
            resolve_config: Some("resolveConfig".into()),
        };
        let manifest = pkg.join("package.json");
        let file = dir.path().join("a.ts");
        for i in 0..3 {
            let out = worker_format(&node, &manifest, "fakefmt", &api, &file, &format!(" x{i} "))
                .expect("the worker runs");
            // Loaded once: every answer comes from the same, warm, module.
            assert_eq!(out, Ok(format!("[ts 1] x{i}")));
        }
        let broken = worker_format(&node, &manifest, "fakefmt", &api, &file, "BROKEN").unwrap();
        assert_eq!(broken, Err("Unexpected token (1:1)".to_owned()));
        // A package Node.js cannot load: the worker exits, so it is not used.
        let missing = dir.path().join("node_modules/nothing/package.json");
        std::fs::create_dir_all(missing.parent().unwrap()).unwrap();
        std::fs::write(&missing, r#"{"name": "nothing"}"#).unwrap();
        assert!(worker_format(&node, &missing, "nothing", &api, &file, "x").is_err());
    }
}
