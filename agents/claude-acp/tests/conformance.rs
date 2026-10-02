//! Conformance: the real adapter binary, driven by brief 0005's ACP client
//! (`eludite-acp`), against a fake `claude` replaying a recorded and redacted
//! real session (`tests/fixtures/claude-2.1.287-session.jsonl`).
//!
//! The recorded session has three turns: the diagnostics question (ToolSearch,
//! then the Eludite MCP tool, allowed), a Write (denied), and a long answer
//! interrupted by the client.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use eludite_acp::protocol::{
    EnvVariable, Implementation, McpServer, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest, SessionNotification, SessionUpdate, StopReason, ToolCall,
    ToolCallStatus,
};
use eludite_acp::{AcpClient, AgentDescriptor, ClientEvent, EventSink};
use serde_json::{Value, json};

const ADAPTER: &str = env!("CARGO_BIN_EXE_eludite-claude-acp");
const FAKE: &str = env!("CARGO_BIN_EXE_eludite-fake-claude");
const T: Option<Duration> = Some(Duration::from_secs(20));
const DIAG: &str = "mcp__eludite__diagnostics-list";

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A fresh directory (canonical, so `{{CWD}}` in the fixture matches what
/// the child sees as its working directory).
fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "eludite-claude-acp-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.canonicalize().unwrap()
}

/// What the test's "user" answers to a permission request.
type Policy = Arc<dyn Fn(&RequestPermissionRequest) -> Option<PermissionOptionKind> + Send + Sync>;

struct Harness {
    client: AcpClient,
    events: Receiver<ClientEvent>,
    log: PathBuf,
    cwd: PathBuf,
}

fn spawn(name: &str, env: Vec<(String, String)>, policy: Policy) -> Harness {
    spawn_cmd(name, ADAPTER.to_owned(), env, policy)
}

fn spawn_cmd(
    name: &str,
    command: String,
    mut env: Vec<(String, String)>,
    policy: Policy,
) -> Harness {
    let cwd = temp_dir(name);
    let log = cwd.join("fake-claude.log");
    env.push(("FAKE_CLAUDE_LOG".into(), log.to_string_lossy().into_owned()));
    // Claude Code's own session variables must not reach the child.
    env.push(("CLAUDECODE".into(), "1".into()));
    env.push(("CLAUDE_CODE_ENTRYPOINT".into(), "cli".into()));
    let agent = AgentDescriptor {
        name: "eludite-claude-acp".into(),
        command,
        args: vec![],
        env,
        env_remove: vec![],
    };
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let cell: Arc<OnceLock<AcpClient>> = Arc::default();
    let cell2 = cell.clone();
    let sink: EventSink = Arc::new(move |ev| {
        if let ClientEvent::PermissionRequest { id, request } = &ev
            && let Some(kind) = policy(request)
        {
            let opt = request
                .options
                .iter()
                .find(|o| o.kind == kind)
                .expect("option kind offered")
                .option_id
                .clone();
            cell2
                .get()
                .expect("client")
                .respond_permission(
                    id.clone(),
                    RequestPermissionOutcome::Selected { option_id: opt },
                )
                .unwrap();
        }
        let _ = tx.lock().unwrap().send(ev);
    });
    let client = AcpClient::spawn(&agent, &cwd, sink).expect("spawn adapter");
    let _ = cell.set(client.clone());
    Harness {
        client,
        events: rx,
        log,
        cwd,
    }
}

fn info() -> Implementation {
    Implementation {
        name: "eludite-test".into(),
        title: None,
        version: "0".into(),
    }
}

fn fake_env(fixture_name: &str) -> Vec<(String, String)> {
    vec![
        ("ELUDITE_CLAUDE_PATH".into(), FAKE.into()),
        ("FAKE_CLAUDE_FIXTURE".into(), fixture(fixture_name)),
    ]
}

fn eludite_server() -> McpServer {
    McpServer::Stdio {
        name: "eludite".into(),
        command: "/opt/eludite/eludite".into(),
        args: vec!["--mcp-relay".into(), "127.0.0.1:4000".into()],
        env: vec![EnvVariable {
            name: "ELUDITE_MCP_TOKEN".into(),
            value: "secret-token".into(),
        }],
    }
}

/// Events until the queue has been quiet for a moment.
fn drain(rx: &Receiver<ClientEvent>) -> Vec<ClientEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.recv_timeout(Duration::from_millis(300)) {
        out.push(ev);
    }
    out
}

