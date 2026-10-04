//! The generic language-server client (brief 0019): plain LSP 3.17 to a server the shell launches directly
//! (rust-analyzer), over [`Connection`], the code it shares with the host bridge. See host-rpc.md, "Generic language
//! servers and Cargo".
//!
//! The dialect: the LSP `initialize` / `initialized` handshake with the client capabilities the host advertises to
//! Roslyn (so the editor features need no per-server branch) plus work-done progress and rust-analyzer's
//! `experimental/serverStatus`; no `eluditeGeneration` on the wire, but every result pinned to the client's own
//! generation, which moves on every (re)start; `$/progress` and `experimental/serverStatus` as events;
//! `workspace/configuration` answered from the registration's settings; LSP `shutdown` and `exit`. Diagnostics are
//! delivered as the host delivers them, one merged list per document: pushed ones as they come, and, for a server
//! that advertises `diagnosticProvider` (the pinned rust-analyzer computes its native diagnostics only on pull),
//! pulled ones on open, after edits and when the server becomes quiescent (see `pull`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use eludite_protocol::host::{Generation, methods};
use eludite_protocol::lsp::{ApplyWorkspaceEditResult, PublishDiagnosticsParams};
use eludite_protocol::{ErrorObject, Id, Notification, NotificationType, Request, RequestType};
use serde_json::{Value, json};

use std::sync::Arc;

use crate::connection::{
    ClientInfo, Connection, Connector, Dialect, Error, Event, Launch, PendingRequest, Progress,
    RestartPolicy, ServerCommand, ServerStatus,
};
use crate::pull::{self, Diagnostics};

/// How long the client waits for the server's `initialize` reply. rust-analyzer answers before it loads the
/// workspace, so this is generous.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// LSP method names the generic client speaks beyond the shared typed set.
pub mod methods_generic {
    pub const INITIALIZE: &str = "initialize";
    pub const INITIALIZED: &str = "initialized";
    pub const SHUTDOWN: &str = "shutdown";
    pub const EXIT: &str = "exit";
    pub const PROGRESS: &str = "$/progress";
    pub const SERVER_STATUS: &str = "experimental/serverStatus";
    pub const LOG_MESSAGE: &str = "window/logMessage";
    pub const SHOW_MESSAGE: &str = "window/showMessage";
    pub const CONFIGURATION: &str = "workspace/configuration";
    /// Sent after `initialized` to a registration with `pushSettings` (brief 0050): its settings.
    pub const DID_CHANGE_CONFIGURATION: &str = "workspace/didChangeConfiguration";
    /// Answered with the one workspace folder (the root).
    pub const WORKSPACE_FOLDERS: &str = "workspace/workspaceFolders";
    /// Answered `null`; every open document is pulled again.
    pub const DIAGNOSTIC_REFRESH: &str = "workspace/diagnostic/refresh";
    /// Answered `null`; the lens generation moves and the shell asks for its documents' lenses again (brief 0052).
    pub const CODE_LENS_REFRESH: &str = "workspace/codeLens/refresh";
    /// The client commands rust-analyzer needs listed in `experimental.commands.commands` before it offers its run,
    /// debug, implementations and references lenses (brief 0052).
    pub const LENS_CLIENT_COMMANDS: &[&str] = &[
        "rust-analyzer.runSingle",
        "rust-analyzer.debugSingle",
        "rust-analyzer.showReferences",
    ];
    /// Server-to-client requests answered `null`.
    pub const ANSWERED_NULL: &[&str] = &[
        "window/workDoneProgress/create",
        "client/registerCapability",
        "client/unregisterCapability",
        "workspace/semanticTokens/refresh",
        "workspace/inlayHint/refresh",
    ];
}

/// What the generic client needs to start one server for one workspace.
#[derive(Debug, Clone)]
pub struct ServerSetup {
    /// The registration's id (`rust-analyzer`), for thread names and logs.
    pub name: String,
    pub client: ClientInfo,
    /// The workspace root: `rootUri` and the one workspace folder.
    pub root: PathBuf,
    pub initialization_options: Value,
    /// Answers to `workspace/configuration`, by section (`{"rust-analyzer": {...}}`).
    pub settings: Value,
    /// Also send the settings with `workspace/didChangeConfiguration` after `initialized` (brief 0050: the HTML, CSS
    /// and JSON servers read them only from it).
    pub push_settings: bool,
}

