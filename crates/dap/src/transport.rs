//! Reaching an adapter (ADR-0007): a local child over stdio, an adapter listening on TCP (possibly on another machine),
//! or one run over ssh with its stdio forwarded. Each gives the client the same thing, a [`Connection`]: a byte reader
//! and writer carrying `Content-Length`-framed DAP, plus the child process when there is one.
//!
//! A local adapter that serves DAP on a TCP port it chooses (brief 0038: vscode-js-debug's `dapDebugServer.js`, run
//! as `node dapDebugServer.js 0 127.0.0.1`) is started by [`start_tcp_server`]: the port is read from its first stdout
//! line, and the [`TcpServer`] opens as many connections as the adapter asks for (one per child session); the first
//! owns the process. [`AdapterServer`] is what the shell holds of such an adapter, so a test serves its fake (or a
//! replay) behind the same interface.

use std::io::{self, BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use crate::AdapterTransport;

/// How long a TCP connect may take.
pub const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// An open channel to one adapter.
pub struct Connection {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    /// The adapter process, for a stdio or ssh transport.
    pub child: Option<Child>,
    /// The adapter's stderr (its log), drained by the client.
    pub stderr: Option<ChildStderr>,
    /// Closes the channel from any thread (a TCP socket's shutdown); the child is killed instead when there is one.
    pub shutdown: Option<Box<dyn Fn() + Send + Sync>>,
    /// For messages: `netcoredbg --interpreter=vscode (stdio)`, `tcp winbox:4711`.
    pub description: String,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("description", &self.description)
            .field("child", &self.child.as_ref().map(Child::id))
            .finish()
    }
}

impl Connection {
    /// A connection over any pair of streams (an in-process fake adapter).
    pub fn from_streams(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        description: impl Into<String>,
    ) -> Self {
        Self {
            reader: Box::new(reader),
            writer: Box::new(writer),
            child: None,
            stderr: None,
            shutdown: None,
            description: description.into(),
        }
    }

    /// A connected TCP stream.
    pub fn from_tcp(stream: TcpStream, description: impl Into<String>) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        let reader = stream.try_clone()?;
        let closer = stream.try_clone()?;
        Ok(Self {
            reader: Box::new(reader),
            writer: Box::new(stream),
            child: None,
            stderr: None,
            shutdown: Some(Box::new(move || {
                let _ = closer.shutdown(std::net::Shutdown::Both);
            })),
            description: description.into(),
        })
    }
}

/// The command line a stdio or ssh transport runs: (program, arguments).
pub fn command_line(transport: &AdapterTransport) -> Option<(String, Vec<String>)> {
    match transport {
        AdapterTransport::Stdio { command, args } => Some((command.clone(), args.clone())),
        // `-T`: no pseudo-terminal, so the adapter's stdio is a clean byte stream; `--` ends ssh's own options.
        AdapterTransport::Ssh {
            destination,
            command,
            args,
        } => {
            let mut a = vec!["-T".to_owned(), destination.clone(), "--".to_owned()];
            a.push(command.clone());
            a.extend(args.iter().cloned());
            Some(("ssh".to_owned(), a))
        }
        AdapterTransport::Tcp { .. } => None,
    }
}

/// A short description of `transport` for messages.
pub fn describe(transport: &AdapterTransport) -> String {
    match transport {
        AdapterTransport::Stdio { command, args } => {
            format!("{command} {} (stdio)", args.join(" "))
        }
        AdapterTransport::Tcp { host, port } => format!("tcp {host}:{port}"),
        AdapterTransport::Ssh {
            destination,
            command,
            ..
        } => format!("ssh {destination} {command}"),
    }
}

/// Open `transport`. Blocks for the spawn or the TCP connect: call it off the UI thread.
pub fn connect(transport: &AdapterTransport) -> io::Result<Connection> {
    connect_with_env(transport, &[])
}

/// [`connect`], with `env` added to a spawned adapter's environment (a relocated Mono's variables for
/// `eludite-dbg-mono`; ignored for TCP).
pub fn connect_with_env(
    transport: &AdapterTransport,
    env: &[(String, String)],
) -> io::Result<Connection> {
    let description = describe(transport);
    match transport {
        AdapterTransport::Tcp { host, port } => {
            let addr = (host.as_str(), *port)
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, format!("{host}: no address"))
                })?;
            let stream = TcpStream::connect_timeout(&addr, TCP_CONNECT_TIMEOUT)?;
            Connection::from_tcp(stream, description)
        }
        other => {
            let (program, args) = command_line(other).expect("stdio and ssh have a command line");
            let mut child = Command::new(&program)
                .args(&args)
                .envs(env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| io::Error::new(e.kind(), format!("{program}: {e}")))?;
            let reader = child.stdout.take().expect("piped stdout");
            let writer = child.stdin.take().expect("piped stdin");
            let stderr = child.stderr.take();
            Ok(Connection {
                reader: Box::new(reader),
                writer: Box::new(writer),
                child: Some(child),
                stderr,
                shutdown: None,
                description,
            })
        }
    }
}

