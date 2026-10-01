//! Headless GPUI tests of the Agents window (brief 0016) with the scripted fake ACP agent from brief 0005, run
//! in-process over pipes (its MCP calls reach Eludite's real loopback endpoint with the real token): start, prompt,
//! streamed transcript rows, a read tool answered without a prompt, an edit held as pending changes that are accepted
//! (one undo step) and rejected, an execute tool prompting and denied, cancel mid-turn, an agent exiting and a
//! restart, the logged-out state, permissions persisted per solution, and the audit link from a transcript row to the
//! change it made.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_acp::fake_agent::{self, Options};
use eludite_acp::{AgentDescriptor, AgentSource, RegisteredAgent};
use eludite_docking::ids;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::super::tests::{T, Ws, setup_full};
use super::window::StateKind;
use super::{AgentsSetup, Shell};

/// The registry the tests offer, by name: each runs the fake agent with a scenario.
const AGENTS: [(&str, &str); 6] = [
    ("Fake agent", "diagnostics-then-shell"),
    ("Fake editor", "edit"),
    ("Fake writer", "write"),
    ("Fake streamer", "stream"),
    ("Logged out", "login-required"),
    ("Crasher", "exit"),
];

fn fake_setup() -> AgentsSetup {
    let registry = AGENTS
        .iter()
        .map(|(name, scenario)| RegisteredAgent {
            descriptor: AgentDescriptor {
                name: (*name).into(),
                command: "eludite-fake-acp-agent".into(),
                args: vec![
                    "--scenario".into(),
                    (*scenario).into(),
                    "--chunks".into(),
                    "400".into(),
                    "--rate".into(),
                    "2000".into(),
                ],
                env: Vec::new(),
                env_remove: Vec::new(),
            },
            source: AgentSource::Settings,
        })
        .collect();
    AgentsSetup {
        registry: Some(registry),
        relay_exe: "eludite".into(),
        connect: Some(Arc::new(|agent, _cwd| {
            let mut opts = Options::from_args(agent.args.clone()).map_err(std::io::Error::other)?;
            // In-process: reach the MCP endpoint directly instead of launching `eludite --mcp-relay`.
            opts.mcp_direct = true;
            let (agent_in_r, agent_in_w) = std::io::pipe()?;
            let (agent_out_r, agent_out_w) = std::io::pipe()?;
            std::thread::Builder::new()
                .name("fake-agent".into())
                .spawn(move || {
                    let _ = fake_agent::run(std::io::BufReader::new(agent_in_r), agent_out_w, opts);
                })?;
            Ok((Box::new(agent_out_r), Box::new(agent_in_w)))
        })),
        preferred: None,
        transcript_out: None,
    }
}

fn setup(cx: &mut TestAppContext) -> Ws {
    let mut w = setup_full(cx, |_| {}, Some(fake_setup()));
    w.open_solution();
    w
}

impl Ws {
    fn agents_state(&self) -> StateKind {
        self.shell.read_with(&self.vcx, |s, _| s.agents().state)
    }

    fn start_agent(&mut self, name: &str) {
        let name = name.to_owned();
        self.shell
            .update(&mut self.vcx, |s, cx| s.agents_start(Some(&name), true, cx))
            .unwrap();
        self.wait("the agent to be ready", |w| {
            matches!(w.agents_state(), StateKind::Ready | StateKind::NeedsLogin)
        });
    }

    fn transcript(&self) -> Value {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().window.read(cx).transcript.to_json()
        })
    }

    fn agent_text(&self) -> String {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().window.read(cx).transcript.agent_message()
        })
    }

    fn turn_ended(&self) -> bool {
        self.shell.read_with(&self.vcx, |s, _| {
            s.agents().last_stop.is_some() && s.agents().state != StateKind::Running
        })
    }

    fn wait_turn(&mut self) -> String {
        self.wait("the turn's end", |w| w.turn_ended());
        self.shell
            .read_with(&self.vcx, |s, _| s.agents().last_stop.clone().unwrap())
    }

    fn tool(&self, id: &str) -> Value {
        let calls = self.transcript();
        calls
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["tool_call"]["id"] == id)
            .map(|r| r["tool_call"].clone())
            .unwrap_or(Value::Null)
    }

    /// Type `text` in the prompt box and press Enter.
    fn type_prompt(&mut self, text: &str) {
        self.show_agents();
        self.click(super::window::PROMPT_BOX);
        let keys: Vec<String> = text
            .chars()
            .map(|c| {
                if c == ' ' {
                    "space".into()
                } else {
                    c.to_string()
                }
            })
            .collect();
        self.vcx.simulate_keystrokes(&keys.join(" "));
        self.vcx.simulate_keystrokes("enter");
        self.vcx.run_until_parked();
    }

    /// View > Agents.
    fn show_agents(&mut self) {
        self.commands
            .invoke("eludite.view.show", json!({"id": ids::AGENTS}))
            .unwrap();
        self.vcx.run_until_parked();
    }

    fn wait_for_prompt(&mut self) -> u64 {
        self.wait("a permission prompt", |w| {
            w.shell
                .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).prompt.is_some())
        });
        self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().window.read(cx).prompt.as_ref().unwrap().request
        })
    }
}