/// The LSP client capabilities the generic client sends: the set `eludite-host` advertises to Roslyn
/// (host-rpc.md, "LSP client capabilities the host advertises"), plus `workspace.diagnostics.refreshSupport` and
/// `experimental.serverStatusNotification` for rust-analyzer's status.
pub fn client_capabilities() -> Value {
    json!({
        "workspace": {
            "configuration": true,
            "workspaceFolders": true,
            "didChangeWatchedFiles": {"dynamicRegistration": false},
            "applyEdit": true,
            "diagnostics": {"refreshSupport": true},
            "codeLens": {"refreshSupport": true},
            "workspaceEdit": {
                "documentChanges": true,
                "resourceOperations": ["create", "rename", "delete"],
                "failureHandling": "abort"
            }
        },
        "textDocument": {
            "synchronization": {"didSave": true},
            "completion": {
                "contextSupport": true,
                "completionItem": {
                    "snippetSupport": true,
                    "insertReplaceSupport": true,
                    "labelDetailsSupport": true,
                    "resolveSupport": {"properties": ["documentation", "detail", "additionalTextEdits"]}
                },
                "completionList": {"itemDefaults": ["commitCharacters", "editRange", "insertTextFormat", "data"]}
            },
            "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
            "hover": {"contentFormat": ["markdown", "plaintext"]},
            "signatureHelp": {},
            "definition": {},
            "references": {},
            "codeAction": {
                "codeActionLiteralSupport": {
                    "codeActionKind": {
                        "valueSet": ["quickfix", "refactor", "refactor.extract", "refactor.inline",
                                     "refactor.rewrite", "source", "source.organizeImports"]
                    }
                },
                "resolveSupport": {"properties": ["edit"]},
                "dataSupport": true,
                "isPreferredSupport": true,
                "disabledSupport": true
            },
            "rename": {"prepareSupport": true},
            "codeLens": {},
            "publishDiagnostics": {},
            "diagnostic": {"dynamicRegistration": false}
        },
        "window": {"workDoneProgress": true},
        "experimental": {
            "serverStatusNotification": true,
            "commands": {"commands": methods_generic::LENS_CLIENT_COMMANDS}
        }
    })
}

/// `file:///abs/path` with reserved and non-ASCII bytes percent-encoded (Windows paths as `file:///C:/...`).
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

struct ServerDialect {
    setup: ServerSetup,
    diagnostics: Arc<Diagnostics>,
}

impl ServerDialect {
    /// The one workspace folder: the root.
    fn folder(&self) -> Value {
        let uri = path_to_uri(&self.setup.root);
        let name = self
            .setup
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| uri.clone());
        json!({"uri": uri, "name": name})
    }

    fn initialize_params(&self) -> Value {
        let uri = path_to_uri(&self.setup.root);
        let mut params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": self.setup.client.name, "version": self.setup.client.version},
            "rootUri": uri,
            "rootPath": self.setup.root.to_string_lossy(),
            "workspaceFolders": [self.folder()],
            "capabilities": client_capabilities(),
            "trace": "off"
        });
        if !self.setup.initialization_options.is_null() {
            params["initializationOptions"] = self.setup.initialization_options.clone();
        }
        params
    }
}

impl Dialect for ServerDialect {
    fn name(&self) -> &str {
        &self.setup.name
    }

    /// Each (re)start is a new generation: results from a previous process are never delivered.
    fn initial_generation(&self, epoch: u64) -> Generation {
        self.diagnostics.reset();
        epoch
    }

    fn handshake(&self, conn: &Connection) -> Result<(), Error> {
        let result = conn
            .request_untyped(methods_generic::INITIALIZE, self.initialize_params())?
            .wait_timeout(INITIALIZE_TIMEOUT)?;
        let pulls = pull::advertises_pull(result.get("capabilities"));
        conn.set_initialize_result(result);
        conn.notify_untyped(methods_generic::INITIALIZED, json!({}))?;
        if self.setup.push_settings {
            conn.notify_untyped(
                methods_generic::DID_CHANGE_CONFIGURATION,
                json!({"settings": self.setup.settings}),
            )?;
        }
        if pulls {
            self.diagnostics.enable_pull(conn);
        }
        Ok(())
    }

    fn injects_generation(&self, _method: &str, _typed_generational: Option<bool>) -> bool {
        false
    }

    fn pins_result(&self, method: &str, _injected: bool) -> bool {
        method != methods_generic::SHUTDOWN && method != methods_generic::INITIALIZE
    }

