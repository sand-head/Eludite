//! Wire types for Eludite's protocols (PLAN.md D2, D3, D5).
//!
//! `jsonrpc`: minimal JSON-RPC 2.0 messages shared by the LSP, ACP and MCP crates.
//! `framing`: `Content-Length` framing used by LSP and DAP. `host`: the Eludite messages between the shell and
//! `eludite-host` (see `protocol/schemas/host-rpc.md`). `lsp`: the typed subset of LSP 3.17 the host forwards.
//! `typed`: method-to-type traits a client uses to send typed requests.
//! MIT-licensed so alternative hosts and clients can use it under any license.

pub mod framing;
pub mod host;
pub mod jsonrpc;
pub mod lsp;
pub mod typed;

pub use jsonrpc::{ErrorObject, Id, Message, Notification, Request, Response};
pub use typed::{NotificationType, RequestType};

#[cfg(test)]
mod test_util {
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::Value;

    /// Asserts `value` encodes to `expected` and decodes back to an equal value.
    pub fn round_trip<T>(value: &T, expected: Value)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let v = serde_json::to_value(value).unwrap();
        assert_eq!(v, expected);
        let back: T = serde_json::from_value(v).unwrap();
        assert_eq!(&back, value);
    }
}

#[cfg(test)]
mod schema_tests;