#[gpui::test]
fn start_prompt_stream_read_without_prompt_and_deny_the_shell(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    // View > Agents shows the window, tabbed with Workspace.
    w.commands
        .invoke("eludite.view.show", json!({"id": ids::AGENTS}))
        .unwrap();
    w.vcx.run_until_parked();
    assert!(w.vcx.debug_bounds("agents-window").is_some());
    let header = w
        .shell
        .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).header.clone());
    assert_eq!(header.agents[0], "Fake agent");
    assert_eq!(header.state, StateKind::Stopped);

    w.start_agent("Fake agent");
    let detail = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents().window.read(cx).header.detail.clone()
    });
    assert!(detail.contains("eludite-fake-acp-agent"), "{detail}");
    assert!(
        detail.contains("MCP: eludite via stdio relay to 127.0.0.1:"),
        "{detail}"
    );
    let started = Instant::now();
    w.type_prompt("List the errors");

    // The read tool runs without a prompt; the shell command waits for the user.
    let request = w.wait_for_prompt();
    let read = w.tool("toolu_fake_diagnostics");
    assert_eq!(read["status"], "completed", "{read}");
    let note = read["note"].as_str().unwrap();
    assert!(
        note.contains("Allowed without prompt: diagnostics.list is class read"),
        "{note}"
    );
    assert!(note.contains("audit #"), "{note}");
    let shell_call = w.tool("toolu_fake_shell");
    assert_eq!(shell_call["status"], "awaiting permission", "{shell_call}");
    w.click(&super::window::decision_button(
        super::window::Decision::Deny,
    ));
    let _ = request;
    assert_eq!(w.wait_turn(), "end_turn");
    assert!(started.elapsed() < T);
    let shell_call = w.tool("toolu_fake_shell");
    assert_eq!(shell_call["status"], "denied", "{shell_call}");
    let text = w.agent_text();
    assert!(text.contains("The Error List has no errors."), "{text}");
    assert!(
        text.contains("You denied permission to run `rm -rf obj/`"),
        "{text}"
    );
    let rows = w.transcript();
    assert_eq!(rows[1]["user"], "List the errors");
    // The read call went through the bus as the agent, with its arguments.
    let audit = w.commands.audit_log().entries();
    let diag = audit
        .iter()
        .find(|e| e.command == "diagnostics.list")
        .expect("audited");
    assert!(diag.caller.is_agent());
    assert_eq!(diag.arguments, Some(json!({"severity": "error"})));
    // So is the agent's own tool, with its arguments and how it ended, linked from its row.
    let bash = audit.iter().find(|e| e.command == "Bash").expect("audited");
    assert!(!bash.is_ok());
    assert_eq!(bash.arguments.as_ref().unwrap()["command"], "rm -rf obj/");
    assert!(
        matches!(&bash.caller, eludite_commands::Caller::Agent { tool_call: Some(t), .. } if t == "toolu_fake_shell")
    );
    let note = w.tool("toolu_fake_shell")["note"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(note.contains(&format!("audit #{}", bash.seq)), "{note}");
    assert_eq!(w.agents_state(), StateKind::Ready);
    let _ = Duration::ZERO;
    let _ = std::marker::PhantomData::<Shell>;
}

