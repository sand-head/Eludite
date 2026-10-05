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

/// The registry the tests offer, by name: each runs the fake agent with a scenario, then its own arguments (after
/// the common ones, so they win).
const AGENTS: [(&str, &str); 8] = [
    // Brief 0059: its turns end with the stream scenario's `usage_update`, so the strip fills after a tool call.
    ("Fake agent", "diagnostics-then-shell --usage"),
    ("Fake editor", "edit"),
    ("Fake writer", "write"),
    ("Fake streamer", "stream"),
    ("Logged out", "login-required"),
    ("Crasher", "exit"),
    // Brief 0058: modes and config options, and a turn long enough to change them during it (1 s).
    ("Fake options", "stream --options --chunks 2000"),
    // Brief 0058: modes and config options, and a turn held open at a shell command's permission prompt until the
    // test answers it.
    ("Fake asker", "diagnostics-then-shell --options"),
];

fn fake_setup() -> AgentsSetup {
    fake_agents(
        AGENTS
            .iter()
            .map(|(name, scenario)| {
                let mut words = scenario.split_whitespace().map(str::to_owned);
                let mut args = vec![
                    "--scenario".into(),
                    words.next().unwrap(),
                    "--chunks".into(),
                    "400".into(),
                    "--rate".into(),
                    "2000".into(),
                ];
                args.extend(words);
                ((*name).to_owned(), args)
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
        agents_file: None,
        credentials: memory_credentials(),
        openai_adapter: None,
        sessions_root: None,
    }
}

/// A credential store in memory (brief 0060): tests never touch the machine's keyring.
fn memory_credentials() -> Arc<eludite_forge::credentials::Credentials> {
    Arc::new(eludite_forge::credentials::Credentials::new(
        Box::new(eludite_forge::credentials::MemoryStore::new()),
        None,
    ))
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

    /// Type `text` in the prompt box without sending it (the box must have focus).
    fn type_keys(&mut self, text: &str) {
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
        self.vcx.run_until_parked();
    }

    /// The prompt box's text and caret (brief 0057).
    fn prompt_box(&self) -> (String, usize) {
        self.shell.read_with(&self.vcx, |s, cx| {
            let input = s.agents().window.read(cx).input.read(cx);
            (input.text(), input.caret())
        })
    }

    /// The slash menu's commands and selected row, when it is open.
    fn slash_menu(&self) -> Option<(Vec<String>, usize)> {
        self.shell
            .read_with(&self.vcx, |s, cx| s.agents().window.read(cx).menu_items(cx))
    }

    fn user_rows(&self) -> Vec<String> {
        self.transcript()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["user"].as_str().map(str::to_owned))
            .collect()
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
    // Brief 0059: the agent's thought is one collapsed line that says how long it thought.
    let thought = w.agents_window(|w, _| {
        w.transcript.rows.iter().find_map(|r| match r {
            super::transcript::Row::Thought(t) => Some((t.label(), t.expanded)),
            _ => None,
        })
    });
    let (label, expanded) = thought.expect("the edit scenario thinks first");
    assert!(
        label.starts_with("Thought for ") && label.ends_with(" s"),
        "{label}"
    );
    assert!(!expanded);
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
    // The window's Restart (Start) runs a new session with a new generation. Brief 0059: the error is the bordered
    // block under the header.
    w.show_agents();
    assert!(w.vcx.debug_bounds("agents-error").is_some());
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

/// Brief 0057: the prompt box is an editor. A prompt edited in the middle arrives as edited; Up recalls it and Enter
/// sends it again; the box keeps its text and caret while the window is hidden; Enter with only whitespace sends
/// nothing.
#[gpui::test]
fn the_prompt_box_edits_at_the_caret_and_recalls_prompts(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Fake streamer");
    w.show_agents();
    w.click(super::window::PROMPT_BOX);
    w.type_keys("world");
    w.vcx.simulate_keystrokes("home");
    w.type_keys("hello ");
    w.vcx.simulate_keystrokes("end");
    w.type_keys("!");
    assert_eq!(w.prompt_box(), ("hello world!".into(), 12));
    w.vcx.simulate_keystrokes("enter");
    w.vcx.run_until_parked();
    assert_eq!(w.wait_turn(), "end_turn");
    assert_eq!(w.user_rows(), ["hello world!"]);
    assert_eq!(w.prompt_box(), (String::new(), 0), "sending clears the box");
    // Up on the empty box recalls the last prompt; Enter sends it again.
    w.click(super::window::PROMPT_BOX);
    w.vcx.simulate_keystrokes("up");
    w.vcx.run_until_parked();
    assert_eq!(w.prompt_box().0, "hello world!");
    // Down past the newest prompt is the empty box again; Up brings it back.
    w.vcx.simulate_keystrokes("down");
    w.vcx.run_until_parked();
    assert_eq!(w.prompt_box().0, "");
    w.vcx.simulate_keystrokes("up enter");
    w.vcx.run_until_parked();
    assert_eq!(w.wait_turn(), "end_turn");
    assert_eq!(w.user_rows(), ["hello world!", "hello world!"]);
    // Up does not replace a draft.
    w.click(super::window::PROMPT_BOX);
    w.type_keys("draft");
    w.vcx.simulate_keystrokes("left up");
    w.vcx.run_until_parked();
    assert_eq!(w.prompt_box(), ("draft".into(), 4));
    // Hidden behind Workspace and shown again, the box keeps the draft and its caret.
    w.commands
        .invoke("eludite.view.show", json!({"id": ids::WORKSPACE}))
        .unwrap();
    w.vcx.run_until_parked();
    assert!(w.vcx.debug_bounds(super::window::PROMPT_BOX).is_none());
    w.show_agents();
    assert_eq!(w.prompt_box(), ("draft".into(), 4));
    // Only whitespace: nothing is sent.
    w.click(super::window::PROMPT_BOX);
    w.vcx
        .simulate_keystrokes("ctrl-a backspace space shift-enter space enter");
    w.vcx.run_until_parked();
    assert_eq!(w.user_rows().len(), 2);
    let history = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents().window.read(cx).history().to_vec()
    });
    assert_eq!(history, ["hello world!"], "a repeat is kept once");
}

/// Brief 0057: the agent's slash commands in the menu above the prompt box, filtered as the person types, completed
/// with Tab, Enter or a click, closed with Escape; the state output lists them; an agent with none has no menu.
#[gpui::test]
fn the_slash_menu_offers_the_agents_commands(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Fake streamer");
    w.wait("the agent's commands", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            !s.agents().window.read(cx).transcript.commands.is_empty()
        })
    });
    // The state output lists them, as the menu does (agents-state.output.json's `commands`).
    let state = w.shell.read_with(&w.vcx, |s, cx| {
        serde_json::to_value(s.agents_state(cx)).unwrap()
    });
    assert_eq!(
        state["commands"],
        json!([
            {"name": "compact", "description": "Free up context by summarizing the conversation so far",
             "hint": "<optional custom summarization instructions>"},
            {"name": "model", "description": "Set the AI model for Claude Code", "hint": "<model>"}
        ])
    );
    w.show_agents();
    w.click(super::window::PROMPT_BOX);
    w.type_keys("/");
    assert_eq!(
        w.slash_menu(),
        Some((vec!["compact".to_owned(), "model".to_owned()], 0))
    );
    assert!(w.vcx.debug_bounds(super::window::SLASH_MENU).is_some());
    let menu = w.vcx.debug_bounds(super::window::SLASH_MENU).unwrap();
    let prompt = w.vcx.debug_bounds(super::window::PROMPT_BOX).unwrap();
    assert!(
        menu.bottom() <= prompt.top() + gpui::px(1.),
        "above the box: {menu:?} {prompt:?}"
    );
    assert!(
        w.vcx
            .debug_bounds(String::leak(super::window::slash_item("model")))
            .is_some()
    );
    // Down and Up move the selection, wrapping.
    w.vcx.simulate_keystrokes("down");
    assert_eq!(w.slash_menu().unwrap().1, 1);
    w.vcx.simulate_keystrokes("down");
    assert_eq!(w.slash_menu().unwrap().1, 0);
    w.vcx.simulate_keystrokes("up");
    assert_eq!(w.slash_menu().unwrap().1, 1);
    // Typing filters: `co` is in `compact` only.
    w.type_keys("co");
    assert_eq!(w.slash_menu(), Some((vec!["compact".to_owned()], 0)));
    // Escape closes the menu and leaves the text; it does not cancel anything.
    w.vcx.simulate_keystrokes("escape");
    w.vcx.run_until_parked();
    assert_eq!(w.slash_menu(), None);
    assert!(w.vcx.debug_bounds(super::window::SLASH_MENU).is_none());
    assert_eq!(w.prompt_box(), ("/co".into(), 3));
    assert!(!w.audit().contains(&"eludite.agents.cancel".to_owned()));
    // Typing again reopens it; Tab completes to `/compact ` with the caret after the space, and the menu closes.
    w.type_keys("m");
    assert_eq!(w.slash_menu(), Some((vec!["compact".to_owned()], 0)));
    w.vcx.simulate_keystrokes("tab");
    w.vcx.run_until_parked();
    assert_eq!(w.prompt_box(), ("/compact ".into(), 9));
    assert_eq!(w.slash_menu(), None);
    // Enter sends it; the fake agent answers a slash command with one line naming it.
    w.vcx.simulate_keystrokes("enter");
    w.vcx.run_until_parked();
    assert_eq!(w.wait_turn(), "end_turn");
    assert_eq!(w.user_rows(), ["/compact"]);
    assert!(
        w.agent_text()
            .contains(&eludite_acp::fake_agent::slash_reply("/compact"))
    );
    // Enter on a partial name completes and does not send; on the full name it sends (`/model` Enter Enter).
    w.click(super::window::PROMPT_BOX);
    w.type_keys("/mo");
    w.vcx.simulate_keystrokes("enter");
    w.vcx.run_until_parked();
    assert_eq!(w.prompt_box(), ("/model ".into(), 7));
    assert_eq!(w.user_rows().len(), 1);
    w.vcx.simulate_keystrokes("ctrl-a backspace");
    w.type_keys("/model");
    w.vcx.simulate_keystrokes("enter");
    w.vcx.run_until_parked();
    assert_eq!(w.wait_turn(), "end_turn");
    assert_eq!(w.user_rows(), ["/compact", "/model"]);
    // A click on a row completes it.
    w.click(super::window::PROMPT_BOX);
    w.type_keys("/");
    w.click(&super::window::slash_item("model"));
    assert_eq!(w.prompt_box(), ("/model ".into(), 7));
    // Backspace past the `/` closes the menu.
    w.vcx.simulate_keystrokes("ctrl-a backspace");
    w.type_keys("/");
    assert!(w.slash_menu().is_some());
    w.vcx.simulate_keystrokes("backspace");
    w.vcx.run_until_parked();
    assert_eq!(w.slash_menu(), None);
    // Not in the first word: no menu.
    w.type_keys("/model x");
    assert_eq!(w.slash_menu(), None);

    // An agent with no commands: `/` is plain text, and the state lists none.
    w.start_agent("Fake editor");
    w.click(super::window::PROMPT_BOX);
    w.vcx.simulate_keystrokes("ctrl-a backspace");
    w.type_keys("/co");
    assert_eq!(w.slash_menu(), None);
    assert!(w.vcx.debug_bounds(super::window::SLASH_MENU).is_none());
    assert_eq!(w.prompt_box(), ("/co".into(), 3));
    let state = w.shell.read_with(&w.vcx, |s, cx| {
        serde_json::to_value(s.agents_state(cx)).unwrap()
    });
    assert_eq!(state["commands"], json!([]));
}

