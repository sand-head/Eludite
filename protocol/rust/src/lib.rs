//! Wire types for Niello's protocols (PLAN.md D2, D3, D5).
//!
//! `jsonrpc`: minimal JSON-RPC 2.0 messages shared by the LSP, ACP and MCP crates.
//! `framing`: `Content-Length` framing used by LSP and DAP. `host`: params and
//! results of the `niello-host` methods (see `protocol/schemas/host-rpc.md`).
//! MIT-licensed so alternative hosts and clients can use it under any license.

pub mod framing;
pub mod host;
pub mod jsonrpc;

pub use jsonrpc::{ErrorObject, Id, Message, Notification, Request, Response};