/// How long a TCP server adapter may take to say where it listens.
pub const TCP_SERVER_START_TIMEOUT: Duration = Duration::from_secs(10);

/// An adapter that serves DAP on several connections (brief 0038): the browser session's, then one per child session
/// the adapter starts with `startDebugging`.
pub trait AdapterServer: Send + Sync {
    /// Open the next connection. Blocks for the connect: call it off the UI thread.
    fn connect(&self) -> io::Result<Connection>;
    /// For messages: `tcp 127.0.0.1:41234`.
    fn describe(&self) -> String;
}

/// The port in an adapter's line saying where it listens: the number after the last colon of
/// `Debug server listening at 127.0.0.1:41234` (vscode-js-debug's first stdout line).
pub fn listening_port(line: &str) -> Option<u16> {
    if !line.to_ascii_lowercase().contains("listening") {
        return None;
    }
    let digits: String = line
        .trim_end()
        .rsplit(':')
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok().filter(|p| *p > 0)
}

/// A local adapter process listening on a loopback port (started by [`start_tcp_server`]), or an adapter already
/// listening there (`child` none: a test's fake). The first connection owns the process: closing it kills the
/// adapter, and every other connection with it.
pub struct TcpServer {
    pub host: String,
    pub port: u16,
    child: Mutex<Option<(Child, Option<ChildStderr>)>>,
    description: String,
}

impl std::fmt::Debug for TcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpServer")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

impl TcpServer {
    /// An adapter already listening at `host:port`, started by someone else (a test's fake adapter).
    pub fn listening(host: &str, port: u16) -> Self {
        Self {
            host: host.to_owned(),
            port,
            child: Mutex::new(None),
            description: format!("tcp {host}:{port}"),
        }
    }

    /// The adapter process's id, while the first connection has not taken it.
    pub fn pid(&self) -> Option<u32> {
        self.child
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(c, _)| c.id())
    }
}

impl AdapterServer for TcpServer {
    fn connect(&self) -> io::Result<Connection> {
        let t = AdapterTransport::Tcp {
            host: self.host.clone(),
            port: self.port,
        };
        let mut c = connect(&t)?;
        if let Some((child, stderr)) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            c.child = Some(child);
            c.stderr = stderr;
        }
        c.description = self.description.clone();
        Ok(c)
    }

    fn describe(&self) -> String {
        self.description.clone()
    }
}