impl Ws {
    /// `agents-state.output.json` as the bus would answer it.
    fn state_json(&self) -> Value {
        self.shell.read_with(&self.vcx, |s, cx| {
            serde_json::to_value(s.agents_state(cx)).unwrap()
        })
    }

    /// The state's option `id`'s current value.
    fn option_current(&self, id: &str) -> Option<String> {
        let state = self.state_json();
        state["options"]
            .as_array()?
            .iter()
            .find(|o| o["id"] == id)
            .and_then(|o| o["current"].as_str().map(str::to_owned))
    }

    /// What picker `key`'s button says, and whether it is muted.
    fn picker(&self, key: &str) -> Option<(String, bool)> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().window.read(cx).picker_label(key)
        })
    }

    fn open_picker(&self) -> Option<(String, usize)> {
        self.shell
            .read_with(&self.vcx, |s, cx| s.agents().window.read(cx).open_picker())
    }

    /// A setting's effective value and where it came from.
    fn setting(&self, key: &str) -> (Value, eludite_commands::settings::SettingSource) {
        self.shell
            .read_with(&self.vcx, |s, _| s.settings.lock().effective(key))
    }

    fn wait_options(&mut self) {
        self.wait("the agent's options", |w| {
            w.option_current("model").is_some()
        });
    }

    fn configure(&mut self, option: &str, value: &str) -> Result<Value, String> {
        let commands = self.commands.clone();
        let args = json!({"option": option, "value": value});
        self.agent(move || {
            commands
                .invoke("eludite.agents.configure", args)
                .map_err(|e| e.to_string())
        })
    }
}

/// Brief 0058: the footer's pickers show the fake agent's model, effort and mode; picking a model runs
/// `eludite.agents.configure`, shows the pick muted until the agent answers, remembers it in the user settings and the
/// next session starts with it; the keyboard works the picker; a choice the agent refuses reverts with an error row.
#[gpui::test]
fn the_footer_picks_the_model_and_the_next_session_starts_with_it(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.show_agents();
    w.start_agent("Fake options");
    w.wait_options();
    w.vcx.run_until_parked();
    assert_eq!(w.picker("model"), Some(("Smart \u{25BE}".into(), false)));
    assert_eq!(w.picker("effort"), Some(("Default \u{25BE}".into(), false)));
    assert_eq!(w.picker("mode"), Some(("Manual \u{25BE}".into(), false)));
    // Left to right: model, effort, mode, then Send, all under the prompt box.
    let at = |w: &mut Ws, sel: &str| w.bounds(sel);
    let model = at(&mut w, &super::window::option_picker("model"));
    let effort = at(&mut w, &super::window::option_picker("effort"));
    let mode = at(&mut w, super::window::MODE_PICKER);
    let send = at(&mut w, super::window::SEND_BUTTON);
    let prompt = at(&mut w, super::window::PROMPT_BOX);
    assert!(
        model.left() < effort.left() && effort.left() < mode.left() && mode.right() < send.left()
    );
    for b in [model, effort, mode, send] {
        assert!(
            b.top() >= prompt.bottom(),
            "under the box: {b:?} {prompt:?}"
        );
    }
    // The state output and the status bar.
    let state = w.state_json();
    assert_eq!(state["mode"]["current"], "default");
    assert_eq!(state["mode"]["available"][1]["name"], "Plan");
    assert_eq!(
        state["options"][0]["choices"][0],
        json!({"value": "fast", "name": "Fast", "description": "Quick answers"})
    );
    assert_eq!(state["options"][1]["category"], "thought_level");
    let status = w.shell.read_with(&w.vcx, |s, _| {
        s.status().get(super::AGENTS_SLOT).map(str::to_owned)
    });
    assert_eq!(status.as_deref(), Some("Fake options: ready \u{b7} Smart"));

    // Pick Fast with the mouse: the list, the current one checked, then the command.
    w.click(&super::window::option_picker("model"));
    assert_eq!(
        w.open_picker(),
        Some(("model".into(), 1)),
        "the current choice is highlighted"
    );
    assert!(w.vcx.debug_bounds(super::window::PICKER_MENU).is_some());
    w.click(&super::window::option_item("model", "fast"));
    assert_eq!(w.open_picker(), None);
    assert_eq!(w.picker("model").unwrap().0, "Fast \u{25BE}");
    assert!(w.audit().contains(&"eludite.agents.configure".to_owned()));
    w.wait("the agent's answer", |w| {
        w.option_current("model").as_deref() == Some("fast")
    });
    w.vcx.run_until_parked();
    assert_eq!(w.picker("model"), Some(("Fast \u{25BE}".into(), false)));
    let (value, source) = w.setting("agents.model");
    assert_eq!(
        (value, source),
        (
            json!("fast"),
            eludite_commands::settings::SettingSource::User
        )
    );
    let status = w.shell.read_with(&w.vcx, |s, _| {
        s.status().get(super::AGENTS_SLOT).map(str::to_owned)
    });
    assert_eq!(status.as_deref(), Some("Fake options: ready \u{b7} Fast"));

    // The keyboard: Down, Down, Enter picks High; Escape closes a picker without a pick or a cancel.
    w.click(&super::window::option_picker("effort"));
    assert_eq!(w.open_picker(), Some(("effort".into(), 0)));
    w.vcx.simulate_keystrokes("down down");
    w.vcx.run_until_parked();
    assert_eq!(w.open_picker(), Some(("effort".into(), 2)));
    w.vcx.simulate_keystrokes("enter");
    w.vcx.run_until_parked();
    assert_eq!(w.open_picker(), None);
    w.wait("the effort", |w| {
        w.option_current("effort").as_deref() == Some("high")
    });
    assert_eq!(w.setting("agents.effort").0, json!("high"));
    assert!(w.user_rows().is_empty(), "Enter picked, it sent nothing");
    w.click(super::window::MODE_PICKER);
    assert_eq!(w.open_picker(), Some(("mode".into(), 0)));
    w.vcx.simulate_keystrokes("up escape");
    w.vcx.run_until_parked();
    assert_eq!(w.open_picker(), None);
    assert!(!w.audit().contains(&"eludite.agents.cancel".to_owned()));
    // A click on the open picker's button closes it.
    w.click(super::window::MODE_PICKER);
    w.click(super::window::MODE_PICKER);
    assert_eq!(w.open_picker(), None);

    // A choice the agent refuses: the picker reverts and the transcript says why.
    w.click(&super::window::option_picker("effort"));
    w.click(&super::window::option_item("effort", "max"));
    w.wait("the refusal", |w| {
        w.transcript().as_array().unwrap().iter().any(|r| {
            r["error"]
                .as_str()
                .is_some_and(|e| e.contains(fake_agent::REFUSED_WHY))
        })
    });
    w.vcx.run_until_parked();
    assert_eq!(w.picker("effort"), Some(("High \u{25BE}".into(), false)));
    assert_eq!(w.setting("agents.effort").0, json!("high"));

    // The next session starts with the remembered model and effort (the fake agent sets its current values from
    // `session/new`'s `_meta.claudeCode.options`).
    w.start_agent("Fake options");
    w.wait_options();
    assert_eq!(w.option_current("model").as_deref(), Some("fast"));
    assert_eq!(w.option_current("effort").as_deref(), Some("high"));
    assert_eq!(
        w.state_json()["mode"]["current"],
        "default",
        "the mode is not remembered"
    );
}

