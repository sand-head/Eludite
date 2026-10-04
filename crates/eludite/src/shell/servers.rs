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
//!
//! Several servers per document (brief 0050): every registration that matches a file (and is activated: ESLint only
//! with a configuration, `languageServers.eslint`) serves it, in `servers.json`'s order; the document's session fans
//! out to them ([`ServerSession::fan_out`]). Each server's diagnostics are kept apart and shown together; the
//! document's generation is the sum of its servers', so a restart of any of them drops what was computed before.
//! A server that is not found says `not found (run tools/web-servers/fetch.sh)` in its slot; one that runs on a
//! located module says which (`TypeScript: ready (TypeScript 5.9.3, project)`).

use std::collections::{BTreeMap, HashMap};
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
    /// Several generic servers serving one document (brief 0050), primary first: the key whose generation an edit
    /// computed for that document is checked against (the sum of theirs).
    Group(Vec<String>),
}

/// How the shell finds generic servers.
#[derive(Clone)]
pub struct ServerLaunches {
    pub registry: ServerRegistry,
    /// Servers running in this process instead of the located executables, by registration id (the fake server in
    /// tests).
    pub in_process: BTreeMap<String, Connector>,
    /// The setting `languageServers.rustAnalyzerPath` (brief 0020): tried first when the registration's override
    /// variable is not set.
    pub rust_analyzer: Option<PathBuf>,
    /// The values of the registrations' settings (brief 0050): activation settings (`languageServers.eslint`) and
    /// module paths (`languageServers.typescriptPath`), by key; empty when unset.
    pub settings: BTreeMap<String, String>,
    /// `languageServers.nodePath`.
    pub node: Option<PathBuf>,
    /// The web servers' cache folder (`ELUDITE_WEB_SERVERS`, else `~/.cache/eludite/web-servers/<pin>`), as found
    /// when the first web document opened; `None` until looked for, `Some(None)` when it is not there.
    pub cache: Option<Option<PathBuf>>,
    /// `editor.formatter` (brief 0050): `auto`, `prettier`, `biome` or `server`.
    pub formatter: String,
    /// `editor.formatOnSave.<registration id>`, by the registration of a document's primary server.
    pub format_on_save: BTreeMap<String, bool>,
    /// `editor.emmet`.
    pub emmet: bool,
}

impl Default for ServerLaunches {
    fn default() -> Self {
        Self {
            registry: ServerRegistry::builtin(),
            in_process: BTreeMap::new(),
            rust_analyzer: None,
            settings: BTreeMap::new(),
            node: None,
            cache: None,
            formatter: "auto".into(),
            format_on_save: BTreeMap::new(),
            emmet: true,
        }
    }
}

