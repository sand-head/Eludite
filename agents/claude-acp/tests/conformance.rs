//! Conformance: the real adapter binary, driven by brief 0005's ACP client
//! (`eludite-acp`), against a fake `claude` replaying a recorded and redacted
//! real session (`tests/fixtures/claude-2.1.287-session.jsonl`).
//!
//! Brief 0057: `claude-2.1.289-commands.jsonl` (no prompt; the `initialize`
//! reply with Claude Code's built-in slash commands) proves the adapter sends
//! one `available_commands_update` right after `session/new`.
//!
//! Brief 0058: `claude-2.1.289-options.jsonl` (no model call: the
//! `initialize` reply with `claude`'s models, `set_permission_mode` to `plan`
//! and back, `set_model` to `opus`, the local command `/effort high`) proves
//! `session/new`'s modes and config options, `session/set_mode` and
//! `session/set_config_option`, their notifications, and that the effort's
//! local command is not a turn. The ACP side is pinned in
//! `claude-2.1.289-options.acp.jsonl` (`UPDATE_GOLDEN=1` rewrites it).
//!
//! Brief 0061: `claude-2.1.289-resume.jsonl` holds three `claude` processes
//! (no model call): a new session that runs the local command `/effort low`
//! (which writes Claude Code's session file), `--resume` of that session (its
//! own `initialize`), and `--resume` of an id no session has (refused). With
//! `claude-2.1.289-session-file.jsonl` as Claude Code's session file, it
//! proves `session/load`: the replay before the answer, `--resume` in the
//! child's argv, an id already live refused, and the refusal's message.
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
use eludite_acp::{AcpClient, AcpError, AgentDescriptor, ClientEvent, EventSink};
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
    // Long names, without the verbatim `\\?\` prefix canonicalize adds on Windows (claude reports plain paths).
    let d = d.canonicalize().unwrap();
    match d.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) => PathBuf::from(plain),
        None => d,
    }
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
    // Brief 0061: the adapter resumes sessions (`session/load` over `claude --resume`).
    assert!(init.agent_capabilities.load_session);
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
    // Compared as paths: the recorded path joins with `/`.
    assert_eq!(Path::new(diff["path"].as_str().unwrap()), notes);
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
        // The home fallback (~/.local/bin) reads USERPROFILE on Windows.
        ("USERPROFILE".into(), empty.to_string_lossy().into_owned()),
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

#[test]
fn recorded_commands_follow_session_new() {
    let mut env = fake_env("claude-2.1.289-commands.jsonl");
    env.push(("FAKE_CLAUDE_VERSION".into(), "2.1.289 (Claude Code)".into()));
    let h = spawn("commands", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    // Nothing arrives before the session exists.
    assert!(updates(&drain(&h.events)).is_empty());
    let session = h
        .client
        .new_session(&h.cwd, vec![eludite_server()], T)
        .unwrap();
    let ev = drain(&h.events);
    let lists: Vec<(String, Vec<String>)> = ev
        .iter()
        .filter_map(|e| match e {
            ClientEvent::Update(SessionNotification { session_id, update }) => update
                .available_commands()
                .map(|c| (session_id.clone(), c.into_iter().map(|c| c.name).collect())),
            _ => None,
        })
        .collect();
    assert_eq!(lists.len(), 1, "one list: {lists:?}");
    let (sid, names) = &lists[0];
    assert_eq!(sid, &session.session_id);
    for want in ["model", "effort", "compact", "clear", "context"] {
        assert!(names.iter().any(|n| n == want), "{want} in {names:?}");
    }
    assert!(!names.iter().any(|n| n.starts_with("__")), "{names:?}");
    let commands = updates(&ev)
        .iter()
        .find_map(SessionUpdate::available_commands)
        .unwrap();
    let compact = commands.iter().find(|c| c.name == "compact").unwrap();
    assert_eq!(
        compact.hint(),
        Some("<optional custom summarization instructions>")
    );
    let context = commands.iter().find(|c| c.name == "context").unwrap();
    assert_eq!(context.hint(), None);
    // The replay matched: the fake saw only the initialize request, no mismatch.
    h.client.shutdown();
    let log = fake_log(&h.log);
    assert!(!log.iter().any(|e| e["event"] == "mismatch"), "{log:?}");
}

/// A JSON-RPC error as the agent sent it (code, message, data).
fn rpc_error(e: &AcpError) -> Value {
    match e {
        AcpError::Rpc(o) => serde_json::to_value(o).unwrap(),
        other => panic!("not an agent error: {other}"),
    }
}

/// Write or compare the golden `name` (one JSON value per line).
fn check_golden(name: &str, got: &[Value]) {
    let rendered: String = got.iter().map(|v| format!("{v}\n")).collect();
    let path = fixture(name);
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::write(&path, &rendered).unwrap();
    }
    let want = std::fs::read_to_string(&path).expect("golden file (UPDATE_GOLDEN=1 to create)");
    for (i, (g, w)) in rendered.lines().zip(want.lines()).enumerate() {
        assert_eq!(g, w, "line {} of {name}", i + 1);
    }
    assert_eq!(rendered.lines().count(), want.lines().count());
}