/// Brief 0058: `eludite.agents.configure` from the bus answers once the agent has and writes no setting; it fails
/// with `busy` during a turn (when the pickers are disabled), and with `unknown option`, `unknown value` or (an agent
/// with no options, which shows no pickers) `not supported`.
#[gpui::test]
fn configure_on_the_bus_sets_the_mode_and_is_refused_during_a_turn(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.show_agents();
    w.start_agent("Fake asker");
    w.wait_options();
    let state = w.configure("mode", "plan").unwrap();
    assert_eq!(
        state["mode"]["current"], "plan",
        "answered after the agent did"
    );
    w.vcx.run_until_parked();
    assert_eq!(w.picker("mode"), Some(("Plan \u{25BE}".into(), false)));
    let state = w.configure("model", "fast").unwrap();
    assert_eq!(state["options"][0]["current"], "fast");
    // An agent's choice is not remembered.
    assert_eq!(w.setting("agents.model").0, json!(""));
    assert_eq!(w.setting("agents.effort").0, json!(""));
    let e = w.configure("speed", "1").unwrap_err();
    assert!(e.contains("unknown option"), "{e}");
    let e = w.configure("model", "huge").unwrap_err();
    assert!(e.contains("unknown value"), "{e}");
    let e = w.configure("mode", "bypassPermissions").unwrap_err();
    assert!(e.contains("unknown value"), "{e}");

    // During a turn, held open at the shell command's permission prompt: busy, and the pickers are muted and open
    // nothing.
    w.shell
        .update(&mut w.vcx, |s, cx| s.agents_prompt("List the errors", cx))
        .unwrap();
    let request = w.wait_for_prompt();
    assert_eq!(w.agents_state(), StateKind::Running);
    let e = w.configure("model", "smart").unwrap_err();
    assert!(e.starts_with("busy") || e.contains("busy:"), "{e}");
    assert_eq!(w.picker("model"), Some(("Fast \u{25BE}".into(), true)));
    w.click(&super::window::option_picker("model"));
    assert_eq!(w.open_picker(), None);
    assert!(w.vcx.debug_bounds(super::window::PICKER_MENU).is_none());
    assert_eq!(w.option_current("model").as_deref(), Some("fast"));
    // Deny the command; the turn ends and the pickers work again.
    w.shell
        .update(&mut w.vcx, |s, cx| {
            s.agents_answer(request, super::window::Decision::Deny, cx)
        })
        .unwrap();
    assert_eq!(w.wait_turn(), "end_turn");
    w.vcx.run_until_parked();
    assert_eq!(w.picker("model"), Some(("Fast \u{25BE}".into(), false)));
    w.click(&super::window::option_picker("model"));
    assert_eq!(w.open_picker(), Some(("model".into(), 0)));

    // An agent without options: no pickers, and the command is not supported.
    w.start_agent("Fake streamer");
    w.vcx.run_until_parked();
    assert_eq!(w.picker("model"), None);
    assert!(w.vcx.debug_bounds(super::window::MODE_PICKER).is_none());
    assert!(
        w.vcx
            .debug_bounds(String::leak(super::window::option_picker("model")))
            .is_none()
    );
    let state = w.state_json();
    assert!(
        state.get("mode").is_none() && state.get("options").is_none(),
        "{state}"
    );
    let e = w.configure("model", "fast").unwrap_err();
    assert!(e.contains("not supported"), "{e}");
    let status = w.shell.read_with(&w.vcx, |s, _| {
        s.status().get(super::AGENTS_SLOT).map(str::to_owned)
    });
    assert_eq!(status.as_deref(), Some("Fake streamer: ready"));
}

impl Ws {
    /// `inner` lies within `outer` (to the pixel's rounding).
    fn assert_within(
        what: &str,
        inner: gpui::Bounds<gpui::Pixels>,
        outer: gpui::Bounds<gpui::Pixels>,
    ) {
        let e = gpui::px(0.5);
        assert!(
            inner.left() >= outer.left() - e
                && inner.right() <= outer.right() + e
                && inner.top() >= outer.top() - e
                && inner.bottom() <= outer.bottom() + e,
            "{what} {inner:?} leaves the Agents window {outer:?}"
        );
    }
}

/// The slash menu and the pickers stay inside the Agents window docked at the right edge of a 1280 by 800 window
/// with the panel about 300 px wide: the slash menu is no wider than the prompt box, and each picker's list is no
/// wider than the panel and its right edge is not clipped.
#[gpui::test]
fn the_menus_stay_inside_a_narrow_agents_window(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.show_agents();
    let panel = w.bounds("agents-window");
    assert!(
        (250.0..=350.0).contains(&f32::from(panel.size.width)),
        "the Agents panel is about 300 px wide: {panel:?}"
    );
    assert!(
        panel.right() >= gpui::px(1280. - 24.),
        "docked at the right edge: {panel:?}"
    );

    // The slash menu.
    w.start_agent("Fake streamer");
    w.wait("the agent's commands", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            !s.agents().window.read(cx).transcript.commands.is_empty()
        })
    });
    w.click(super::window::PROMPT_BOX);
    w.type_keys("/");
    assert!(w.slash_menu().is_some());
    let panel = w.bounds("agents-window");
    let prompt = w.bounds(super::window::PROMPT_BOX);
    let menu = w.bounds(super::window::SLASH_MENU);
    Ws::assert_within("the slash menu", menu, panel);
    assert!(
        menu.size.width <= prompt.size.width + gpui::px(0.5),
        "no wider than the box: {menu:?} {prompt:?}"
    );
    assert!(
        menu.left() >= prompt.left() - gpui::px(0.5),
        "at the box's left edge: {menu:?} {prompt:?}"
    );
    assert!(
        menu.bottom() <= prompt.top() + gpui::px(1.),
        "above the box: {menu:?} {prompt:?}"
    );
    for row in [
        super::window::slash_item("compact"),
        super::window::slash_item("model"),
        super::window::SLASH_DETAIL.to_owned(),
    ] {
        let b = w.bounds(&row);
        Ws::assert_within(&row, b, menu);
    }
    w.vcx.simulate_keystrokes("escape");
    w.vcx.run_until_parked();
    assert_eq!(w.slash_menu(), None);

    // Each picker's list.
    w.start_agent("Fake options");
    w.wait_options();
    w.vcx.run_until_parked();
    for button in [
        super::window::option_picker("model"),
        super::window::option_picker("effort"),
        super::window::MODE_PICKER.to_owned(),
    ] {
        w.click(&button);
        assert!(w.open_picker().is_some(), "{button} opens");
        let panel = w.bounds("agents-window");
        let b = w.bounds(&button);
        let list = w.bounds(super::window::PICKER_MENU);
        Ws::assert_within(&format!("{button}'s list"), list, panel);
        assert!(
            list.size.width <= panel.size.width,
            "{button}'s list is no wider than the panel: {list:?} {panel:?}"
        );
        assert!(
            list.bottom() <= b.top() + gpui::px(0.5),
            "{button}'s list opens upward: {list:?} {b:?}"
        );
        // At the button's left edge, unless that would overflow the window; then its right edge is the footer's.
        let footer = w.bounds(super::window::FOOTER);
        let at_button = (list.left() - b.left()).abs() < gpui::px(1.);
        let at_right = (list.right() - footer.right()).abs() < gpui::px(1.);
        assert!(
            at_button || (at_right && b.left() + list.size.width > footer.right()),
            "{button}'s list is aligned: {list:?} {b:?} {footer:?}"
        );
        w.vcx.simulate_keystrokes("escape");
        w.vcx.run_until_parked();
        assert_eq!(w.open_picker(), None);
    }
}

impl Ws {
    /// Read the Agents window.
    fn agents_window<R>(&self, f: impl FnOnce(&super::window::AgentsWindow, &gpui::App) -> R) -> R {
        self.shell
            .read_with(&self.vcx, |s, cx| f(s.agents().window.read(cx), cx))
    }

    /// The transcript row of tool call `id`.
    fn tool_row(&self, id: &str) -> usize {
        self.agents_window(|w, _| {
            w.transcript
                .rows
                .iter()
                .position(
                    |r| matches!(r, super::transcript::Row::Tool(t) if t.call.tool_call_id == id),
                )
                .unwrap()
        })
    }

    /// The transcript's notices, in order.
    fn notices(&self) -> Vec<String> {
        self.transcript()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["notice"].as_str().map(str::to_owned))
            .collect()
    }

    /// The Output window's Agents source, all of it.
    fn agents_output(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            let pane = s
                .output
                .read(cx)
                .pane(eludite_commands::build::OutputSource::Agents);
            pane.tail(pane.len())
        })
    }
}

/// Brief 0059: the usage strip above the prompt box says `No usage yet` until the agent's first `usage_update`, then
/// the session's context and cost (`61k of 1M · $0.95` after the `stream` turn); the per-turn line stays in the
/// transcript in its short form; `end_turn` adds no notice; a restart clears the strip.
#[gpui::test]
fn the_usage_strip_shows_the_sessions_context_and_cost(cx: &mut TestAppContext) {
    use super::window::{STATUS_LINE, USAGE_STRIP};
    let mut w = setup(cx);
    w.show_agents();
    assert!(w.vcx.debug_bounds(USAGE_STRIP).is_some());
    assert_eq!(w.agents_window(|w, _| w.usage_text()), "No usage yet");
    w.start_agent("Fake streamer");
    assert_eq!(w.agents_window(|w, _| w.usage_text()), "No usage yet");
    w.type_prompt("stream");
    assert_eq!(w.wait_turn(), "end_turn");
    w.vcx.run_until_parked();
    assert_eq!(
        w.agents_window(|w, _| w.usage_text()),
        "61k of 1M \u{B7} $0.95"
    );
    assert_eq!(
        crate::bench::AGENT_STREAM_STRIP,
        "61k of 1M \u{B7} $0.95",
        "the stream bench checks the same text"
    );
    // The state output says the same (`agents-state.output.json`'s `usage`).
    let state = w.state_json();
    assert_eq!(
        state["usage"],
        json!({"used": 61_204, "size": 1_000_000, "cost": {"amount": 0.9512, "currency": "USD"}})
    );
    // The strip reads the session's last `usage_update` as the agent sent it.
    let usage = w
        .agents_window(|w, _| w.transcript.usage.clone())
        .expect("the session's usage");
    assert_eq!((usage.usage.used, usage.usage.size), (61_204, 1_000_000));
    assert_eq!(usage.usage.cost, Some((0.9512, "USD".to_owned())));
    // The turn's own line stays in the transcript, short, and in the record unchanged (brief 0034).
    let short = w.agents_window(|w, _| {
        w.transcript.rows.iter().find_map(|r| match r {
            super::transcript::Row::Usage(u) => Some(u.short()),
            _ => None,
        })
    });
    assert_eq!(
        short.as_deref(),
        Some("260k in \u{B7} 2.9k out \u{B7} $0.95")
    );
    // `end_turn` is how a turn normally ends: no notice says so.
    assert!(
        w.notices().iter().all(|n| !n.starts_with("Turn ended")),
        "{:?}",
        w.notices()
    );
    // The turn is over: no status line, and the window no longer redraws on its own.
    assert!(w.vcx.debug_bounds(STATUS_LINE).is_none());
    assert!(!w.agents_window(|w, _| w.ticking()));
    // A restart is a new session: no usage yet.
    w.start_agent("Fake streamer");
    w.vcx.run_until_parked();
    assert_eq!(w.agents_window(|w, _| w.usage_text()), "No usage yet");
    assert_eq!(w.state_json().get("usage"), None);
}

