//! The control channel (`protocol/schemas/browser-rpc/browser-rpc.md`): JSON-RPC 2.0 framed with `Content-Length`
//! on stdin and the protocol copy of stdout. One writer thread owns the output, so CEF's UI thread only queues
//! messages and never blocks on a full pipe.

use std::io::{BufReader, Read, Write};
use std::sync::mpsc;

use serde_json::{Value, json};

/// The protocol version both sides speak (`initialize`).
pub const PROTOCOL_VERSION: u64 = 1;

pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// A `tab` the engine does not know.
pub const NO_SUCH_TAB: i64 = -32001;

/// A request or notification from the shell.
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

impl Incoming {
    pub fn parse(body: &[u8]) -> Result<Incoming, String> {
        let v: Value = serde_json::from_slice(body).map_err(|e| format!("bad JSON: {e}"))?;
        let method = v["method"]
            .as_str()
            .ok_or("a message without a method")?
            .to_owned();
        Ok(Incoming {
            id: v.get("id").cloned(),
            method,
            params: v.get("params").cloned().unwrap_or(json!({})),
        })
    }
}

enum Msg {
    Body(Vec<u8>),
    Flush(mpsc::Sender<()>),
}

/// Where the engine's outgoing messages go: a channel to the writer thread.
#[derive(Clone)]
pub struct Out(mpsc::Sender<Msg>);

impl Out {
    /// Start the writer thread over `w` (the protocol copy of stdout).
    pub fn spawn(w: impl Write + Send + 'static) -> Out {
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::Builder::new()
            .name("rpc-writer".into())
            .spawn(move || {
                let mut w = w;
                while let Ok(m) = rx.recv() {
                    match m {
                        Msg::Body(body) => {
                            if eludite_protocol::framing::write_message(&mut w, &body).is_err() {
                                break;
                            }
                        }
                        Msg::Flush(done) => {
                            let _ = w.flush();
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("spawn the writer thread");
        Out(tx)
    }

    /// A channel for tests: every message's body.
    pub fn channel() -> (Out, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let (out_tx, out_rx) = mpsc::channel::<Msg>();
        std::thread::spawn(move || {
            while let Ok(m) = out_rx.recv() {
                match m {
                    Msg::Body(b) => {
                        let _ = tx.send(b);
                    }
                    Msg::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        (Out(out_tx), rx)
    }

    pub fn raw(&self, body: Vec<u8>) {
        let _ = self.0.send(Msg::Body(body));
    }

    /// Wait (up to `timeout`) until everything queued before this call is written.
    pub fn flush(&self, timeout: std::time::Duration) {
        let (tx, rx) = mpsc::channel();
        if self.0.send(Msg::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(timeout);
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.raw(
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
                .unwrap_or_default(),
        );
    }

    pub fn result(&self, id: &Value, result: Value) {
        self.raw(
            serde_json::to_vec(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
                .unwrap_or_default(),
        );
    }

    pub fn error(&self, id: &Value, code: i64, message: &str) {
        self.raw(
            serde_json::to_vec(&json!({
                "jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}
            }))
            .unwrap_or_default(),
        );
    }

    /// `tab/cdpEvent` around a message from DevTools, unparsed: the agent's bytes are a JSON object already.
    pub fn cdp_event(&self, tab: &str, message: &[u8]) {
        let mut body = Vec::with_capacity(message.len() + 96);
        body.extend_from_slice(br#"{"jsonrpc":"2.0","method":"tab/cdpEvent","params":{"tab":"#);
        body.extend_from_slice(&serde_json::to_vec(tab).unwrap_or_default());
        body.extend_from_slice(br#","message":"#);
        body.extend_from_slice(message);
        body.extend_from_slice(b"}}");
        self.raw(body);
    }
}

/// Read framed messages from `r` until EOF, handing each to `f`; malformed bodies go to `bad`.
pub fn read_loop(r: impl Read, mut f: impl FnMut(Incoming), mut bad: impl FnMut(String)) {
    let mut r = BufReader::new(r);
    loop {
        match eludite_protocol::framing::read_message(&mut r) {
            Ok(Some(body)) => match Incoming::parse(&body) {
                Ok(m) => f(m),
                Err(e) => bad(e),
            },
            Ok(None) => return,
            Err(e) => {
                bad(format!("reading stdin: {e}"));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_through_the_framing() {
        let mut wire = Vec::new();
        for body in [
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientName":"t"}}"#
                .as_slice(),
            br#"{"jsonrpc":"2.0","method":"tab/input","params":{"tab":"1"}}"#.as_slice(),
            b"not json".as_slice(),
        ] {
            eludite_protocol::framing::write_message(&mut wire, body).unwrap();
        }
        let mut got = Vec::new();
        let mut errors = Vec::new();
        read_loop(wire.as_slice(), |m| got.push(m), |e| errors.push(e));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].id, Some(json!(1)));
        assert_eq!(got[0].params["clientName"], "t");
        assert_eq!(got[1].id, None);
        assert_eq!(got[1].method, "tab/input");
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn a_cdp_event_wraps_the_agent_message_unparsed() {
        let (out, rx) = Out::channel();
        out.cdp_event("1", br#"{"id":5,"result":{"value":2}}"#);
        let v: Value = serde_json::from_slice(&rx.recv().unwrap()).unwrap();
        assert_eq!(v["method"], "tab/cdpEvent");
        assert_eq!(v["params"]["tab"], "1");
        assert_eq!(v["params"]["message"]["result"]["value"], 2);
        out.error(&json!(3), NO_SUCH_TAB, "no tab 9");
        let v: Value = serde_json::from_slice(&rx.recv().unwrap()).unwrap();
        assert_eq!(v["error"]["code"], NO_SUCH_TAB);
    }
}
