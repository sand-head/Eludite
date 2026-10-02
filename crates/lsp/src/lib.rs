//! The shell's client of `niello-host` (PLAN.md D2, D3, 4.3; contract: `protocol/schemas/host-rpc.md`).
//!
//! The shell speaks the `niello/*` vocabulary plus forwarded LSP 3.17 to `niello-host` over the host's stdio with
//! `Content-Length` framing. This crate owns that connection:
//!
//! - **Process supervision**: [`HostClient::start`] spawns the host from a [`HostCommand`], keeps its stderr out of
//!   the protocol stream, reports exits as [`HostEvent`]s and restarts it under a [`RestartPolicy`].
//! - **Requests**: typed with [`niello_protocol::RequestType`] marker types ([`niello_protocol::host`] and
//!   [`niello_protocol::lsp`]), correlated by id, awaited off the UI thread through [`PendingRequest`].
//! - **Cancellation**: [`PendingRequest::cancel`] sends `$/cancelRequest`; the wait ends with [`Error::Canceled`]
//!   and any result is discarded.
//! - **Generations**: forwarded requests are pinned to the current solution generation (`nielloGeneration`);
//!   results and diagnostics for an older generation are dropped ([`Error::Stale`]).
//! - **Events**: solution status, language server status, diagnostics and host lifecycle arrive on a channel as
//!   [`Event`]s.
//!
//! Public API boundary: [`HostClient`], [`PendingRequest`], [`HostCommand`], [`ClientInfo`], [`RestartPolicy`],
//! [`StderrMode`], [`Event`], [`HostEvent`], [`Error`], plus the low-level [`Transport`] / [`FramedTransport`].
//! Message types live in `niello-protocol`.

mod client;
mod transport;

pub use client::{
    ClientInfo, Error, Event, HostClient, HostCommand, HostEvent, PendingRequest, RestartPolicy,
    StderrMode,
};
pub use niello_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};
pub use niello_protocol::{host, lsp};
pub use transport::{FramedTransport, Transport};
