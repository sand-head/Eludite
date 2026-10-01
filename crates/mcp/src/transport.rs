//! Transports: newline-delimited JSON-RPC over any byte stream (MCP stdio
//! framing), a local TCP endpoint inside the IDE process, and a stdio relay.
//!
//! Agents launch MCP servers as child processes speaking stdio, but the command
//! bus lives in the IDE process. So the IDE listens on `127.0.0.1:<random
//! port>` and passes the agent a stdio server whose command is the IDE binary
//! itself in relay mode ([`relay_stdio`]), which pipes stdin and stdout to that
//! port. The first line on each connection must be the per-run token (128 bits
//! from the OS random source), so other local processes cannot drive the
//! command bus by guessing the port.
//!
//! When the bus gains or replaces a command after a client listed the tools,
//! the server sends `notifications/tools/list_changed` (on the TCP endpoint
//! within [`LIST_WATCH_INTERVAL`], on [`serve_lines`] after the next message).

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::server::McpServer;

/// Environment variable carrying the token from the IDE to the relay process.
pub const TOKEN_ENV: &str = "ELUDITE_MCP_TOKEN";

/// How often a connection checks whether the tool list changed.
pub const LIST_WATCH_INTERVAL: Duration = Duration::from_millis(200);

const LIST_CHANGED: &[u8] =
    b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n";

/// The registry generation a connection last listed the tools at (0: not listed yet).
#[derive(Debug, Default)]
struct Listed(AtomicU64);

impl Listed {
    /// Remember the generation when `line` is a `tools/list` request.
    fn note(&self, server: &McpServer, line: &str) {
        if line.contains("\"tools/list\"") {
            self.0
                .store(server.registry().generation().max(1), Ordering::Release);
        }
    }

    /// True (once) when the bus changed since the tools were listed.
    fn changed(&self, server: &McpServer) -> bool {
        let listed = self.0.load(Ordering::Acquire);
        let now = server.registry().generation().max(1);
        listed != 0
            && now != listed
            && self
                .0
                .compare_exchange(listed, now, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
}

fn write_reply(server: &McpServer, line: &str, writer: &mut impl Write) -> io::Result<()> {
    if let Some(reply) = server.handle_line(line) {
        let mut out = serde_json::to_vec(&reply).map_err(io::Error::other)?;
        out.push(b'\n');
        writer.write_all(&out)?;
        writer.flush()?;
    }
    Ok(())
}

/// Serve newline-delimited JSON-RPC until `reader` reaches EOF.
pub fn serve_lines(
    server: &McpServer,
    reader: impl BufRead,
    mut writer: impl Write,
) -> io::Result<()> {
    let listed = Listed::default();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if listed.changed(server) {
            writer.write_all(LIST_CHANGED)?;
        }
        listed.note(server, &line);
        write_reply(server, &line, &mut writer)?;
    }
    Ok(())
}

/// Where the IDE's MCP endpoint listens and the token a relay must present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    pub addr: SocketAddr,
    pub token: String,
}

/// A 128-bit token from the operating system's random source.
fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Listen on `127.0.0.1:0` and serve each authenticated connection on its own
/// thread. Returns once the socket is bound; the accept loop runs until the
/// process exits.
pub fn listen_local(server: Arc<McpServer>) -> io::Result<LocalEndpoint> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let endpoint = LocalEndpoint {
        addr: listener.local_addr()?,
        token: new_token()?,
    };
    let token = endpoint.token.clone();
    thread::Builder::new()
        .name("mcp-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let server = server.clone();
                let token = token.clone();
                let _ = thread::Builder::new()
                    .name("mcp-conn".into())
                    .spawn(move || {
                        let _ = serve_authenticated(&server, stream, &token);
                    });
            }
        })?;
    Ok(endpoint)
}

fn serve_authenticated(server: &McpServer, stream: TcpStream, token: &str) -> io::Result<()> {
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    if first.trim_end() != token {
        let _ = stream.shutdown(Shutdown::Both);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bad MCP token",
        ));
    }
    // Replies and list-changed notifications share the socket.
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    let listed = Arc::new(Listed::default());
    let open = Arc::new(AtomicBool::new(true));
    {
        let (server, writer, listed, open) =
            (server.clone(), writer.clone(), listed.clone(), open.clone());
        thread::Builder::new()
            .name("mcp-list-watch".into())
            .spawn(move || {
                while open.load(Ordering::Acquire) {
                    thread::sleep(LIST_WATCH_INTERVAL);
                    if listed.changed(&server) {
                        let mut w = writer.lock().unwrap_or_else(|e| e.into_inner());
                        if w.write_all(LIST_CHANGED).and_then(|()| w.flush()).is_err() {
                            break;
                        }
                    }
                }
            })?;
    }
    let result = (|| {
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            listed.note(server, &line);
            // A tool call may wait for the user (a permission prompt, a pending change under review), so each one
            // runs on its own thread and the connection keeps serving others; replies may come out of order, as
            // JSON-RPC allows.
            if line.contains("\"tools/call\"") {
                let (server, writer) = (server.clone(), writer.clone());
                thread::Builder::new()
                    .name("mcp-call".into())
                    .spawn(move || {
                        if let Some(reply) = server.handle_line(&line) {
                            let _ = send(&writer, &reply);
                        }
                    })?;
            } else if let Some(reply) = server.handle_line(&line) {
                send(&writer, &reply)?;
            }
        }
        Ok(())
    })();
    open.store(false, Ordering::Release);
    result
}

fn send(writer: &Mutex<TcpStream>, reply: &crate::Response) -> io::Result<()> {
    let mut out = serde_json::to_vec(reply).map_err(io::Error::other)?;
    out.push(b'\n');
    let mut w = writer.lock().unwrap_or_else(|e| e.into_inner());
    w.write_all(&out)?;
    w.flush()
}

/// Relay mode: connect to the IDE's endpoint, send the token, then copy stdin
/// to the socket and the socket to stdout until either side closes.
pub fn relay_stdio(addr: SocketAddr, token: &str) -> io::Result<()> {
    relay(addr, token, io::stdin(), io::stdout())
}

/// [`relay_stdio`] over arbitrary streams (for tests).
pub fn relay(
    addr: SocketAddr,
    token: &str,
    mut input: impl Read + Send + 'static,
    mut output: impl Write,
) -> io::Result<()> {
    let mut sock = TcpStream::connect(addr)?;
    let _ = sock.set_nodelay(true);
    sock.write_all(format!("{token}\n").as_bytes())?;
    let mut upstream = sock.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut input, &mut upstream);
        let _ = upstream.shutdown(Shutdown::Write);
    });
    // Copy line by line with a flush each time: stdout may be block-buffered.
    let mut reader = BufReader::new(sock);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            // The IDE hung up (for example after rejecting the token). macOS and
            // Windows report that as a reset or abort rather than a clean EOF.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        output.write_all(&line)?;
        output.flush()?;
    }
}
