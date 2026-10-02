//! `niello-acp` against the scripted fake agent and the recorded real session.

use std::io::Write;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use niello_acp::fake_agent::{self, DIAGNOSTICS_TOOL, Options, SHELL_TOOL, Scenario};
use niello_acp::protocol::{
    InitializeResponse, NewSessionResponse, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, SessionNotification, SessionUpdate, StopReason, ToolCall,
    ToolCallStatus,
};
use niello_acp::{AcpClient, AgentDescriptor, ClientEvent, EventSink};
use serde_json::Value;

const T: Option<Duration> = Some(Duration::from_secs(10));

fn info() -> niello_acp::protocol::Implementation {
    niello_acp::protocol::Implementation {
        name: "niello-test".into(),
        title: None,
        version: "0".into(),
    }
}

/// Copies everything the client writes, so the test can assert the client's
/// side of the conversation.
#[derive(Clone)]
struct Tee<W> {
    inner: Arc<Mutex<W>>,
    seen: Arc<Mutex<Vec<u8>>>,
}

impl<W: Write> Write for Tee<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.seen.lock().unwrap().extend_from_slice(buf);
        self.inner.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.lock().unwrap().flush()
    }
}

/// What the test's "panel" decides for a permission request.
type Policy = Arc<dyn Fn(&RequestPermissionRequest) -> Option<bool> + Send + Sync>;

struct Harness {
    client: AcpClient,
    events: Receiver<ClientEvent>,
    sent: Arc<Mutex<Vec<u8>>>,
}

/// Connect a client to an in-process fake agent over OS pipes. `policy`
/// answers permission requests (Some(allow)), or None to leave them pending.
fn harness(opts: Options, policy: Policy) -> Harness {
    let (agent_in_r, agent_in_w) = std::io::pipe().unwrap();
    let (agent_out_r, agent_out_w) = std::io::pipe().unwrap();
    thread::spawn(move || {
        fake_agent::run(std::io::BufReader::new(agent_in_r), agent_out_w, opts).unwrap();
    });
    let sent = Arc::new(Mutex::new(Vec::new()));
    let tee = Tee {
        inner: Arc::new(Mutex::new(agent_in_w)),
        seen: sent.clone(),
    };
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let cell: Arc<OnceLock<AcpClient>> = Arc::default();
    let cell2 = cell.clone();
    let sink: EventSink = Arc::new(move |ev| {
        if let ClientEvent::PermissionRequest { id, request } = &ev
            && let Some(allow) = policy(request)
        {
            let opt = request.option(allow).expect("option").option_id.clone();
            let c = cell2.get().expect("client set");
            c.respond_permission(
                id.clone(),
                RequestPermissionOutcome::Selected { option_id: opt },
            )
            .unwrap();
        }
        let _ = tx.lock().unwrap().send(ev);
    });
    let client = AcpClient::connect(agent_out_r, tee, sink);
    let _ = cell.set(client.clone());
    Harness {
        client,
        events: rx,
        sent,
    }
}

/// The panel's policy from brief 0005: the Niello read tool runs without a
/// prompt; everything else is "prompted", and the simulated user denies it.
fn read_only_policy() -> Policy {
    Arc::new(|req| Some(req.tool_call.agent_tool_name() == Some(DIAGNOSTICS_TOOL)))
}

fn drain(rx: &Receiver<ClientEvent>) -> Vec<ClientEvent> {
    // Updates precede the prompt response on the wire, but the reader thread
    // may still be delivering; wait briefly for stragglers.
    let mut out = Vec::new();
    while let Ok(ev) = rx.recv_timeout(Duration::from_millis(200)) {
        out.push(ev);
    }
    out
}

fn message_text(events: &[ClientEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::Update(SessionNotification {
                update: SessionUpdate::AgentMessageChunk(c),
                ..
            }) => c.as_text().map(str::to_owned),
            _ => None,
        })
        .collect()
}

/// Fold tool_call / tool_call_update into final tool call states, in order.
fn tool_calls(events: &[ClientEvent]) -> Vec<ToolCall> {
    let mut calls: Vec<ToolCall> = Vec::new();
    for e in events {
        if let ClientEvent::Update(n) = e {
            match &n.update {
                SessionUpdate::ToolCall(t) => calls.push(t.clone()),
                SessionUpdate::ToolCallUpdate(u) => {
                    if let Some(c) = calls.iter_mut().find(|c| c.tool_call_id == u.tool_call_id) {
                        c.apply(u.clone());
                    }
                }
                _ => {}
            }
        }
    }
    calls
}

fn sent_methods(sent: &Mutex<Vec<u8>>) -> Vec<String> {
    String::from_utf8(sent.lock().unwrap().clone())
        .unwrap()
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            match v.get("method").and_then(Value::as_str) {
                Some(m) => m.to_owned(),
                None => format!("response:{}", v["id"].as_str().unwrap_or("?")),
            }
        })
        .collect()
}