#[gpui::test]
fn always_allow_persists_per_solution_and_the_rule_decides_next_time(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Fake agent");
    w.type_prompt("go");
    w.wait_for_prompt();
    let can_persist = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents()
            .window
            .read(cx)
            .prompt
            .as_ref()
            .unwrap()
            .can_persist
    });
    assert!(can_persist, "a solution is open, so there is a policy file");
    w.click(&super::window::decision_button(
        super::window::Decision::AlwaysAllow,
    ));
    assert_eq!(w.wait_turn(), "end_turn");
    assert!(w.agent_text().contains("I also cleaned obj/"));
    let shell_call = w.tool("toolu_fake_shell");
    assert_eq!(shell_call["status"], "completed", "{shell_call}");
    assert!(
        shell_call["note"]
            .as_str()
            .unwrap()
            .contains("always for this solution")
    );

    // The rule is in the solution's committable policy file.
    let path = w.path(".eludite/agents-policy.json");
    w.wait("the policy file", |w| {
        w.path(".eludite/agents-policy.json").is_file()
    });
    let policy: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        policy,
        json!({"version": 1, "rules": [{"tool": "Bash", "command_prefix": "rm -rf obj/", "decision": "allow"}]})
    );

    // A new session reads it: the same command now runs without a prompt.
    w.start_agent("Fake agent");
    w.type_prompt("again");
    assert_eq!(w.wait_turn(), "end_turn");
    let shell_call = w.tool("toolu_fake_shell");
    let note = shell_call["note"].as_str().unwrap().to_owned();
    assert!(
        note.contains("Allowed without prompt: the solution's policy rule for Bash `rm -rf obj/`"),
        "{note}"
    );
}

#[gpui::test]
fn eludite_execute_commands_prompt_at_the_mcp_boundary(cx: &mut TestAppContext) {
    use std::io::{BufRead, BufReader, Write};
    let mut w = setup(cx);
    w.start_agent("Fake agent");
    let (addr, token) = w.shell.read_with(&w.vcx, |s, _| {
        let e = s.agents().endpoint().unwrap();
        (e.addr(), e.token().to_owned())
    });
    // An agent calls an execute command (opening a solution runs MSBuild): the window asks.
    let sln = w.path("App.slnx").to_string_lossy().into_owned();
    let call = std::thread::spawn(move || {
        let mut sock = std::net::TcpStream::connect(addr).unwrap();
        writeln!(sock, "{token}").unwrap();
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "eludite-solution-open", "arguments": {"path": sln}}});
        writeln!(sock, "{msg}").unwrap();
        let mut line = String::new();
        BufReader::new(sock).read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    });
    let request = w.wait_for_prompt();
    let prompt = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents().window.read(cx).prompt.clone().unwrap()
    });
    assert_eq!(prompt.tool, "mcp__eludite__eludite-solution-open");
    assert_eq!(prompt.class, "execute");
    let rows = w.transcript();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["tool_call"]["status"] == "awaiting permission")
    );
    let answered = w
        .shell
        .update(&mut w.vcx, |s, cx| {
            s.agents_answer(request, super::window::Decision::Deny, cx)
        })
        .unwrap();
    assert_eq!(answered.class, eludite_commands::PermissionClass::Execute);
    assert!(!answered.persisted);
    let reply = loop {
        w.vcx.run_until_parked();
        if call.is_finished() {
            break call.join().unwrap();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("the user denied it")
    );
    // Denied calls are audited as the agent's.
    let audit = w.commands.audit_log().entries();
    let e = audit
        .iter()
        .rev()
        .find(|e| e.command == "eludite.solution.open")
        .unwrap();
    assert!(e.caller.is_agent());
    assert!(!e.is_ok());
}

fn change_ids(w: &Ws) -> Vec<(u64, String, String)> {
    w.shell.read_with(&w.vcx, |s, _| {
        s.agents()
            .changes
            .values()
            .map(|c| (c.id, c.file_name(), c.state.label().to_owned()))
            .collect()
    })
}