fn updates(events: &[ClientEvent]) -> Vec<SessionUpdate> {
    events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::Update(SessionNotification { update, .. }) => Some(update.clone()),
            _ => None,
        })
        .collect()
}

fn message_text(events: &[ClientEvent]) -> String {
    updates(events)
        .iter()
        .filter_map(|u| match u {
            SessionUpdate::AgentMessageChunk(c) => c.as_text().map(str::to_owned),
            _ => None,
        })
        .collect()
}

fn tool_calls(events: &[ClientEvent]) -> Vec<ToolCall> {
    let mut calls: Vec<ToolCall> = Vec::new();
    for u in updates(events) {
        match u {
            SessionUpdate::ToolCall(t) => calls.push(t),
            SessionUpdate::ToolCallUpdate(u) => {
                let c = calls
                    .iter_mut()
                    .find(|c| c.tool_call_id == u.tool_call_id)
                    .expect("update for a known tool call");
                c.apply(u);
            }
            _ => {}
        }
    }
    calls
}

fn permission_requests(events: &[ClientEvent]) -> Vec<RequestPermissionRequest> {
    events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::PermissionRequest { request, .. } => Some(request.clone()),
            _ => None,
        })
        .collect()
}

/// The fake's log, once it has seen its stdin close (it outlives the
/// adapter by a moment after `shutdown`).
fn fake_log(path: &Path) -> Vec<Value> {
    let read = || -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    for _ in 0..50 {
        let log = read();
        if log.iter().any(|e| e["event"] == "eof") {
            return log;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    read()
}

fn read_policy() -> Policy {
    Arc::new(|req| {
        Some(if req.tool_call.agent_tool_name() == Some(DIAG) {
            PermissionOptionKind::AllowOnce
        } else {
            PermissionOptionKind::RejectOnce
        })
    })
}

#[test]
fn recorded_session_full_mapping_permissions_and_cancel() {
    let h = spawn(
        "full",
        fake_env("claude-2.1.287-session.jsonl"),
        read_policy(),
    );
    let init = h.client.initialize(info(), T).unwrap();
    assert_eq!(init.protocol_version, 1);
    let agent = init.agent_info.as_ref().unwrap();
    assert_eq!(agent.name, "eludite-claude-acp");
    assert!(init.agent_capabilities.mcp_capabilities.http);
    assert!(!init.agent_capabilities.mcp_capabilities.sse);
    assert!(!init.agent_capabilities.load_session);
    let login = &init.auth_methods[0];
    assert_eq!(login.kind.as_deref(), Some("terminal"));
    assert_eq!(login.args, ["auth", "login"]);

    let session = h
        .client
        .new_session(&h.cwd, vec![eludite_server()], T)
        .unwrap();
    let sid = session.session_id.clone();
    assert_eq!(
        sid.len(),
        36,
        "a UUID, also passed to claude as --session-id"
    );

    // Turn 1: ToolSearch, then the Eludite MCP tool (allowed by policy).
    let r = h
        .client
        .prompt(
            &sid,
            "List the current errors in the Error List and tell me which file has the most.",
        )
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    let ev = drain(&h.events);
    let perms = permission_requests(&ev);
    assert_eq!(perms.len(), 1);
    let p = &perms[0];
    assert_eq!(p.session_id, sid);
    assert_eq!(p.tool_call.agent_tool_name(), Some(DIAG));
    assert_eq!(
        p.tool_call
            .meta
            .as_ref()
            .unwrap()
            .pointer("/claudeCode/mcpServer/name"),
        Some(&json!("eludite"))
    );
    assert_eq!(p.tool_call.raw_input, Some(json!({"severity": "error"})));
    let kinds: Vec<_> = p.options.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways
        ]
    );
    let calls = tool_calls(&ev);
    assert_eq!(calls.len(), 2, "{calls:#?}");
    assert_eq!(calls[0].agent_tool_name(), Some("ToolSearch"));
    assert_eq!(calls[0].status, Some(ToolCallStatus::Completed));
    assert_eq!(
        calls[0].content_text(),
        "Tool: mcp__eludite__diagnostics-list"
    );
    assert_eq!(calls[1].agent_tool_name(), Some(DIAG));
    assert_eq!(calls[1].kind.as_deref(), Some("other"));
    assert_eq!(calls[1].title.as_deref(), Some(DIAG));
    assert_eq!(calls[1].status, Some(ToolCallStatus::Completed));
    assert_eq!(calls[1].raw_input, Some(json!({"severity": "error"})));
    assert!(calls[1].content_text().contains("CS0103"));
    assert!(
        calls[1]
            .raw_output
            .as_ref()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("PricingService.cs")
    );
    let text = message_text(&ev);
    assert!(
        text.starts_with("I'll pull the IDE's diagnostics list and tally errors by file."),
        "{text}"
    );
    assert!(
        text.contains(
            "`src/Contoso.Web/Controllers/OrderController.cs` has the most, with 3 of them."
        ),
        "{text}"
    );
    // Thinking in this recording is redacted to "", so no thought chunks.
    assert!(
        !updates(&ev)
            .iter()
            .any(|u| matches!(u, SessionUpdate::AgentThoughtChunk(_)))
    );

    // Turn 2: Write, denied by the user.
    let r = h
        .client
        .prompt(
            &sid,
            "Create a file named notes.txt in the current directory containing the word hello.",
        )
        .unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    let ev = drain(&h.events);
    let perms = permission_requests(&ev);
    assert_eq!(perms.len(), 1);
    let w = &perms[0].tool_call;
    assert_eq!(w.agent_tool_name(), Some("Write"));
    assert_eq!(w.title.as_deref(), Some("Write notes.txt"));
    assert_eq!(w.kind.as_deref(), Some("edit"));
    assert_eq!(
        perms[0].options[1].name,
        "Yes, allow all edits during this session"
    );
    let notes = h.cwd.join("notes.txt");
    let diff = &w.content.as_ref().unwrap()[0];
    assert_eq!(diff["type"], "diff");
    assert_eq!(diff["path"], json!(notes));
    assert_eq!(diff["newText"], "hello\n");
    let calls = tool_calls(&ev);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].status, Some(ToolCallStatus::Failed));
    assert_eq!(calls[0].title.as_deref(), Some("Write notes.txt"));
    assert!(message_text(&ev).starts_with("The write to notes.txt was declined"));
    assert!(!notes.exists());

    // Turn 3: cancelled after the first chunk.
    let (client, sid3) = (h.client.clone(), sid.clone());
    let turn = std::thread::spawn(move || {
        client.prompt(
            &sid3,
            "Count from 1 to 300, one number per line, with no other text.",
        )
    });
    loop {
        match h
            .events
            .recv_timeout(Duration::from_secs(10))
            .expect("first chunk")
        {
            ClientEvent::Update(SessionNotification {
                update: SessionUpdate::AgentMessageChunk(_),
                ..
            }) => break,
            _ => continue,
        }
    }
    h.client.cancel(&sid).unwrap();
    assert_eq!(
        turn.join().unwrap().unwrap().stop_reason,
        StopReason::Cancelled
    );
    drain(&h.events);

    // The child side, as the fake recorded it.
    h.client.shutdown();
    let log = fake_log(&h.log);
    assert!(log.iter().all(|e| e["event"] != "mismatch"), "{log:#?}");
    let start = log.iter().find(|e| e["event"] == "start").unwrap();
    let argv: Vec<&str> = start["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for flag in [
        "--print",
        "--include-partial-messages",
        "--replay-user-messages",
        "--verbose",
        "--strict-mcp-config",
    ] {
        assert!(argv.contains(&flag), "{flag} in {argv:?}");
    }
    let pair = |f: &str| argv[argv.iter().position(|a| *a == f).unwrap() + 1];
    assert_eq!(pair("--input-format"), "stream-json");
    assert_eq!(pair("--output-format"), "stream-json");
    assert_eq!(pair("--permission-prompt-tool"), "stdio");
    assert_eq!(pair("--permission-mode"), "default");
    assert_eq!(pair("--session-id"), sid);
    assert!(!argv.contains(&"--model"), "no model is hard-coded");
    assert_eq!(start["cwd"], json!(h.cwd));
    assert_eq!(start["leaked_session_env"], json!([]));
    assert_eq!(
        start["mcp_config"],
        json!({"mcpServers": {"eludite": {"type": "stdio", "command": "/opt/eludite/eludite",
            "args": ["--mcp-relay", "127.0.0.1:4000"], "env": {"ELUDITE_MCP_TOKEN": "secret-token"}}}})
    );
    let inputs: Vec<&Value> = log
        .iter()
        .filter(|e| e["event"] == "input")
        .map(|e| &e["m"])
        .collect();
    let behaviors: Vec<&str> = inputs
        .iter()
        .filter(|m| m["type"] == "control_response")
        .map(|m| m["response"]["response"]["behavior"].as_str().unwrap())
        .collect();
    assert_eq!(behaviors, ["allow", "deny"]);
    let controls: Vec<&str> = inputs
        .iter()
        .filter(|m| m["type"] == "control_request")
        .map(|m| m["request"]["subtype"].as_str().unwrap())
        .collect();
    assert_eq!(controls, ["initialize", "interrupt"]);
    let users: Vec<&Value> = inputs
        .iter()
        .copied()
        .filter(|m| m["type"] == "user")
        .collect();
    assert_eq!(users.len(), 3);
    assert_eq!(users[0]["session_id"], json!(sid));
    assert_eq!(users[0]["message"]["content"][0]["type"], "text");
    // The temporary MCP config is removed with the session.
    let config = std::env::temp_dir().join(format!("eludite-claude-acp-{sid}.mcp.json"));
    for _ in 0..50 {
        if !config.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !config.exists(),
        "{config:?} removed after the client disconnects"
    );
}