/// The option `id` of a `configOptions` list.
fn config_option<'a>(options: &'a Value, id: &str) -> &'a Value {
    options
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == id)
        .unwrap_or_else(|| panic!("no option {id} in {options}"))
}

fn choice_values(option: &Value) -> Vec<&str> {
    option["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect()
}

/// The updates of `kind` among `events`, raw, with the session id.
fn raw_updates(events: &[ClientEvent], kind: &str) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::Update(SessionNotification {
                session_id,
                update: SessionUpdate::Other { kind: k, raw },
            }) if k == kind => Some((session_id.clone(), raw.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn recorded_options_follow_mode_model_and_effort_changes() {
    let mut env = fake_env("claude-2.1.289-options.jsonl");
    env.push(("FAKE_CLAUDE_VERSION".into(), "2.1.289 (Claude Code)".into()));
    let h = spawn("options", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    let mut golden = Vec::new();
    // session/new: the two modes, the model and the effort options.
    let new = h
        .client
        .call("session/new", json!({"cwd": h.cwd, "mcpServers": []}), T)
        .unwrap();
    let sid = new["sessionId"].as_str().unwrap().to_owned();
    assert_eq!(
        new["modes"],
        json!({"currentModeId": "default", "availableModes": [
            {"id": "default", "name": "Manual", "description": "Eludite reviews each edit and prompts as the policy says"},
            {"id": "plan", "name": "Plan", "description": "Plans before making changes"}
        ]})
    );
    let model = config_option(&new["configOptions"], "model");
    assert_eq!(
        (&model["category"], &model["currentValue"]),
        (&json!("model"), &json!("default"))
    );
    let values = choice_values(model);
    assert_eq!(values.len(), 12, "{values:?}");
    assert_eq!(
        &values[..5],
        ["default", "opus", "fable", "sonnet", "haiku"]
    );
    assert_eq!(model["options"][1]["name"], "Opus 5.5");
    assert_eq!(model["options"][0]["description"], "Fable 5.1");
    let effort = config_option(&new["configOptions"], "effort");
    assert_eq!(
        (&effort["category"], &effort["currentValue"]),
        (&json!("thought_level"), &json!("default"))
    );
    assert_eq!(
        choice_values(effort),
        ["default", "low", "medium", "high", "xhigh", "max"]
    );
    golden.push(
        json!({"session/new": {"modes": new["modes"], "configOptions": new["configOptions"]}}),
    );
    let ev = drain(&h.events);
    assert_eq!(raw_updates(&ev, "available_commands_update").len(), 1);

    // set_mode: plan and back, each answered and notified; a mode not offered is refused with nothing sent.
    let set_mode = |mode: &str| {
        h.client.call(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": mode}),
            T,
        )
    };
    for mode in ["plan", "default"] {
        let answer = set_mode(mode).unwrap();
        assert_eq!(answer, json!({}));
        let ev = drain(&h.events);
        let notified = raw_updates(&ev, "current_mode_update");
        assert_eq!(
            notified,
            [(
                sid.clone(),
                json!({"sessionUpdate": "current_mode_update", "currentModeId": mode})
            )]
        );
        golden.push(json!({"session/set_mode": mode, "answer": answer, "notified": notified[0].1}));
    }
    let refused = rpc_error(&set_mode("bypassPermissions").unwrap_err());
    assert_eq!(refused["code"], -32602);
    assert_eq!(refused["data"], "unknown mode bypassPermissions");
    golden.push(json!({"session/set_mode": "bypassPermissions", "error": refused}));

    // set_config_option: the model, then the effort; each answers every option and notifies the same list.
    let set_option = |id: &str, value: &str| {
        h.client.call(
            "session/set_config_option",
            json!({"sessionId": sid, "configId": id, "value": value}),
            T,
        )
    };
    for (id, value) in [("model", "opus"), ("effort", "high")] {
        let started = std::time::Instant::now();
        let answer = set_option(id, value).unwrap();
        let took = started.elapsed();
        assert_budget(
            &format!("set_config_option {id}"),
            took,
            Duration::from_secs(2),
        );
        assert_eq!(
            config_option(&answer["configOptions"], id)["currentValue"],
            value
        );
        let ev = drain(&h.events);
        let notified = raw_updates(&ev, "config_option_update");
        assert_eq!(notified.len(), 1, "{notified:?}");
        assert_eq!(notified[0].1["configOptions"], answer["configOptions"]);
        // The effort's local command (`/effort high`, answered by a synthetic assistant message) is not a turn.
        assert_eq!(message_text(&ev), "", "{id}");
        assert!(
            !updates(&ev).iter().any(|u| matches!(
                u,
                SessionUpdate::AgentMessageChunk(_) | SessionUpdate::AgentThoughtChunk(_)
            )),
            "{id}"
        );
        assert!(raw_updates(&ev, "usage_update").is_empty(), "{id}");
        golden.push(json!({"session/set_config_option": {id: value}, "answer": answer, "notified": notified[0].1}));
    }
    // Opus supports effort: after the model change the effort stayed, with Opus's levels.
    let answer = &golden[golden.len() - 1]["answer"]["configOptions"];
    assert_eq!(config_option(answer, "model")["currentValue"], "opus");
    // `default` cannot be set (claude has no way back to adaptive effort in a session); neither can an unknown
    // value or option.
    for (id, value) in [
        ("effort", "default"),
        ("effort", "huge"),
        ("speed", "1"),
        ("model", "nope"),
    ] {
        let e = rpc_error(&set_option(id, value).unwrap_err());
        assert_eq!(e["code"], -32602, "{id}={value}: {e}");
        golden.push(json!({"session/set_config_option": {id: value}, "error": e}));
    }
    // A refusal notifies nothing.
    assert!(updates(&drain(&h.events)).is_empty());
    check_golden("claude-2.1.289-options.acp.jsonl", &golden);

    // The child side: the recorded controls in order, one user message (the local command), no mismatch.
    h.client.shutdown();
    let log = fake_log(&h.log);
    assert!(!log.iter().any(|e| e["event"] == "mismatch"), "{log:?}");
    let inputs: Vec<&Value> = log
        .iter()
        .filter(|e| e["event"] == "input")
        .map(|e| &e["m"])
        .collect();
    let controls: Vec<(&str, &Value)> = inputs
        .iter()
        .filter(|m| m["type"] == "control_request")
        .map(|m| (m["request"]["subtype"].as_str().unwrap(), &m["request"]))
        .collect();
    let subtypes: Vec<&str> = controls.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        subtypes,
        [
            "initialize",
            "set_permission_mode",
            "set_permission_mode",
            "set_model"
        ]
    );
    assert_eq!(controls[1].1["mode"], "plan");
    assert_eq!(controls[2].1["mode"], "default");
    assert_eq!(controls[3].1["model"], "opus");
    let users: Vec<&Value> = inputs
        .iter()
        .copied()
        .filter(|m| m["type"] == "user")
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["message"]["content"][0]["text"], "/effort high");
}

/// A change during a turn is refused (`in_turn`) and nothing reaches `claude`.
#[test]
fn a_change_during_a_turn_is_refused_without_writing_to_claude() {
    let env = vec![
        ("ELUDITE_CLAUDE_PATH".into(), FAKE.into()),
        ("FAKE_CLAUDE_SCENARIO".into(), "stream".into()),
        ("FAKE_CLAUDE_CHUNKS".into(), "300".into()),
        ("FAKE_CLAUDE_RATE".into(), "100".into()),
    ];
    let h = spawn("in-turn", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    let sid = h.client.new_session(&h.cwd, vec![], T).unwrap().session_id;
    drain(&h.events);
    let (client, sid2) = (h.client.clone(), sid.clone());
    let turn = std::thread::spawn(move || client.prompt(&sid2, "stream"));
    loop {
        if let ClientEvent::Update(SessionNotification {
            update: SessionUpdate::AgentMessageChunk(_),
            ..
        }) = h
            .events
            .recv_timeout(Duration::from_secs(10))
            .expect("a chunk")
        {
            break;
        }
    }
    for (method, params) in [
        (
            "session/set_config_option",
            json!({"sessionId": sid, "configId": "model", "value": "opus"}),
        ),
        (
            "session/set_config_option",
            json!({"sessionId": sid, "configId": "effort", "value": "high"}),
        ),
        (
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "plan"}),
        ),
    ] {
        let e = rpc_error(&h.client.call(method, params, T).unwrap_err());
        assert!(
            e["data"].as_str().unwrap().starts_with("in_turn"),
            "{method}: {e}"
        );
    }
    h.client.cancel(&sid).unwrap();
    turn.join().unwrap().unwrap();
    h.client.shutdown();
    let log = fake_log(&h.log);
    let inputs: Vec<&Value> = log
        .iter()
        .filter(|e| e["event"] == "input")
        .map(|e| &e["m"])
        .collect();
    let controls: Vec<&str> = inputs
        .iter()
        .filter(|m| m["type"] == "control_request")
        .map(|m| m["request"]["subtype"].as_str().unwrap())
        .collect();
    assert_eq!(
        controls,
        ["initialize", "interrupt"],
        "only the turn's own traffic"
    );
    assert_eq!(inputs.iter().filter(|m| m["type"] == "user").count(), 1);
}

#[test]
fn effort_comes_from_the_environment_or_the_session_meta() {
    let argv_of = |env: Vec<(String, String)>, meta: Option<Value>| {
        let mut env = env;
        env.extend(fake_env("claude-2.1.289-commands.jsonl"));
        env.push(("FAKE_CLAUDE_VERSION".into(), "2.1.289 (Claude Code)".into()));
        let h = spawn("effort", env, read_policy());
        h.client.initialize(info(), T).unwrap();
        h.client
            .new_session_with_meta(&h.cwd, vec![], meta, T)
            .unwrap();
        h.client.shutdown();
        let log = fake_log(&h.log);
        log.iter().find(|e| e["event"] == "start").unwrap()["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let after = |argv: &[String], flag: &str| {
        argv.iter()
            .position(|a| a == flag)
            .map(|i| argv[i + 1].clone())
    };
    let argv = argv_of(vec![("ELUDITE_CLAUDE_EFFORT".into(), "high".into())], None);
    assert_eq!(after(&argv, "--effort").as_deref(), Some("high"));
    // The client's remembered choice, in the Node adapter's key.
    let meta = json!({"claudeCode": {"options": {"model": "opus", "effort": "max"}}});
    let argv = argv_of(vec![], Some(meta));
    assert_eq!(after(&argv, "--effort").as_deref(), Some("max"));
    assert_eq!(after(&argv, "--model").as_deref(), Some("opus"));
    // `default` and unknown levels pass nothing.
    let meta = json!({"claudeCode": {"options": {"model": "default", "effort": "huge"}}});
    let argv = argv_of(vec![], Some(meta));
    assert_eq!(after(&argv, "--effort"), None);
    assert_eq!(after(&argv, "--model"), None);
    let argv = argv_of(vec![], None);
    assert_eq!(after(&argv, "--effort"), None);
}

/// `/effort high` typed as a prompt (the same recorded exchange, reached through `session/prompt`): the person sees
/// Claude Code's reply as the turn's text, and the effort option follows in a `config_option_update`.
#[test]
fn a_typed_local_command_keeps_its_reply_and_moves_the_option() {
    let mut env = fake_env("claude-2.1.289-options.jsonl");
    env.push(("FAKE_CLAUDE_VERSION".into(), "2.1.289 (Claude Code)".into()));
    let h = spawn("typed-effort", env, read_policy());
    h.client.initialize(info(), T).unwrap();
    let sid = h.client.new_session(&h.cwd, vec![], T).unwrap().session_id;
    for mode in ["plan", "default"] {
        h.client
            .call(
                "session/set_mode",
                json!({"sessionId": sid, "modeId": mode}),
                T,
            )
            .unwrap();
    }
    h.client
        .call(
            "session/set_config_option",
            json!({"sessionId": sid, "configId": "model", "value": "opus"}),
            T,
        )
        .unwrap();
    drain(&h.events);
    let r = h.client.prompt(&sid, "/effort high").unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    let ev = drain(&h.events);
    assert!(
        message_text(&ev).starts_with("Set effort level to high (this session only)"),
        "{}",
        message_text(&ev)
    );
    let notified = raw_updates(&ev, "config_option_update");
    assert_eq!(notified.len(), 1, "{notified:?}");
    let options = &notified[0].1["configOptions"];
    assert_eq!(config_option(options, "effort")["currentValue"], "high");
    assert_eq!(config_option(options, "model")["currentValue"], "opus");
    h.client.shutdown();
    assert!(!fake_log(&h.log).iter().any(|e| e["event"] == "mismatch"));
}

/// The folder Claude Code keeps `cwd`'s sessions in under `projects` (as `process::session_file` finds it).
fn project_folder(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Brief 0061: `session/load` against the recorded `--resume`. A first adapter starts a session that runs `/effort
/// low`; a second adapter resumes it: `claude` is started with `--resume <id>` and answers its own `initialize`, the
/// conversation from Claude Code's session file is replayed before the answer, the answer carries the modes and the
/// options, and the slash commands follow it. Loading it again while it is live is `invalid_params`; a `--resume`
/// `claude` refuses fails the load with its message.
#[test]
fn recorded_resume_loads_the_session_and_replays_its_file() {
    let mut env = fake_env("claude-2.1.289-resume.jsonl");
    env.push(("FAKE_CLAUDE_VERSION".into(), "2.1.289 (Claude Code)".into()));
    let first = spawn("resume-new", env.clone(), read_policy());
    let init = first.client.initialize(info(), T).unwrap();
    assert!(init.agent_capabilities.load_session);
    let session = first
        .client
        .new_session(&first.cwd, vec![eludite_server()], T)
        .unwrap();
    let sid = session.session_id.clone();
    let r = first.client.prompt(&sid, "/effort low").unwrap();
    assert_eq!(r.stop_reason, StopReason::EndTurn);
    first.client.shutdown();
    let log = fake_log(&first.log);
    assert!(!log.iter().any(|e| e["event"] == "mismatch"), "{log:?}");

    // Claude Code's session file for it, where `claude` would have written it.
    let config = temp_dir("resume-config");
    let folder = config.join("projects").join(project_folder(&first.cwd));
    std::fs::create_dir_all(&folder).unwrap();
    let sample = std::fs::read_to_string(fixture("claude-2.1.289-session-file.jsonl"))
        .unwrap()
        .replace("{{SESSION_ID}}", &sid)
        .replace("{{CWD}}", &first.cwd.to_string_lossy());
    std::fs::write(folder.join(format!("{sid}.jsonl")), sample).unwrap();

    let mut env2 = env.clone();
    env2.push((
        "CLAUDE_CONFIG_DIR".into(),
        config.to_string_lossy().into_owned(),
    ));
    let h = spawn("resume-load", env2, read_policy());
    h.client.initialize(info(), T).unwrap();
    let loaded = h
        .client
        .load_session(&sid, &first.cwd, vec![eludite_server()], T)
        .unwrap();
    // The replay was on the wire before the answer, so it is all here already.
    let mut before = Vec::new();
    while let Ok(ev) = h.events.try_recv() {
        before.push(ev);
    }
    let kinds: Vec<String> = updates(&before)
        .iter()
        .map(|u| {
            serde_json::to_value(u).unwrap()["sessionUpdate"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        kinds[..8],
        [
            "user_message_chunk",
            "agent_thought_chunk",
            "tool_call",
            "tool_call_update",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk",
            "user_message_chunk",
        ]
    );
    for ev in &before {
        if let ClientEvent::Update(n) = ev {
            assert_eq!(n.session_id, sid);
        }
    }
    let calls = tool_calls(&before);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].status, Some(ToolCallStatus::Completed));
    assert_eq!(calls[1].status, Some(ToolCallStatus::Failed));
    // The modes and options, as `session/new` answers them.
    assert_eq!(loaded.modes.unwrap().current_mode_id, "default");
    assert!(!loaded.config_options.unwrap_or_default().is_empty());
    let after = [before, drain(&h.events)].concat();
    assert_eq!(
        updates(&after)
            .iter()
            .filter(|u| u.available_commands().is_some())
            .count(),
        1
    );
    // Loading it again while it is live.
    let again = h
        .client
        .load_session(&sid, &first.cwd, vec![eludite_server()], T)
        .unwrap_err();
    let e = rpc_error(&again);
    assert_eq!(e["code"], -32602, "{e}");
    assert!(e["data"].as_str().unwrap().contains("already live"), "{e}");
    h.client.shutdown();
    let log = fake_log(&h.log);
    assert!(!log.iter().any(|e| e["event"] == "mismatch"), "{log:?}");
    let argv: Vec<&str> = log.iter().find(|e| e["event"] == "start").unwrap()["argv"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let at = argv.iter().position(|a| *a == "--resume").unwrap();
    assert_eq!(argv[at + 1], sid);
    assert!(!argv.contains(&"--session-id"));

    // A `--resume` `claude` refuses (the recording's third process).
    let mut env3 = env;
    env3.push(("FAKE_CLAUDE_RESUME".into(), "refused".into()));
    let refused = spawn("resume-refused", env3, read_policy());
    refused.client.initialize(info(), T).unwrap();
    let unknown = "11111111-2222-4333-8444-555555555555";
    let err = refused
        .client
        .load_session(unknown, &refused.cwd, vec![eludite_server()], T)
        .unwrap_err();
    let e = rpc_error(&err);
    let message = e.to_string();
    assert!(
        message.contains(&format!("No conversation found with session ID: {unknown}")),
        "{e}"
    );
    refused.client.shutdown();
    let _ = std::fs::remove_dir_all(&config);
}

/// `measured` under `limit`, asserted on a developer machine only: under CI (`CI` set) the hosted runners are shared
/// VMs, not a reference machine, so the number is printed instead.
fn assert_budget(what: &str, measured: Duration, limit: Duration) {
    if std::env::var_os("CI").is_some() {
        eprintln!(
            "timing: {what} {:.2} ms not asserted against {:.0} ms: a CI run, not a reference machine",
            measured.as_secs_f64() * 1e3,
            limit.as_secs_f64() * 1e3
        );
    } else {
        assert!(
            measured < limit,
            "{what}: {measured:?} is not under {limit:?}"
        );
    }
}
