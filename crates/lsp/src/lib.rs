//! The shell's language-server clients (PLAN.md D2, D3, 4.3; contract: `protocol/schemas/host-rpc.md`).
//!
//! Two peers share one connection core, [`Connection`]:
//!
//! - **`eludite-host`** ([`HostClient`]): the `eludite/*` vocabulary plus forwarded LSP 3.17, with the solution
//!   generation (`eluditeGeneration`) on forwarded requests, solution and build notifications.
//! - **Generic language servers** ([`ServerClient`], brief 0019): plain LSP 3.17 to a server the shell launches
//!   directly (rust-analyzer, the web servers, FsAutoComplete for F#), with the `initialize` handshake, work-done
//!   progress and `experimental/serverStatus` as events, notifications the client does not know passed up as
//!   [`Event::Notification`] (the shell ignores them), and `workspace/configuration` answered from the server's
//!   registration. Which server handles which files, and how it is found and started, is data: [`registry`]
//!   (`servers.json`): beside `eludite`, the `ELUDITE_<SERVER>` variable, then `PATH` and the rustup component
//!   (rust-analyzer); the project's `node_modules`, the variable, the web servers' cache and `PATH` (the npm servers,
//!   brief 0050); beside `eludite`, the variable, the pinned cache, `~/.dotnet/tools` and `PATH` for a .NET tool,
//!   spawned with `DOTNET_ROOT` when it needs it (FsAutoComplete, brief 0057). `*.vb` goes to the host as `vb`.
//!
//! The shared core owns, for both:
//!
//! - **Process supervision**: spawning from a [`HostCommand`], stderr kept out of the protocol stream, exits as
//!   [`HostEvent`]s and restarts under a [`RestartPolicy`].
//! - **Requests**: typed with [`eludite_protocol::RequestType`] marker types, correlated by id, awaited off the UI
//!   thread through [`PendingRequest`].
//! - **Cancellation**: [`PendingRequest::cancel`] sends `$/cancelRequest`; the wait ends with [`Error::Canceled`]
//!   and any result is discarded.
//! - **Generations**: results that arrive after the generation moved (a new solution in the host, a restarted
//!   generic server) are dropped ([`Error::Stale`]), and so are older diagnostics.
//! - **Document notifications**: `didOpen`, `didChange`, `didSave`, `didClose`.
//! - **Events**: everything the server sends unasked arrives on a channel as [`Event`]s; `workspace/applyEdit` as
//!   [`Event::ApplyEdit`], answered with `respond_apply_edit`.
//!
//! Several servers may serve one document (brief 0050: TypeScript and ESLint): [`fanout`] decides which of them a
//! request goes to and merges their answers; [`node`] finds the Node.js the web servers (and vscode-js-debug, through
//! `eludite-dap`) run on.
//!
//! Public API boundary: [`Connection`], [`HostClient`], [`ServerClient`], [`ServerSetup`], [`PendingRequest`],
//! [`HostCommand`] (alias [`ServerCommand`]), [`Connector`] (a server running in this process), [`ClientInfo`],
//! [`RestartPolicy`], [`StderrMode`], [`Event`], [`HostEvent`], [`Progress`], [`ServerStatus`], [`Error`], the
//! [`registry`] types, [`fanout`], [`node`], [`codelens`] (what a lens's command does, brief 0052), plus the low-level
//! [`Transport`] / [`FramedTransport`]. Message types live in `eludite-protocol`. With the `fake` feature,
//! [`fake::FakeHost`] is a scripted in-process host and [`fake_server::FakeServer`] a scripted in-process generic
//! server, for tests.
//!
//! CodeLens (brief 0052): a generic server's `workspace/codeLens/refresh` and the host's `eludite/codeLens/refresh`
//! both arrive as [`Event::CodeLensRefresh`] and move the connection's lens generation
//! ([`Connection::code_lens_generation`]).

mod client;
pub mod codelens;
mod connection;
#[cfg(feature = "fake")]
pub mod fake;
#[cfg(feature = "fake")]
mod fake_nuget;
#[cfg(feature = "fake")]
pub mod fake_server;
pub mod fanout;
pub mod node;
mod pull;
pub mod registry;
mod server;
mod transport;

pub use client::HostClient;
pub use connection::{
    ClientInfo, Connection, Connector, Error, Event, HostCommand, HostEvent, PendingRequest,
    Progress, RestartPolicy, ServerCommand, ServerStatus, StderrMode,
};
pub use eludite_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};
pub use eludite_protocol::{host, lsp};
pub use registry::{FormatterPick, FormatterSpec, ServerRegistration, ServerRegistry, Via};
pub use server::{
    ServerClient, ServerSetup, client_capabilities, configuration, methods_generic, path_to_uri,
};
pub use transport::{FramedTransport, Transport};