#[test]
fn client_sequence_read_runs_and_shell_is_denied() {
    let h = harness(
        Options {
            scenario: Scenario::DiagnosticsThenShell,
            ..Options::default()
        },
        read_only_policy(),
    );
    let init = h.client.initialize(info(), T).unwrap();
    assert_eq!(init.protocol_version, 1);
    let session = h.client.new_session(Path::new("/work"), vec![], T).unwrap();
    assert_eq!(session.session_id, "fake-session-1");
    let stop = h
        .client
        .prompt(&session.session_id, "List the current errors")
        .unwrap();
    assert_eq!(stop.stop_reason, StopReason::EndTurn);
    let events = drain(&h.events);

    // Client side: initialize, session/new, session/prompt, then one answer
    // per permission request.
    assert_eq!(
        sent_methods(&h.sent),
        [
            "initialize",
            "session/new",
            "session/prompt",
            "response:perm-1",
            "response:perm-2"
        ]
    );
    // Agent side: streamed chunks, two tool calls, two permission requests.
    let perms: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::PermissionRequest { request, .. } => {
                request.tool_call.agent_tool_name().map(str::to_owned)
            }
            _ => None,
        })
        .collect();
    assert_eq!(perms, [DIAGNOSTICS_TOOL, SHELL_TOOL]);
    let chunks = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ClientEvent::Update(SessionNotification {
                    update: SessionUpdate::AgentMessageChunk(_),
                    ..
                })
            )
        })
        .count();
    assert!(chunks > 5, "streamed in many chunks, got {chunks}");

    let calls = tool_calls(&events);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].agent_tool_name(), Some(DIAGNOSTICS_TOOL));
    assert_eq!(calls[0].status, Some(ToolCallStatus::Completed));
    assert_eq!(calls[0].raw_input.as_ref().unwrap()["severity"], "error");
    assert_eq!(calls[1].kind.as_deref(), Some("execute"));
    assert_eq!(
        calls[1].status,
        Some(ToolCallStatus::Failed),
        "denied shell call must not run"
    );
    assert!(calls[1].content_text().contains("rejected"));
    let text = message_text(&events);
    assert!(
        text.contains("You denied permission to run `rm -rf obj/`"),
        "{text}"
    );
    assert!(!text.contains("cleaned"), "{text}");
}

#[test]
fn denied_read_stops_the_call_and_agent_reports_it() {
    let h = harness(
        Options {
            scenario: Scenario::Diagnostics,
            ..Options::default()
        },
        Arc::new(|_| Some(false)),
    );
    h.client.initialize(info(), T).unwrap();
    let s = h.client.new_session(Path::new("/work"), vec![], T).unwrap();
    let stop = h.client.prompt(&s.session_id, "errors?").unwrap();
    assert_eq!(stop.stop_reason, StopReason::EndTurn);
    let events = drain(&h.events);
    let calls = tool_calls(&events);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].status, Some(ToolCallStatus::Failed));
    assert!(calls[0].raw_output.is_none(), "no result for a denied call");
    assert!(message_text(&events).contains("denied"));
}

#[test]
fn cancel_while_permission_pending() {
    let (ptx, prx) = mpsc::channel();
    let ptx = Mutex::new(ptx);
    let h = harness(
        Options {
            scenario: Scenario::Diagnostics,
            ..Options::default()
        },
        Arc::new(move |req| {
            let _ = ptx.lock().unwrap().send(req.session_id.clone());
            None
        }),
    );
    h.client.initialize(info(), T).unwrap();
    let s = h.client.new_session(Path::new("/work"), vec![], T).unwrap();
    let c2 = h.client.clone();
    let sid = s.session_id.clone();
    let canceller = thread::spawn(move || {
        let session = prx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(session, sid);
        // ACP: cancel the turn, and answer pending permission requests with `cancelled`.
        c2.cancel(&session).unwrap();
    });
    // The fake agent treats session/cancel during a permission wait as cancellation.
    let stop = h.client.prompt(&s.session_id, "errors?").unwrap();
    canceller.join().unwrap();
    assert_eq!(stop.stop_reason, StopReason::Cancelled);
    assert!(sent_methods(&h.sent).contains(&"session/cancel".to_owned()));
}

#[test]
fn login_required_surfaces_auth_methods_and_error() {
    let h = harness(
        Options {
            scenario: Scenario::LoginRequired,
            ..Options::default()
        },
        Arc::new(|_| None),
    );
    let init = h.client.initialize(info(), T).unwrap();
    let m = &init.auth_methods[0];
    assert_eq!(m.kind.as_deref(), Some("terminal"));
    assert_eq!(
        niello_acp::npx_claude_agent().terminal_auth_command(m),
        "npx -y @agentclientprotocol/claude-agent-acp@0.85.0 --cli auth login --claudeai"
    );
    let s = h.client.new_session(Path::new("/work"), vec![], T).unwrap();
    let err = h.client.prompt(&s.session_id, "hi").unwrap_err();
    assert!(err.is_auth_required(), "{err}");
    let events = drain(&h.events);
    assert!(events.iter().any(
        |e| matches!(e, ClientEvent::OtherNotification { method, params: Some(p) }
        if method == "_auth/status_update" && p["authStatus"]["kind"] == "none")
    ));
}

