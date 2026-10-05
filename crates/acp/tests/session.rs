//! [`AgentSession`] against the scripted fake agent as a real child process: the lifecycle (starting, ready,
//! running, the turn's end), streaming, permission requests answered by the policy or by the owner, cancel while a
//! request is pending, the logged-out state, an agent exiting mid-turn and a restart, and the threads events arrive
//! on.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_acp::fake_agent::{DIAGNOSTICS_TOOL, SHELL_TOOL, WRITE_FILE, WRITE_TEXT};
use eludite_acp::protocol::{
    Implementation, PermissionOptionKind, SessionUpdate, StopReason, ToolCallStatus,
};
use eludite_acp::{
    AgentDescriptor, AgentSession, AgentState, PermissionPolicy, PolicyAnswer, SessionConfig,
    SessionEvent,
};

const T: Duration = Duration::from_secs(20);

fn fake(scenario: &str, extra: &[&str]) -> AgentDescriptor {
    let mut args = vec!["--scenario".to_owned(), scenario.to_owned()];
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    AgentDescriptor {
        name: "Fake agent".into(),
        command: env!("CARGO_BIN_EXE_eludite-fake-acp-agent").into(),
        args,
        env: Vec::new(),
        env_remove: Vec::new(),
    }
}

struct Run {
    session: AgentSession,
    events: Receiver<(u64, SessionEvent, String)>,
    seen: Vec<SessionEvent>,
}

fn start(agent: AgentDescriptor, cwd: &std::path::Path, policy: PermissionPolicy, g: u64) -> Run {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let session = AgentSession::start(
        SessionConfig {
            agent,
            connect: None,
            cwd: cwd.to_owned(),
            mcp_servers: Vec::new(),
            client_info: Implementation {
                name: "eludite-test".into(),
                title: None,
                version: "0".into(),
            },
            handshake_timeout: Duration::from_secs(10),
            policy,
        },
        g,
        Arc::new(move |generation, e| {
            let thread = std::thread::current().name().unwrap_or("?").to_owned();
            let _ = tx.lock().unwrap().send((generation, e, thread));
        }),
    );
    Run {
        session,
        events: rx,
        seen: Vec::new(),
    }
}

impl Run {
    /// Collect events until `done` matches one; every event must be of generation `g`, from a session thread.
    fn until(&mut self, g: u64, what: &str, done: impl Fn(&SessionEvent) -> bool) -> SessionEvent {
        let deadline = Instant::now() + T;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (generation, e, thread) = self
                .events
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("timed out waiting for {what}; saw {:#?}", self.seen));
            assert_eq!(generation, g);
            assert!(
                ["acp-driver", "acp-reader", "acp-stderr"].contains(&thread.as_str()),
                "event on {thread}"
            );
            self.seen.push(e.clone());
            if done(&e) {
                return e;
            }
        }
    }

    fn states(&self) -> Vec<AgentState> {
        self.seen
            .iter()
            .filter_map(|e| match e {
                SessionEvent::State(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn text(&self) -> String {
        self.seen
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Update(SessionUpdate::AgentMessageChunk(c)) => c.as_text(),
                _ => None,
            })
            .collect()
    }
}

fn ask_all() -> PermissionPolicy {
    Arc::new(|_| PolicyAnswer::Ask)
}

/// The Agents window's rule for these tests: Eludite's read tool runs, everything else is asked.
fn read_allowed() -> PermissionPolicy {
    Arc::new(|r| {
        if r.tool_call.agent_tool_name() == Some(DIAGNOSTICS_TOOL) {
            PolicyAnswer::Allow("class read".into())
        } else {
            PolicyAnswer::Ask
        }
    })
}

fn is_ready(e: &SessionEvent) -> bool {
    matches!(e, SessionEvent::Ready { .. })
}

fn turn_end(e: &SessionEvent) -> bool {
    matches!(e, SessionEvent::TurnEnded(_))
}

#[test]
fn lifecycle_streaming_and_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = start(
        fake("diagnostics-then-shell", &[]),
        dir.path(),
        read_allowed(),
        1,
    );
    let ready = r.until(1, "ready", is_ready);
    let SessionEvent::Ready {
        session_id,
        agent_info,
        protocol_version,
    } = ready
    else {
        unreachable!()
    };
    assert_eq!(session_id, "fake-session-1");
    assert_eq!(protocol_version, 1);
    assert_eq!(agent_info.unwrap().name, "eludite-fake-acp-agent");
    assert_eq!(r.session.session_id().as_deref(), Some("fake-session-1"));
    let timings: Vec<&str> = r
        .seen
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Timing { name, .. } => Some(*name),
            _ => None,
        })
        .collect();
    assert_eq!(timings, ["spawned", "initialized", "session_ready"]);

    r.session.prompt("List the errors");
    // The read tool is answered by the policy, without the owner.
    let answered = r.until(1, "the read tool's answer", |e| {
        matches!(e, SessionEvent::PermissionAnswered { .. })
    });
    let SessionEvent::PermissionAnswered {
        allowed, reason, ..
    } = answered
    else {
        unreachable!()
    };
    assert!(allowed);
    assert_eq!(reason, "class read");
    // The shell command is the owner's to answer: deny it.
    let asked = r.until(1, "the shell prompt", |e| {
        matches!(e, SessionEvent::Permission { .. })
    });
    let SessionEvent::Permission { key, request } = asked else {
        unreachable!()
    };
    assert_eq!(request.tool_call.agent_tool_name(), Some(SHELL_TOOL));
    assert_eq!(r.session.pending_permissions(), [key]);
    let option = r
        .session
        .answer(key, PermissionOptionKind::RejectOnce)
        .unwrap();
    assert_eq!(option.name, "No");
    assert!(r.session.pending_permissions().is_empty());
    assert!(
        r.session
            .answer(key, PermissionOptionKind::AllowOnce)
            .is_none(),
        "answered once"
    );
    let end = r.until(1, "the turn's end", turn_end);
    assert!(matches!(
        end,
        SessionEvent::TurnEnded(Ok(StopReason::EndTurn))
    ));
    let text = r.text();
    assert!(
        text.contains("The file with the most is `Program.cs`"),
        "{text}"
    );
    assert!(text.contains("You denied permission to run"), "{text}");
    let failed = r.seen.iter().any(|e| {
        matches!(e, SessionEvent::Update(SessionUpdate::ToolCallUpdate(t))
            if t.tool_call_id == "toolu_fake_shell" && t.status == Some(ToolCallStatus::Failed))
    });
    assert!(failed);
    r.until(1, "ready again", |e| {
        matches!(e, SessionEvent::State(AgentState::Ready))
    });
    assert_eq!(
        r.states(),
        [AgentState::Starting, AgentState::Running, AgentState::Ready]
    );
    r.session.shutdown();
    r.until(1, "the agent's exit", |e| {
        matches!(e, SessionEvent::State(AgentState::Exited))
    });
}

