//! Headless GPUI tests of the Agents window (brief 0016) with the scripted fake ACP agent from brief 0005, run
//! in-process over pipes (its MCP calls reach Eludite's real loopback endpoint with the real token): start, prompt,
//! streamed transcript rows, a read tool answered without a prompt, an edit held as pending changes that are accepted
//! (one undo step) and rejected, an execute tool prompting and denied, cancel mid-turn, an agent exiting and a
//! restart, the logged-out state, permissions persisted per solution, and the audit link from a transcript row to the
//! change it made.
//!
//! Brief 0024's proof (proposal 0002 brief A): the scripted agent fills `act.html` in a real headless Chrome through
//! the MCP endpoint (tab_open, screenshot, read_page, form_input, input, wait, page_text) and reads the result;
//! skipped without a Chrome.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_acp::fake_agent::{self, Options};
use eludite_acp::{AgentDescriptor, AgentSource, RegisteredAgent};
use eludite_docking::ids;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::super::tests::{T, Ws, setup_full};
use super::scenario;
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
    fake_agents(
        AGENTS
            .iter()
            .map(|(name, scenario)| {
                (
                    (*name).to_owned(),
                    vec![
                        "--scenario".into(),
                        (*scenario).into(),
                        "--chunks".into(),
                        "400".into(),
                        "--rate".into(),
                        "2000".into(),
                    ],
                )
            })
            .collect(),
    )
}

