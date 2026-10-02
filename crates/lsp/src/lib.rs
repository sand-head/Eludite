//! The shell's client of `eludite-host` (PLAN.md D2, D3, 4.3; contract: `protocol/schemas/host-rpc.md`).
//!
//! The shell speaks the `eludite/*` vocabulary plus forwarded LSP 3.17 to `eludite-host` over the host's stdio with
//! `Content-Length` framing. This crate owns that connection:
//!
//! - **Process supervision**: [`HostClient::start`] spawns the host from a [`HostCommand`], keeps its stderr out of
//!   the protocol stream, reports exits as [`HostEvent`]s and restarts it under a [`RestartPolicy`].
//! - **Requests**: typed with [`eludite_protocol::RequestType`] marker types ([`eludite_protocol::host`] and
//!   [`eludite_protocol::lsp`]), correlated by id, awaited off the UI thread through [`PendingRequest`].
//! - **Cancellation**: [`PendingRequest::cancel`] sends `$/cancelRequest`; the wait ends with [`Error::Canceled`]
//!   and any result is discarded.
//! - **Generations**: forwarded requests are pinned to the current solution generation (`eluditeGeneration`);
//!   results and diagnostics for an older generation are dropped ([`Error::Stale`]).
//! - **Events**: solution status, language server status, diagnostics and host lifecycle arrive on a channel as
//!   [`Event`]s.
//!
//! Public API boundary: [`HostClient`], [`PendingRequest`], [`HostCommand`], [`Connector`] (a host running in this
//! process, [`HostClient::start_in_process`]), [`ClientInfo`], [`RestartPolicy`], [`StderrMode`], [`Event`],
//! [`HostEvent`], [`Error`], plus the low-level [`Transport`] / [`FramedTransport`]. Message types live in
//! `eludite-protocol`. With the `fake` feature, [`fake::FakeHost`] is a scripted in-process host for tests.

mod client;
#[cfg(feature = "fake")]
pub mod fake;
mod transport;

pub use client::{
    ClientInfo, Connector, Error, Event, HostClient, HostCommand, HostEvent, PendingRequest,
    RestartPolicy, StderrMode,
};
pub use eludite_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};
pub use eludite_protocol::{host, lsp};
pub use transport::{FramedTransport, Transport};