#[test]
fn cancel_answers_pending_requests_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = start(fake("diagnostics", &[]), dir.path(), ask_all(), 4);
    r.until(4, "ready", is_ready);
    r.session.prompt("go");
    let SessionEvent::Permission { key, .. } = r.until(4, "the prompt", |e| {
        matches!(e, SessionEvent::Permission { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(r.session.cancel(), [key]);
    assert!(r.session.pending_permissions().is_empty());
    let end = r.until(4, "the turn's end", turn_end);
    assert!(
        matches!(end, SessionEvent::TurnEnded(Ok(StopReason::Cancelled))),
        "{end:?}"
    );
}

#[test]
fn logged_out_agent_reports_login_methods() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = start(fake("login-required", &[]), dir.path(), ask_all(), 2);
    // Known at once from the adapter's status notification, before any prompt.
    let state = r.until(2, "needs login", |e| {
        matches!(e, SessionEvent::State(AgentState::NeedsLogin { .. }))
    });
    let SessionEvent::State(AgentState::NeedsLogin { label, methods }) = state else {
        unreachable!()
    };
    assert_eq!(label, "Not logged in");
    assert_eq!(methods.len(), 1);
    assert!(
        methods[0]
            .command
            .ends_with("--scenario login-required --cli auth login --claudeai"),
        "{}",
        methods[0].command
    );
    // A prompt anyway: the -32000 answer keeps the state.
    r.session.prompt("hello");
    let end = r.until(2, "the failed turn", turn_end);
    assert!(matches!(&end, SessionEvent::TurnEnded(Err(e)) if e.contains("-32000")));
    r.until(2, "needs login again", |e| {
        matches!(e, SessionEvent::State(AgentState::NeedsLogin { .. }))
    });
}

#[test]
fn agent_exit_mid_turn_then_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = start(fake("exit", &[]), dir.path(), ask_all(), 7);
    r.until(7, "ready", is_ready);
    r.session.prompt("go");
    let end = r.until(7, "the broken turn", turn_end);
    assert!(matches!(end, SessionEvent::TurnEnded(Err(_))), "{end:?}");
    r.until(7, "exited", |e| {
        matches!(e, SessionEvent::State(AgentState::Exited))
    });
    assert!(r.text().contains("Starting on it"));

    // The owner restarts: a new session, a new generation.
    let mut again = start(fake("write", &[]), dir.path(), ask_all(), 8);
    again.until(8, "ready", is_ready);
    again.session.prompt("write the notes");
    let SessionEvent::Permission { key, request } = again.until(8, "the write's prompt", |e| {
        matches!(e, SessionEvent::Permission { .. })
    }) else {
        unreachable!()
    };
    // The agent's own file tool shows what it will write.
    let diffs = request.tool_call.diffs();
    assert_eq!(diffs.len(), 1);
    assert!(diffs[0].path.ends_with(WRITE_FILE));
    assert_eq!(diffs[0].old_text, None);
    assert_eq!(diffs[0].new_text, WRITE_TEXT);
    assert_eq!(request.tool_call.kind.as_deref(), Some("edit"));
    again.session.answer(key, PermissionOptionKind::AllowOnce);
    again.until(8, "the turn's end", turn_end);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(WRITE_FILE)).unwrap(),
        WRITE_TEXT
    );
    again.session.shutdown();
}