/// Brief 0059: while a turn runs the status line says the agent is working, with the elapsed time, and the window
/// redraws on a timer; a tool call is one line named by the adapter's title, collapsed until clicked (its record says
/// `expanded`); the permission prompt names the call's title; after the turn the status line and the timer are gone.
#[gpui::test]
fn a_running_turn_shows_its_status_and_tool_cards_fold(cx: &mut TestAppContext) {
    use super::window::{STATUS_LINE, permission_sentence, tool_card};
    let mut w = setup(cx);
    w.start_agent("Fake agent");
    w.type_prompt("List the errors");
    w.wait_for_prompt();
    // The status line, while the turn waits at the permission prompt.
    assert!(w.vcx.debug_bounds(STATUS_LINE).is_some());
    let status = w.agents_window(|w, _| w.status_text()).unwrap();
    assert!(
        status.starts_with("Fake agent is working\u{2026} 0:")
            && status.ends_with(" \u{B7} Esc to stop"),
        "{status}"
    );
    assert!(w.agents_window(|w, _| w.ticking()));
    // The prompt names what the call does, as the adapter titled it.
    let prompt = w.agents_window(|w, _| w.prompt.clone()).unwrap();
    assert_eq!(prompt.tool, "Bash");
    assert_eq!(prompt.title, "`rm -rf obj/`");
    assert!(w.vcx.debug_bounds("agents-permission-text").is_some());
    let (sentence, bold) = permission_sentence("Fake agent", &prompt.title, &prompt.class, None);
    assert_eq!(sentence, "Fake agent wants to run `rm -rf obj/` (execute).");
    assert_eq!(&sentence[bold], "`rm -rf obj/`");
    // Deny; the turn ends normally: no notice, no status line, no timer.
    w.click(&super::window::decision_button(
        super::window::Decision::Deny,
    ));
    assert_eq!(w.wait_turn(), "end_turn");
    w.vcx.run_until_parked();
    assert!(w.vcx.debug_bounds(STATUS_LINE).is_none());
    assert!(w.agents_window(|w, _| w.status_text()).is_none());
    assert!(!w.agents_window(|w, _| w.ticking()));
    // The fake agent's `--usage` ends the turn with its usage: the strip fills after a turn with tool calls.
    assert_eq!(
        w.agents_window(|w, _| w.usage_text()),
        "61k of 1M \u{B7} $0.95"
    );
    assert_eq!(w.state_json()["usage"]["used"], 61_204);
    // The diagnostics call is one collapsed line; a click on it shows its arguments and result, another folds them.
    let ix = w.tool_row("toolu_fake_diagnostics");
    let window = w.shell.read_with(&w.vcx, |s, _| s.agents().window.clone());
    window.update(&mut w.vcx, |w, cx| w.reveal(ix, cx));
    let card = tool_card(ix);
    let card: &'static str = Box::leak(card.into_boxed_str());
    w.wait("the card drawn", |w| w.vcx.debug_bounds(card).is_some());
    assert_eq!(w.tool("toolu_fake_diagnostics")["expanded"], false);
    assert_eq!(
        w.tool("toolu_fake_diagnostics")["title"],
        "mcp__eludite__diagnostics-list"
    );
    let collapsed = w.vcx.debug_bounds(card).unwrap();
    w.click(card);
    assert_eq!(w.tool("toolu_fake_diagnostics")["expanded"], true);
    window.update(&mut w.vcx, |w, cx| w.reveal(ix, cx));
    w.vcx.run_until_parked();
    let expanded = w.vcx.debug_bounds(card).unwrap();
    assert!(
        expanded.size.height > collapsed.size.height,
        "{collapsed:?} -> {expanded:?}"
    );
    w.click(card);
    assert_eq!(w.tool("toolu_fake_diagnostics")["expanded"], false);
    assert!(
        w.notices().iter().all(|n| !n.starts_with("Turn ended")),
        "{:?}",
        w.notices()
    );
}

/// Brief 0059: a cancelled turn ends with the notice "Stopped"; the Output window's Agents source holds the start,
/// the ready line (the agent's version and the MCP endpoint, which the header shows only as a tooltip), the prompt
/// and the turn's end, in order, and `eludite.output.show` reads it.
#[gpui::test]
fn a_cancel_says_stopped_and_the_output_window_logs_the_session(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.start_agent("Fake agent");
    w.type_prompt("List the errors");
    w.wait_for_prompt();
    w.click(super::window::PROMPT_BOX);
    w.vcx.simulate_keystrokes("escape");
    assert_eq!(w.wait_turn(), "cancelled");
    w.vcx.run_until_parked();
    assert_eq!(w.notices().last().map(String::as_str), Some("Stopped"));
    for (stop, words) in [
        ("end_turn", None),
        ("cancelled", Some("Stopped")),
        ("max_tokens", Some("The model reached its output limit")),
        (
            "max_turn_requests",
            Some("The agent reached its request limit"),
        ),
        ("refusal", Some("The model declined to continue")),
        ("something_new", Some("something_new")),
    ] {
        assert_eq!(super::window::stop_notice(stop).as_deref(), words, "{stop}");
    }
    let lines = w.agents_output();
    let at = |prefix: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starting {prefix:?} in {lines:#?}"))
    };
    let start = at("Starting Fake agent: eludite-fake-acp-agent --scenario diagnostics-then-shell");
    let ready = at("Fake agent is ready: eludite-fake-acp-agent ");
    let prompt = at("Prompt: List the errors");
    let ended = at("Turn ended: cancelled in ");
    assert!(
        start < ready && ready < prompt && prompt < ended,
        "{lines:#?}"
    );
    assert!(
        lines[ready].contains("(ACP v1). MCP: eludite via stdio relay to 127.0.0.1:"),
        "{}",
        lines[ready]
    );
    assert!(lines[ended].ends_with(" s"), "{}", lines[ended]);
    // The header's detail is that line's text; the window shows it as the state's tooltip, not under the header.
    w.show_agents();
    assert!(w.vcx.debug_bounds("agents-state").is_some());
    assert!(w.vcx.debug_bounds("agents-error").is_none());
    assert!(w.vcx.debug_bounds("agents-login").is_none());
    // Agents read it on the bus.
    let commands = w.commands.clone();
    let shown = w.agent(move || {
        commands
            .invoke(
                "eludite.output.show",
                json!({"source": "agents", "tail": 50}),
            )
            .unwrap()
    });
    assert_eq!(shown["source"], "agents");
    let tail: Vec<&str> = shown["tail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(tail.contains(&"Prompt: List the errors"), "{tail:?}");
    // Clear All on the Agents source empties it.
    let commands = w.commands.clone();
    let cleared = w.agent(move || {
        commands
            .invoke("eludite.output.clear", json!({"source": "agents"}))
            .unwrap()
    });
    assert_eq!(cleared["source"], "agents");
    assert_eq!(cleared["cleared"], lines.len());
    assert!(w.agents_output().is_empty());
}

// ---- Brief 0060: OpenAI-compatible servers through eludite-openai-acp ----

/// The adapter's loopback fake of an OpenAI-compatible server (hand-written HTTP/1.1, scripted per test).
#[path = "../../../../../agents/openai-acp/tests/fake_server.rs"]
mod fake_openai;

/// `eludite-openai-acp` and its test relay (`eludite-openai-fake-relay`, what `eludite --mcp-relay` does), built
/// once from `agents/openai-acp` (its own workspace: no `CARGO_BIN_EXE_` reaches it). `None`, with a message, when
/// cargo cannot build it here.
fn openai_adapter() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    static BUILT: std::sync::OnceLock<Option<(std::path::PathBuf, std::path::PathBuf)>> =
        std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let root =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agents/openai-acp");
            let target = root.join("target");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let built = std::process::Command::new(cargo)
                .args(["build", "--quiet", "--bins", "--manifest-path"])
                .arg(root.join("Cargo.toml"))
                .arg("--target-dir")
                .arg(&target)
                .status();
            let exe = |n: &str| {
                target
                    .join("debug")
                    .join(format!("{n}{}", std::env::consts::EXE_SUFFIX))
            };
            let (adapter, relay) = (exe("eludite-openai-acp"), exe("eludite-openai-fake-relay"));
            match built {
                Ok(s) if s.success() && adapter.is_file() && relay.is_file() => {
                    Some((adapter, relay))
                }
                other => {
                    eprintln!("skipped: could not build agents/openai-acp with cargo ({other:?})");
                    None
                }
            }
        })
        .clone()
}

/// The shell with no agents but the servers of a temporary `agents.json`, keys in memory, and (when given) the
/// built adapter launched for real, its MCP relay the test relay.
fn provider_setup(
    cx: &mut TestAppContext,
    adapter: Option<&(std::path::PathBuf, std::path::PathBuf)>,
) -> (
    Ws,
    tempfile::TempDir,
    Arc<eludite_forge::credentials::Credentials>,
) {
    let config = tempfile::tempdir().unwrap();
    let credentials = memory_credentials();
    let setup = AgentsSetup {
        registry: Some(Vec::new()),
        relay_exe: adapter.map_or_else(|| "eludite".into(), |a| a.1.clone()),
        connect: None,
        preferred: None,
        transcript_out: None,
        agents_file: Some(config.path().join("agents.json")),
        credentials: credentials.clone(),
        openai_adapter: adapter.map(|a| a.0.clone()),
        sessions_root: None,
    };
    let mut w = setup_full(cx, |_| {}, Some(setup));
    w.open_solution();
    (w, config, credentials)
}

impl Ws {
    fn registry_names(&self) -> Vec<(String, AgentSource)> {
        self.shell.read_with(&self.vcx, |s, _| {
            s.agents()
                .registry
                .iter()
                .map(|a| (a.name().to_owned(), a.source))
                .collect()
        })
    }

    fn wait_registry(&mut self, name: &str, present: bool) {
        let name = name.to_owned();
        self.wait("the registry", |w| {
            w.registry_names().iter().any(|(n, _)| *n == name) == present
        });
    }
}

#[test]
fn providers_are_kept_when_agents_custom_is_set() {
    // Phase one's finding: with `agents.custom` set, `agents.json`'s servers were dropped.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("agents.json");
    std::fs::write(
        &file,
        r#"{"agents": [{"name": "From the file", "command": "x"}],
            "providers": [{"name": "Local llama", "baseUrl": "http://localhost:8080/v1"}]}"#,
    )
    .unwrap();
    let custom = super::RegistryConfig {
        custom: Some(vec![eludite_acp::settings::ConfiguredAgent {
            name: "From the settings".into(),
            command: "y".into(),
            args: Vec::new(),
            env: Default::default(),
        }]),
        ..Default::default()
    };
    let none = eludite_acp::AdapterSearch::default();
    let (registry, error, settings) = super::search_registry(&custom, Some(&file), &none);
    assert!(error.is_none());
    let names: Vec<_> = registry.iter().map(|a| (a.name(), a.source)).collect();
    assert!(
        names.contains(&("Local llama", AgentSource::Provider)),
        "{names:?}"
    );
    assert!(
        names.contains(&("From the settings", AgentSource::Settings)),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|(n, _)| *n == "From the file"),
        "{names:?}"
    );
    assert_eq!(settings.providers.len(), 1);
    // Without the setting, the file's agents come too; a malformed file is reported and adds no server.
    let (registry, _, _) = super::search_registry(&Default::default(), Some(&file), &none);
    assert!(registry.iter().any(|a| a.name() == "From the file"));
    std::fs::write(&file, "{").unwrap();
    let (registry, error, _) = super::search_registry(&custom, Some(&file), &none);
    assert!(error.unwrap().contains("agents.json"));
    assert!(!registry.iter().any(|a| a.source == AgentSource::Provider));
}

