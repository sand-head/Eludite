//! Transports: newline-delimited JSON-RPC over any byte stream (MCP stdio
//! framing), a local TCP endpoint inside the IDE process, and a stdio relay.
//!
//! Agents launch MCP servers as child processes speaking stdio, but the command
//! bus lives in the IDE process. So the IDE listens on `127.0.0.1:<random
//! port>` and passes the agent a stdio server whose command is the IDE binary
//! itself in relay mode ([`relay_stdio`]), which pipes stdin and stdout to that
//! port. The first line on each connection must be the per-run token, so other
//! local processes cannot drive the command bus by guessing the port.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use crate::server::McpServer;

/// Environment variable carrying the token from the IDE to the relay process.
pub const TOKEN_ENV: &str = "NIELLO_MCP_TOKEN";

/// Serve newline-delimited JSON-RPC until `reader` reaches EOF.
pub fn serve_lines(
    server: &McpServer,
    reader: impl BufRead,
    mut writer: impl Write,
) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_line(&line) {
            let mut out = serde_json::to_vec(&reply).map_err(io::Error::other)?;
            out.push(b'\n');
            writer.write_all(&out)?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Where the IDE's MCP endpoint listens and the token a relay must present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEndpoint {
    pub addr: SocketAddr,
    pub token: String,
}

/// A 128-bit token from the OS-seeded `RandomState` (SipHash keys). Good enough
/// to stop another local process guessing it; not a cryptographic RNG.
fn new_token() -> String {
    let mut out = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Listen on `127.0.0.1:0` and serve each authenticated connection on its own
/// thread. Returns once the socket is bound; the accept loop runs until the
/// process exits.
pub fn listen_local(server: Arc<McpServer>) -> io::Result<LocalEndpoint> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let endpoint = LocalEndpoint {
        addr: listener.local_addr()?,
        token: new_token(),
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
    serve_lines(server, reader, stream)
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