impl ServerLaunches {
    /// The web servers' cache folder, looked for once (a directory check).
    pub fn cache(&mut self) -> Option<PathBuf> {
        if self.cache.is_none() {
            self.cache = Some(self.registry.web_servers_cache(
                &|k| std::env::var_os(k).filter(|v| !v.is_empty()),
                eludite_lsp::node::home_dir().as_deref(),
            ));
        }
        self.cache.clone().flatten()
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
    /// The modules it runs on, as located (`TypeScript 5.9.3, project`).
    pub modules: Vec<String>,
    /// Its own diagnostics, by URI (a document's servers' lists are shown together).
    pub diagnostics: HashMap<String, Vec<eludite_lsp::lsp::Diagnostic>>,
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
            Some(LanguageServerState::Unavailable)
                if self
                    .message
                    .as_deref()
                    .is_some_and(|m| m.starts_with(super::session::NOT_FOUND)) =>
            {
                format!("{name}: {}", self.message.as_deref().unwrap_or_default())
            }
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
                    _ => {
                        let mut about: Vec<&str> = self.version.as_deref().into_iter().collect();
                        about.extend(self.modules.iter().map(String::as_str));
                        if about.is_empty() {
                            format!("{name}: ready")
                        } else {
                            format!("{name}: ready ({})", about.join("; "))
                        }
                    }
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
    /// The servers for a file at `path`: their keys (the primary first), the document's session (fanned out when
    /// there are several) and its LSP language id; `None` when no registration matches. Starts a generic session (not
    /// yet the server) for each new registration and root; a registration that is not activated for the root
    /// (ESLint without a configuration) is left out.
    pub(super) fn server_for_path(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(Vec<ServerKey>, ServerSession, String)> {
        let regs: Vec<ServerRegistration> = self
            .launches
            .registry
            .all_for_path(path)
            .into_iter()
            .cloned()
            .collect();
        let primary = regs.first()?;
        let language_id = primary.language_id_for(path).to_owned();
        if primary.via == Via::EluditeHost {
            return Some((vec![ServerKey::Host], self.session.clone(), language_id));
        }
        let mut keys = Vec::new();
        let mut members = Vec::new();
        for (i, reg) in regs.iter().enumerate() {
            if reg.via != Via::Process {
                continue;
            }
            let root = self.generic_root(reg, path);
            let setting = reg
                .activation
                .as_ref()
                .and_then(|a| self.launches.settings.get(&a.setting))
                .map(String::as_str);
            // The primary server always runs; another one only when activated.
            if i > 0 && !reg.activated(&root, setting) {
                continue;
            }
            let key = format!("{}|{}", reg.id, root.to_string_lossy());
            if !self.generic.contains_key(&key) {
                self.start_generic(key.clone(), reg.clone(), root, window, cx);
            }
            members.push((reg.name.clone(), self.generic[&key].session.clone()));
            keys.push(ServerKey::Generic(key));
        }
        Some((keys, ServerSession::fan_out(members), language_id))
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
        let configured = (registration.id == "rust-analyzer")
            .then(|| self.launches.rust_analyzer.clone())
            .flatten();
        let npm = registration
            .command
            .as_ref()
            .is_some_and(|c| c.npm_package.is_some());
        let cache = if npm { self.launches.cache() } else { None };
        let modules = registration
            .modules
            .iter()
            .filter_map(|m| {
                let value = self.launches.settings.get(m.setting.as_ref()?)?;
                (!value.is_empty()).then(|| (m.name.clone(), PathBuf::from(value)))
            })
            .collect();
        let (session, mut events) = ServerSession::spawn_generic(GenericLaunch {
            registration: registration.clone(),
            root: root.clone(),
            connector,
            configured,
            cache,
            node: self.launches.node.clone(),
            modules,
            fetch: self
                .launches
                .registry
                .fetch_command()
                .filter(|_| npm)
                .map(str::to_owned),
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
                modules: Vec::new(),
                diagnostics: HashMap::new(),
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
            SessionEvent::ServerModules(modules) => {
                if let Some(server) = self.generic.get_mut(key) {
                    server.modules = modules;
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
                let params = self.merge_server_diagnostics(key, params);
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
        if let Some(server) = self.generic.get_mut(key) {
            server.diagnostics.clear();
        }
        let docs: Vec<(String, usize)> = self
            .documents
            .values()
            .filter(|d| d.servers.contains(&generic))
            .map(|d| (d.uri.clone(), d.servers.len()))
            .collect();
        for doc in self
            .documents
            .values_mut()
            .filter(|d| d.servers.contains(&generic))
        {
            doc.intellisense.cancel_all();
        }
        for (uri, servers) in docs {
            if servers > 1 {
                // The document's other servers' diagnostics stay (brief 0050): show what they have.
                let merged = self.merge_server_diagnostics(
                    key,
                    eludite_lsp::lsp::PublishDiagnosticsParams {
                        uri,
                        version: None,
                        diagnostics: Vec::new(),
                    },
                );
                self.on_diagnostics(merged, cx);
            } else {
                if let Some(doc) = self.documents.values().find(|d| d.uri == uri) {
                    doc.clear_diagnostics(cx);
                }
                self.diagnostics.remove(&uri);
            }
        }
        self.update_error_list(cx);
    }

    /// The generation of the servers document `id` belongs to (the solution generation for the host; the sum of its
    /// servers' for a document several serve).
    pub(super) fn doc_generation(&self, id: &str) -> u64 {
        self.key_generation(Some(&self.doc_key(id)))
    }

    /// The key edits computed for document `id` are checked against: its server's, or the group of its servers.
    pub(super) fn doc_key(&self, id: &str) -> ServerKey {
        match self.documents.get(id) {
            Some(d) if d.servers.len() > 1 => ServerKey::Group(
                d.servers
                    .iter()
                    .filter_map(|k| match k {
                        ServerKey::Generic(k) => Some(k.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            Some(d) => d.server.clone(),
            None => ServerKey::Host,
        }
    }

    /// The generation of server `key` (the sum of a group's).
    pub(super) fn key_generation(&self, key: Option<&ServerKey>) -> u64 {
        match key {
            Some(ServerKey::Generic(k)) => self.generic.get(k).map_or(0, |g| g.generation),
            Some(ServerKey::Group(keys)) => keys
                .iter()
                .map(|k| self.generic.get(k).map_or(0, |g| g.generation))
                .sum(),
            _ => self.generation,
        }
    }

    /// What the servers of document `id` support (for several: what any of them supports).
    pub(super) fn doc_features(&self, id: &str) -> ServerFeatures {
        let Some(doc) = self.documents.get(id) else {
            return self.features.clone();
        };
        let mut features = doc.servers.iter().filter_map(|k| match k {
            ServerKey::Generic(k) => self.generic.get(k).map(|g| g.features.clone()),
            _ => None,
        });
        match &doc.server {
            ServerKey::Generic(_) => {
                let first = features.next().unwrap_or_default();
                features.fold(first, ServerFeatures::union)
            }
            _ => self.features.clone(),
        }
    }

    /// Server `key`'s diagnostics for `params.uri` replace its earlier ones; the answer is every server's list for
    /// that document together, in the document's server order (TypeScript's, then ESLint's).
    fn merge_server_diagnostics(
        &mut self,
        key: &str,
        params: eludite_lsp::lsp::PublishDiagnosticsParams,
    ) -> eludite_lsp::lsp::PublishDiagnosticsParams {
        if let Some(server) = self.generic.get_mut(key) {
            if params.diagnostics.is_empty() {
                server.diagnostics.remove(&params.uri);
            } else {
                server
                    .diagnostics
                    .insert(params.uri.clone(), params.diagnostics.clone());
            }
        }
        let order: Vec<String> = match self.documents.values().find(|d| d.uri == params.uri) {
            Some(d) => d
                .servers
                .iter()
                .filter_map(|k| match k {
                    ServerKey::Generic(k) => Some(k.clone()),
                    _ => None,
                })
                .collect(),
            None => vec![key.to_owned()],
        };
        if order.len() < 2 {
            return params;
        }
        let diagnostics = order
            .iter()
            .filter_map(|k| self.generic.get(k)?.diagnostics.get(&params.uri))
            .flatten()
            .cloned()
            .collect();
        eludite_lsp::lsp::PublishDiagnosticsParams {
            diagnostics,
            ..params
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
            modules: Vec::new(),
            diagnostics: HashMap::new(),
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
