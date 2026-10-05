//! The adapter's MCP client against `crates/mcp`'s real server: the IDE's local endpoint (`listen_local`, the token
//! on the first line) behind the stdio relay, serving the real command bus with `eludite.file.read` and
//! `eludite.file.edit` (brief 0059) over a temporary workspace, the real guides as resources, and the server's
//! default gate (anything above `read` is denied, as when the person says no).

mod acp;
mod fake_server;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use acp::{Adapter, RELAY, stop_reason, temp_dir};
use eludite_commands::files::{self, FilesTarget};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_mcp::McpServer;
use eludite_mcp::transport::listen_local;
use fake_server::{FakeServer, text_reply, tool_reply};
use serde_json::{Value, json};

struct Workspace(PathBuf);

impl FilesTarget for Workspace {
    fn workspace_root(&self) -> Option<PathBuf> {
        Some(self.0.clone())
    }
    fn open_text(&self, _: &Path) -> Option<String> {
        None
    }
    fn apply_edit(&self, _: Value) -> Result<Value, CommandError> {
        Err(CommandError::Failed("not in this test".into()))
    }
}

#[test]
fn the_client_speaks_to_the_real_mcp_server_through_the_relay() {
    let cwd = temp_dir("real-mcp");
    std::fs::write(cwd.join("notes.txt"), "The secret word is marmalade.\n").unwrap();
    let registry = Arc::new(CommandRegistry::new());
    files::register(&registry, Arc::new(Workspace(cwd.clone())));
    let endpoint = listen_local(Arc::new(
        McpServer::new(registry.clone()).with_agent("openai"),
    ))
    .unwrap();

    let server = FakeServer::start();
    server.llama_models(&["m"], 32_768);
    server.push(tool_reply(
        "r1",
        "eludite-file-read",
        r#"{"path": "notes.txt"}"#,
        10,
    ));
    server.push(tool_reply(
        "e1",
        "eludite-file-edit",
        r#"{"path": "notes.txt", "oldText": "marmalade", "newText": "jam"}"#,
        10,
    ));
    server.push(text_reply(
        "The word is marmalade; the edit was refused.",
        10,
        5,
    ));

    let mut a = Adapter::spawn(&["--base-url", &server.url], &[]);
    let relay = json!({"name": "eludite", "command": RELAY, "args": ["--mcp-relay", endpoint.addr.to_string()],
        "env": [{"name": "ELUDITE_MCP_TOKEN", "value": endpoint.token}]});
    let session = a.start(&cwd, vec![relay]);
    let resp = a.prompt(
        &session,
        "What is the secret word in notes.txt? Then change it to jam.",
    );
    assert_eq!(stop_reason(&resp), "end_turn");

    let chats = server.chats();
    let tools: Vec<&str> = chats[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        ["eludite-file-edit", "eludite-file-read", "eludite-tools"],
        "the core set of what is registered"
    );
    // The real guides, within the budget.
    let system = chats[0]["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("# Driving the debugger") && system.contains("# Using git"));
    assert!(system.len().div_ceil(4) < 4_000);
    // The read's output went back to the model.
    let read = chats[1]["messages"][3]["content"].as_str().unwrap();
    assert!(read.contains("1\\tThe secret word is marmalade."), "{read}");
    assert!(read.contains("\"source\":\"disk\""), "{read}");
    // The edit was refused by the gate, and the refusal is the tool's result.
    let edit = chats[2]["messages"][5]["content"].as_str().unwrap();
    assert!(
        edit.contains("permission denied") && edit.contains("no one is there to allow it"),
        "{edit}"
    );
    let failed = a
        .updates_of("tool_call_update")
        .into_iter()
        .any(|u| u["toolCallId"] == "e1" && u["status"] == "failed");
    assert!(failed);
    // The bus audited both calls as the agent, tied to the ACP tool call ids.
    let audit = serde_json::to_value(registry.audit_log().entries())
        .unwrap()
        .to_string();
    assert!(
        audit.contains("eludite.file.read") && audit.contains("eludite.file.edit"),
        "{audit}"
    );
    assert!(
        audit.contains("\"r1\"") && audit.contains("\"e1\""),
        "{audit}"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("notes.txt")).unwrap(),
        "The secret word is marmalade.\n"
    );
}