#[gpui::test]
fn a_server_without_the_adapter_is_in_the_error_state(cx: &mut TestAppContext) {
    use eludite_commands::agents::{PROVIDER_REMOVE, PROVIDER_SET};
    let (mut w, config, credentials) = provider_setup(cx, None);
    let out = w
        .commands
        .invoke(
            PROVIDER_SET,
            json!({"name": "Ollama", "baseUrl": "http://localhost:11434/v1"}),
        )
        .unwrap();
    assert_eq!(out["action"], "saved");
    assert_eq!(out["hasKey"], false);
    w.wait_registry("Ollama", true);
    assert_eq!(
        w.registry_names(),
        [("Ollama".to_owned(), AgentSource::Provider)]
    );
    // Starting it says why it cannot run; nothing is launched.
    let e = w
        .shell
        .update(&mut w.vcx, |s, cx| s.agents_start(Some("Ollama"), true, cx))
        .unwrap_err();
    assert_eq!(e, "eludite-openai-acp was not found");
    let state = w.shell.read_with(&w.vcx, |s, cx| s.agents_state(cx));
    assert_eq!(state.state, "error");
    assert_eq!(
        state.message.as_deref(),
        Some("eludite-openai-acp was not found")
    );
    assert_eq!(state.agents[0].source, "provider");
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.agents().session().is_none())
    );
    // A key given, then deleted with `""`; Remove takes the entry and the key.
    w.commands
        .invoke(
            PROVIDER_SET,
            json!({"name": "Ollama", "baseUrl": "http://localhost:11434/v1", "apiKey": "k-1"}),
        )
        .unwrap();
    assert!(credentials.get("provider:Ollama").is_some());
    let out = w
        .commands
        .invoke(
            PROVIDER_SET,
            json!({"name": "Ollama", "baseUrl": "http://localhost:11434/v1", "apiKey": ""}),
        )
        .unwrap();
    assert_eq!(out["hasKey"], false);
    assert!(credentials.get("provider:Ollama").is_none());
    w.commands
        .invoke(PROVIDER_REMOVE, json!({"name": "Ollama"}))
        .unwrap();
    w.wait_registry("Ollama", false);
    let file = std::fs::read_to_string(config.path().join("agents.json")).unwrap();
    assert!(!file.contains("Ollama"), "{file}");
    assert!(
        w.commands
            .invoke(PROVIDER_REMOVE, json!({"name": "Ollama"}))
            .unwrap_err()
            .to_string()
            .contains("no server named")
    );
}

#[gpui::test]
fn file_read_serves_an_open_documents_unsaved_text_off_the_ui_thread(cx: &mut TestAppContext) {
    use eludite_commands::files::FILE_READ;
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(0);
            e.insert("// unsaved\n");
        })
    });
    w.vcx.run_until_parked();
    let commands = w.commands.clone();
    let out = w.agent(move || {
        commands
            .invoke(
                FILE_READ,
                json!({"path": "src/App/Program.cs", "endLine": 2}),
            )
            .unwrap()
    });
    assert_eq!(out["source"], "buffer");
    assert_eq!(out["text"], "1\t// unsaved\n2\tclass Program");
    assert_eq!(
        (out["endLine"].as_u64(), out["totalLines"].as_u64()),
        (Some(2), Some(5))
    );
    assert_eq!(
        std::path::PathBuf::from(out["path"].as_str().unwrap()),
        super::super::documents::normalize_path(&program)
    );
    // A closed file is read from disk; the UI thread is refused (it would wait for itself).
    let commands = w.commands.clone();
    let out = w.agent(move || {
        commands
            .invoke(FILE_READ, json!({"path": "src/App/Models/Order.cs"}))
            .unwrap()
    });
    assert_eq!(
        (out["source"].as_str(), out["text"].as_str()),
        (Some("disk"), Some("1\tclass Order { }"))
    );
    let e = w
        .commands
        .invoke(FILE_READ, json!({"path": "src/App/Program.cs"}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("runs off the UI thread"), "{e}");
    // Both are agent-visible tools; file.edit is reviewed like workspace.apply_edit.
    for id in eludite_commands::files::ALL {
        assert!(w.commands.lookup(id).unwrap().agent_visible, "{id}");
    }
    assert!(super::REVIEWED_COMMANDS.contains(&eludite_commands::files::FILE_EDIT));
}

