//! The shell's client of `eludite-host`: the host dialect of [`Connection`] (the `eludite/host/*` handshake,
//! `eluditeGeneration` on forwarded requests, solution and build notifications) and the solution lifecycle
//! (protocol/schemas/host-rpc.md).

use std::sync::mpsc::Receiver;
use std::time::Duration;

use eludite_protocol::host::{
    self, BuildFinished, BuildOutput, BuildProgress, Generation, GenerationResult,
    InitializeParams, InitializeResult, LanguageServerStatus, SolutionOpenParams, SolutionStatus,
    WithGeneration, methods,
};
use eludite_protocol::lsp::{ApplyWorkspaceEditResult, PublishDiagnosticsParams};
use eludite_protocol::{ErrorObject, Id, Notification, NotificationType, Request, RequestType};
use serde_json::Value;

use crate::connection::{
    ClientInfo, Connection, Connector, Dialect, Error, Event, HostCommand, Launch, PendingRequest,
    RestartPolicy, typed_or_untyped,
};

/// How long [`HostClient::start`] waits for the `eludite/host/initialize` reply.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);

struct HostDialect {
    client: ClientInfo,
}

impl Dialect for HostDialect {
    fn name(&self) -> &str {
        "eludite-host"
    }

    /// The generation is 0 after `eludite/host/initialize` (host-rpc.md, "Solution generation").
    fn initial_generation(&self, _epoch: u64) -> Generation {
        0
    }

    fn handshake(&self, conn: &Connection) -> Result<(), Error> {
        let init = conn
            .request::<host::HostInitialize>(InitializeParams {
                client_name: self.client.name.clone(),
                client_version: self.client.version.clone(),
            })?
            .wait_timeout(INITIALIZE_TIMEOUT)?;
        conn.set_initialize_result(serde_json::to_value(init)?);
        Ok(())
    }

    fn injects_generation(&self, method: &str, typed_generational: Option<bool>) -> bool {
        typed_generational.unwrap_or_else(|| {
            methods::FORWARDED_TYPED_REQUESTS.contains(&method)
                || methods::FORWARDED_UNTYPED_REQUESTS.contains(&method)
        })
    }

    fn pins_result(&self, _method: &str, injected: bool) -> bool {
        injected
    }

    fn notification(&self, conn: &Connection, n: Notification) -> Option<Event> {
        Some(match n.method.as_str() {
            methods::SOLUTION_STATUS => typed_or_untyped::<SolutionStatus>(n, |status| {
                conn.observe_generation(status.generation);
                Event::SolutionStatus(status)
            }),
            methods::LANGUAGE_SERVER_STATUS => {
                typed_or_untyped::<LanguageServerStatus>(n, Event::LanguageServerStatus)
            }
            methods::PUBLISH_DIAGNOSTICS => {
                let event = typed_or_untyped::<WithGeneration<PublishDiagnosticsParams>>(
                    n,
                    Event::Diagnostics,
                );
                // Diagnostics computed under an older generation are not rendered (CLAUDE.md invariant 12).
                if let Event::Diagnostics(d) = &event
                    && d.generation < conn.generation()
                {
                    return None;
                }
                event
            }
            methods::BUILD_OUTPUT => typed_or_untyped::<BuildOutput>(n, Event::BuildOutput),
            methods::BUILD_PROGRESS => typed_or_untyped::<BuildProgress>(n, Event::BuildProgress),
            methods::BUILD_FINISHED => typed_or_untyped::<BuildFinished>(n, Event::BuildFinished),
            methods::TEST_UPDATE => {
                typed_or_untyped::<host::TestUpdate>(n, |u| Event::TestUpdate(Box::new(u)))
            }
            _ => Event::Notification(n),
        })
    }

    /// `workspace/applyEdit` is the host's only request to the shell; anything else is MethodNotFound
    /// (host-rpc.md, "Messages the host sends").
    fn request(&self, _conn: &Connection, r: &Request) -> Result<Value, ErrorObject> {
        Err(ErrorObject::new(
            ErrorObject::METHOD_NOT_FOUND,
            format!("{} is not a shell method", r.method),
        ))
    }

    fn apply_edit_has_generation(&self) -> bool {
        true
    }

    fn shutdown(&self, conn: &Connection, timeout: Duration) -> Result<(), Error> {
        conn.request::<host::HostShutdown>(())?
            .wait_timeout(timeout)?;
        conn.notify::<host::HostExit>(())
    }
}

/// A running `eludite-host` and the connection to it. Cheap to clone; all methods are thread-safe and never block on
/// the host except the `wait` calls of [`PendingRequest`]. The document and request traffic is [`Connection`]'s,
/// shared with the generic client ([`crate::ServerClient`]); see [`HostClient::connection`].
#[derive(Clone)]
pub struct HostClient {
    conn: Connection,
}