impl Drop for TcpServer {
    fn drop(&mut self) {
        // Never connected to: nobody else will end it.
        if let Some((mut child, _)) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Start `program args` (`node dapDebugServer.js 0 127.0.0.1`) and read its first stdout line for the port it
/// listens on, within `timeout`. Its later stdout is drained; its stderr goes to the first connection (the client's
/// `adapter` output). Blocks: call it off the UI thread.
pub fn start_tcp_server(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> io::Result<TcpServer> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", program.display())))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("dap-server-stdout".into())
        .spawn(move || {
            let mut lines = io::BufReader::new(stdout).lines();
            let first = lines.next();
            let _ = tx.send(first);
            // The rest is drained so the adapter never blocks on a full pipe.
            for _ in lines.map_while(Result::ok) {}
        })?;
    let first = match rx.recv_timeout(timeout) {
        Ok(Some(Ok(line))) => line,
        Ok(_) => {
            let _ = child.kill();
            let _ = child.wait();
            let mut err = String::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_string(&mut err);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "{} exited before it said where it listens{}",
                    program.display(),
                    err.lines()
                        .last()
                        .map(|l| format!(": {l}"))
                        .unwrap_or_default()
                ),
            ));
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{} did not say where it listens within {} s",
                    program.display(),
                    timeout.as_secs()
                ),
            ));
        }
    };
    let Some(port) = listening_port(&first) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}'s first line names no port: {first}", program.display()),
        ));
    };
    let host = "127.0.0.1".to_owned();
    Ok(TcpServer {
        description: format!("tcp {host}:{port}"),
        host,
        port,
        child: Mutex::new(Some((child, stderr))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_lines_and_descriptions() {
        let stdio = AdapterTransport::Stdio {
            command: "netcoredbg".into(),
            args: vec!["--interpreter=vscode".into()],
        };
        assert_eq!(
            command_line(&stdio),
            Some(("netcoredbg".into(), vec!["--interpreter=vscode".into()]))
        );
        assert_eq!(describe(&stdio), "netcoredbg --interpreter=vscode (stdio)");
        let ssh = AdapterTransport::Ssh {
            destination: "me@winbox".into(),
            command: "eludite-dbg-netfx".into(),
            args: vec!["--stdio".into()],
        };
        assert_eq!(
            command_line(&ssh),
            Some((
                "ssh".into(),
                vec![
                    "-T".into(),
                    "me@winbox".into(),
                    "--".into(),
                    "eludite-dbg-netfx".into(),
                    "--stdio".into()
                ]
            ))
        );
        let tcp = AdapterTransport::Tcp {
            host: "127.0.0.1".into(),
            port: 4711,
        };
        assert_eq!(command_line(&tcp), None);
        assert_eq!(describe(&tcp), "tcp 127.0.0.1:4711");
    }

    #[test]
    fn missing_adapter_is_an_error_naming_it() {
        let err = connect(&AdapterTransport::Stdio {
            command: "eludite-no-such-adapter".into(),
            args: vec![],
        })
        .unwrap_err();
        assert!(err.to_string().contains("eludite-no-such-adapter"), "{err}");
    }

    #[test]
    fn the_listening_line_names_the_port() {
        assert_eq!(
            listening_port("Debug server listening at 127.0.0.1:41234"),
            Some(41234)
        );
        assert_eq!(
            listening_port("Debug server listening at ::1:8123\n"),
            Some(8123)
        );
        assert_eq!(listening_port("Debug server listening at /tmp/sock"), None);
        assert_eq!(listening_port("Usage: dapDebugServer.js [port]"), None);
    }

    /// A fake `node` (a script printing the line vscode-js-debug prints) starts, its port is read and connected to,
    /// the first connection owns the process, and a second connection reaches the same port (brief 0038).
    #[cfg(unix)]
    #[test]
    fn a_tcp_server_adapter_says_its_port_and_serves_several_connections() {
        use std::os::unix::fs::PermissionsExt as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = std::thread::spawn(move || {
            let a = listener.accept().unwrap().0;
            let b = listener.accept().unwrap().0;
            (a, b)
        });
        let t = tempfile::tempdir().unwrap();
        let node = t.path().join("node");
        std::fs::write(
            &node,
            format!(
                "#!/bin/sh\necho \"Debug server listening at $3:{port}\"\necho \"args: $*\" >&2\nexec sleep 30\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = vec![
            "/x/dapDebugServer.js".to_owned(),
            "0".into(),
            "127.0.0.1".into(),
        ];
        let server = start_tcp_server(&node, &args, TCP_SERVER_START_TIMEOUT).unwrap();
        assert_eq!((server.host.as_str(), server.port), ("127.0.0.1", port));
        assert_eq!(server.describe(), format!("tcp 127.0.0.1:{port}"));
        let pid = server.pid().expect("the process");
        let first = server.connect().unwrap();
        assert_eq!(first.child.as_ref().map(Child::id), Some(pid));
        let mut stderr = String::new();
        io::BufReader::new(first.stderr.unwrap())
            .read_line(&mut stderr)
            .unwrap();
        assert!(
            stderr.contains("/x/dapDebugServer.js 0 127.0.0.1"),
            "{stderr}"
        );
        let second = server.connect().unwrap();
        assert!(second.child.is_none() && server.pid().is_none());
        let _ = accepted.join().unwrap();
        let mut child = first.child.unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        // A program that exits without the line, or says no port, is an error naming it.
        std::fs::write(&node, "#!/bin/sh\necho nope >&2\nexit 1\n").unwrap();
        let e = start_tcp_server(&node, &args, TCP_SERVER_START_TIMEOUT).unwrap_err();
        assert!(
            e.to_string()
                .contains("exited before it said where it listens: nope"),
            "{e}"
        );
        std::fs::write(&node, "#!/bin/sh\necho hello\nsleep 5\n").unwrap();
        let e = start_tcp_server(&node, &args, TCP_SERVER_START_TIMEOUT).unwrap_err();
        assert!(e.to_string().contains("names no port: hello"), "{e}");
    }
}