#[gpui::test]
fn an_edit_is_held_for_review_accepted_as_one_undo_and_rejected(cx: &mut TestAppContext) {
    let mut w = setup_full(cx, |_| {}, Some(fake_setup()));
    let (program, view) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    let order_before = std::fs::read_to_string(&order).unwrap();
    w.start_agent("Fake editor");
    w.type_prompt("Add a header comment");

    // Both files are pending; nothing is applied yet; the first opens for review; the editor marks the line.
    w.wait("two pending changes with their diffs", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.agents().changes.len() == 2 && s.agents().changes.values().all(|c| c.diff.is_some())
        })
    });
    let changes = change_ids(&w);
    let mut names: Vec<_> = changes
        .iter()
        .map(|c| (c.1.as_str(), c.2.as_str()))
        .collect();
    names.sort();
    assert_eq!(names, [("Order.cs", "pending"), ("Program.cs", "pending")]);
    let state_of = |w: &Ws, name: &str| change_ids(w).into_iter().find(|c| c.1 == name).unwrap().2;
    assert_eq!(w.text(&view), super::super::tests::PROGRAM);
    assert_eq!(std::fs::read_to_string(&order).unwrap(), order_before);
    let id_of = |name: &str| changes.iter().find(|c| c.1 == name).unwrap().0;
    let (p_id, o_id) = (id_of("Program.cs"), id_of("Order.cs"));
    let first = changes[0].0;
    let tab = super::review::review_tab(first);
    assert!(
        w.controller.layout().documents.get(&tab).is_some(),
        "the review view opened"
    );
    let marks = w.shell.read_with(&w.vcx, |s, cx| {
        let id = super::super::documents::normalize_path(&program)
            .to_string_lossy()
            .into_owned();
        s.agents()
            .gutters
            .borrow()
            .get(&id)
            .map(|g| g.read(cx).rows.clone())
    });
    assert_eq!(marks, Some(vec![(0, 0)]), "an insertion before line 1");
    let tool = w.tool("toolu_fake_edit");
    assert_eq!(tool["status"], "awaiting review", "{tool}");
    assert!(!w.turn_ended(), "the agent's call waits for the review");

    // Accept Program.cs in its review view; reject Order.cs in the window.
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| s.open_review(p_id, window, cx));
    w.vcx.run_until_parked();
    w.click(&super::review::review_button(p_id, true));
    w.wait("Program.cs accepted", |w| {
        state_of(w, "Program.cs") == "accepted"
    });
    assert_eq!(
        w.text(&view),
        format!(
            "{}{}",
            eludite_acp::fake_agent::EDIT_HEADER,
            super::super::tests::PROGRAM
        )
    );
    w.show_agents();
    w.click(&super::window::review_button(Some(o_id), false));
    assert_eq!(w.wait_turn(), "end_turn");
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        order_before,
        "rejected: not written"
    );
    let text = w.agent_text();
    assert!(
        text.contains("The edit ended `applied`: 1 edits in 1 files."),
        "{text}"
    );
    assert!(text.contains("Accepted by the user: Program.cs."), "{text}");
    assert!(
        text.contains("Rejected by the user (discarded, not applied): Order.cs."),
        "{text}"
    );
    let tool = w.tool("toolu_fake_edit");
    assert_eq!(tool["status"], "completed", "{tool}");
    let note = tool["note"].as_str().unwrap();
    assert!(
        note.contains(&format!("Change #{p_id} Program.cs: accepted")),
        "{note}"
    );
    assert!(
        note.contains(&format!("Change #{o_id} Order.cs: rejected")),
        "{note}"
    );
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.agents().gutters.borrow().is_empty()),
        "marks cleared"
    );

    // The audit entry of the agent's call carries both decisions.
    let audit = w.commands.audit_log().entries();
    let e = audit
        .iter()
        .find(|e| e.command == "eludite.workspace.apply_edit" && e.caller.is_agent())
        .unwrap();
    let states: Vec<_> = e.edits.iter().map(|r| (r.change, r.state)).collect();
    assert_eq!(
        states,
        [
            (p_id, eludite_commands::EditState::Accepted),
            (o_id, eludite_commands::EditState::Rejected)
        ]
    );

    // The row links to what was applied: the file, at the change.
    w.click(&super::window::change_link(p_id));
    let id = super::super::documents::normalize_path(&program)
        .to_string_lossy()
        .into_owned();
    assert_eq!(w.controller.active_document().as_deref(), Some(id.as_str()));
    // A rejected change's link shows its review view.
    w.click(&super::window::change_link(o_id));
    assert_eq!(
        w.controller.active_document(),
        Some(super::review::review_tab(o_id))
    );

    // One undo step takes the accepted change back.
    let path = program.to_string_lossy().into_owned();
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.run("eludite.editor.undo", json!({"path": path}), window, cx)
    });
    w.vcx.run_until_parked();
    assert_eq!(w.text(&view), super::super::tests::PROGRAM);
}

