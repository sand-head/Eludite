//! Test-only: `eludite-openai-fake-relay --mcp-relay ADDR` does what `eludite --mcp-relay ADDR` does (connect to
//! ADDR, send `$ELUDITE_MCP_TOKEN` as the first line, then copy stdin to the socket and the socket to stdout), so
//! the tests can hand the adapter "the IDE's relay" without building the IDE.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpStream};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let addr = args
        .iter()
        .skip_while(|a| *a != "--mcp-relay")
        .nth(1)
        .expect("--mcp-relay ADDR");
    let token = std::env::var("ELUDITE_MCP_TOKEN").unwrap_or_default();
    let mut sock = TcpStream::connect(addr).expect("connect");
    let _ = sock.set_nodelay(true);
    sock.write_all(format!("{token}\n").as_bytes())
        .expect("token");
    let mut up = sock.try_clone().expect("clone");
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut up);
        let _ = up.shutdown(Shutdown::Write);
    });
    let mut out = std::io::stdout();
    let mut reader = BufReader::new(sock);
    let mut line = Vec::new();
    while let Ok(n) = reader.read_until(b'\n', &mut line) {
        if n == 0 || out.write_all(&line).and_then(|()| out.flush()).is_err() {
            break;
        }
        line.clear();
    }
}
