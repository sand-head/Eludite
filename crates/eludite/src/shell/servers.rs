//! Which language server a document belongs to (brief 0019): `eludite-host` for C#, or a generic server the shell
//! launches from its registration (`eludite-lsp`'s `servers.json`: rust-analyzer for `*.rs`), one per registration
//! and workspace root. Every editor feature reaches the server through the document's [`ServerSession`] and reads
//! the server's state, capabilities and generation through [`Shell::doc_generation`], [`Shell::doc_features`] and
//! [`Shell::provider`](super::intellisense), so none of them has a per-language path.
//!
//! A generic server's state goes to its own status bar slot (`language_server:<id>`): `starting`, its work-done
//! progress (`Indexing 120/300 (core) 40%`), `ready` once `experimental/serverStatus` reports it quiescent, or why it
//! is unavailable. Its log (stderr, `window/logMessage`, `window/showMessage`) goes to the Output window's Language
//! Servers source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use eludite_commands::build::OutputSource;
use eludite_lsp::host::{LanguageServerState, LanguageServerStatus};
use eludite_lsp::{Connector, Progress, ServerRegistration, ServerRegistry, ServerStatus, Via};
use futures::StreamExt as _;
use gpui::{Context, Window};

use super::Shell;
use super::documents::trace;
use super::intellisense::ServerFeatures;
use super::session::{GenericLaunch, ServerSession, SessionEvent};

/// The server a document belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum ServerKey {
    /// `eludite-host` (Roslyn), or no server at all for a file no registration matches.
    #[default]
    Host,
    /// A generic server: `<registration id>|<root>`.
    Generic(String),
}

/// How the shell finds generic servers.
#[derive(Clone)]
pub struct ServerLaunches {
    pub registry: ServerRegistry,
    /// Servers running in this process instead of the located executables, by registration id (the fake server in
    /// tests).
    pub in_process: BTreeMap<String, Connector>,
}

impl Default for ServerLaunches {
    fn default() -> Self {
        Self {
            registry: ServerRegistry::builtin(),
            in_process: BTreeMap::new(),
        }
    }
}

/// When the steps of a generic server's start happened (the brief 0019 budgets).
#[derive(Debug, Default, Clone)]
// Read by the tests and the `--timings-out` harness.
#[allow(dead_code)]
pub struct ServerTimings {
    /// Its first document was opened (the session was created).
    pub requested: Option<Instant>,
    /// `initialize` answered.
    pub running: Option<Instant>,
    /// The first diagnostics for an open document arrived.
    pub first_diagnostics: Option<Instant>,
    /// The first non-empty ones.
    pub first_nonempty_diagnostics: Option<Instant>,
    /// `experimental/serverStatus` first reported it quiescent.
    pub quiescent: Option<Instant>,
}

/// A generic server and what the shell knows of it.
pub struct GenericServer {
    pub registration: ServerRegistration,
    pub root: PathBuf,
    pub session: ServerSession,
    pub state: Option<LanguageServerState>,
    pub version: Option<String>,
    pub message: Option<String>,
    pub status: Option<ServerStatus>,
    /// Work-done progress in flight, by token.
    pub progress: BTreeMap<String, Progress>,
    /// The token updated last (shown).
    pub latest: Option<String>,
    pub features: ServerFeatures,
    /// One more per (re)start; results computed under another one are dropped.
    pub generation: u64,
    pub timings: ServerTimings,
}

impl GenericServer {
    /// The status bar slot's text.
    pub fn status_text(&self) -> String {
        let name = &self.registration.name;
        match self.state {
            None | Some(LanguageServerState::Starting) => format!("{name}: starting\u{2026}"),
            Some(LanguageServerState::Restarting) => format!(
                "{name}: restarting\u{2026}{}",
                self.message
                    .as_deref()
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            ),
            Some(LanguageServerState::Unavailable) => format!(
                "{name}: unavailable{}",
                self.message
                    .as_deref()
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            ),
            Some(LanguageServerState::Exited) => format!(
                "{name}: exited{}",
                self.message
                    .as_deref()
                    .map(|m| format!(" ({m})"))
                    .unwrap_or_default()
            ),
            Some(LanguageServerState::Running) => {
                if let Some(p) = self.latest.as_ref().and_then(|t| self.progress.get(t)) {
                    let mut text = format!("{name}: {}", p.title.as_deref().unwrap_or("working"));
                    if let Some(m) = &p.message {
                        text.push(' ');
                        text.push_str(m);
                    }
                    if let Some(pct) = p.percentage {
                        text.push_str(&format!(" {pct}%"));
                    }
                    return text;
                }
                match &self.status {
                    Some(s) if s.health != "ok" => format!(
                        "{name}: {}{}",
                        s.health,
                        s.message
                            .as_deref()
                            .map(|m| format!(" ({})", m.lines().next().unwrap_or_default()))
                            .unwrap_or_default()
                    ),
                    Some(s) if !s.quiescent => format!("{name}: loading\u{2026}"),
                    _ => format!(
                        "{name}: ready{}",
                        self.version
                            .as_deref()
                            .map(|v| format!(" ({v})"))
                            .unwrap_or_default()
                    ),
                }
            }
        }
    }

