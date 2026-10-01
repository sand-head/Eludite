//! `eludite --mcp-relay ADDR` (brief 0016): the stdio MCP server Eludite hands a hosted agent pipes MCP to the IDE's
//! token-checked loopback endpoint, opens no window, and refuses to run without the token.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;

use eludite_commands::builtins;
use eludite_commands::diagnostics;
use eludite_mcp::McpServer;
use eludite_mcp::transport::{TOKEN_ENV, listen_local};
use serde_json::{Value, json};

fn endpoint() -> eludite_mcp::transport::LocalEndpoint {
    let mut r = builtins::default_registry();
    diagnostics::register(&mut r, Arc::new(diagnostics::fixture)).unwrap();
    listen_local(Arc::new(McpServer::new(Arc::new(r)))).unwrap()
}

fn relay(addr: &str, token: Option<&str>) -> std::process::Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_eludite"));
    cmd.args(["--mcp-relay", addr])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove(TOKEN_ENV);
    if let Some(t) = token {
        cmd.env(TOKEN_ENV, t);
    }
    cmd.spawn().unwrap()
}

#[test]
fn relays_mcp_to_the_endpoint() {
    let e = endpoint();
    let mut child = relay(&e.addr.to_string(), Some(&e.token));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |msg: Value| -> Value {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let init = ask(
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}}),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "eludite");
    let list = ask(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"diagnostics-list"), "{names:?}");
    let out = ask(
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "diagnostics-list", "arguments": {"severity": "error"}}}),
    );
    assert_eq!(
        out["result"]["structuredContent"]["result"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn refuses_without_the_token_and_with_a_wrong_one() {
    let e = endpoint();
    let out = relay(&e.addr.to_string(), None).wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains(TOKEN_ENV));
    assert!(out.stdout.is_empty());

    let mut child = relay(&e.addr.to_string(), Some("0123"));
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.stdout.is_empty(),
        "the endpoint hangs up on a wrong token"
    );
}