#[gpui::test]
fn the_agents_own_write_is_reviewed_and_allowed_on_accept(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Fake writer");
    w.type_prompt("Write the notes");
    w.wait("the write's pending change", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.agents().changes.values().any(|c| c.diff.is_some())
        })
    });
    let (id, name, state) = change_ids(&w).remove(0);
    assert_eq!((name.as_str(), state.as_str()), ("notes.txt", "pending"));
    // No permission prompt: the review is the question.
    assert!(
        w.shell
            .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).prompt.is_none())
    );
    let notes = w.path("notes.txt");
    assert!(!notes.exists());
    w.click(&super::window::review_button(Some(id), true));
    assert_eq!(w.wait_turn(), "end_turn");
    assert_eq!(
        std::fs::read_to_string(&notes).unwrap(),
        eludite_acp::fake_agent::WRITE_TEXT
    );
    assert!(w.agent_text().contains("Created notes.txt."));
    let audit = w.commands.audit_log().entries();
    let e = audit
        .iter()
        .find(|e| e.command == "Write")
        .expect("the agent's own tool is audited");
    assert_eq!(e.edits[0].state, eludite_commands::EditState::Accepted);
}

#[gpui::test]
fn streaming_fills_the_transcript_without_blocking_the_ui(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Fake streamer");
    w.type_prompt("stream");
    // While 400 chunks stream in, the UI thread keeps answering.
    let mut longest = Duration::ZERO;
    w.wait("the stream", |w| {
        let t = Instant::now();
        let _ = w.shell.read_with(&w.vcx, |s, _| s.agents().state);
        longest = longest.max(t.elapsed());
        w.turn_ended()
    });
    assert!(longest < Duration::from_millis(50), "{longest:?}");
    let text = w.agent_text();
    assert!(text.contains("chunk 00000 lorem ipsum"), "{text}");
    assert!(text.contains("chunk 00399"), "every chunk arrived");
    // One row per line: 400 chunks with a newline every 8th.
    let lines = text.lines().count();
    assert_eq!(lines, 51, "{lines}");
    // The prompt box and the audit: the prompt went through the bus.
    assert!(w.audit().contains(&"eludite.agents.prompt".to_owned()));
}

#[gpui::test]
fn escape_cancels_the_turn_mid_permission(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Fake agent");
    w.type_prompt("go");
    w.wait_for_prompt();
    // Escape in the prompt box cancels the turn: the pending request is answered `cancelled`.
    w.click(super::window::PROMPT_BOX);
    w.vcx.simulate_keystrokes("escape");
    assert_eq!(w.wait_turn(), "cancelled");
    assert!(w.audit().contains(&"eludite.agents.cancel".to_owned()));
    let shell_call = w.tool("toolu_fake_shell");
    assert_eq!(shell_call["status"], "denied", "{shell_call}");
    assert!(
        shell_call["note"]
            .as_str()
            .unwrap()
            .contains("the turn was cancelled")
    );
    assert!(
        w.shell
            .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).prompt.is_none())
    );
    assert_eq!(w.agents_state(), StateKind::Ready);
}

#[gpui::test]
fn an_agent_that_exits_is_an_error_until_restarted(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Crasher");
    let g = w.shell.read_with(&w.vcx, |s, _| s.agents().generation);
    w.type_prompt("go");
    w.wait("the exit", |w| w.agents_state() == StateKind::Error);
    let rows = w.transcript();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["error"] == "The agent process exited."),
        "{rows}"
    );
    let state = w.shell.read_with(&w.vcx, |s, cx| s.agents_state(cx));
    assert_eq!(state.state, "error");
    assert!(state.message.unwrap().contains("Restart"));
    // The window's Restart (Start) runs a new session with a new generation.
    w.show_agents();
    w.click(super::window::START_BUTTON);
    w.wait("ready again", |w| w.agents_state() == StateKind::Ready);
    assert_eq!(
        w.shell.read_with(&w.vcx, |s, _| s.agents().generation),
        g + 1
    );
    assert!(w.audit().contains(&"eludite.agents.start".to_owned()));
}

#[gpui::test]
fn a_logged_out_agent_shows_its_login_command(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Logged out");
    w.wait("needs login", |w| w.agents_state() == StateKind::NeedsLogin);
    // The handshake's end does not hide it.
    w.wait("the handshake", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.agents().session_id.is_some())
    });
    assert_eq!(w.agents_state(), StateKind::NeedsLogin);
    w.show_agents();
    assert!(w.vcx.debug_bounds("agents-login").is_some());
    let state = w.shell.read_with(&w.vcx, |s, cx| s.agents_state(cx));
    assert_eq!(state.state, "needs_login");
    assert_eq!(state.login.len(), 1);
    assert!(
        state.login[0]
            .command
            .ends_with("--cli auth login --claudeai"),
        "{}",
        state.login[0].command
    );
    let status = w.shell.read_with(&w.vcx, |s, _| {
        s.status().get(super::AGENTS_SLOT).map(str::to_owned)
    });
    assert_eq!(status.as_deref(), Some("Logged out: needs login"));
}