#[test]
fn allow_always_and_reject_always_carry_updated_permissions() {
    let policy: Policy = Arc::new(|req| {
        Some(if req.tool_call.agent_tool_name() == Some(DIAG) {
            PermissionOptionKind::AllowAlways
        } else {
            PermissionOptionKind::RejectAlways
        })
    });
    let h = spawn("always", fake_env("claude-2.1.287-session.jsonl"), policy);
    h.client.initialize(info(), T).unwrap();
    let sid = h.client.new_session(&h.cwd, vec![], T).unwrap().session_id;
    h.client.prompt(&sid, "diagnostics").unwrap();
    h.client.prompt(&sid, "write").unwrap();
    drain(&h.events);
    h.client.shutdown();
    let log = fake_log(&h.log);
    let answers: Vec<&Value> = log
        .iter()
        .filter(|e| e["event"] == "input" && e["m"]["type"] == "control_response")
        .map(|e| &e["m"]["response"]["response"])
        .collect();
    assert_eq!(answers[0]["behavior"], "allow");
    assert_eq!(answers[0]["updatedInput"], json!({"severity": "error"}));
    // The child's own suggestion is passed back.
    assert_eq!(
        answers[0]["updatedPermissions"],
        json!([{"type": "addRules", "rules": [{"toolName": DIAG}], "behavior": "allow", "destination": "localSettings"}])
    );
    assert_eq!(answers[1]["behavior"], "deny");
    assert_eq!(
        answers[1]["updatedPermissions"],
        json!([{"type": "addRules", "rules": [{"toolName": "Write"}], "behavior": "deny", "destination": "session"}])
    );
}