impl std::fmt::Debug for HostClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostClient")
            .field("pid", &self.pid())
            .field("generation", &self.generation())
            .finish()
    }
}

impl HostClient {
    /// Spawns the host, sends `eludite/host/initialize` and waits for its reply. Host events arrive on the returned
    /// receiver.
    pub fn start(
        command: HostCommand,
        client: ClientInfo,
        restart: RestartPolicy,
    ) -> Result<(HostClient, Receiver<Event>), Error> {
        Self::launch(Launch::Process(command), client, restart)
    }

    /// As [`HostClient::start`], with a host running in this process behind `connector` (the fake host in tests).
    /// [`HostClient::pid`] is `None` and [`HostClient::kill`] closes the host's input.
    pub fn start_in_process(
        connector: Connector,
        client: ClientInfo,
        restart: RestartPolicy,
    ) -> Result<(HostClient, Receiver<Event>), Error> {
        Self::launch(Launch::InProcess(connector), client, restart)
    }

    fn launch(
        launch: Launch,
        client: ClientInfo,
        restart: RestartPolicy,
    ) -> Result<(HostClient, Receiver<Event>), Error> {
        let (conn, rx) = Connection::launch(launch, Box::new(HostDialect { client }), restart)?;
        Ok((HostClient { conn }, rx))
    }

    /// The connection, for the editor features' document and request traffic.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The host's `eludite/host/initialize` result.
    pub fn initialize_result(&self) -> Option<InitializeResult> {
        self.conn
            .initialize_result_value()
            .and_then(|v| serde_json::from_value(v).ok())
    }

    /// The host process id, while it runs (`None` for an in-process host).
    pub fn pid(&self) -> Option<u32> {
        self.conn.pid()
    }

    /// The current solution generation as this client knows it.
    pub fn generation(&self) -> Generation {
        self.conn.generation()
    }

    /// Sends a typed request. Forwarded LSP requests ([`RequestType::GENERATIONAL`]) are pinned to the current
    /// generation: `eluditeGeneration` is added to their params and a result that arrives after the generation moved
    /// is dropped ([`Error::Stale`]).
    pub fn request<R: RequestType>(
        &self,
        params: R::Params,
    ) -> Result<PendingRequest<R::Result>, Error> {
        self.conn.request::<R>(params)
    }

    /// Sends a request with raw JSON params ("forwarded, untyped" methods). Forwarded methods are pinned to the
    /// current generation like typed ones.
    pub fn request_untyped(
        &self,
        method: &str,
        params: Value,
    ) -> Result<PendingRequest<Value>, Error> {
        self.conn.request_untyped(method, params)
    }

    /// Sends a typed notification.
    pub fn notify<N: NotificationType>(&self, params: N::Params) -> Result<(), Error> {
        self.conn.notify::<N>(params)
    }

    /// Sends a notification with raw JSON params.
    pub fn notify_untyped(&self, method: &str, params: Value) -> Result<(), Error> {
        self.conn.notify_untyped(method, params)
    }

    /// `eludite/solution/open`: returns the new generation, which becomes current at once.
    pub fn open_solution(&self, path: &str, timeout: Duration) -> Result<Generation, Error> {
        let r: GenerationResult = self
            .request::<host::SolutionOpen>(SolutionOpenParams { path: path.into() })?
            .wait_timeout(timeout)?;
        self.conn.observe_generation(r.generation);
        Ok(r.generation)
    }

    /// `eludite/solution/close`: returns the current generation after the close.
    pub fn close_solution(&self, timeout: Duration) -> Result<Generation, Error> {
        let r: GenerationResult = self
            .request::<host::SolutionClose>(())?
            .wait_timeout(timeout)?;
        self.conn.observe_generation(r.generation);
        Ok(r.generation)
    }

    /// `eludite/host/shutdown`, `eludite/host/exit`, then waits for the process to exit (killing it after `timeout`).
    /// Disables restarts. Returns the exit code.
    pub fn shutdown(&self, timeout: Duration) -> Result<Option<i32>, Error> {
        self.conn.shutdown(timeout)
    }

    /// Kills the host without a shutdown. Restarts according to the policy unless [`HostClient::shutdown`] ran.
    pub fn kill(&self) -> std::io::Result<()> {
        self.conn.kill()
    }

    /// Answer the host's `workspace/applyEdit` request `id` ([`Event::ApplyEdit`]).
    pub fn respond_apply_edit(
        &self,
        id: Id,
        result: ApplyWorkspaceEditResult,
    ) -> Result<(), Error> {
        self.conn.respond_apply_edit(id, result)
    }
}