#[test]
fn a_missing_agent_is_an_error_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = fake("diagnostics", &[]);
    agent.command = dir
        .path()
        .join("no-such-agent")
        .to_string_lossy()
        .into_owned();
    let mut r = start(agent, dir.path(), ask_all(), 3);
    let e = r.until(3, "the error", |e| {
        matches!(e, SessionEvent::State(AgentState::Error(_)))
    });
    assert!(
        matches!(e, SessionEvent::State(AgentState::Error(m)) if m.contains("could not start"))
    );
}

#[test]
fn edit_scenario_streams_a_thought_and_a_plan() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = start(fake("edit", &[]), dir.path(), ask_all(), 5);
    r.until(5, "ready", is_ready);
    r.session.prompt("edit");
    let SessionEvent::Permission { key, .. } = r.until(5, "the tool's prompt", |e| {
        matches!(e, SessionEvent::Permission { .. })
    }) else {
        unreachable!()
    };
    r.session.answer(key, PermissionOptionKind::AllowOnce);
    r.until(5, "the turn's end", turn_end);
    let thought = r
        .seen
        .iter()
        .any(|e| matches!(e, SessionEvent::Update(SessionUpdate::AgentThoughtChunk(_))));
    assert!(thought);
    let plan = r
        .seen
        .iter()
        .find_map(|e| match e {
            SessionEvent::Update(u) => u.plan_entries(),
            _ => None,
        })
        .unwrap();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].status, "in_progress");
    // No MCP server was passed, so the tool fails and the agent says so.
    assert!(r.text().contains("The edit tool failed"), "{}", r.text());
}

/// The fake agent run in this process over pipes (brief 0060), so the session ids it gives stay known to a later run.
fn in_process() -> eludite_acp::session::StreamConnector {
    Arc::new(|agent, _cwd| {
        let opts = eludite_acp::fake_agent::Options::from_args(agent.args.clone())
            .map_err(std::io::Error::other)?;
        let (agent_in_r, agent_in_w) = std::io::pipe()?;
        let (agent_out_r, agent_out_w) = std::io::pipe()?;
        std::thread::Builder::new()
            .name("fake-agent".into())
            .spawn(move || {
                let _ = eludite_acp::fake_agent::run(
                    std::io::BufReader::new(agent_in_r),
                    agent_out_w,
                    opts,
                );
            })?;
        Ok((Box::new(agent_out_r), Box::new(agent_in_w)))
    })
}

/// [`start`], or with `load` [`AgentSession::resume`], over `connect` when given.
fn open(
    agent: AgentDescriptor,
    cwd: &std::path::Path,
    connect: Option<eludite_acp::session::StreamConnector>,
    load: Option<&str>,
    g: u64,
) -> Run {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let config = SessionConfig {
        agent,
        connect,
        cwd: cwd.to_owned(),
        mcp_servers: Vec::new(),
        client_info: Implementation {
            name: "eludite-test".into(),
            title: None,
            version: "0".into(),
        },
        handshake_timeout: Duration::from_secs(10),
        policy: ask_all(),
    };
    let sink: eludite_acp::SessionSink = Arc::new(move |generation, e| {
        let thread = std::thread::current().name().unwrap_or("?").to_owned();
        let _ = tx.lock().unwrap().send((generation, e, thread));
    });
    let session = match load {
        Some(id) => AgentSession::resume(config, id, g, sink),
        None => AgentSession::start(config, g, sink),
    };
    Run {
        session,
        events: rx,
        seen: Vec::new(),
    }
}