#[test]
fn logged_out_claude_is_auth_required() {
    let h = spawn(
        "loggedout",
        fake_env("claude-2.1.287-logged-out.jsonl"),
        read_policy(),
    );
    let init = h.client.initialize(info(), T).unwrap();
    let sid = h.client.new_session(&h.cwd, vec![], T).unwrap().session_id;
    let err = h.client.prompt(&sid, "Say hi").unwrap_err();
    assert!(err.is_auth_required(), "{err}");
    // The panel shows the login method as "<adapter> auth login".
    let agent = AgentDescriptor {
        name: "Claude Code".into(),
        command: "eludite-claude-acp".into(),
        args: vec![],
        env: vec![],
        env_remove: vec![],
    };
    assert_eq!(
        agent.terminal_auth_command(&init.auth_methods[0]),
        "eludite-claude-acp auth login"
    );
    // The synthetic "Not logged in" text is not streamed as an answer.
    assert_eq!(message_text(&drain(&h.events)), "");
    h.client.shutdown();
}

#[test]
fn refuses_claude_older_than_validated() {
    let mut env = fake_env("claude-2.1.287-session.jsonl");
    env.push(("FAKE_CLAUDE_VERSION".into(), "2.1.200 (Claude Code)".into()));
    let h = spawn("old", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    let err = h.client.new_session(&h.cwd, vec![], T).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("2.1.200") && msg.contains("older than 2.1.287"),
        "{msg}"
    );
    h.client.shutdown();
    assert!(
        !fake_log(&h.log).iter().any(|e| e["event"] == "start"),
        "no session child is started"
    );
}