#[gpui::test]
fn a_server_added_through_provider_set_runs_a_turn_with_eludites_tools(cx: &mut TestAppContext) {
    use eludite_commands::agents::{PROVIDER_MODELS, PROVIDER_REMOVE, PROVIDER_SET};
    use fake_openai::{FakeServer, Reply, finish, text_reply, tool_frag, tool_reply, usage};
    let Some(adapter) = openai_adapter() else {
        return;
    };
    let fake = FakeServer::start();
    fake.llama_models(&["qwen3-8b", "llama-3.1-8b"], 16_384);
    let (mut w, config, credentials) = provider_setup(cx, Some(&adapter));
    // Added as an agent would add it: the audit keeps its arguments with the key redacted.
    let commands = w.commands.clone();
    let url = fake.url.clone();
    let out = w.agent(move || {
        eludite_commands::with_caller(
            eludite_commands::Caller::Agent {
                agent: "Outer".into(),
                call: eludite_commands::next_call_id(),
                tool_call: None,
            },
            || {
                commands.invoke(
                    PROVIDER_SET,
                    json!({"name": "Local llama", "baseUrl": url, "apiKey": "sk-test-secret"}),
                )
            },
        )
        .unwrap()
    });
    assert_eq!(
        (out["action"].as_str(), out["hasKey"].as_bool()),
        (Some("saved"), Some(true))
    );
    let audit = w.commands.audit_log().entries();
    let set = audit
        .iter()
        .find(|e| e.command == PROVIDER_SET)
        .expect("audited");
    assert_eq!(set.arguments.as_ref().unwrap()["apiKey"], "<redacted>");
    assert!(!format!("{audit:?}").contains("sk-test-secret"));
    // The key is in the store, not in agents.json; the server is in the registry with source `provider`.
    let file = std::fs::read_to_string(config.path().join("agents.json")).unwrap();
    assert!(
        !file.contains("sk-test-secret") && file.contains("Local llama"),
        "{file}"
    );
    assert_eq!(
        credentials
            .get("provider:Local llama")
            .unwrap()
            .0
            .token
            .expose(),
        "sk-test-secret"
    );
    w.wait_registry("Local llama", true);
    let state = w.shell.read_with(&w.vcx, |s, cx| s.agents_state(cx));
    assert_eq!(state.agents[0].source, "provider");
    assert!(state.agents[0].command.contains("--base-url"));
    assert!(!state.agents[0].command.contains("sk-test-secret"));

    // Started for real: the model picker lists the server's two models.
    w.start_agent("Local llama");
    assert_eq!(w.agents_state(), StateKind::Ready);
    let models = w.shell.read_with(&w.vcx, |s, _| {
        s.agents()
            .pickers()
            .into_iter()
            .find(|p| p.category.as_deref() == Some("model"))
            .map(|p| {
                p.choices
                    .iter()
                    .map(|c| c.value.clone())
                    .collect::<Vec<_>>()
            })
    });
    assert_eq!(
        models,
        Some(vec!["llama-3.1-8b".to_owned(), "qwen3-8b".to_owned()])
    );
    let models_request = fake
        .requests()
        .into_iter()
        .find(|r| r.path.ends_with("/models"))
        .expect("the adapter listed the models");
    assert_eq!(
        models_request.header("authorization"),
        Some("Bearer sk-test-secret")
    );

    // A pick in the window's model picker changes the model for the next request, and is not remembered in
    // `agents.model` (that setting is Claude Code's next start).
    w.shell.update(&mut w.vcx, |s, cx| {
        s.agents().window.update(cx, |_, cx| {
            cx.emit(super::window::AgentsWindowEvent::Configure {
                option: "model".into(),
                value: "qwen3-8b".into(),
            })
        })
    });
    w.wait("the agent took the model", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.agents()
                .pickers()
                .iter()
                .any(|p| p.key == "model" && p.current == "qwen3-8b")
        })
    });
    let remembered = w
        .shell
        .read_with(&w.vcx, |s, _| s.settings.lock().string("agents.model"));
    assert_eq!(remembered, "");

    // The turn: diagnostics.list runs without a prompt, terminal.send prompts and is denied, file.edit is held as a
    // pending change and accepted, then the answer.
    fake.push(tool_reply(
        "call_diag",
        "eludite-diagnostics-list",
        "{}",
        900,
    ));
    fake.push(tool_reply(
        "call_term",
        "eludite-terminal-send",
        r#"{"text": "ls\n"}"#,
        950,
    ));
    // Arguments split mid-token across fragments.
    fake.push(Reply::sse(vec![
        tool_frag(0, Some("call_edit"), Some("eludite-file-edit"), ""),
        tool_frag(0, None, None, r#"{"path": "src/App/Program.cs", "oldT"#),
        tool_frag(
            0,
            None,
            None,
            r#"ext": "static void Main() { }", "newText": "static void Main() { Run(); }"}"#,
        ),
        finish("tool_calls"),
        usage(1_000, 10),
    ]));
    fake.push(text_reply("Fixed Program.cs.", 1_200, 30));
    w.shell
        .update(&mut w.vcx, |s, cx| s.agents_prompt("Fix the program", cx))
        .unwrap();
    let request = w.wait_for_prompt();
    let prompt = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents().window.read(cx).prompt.clone().unwrap()
    });
    // The adapter titles a call with its tool's name and first argument.
    assert!(
        prompt.tool.starts_with("eludite-terminal-send"),
        "{}",
        prompt.tool
    );
    assert!(
        ["execute", "dangerous"].contains(&prompt.class.as_str()),
        "{}",
        prompt.class
    );
    w.shell
        .update(&mut w.vcx, |s, cx| {
            s.agents_answer(request, super::window::Decision::Deny, cx)
        })
        .unwrap();
    w.wait("the file edit's pending change", |w| {
        change_ids(w)
            .iter()
            .any(|c| c.1 == "Program.cs" && c.2 == "pending")
    });
    let program = w.path("src/App/Program.cs");
    assert_eq!(
        std::fs::read_to_string(&program).unwrap(),
        super::super::tests::PROGRAM,
        "nothing applied before the review"
    );
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.invoke(
            eludite_commands::agents::REVIEW,
            json!({"decision": "accept", "all": true}),
            window,
            cx,
        )
        .unwrap()
    });
    assert_eq!(w.wait_turn(), "end_turn");
    assert!(w.agent_text().contains("Fixed Program.cs."));
    // What the model was sent back: the diagnostics, the refusal, the applied edit, in order.
    let chats = fake.chats();
    assert_eq!(chats.len(), 4, "{chats:?}");
    assert!(
        chats.iter().all(|c| c["model"] == "qwen3-8b"),
        "the picked model"
    );
    let tool_result = |chat: &Value, id: &str| {
        chat["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
            .unwrap_or_default()
    };
    assert!(!tool_result(&chats[1], "call_diag").is_empty());
    let refusal = tool_result(&chats[2], "call_term");
    assert!(refusal.contains("denied"), "{refusal}");
    let applied = tool_result(&chats[3], "call_edit");
    assert!(applied.contains("applied"), "{applied}");
    let audit = w.commands.audit_log().entries();
    let calls: Vec<_> = audit
        .iter()
        .filter(|e| e.caller.is_agent())
        .map(|e| {
            (
                e.command.as_str(),
                e.edits.iter().map(|x| x.state).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert!(
        calls
            .iter()
            .any(|(c, _)| *c == "eludite.workspace.apply_edit"),
        "file.edit ran one workspace.apply_edit as the agent: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|(_, e)| e.as_slice() == [eludite_commands::EditState::Accepted]),
        "{calls:?}"
    );
    let after = std::fs::read_to_string(&program).unwrap();
    assert!(after.contains("static void Main() { Run(); }"), "{after}");
    // The usage strip reads the last response's counts against the model's window.
    let usage = w
        .shell
        .read_with(&w.vcx, |s, cx| s.agents_state(cx))
        .usage
        .expect("usage");
    assert_eq!((usage.used, usage.size), (1_230, 16_384));

    // provider_models on a server that does not list models, with a catalog: the catalog.
    let unlisted = FakeServer::start();
    unlisted.set_models(Reply::json(404, json!({"error": {"message": "not found"}})));
    w.commands
        .invoke(
            PROVIDER_SET,
            json!({"name": "Catalogued", "baseUrl": unlisted.url, "models": [{"id": "m-1", "contextWindow": 8192}]}),
        )
        .unwrap();
    let commands = w.commands.clone();
    let out = w.agent(move || {
        commands
            .invoke(PROVIDER_MODELS, json!({"name": "Catalogued"}))
            .unwrap()
    });
    assert_eq!(out["listing"], "catalog");
    assert_eq!(out["models"][0]["id"], "m-1");
    // And a saved server listed with its stored key; a url being added, with none.
    let commands = w.commands.clone();
    let out = w.agent(move || {
        commands
            .invoke(PROVIDER_MODELS, json!({"name": "Local llama"}))
            .unwrap()
    });
    assert_eq!(out["listing"], "server");
    assert_eq!(out["models"].as_array().unwrap().len(), 2);
    let commands = w.commands.clone();
    let url = unlisted.url.clone();
    let out = w.agent(move || {
        commands
            .invoke(PROVIDER_MODELS, json!({"baseUrl": url}))
            .unwrap()
    });
    assert_eq!(out["listing"], "none");
    // Remove deletes the credential too.
    w.agents_stop_now();
    w.commands
        .invoke(PROVIDER_REMOVE, json!({"name": "Local llama"}))
        .unwrap();
    assert!(credentials.get("provider:Local llama").is_none());
    w.wait_registry("Local llama", false);
}

impl Ws {
    fn agents_stop_now(&mut self) {
        self.shell.update(&mut self.vcx, |s, cx| s.agents_stop(cx));
        self.vcx.run_until_parked();
    }
}

#[gpui::test]
fn a_server_is_added_from_the_picker_tested_and_listed_in_options(cx: &mut TestAppContext) {
    use super::providers::{
        KEY_BOX, MESSAGE, NAME_BOX, PRESETS, SAVE, TEST, URL_BOX, preset_selector, remove_selector,
    };
    use super::window::{ADD_SERVER_ITEM, AGENT_PICKER};
    let Some(adapter) = openai_adapter() else {
        return;
    };
    let fake = FakeServerHandle::start();
    let (mut w, config, credentials) = provider_setup(cx, Some(&adapter));
    w.show_agents();
    // The picker's list ends with "Add server…", which opens the dialog on the llama.cpp preset.
    w.click(AGENT_PICKER);
    assert!(w.shell.read_with(&w.vcx, |s, cx| {
        s.agents().window.read(cx).picker_list_open()
    }));
    w.click(ADD_SERVER_ITEM);
    let dialog = w
        .shell
        .read_with(&w.vcx, |s, cx| s.provider_dialog(cx))
        .expect("the dialog opened");
    for sel in [NAME_BOX, URL_BOX, KEY_BOX, TEST, SAVE] {
        assert!(w.vcx.debug_bounds(sel).is_some(), "{sel}");
    }
    w.bounds(&preset_selector(0));
    let fields = |w: &mut Ws| {
        dialog.read_with(&w.vcx, |d, cx| {
            (d.name.read(cx).text(), d.url.read(cx).text(), d.preset)
        })
    };
    assert_eq!(
        fields(&mut w),
        (
            "llama.cpp".to_owned(),
            "http://localhost:8080/v1".to_owned(),
            0
        )
    );
    // Another preset fills its url and name; Custom keeps them.
    w.click(&preset_selector(5));
    assert_eq!(
        fields(&mut w),
        (
            "OpenRouter".to_owned(),
            "https://openrouter.ai/api/v1".to_owned(),
            5
        )
    );
    w.click(&preset_selector(0));
    assert_eq!(PRESETS[0].name, "llama.cpp");
    // The fake's url typed over the preset's; Test lists its models through the bus, off the UI thread.
    dialog.update(&mut w.vcx, |d, cx| {
        d.url.update(cx, |i, cx| i.set_text(&fake.0.url, cx))
    });
    w.click(KEY_BOX);
    w.vcx.simulate_keystrokes("s k x 9");
    dialog.read_with(&w.vcx, |d, cx| {
        assert!(d.test_args(cx).unwrap()["apiKey"] == "skx9")
    });
    w.click(TEST);
    w.wait("the test's answer", |w| {
        dialog.read_with(&w.vcx, |d, _| {
            d.message.as_ref().is_some_and(|m| m.0 == "2 models")
        })
    });
    assert!(w.vcx.debug_bounds(MESSAGE).is_some());
    // Save adds it, selected, and closes the dialog; the key went to the store only.
    w.click(SAVE);
    w.wait("the dialog to close", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.provider_dialog(cx).is_none())
    });
    w.wait_registry("llama.cpp", true);
    w.wait("the server selected", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.agents().selected_agent().map(|a| a.name().to_owned()) == Some("llama.cpp".into())
        })
    });
    assert_eq!(
        credentials
            .get("provider:llama.cpp")
            .unwrap()
            .0
            .token
            .expose(),
        "skx9"
    );
    let file = std::fs::read_to_string(config.path().join("agents.json")).unwrap();
    assert!(!file.contains("skx9"), "{file}");
    // Tools > Options > Agents lists it with Edit and Remove; Remove takes it and its key.
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.run(
            eludite_commands::settings::OPTIONS,
            json!({"section": "Agents"}),
            window,
            cx,
        )
    });
    w.vcx.run_until_parked();
    let rows = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents()
            .providers_page
            .as_ref()
            .map(|p| p.read(cx).rows.clone())
    });
    let rows = rows.expect("the page is on the Agents page");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].name.as_str(), rows[0].has_key),
        ("llama.cpp", true)
    );
    assert!(w.vcx.debug_bounds("options-providers").is_some());
    // The page sits under the Agents settings, scrolled out of the dialog's view: its buttons are pressed by their
    // events.
    w.bounds(&super::providers::edit_selector(0));
    w.bounds(&remove_selector(0));
    let page = |w: &Ws| {
        w.shell
            .read_with(&w.vcx, |s, _| s.agents().providers_page.clone())
            .unwrap()
    };
    page(&w).update(&mut w.vcx, |_, cx| {
        cx.emit(super::providers::ProvidersPageEvent::Edit(Some(
            "llama.cpp".into(),
        )))
    });
    w.vcx.run_until_parked();
    let editing = w
        .shell
        .read_with(&w.vcx, |s, cx| s.provider_dialog(cx))
        .expect("Edit opens the dialog");
    editing.read_with(&w.vcx, |d, cx| {
        assert_eq!(d.editing.as_deref(), Some("llama.cpp"));
        assert!(d.has_key);
        // An empty key box keeps the stored key.
        assert!(d.save_args(cx).unwrap().get("apiKey").is_none());
    });
    w.vcx.simulate_keystrokes("escape");
    w.vcx.run_until_parked();
    assert!(
        w.shell
            .read_with(&w.vcx, |s, cx| s.provider_dialog(cx).is_none())
    );
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.run(
            eludite_commands::settings::OPTIONS,
            json!({"section": "Agents"}),
            window,
            cx,
        )
    });
    w.vcx.run_until_parked();
    page(&w).update(&mut w.vcx, |_, cx| {
        cx.emit(super::providers::ProvidersPageEvent::Remove(
            "llama.cpp".into(),
        ))
    });
    w.wait_registry("llama.cpp", false);
    assert!(credentials.get("provider:llama.cpp").is_none());
    w.wait("the page's rows", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.agents()
                .providers_page
                .as_ref()
                .is_some_and(|p| p.read(cx).rows.is_empty())
        })
    });
}

/// The fake server with two llama.cpp-shaped models.
struct FakeServerHandle(fake_openai::FakeServer);