    fn notification(&self, conn: &Connection, n: Notification) -> Option<Event> {
        let params = n.params.clone().unwrap_or(Value::Null);
        Some(match n.method.as_str() {
            methods::PUBLISH_DIAGNOSTICS => {
                match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                    Ok(params) => self.diagnostics.on_pushed(conn, params),
                    Err(_) => Event::Notification(n),
                }
            }
            methods_generic::PROGRESS => match Progress::from_params(&params) {
                Some(p) => Event::Progress(p),
                None => Event::Notification(n),
            },
            methods_generic::SERVER_STATUS => match ServerStatus::from_params(&params) {
                Some(s) => {
                    // Native diagnostics computed before the workspace loaded are incomplete.
                    if s.quiescent {
                        self.diagnostics.pull_all();
                    }
                    Event::ServerStatus(s)
                }
                None => Event::Notification(n),
            },
            methods_generic::LOG_MESSAGE => Event::Log(
                params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ),
            _ => Event::Notification(n),
        })
    }

    fn request(&self, conn: &Connection, r: &Request) -> Result<Value, ErrorObject> {
        if r.method == methods_generic::CONFIGURATION {
            let items = r
                .params
                .as_ref()
                .and_then(|p| p.get("items"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            return Ok(Value::Array(
                items
                    .iter()
                    .map(|item| configuration(&self.setup.settings, item))
                    .collect(),
            ));
        }
        if r.method == methods_generic::WORKSPACE_FOLDERS {
            return Ok(json!([self.folder()]));
        }
        if r.method == methods_generic::DIAGNOSTIC_REFRESH {
            self.diagnostics.pull_all();
            return Ok(Value::Null);
        }
        if r.method == methods_generic::CODE_LENS_REFRESH {
            conn.code_lens_refresh(conn.generation());
            return Ok(Value::Null);
        }
        if methods_generic::ANSWERED_NULL.contains(&r.method.as_str()) {
            return Ok(Value::Null);
        }
        Err(ErrorObject::new(
            ErrorObject::METHOD_NOT_FOUND,
            format!("{} is not supported by the Eludite shell", r.method),
        ))
    }

    fn apply_edit_has_generation(&self) -> bool {
        false
    }

    fn on_sent(&self, conn: &Connection, method: &str, params: &Value) {
        self.diagnostics.on_sent(conn, method, params);
    }

    fn shutdown(&self, conn: &Connection, timeout: Duration) -> Result<(), Error> {
        self.diagnostics.stop();
        conn.request_untyped(methods_generic::SHUTDOWN, Value::Null)?
            .wait_timeout(timeout)?;
        conn.notify_untyped(methods_generic::EXIT, Value::Null)
    }
}

/// The answer to one `workspace/configuration` item: the settings at its dotted `section`, `null` when absent; the
/// whole settings without a section or with an empty one (ESLint asks for `""`).
pub fn configuration(settings: &Value, item: &Value) -> Value {
    let Some(section) = item
        .get("section")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return settings.clone();
    };
    let mut at = settings;
    for part in section.split('.') {
        match at.get(part) {
            Some(v) => at = v,
            None => return Value::Null,
        }
    }
    at.clone()
}

/// A running generic language server and the connection to it. Cheap to clone; thread-safe; nothing blocks on the
/// server but [`PendingRequest`]'s waits and [`ServerClient::shutdown`].
#[derive(Clone)]
pub struct ServerClient {
    conn: Connection,
}

impl std::fmt::Debug for ServerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerClient")
            .field("connection", &self.conn)
            .finish()
    }
}

impl ServerClient {
    /// Spawns the server, performs `initialize` / `initialized` and returns. Events arrive on the receiver.
    pub fn start(
        command: ServerCommand,
        setup: ServerSetup,
        restart: RestartPolicy,
    ) -> Result<(ServerClient, Receiver<Event>), Error> {
        Self::launch(Launch::Process(command), setup, restart)
    }

    /// As [`ServerClient::start`], with a server running in this process behind `connector` (the fake server in
    /// tests).
    pub fn start_in_process(
        connector: Connector,
        setup: ServerSetup,
        restart: RestartPolicy,
    ) -> Result<(ServerClient, Receiver<Event>), Error> {
        Self::launch(Launch::InProcess(connector), setup, restart)
    }

    fn launch(
        launch: Launch,
        setup: ServerSetup,
        restart: RestartPolicy,
    ) -> Result<(ServerClient, Receiver<Event>), Error> {
        let dialect = ServerDialect {
            setup,
            diagnostics: Arc::default(),
        };
        let (conn, rx) = Connection::launch(launch, Box::new(dialect), restart)?;
        Ok((ServerClient { conn }, rx))
    }

    /// The connection, for the editor features' document and request traffic.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The server's `initialize` result.
    pub fn initialize_result(&self) -> Option<Value> {
        self.conn.initialize_result_value()
    }

