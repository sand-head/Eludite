//! LSP client (PLAN.md D3, 4.3).
//!
//! The shell speaks LSP 3.17 plus Roslyn's `roslyn/*` and our `niello/*`
//! extensions to `niello-host` over stdio with `Content-Length` framing. This
//! crate will hold the client; today it provides the typed message transport.

use std::io::{self, BufRead, Write};

use niello_protocol::framing;
pub use niello_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};

/// A bidirectional JSON-RPC message channel.
pub trait Transport {
    fn send(&mut self, message: &Message) -> io::Result<()>;
    /// `Ok(None)` when the peer closed the stream.
    fn recv(&mut self) -> io::Result<Option<Message>>;
}

/// JSON-RPC over a `Content-Length`-framed byte stream (a child's stdio, a socket).
#[derive(Debug)]
pub struct FramedTransport<R, W> {
    reader: R,
    writer: W,
}

impl<R: BufRead, W: Write> FramedTransport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

impl<R: BufRead, W: Write> Transport for FramedTransport<R, W> {
    fn send(&mut self, message: &Message) -> io::Result<()> {
        let body = serde_json::to_vec(message)?;
        framing::write_message(&mut self.writer, &body)
    }

    fn recv(&mut self) -> io::Result<Option<Message>> {
        match framing::read_message(&mut self.reader)? {
            Some(body) => Ok(Some(serde_json::from_slice(&body)?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn framed_round_trip() {
        let mut wire = Vec::new();
        {
            let mut t = FramedTransport::new(io::empty(), &mut wire);
            t.send(&Message::Request(Request::new(
                1,
                "initialize",
                Some(json!({"clientName": "niello", "clientVersion": "0.1.0"})),
            )))
            .unwrap();
            t.send(&Message::Notification(Notification::new("exit", None)))
                .unwrap();
        }
        let mut t = FramedTransport::new(io::Cursor::new(wire), io::sink());
        assert!(matches!(t.recv().unwrap(), Some(Message::Request(r)) if r.method == "initialize"));
        assert!(matches!(t.recv().unwrap(), Some(Message::Notification(n)) if n.method == "exit"));
        assert!(t.recv().unwrap().is_none());
    }
}