    /// Whether IntelliSense can rely on the server alone (else it shows the syntax fallback too).
    pub fn ready(&self) -> bool {
        self.state == Some(LanguageServerState::Running)
            && self.status.as_ref().is_none_or(|s| s.quiescent)
    }

    /// Whether the server cannot answer at all.
    pub fn down(&self) -> bool {
        matches!(
            self.state,
            Some(LanguageServerState::Unavailable | LanguageServerState::Exited)
        )
    }
}

/// The status bar slot of generic server `key`.
pub fn slot(key: &str) -> String {
    format!(
        "{}:{}",
        super::LANGUAGE_SERVER_SLOT,
        key.split('|').next().unwrap_or(key)
    )
}

impl Shell {
    /// The server for a file at `path`: its key, its session and its LSP language id; `None` when no registration
    /// matches. Starts a generic session (not yet the server) for a new registration and root.
    pub(super) fn server_for_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(ServerKey, ServerSession, String)> {
        let reg = self.launches.registry.for_path(path)?.clone();
        match reg.via {
            Via::EluditeHost => Some((ServerKey::Host, self.session.clone(), reg.language_id)),
            Via::Process => {
                let root = self.generic_root(&reg, path);
                let key = format!("{}|{}", reg.id, root.to_string_lossy());
                if !self.generic.contains_key(&key) {
                    self.start_generic(key.clone(), reg.clone(), root, window, cx);
                }
                let session = self.generic[&key].session.clone();
                Some((ServerKey::Generic(key), session, reg.language_id))
            }
        }
    }

    /// The workspace root a generic server for `path` runs at: the open workspace's root for the registration (the
    /// Cargo workspace root from `cargo metadata`) when the file is inside it, else the nearest folder with a root
    /// marker, else the file's folder.
    fn generic_root(&self, reg: &ServerRegistration, path: &Path) -> PathBuf {
        if let Some(root) = self.folder_root_for(reg, path) {
            return root;
        }
        reg.find_root(path)
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_default()
    }

