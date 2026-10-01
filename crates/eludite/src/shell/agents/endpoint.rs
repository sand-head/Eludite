//! Eludite's MCP endpoint, inside the `eludite` process (brief 0016): the command bus's agent-visible commands served
//! on a token-checked `127.0.0.1` port, which a hosted agent reaches through `eludite --mcp-relay ADDR`, a stdio MCP
//! server it launches itself (ACP requires every agent to support stdio MCP servers; brief 0005 report, section 6).
//!
//! The endpoint starts with the first agent session, never at startup, and serves every session after it. Its
//! threads (`mcp-accept`, `mcp-conn`, `mcp-call`) never touch the UI: the permission gate and the invoker it is given
//! post to the UI and wait on their own thread.

use std::io;
use std::path::Path;
use std::sync::Arc;

use eludite_acp::protocol::{EnvVariable, McpServer as AcpMcpServer};
use eludite_commands::CommandRegistry;
use eludite_mcp::transport::{LocalEndpoint, TOKEN_ENV, listen_local};
use eludite_mcp::{AgentName, CallObserver, Invoker, McpServer, PermissionGate};

/// The MCP server name Eludite registers with agents. Claude prefixes tool names with it:
/// `mcp__eludite__diagnostics-list`.
pub const MCP_SERVER_NAME: &str = "eludite";

/// How the endpoint decides and runs calls (the Agents window's policy, pending changes and audit).
pub struct EndpointHooks {
    pub agent: AgentName,
    pub gate: PermissionGate,
    pub invoker: Option<Invoker>,
    pub observer: Option<CallObserver>,
}

#[derive(Debug, Clone)]
pub struct McpEndpoint {
    local: LocalEndpoint,
}

impl McpEndpoint {
    /// Bind the port and start serving. Returns once the socket is bound.
    pub fn start(commands: Arc<CommandRegistry>, hooks: EndpointHooks) -> io::Result<Self> {
        let mut server = McpServer::new(commands)
            .with_agent_name(hooks.agent)
            .with_permission_gate(hooks.gate);
        if let Some(i) = hooks.invoker {
            server = server.with_invoker(i);
        }
        if let Some(o) = hooks.observer {
            server = server.with_observer(o);
        }
        Ok(Self {
            local: listen_local(Arc::new(server))?,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn addr(&self) -> std::net::SocketAddr {
        self.local.addr
    }

    /// The stdio MCP server to pass in ACP `session/new`: `relay_exe --mcp-relay ADDR` with the token in its
    /// environment (never on its command line, where other local users could read it).
    pub fn acp_server(&self, relay_exe: &Path) -> AcpMcpServer {
        AcpMcpServer::Stdio {
            name: MCP_SERVER_NAME.into(),
            command: relay_exe.to_string_lossy().into_owned(),
            args: vec!["--mcp-relay".into(), self.local.addr.to_string()],
            env: vec![EnvVariable {
                name: TOKEN_ENV.into(),
                value: self.local.token.clone(),
            }],
        }
    }

    /// The text the window shows for it.
    pub fn describe(&self) -> String {
        format!("{MCP_SERVER_NAME} via stdio relay to {}", self.local.addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_commands::{builtins, diagnostics};
    use eludite_mcp::GateDecision;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn serves_the_bus_on_loopback_and_describes_the_relay() {
        let mut r = builtins::default_registry();
        diagnostics::register(&mut r, Arc::new(diagnostics::fixture)).unwrap();
        let e = McpEndpoint::start(
            Arc::new(r),
            EndpointHooks {
                agent: Arc::new(|| "Fake agent".into()),
                gate: Arc::new(|_, _, _| GateDecision::Deny("no".into())),
                invoker: None,
                observer: None,
            },
        )
        .unwrap();
        assert!(e.addr().ip().is_loopback());
        let AcpMcpServer::Stdio {
            name,
            command,
            args,
            env,
        } = e.acp_server(Path::new("/opt/eludite/eludite"))
        else {
            panic!("a stdio server")
        };
        assert_eq!(name, "eludite");
        assert_eq!(command, "/opt/eludite/eludite");
        assert_eq!(args, ["--mcp-relay".to_owned(), e.addr().to_string()]);
        assert_eq!(env[0].name, TOKEN_ENV);
        assert_eq!(env[0].value.len(), 32);
        assert!(!args.iter().any(|a| a.contains(&env[0].value)));
        // What the relay does: the token line, then MCP.
        let mut sock = std::net::TcpStream::connect(e.addr()).unwrap();
        writeln!(sock, "{}", env[0].value).unwrap();
        writeln!(sock, r#"{{"jsonrpc":"2.0","id":1,"method":"ping"}}"#).unwrap();
        let mut line = String::new();
        BufReader::new(sock).read_line(&mut line).unwrap();
        assert!(line.contains(r#""result":{}"#), "{line}");
        assert!(
            e.describe()
                .starts_with("eludite via stdio relay to 127.0.0.1:")
        );
    }
}
