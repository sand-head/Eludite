//! Diagnostics of a generic server, pushed and pulled, merged per document (brief 0019).
//!
//! The pinned rust-analyzer answers `textDocument/diagnostic` (pull) with its own native diagnostics and pushes
//! `textDocument/publishDiagnostics` for `cargo check` results. The shell's editor features take one diagnostics
//! list per document, as `eludite-host` delivers them (its warming turns Roslyn's pulls into pushes), so this
//! client does the same for a generic server that advertises `diagnosticProvider`: it pulls on `didOpen` at once,
//! 150 ms after the last `didChange`, for every open document when the server becomes quiescent or asks with
//! `workspace/diagnostic/refresh`, cancels a superseded pull, drops a result for a document version that is no
//! longer current, and delivers the union of the pushed and pulled lists as [`Event::Diagnostics`].

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eludite_protocol::Id;
use eludite_protocol::host::{Generation, WithGeneration};
use eludite_protocol::lsp::{
    self, Diagnostic, DocumentDiagnosticParams, DocumentDiagnosticReport, PublishDiagnosticsParams,
    TextDocumentIdentifier,
};
use serde_json::Value;

use crate::connection::{Connection, Event, lock};

/// Quiet time after the last `didChange` before a pull (as the host's warming debounce).
pub const PULL_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Default)]
struct State {
    /// Open documents and the version last sent.
    versions: HashMap<String, i32>,
    pushed: HashMap<String, Vec<Diagnostic>>,
    pulled: HashMap<String, Vec<Diagnostic>>,
}

enum Cmd {
    Due(String, Duration),
    All,
    Stop,
}

/// The merged diagnostics of one generic connection.
#[derive(Default)]
pub(crate) struct Diagnostics {
    state: Mutex<State>,
    puller: Mutex<Option<Sender<Cmd>>>,
}

fn key(d: &Diagnostic) -> String {
    format!(
        "{}:{}-{}:{}|{}|{}",
        d.range.start.line,
        d.range.start.character,
        d.range.end.line,
        d.range.end.character,
        d.code.as_ref().map(Value::to_string).unwrap_or_default(),
        d.message
    )
}

impl Diagnostics {
    /// Forget everything (a new server process).
    pub fn reset(&self) {
        *lock(&self.state) = State::default();
    }