/// The fake agent under each name with its arguments (`--scenario ...`), run in-process over pipes, reaching the
/// MCP endpoint directly.
pub(in crate::shell) fn fake_agents(agents: Vec<(String, Vec<String>)>) -> AgentsSetup {
    let registry = agents
        .into_iter()
        .map(|(name, args)| RegisteredAgent {
            descriptor: AgentDescriptor {
                name,
                command: "eludite-fake-acp-agent".into(),
                args,
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
    // The agent's `usage_update` is the line under the turn (brief 0034), in the record too.
    let rows = w.transcript();
    let usage = rows
        .as_array()
        .unwrap()
        .iter()
        .find_map(|r| r.get("usage"))
        .expect("a usage line");
    assert_eq!(
        usage["text"],
        "tokens: 260k in (208k cache read, 52k cache write), 2.9k out, $0.95"
    );
    assert_eq!(usage["input_total"], 259_826);
    assert_eq!(usage["output_tokens"], 2_897);
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

/// Brief 0027: the endpoint serves the debugging guide as an MCP resource, and the session is told it has it.
#[gpui::test]
fn the_endpoint_serves_the_debugging_guide_and_the_session_lists_it(cx: &mut TestAppContext) {
    use std::io::{BufRead, BufReader, Write};
    let mut w = setup(cx);
    w.start_agent("Fake agent");
    let (addr, token) = w.shell.read_with(&w.vcx, |s, _| {
        let e = s.agents().endpoint().unwrap();
        (e.addr(), e.token().to_owned())
    });
    let mut sock = std::net::TcpStream::connect(addr).unwrap();
    writeln!(sock, "{token}").unwrap();
    let mut reader = BufReader::new(sock.try_clone().unwrap());
    let mut ask = |id: u32, method: &str, params: Value| -> Value {
        writeln!(
            sock,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    let init = ask(1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["capabilities"]["resources"], json!({}));
    let list = ask(2, "resources/list", json!({}));
    let resources = list["result"]["resources"].as_array().unwrap();
    assert_eq!(resources[0]["uri"], "eludite://guides/debugging");
    assert_eq!(resources[0]["mimeType"], "text/markdown");
    let read = ask(
        3,
        "resources/read",
        json!({"uri": "eludite://guides/debugging"}),
    );
    let text = read["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("# Debugging with Eludite: a guide for agents"));
    assert!(text.contains("interrupted_by: \"user\""));
    let templates = ask(4, "resources/templates/list", json!({}));
    assert_eq!(templates["result"]["resourceTemplates"], json!([]));
    // The window says which guides the session has, at its start.
    let rows = w.transcript();
    assert!(
        rows.as_array().unwrap().iter().any(|r| r["notice"]
            .as_str()
            .is_some_and(|n| n.starts_with("Starting Fake agent")
                && n.contains("eludite://guides/debugging"))),
        "{rows}"
    );
}

/// Serve `crates/browser/tests/fixtures/` on 127.0.0.1 (the proof's page); answers the base url.
fn serve_fixtures() -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../browser/tests/fixtures");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    return;
                }
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 2 {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/");
                let path = path
                    .split(['?', '#'])
                    .next()
                    .unwrap_or("/")
                    .trim_start_matches('/');
                let file = dir.join(path);
                let (status, body) = match std::fs::read(&file) {
                    Ok(b) if !path.is_empty() && !path.contains("..") => ("200 OK", b),
                    _ => ("404 Not Found", b"not found".to_vec()),
                };
                let mime = if path.ends_with(".html") {
                    "text/html; charset=utf-8"
                } else {
                    "text/plain"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
                let _ = stream.flush();
                let _ = stream.read(&mut [0u8; 1]);
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn running_as_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

#[gpui::test]
fn a_scripted_agent_fills_the_form_in_a_real_chrome_through_mcp(cx: &mut TestAppContext) {
    let chrome = match eludite_browser::ChromeSearch::from_env().find() {
        Ok(p) => p,
        Err(why) => {
            println!("SKIPPED: no Chrome for the fake-agent proof ({why})");
            return;
        }
    };
    if !cfg!(target_os = "linux") && std::env::var_os("ELUDITE_CHROME").is_none_or(|v| v.is_empty())
    {
        println!(
            "SKIPPED: the fake-agent proof runs on Linux, or where ELUDITE_CHROME names a Chrome"
        );
        return;
    }
    let url = format!("{}/act.html", serve_fixtures());
    let setup = fake_agents(vec![(
        "Fake browser".into(),
        vec![
            "--scenario".into(),
            "browser-form".into(),
            "--url".into(),
            url.clone(),
        ],
    )]);
    let mut w = setup_full(cx, |_| {}, Some(setup));
    w.open_solution();
    // The policy runs execute without asking; 127.0.0.1 is an allowed origin, so nothing escalates.
    let file = eludite_commands::policy::AgentPolicy::path_for(w.dir.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"version": 1, "execute": "allow"}"#).unwrap();
    // Headless (no display here), and without the sandbox when this runs as root, as crates/browser's tests do.
    w.agent_invoke(
        eludite_commands::settings::SET,
        json!({"key": "browser.headless", "value": true}),
    )
    .unwrap();
    w.agent_invoke(
        eludite_commands::settings::SET,
        json!({"key": "browser.chromePath", "value": chrome.to_string_lossy()}),
    )
    .unwrap();
    w.wait("the browser settings applied", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.browser().settings().headless)
    });
    let no_sandbox = running_as_root() || eludite_browser::chrome::no_sandbox_from_env();
    w.shell.read_with(&w.vcx, |s, _| {
        s.browser().set_engine_factory(Arc::new(move |config, log| {
            Box::new(
                eludite_browser::ExternalChrome::new(
                    config,
                    eludite_browser::ChromeSearch::defaults(),
                    log,
                )
                .no_sandbox(no_sandbox),
            )
        }))
    });
    w.commands
        .invoke("eludite.view.show", json!({"id": ids::AGENTS}))
        .unwrap();

    w.start_agent("Fake browser");
    let started = Instant::now();
    w.shell
        .update(&mut w.vcx, |s, cx| {
            s.agents_prompt("Fill in the order form", cx)
        })
        .unwrap();
    let stop = w.wait_turn();
    let took = started.elapsed();
    assert_eq!(stop, "end_turn");
    let text = w.agent_text();
    assert!(
        text.contains(
            "The form answered: Ordered: Ada Lovelace, large, free, extras none, gift false"
        ),
        "{text}\n{:#}",
        w.transcript()
    );
    // One row per tool call, each served by Eludite's command and audited; the screenshot as a thumbnail.
    let expected = [
        (
            "eludite-browser-tab_open",
            "eludite.browser.tab_open (execute)",
        ),
        (
            "eludite-browser-screenshot",
            "eludite.browser.screenshot (read)",
        ),
        (
            "eludite-browser-read_page",
            "eludite.browser.read_page (read)",
        ),
        (
            "eludite-browser-form_input",
            "eludite.browser.form_input (execute)",
        ),
        ("eludite-browser-input", "eludite.browser.input (execute)"),
        ("eludite-browser-wait", "eludite.browser.wait (read)"),
        (
            "eludite-browser-page_text",
            "eludite.browser.page_text (read)",
        ),
    ];
    let rows: Vec<Value> = w
        .transcript()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.get("tool_call").cloned())
        .collect();
    assert_eq!(rows.len(), expected.len(), "{rows:#?}");
    for (row, (tool, command)) in rows.iter().zip(expected) {
        assert_eq!(row["tool"], format!("mcp__eludite__{tool}"), "{row}");
        assert_eq!(row["status"], "completed", "{row}");
        assert!(row["note"].as_str().unwrap().contains(command), "{row}");
    }
    w.wait("the screenshot's thumbnail", |w| {
        w.transcript()
            .as_array()
            .unwrap()
            .iter()
            .any(|r| !r["tool_call"]["images"].is_null())
    });
    let shot = w.tool("toolu_fake_step_2");
    let image = &shot["images"][0];
    assert_eq!(image["mime"], "image/png", "{shot}");
    assert_eq!(image["width"], 640);
    assert!(image["thumb_width"].as_u64().unwrap() <= 160);
    assert!(rows.iter().filter(|r| r.get("images").is_some()).count() == 1);
    // Every call is in the audit log as the agent's.
    let audited: Vec<String> = w
        .commands
        .audit_log()
        .entries()
        .into_iter()
        .filter(|e| e.caller.is_agent() && e.command.starts_with("eludite.browser."))
        .map(|e| e.command)
        .collect();
    assert_eq!(audited.len(), 7, "{audited:?}");
    println!(
        "fake-agent proof: prompt to the turn's end, Chrome's launch included: {:.0} ms (budget 10 s)",
        took.as_secs_f64() * 1e3
    );
    assert!(took < Duration::from_secs(10), "{took:?}");
    let done = w.shell.read_with(&w.vcx, |s, _| s.browser().shutdown());
    done.recv_timeout(Duration::from_secs(10)).unwrap();
}

// ----- Brief 0030: the agent debugging proving scenario. -----

/// `corpus/debugging`.
fn corpus() -> std::path::PathBuf {
    std::fs::canonicalize(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/debugging"),
    )
    .expect("corpus/debugging")
}

/// One program's expected answer, from `corpus/debugging/README.md`'s table.
#[derive(Debug)]
struct Expected {
    message: String,
    statement: String,
    line: u64,
    /// (name, part of the value): `this.Path` names a member of a local.
    locals: Vec<(String, String)>,
}

/// The text between each pair of backticks in `cell`.
fn quoted(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

fn expected(program: &str) -> Expected {
    let readme = std::fs::read_to_string(corpus().join("README.md")).unwrap();
    let row = readme
        .lines()
        .find(|l| l.starts_with(&format!("| `{program}` |")))
        .unwrap_or_else(|| panic!("no row for {program} in corpus/debugging/README.md"));
    let cells: Vec<&str> = row.split(" | ").collect();
    let line = quoted(cells[4])[0]
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let locals = quoted(cells[5])
        .chunks(2)
        .map(|p| (p[0].clone(), p[1].clone()))
        .collect();
    Expected {
        message: quoted(cells[2])[0].clone(),
        statement: quoted(cells[3])[0].clone(),
        line,
        locals,
    }
}

/// The value of local `name` (`this.Path`: a member, from the rows' children) in a stop summary's locals.
fn local_value(rows: &Value, name: &str) -> Option<String> {
    let mut parts = name.split('.');
    let first = parts.next()?;
    let mut row = rows.as_array()?.iter().find(|r| r["name"] == first)?;
    for part in parts {
        row = row["children"]
            .as_array()?
            .iter()
            .find(|r| r["name"] == part)?;
    }
    row["value"].as_str().map(str::to_owned)
}

#[test]
fn the_corpus_programs_fail_their_checks_with_the_expected_message_and_line() {
    let root = corpus();
    for program in ["OffByOne", "MissingCase", "NullField"] {
        let e = expected(program);
        // The README's line holds its statement.
        let source = std::fs::read_to_string(root.join(program).join("Program.cs")).unwrap();
        assert_eq!(
            source.lines().nth(e.line as usize - 1).map(str::trim),
            Some(e.statement.as_str()),
            "{program}"
        );
        let dir = root.join(program).join("bin/Debug");
        let runs: [(&str, Vec<String>, std::path::PathBuf); 2] = [
            (
                "dotnet",
                vec![
                    dir.join(format!("net10.0/{program}.dll"))
                        .to_string_lossy()
                        .into_owned(),
                ],
                dir.join(format!("net10.0/{program}.dll")),
            ),
            (
                "mono",
                vec![
                    dir.join(format!("net472/{program}.exe"))
                        .to_string_lossy()
                        .into_owned(),
                ],
                dir.join(format!("net472/{program}.exe")),
            ),
        ];
        for (runtime, args, built) in runs {
            if !built.is_file() {
                println!(
                    "SKIPPED: {program} under {runtime}: not built (corpus/debugging/build.sh)"
                );
                continue;
            }
            if runtime == "mono" && cfg!(windows) {
                continue;
            }
            let out = match std::process::Command::new(runtime).args(&args).output() {
                Ok(o) => o,
                Err(e) => {
                    println!("SKIPPED: {program} under {runtime}: {e}");
                    continue;
                }
            };
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert_eq!(
                out.status.code(),
                Some(1),
                "{program} under {runtime}: {stdout}"
            );
            assert!(
                stdout.trim_end().starts_with(&e.message),
                "{program} under {runtime}: {stdout:?} does not start with {:?}",
                e.message
            );
            println!("{program} under {runtime}: exit 1, {}", stdout.trim_end());
        }
    }
}

/// The adapter the scenario runs a corpus program under: netcoredbg where found, else eludite-dbg-mono under Mono.
enum CorpusAdapter {
    Netcoredbg(std::path::PathBuf),
    Mono {
        prefix: std::path::PathBuf,
        adapter: std::path::PathBuf,
    },
}

impl CorpusAdapter {
    fn find() -> Result<Self, String> {
        if let Ok(found) = eludite_dap::discovery::AdapterSearch::from_env().find_netcoredbg() {
            return Ok(CorpusAdapter::Netcoredbg(found.path));
        }
        if cfg!(windows) {
            return Err(
                "netcoredbg was not found (tools/netcoredbg/fetch.sh, ELUDITE_NETCOREDBG)".into(),
            );
        }
        let mono = eludite_dap::discovery::MonoSearch::from_env()
            .find_mono()
            .map_err(|e| format!("neither netcoredbg nor Mono was found: {e}"))?;
        let adapter = std::env::var_os("ELUDITE_DBG_MONO")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe")
            });
        if !adapter.is_file() {
            return Err(format!(
                "netcoredbg was not found and eludite-dbg-mono is not built at {} (dotnet build dotnet/Eludite.slnx)",
                adapter.display()
            ));
        }
        Ok(CorpusAdapter::Mono {
            prefix: mono.prefix,
            adapter,
        })
    }

    fn name(&self) -> &'static str {
        match self {
            CorpusAdapter::Netcoredbg(_) => "netcoredbg (net10.0)",
            CorpusAdapter::Mono { .. } => "eludite-dbg-mono (net472)",
        }
    }

    /// The built program it runs.
    fn program(&self, dir: &std::path::Path, program: &str) -> std::path::PathBuf {
        match self {
            CorpusAdapter::Netcoredbg(_) => dir.join(format!("bin/Debug/net10.0/{program}.dll")),
            CorpusAdapter::Mono { .. } => dir.join(format!("bin/Debug/net472/{program}.exe")),
        }
    }
}

/// One scenario at a time per corpus program: its project's `.user` file (below) and its build output are shared
/// (brief 0036 runs a second MissingCase scenario).
fn corpus_program_lock(program: &str) -> Arc<std::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Arc<std::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(program.to_owned())
        .or_default()
        .clone()
}

/// Visual Studio's `ActiveDebugFramework` for a corpus project while the scenario runs (net472 under Mono; none, the
/// project's first framework, under netcoredbg); removed at the end.
struct ActiveDebugFramework(std::path::PathBuf);

impl ActiveDebugFramework {
    fn set(project: &std::path::Path, framework: Option<&str>) -> Self {
        let user = std::path::PathBuf::from(format!("{}.user", project.display()));
        match framework {
            Some(f) => std::fs::write(
                &user,
                format!(
                    "<Project>\n  <PropertyGroup>\n    <ActiveDebugFramework>{f}</ActiveDebugFramework>\n  </PropertyGroup>\n</Project>\n"
                ),
            )
            .unwrap(),
            None => {
                let _ = std::fs::remove_file(&user);
            }
        }
        Self(user)
    }
}

impl Drop for ActiveDebugFramework {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// An Agents window offering the fake agent's planned scenario with `planner`, in-process over pipes.
fn planned_agent(name: &str, planner: Arc<std::sync::Mutex<scenario::DebugAgent>>) -> AgentsSetup {
    let mut setup = fake_agents(vec![(
        name.to_owned(),
        vec!["--scenario".into(), "planned".into()],
    )]);
    setup.connect = Some(Arc::new(move |agent, _cwd| {
        let mut opts = Options::from_args(agent.args.clone()).map_err(std::io::Error::other)?;
        opts.mcp_direct = true;
        opts.planner = Some(fake_agent::PlannerHandle(planner.clone()));
        let (agent_in_r, agent_in_w) = std::io::pipe()?;
        let (agent_out_r, agent_out_w) = std::io::pipe()?;
        std::thread::Builder::new()
            .name("fake-agent".into())
            .spawn(move || {
                let _ = fake_agent::run(std::io::BufReader::new(agent_in_r), agent_out_w, opts);
            })?;
        Ok((Box::new(agent_out_r), Box::new(agent_in_w)))
    }));
    setup
}

impl Ws {
    /// Set a path setting through the bus and wait until `applied` sees it.
    fn set_path(&mut self, key: &str, value: &std::path::Path, applied: impl Fn(&Shell) -> bool) {
        self.commands
            .invoke(
                eludite_commands::settings::SET,
                json!({"key": key, "value": value.to_string_lossy()}),
            )
            .unwrap();
        self.wait(key, |w| w.shell.read_with(&w.vcx, |s, _| applied(s)));
    }

    /// Wait up to `limit` for `done`.
    fn wait_long(&mut self, what: &str, limit: Duration, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + limit;
        loop {
            self.vcx.run_until_parked();
            if done(self) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// The summary budget of brief 0025, brief 0034's for a whole scenario's answers (brief 0030's was 40 KB) and for the
/// compact `toggle_breakpoint` answer.
const SUMMARY_BUDGET: usize = 8 * 1024;
const SCENARIO_BUDGET: usize = 30 * 1024;
const TOGGLE_BUDGET: usize = 500;

/// Brief 0030's scripted scenario for corpus program `program`, re-run by brief 0034 with the new answers: the fake
/// agent, given the proving prompt, debugs it through Eludite's MCP tools (`steps` step_overs after its breakpoint's
/// stop) against the adapter found here; the test checks the stop against the README's answer, counts the debug calls
/// from the audit log (seven or fewer: the threshold is eight), the answers' sizes (each summary under 8 KB, the
/// `toggle_breakpoint` answer under 500 bytes, all under 30 KB, no `(null)`) and the time (under 15 s, adapter launches
/// included).
fn debug_scenario(cx: &mut TestAppContext, program: &str, steps: usize) {
    debug_scenario_with(cx, program, scenario::DebugAgent::new(steps), 7);
}

/// [`debug_scenario`] with a planner of the test's (brief 0036: a breakpoint condition and its replacement) and at most
/// `max_debug_calls` debug calls; the reasons the planner read in `breakpoints_failed` and every debug answer, or
/// `None` when the scenario was skipped.
fn debug_scenario_with(
    cx: &mut TestAppContext,
    program: &str,
    agent: scenario::DebugAgent,
    max_debug_calls: usize,
) -> Option<(Vec<String>, Vec<Value>)> {
    let dir = corpus().join(program);
    let project = dir.join(format!("{program}.csproj"));
    let adapter = match CorpusAdapter::find() {
        Ok(a) => a,
        Err(why) => {
            println!("SKIPPED: the {program} scenario: {why}");
            return None;
        }
    };
    if !adapter.program(&dir, program).is_file() {
        println!(
            "SKIPPED: the {program} scenario: {} is not built (corpus/debugging/build.sh)",
            adapter.program(&dir, program).display()
        );
        return None;
    }
    let lock = corpus_program_lock(program);
    let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
    let _framework = ActiveDebugFramework::set(
        &project,
        matches!(adapter, CorpusAdapter::Mono { .. }).then_some("net472"),
    );
    let e = expected(program);
    let planner = Arc::new(std::sync::Mutex::new(agent));
    let store = tempfile::tempdir().unwrap();
    let debug = crate::shell::debug::DebugSetup {
        connect: None,
        search: eludite_dap::discovery::AdapterSearch::default(),
        mono: eludite_dap::discovery::MonoSearch::default(),
        mono_adapter: eludite_dap::discovery::MonoAdapterSearch::default(),
        platform: eludite_dap::launch::Platform::current(),
        store_dir: Some(store.path().to_path_buf()),
        dotnet: "dotnet".into(),
        js: Default::default(),
    };
    let mut w = super::super::tests::setup_debug(
        cx,
        |_| {},
        Some(planned_agent("Fake debugger", planner.clone())),
        Some(debug),
    );
    match &adapter {
        CorpusAdapter::Netcoredbg(path) => {
            let want = Some(path.clone().into_os_string());
            w.set_path("debugger.netcoredbgPath", path, move |s| {
                s.debugger().setup().search.env == want
            });
        }
        CorpusAdapter::Mono { prefix, adapter } => {
            let (p, a) = (Some(prefix.clone()), Some(adapter.clone()));
            w.set_path("debugger.monoPrefix", prefix, move |s| {
                s.debugger().setup().mono.configured == p
            });
            w.set_path("debugger.monoAdapterPath", adapter, move |s| {
                s.debugger().setup().mono_adapter.configured == a
            });
        }
    }
    w.open_solution();
    // The corpus is built (corpus/debugging/build.sh): F5 launches it at once.
    w.commands
        .invoke(
            eludite_commands::settings::SET,
            json!({"key": "build.beforeRun", "value": false}),
        )
        .unwrap();
    w.wait("build before run off", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| !s.builds().build_before_run)
    });
    // The policy runs execute without asking; the debug policy's defaults let agents drive and evaluate.
    let file = eludite_commands::policy::AgentPolicy::path_for(w.dir.path());
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{"version": 1, "execute": "allow"}"#).unwrap();

    w.start_agent("Fake debugger");
    let started = Instant::now();
    let prompt = scenario::prompt(&project);
    w.shell
        .update(&mut w.vcx, |s, cx| s.agents_prompt(&prompt, cx))
        .unwrap();
    w.wait_long("the scenario's turn", Duration::from_secs(120), |w| {
        w.turn_ended()
    });
    let took = started.elapsed();
    let stop = w
        .shell
        .read_with(&w.vcx, |s, _| s.agents().last_stop.clone().unwrap());
    let text = w.agent_text();
    let p = planner.lock().unwrap();
    assert_eq!(stop, "end_turn", "{text}");
    assert!(p.gave_up.is_none(), "{text}\n{:#?}", p.calls);

    // The calls, as the audit log has them: at most eight debug commands, all the agent's.
    let audited: Vec<String> = w
        .commands
        .audit_log()
        .entries()
        .into_iter()
        .filter(|e| e.caller.is_agent())
        .map(|e| e.command)
        .collect();
    let debug_calls: Vec<&String> = audited
        .iter()
        .filter(|c| c.starts_with("eludite.debug."))
        .collect();
    println!(
        "{program} scenario under {}: {} tool calls, {} debug calls, {:.0} ms from the prompt to the turn's end",
        adapter.name(),
        audited.len(),
        debug_calls.len(),
        took.as_secs_f64() * 1e3
    );
    let mut total = 0;
    for (n, c) in p.calls.iter().enumerate() {
        total += c.bytes;
        println!(
            "  {:>2}. {:<34} {:>6} bytes {:>8.1} ms",
            n + 1,
            c.tool,
            c.bytes,
            c.ms
        );
    }
    println!("  answers: {total} bytes in all\n  the agent's answer: {text}");
    assert_eq!(audited.len(), p.calls.len(), "{audited:?}");
    assert!(debug_calls.len() <= max_debug_calls, "{debug_calls:?}");

    // The stop: the README's statement, with the locals that show the bug.
    let last = p.last.clone().expect("a stop summary");
    let location = &last["stopped"]["location"];
    let path = std::path::PathBuf::from(location["path"].as_str().unwrap_or_default());
    assert!(
        path.ends_with(std::path::Path::new(program).join("Program.cs")),
        "{location}"
    );
    assert_eq!(location["line"], e.line, "{last:#}");
    for (name, value) in &e.locals {
        let shown = local_value(&last["locals"]["rows"], name);
        assert!(
            shown.as_deref().is_some_and(|v| v.contains(value.as_str())),
            "{name}: {shown:?}, want {value:?}: {:#}",
            last["locals"]
        );
        assert!(text.contains(&format!("- {name} = ")), "{text}");
    }
    assert!(
        text.contains(&format!("Program.cs:{}", e.line)),
        "the answer names the line: {text}"
    );

    // Token cost: each summary under brief 0025's 8 KB, the whole scenario under 40 KB.
    for c in p
        .calls
        .iter()
        .filter(|c| c.tool.starts_with("eludite-debug-"))
    {
        assert!(
            c.bytes < SUMMARY_BUDGET,
            "{} answered {} bytes",
            c.tool,
            c.bytes
        );
        if c.tool == "eludite-debug-toggle_breakpoint" {
            assert!(
                c.bytes < TOGGLE_BUDGET,
                "toggle_breakpoint answered {} bytes",
                c.bytes
            );
        }
        // One spelling of null on every adapter (brief 0034).
        if let Ok(r) = &c.result {
            assert!(!r.to_string().contains("\"(null)\""), "{}: {r}", c.tool);
        }
    }
    assert!(total < SCENARIO_BUDGET, "{total} bytes");
    assert!(took < Duration::from_secs(15), "{took:?}");
    let read = (
        p.failures_read.clone(),
        p.calls
            .iter()
            .filter(|c| c.tool.starts_with("eludite-debug-"))
            .filter_map(|c| c.result.clone().ok())
            .collect(),
    );
    drop(p);

    // End the session the scenario left at its break.
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke("eludite.debug.stop", json!({}), window, cx)
        })
        .unwrap();
    w.wait_long("the session's end", Duration::from_secs(20), |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger().model.mode == crate::shell::debug::state::Mode::Design
        })
    });
    Some(read)
}