impl FakeServerHandle {
    fn start() -> Self {
        let f = fake_openai::FakeServer::start();
        f.llama_models(&["qwen3-8b", "llama-3.1-8b"], 16_384);
        Self(f)
    }
}

// Brief 0060: sessions.

/// The fake agents of the session tests, keeping their sessions under `root` (the per-workspace state folders): a
/// streamer slow enough to switch away mid-stream (300 chunks at 100 a second), the shell asker, the editor, a quick
/// streamer that resumes sessions (`--load`) and one that cannot.
fn session_agents(root: &std::path::Path) -> AgentsSetup {
    let args = |s: &str| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
    let mut setup = fake_agents(vec![
        (
            "Fake streamer".into(),
            args("--scenario stream --chunks 300 --rate 100"),
        ),
        (
            "Fake agent".into(),
            args("--scenario diagnostics-then-shell"),
        ),
        ("Fake editor".into(), args("--scenario edit")),
        (
            "Fake loader".into(),
            args("--scenario stream --chunks 20 --rate 2000 --load"),
        ),
        (
            "Fake quick".into(),
            args("--scenario stream --chunks 20 --rate 2000"),
        ),
    ]);
    setup.sessions_root = Some(root.to_path_buf());
    setup
}

impl Ws {
    /// The session shown.
    fn shown(&self) -> Option<String> {
        self.shell
            .read_with(&self.vcx, |s, _| s.agents().shown.clone())
    }

    /// Session `id`'s transcript rows, shown or not.
    fn rows_of(&self, id: &str) -> usize {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().session_rows(id, cx).unwrap_or(0)
        })
    }

    /// How much agent text session `id` has, shown or not.
    fn text_of(&self, id: &str) -> usize {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().session_text(id, cx).map_or(0, |t| t.len())
        })
    }

    /// Session `id`'s state and its last turn's end.
    fn session_state(&self, id: &str) -> (StateKind, Option<String>) {
        self.shell.read_with(&self.vcx, |s, _| {
            s.agents()
                .session_slot(id)
                .map(|x| (x.state, x.last_stop.clone()))
                .unwrap_or_default()
        })
    }

    fn agents_bus(&mut self, command: &'static str, args: Value) -> Result<Value, String> {
        let commands = self.commands.clone();
        self.agent(move || commands.invoke(command, args).map_err(|e| e.to_string()))
    }

    /// `eludite.agents.sessions`.
    fn sessions(&mut self) -> Value {
        self.agents_bus(eludite_commands::agents::SESSIONS, json!({}))
            .unwrap()
    }

    fn session_row(&mut self, id: &str) -> Value {
        let all = self.sessions();
        all["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn store_dir(&self) -> std::path::PathBuf {
        self.shell.read_with(&self.vcx, |s, _| {
            s.agents()
                .store_dir()
                .expect("a store folder")
                .to_path_buf()
        })
    }

    fn history_rows(&self) -> Vec<super::window::HistoryRow> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.agents().window.read(cx).history_rows.clone()
        })
    }

    /// Wait until session `id`'s turn has ended (shown or not).
    fn wait_session_turn(&mut self, id: &str) {
        let id = id.to_owned();
        self.wait("the session's turn to end", |w| {
            let (state, stop) = w.session_state(&id);
            stop.is_some() && state != StateKind::Running
        });
    }

    /// The record of session `id` once its file says the turn ended.
    fn record(&mut self, id: &str) -> super::sessions::Record {
        let dir = self.store_dir();
        let id = id.to_owned();
        self.wait("the session's record", |_| {
            super::sessions::load(&dir, &id).is_some_and(|r| r.ended || r.meta.prompted())
        });
        super::sessions::load(&dir, &id).unwrap()
    }
}

/// Brief 0060's proof, live sessions: a streaming session keeps streaming off screen while another is shown; that
/// other session's permission request waits off screen with `?` in the history list and `waiting` in
/// `eludite.agents.sessions`, and is answered after switching to it (with the keyboard in the history list); both are
/// listed with their titles and flags, and each has its record (mode 0600) once its turn ended.
#[gpui::test]
fn two_live_sessions_stream_and_wait_off_screen_and_are_kept(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let mut w = setup_full(cx, |_| {}, Some(session_agents(state.path())));
    w.open_solution();
    w.show_agents();
    // The window opens on no session.
    assert_eq!(w.shown(), None);
    assert_eq!(w.state_json().get("session"), None);
    assert_eq!(w.sessions(), json!({"current": null, "sessions": []}));

    w.start_agent("Fake streamer");
    let a = w.shown().expect("a session");
    w.type_prompt("Stream the numbers");
    w.wait("the stream to start", |w| {
        w.agent_text().contains("chunk 00010")
    });

    // A second session, the first keeps running.
    let state_b = w
        .agents_bus(
            eludite_commands::agents::NEW_SESSION,
            json!({"agent": "Fake agent"}),
        )
        .unwrap();
    let b = w.shown().unwrap();
    assert_ne!(a, b);
    assert_eq!(state_b["session"]["id"], b.as_str());
    assert_eq!(state_b["session"]["title"], "New session");
    w.wait("the second agent", |w| w.agents_state() == StateKind::Ready);
    let off_screen = w.text_of(&a);
    w.wait("the first session's text to grow off screen", |w| {
        w.text_of(&a) > off_screen + 200
    });
    assert_eq!(w.session_state(&a).0, StateKind::Running);
    w.type_prompt("List the errors");
    let request = w.wait_for_prompt();

    // Back to the stream: the request waits off screen.
    w.click(super::window::HISTORY_BUTTON);
    assert!(w.vcx.debug_bounds(super::window::HISTORY_MENU).is_some());
    w.click(&super::window::session_item(&a));
    assert_eq!(w.shown().as_deref(), Some(a.as_str()));
    assert!(w.agent_text().contains("chunk 0"), "the stream shows");
    let prompt = w
        .shell
        .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).prompt.clone());
    assert!(
        prompt.is_none(),
        "the other session's request is not shown here"
    );
    let shown_text = w.text_of(&a);
    w.wait("the shown stream to grow", |w| w.text_of(&a) > shown_text);
    let row_b = w.session_row(&b);
    assert_eq!(row_b["waiting"], true, "{row_b}");
    assert_eq!(row_b["live"], true);
    assert_eq!(row_b["title"], "List the errors");
    assert_eq!(row_b["agent"], "Fake agent");
    let row_a = w.session_row(&a);
    assert_eq!(row_a["running"], true, "{row_a}");
    assert_eq!(row_a["title"], "Stream the numbers");
    assert_eq!(w.sessions()["current"], a.as_str());
    // The history list: b waits (`?`), a runs and is checked.
    w.click(super::window::HISTORY_BUTTON);
    let rows = w.history_rows();
    let hb = rows.iter().find(|r| r.id == b).unwrap();
    assert!(hb.waiting && !hb.current);
    let ha = rows.iter().find(|r| r.id == a).unwrap();
    assert!(ha.running && ha.current);
    assert_eq!(ha.when, "just now");
    // Down to b's row (the list is newest first and opens on the shown one), Enter.
    let open_at = w
        .shell
        .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).history_open());
    let from = open_at.unwrap();
    let to = rows.iter().position(|r| r.id == b).unwrap();
    let key = if to > from { "down" } else { "up" };
    for _ in 0..from.abs_diff(to) {
        w.vcx.simulate_keystrokes(key);
    }
    w.vcx.simulate_keystrokes("enter");
    w.vcx.run_until_parked();
    assert_eq!(w.shown().as_deref(), Some(b.as_str()));
    assert_eq!(w.wait_for_prompt(), request);
    w.click(&super::window::decision_button(
        super::window::Decision::Deny,
    ));
    w.wait_session_turn(&b);
    w.wait_session_turn(&a);
    assert_eq!(w.session_row(&b)["waiting"], false);

    // One record per session with a prompt, private.
    let ra = w.record(&a);
    let rb = w.record(&b);
    assert_eq!(ra.meta.title, "Stream the numbers");
    assert_eq!(rb.meta.title, "List the errors");
    assert_eq!(rb.meta.agent, "Fake agent");
    assert!(rb.meta.acp_session_id.is_some());
    assert!(
        ra.transcript
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["user"] == "Stream the numbers")
    );
    let dir = w.store_dir();
    assert!(dir.starts_with(state.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(format!("{a}.json"))), 0o600);
    }
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(files.len(), 2);
    // The status bar names the session.
    let status = w.shell.read_with(&w.vcx, |s, _| {
        s.status().get(super::AGENTS_SLOT).map(str::to_owned)
    });
    assert_eq!(
        status.as_deref(),
        Some("Fake agent: ready \u{b7} List the errors")
    );
}