    /// The server's `ServerCapabilities`.
    pub fn capabilities(&self) -> Option<Value> {
        self.initialize_result()
            .and_then(|r| r.get("capabilities").cloned())
    }

    /// `serverInfo.name` and `serverInfo.version`.
    pub fn server_info(&self) -> Option<(String, Option<String>)> {
        let r = self.initialize_result()?;
        let info = r.get("serverInfo")?;
        Some((
            info.get("name")?.as_str()?.to_owned(),
            info.get("version")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ))
    }

    pub fn pid(&self) -> Option<u32> {
        self.conn.pid()
    }

    /// The client's generation: 1 for the first process, one more for each restart.
    pub fn generation(&self) -> Generation {
        self.conn.generation()
    }

    pub fn request<R: RequestType>(
        &self,
        params: R::Params,
    ) -> Result<PendingRequest<R::Result>, Error> {
        self.conn.request::<R>(params)
    }

    pub fn request_untyped(
        &self,
        method: &str,
        params: Value,
    ) -> Result<PendingRequest<Value>, Error> {
        self.conn.request_untyped(method, params)
    }

    pub fn notify<N: NotificationType>(&self, params: N::Params) -> Result<(), Error> {
        self.conn.notify::<N>(params)
    }

    pub fn notify_untyped(&self, method: &str, params: Value) -> Result<(), Error> {
        self.conn.notify_untyped(method, params)
    }

    pub fn respond_apply_edit(
        &self,
        id: Id,
        result: ApplyWorkspaceEditResult,
    ) -> Result<(), Error> {
        self.conn.respond_apply_edit(id, result)
    }

    /// LSP `shutdown`, `exit`, then waits for the process (killing it after `timeout`). Disables restarts.
    pub fn shutdown(&self, timeout: Duration) -> Result<Option<i32>, Error> {
        self.conn.shutdown(timeout)
    }

    /// Kills the server without a shutdown; it restarts under the policy.
    pub fn kill(&self) -> std::io::Result<()> {
        self.conn.kill()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_answers_by_dotted_section() {
        let settings = json!({"rust-analyzer": {"cargo": {"targetDir": true}}});
        assert_eq!(
            configuration(&settings, &json!({"section": "rust-analyzer"})),
            json!({"cargo": {"targetDir": true}})
        );
        assert_eq!(
            configuration(
                &settings,
                &json!({"section": "rust-analyzer.cargo.targetDir"})
            ),
            json!(true)
        );
        assert_eq!(
            configuration(&settings, &json!({"section": "files"})),
            Value::Null
        );
        assert_eq!(configuration(&settings, &json!({})), settings);
        assert_eq!(configuration(&settings, &json!({"section": ""})), settings);
    }

    #[test]
    fn capabilities_match_the_hosts_plus_status_and_refresh() {
        let caps = client_capabilities();
        assert_eq!(caps["experimental"]["serverStatusNotification"], true);
        assert_eq!(caps["window"]["workDoneProgress"], true);
        assert_eq!(caps["workspace"]["diagnostics"]["refreshSupport"], true);
        assert!(caps["textDocument"].get("diagnostic").is_some());
        assert_eq!(caps["textDocument"]["rename"]["prepareSupport"], true);
    }

    /// Every method the generic client sends or answers is documented in host-rpc.md's generic-client section.
    #[test]
    fn host_rpc_md_documents_the_generic_methods() {
        let md = include_str!("../../../protocol/schemas/host-rpc.md");
        let section = &md[md
            .find("## Generic language servers and Cargo")
            .expect("generic-client section")..];
        use methods_generic::*;
        let mut all = vec![
            INITIALIZE,
            INITIALIZED,
            SHUTDOWN,
            EXIT,
            PROGRESS,
            SERVER_STATUS,
            LOG_MESSAGE,
            SHOW_MESSAGE,
            CONFIGURATION,
            DID_CHANGE_CONFIGURATION,
            WORKSPACE_FOLDERS,
            DIAGNOSTIC_REFRESH,
            CODE_LENS_REFRESH,
            "textDocument/codeLens",
            "codeLens/resolve",
            "textDocument/diagnostic",
            "workspace/applyEdit",
            "textDocument/publishDiagnostics",
        ];
        all.extend(ANSWERED_NULL);
        for m in all {
            assert!(
                section.contains(&format!("`{m}`")),
                "{m} is not in host-rpc.md"
            );
        }
    }

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(
            path_to_uri(Path::new("/a b/c.rs")),
            "file:///a%20b/c.rs".to_owned()
        );
    }
}
