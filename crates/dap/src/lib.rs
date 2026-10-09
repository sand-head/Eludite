//! DAP client and transports (PLAN.md D3, D7, 4.5; brief 0018).
//!
//! Debug adapters (netcoredbg, `eludite-dbg-mono` under Mono, lldb-dap for Cargo packages, later `eludite-dbg-netfx`) are reached through a transport abstraction so a local
//! stdio child and a remote TCP or ssh adapter look the same to the debugger UI (ADR-0007). DAP is not JSON-RPC; it
//! has its own envelope ([`ProtocolMessage`]) but shares `Content-Length` framing with LSP
//! (`eludite_protocol::framing`).
//!
//! Public API:
//! - [`AdapterTransport`] and [`transport::connect`]: how to reach an adapter, giving a [`Connection`].
//! - [`DapClient`]: requests and events over a connection, without ever blocking the caller on the adapter unless
//!   it asks to ([`DapClient::request_wait`], for worker threads).
//! - [`session::start`]: the launch or attach handshake in DAP's order, on a worker thread.
//! - [`types`]: the typed subset of DAP Eludite uses, decoded tolerantly.
//! - [`discovery`]: locating netcoredbg (beside the executable, `ELUDITE_NETCOREDBG`, `PATH`), Mono (the setting
//!   `debugger.monoPrefix`, `PATH`, the usual prefixes) and `eludite-dbg-mono` (beside the executable, the setting
//!   `debugger.monoAdapterPath`), lldb-dap (the setting `debugger.lldbDapPath`, `lldb-dap` and `lldb-dap-NN` on
//!   `PATH`, `/usr/lib/llvm-NN`, `xcrun`, CodeLLDB).
//! - [`launch`]: a .NET project's launch configuration: its built program (the DLL, or a .NET Framework project's
//!   .exe), its `launchSettings.json` profile, and the adapter for its target framework and the platform (brief 0022).
//! - [`cargo`]: a Cargo package's launch configuration for lldb-dap: the binary or test executable, the
//!   `[package.metadata.eludite.run]` table, the Rust formatters' `initCommands` (brief 0029).
//! - [`processes`]: the processes a debugger could attach to, with their command lines and runtimes (`/proc` on
//!   Linux, `ps` on macOS, `tasklist` on Windows), and which of them Eludite started (brief 0027).
//! - [`attach`]: which adapter attaches to a process and its `attach` arguments (brief 0027).
//! - `fake` (feature `fake`): a scripted fake adapter for tests, in-process, over TCP or on stdio.
//! - [`record`]: the recorder, a [`Connection`] wrapper that writes a session to a scrubbed `.dap.json` file, and the
//!   re-record check (brief 0033).
//! - `replay` (feature `replay`): the replaying adapter, which serves a recording to a client (brief 0033).
//! - vscode-js-debug (brief 0038): [`discovery::JsDebugSearch`] and [`discovery::NodeSearch`] locate it and the Node.js
//!   it runs on; [`transport::start_tcp_server`] starts it as a TCP server ([`transport::AdapterServer`] opens a
//!   connection per session); [`attach::browser_attach`] is its `pwa-chrome` attach; [`DapClient::start_with`] answers
//!   its `startDebugging` reverse requests; [`session::AdapterFamily`] sends each breakpoint to the adapters that claim
//!   its file; [`sourcemap`] gives its frames their generated location.

pub mod attach;
pub mod cargo;
pub mod client;
pub mod discovery;
#[cfg(feature = "fake")]
pub mod fake;
pub mod launch;
pub mod processes;
pub mod record;
#[cfg(feature = "replay")]
pub mod replay;
pub mod session;
pub mod sourcemap;
pub mod transport;
pub mod types;

#[cfg(test)]
pub(crate) use eludite_test_support::assert_budget;

pub use client::{ClientEvent, DapClient, DapError, EventSink, ReverseHandler};
pub use transport::Connection;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use eludite_protocol::framing;

/// How to reach a debug adapter (D7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdapterTransport {
    /// Spawn the adapter locally and talk over its stdio.
    Stdio { command: String, args: Vec<String> },
    /// Connect to an adapter already listening, possibly on another machine.
    Tcp { host: String, port: u16 },
    /// Run the adapter on `destination` via ssh and talk over the forwarded stdio.
    Ssh {
        destination: String,
        command: String,
        args: Vec<String>,
    },
}

/// A DAP protocol message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProtocolMessage {
    Request {
        seq: i64,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arguments: Option<Value>,
    },
    Response {
        seq: i64,
        request_seq: i64,
        success: bool,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
    },
    Event {
        seq: i64,
        event: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_and_event_shapes() {
        let req = ProtocolMessage::Request {
            seq: 1,
            command: "initialize".into(),
            arguments: Some(json!({"adapterID": "netfx"})),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["type"], json!("request"));
        assert_eq!(serde_json::from_value::<ProtocolMessage>(v).unwrap(), req);

        let ev: ProtocolMessage = serde_json::from_value(
            json!({"seq": 5, "type": "event", "event": "stopped", "body": {"reason": "breakpoint"}}),
        )
        .unwrap();
        assert!(matches!(ev, ProtocolMessage::Event { ref event, .. } if event == "stopped"));
    }

    #[test]
    fn transport_serde() {
        let t = AdapterTransport::Tcp {
            host: "winbox".into(),
            port: 4711,
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v, json!({"kind": "tcp", "host": "winbox", "port": 4711}));
    }
}
