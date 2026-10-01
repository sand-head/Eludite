//! JSON-RPC over `Content-Length` framing.

use std::io::{self, BufRead, Write};

use eludite_protocol::Message;
use eludite_protocol::framing;

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
    use eludite_protocol::{Notification, Request};
    use serde_json::json;

    #[test]
    fn framed_round_trip() {
        let mut wire = Vec::new();
        {
            let mut t = FramedTransport::new(io::empty(), &mut wire);
            t.send(&Message::Request(Request::new(
                1,
                "eludite/host/initialize",
                Some(json!({"clientName": "eludite", "clientVersion": "0.1.0"})),
            )))
            .unwrap();
            t.send(&Message::Notification(Notification::new(
                "eludite/host/exit",
                None,
            )))
            .unwrap();
        }
        let mut t = FramedTransport::new(io::Cursor::new(wire), io::sink());
        assert!(
            matches!(t.recv().unwrap(), Some(Message::Request(r)) if r.method == "eludite/host/initialize")
        );
        assert!(
            matches!(t.recv().unwrap(), Some(Message::Notification(n)) if n.method == "eludite/host/exit")
        );
        assert!(t.recv().unwrap().is_none());
    }
}