    fn start_generic(
        &mut self,
        key: String,
        registration: ServerRegistration,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        trace(format_args!(
            "{} session for {}",
            registration.id,
            root.display()
        ));
        let connector = self.launches.in_process.get(&registration.id).cloned();
        let (session, mut events) = ServerSession::spawn_generic(GenericLaunch {
            registration: registration.clone(),
            root: root.clone(),
            connector,
        });
        let slot_id = slot(&key);
        if !self.status.has_slot(&slot_id) {
            self.status.add_slot(slot_id, eludite_ui::SlotAlign::Right);
        }
        let task_key = key.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        for event in batch {
                            shell.on_server_event(&task_key, event, window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self._tasks.push(task);
        self.generic.insert(
            key.clone(),
            GenericServer {
                registration,
                root,
                session,
                state: Some(LanguageServerState::Starting),
                version: None,
                message: None,
                status: None,
                progress: BTreeMap::new(),
                latest: None,
                features: ServerFeatures::default(),
                generation: 0,
                timings: ServerTimings {
                    requested: Some(Instant::now()),
                    ..Default::default()
                },
            },
        );
        self.refresh_server_slot(&key);
    }

    fn refresh_server_slot(&mut self, key: &str) {
        if let Some(s) = self.generic.get(key) {
            let text = s.status_text();
            self.status.set(&slot(key), text);
        }
    }

    /// What a generic server's session reports.
    pub(super) fn on_server_event(
        &mut self,
        key: &str,
        event: SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.generic.contains_key(key) {
            return;
        }
        match event {
            SessionEvent::LanguageServer(status) => self.on_generic_status(key, status, cx),
            SessionEvent::ServerGeneration(g) => {
                let server = self.generic.get_mut(key).expect("checked above");
                if g != server.generation {
                    server.generation = g;
                    server.progress.clear();
                    server.latest = None;
                    server.status = None;
                    self.on_generic_generation(key, cx);
                }
            }
            SessionEvent::Progress(p) => {
                let server = self.generic.get_mut(key).expect("checked above");
                if p.kind == "end" {
                    server.progress.remove(&p.token);
                    if server.latest.as_deref() == Some(p.token.as_str()) {
                        server.latest = server.progress.keys().next_back().cloned();
                    }
                } else {
                    let token = p.token.clone();
                    let entry = server.progress.entry(token.clone()).or_insert(p.clone());
                    if p.kind == "report" {
                        // A report carries only what changed.
                        if p.message.is_some() {
                            entry.message = p.message;
                        }
                        if p.percentage.is_some() {
                            entry.percentage = p.percentage;
                        }
                    } else {
                        *entry = p;
                    }
                    server.latest = Some(token);
                }
            }
            SessionEvent::ServerStatus(s) => {
                let server = self.generic.get_mut(key).expect("checked above");
                let was_ready = server.ready();
                if s.quiescent {
                    server.timings.quiescent.get_or_insert_with(Instant::now);
                    trace(format_args!("{} quiescent ({})", key, s.health));
                }
                server.status = Some(s);
                if server.ready() && !was_ready {
                    self.refresh_fallback_lists(cx);
                }
            }
            SessionEvent::Diagnostics(params) => {
                if let Some(server) = self.generic.get_mut(key)
                    && self.documents.values().any(|d| d.uri == params.uri)
                {
                    server
                        .timings
                        .first_diagnostics
                        .get_or_insert_with(Instant::now);
                    if !params.diagnostics.is_empty() {
                        server
                            .timings
                            .first_nonempty_diagnostics
                            .get_or_insert_with(Instant::now);
                    }
                }
                self.on_diagnostics(params, cx);
            }
            SessionEvent::ApplyEdit {
                id,
                generation,
                params,
            } => self.on_server_apply_edit(
                ServerKey::Generic(key.to_owned()),
                id,
                generation,
                params,
                window,
                cx,
            ),
            SessionEvent::HostLog(line) => self.output.update(cx, |o, cx| {
                o.append(OutputSource::LanguageServers, &format!("{line}\n"), cx)
            }),
            // Host-only events never come from a generic session.
            _ => {}
        }
        self.refresh_server_slot(key);
        cx.notify();
    }

    fn on_generic_status(
        &mut self,
        key: &str,
        status: LanguageServerStatus,
        cx: &mut Context<Self>,
    ) {
        let server = self.generic.get_mut(key).expect("checked by the caller");
        trace(format_args!(
            "{key}: {:?} {:?}",
            status.state, status.message
        ));
        server.state = Some(status.state);
        server.message = status.message;
        if let Some(info) = status.server_info {
            server.version = info.version;
        }
        if let Some(caps) = &status.capabilities {
            server.features = ServerFeatures::from_capabilities(caps);
        }
        if status.state == LanguageServerState::Running {
            server.timings.running.get_or_insert_with(Instant::now);
        }
        if server.down() {
            // IntelliSense falls back to the syntax tree for its documents.
            self.refresh_fallback_lists(cx);
        }
    }

    /// A generic server restarted: what it computed before is stale (CLAUDE.md invariant 12).
    fn on_generic_generation(&mut self, key: &str, cx: &mut Context<Self>) {
        let generic = ServerKey::Generic(key.to_owned());
        let uris: Vec<String> = self
            .documents
            .values()
            .filter(|d| d.server == generic)
            .map(|d| d.uri.clone())
            .collect();
        for doc in self.documents.values_mut().filter(|d| d.server == generic) {
            doc.clear_diagnostics(cx);
            doc.intellisense.cancel_all();
        }
        for uri in uris {
            self.diagnostics.remove(&uri);
        }
        self.update_error_list(cx);
    }

    /// The generation of the server document `id` belongs to (the solution generation for the host).
    pub(super) fn doc_generation(&self, id: &str) -> u64 {
        self.key_generation(self.documents.get(id).map(|d| &d.server))
    }

    /// The generation of server `key`.
    pub(super) fn key_generation(&self, key: Option<&ServerKey>) -> u64 {
        match key {
            Some(ServerKey::Generic(k)) => self.generic.get(k).map_or(0, |g| g.generation),
            _ => self.generation,
        }
    }

    /// What the server of document `id` supports.
    pub(super) fn doc_features(&self, id: &str) -> ServerFeatures {
        match self.documents.get(id).map(|d| &d.server) {
            Some(ServerKey::Generic(k)) => self
                .generic
                .get(k)
                .map(|g| g.features.clone())
                .unwrap_or_default(),
            _ => self.features.clone(),
        }
    }

    /// The session of document `id` (the host's for a document without a server).
    pub(super) fn session_for(&self, id: &str) -> ServerSession {
        self.documents
            .get(id)
            .map(|d| d.session.clone())
            .unwrap_or_else(|| self.session.clone())
    }

    /// The server a diagnostics URI belongs to: its open document's, else by registration.
    pub(super) fn uri_server(&self, uri: &str) -> ServerKey {
        if let Some(d) = self.documents.values().find(|d| d.uri == uri) {
            return d.server.clone();
        }
        let Some(path) = super::documents::uri_to_path(uri) else {
            return ServerKey::Host;
        };
        match self.launches.registry.for_path(&path) {
            Some(reg) if reg.via == Via::Process => self
                .generic
                .iter()
                .find(|(_, g)| g.registration.id == reg.id && path.starts_with(&g.root))
                .map(|(k, _)| ServerKey::Generic(k.clone()))
                .unwrap_or(ServerKey::Host),
            _ => ServerKey::Host,
        }
    }

    /// The generic servers (tests, the timing harness).
    // Read by the tests and the `--timings-out` harness.
    #[allow(dead_code)]
    pub fn generic_servers(&self) -> &BTreeMap<String, GenericServer> {
        &self.generic
    }

    /// Shut every generic server down (window close).
    pub fn shutdown_generic(&self) -> Vec<std::sync::mpsc::Receiver<()>> {
        self.generic
            .values()
            .map(|g| g.session.shutdown())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(state: Option<LanguageServerState>) -> GenericServer {
        let (session, _events) =
            ServerSession::spawn(super::super::session::HostLaunch::Missing("none".into()));
        GenericServer {
            registration: ServerRegistry::builtin()
                .get("rust-analyzer")
                .unwrap()
                .clone(),
            root: "/w".into(),
            session,
            state,
            version: Some("1.98.1".into()),
            message: None,
            status: None,
            progress: BTreeMap::new(),
            latest: None,
            features: ServerFeatures::default(),
            generation: 1,
            timings: ServerTimings::default(),
        }
    }

    #[test]
    fn the_slot_shows_state_progress_and_readiness() {
        let mut s = server(Some(LanguageServerState::Starting));
        assert_eq!(s.status_text(), "rust-analyzer: starting\u{2026}");
        s.state = Some(LanguageServerState::Running);
        s.progress.insert(
            "t".into(),
            Progress {
                token: "t".into(),
                kind: "report".into(),
                title: Some("Indexing".into()),
                message: Some("120/300 (core)".into()),
                percentage: Some(40),
            },
        );
        s.latest = Some("t".into());
        assert_eq!(
            s.status_text(),
            "rust-analyzer: Indexing 120/300 (core) 40%"
        );
        assert!(s.ready());
        s.progress.clear();
        s.status = Some(ServerStatus {
            health: "ok".into(),
            quiescent: false,
            message: None,
        });
        assert!(!s.ready());
        assert_eq!(s.status_text(), "rust-analyzer: loading\u{2026}");
        s.status = Some(ServerStatus {
            health: "ok".into(),
            quiescent: true,
            message: None,
        });
        assert_eq!(s.status_text(), "rust-analyzer: ready (1.98.1)");
        s.state = Some(LanguageServerState::Unavailable);
        s.message = Some("rust-analyzer not found".into());
        assert!(s.down());
        assert_eq!(
            s.status_text(),
            "rust-analyzer: unavailable (rust-analyzer not found)"
        );
        assert_eq!(slot("rust-analyzer|/w"), "language_server:rust-analyzer");
    }
}