#[test]
fn missing_claude_is_a_clear_error() {
    let empty = temp_dir("missing-path");
    let env = vec![
        ("PATH".into(), empty.to_string_lossy().into_owned()),
        ("HOME".into(), empty.to_string_lossy().into_owned()),
    ];
    let h = spawn("missing", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    let msg = h
        .client
        .new_session(&h.cwd, vec![], T)
        .unwrap_err()
        .to_string();
    assert!(msg.contains("was not found"), "{msg}");
    h.client.shutdown();
}

#[test]
fn model_comes_from_the_environment_never_hard_coded() {
    let mut env = fake_env("claude-2.1.287-session.jsonl");
    env.push(("ELUDITE_CLAUDE_MODEL".into(), "sonnet".into()));
    let h = spawn("model", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    h.client.new_session(&h.cwd, vec![], T).unwrap();
    h.client.shutdown();
    let log = fake_log(&h.log);
    let argv = &log.iter().find(|e| e["event"] == "start").unwrap()["argv"];
    let argv: Vec<&str> = argv
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let i = argv
        .iter()
        .position(|a| *a == "--model")
        .expect("--model passed");
    assert_eq!(argv[i + 1], "sonnet");
}

/// The no-Node proof, automated: PATH is one directory holding only a
/// `claude` symlink (to the fake). The adapter finds it there and runs a full
/// turn; `node`, `npm` and `npx` cannot be found.
#[cfg(unix)]
#[test]
fn runs_with_only_claude_on_path() {
    let bin = temp_dir("only-claude-bin");
    std::os::unix::fs::symlink(FAKE, bin.join("claude")).unwrap();
    for tool in ["node", "npm", "npx"] {
        assert!(!bin.join(tool).exists());
    }
    let env = vec![
        ("PATH".into(), bin.to_string_lossy().into_owned()),
        ("HOME".into(), bin.to_string_lossy().into_owned()),
        (
            "FAKE_CLAUDE_FIXTURE".into(),
            fixture("claude-2.1.287-session.jsonl"),
        ),
    ];
    let h = spawn("only-claude", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    let sid = h
        .client
        .new_session(&h.cwd, vec![eludite_server()], T)
        .unwrap()
        .session_id;
    let r = h.client.prompt(&sid, "diagnostics").unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    assert!(message_text(&drain(&h.events)).contains("has the most"));
    h.client.shutdown();
    let log = fake_log(&h.log);
    let start = log.iter().find(|e| e["event"] == "start").unwrap();
    assert_eq!(start["exe"], json!(bin.join("claude")));
    assert_eq!(start["path"], json!(bin));
}

/// `session/cancel` while a permission request is pending: the adapter sends
/// the interrupt, the client answers the request `cancelled` (as ACP
/// requires), the child gets a deny with `interrupt: true`, and the prompt
/// ends `cancelled`.
#[test]
fn cancel_while_permission_pending() {
    let policy: Policy = Arc::new(|req| {
        (req.tool_call.agent_tool_name() == Some(DIAG)).then_some(PermissionOptionKind::AllowOnce)
    });
    let mut env = fake_env("claude-2.1.287-session.jsonl");
    // The interrupt arrives where the recording has the Write answer.
    env.push(("FAKE_CLAUDE_LENIENT".into(), "1".into()));
    let h = spawn("cancel-pending", env, policy);
    h.client.initialize(info(), T).unwrap();
    let sid = h.client.new_session(&h.cwd, vec![], T).unwrap().session_id;
    h.client.prompt(&sid, "diagnostics").unwrap();
    drain(&h.events);

    let (client, sid2) = (h.client.clone(), sid.clone());
    let turn = std::thread::spawn(move || client.prompt(&sid2, "write"));
    let pending_id = loop {
        match h
            .events
            .recv_timeout(Duration::from_secs(10))
            .expect("permission request")
        {
            ClientEvent::PermissionRequest { id, request } => {
                assert_eq!(request.tool_call.agent_tool_name(), Some("Write"));
                break id;
            }
            _ => continue,
        }
    };
    h.client.cancel(&sid).unwrap();
    h.client
        .respond_permission(pending_id, RequestPermissionOutcome::Cancelled)
        .unwrap();
    assert_eq!(
        turn.join().unwrap().unwrap().stop_reason,
        StopReason::Cancelled
    );
    h.client.shutdown();

    let log = fake_log(&h.log);
    let inputs: Vec<&Value> = log
        .iter()
        .filter(|e| e["event"] == "input")
        .map(|e| &e["m"])
        .collect();
    let interrupt = inputs
        .iter()
        .position(|m| m["type"] == "control_request" && m["request"]["subtype"] == "interrupt")
        .expect("interrupt sent");
    let answer = inputs
        .iter()
        .rposition(|m| m["type"] == "control_response")
        .expect("Write answered");
    assert!(interrupt < answer);
    assert_eq!(
        inputs[answer]["response"]["response"],
        json!({"behavior": "deny", "message": "The prompt was cancelled", "interrupt": true})
    );
}
