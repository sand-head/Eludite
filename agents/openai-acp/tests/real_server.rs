//! Against a real OpenAI-compatible server, skipped without `ELUDITE_LLAMA_SERVER=URL` (the API root, such as
//! `http://localhost:8080/v1`; `ELUDITE_LLAMA_MODEL` picks the model, else the server's first). The model must call
//! `eludite-file-read` on a file the test wrote and quote a word from it: the test asserts the call and the word,
//! not the prose. `ELUDITE_OPENAI_API_KEY` is passed through when set.

mod acp;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use acp::{Adapter, RELAY, stop_reason, temp_dir};
use eludite_commands::files::{self, FilesTarget};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_mcp::transport::listen_local;
use eludite_mcp::{McpServer, ToolCallRecord};
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
        Err(CommandError::Failed("read only in this test".into()))
    }
}

#[test]
fn a_real_model_reads_a_file_through_the_ide_and_quotes_it() {
    let Some(url) = std::env::var("ELUDITE_LLAMA_SERVER")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!(
            "skipped: set ELUDITE_LLAMA_SERVER to an OpenAI-compatible API root (http://localhost:8080/v1)"
        );
        return;
    };
    let cwd = temp_dir("real-server");
    std::fs::write(
        cwd.join("notes.txt"),
        "Project notes.\nThe codename of this release is PERIWINKLE.\n",
    )
    .unwrap();
    let registry = Arc::new(CommandRegistry::new());
    files::register(&registry, Arc::new(Workspace(cwd.clone())));
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = calls.clone();
    let mcp = McpServer::new(registry)
        .with_agent("openai")
        .with_observer(Arc::new(move |r: &ToolCallRecord| {
            seen.lock().unwrap().push(r.tool.clone())
        }));
    let endpoint = listen_local(Arc::new(mcp)).unwrap();

    let mut args = vec!["--base-url".to_owned(), url];
    if let Ok(m) = std::env::var("ELUDITE_LLAMA_MODEL") {
        args.extend(["--model".to_owned(), m]);
    }
    let key = std::env::var("ELUDITE_OPENAI_API_KEY").unwrap_or_default();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut a = Adapter::spawn(&args, &[("ELUDITE_OPENAI_API_KEY", &key)]);
    let relay = json!({"name": "eludite", "command": RELAY, "args": ["--mcp-relay", endpoint.addr.to_string()],
        "env": [{"name": "ELUDITE_MCP_TOKEN", "value": endpoint.token}]});
    let session = a.start(&cwd, vec![relay]);
    let resp = a.prompt(
        &session,
        "Use the eludite-file-read tool to read notes.txt, then tell me the codename of this release.",
    );
    assert_eq!(stop_reason(&resp), "end_turn", "{resp}");
    let calls = calls.lock().unwrap().clone();
    assert!(
        calls.iter().any(|c| c == "eludite-file-read"),
        "the model did not read the file: {calls:?}"
    );
    let text = a.message_text();
    assert!(text.to_uppercase().contains("PERIWINKLE"), "{text}");
    let usage = a.updates_of("usage_update");
    eprintln!(
        "tool calls: {calls:?}; requests: {}; usage: {}",
        usage.len(),
        usage.last().unwrap_or(&Value::Null)
    );
}