/// Brief 0060's proof, stored sessions: the edit scenario's transcript round-trips through its record; a new shell on
/// the same state folder lists the stored sessions; switching to the `--load` agent's session shows Eludite's record
/// and resumes it with `session/load` of the stored id (the agent's replay counted and discarded), and the next prompt
/// continues it; the session of an agent without `loadSession` shows the notice and a disabled prompt box, and
/// Restart begins a new session with that agent.
#[gpui::test]
fn stored_sessions_are_listed_rebuilt_and_resumed(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let mut w = setup_full(cx, |_| {}, Some(session_agents(state.path())));
    w.open_solution();
    w.show_agents();
    w.start_agent("Fake editor");
    let edited = w.shown().unwrap();
    w.type_prompt("Add a header comment");
    w.wait("the pending changes", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.agents().changes.len() == 2)
    });
    w.agents_bus(
        "eludite.agents.review",
        json!({"all": true, "decision": "accept"}),
    )
    .unwrap();
    w.wait_session_turn(&edited);
    // The round trip: the record rebuilds into rows whose record is the same.
    let record = w.transcript();
    let rebuilt = super::transcript::Transcript::from_json(&record);
    assert_eq!(rebuilt.to_json(), record);
    let row = rebuilt
        .tools()
        .find(|t| !t.changes.is_empty())
        .expect("a call with changes");
    assert_eq!(row.status(), eludite_ui::transcript::ToolStatus::Completed);
    assert!(
        row.changes.iter().all(|(_, _, s)| s == "accepted"),
        "{:?}",
        row.changes
    );
    assert!(
        rebuilt.unaudited().is_empty(),
        "a rebuilt row is never audited again"
    );
    let shown_rows = w.shell.read_with(&w.vcx, |s, cx| {
        s.agents().window.read(cx).transcript.rows.len()
    });
    assert_eq!(rebuilt.rows.len(), shown_rows);

    w.agents_bus(
        eludite_commands::agents::NEW_SESSION,
        json!({"agent": "Fake loader"}),
    )
    .unwrap();
    let loader = w.shown().unwrap();
    w.wait("the loader", |w| w.agents_state() == StateKind::Ready);
    w.type_prompt("Hello loader");
    w.wait_session_turn(&loader);
    let acp_id = w
        .record(&loader)
        .meta
        .acp_session_id
        .expect("the agent's id");
    w.agents_bus(
        eludite_commands::agents::NEW_SESSION,
        json!({"agent": "Fake quick"}),
    )
    .unwrap();
    let quick = w.shown().unwrap();
    w.wait("the quick agent", |w| w.agents_state() == StateKind::Ready);
    w.type_prompt("Hello quick");
    w.wait_session_turn(&quick);
    for id in [&edited, &loader, &quick] {
        let r = w.record(id);
        assert!(r.meta.prompted());
    }
    let loader_rows = w.rows_of(&loader);

    // A new shell on the same state folder and the same workspace.
    let sln = w.path("App.slnx");
    let mut w2 = setup_full(cx, |_| {}, Some(session_agents(state.path())));
    w2.commands
        .invoke(
            eludite_commands::workspace::SOLUTION_OPEN,
            json!({"path": sln.to_string_lossy()}),
        )
        .unwrap();
    w2.show_agents();
    w2.wait("the stored sessions", |w| {
        w.sessions()["sessions"].as_array().unwrap().len() == 3
    });
    let listed = w2.sessions();
    assert_eq!(listed["current"], Value::Null);
    let titles: Vec<&str> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Hello quick", "Hello loader", "Add a header comment"]
    );
    assert!(
        listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["live"] == false)
    );

    // The loader's session: the record shows, the agent resumes it, its replay is discarded.
    let state_json = w2
        .agents_bus(eludite_commands::agents::SWITCH, json!({"session": loader}))
        .unwrap();
    assert_eq!(state_json["session"]["id"], loader.as_str());
    w2.wait("the resumed session", |w| {
        w.agents_state() == StateKind::Ready
    });
    let (replayed, from_record) = w2.shell.read_with(&w2.vcx, |s, _| {
        let slot = s.agents().session_slot(&loader).unwrap();
        (slot.replayed, slot.from_record)
    });
    assert_eq!(replayed, fake_agent::LOAD_REPLAY.len());
    assert!(from_record);
    assert_eq!(w2.state_json()["session_id"], acp_id.as_str());
    assert!(w2.user_rows().contains(&"Hello loader".to_owned()));
    let rows = w2.transcript();
    assert!(
        !rows.to_string().contains("An earlier answer"),
        "the replay is discarded"
    );
    // The rebuilt rows, the resume notice, nothing of the replay.
    assert_eq!(w2.rows_of(&loader), loader_rows + 1);
    assert!(
        w2.agents_output()
            .iter()
            .any(|l| l.contains("replayed 2 updates (discarded"))
    );
    w2.type_prompt("Continue");
    w2.wait_session_turn(&loader);
    assert_eq!(w2.user_rows(), ["Hello loader", "Continue"]);
    assert!(w2.agent_text().contains("chunk 00019"));
    let continued = w2.record(&loader);
    assert_eq!(
        continued.meta.acp_session_id.as_deref(),
        Some(acp_id.as_str())
    );

    // The quick agent cannot resume: the notice, a disabled box, no prompt.
    w2.click(super::window::HISTORY_BUTTON);
    w2.click(&super::window::session_item(&quick));
    assert_eq!(w2.shown().as_deref(), Some(quick.as_str()));
    w2.wait("the notice", |w| {
        w.notices().iter().any(|n| n == super::CANNOT_RESUME)
    });
    let disabled = w2
        .shell
        .read_with(&w2.vcx, |s, cx| s.agents().window.read(cx).disabled.clone());
    assert_eq!(disabled.as_deref(), Some(super::CANNOT_RESUME));
    assert!(w2.vcx.debug_bounds("agents-prompt-disabled").is_some());
    assert!(w2.user_rows().contains(&"Hello quick".to_owned()));
    let refused = w2
        .agents_bus(eludite_commands::agents::PROMPT, json!({"text": "more"}))
        .unwrap_err();
    assert!(refused.contains("cannot resume"), "{refused}");
    assert_eq!(w2.agents_state(), StateKind::Stopped);
    // Restart begins a new session with the same agent.
    w2.agents_bus(eludite_commands::agents::START, json!({"restart": true}))
        .unwrap();
    let fresh = w2.shown().unwrap();
    assert_ne!(fresh, quick);
    w2.wait("the new session", |w| w.agents_state() == StateKind::Ready);
    assert_eq!(w2.state_json()["agent"], "Fake quick");
    let disabled = w2
        .shell
        .read_with(&w2.vcx, |s, cx| s.agents().window.read(cx).disabled.clone());
    assert_eq!(disabled, None);
}

/// Brief 0060: the store keeps the 100 most recent records (the 101st deletes the oldest file), and a ninth live
/// session stops the oldest idle one, whose record stays.
#[gpui::test]
fn the_store_keeps_a_hundred_and_eight_sessions_stay_live(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let mut w = setup_full(cx, |_| {}, Some(session_agents(state.path())));
    let dir = super::sessions::dir_for(state.path(), w.path("App.slnx").parent().unwrap());
    for i in 0..super::sessions::KEEP {
        let r = super::sessions::Record {
            version: super::sessions::RECORD_VERSION,
            meta: super::sessions::SessionMeta {
                id: format!("old-{i:03}"),
                agent: "Fake quick".into(),
                acp_session_id: None,
                title: format!("Old {i}"),
                started: format!("2026-01-01T00:00:00.{i:03}Z"),
                last_activity: format!("2026-01-01T00:00:00.{i:03}Z"),
                model: None,
            },
            ended: true,
            usage: None,
            transcript: json!([{"user": format!("Old {i}"), "time": "09:00"}]),
        };
        super::sessions::write(&dir, &r).unwrap();
    }
    w.open_solution();
    w.show_agents();
    w.wait("the stored sessions", |w| {
        w.sessions()["sessions"].as_array().unwrap().len() == super::sessions::KEEP
    });
    assert_eq!(w.store_dir(), dir);
    // The history list shows 50 and says there are more.
    w.click(super::window::HISTORY_BUTTON);
    assert_eq!(w.history_rows().len(), super::sessions::HISTORY_ROWS);
    assert!(w.vcx.debug_bounds(super::window::HISTORY_MENU).is_some());
    w.vcx.simulate_keystrokes("escape");
    w.vcx.run_until_parked();

    w.start_agent("Fake quick");
    let first = w.shown().unwrap();
    w.type_prompt("The hundred and first");
    w.wait_session_turn(&first);
    w.wait("the oldest record deleted", |_| {
        !dir.join("old-000.json").exists()
    });
    assert!(dir.join("old-001.json").exists());
    assert!(dir.join(format!("{first}.json")).exists());
    assert_eq!(
        w.sessions()["sessions"].as_array().unwrap().len(),
        super::sessions::KEEP
    );

    // Seven more live sessions, idle: eight live.
    let mut ids = vec![first.clone()];
    for _ in 0..7 {
        w.agents_bus(eludite_commands::agents::NEW_SESSION, json!({}))
            .unwrap();
        w.wait("ready", |w| w.agents_state() == StateKind::Ready);
        ids.push(w.shown().unwrap());
    }
    let live = |w: &Ws| {
        w.shell.read_with(&w.vcx, |s, _| {
            ids.iter()
                .filter(|id| s.agents().session_slot(id).is_some_and(|x| x.live()))
                .count()
        })
    };
    assert_eq!(live(&w), 8);
    // The ninth stops the oldest idle one, the first.
    w.agents_bus(eludite_commands::agents::NEW_SESSION, json!({}))
        .unwrap();
    w.wait("ready", |w| w.agents_state() == StateKind::Ready);
    let ninth = w.shown().unwrap();
    assert!(!ids.contains(&ninth));
    assert_eq!(w.session_state(&first).0, StateKind::Stopped);
    assert_eq!(live(&w), 7);
    assert!(w.session_row(&first)["live"] == false);
    assert!(dir.join(format!("{first}.json")).exists());
    assert!(w.record(&first).ended);
}

/// Brief 0060: the header's `+` starts a new session with the shown session's agent and the agent picker's other
/// agent starts a new session with it, the earlier ones running on; each new session's prompt box is empty and the
/// earlier one's text comes back with it.
#[gpui::test]
fn new_session_from_the_header_and_the_agent_picker(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let mut w = setup_full(cx, |_| {}, Some(session_agents(state.path())));
    w.open_solution();
    w.show_agents();
    w.click(super::window::NEW_BUTTON);
    let first = w.shown().expect("a session");
    w.wait("ready", |w| w.agents_state() == StateKind::Ready);
    assert_eq!(w.state_json()["agent"], "Fake streamer");
    w.click(super::window::PROMPT_BOX);
    w.type_keys("draft one");
    w.click(super::window::NEW_BUTTON);
    let second = w.shown().unwrap();
    assert_ne!(first, second);
    assert_eq!(w.prompt_box().0, "");
    w.wait("ready", |w| w.agents_state() == StateKind::Ready);
    assert!(w.session_row(&first)["live"] == true);
    // Another agent in the picker: a new session with it.
    w.click(super::window::AGENT_PICKER);
    w.click(&super::window::agent_item(2));
    let third = w.shown().unwrap();
    assert!(third != first && third != second);
    w.wait("ready", |w| w.agents_state() == StateKind::Ready);
    assert_eq!(w.state_json()["agent"], "Fake editor");
    for id in [&first, &second] {
        assert!(w.session_row(id)["live"] == true);
    }
    // Back to the first: its draft comes back, its list at the end.
    w.agents_bus(eludite_commands::agents::SWITCH, json!({"session": first}))
        .unwrap();
    assert_eq!(w.prompt_box().0, "draft one");
    assert_eq!(w.state_json()["agent"], "Fake streamer");
    // Unknown sessions are refused.
    let e = w
        .agents_bus(eludite_commands::agents::SWITCH, json!({"session": "nope"}))
        .unwrap_err();
    assert!(e.contains("no session"), "{e}");
}