#[gpui::test]
fn a_scripted_agent_finds_the_off_by_one_in_the_corpus(cx: &mut TestAppContext) {
    debug_scenario(cx, "OffByOne", 1);
}

#[gpui::test]
fn a_scripted_agent_finds_the_missing_case_in_the_corpus(cx: &mut TestAppContext) {
    debug_scenario(cx, "MissingCase", 1);
}

#[gpui::test]
fn a_scripted_agent_finds_the_null_field_in_the_corpus(cx: &mut TestAppContext) {
    debug_scenario(cx, "NullField", 1);
}

/// Brief 0036: a condition the adapter rejects never stops, and the answer the agent reads next says why. The scripted
/// agent breaks in `Coins.Cents` on `coin == Money.Quarter` (no such type): the run ends, the `wait` answer's
/// `breakpoints_failed` carries the adapter's reason, the agent sets the unqualified `coin == Coin.Quarter` instead
/// (which `eludite-dbg-mono` resolves from the method's namespace) and reaches the faulting statement.
#[gpui::test]
fn a_scripted_agent_reads_why_a_wrong_condition_never_stopped(cx: &mut TestAppContext) {
    // The replacement is one the adapter evaluates: netcoredbg's evaluator has no enum `==` (CS0019) and no casts, so
    // it compares the enum's backing field; eludite-dbg-mono and the fake take the C# spelling.
    let replacement = match CorpusAdapter::find() {
        Ok(CorpusAdapter::Netcoredbg(_)) => "coin.value__ == 3",
        _ => "coin == Coin.Quarter",
    };
    let agent = scenario::DebugAgent::new(1).with_condition("coin == Money.Quarter", replacement);
    let Some((read, answers)) = debug_scenario_with(cx, "MissingCase", agent, 10) else {
        return;
    };
    assert_eq!(read.len(), 1, "{read:?}");
    assert!(read[0].contains("Money"), "{read:?}");
    // The reason is in an answer the agent received: the end-of-session summary of the run that never stopped (an
    // adapter that refuses the condition up front), or the break's summary (netcoredbg evaluates it at the hit).
    let carried: Vec<&Value> = answers
        .iter()
        .filter(|a| {
            a["breakpoints_failed"]
                .as_array()
                .is_some_and(|f| f.iter().any(|r| r["message"] == read[0].as_str()))
        })
        .collect();
    assert!(!carried.is_empty(), "{answers:#?}");
    assert!(
        matches!(carried[0]["mode"].as_str(), Some("design" | "break")),
        "{}",
        carried[0]
    );
}