    /// Start pulling (the server advertises `diagnosticProvider`). Idempotent.
    pub fn enable_pull(self: &Arc<Self>, conn: &Connection) {
        let mut puller = lock(&self.puller);
        if puller.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<Cmd>();
        let this = self.clone();
        let conn = conn.clone();
        let spawned = thread::Builder::new()
            .name("eludite-lsp-pull".into())
            .spawn(move || {
                let mut due: HashMap<String, Instant> = HashMap::new();
                let mut inflight: HashMap<String, Id> = HashMap::new();
                let inflight_done: Arc<Mutex<HashSet<String>>> = Arc::default();
                loop {
                    let wait = due
                        .values()
                        .min()
                        .map(|t| t.saturating_duration_since(Instant::now()))
                        .unwrap_or(Duration::from_secs(3600));
                    match rx.recv_timeout(wait) {
                        Ok(Cmd::Due(uri, delay)) => {
                            due.insert(uri, Instant::now() + delay);
                        }
                        Ok(Cmd::All) => {
                            for uri in lock(&this.state).versions.keys() {
                                due.insert(uri.clone(), Instant::now());
                            }
                        }
                        Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    if conn.stopping() {
                        return;
                    }
                    let now = Instant::now();
                    let ready: Vec<String> = due
                        .iter()
                        .filter(|(_, t)| **t <= now)
                        .map(|(u, _)| u.clone())
                        .collect();
                    for uri in lock(&inflight_done).drain() {
                        inflight.remove(&uri);
                    }
                    for uri in ready {
                        due.remove(&uri);
                        if let Some(id) = inflight.remove(&uri) {
                            conn.cancel_id(id);
                        }
                        let Some(version) = lock(&this.state).versions.get(&uri).copied() else {
                            continue;
                        };
                        let pending = conn.request::<lsp::DocumentDiagnosticRequest>(
                            DocumentDiagnosticParams {
                                text_document: TextDocumentIdentifier { uri: uri.clone() },
                                identifier: None,
                                previous_result_id: None,
                            },
                        );
                        let Ok(pending) = pending else { continue };
                        inflight.insert(uri.clone(), pending.id());
                        let generation = conn.generation();
                        let (this, conn, done) =
                            (this.clone(), conn.clone(), inflight_done.clone());
                        let _ = thread::Builder::new()
                            .name("eludite-lsp-pull-reply".into())
                            .spawn(move || {
                                let result = pending.wait();
                                lock(&done).insert(uri.clone());
                                if let Ok(DocumentDiagnosticReport::Full { items, .. }) = result {
                                    this.on_pulled(&conn, generation, &uri, version, items);
                                }
                            });
                    }
                }
            });
        if spawned.is_ok() {
            *puller = Some(tx);
        }
    }

    fn send(&self, cmd: Cmd) {
        if let Some(tx) = lock(&self.puller).as_ref() {
            let _ = tx.send(cmd);
        }
    }

    pub fn stop(&self) {
        self.send(Cmd::Stop);
    }

    /// Pull every open document again.
    pub fn pull_all(&self) {
        self.send(Cmd::All);
    }

    /// The shell sent a document notification.
    pub fn on_sent(&self, conn: &Connection, method: &str, params: &Value) {
        let doc = &params["textDocument"];
        let Some(uri) = doc["uri"].as_str().map(str::to_owned) else {
            return;
        };
        match method {
            "textDocument/didOpen" | "textDocument/didChange" => {
                let version = doc["version"].as_i64().unwrap_or(0) as i32;
                lock(&self.state).versions.insert(uri.clone(), version);
                let delay = if method == "textDocument/didOpen" {
                    Duration::ZERO
                } else {
                    PULL_DEBOUNCE
                };
                self.send(Cmd::Due(uri, delay));
            }
            "textDocument/didClose" => {
                {
                    let mut s = lock(&self.state);
                    s.versions.remove(&uri);
                    s.pushed.remove(&uri);
                    s.pulled.remove(&uri);
                }
                conn.emit(Event::Diagnostics(WithGeneration {
                    params: PublishDiagnosticsParams {
                        uri,
                        version: None,
                        diagnostics: Vec::new(),
                    },
                    generation: conn.generation(),
                }));
            }
            _ => {}
        }
    }

    /// `textDocument/publishDiagnostics` from the server: the event to deliver (the merged list).
    pub fn on_pushed(&self, conn: &Connection, params: PublishDiagnosticsParams) -> Event {
        let merged = {
            let mut s = lock(&self.state);
            s.pushed.insert(params.uri.clone(), params.diagnostics);
            merge(&s, &params.uri)
        };
        Event::Diagnostics(WithGeneration {
            params: PublishDiagnosticsParams {
                uri: params.uri,
                version: params.version,
                diagnostics: merged,
            },
            generation: conn.generation(),
        })
    }

    fn on_pulled(
        &self,
        conn: &Connection,
        generation: Generation,
        uri: &str,
        version: i32,
        items: Vec<Diagnostic>,
    ) {
        if conn.generation() != generation {
            return;
        }
        let merged = {
            let mut s = lock(&self.state);
            // A newer version was sent since: its own pull is due, this one is stale.
            if s.versions.get(uri) != Some(&version) {
                return;
            }
            s.pulled.insert(uri.to_owned(), items);
            merge(&s, uri)
        };
        conn.emit(Event::Diagnostics(WithGeneration {
            params: PublishDiagnosticsParams {
                uri: uri.to_owned(),
                version: Some(version),
                diagnostics: merged,
            },
            generation,
        }));
    }
}

/// The pushed and pulled diagnostics of `uri`, without duplicates.
fn merge(s: &State, uri: &str) -> Vec<Diagnostic> {
    let mut seen = HashSet::new();
    s.pulled
        .get(uri)
        .into_iter()
        .chain(s.pushed.get(uri))
        .flatten()
        .filter(|d| seen.insert(key(d)))
        .cloned()
        .collect()
}

/// Whether `capabilities` advertise pull diagnostics.
pub(crate) fn advertises_pull(capabilities: Option<&Value>) -> bool {
    capabilities
        .and_then(|c| c.get("diagnosticProvider"))
        .is_some_and(|p| !p.is_null() && p != &Value::Bool(false))
}