#[gpui::test]
fn an_outer_agent_drives_the_window_on_the_bus(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    // Ctrl+\, Ctrl+C shows the window; View > Agents is enabled.
    w.vcx.simulate_keystrokes("ctrl-\\ ctrl-c");
    w.vcx.run_until_parked();
    assert!(w.vcx.debug_bounds("agents-window").is_some());
    let menu_ok = w.shell.read_with(&w.vcx, |s, cx| {
        s.menu().read(cx).is_item_enabled("View", "Agents")
    });
    assert_eq!(menu_ok, Some(true));

    let commands = w.commands.clone();
    let c = commands.clone();
    let state = w.agent(move || {
        c.invoke("eludite.agents.start", json!({"agent": "Fake agent"}))
            .unwrap()
    });
    assert_eq!(state["agent"], "Fake agent");
    assert_eq!(state["agents"][1]["name"], "Fake editor");
    w.wait("ready", |w| w.agents_state() == StateKind::Ready);
    let c = commands.clone();
    let state = w.agent(move || {
        c.invoke("eludite.agents.prompt", json!({"text": "go"}))
            .unwrap()
    });
    assert_eq!(state["state"], "running");
    w.wait_for_prompt();
    let c = commands.clone();
    let answered = w.agent(move || {
        c.invoke("eludite.agents.permission", json!({"decision": "deny"}))
            .unwrap()
    });
    assert_eq!(answered["tool"], "Bash");
    assert_eq!(answered["class"], "execute");
    assert_eq!(answered["persisted"], false);
    assert_eq!(w.wait_turn(), "end_turn");
    let c = commands.clone();
    let none = w.agent(move || {
        c.invoke(
            "eludite.agents.review",
            json!({"all": true, "decision": "accept"}),
        )
        .unwrap()
    });
    assert_eq!(none["message"], "no pending change");
    // The solution tree, as agents read it.
    let c = commands.clone();
    let tree = w.agent(move || c.invoke("eludite.solution.tree", json!({})).unwrap());
    assert_eq!(tree["projects"][0]["name"], "App");
    assert!(
        tree["projects"][0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().ends_with("Program.cs"))
    );
}

#[gpui::test]
fn the_endpoint_lists_the_bus_and_a_command_added_at_runtime(cx: &mut TestAppContext) {
    use std::io::{BufRead, BufReader, Write};
    let mut w = setup(cx);
    w.start_agent("Fake agent");
    let (addr, token) = w.shell.read_with(&w.vcx, |s, _| {
        let e = s.agents().endpoint().unwrap();
        (e.addr(), e.token().to_owned())
    });
    let list = |addr, token: &str| {
        let mut sock = std::net::TcpStream::connect(addr).unwrap();
        writeln!(sock, "{token}").unwrap();
        writeln!(sock, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
        let mut line = String::new();
        BufReader::new(sock).read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let names = list(addr, &token);
    for visible in [
        "diagnostics-list",
        "eludite-solution-tree",
        "eludite-file-open",
        "eludite-workspace-apply_edit",
        "eludite-editor-rename",
        "eludite-editor-code_actions",
        "eludite-editor-go_to_definition",
        "eludite-agents-prompt",
    ] {
        assert!(names.iter().any(|n| n == visible), "{visible} in {names:?}");
    }
    assert!(!names.iter().any(|n| n.starts_with("eludite-view-")));
    let expected: Vec<String> = w
        .commands
        .agent_visible()
        .iter()
        .map(|s| eludite_mcp::tool_name(&s.id))
        .collect();
    assert_eq!(names, expected);
    w.commands
        .register(
            eludite_commands::CommandSpec {
                id: eludite_commands::CommandId::new("eludite.test.late").unwrap(),
                title: "Test: Late".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                output_schema: json!({"type": "object", "properties": {}}),
                permission: eludite_commands::PermissionClass::Read,
                agent_visible: true,
            },
            |_| Ok(json!({})),
        )
        .unwrap();
    assert!(list(addr, &token).iter().any(|n| n == "eludite-test-late"));
}