/// Brief 0043: a link in the agent's Markdown is clickable; a file link opens the file in the editor at its line.
#[gpui::test]
fn a_file_link_in_the_agents_message_opens_the_file_at_its_line(cx: &mut TestAppContext) {
    use eludite_acp::protocol::{ContentBlock, SessionUpdate};
    let mut w = setup(cx);
    w.show_agents();
    let window = w.shell.read_with(&w.vcx, |s, _| s.agents().window.clone());
    // The link first, so the click at the row's start is on it.
    let ix = window.update(&mut w.vcx, |win, cx| {
        let ix = win.transcript.rows.len();
        win.transcript
            .apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
                "[Program.cs line 3](src/App/Program.cs#L3) has `Main`.\n\n",
            )));
        win.transcript.notice("Turn ended");
        win.sync(cx);
        win.reveal(ix, cx);
        ix
    });
    w.vcx.run_until_parked();
    let row = w.bounds(&super::window::agent_text(ix));
    // px_3 padding, then the first characters of the link.
    let at = gpui::point(row.left() + gpui::px(20.), row.top() + gpui::px(8.));
    w.vcx.simulate_click(at, gpui::Modifiers::none());
    w.vcx.run_until_parked();
    let program = w.path("src/App/Program.cs");
    let view = w.editor(&program);
    let active = w.shell.read_with(&w.vcx, |s, _| s.active_document());
    assert_eq!(
        active.as_deref().map(std::path::Path::new),
        Some(program.as_path())
    );
    let caret_row = view.read_with(&w.vcx, |v, _| v.editor().primary_head().row);
    assert_eq!(caret_row, 2, "line 3");
}