impl Run {
    fn replayed(&self) -> Vec<SessionUpdate> {
        self.seen
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Replay(u) => Some(u.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Brief 0060: `session/load` round trip. A session the fake gave in this process is resumed: its replay arrives as
/// `Replay` events (not `Update`s) before `Ready` names the same id, the modes and options come back as after
/// `session/new`, and the next prompt runs a turn in it. An id the fake never gave fails the load; `--load-any`
/// (a real child process) resumes any id.
#[test]
fn a_session_is_resumed_with_session_load_and_its_replay_is_marked() {
    let dir = tempfile::tempdir().unwrap();
    let agent = fake("stream", &["--load", "--options", "--chunks", "20"]);
    let mut first = open(agent.clone(), dir.path(), Some(in_process()), None, 1);
    let SessionEvent::Ready { session_id, .. } = first.until(1, "ready", is_ready) else {
        unreachable!()
    };
    assert!(session_id.starts_with("fake-session-"), "{session_id}");
    first.session.shutdown();

    let mut r = open(
        agent.clone(),
        dir.path(),
        Some(in_process()),
        Some(&session_id),
        2,
    );
    let SessionEvent::Ready {
        session_id: loaded, ..
    } = r.until(2, "the load", is_ready)
    else {
        unreachable!()
    };
    assert_eq!(loaded, session_id);
    assert_eq!(r.session.session_id().as_deref(), Some(session_id.as_str()));
    let replayed = r.replayed();
    assert_eq!(replayed.len(), 2, "{replayed:?}");
    assert!(
        matches!(&replayed[0], SessionUpdate::UserMessageChunk(c) if c.as_text() == Some("An earlier prompt"))
    );
    assert!(
        matches!(&replayed[1], SessionUpdate::AgentMessageChunk(c) if c.as_text() == Some("An earlier answer"))
    );
    // Nothing of the replay is an ordinary update, and it all came before `Ready` (the slash commands the agent
    // sends right after its answer may arrive on the reader thread before the driver reports `Ready`).
    let ready_at = r.seen.iter().position(is_ready).unwrap();
    let last_replay = r
        .seen
        .iter()
        .rposition(|e| matches!(e, SessionEvent::Replay(_)))
        .unwrap();
    assert!(last_replay < ready_at);
    assert!(r.text().is_empty());
    let SessionEvent::Options {
        modes,
        config_options,
    } = r.until(2, "the options", |e| {
        matches!(e, SessionEvent::Options { .. })
    })
    else {
        unreachable!()
    };
    assert_eq!(modes.unwrap().current_mode_id, "default");
    assert_eq!(config_options.len(), 2);
    // The slash commands the agent sends after the load are an update, as after `session/new`.
    let commands =
        |e: &SessionEvent| matches!(e, SessionEvent::Update(u) if u.available_commands().is_some());
    if !r.seen.iter().any(commands) {
        r.until(2, "the commands", commands);
    }
    r.session.prompt("continue");
    assert!(matches!(
        r.until(2, "the turn's end", turn_end),
        SessionEvent::TurnEnded(Ok(StopReason::EndTurn))
    ));
    assert!(r.text().contains("chunk 00019"), "{}", r.text());
    r.session.shutdown();

    // An id this process never gave.
    let mut unknown = open(
        agent,
        dir.path(),
        Some(in_process()),
        Some("fake-session-x"),
        3,
    );
    let e = unknown.until(3, "the refusal", |e| {
        matches!(e, SessionEvent::State(AgentState::Error(_)))
    });
    assert!(
        matches!(&e, SessionEvent::State(AgentState::Error(m)) if m.starts_with("session/load:") && m.contains("-32602")),
        "{e:?}"
    );
    unknown.session.shutdown();

    // A real child with `--load-any` resumes whatever it is given.
    let mut any = open(
        fake("diagnostics", &["--load-any"]),
        dir.path(),
        None,
        Some("from-another-day"),
        4,
    );
    let SessionEvent::Ready { session_id, .. } = any.until(4, "the load", is_ready) else {
        unreachable!()
    };
    assert_eq!(session_id, "from-another-day");
    assert_eq!(any.replayed().len(), 2);
    any.session.shutdown();
}

/// Brief 0060: an agent that does not advertise `loadSession` cannot resume: the session reports `LoadUnsupported`,
/// never `Ready`, and sends no `session/load`.
#[test]
fn an_agent_without_load_session_reports_it_cannot_resume() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = open(
        fake("stream", &[]),
        dir.path(),
        None,
        Some("fake-session-1"),
        9,
    );
    r.until(9, "the refusal", |e| {
        matches!(e, SessionEvent::LoadUnsupported)
    });
    assert!(!r.seen.iter().any(is_ready));
    assert!(r.replayed().is_empty());
    r.session.shutdown();
}