#[test]
fn stream_arrives_complete_and_in_order() {
    let h = harness(
        Options {
            scenario: Scenario::Stream,
            chunks: 300,
            rate_hz: 2000.,
        },
        Arc::new(|_| None),
    );
    h.client.initialize(info(), T).unwrap();
    let s = h.client.new_session(Path::new("/work"), vec![], T).unwrap();
    h.client.prompt(&s.session_id, "stream").unwrap();
    let events = drain(&h.events);
    let text = message_text(&events);
    let ids: Vec<usize> = text
        .split("chunk ")
        .skip(1)
        .map(|s| s[..5].parse().unwrap())
        .collect();
    assert_eq!(ids, (0..300).collect::<Vec<_>>());
}

#[test]
fn spawned_process_and_close() {
    let agent = AgentDescriptor {
        name: "fake".into(),
        command: env!("CARGO_BIN_EXE_niello-fake-acp-agent").into(),
        args: vec!["--scenario".into(), "diagnostics".into()],
        env: vec![],
        env_remove: vec!["CLAUDECODE".into()],
    };
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let cell: Arc<OnceLock<AcpClient>> = Arc::default();
    let cell2 = cell.clone();
    let client = AcpClient::spawn(
        &agent,
        Path::new("."),
        Arc::new(move |ev| {
            if let ClientEvent::PermissionRequest { id, request } = &ev {
                let opt = request.option(true).unwrap().option_id.clone();
                cell2
                    .get()
                    .unwrap()
                    .respond_permission(
                        id.clone(),
                        RequestPermissionOutcome::Selected { option_id: opt },
                    )
                    .unwrap();
            }
            // Events are delivered on the client's own I/O threads.
            let thread = std::thread::current().name().map(str::to_owned);
            assert!(
                matches!(thread.as_deref(), Some("acp-reader" | "acp-stderr")),
                "{thread:?}"
            );
            let _ = tx.lock().unwrap().send(ev);
        }),
    )
    .unwrap();
    let _ = cell.set(client.clone());
    client.initialize(info(), T).unwrap();
    let s = client.new_session(Path::new("."), vec![], T).unwrap();
    assert_eq!(
        client.prompt(&s.session_id, "errors?").unwrap().stop_reason,
        StopReason::EndTurn
    );
    let events = drain(&rx);
    // No MCP server was passed, so the fake answers from its canned row.
    assert!(message_text(&events).contains("Program.cs"));
    client.shutdown();
    let closed = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(closed, ClientEvent::Closed), "{closed:?}");
    assert!(client.is_closed());
    assert_eq!(
        client.prompt("x", "y").unwrap_err(),
        niello_acp::AcpError::Closed
    );
}

/// Every message in the session recorded from the real adapter decodes with
/// this crate's types (ADR-0003: conformance tests replay recorded sessions).
#[test]
fn recorded_claude_session_decodes() {
    let text = include_str!("fixtures/claude-agent-acp-0.85.0-diagnostics.jsonl");
    let mut updates = Vec::new();
    let mut permission = None;
    let mut responses = Vec::new();
    for line in text.lines() {
        let rec: Value = serde_json::from_str(line).unwrap();
        let m = rec["m"].clone();
        match m.get("method").and_then(Value::as_str) {
            Some("session/update") => updates
                .push(serde_json::from_value::<SessionNotification>(m["params"].clone()).unwrap()),
            Some("session/request_permission") => {
                permission = Some(
                    serde_json::from_value::<RequestPermissionRequest>(m["params"].clone())
                        .unwrap(),
                );
            }
            Some(_) => {}
            None => responses.push(m),
        }
    }
    let init: InitializeResponse = serde_json::from_value(responses[0]["result"].clone()).unwrap();
    assert_eq!(init.protocol_version, 1);
    assert_eq!(init.agent_info.as_ref().unwrap().version, "0.85.0");
    assert!(init.agent_capabilities.mcp_capabilities.http);
    let _: NewSessionResponse = serde_json::from_value(responses[1]["result"].clone()).unwrap();
    let stop: PromptResponse = serde_json::from_value(responses[2]["result"].clone()).unwrap();
    assert_eq!(stop.stop_reason, StopReason::EndTurn);

    let perm = permission.expect("the adapter asked permission for the MCP tool");
    assert_eq!(perm.tool_call.agent_tool_name(), Some(DIAGNOSTICS_TOOL));
    assert_eq!(perm.option(true).unwrap().option_id, "allow-once");
    assert_eq!(perm.option(false).unwrap().option_id, "reject");

    let events: Vec<ClientEvent> = updates.into_iter().map(ClientEvent::Update).collect();
    let calls = tool_calls(&events);
    let diag = calls
        .iter()
        .find(|c| c.agent_tool_name() == Some(DIAGNOSTICS_TOOL))
        .unwrap();
    assert_eq!(diag.status, Some(ToolCallStatus::Completed));
    assert!(diag.content_text().contains("OrderController.cs"));
    assert!(message_text(&events).contains("OrderController.cs` with 3"));
    assert!(events.iter().any(|e| matches!(e, ClientEvent::Update(SessionNotification { update: SessionUpdate::Other { kind, .. }, .. }) if kind == "usage_update")));
}
