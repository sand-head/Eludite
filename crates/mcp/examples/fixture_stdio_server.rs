//! Serve `diagnostics.list` over MCP stdio with the brief 0005 fixture Error
//! List, for attaching any MCP client (or an ACP agent's `mcpServers`) without
//! the IDE: `cargo run -p eludite-mcp --example fixture_stdio_server`.

use std::io;
use std::sync::Arc;

use eludite_commands::diagnostics::{self, DIAGNOSTICS_LIST};
use eludite_commands::{CommandId, CommandRegistry};
use eludite_mcp::McpServer;
use eludite_mcp::transport::serve_lines;

fn main() -> io::Result<()> {
    let mut registry = CommandRegistry::new();
    diagnostics::register(&mut registry, Arc::new(diagnostics::fixture)).expect("register");
    let id = CommandId::new(DIAGNOSTICS_LIST).expect("valid id");
    let server = McpServer::new(Arc::new(registry))
        .with_only([id])
        .with_observer(Arc::new(|r| {
            // stdout carries protocol only (CLAUDE.md invariant 10).
            eprintln!(
                "[audit] {} {:?} args={} ok={} {:?}",
                r.tool,
                r.permission,
                r.arguments,
                r.outcome.is_ok(),
                r.elapsed
            );
        }));
    serve_lines(&server, io::stdin().lock(), io::stdout().lock())
}
