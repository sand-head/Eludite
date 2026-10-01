//! DAP client and transports (PLAN.md D3, D7, 4.5).
//!
//! Debug adapters (netcoredbg, `niello-dbg-netfx`) are reached through a
//! transport abstraction so a local stdio child and a remote TCP/SSH adapter look
//! the same to the debugger UI. DAP is not JSON-RPC; it has its own envelope,
//! but shares `Content-Length` framing with LSP (`niello_protocol::framing`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use niello_protocol::framing;

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
