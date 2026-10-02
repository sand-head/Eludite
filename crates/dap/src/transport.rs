//! Reaching an adapter (ADR-0007): a local child over stdio, an adapter listening on TCP (possibly on another machine),
//! or one run over ssh with its stdio forwarded. Each gives the client the same thing, a [`Connection`]: a byte reader
//! and writer carrying `Content-Length`-framed DAP, plus the child process when there is one.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, ChildStderr, Command, Stdio};
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
}
