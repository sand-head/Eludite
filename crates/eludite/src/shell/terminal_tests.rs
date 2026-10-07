//! Headless tests of the Terminal window (brief 0041), with real shells (`sh`; `bash` with the integration where an
//! exit code matters) on real PTYs: Ctrl+` opens the window with a terminal in the workspace, New Terminal, Split,
//! Kill, the exit line and Restart, typing and the rendered output, an agent's open, send, wait and read with the
//! marker on the tab, the person interrupting an agent's wait, the policy's `terminal.run` through the Agents window,
//! Ctrl+click on a `path:line:col`, Open in Terminal, the reserved chords, the layout, closing the workspace, and the
//! budgets. Every wait is for a marker in the output.
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::terminal as cmds;
use eludite_commands::{Caller, with_caller, workspace};
use eludite_docking::ids;
use eludite_terminal::TerminalView;
use gpui::{Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext};
use serde_json::{Value, json};

use super::terminal::{self as term, TerminalWindow};
use super::tests::{T, Ws, setup, setup_full};

fn agent_caller() -> Caller {
    Caller::Agent {
        agent: "Claude".into(),
        call: 1,
        tool_call: None,
    }
}

struct Tw {
    w: Ws,
    home: tempfile::TempDir,
}

/// The shell with two test profiles in a home without startup files: `sh` (the default; prompt `$ `) and `bash`.
fn setup_terminal(cx: &mut TestAppContext) -> Tw {
    terminal_setup(setup(cx))
}

fn terminal_setup(w: Ws) -> Tw {
    let home = tempfile::tempdir().unwrap();
    let h = home.path().to_string_lossy().into_owned();
    let profiles = json!([
        {"name": "sh", "command": "/bin/sh", "env": {"PS1": "$ ", "HOME": h, "ENV": ""}},
        {"name": "bash", "command": "bash", "env": {"HOME": h}}
    ]);
    w.commands
        .invoke(
            "eludite.settings.set",
            json!({"key": "terminal.profiles", "value": profiles}),
        )
        .unwrap();
    w.commands
        .invoke(
            "eludite.settings.set",
            json!({"key": "terminal.defaultProfile", "value": "sh"}),
        )
        .unwrap();
    let mut t = Tw { w, home };
    t.w.wait("the terminal settings", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.terminal_ui()
                .window
                .read(cx)
                .profiles()
                .first()
                .map(String::as_str)
                == Some("sh")
        })
    });
    t
}

impl Tw {
    fn window(&self) -> Entity<TerminalWindow> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.terminal_ui().window.clone())
    }

    fn tabs(&self) -> Vec<Vec<String>> {
        let win = self.window();
        win.read_with(&self.w.vcx, |w, _| w.tabs().to_vec())
    }

    fn view(&self, id: &str) -> Entity<TerminalView> {
        let win = self.window();
        win.read_with(&self.w.vcx, |w, _| w.view(id).cloned())
            .unwrap_or_else(|| panic!("no view for {id}"))
    }

    fn service(&self) -> Arc<term::TerminalService> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.terminal_ui().service.clone())
    }

    fn terminal(&self, id: &str) -> eludite_terminal::Terminal {
        self.service().handle(id).unwrap()
    }

    /// Run the UI until terminal `id`'s screen satisfies `f`.
    fn wait_screen(&mut self, id: &str, what: &str, f: impl Fn(&str) -> bool) {
        let t = self.terminal(id);
        let deadline = Instant::now() + T;
        loop {
            self.w.vcx.run_until_parked();
            let text = t.screen().text();
            if f(&text) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; screen:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Run the UI until the view of `id` has drawn text satisfying `f`.
    fn wait_drawn(&mut self, id: &str, what: &str, f: impl Fn(&str) -> bool) {
        let view = self.view(id);
        let deadline = Instant::now() + T;
        loop {
            self.w.vcx.run_until_parked();
            let text = view.read_with(&self.w.vcx, |v, _| v.visible_text());
            if f(&text) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; drawn:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Run `command` as the agent on its own thread while the UI runs.
    fn agent(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let commands = self.w.commands.clone();
        let command = command.to_owned();
        let t = std::thread::spawn(move || {
            with_caller(agent_caller(), || commands.invoke(&command, args))
                .map_err(|e| e.to_string())
        });
        while !t.is_finished() {
            self.w.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        t.join().unwrap()
    }

    /// The person's command (as a key or button would run it), until a terminal count is reached.
    fn run_ui(&mut self, command: &str, args: Value) {
        let command = command.to_owned();
        self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.run(&command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
    }

    fn wait_terminals(&mut self, n: usize) -> Vec<String> {
        let service = self.service();
        self.w.wait("the terminals", |_| service.ids().len() == n);
        // And their views.
        let win = self.window();
        self.w.wait("their views", |w| {
            win.read_with(&w.vcx, |win, _| {
                win.tabs().iter().map(Vec::len).sum::<usize>() == n
            })
        });
        service.ids()
    }

    fn focus_terminal(&mut self, id: &str) {
        let view = self.view(id);
        self.w.vcx.update(|window, cx| {
            gpui::Focusable::focus_handle(&view, cx).focus(window, cx);
        });
        self.w.vcx.run_until_parked();
    }

    /// Type `keys` (space-separated keystrokes) into the focused window.
    fn type_keys(&mut self, keys: &str) {
        self.w.vcx.simulate_keystrokes(keys);
        self.w.vcx.run_until_parked();
    }
}

/// Keystrokes for `text` (letters, digits, spaces and a few symbols).
fn keys_for(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            ' ' => "space".to_owned(),
            '-' => "-".to_owned(),
            c => c.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[gpui::test]
fn ctrl_backtick_opens_the_window_with_a_terminal_in_the_workspace(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    // Nothing runs at startup; the window is a closed tab beside Output.
    assert!(t.service().ids().is_empty());
    let layout = t.w.controller.snapshot().layout;
    assert_eq!(
        layout.bottom.groups[0].tabs,
        [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL],
        "the default layout's bottom group"
    );
    t.w.open_solution();
    let root = t.w.dir.path().to_path_buf();
    t.w.vcx.simulate_keystrokes("ctrl-`");
    let ids = t.wait_terminals(1);
    let id = ids[0].clone();
    assert_eq!(
        t.w.controller.snapshot().layout.bottom.groups[0].active_id(),
        Some(ids::TERMINAL)
    );
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    let m = t.terminal(&id).mark();
    t.terminal(&id).write("pwd\r");
    t.wait_screen(&id, "pwd", |s| {
        s.contains(&root.to_string_lossy().into_owned())
    });
    let _ = m;
    // The tab has the profile's name; the view has focus and draws the output.
    let win = t.window();
    assert_eq!(
        win.read_with(&t.w.vcx, |w, _| w.name(&id).map(str::to_owned)),
        Some("sh".into())
    );
    let root_s = root.to_string_lossy().into_owned();
    t.wait_drawn(&id, "the drawn pwd", |s| s.contains(&root_s));
    let view = t.view(&id);
    let focused =
        t.w.vcx
            .update(|window, cx| gpui::Focusable::focus_handle(&view, cx).is_focused(window));
    assert!(focused, "the terminal has focus");
    // Ctrl+` again keeps the one terminal.
    t.w.vcx.simulate_keystrokes("ctrl-`");
    t.w.vcx.run_until_parked();
    assert_eq!(t.service().ids().len(), 1);
}

#[gpui::test]
fn new_terminal_split_kill_and_the_exit_line_with_restart(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let first = t.wait_terminals(1)[0].clone();
    // New Terminal (the button) adds a tab with the profile's name, numbered.
    t.w.click(term::NEW);
    let ids = t.wait_terminals(2);
    let second = ids[1].clone();
    let win = t.window();
    assert_eq!(
        win.read_with(&t.w.vcx, |w, _| w.name(&second).map(str::to_owned)),
        Some("sh (2)".into())
    );
    assert_eq!(t.tabs(), [vec![first.clone()], vec![second.clone()]]);
    // The dropdown opens a terminal of another profile.
    t.w.click(term::PROFILES);
    t.w.click(&term::profile_selector("bash"));
    let ids = t.wait_terminals(3);
    let third = ids[2].clone();
    assert_eq!(
        win.read_with(&t.w.vcx, |w, _| w.name(&third).map(str::to_owned)),
        Some("bash".into())
    );
    // Split: two terminals side by side in the active tab.
    t.w.click(term::SPLIT);
    let ids = t.wait_terminals(4);
    let fourth = ids[3].clone();
    assert_eq!(t.tabs()[2], [third.clone(), fourth.clone()]);
    assert!(
        t.w.vcx
            .debug_bounds(Box::leak(term::pane_selector(&third).into_boxed_str()))
            .is_some()
    );
    let b3 = t.w.bounds(&term::pane_selector(&third));
    let b4 = t.w.bounds(&term::pane_selector(&fourth));
    assert!(
        b4.origin.x > b3.origin.x && b3.size.width > gpui::px(50.),
        "side by side"
    );
    // Kill closes the active terminal.
    t.w.click(term::KILL);
    t.wait_terminals(3);
    assert!(t.service().handle(&fourth).is_none());
    assert_eq!(t.tabs()[2], std::slice::from_ref(&third));
    // A shell that ends shows the exit line; Restart starts a new one in its place.
    t.terminal(&second).write("exit 3\r");
    t.w.click(&term::tab_selector(&second));
    let view = t.view(&second);
    t.w.wait("the exit line", |w| {
        w.vcx.run_until_parked();
        view.read_with(&w.vcx, |v, _| v.terminal().exited() == Some(Some(3)))
            && w.vcx.debug_bounds("terminal-exit").is_some()
    });
    t.w.click("terminal-restart");
    let service = t.service();
    t.w.wait("the restarted terminal", |_| {
        !service.ids().contains(&second) && service.ids().len() == 3
    });
    let restarted = t.tabs()[1][0].clone();
    assert_ne!(restarted, second);
    assert_eq!(
        win.read_with(&t.w.vcx, |w, _| w.name(&restarted).map(str::to_owned)),
        Some("sh (2)".into())
    );
    t.wait_screen(&restarted, "its prompt", |s| s.ends_with('$'));
    // The tab's close button closes it.
    t.w.click(&term::tab_close_selector(&first));
    t.wait_terminals(2);
}

#[gpui::test]
fn typing_reaches_the_shell_and_the_output_renders(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_drawn(&id, "the prompt", |s| s.ends_with('$'));
    t.focus_terminal(&id);
    let keys = format!("{} enter", keys_for("echo typed-123"));
    t.type_keys(&keys);
    t.wait_drawn(&id, "the echo", |s| s.lines().any(|l| l == "typed-123"));
    // Keystroke to the echoed character painted (the view's frame counter).
    for c in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        t.type_keys(c);
        let view = t.view(&id);
        t.w.wait("the echoed key", |w| {
            w.vcx.run_until_parked();
            view.read_with(&w.vcx, |v, _| {
                v.visible_text()
                    .lines()
                    .last()
                    .is_some_and(|l| l.ends_with(c))
            })
        });
    }
    let mut lat: Vec<Duration> = t
        .view(&id)
        .read_with(&t.w.vcx, |v, _| v.key_latencies().to_vec());
    assert!(lat.len() >= 8, "{} latencies", lat.len());
    lat.sort();
    let p95 = lat[(lat.len() * 95 / 100).min(lat.len() - 1)];
    eprintln!(
        "timing: keystroke to echoed character drawn p50 {:.2} ms, p95 {:.2} ms ({} keys, headless)",
        lat[lat.len() / 2].as_secs_f64() * 1e3,
        p95.as_secs_f64() * 1e3,
        lat.len()
    );
    super::tests::assert_budget("keystroke to echo", p95, Duration::from_millis(16));
    // Ctrl+C with no selection interrupts the line (VS's choice); a selection is copied instead.
    t.type_keys("ctrl-c");
    t.wait_drawn(&id, "a new prompt", |s| s.ends_with('$'));
    let view = t.view(&id);
    view.update(&mut t.w.vcx, |v, cx| {
        v.set_selection(
            Some((
                eludite_terminal::view::AbsPoint { line: 0, column: 0 },
                eludite_terminal::view::AbsPoint { line: 0, column: 0 },
            )),
            cx,
        )
    });
    let expected = view.read_with(&t.w.vcx, |v, _| v.selection_text()).unwrap();
    let before = t.terminal(&id).mark();
    t.type_keys("ctrl-c");
    assert_eq!(
        t.w.vcx.read_from_clipboard().and_then(|c| c.text()),
        Some(expected)
    );
    std::thread::sleep(Duration::from_millis(100));
    t.w.vcx.run_until_parked();
    assert_eq!(t.terminal(&id).mark(), before, "nothing went to the shell");
}

#[gpui::test]
fn an_agent_opens_sends_waits_and_reads_with_the_marker_on_the_tab(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.w.open_solution();
    let opened = t
        .agent(
            cmds::OPEN,
            json!({"profile": "bash", "name": "agent shell"}),
        )
        .unwrap();
    let id = opened["terminal"].as_str().unwrap().to_owned();
    assert_eq!(opened["integration"], true, "bash with the integration");
    assert_eq!(opened["cwd"], t.w.dir.path().to_string_lossy().as_ref());
    t.wait_terminals(1);
    // The window shows the agent's terminal (without taking the focus).
    assert_eq!(
        t.w.controller.snapshot().layout.bottom.groups[0].active_id(),
        Some(ids::TERMINAL)
    );
    // The first prompt.
    let w0 = t
        .agent(cmds::WAIT, json!({"terminal": id, "mark": 0}))
        .unwrap();
    assert_eq!(w0["matched"], "prompt", "{w0}");
    // send, then wait for the prompt on another thread while the UI shows the marker.
    let sent = t
        .agent(
            cmds::SEND,
            json!({"terminal": id, "text": "sleep 1; echo agent-$((40 + 2))"}),
        )
        .unwrap();
    let mark = sent["mark"].as_u64().unwrap();
    let commands = t.w.commands.clone();
    let tid = id.clone();
    let waiter = std::thread::spawn(move || {
        with_caller(agent_caller(), || {
            commands.invoke(cmds::WAIT, json!({"terminal": tid, "prompt": true}))
        })
    });
    let win = t.window();
    let marker = term::agent_selector(&id);
    t.w.wait("the marker", |w| {
        win.read_with(&w.vcx, |win, _| win.agent(&id) == Some("Claude"))
            && w.vcx
                .debug_bounds(Box::leak(marker.clone().into_boxed_str()))
                .is_some()
    });
    while !waiter.is_finished() {
        t.w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    let waited = waiter.join().unwrap().unwrap();
    assert_eq!(waited["matched"], "prompt");
    assert_eq!(waited["exit_code"], 0);
    assert_eq!(waited["text"].as_str().unwrap().trim(), "agent-42");
    // read since the send's mark: the command line and its output.
    let read = t
        .agent(
            cmds::READ,
            json!({"terminal": id, "mode": "since", "mark": mark}),
        )
        .unwrap();
    assert!(read["text"].as_str().unwrap().contains("agent-42"));
    assert_eq!(read["integration"], true);
    // The marker goes a moment after the calls end.
    t.w.vcx.executor().advance_clock(Duration::from_secs(1));
    t.w.wait("the marker gone", |w| {
        win.read_with(&w.vcx, |win, _| win.agent(&id).is_none())
    });
    // The person sees it: the view drew the output.
    t.wait_drawn(&id, "the agent's output", |s| s.contains("agent-42"));
    // list reports it.
    let list = t.agent(cmds::LIST, json!({})).unwrap();
    assert_eq!(list["terminals"][0]["name"], "agent shell");
    assert_eq!(list["active"], id.as_str());
    // Audited as the agent's.
    let entries = t.w.commands.audit_log().entries();
    assert!(
        entries
            .iter()
            .any(|e| e.command == cmds::SEND && e.caller.is_agent())
    );
}

#[gpui::test]
fn the_persons_keystroke_interrupts_an_agents_wait_and_refuses_its_next_send(
    cx: &mut TestAppContext,
) {
    let mut t = setup_terminal(cx);
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    t.agent(cmds::SEND, json!({"text": "sleep 20"})).unwrap();
    let commands = t.w.commands.clone();
    let waiter = std::thread::spawn(move || {
        with_caller(agent_caller(), || {
            commands.invoke(cmds::WAIT, json!({"prompt": true, "timeout_ms": 30000}))
        })
    });
    let terminal = t.terminal(&id);
    t.w.wait("the agent waiting", |_| terminal.waiting() > 0);
    let started = Instant::now();
    t.focus_terminal(&id);
    // The person presses Ctrl+C.
    t.type_keys("ctrl-c");
    while !waiter.is_finished() {
        t.w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    let r = waiter.join().unwrap().unwrap();
    assert_eq!(r["matched"], "interrupted");
    assert_eq!(r["interrupted_by"], "user");
    eprintln!(
        "timing: an interrupted wait answered {:.1} ms after the keystroke",
        started.elapsed().as_secs_f64() * 1e3
    );
    // The agent's next send is refused until it reads.
    let refused = t
        .agent(cmds::SEND, json!({"text": "echo again"}))
        .unwrap_err();
    assert!(refused.contains("call eludite.terminal.read"), "{refused}");
    // The wait ends at the keystroke, before `sh` has printed its next prompt: text sent before that is typed ahead
    // and lands after the prompt, so wait for it as an agent's read would show it.
    t.wait_screen(&id, "the prompt after Ctrl+C", |s| s.ends_with('$'));
    t.agent(cmds::READ, json!({})).unwrap();
    t.agent(cmds::SEND, json!({"text": "echo again-ok"}))
        .unwrap();
    t.wait_screen(&id, "the second send", |s| {
        s.lines().any(|l| l == "again-ok")
    });
}

#[gpui::test]
fn ctrl_click_on_a_path_printed_by_the_shell_opens_the_editor_at_the_line(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.w.open_solution();
    let lib = t.w.path("src/lib.rs");
    std::fs::write(&lib, "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n").unwrap();
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    t.terminal(&id)
        .write("echo 'error at src/lib.rs:3:1 here'\r");
    t.wait_drawn(&id, "the path", |s| {
        s.lines().any(|l| l.starts_with("error at"))
    });
    let view = t.view(&id);
    let (row, col) = view.read_with(&t.w.vcx, |v, _| {
        let rows = v.visible_rows();
        let r = rows.iter().position(|l| l.starts_with("error at")).unwrap();
        (r, rows[r].find("lib.rs").unwrap())
    });
    let at = view.read_with(&t.w.vcx, |v, _| v.cell_center(row, col));
    // A plain click selects nothing and opens nothing.
    t.w.vcx.simulate_click(at, Modifiers::none());
    t.w.vcx.run_until_parked();
    assert!(
        t.w.shell
            .read_with(&t.w.vcx, |s, _| s.editor(&lib).is_none())
    );
    // Ctrl+click opens the file at line 3.
    t.w.vcx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::control(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    t.w.vcx.simulate_event(MouseUpEvent {
        position: at,
        modifiers: Modifiers::control(),
        button: MouseButton::Left,
        click_count: 1,
    });
    let editor = t.w.editor(&lib);
    let line = editor.read_with(&t.w.vcx, |v, _| {
        let e = v.editor();
        let head = e.primary_selection().head;
        e.buffer().snapshot().offset_to_point(head).row
    });
    assert_eq!(line, 2, "line 3 (0-based 2)");

    // Ctrl+click on a folder shows the Workspace at it: here the project in `src/App`.
    t.terminal(&id).write("echo 'folder src/App here'\r");
    t.wait_drawn(&id, "the folder", |s| {
        s.lines().any(|l| l.starts_with("folder src/App"))
    });
    let at = view.read_with(&t.w.vcx, |v, _| {
        let rows = v.visible_rows();
        let r = rows
            .iter()
            .position(|l| l.starts_with("folder src/App"))
            .unwrap();
        v.cell_center(r, rows[r].find("App").unwrap())
    });
    t.w.vcx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::control(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    t.w.vcx.simulate_event(MouseUpEvent {
        position: at,
        modifiers: Modifiers::control(),
        button: MouseButton::Left,
        click_count: 1,
    });
    let project =
        t.w.path("src/App/App.csproj")
            .to_string_lossy()
            .into_owned();
    t.w.wait("the Workspace at the folder", |w| {
        let shown =
            w.controller.snapshot().layout.right.groups[0].active_id() == Some(ids::WORKSPACE);
        let selected = w.shell.read_with(&w.vcx, |s, cx| {
            s.explorer.read(cx).selected_id().map(str::to_owned)
        });
        shown && selected.as_deref() == Some(project.as_str())
    });
}

#[gpui::test]
fn open_in_terminal_from_the_workspace_uses_the_projects_folder(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.w.open_solution();
    let project = t.w.path("src/App/App.csproj");
    let row = super::explorer::row_selector(&project.to_string_lossy());
    let at = t.w.bounds(&row).center();
    t.w.vcx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Right,
        click_count: 1,
        first_mouse: false,
    });
    t.w.vcx.run_until_parked();
    t.w.click(&super::explorer::context_item_selector("terminal"));
    let id = t.wait_terminals(1)[0].clone();
    let list = t.agent(cmds::LIST, json!({})).unwrap();
    let folder = t.w.path("src/App");
    assert_eq!(
        list["terminals"][0]["cwd"],
        folder.to_string_lossy().as_ref()
    );
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    t.terminal(&id).write("pwd\r");
    // macOS's temporary folder is a symlink (`/var` to `/private/var`); the shell prints its physical cwd.
    let f = folder.to_string_lossy().into_owned();
    let real = std::fs::canonicalize(&folder)
        .unwrap_or_else(|_| folder.clone())
        .to_string_lossy()
        .into_owned();
    t.wait_screen(&id, "pwd", |s| s.lines().any(|l| l == f || l == real));
    // The terminal window is shown, with the terminal focused.
    assert_eq!(
        t.w.controller.snapshot().layout.bottom.groups[0].active_id(),
        Some(ids::TERMINAL)
    );
}

#[gpui::test]
fn the_reserved_chords_reach_the_shell_window_and_the_rest_the_terminal(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.w.open_solution();
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    t.focus_terminal(&id);
    let audited = |t: &Tw, c: &str| {
        t.w.commands
            .audit_log()
            .entries()
            .iter()
            .filter(|e| e.command == c)
            .count()
    };
    // Ctrl+S is the shell's (XOFF here), not Save; Ctrl+R (a chord prefix in the IDE) reaches bash-style shells.
    let saves = audited(&t, workspace::EDITOR_SAVE);
    let before = t.terminal(&id).mark();
    t.type_keys("ctrl-s");
    t.type_keys("ctrl-q");
    t.type_keys("ctrl-r");
    t.type_keys("ctrl-c");
    assert_eq!(
        audited(&t, workspace::EDITOR_SAVE),
        saves,
        "Ctrl+S did not save"
    );
    let handle = t.terminal(&id);
    t.wait_screen(&id, "the shell answered the keys", |_| {
        handle.mark() > before
    });
    // Ctrl+Shift+B builds while the terminal has focus.
    let builds = audited(&t, eludite_commands::build::SOLUTION);
    t.type_keys("ctrl-shift-b");
    t.w.wait("the build", |w| {
        w.commands
            .audit_log()
            .entries()
            .iter()
            .filter(|e| e.command == eludite_commands::build::SOLUTION)
            .count()
            > builds
    });
    // A tool window's chord stays the shell's: Ctrl+\, Ctrl+E shows the Error List.
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    t.focus_terminal(&id);
    t.type_keys("ctrl-\\ ctrl-e");
    t.w.wait("the Error List", |w| {
        w.controller.snapshot().layout.bottom.groups[0].active_id() == Some(ids::ERROR_LIST)
    });
    // Escape goes back to the editor (here the shell, no document open).
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    t.focus_terminal(&id);
    t.type_keys("escape");
    let view = t.view(&id);
    let focused =
        t.w.vcx
            .update(|window, cx| gpui::Focusable::focus_handle(&view, cx).is_focused(window));
    assert!(!focused, "Escape left the terminal");
}

#[gpui::test]
fn closing_the_workspace_ends_the_shells_and_a_running_command_asks(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.w.open_solution();
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    let terminal = t.terminal(&id);
    terminal.write("sleep 30\r");
    t.w.wait("the sleep", |_| terminal.busy());
    // File > Close Workspace asks, naming the command; No keeps everything.
    t.run_ui(workspace::WORKSPACE_CLOSE, json!({}));
    assert!(t.w.vcx.has_pending_prompt(), "a running command asks");
    t.w.vcx.simulate_prompt_answer("No");
    t.w.vcx.run_until_parked();
    assert_eq!(t.service().ids(), std::slice::from_ref(&id));
    assert!(terminal.busy());
    // Yes closes the workspace and ends the shell.
    t.run_ui(workspace::WORKSPACE_CLOSE, json!({}));
    t.w.vcx.simulate_prompt_answer("Yes");
    let service = t.service();
    t.w.wait("the terminals ended", |_| service.ids().is_empty());
    t.w.wait("the shell gone", |_| terminal.exited().is_some());
    assert!(t.tabs().is_empty());
    // With nothing running it does not ask.
    t.w.open_solution();
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    t.run_ui(workspace::WORKSPACE_CLOSE, json!({}));
    assert!(!t.w.vcx.has_pending_prompt());
    t.w.wait("closed", |_| service.ids().is_empty());
    // Killing a busy terminal from the window asks too.
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    let terminal = t.terminal(&id);
    terminal.write("sleep 30\r");
    t.w.wait("the sleep", |_| terminal.busy());
    t.w.click(term::KILL);
    assert!(
        t.w.vcx.has_pending_prompt(),
        "Kill over a running command asks"
    );
    t.w.vcx.simulate_prompt_answer("Yes");
    t.w.wait("killed", |_| service.ids().is_empty());
}

#[gpui::test]
fn the_policy_refuses_with_deny_and_prompt_asks_once_per_session(cx: &mut TestAppContext) {
    let steps = json!([
        {"tool": "eludite-terminal-open", "arguments": {"profile": "sh"}},
        {"tool": "eludite-terminal-send", "arguments": {"text": "echo one"}},
        {"tool": "eludite-terminal-send", "arguments": {"text": "echo two"}},
        {"tool": "eludite-terminal-wait", "arguments": {"pattern": "(?m)^two"}}
    ]);
    let agents = crate::shell::agents::tests::fake_agents(vec![(
        "Scripted".into(),
        vec![
            "--scenario".into(),
            "script".into(),
            "--script".into(),
            steps.to_string(),
        ],
    )]);
    let w = setup_full(cx, |_| {}, Some(agents));
    let mut t = terminal_setup(w);
    t.w.open_solution();
    let policy = eludite_commands::policy::AgentPolicy::path_for(t.w.dir.path());
    std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
    // deny: the open is refused, naming the policy.
    std::fs::write(&policy, r#"{"version": 1, "terminal": {"run": "deny"}}"#).unwrap();
    run_agent(&mut t);
    wait_turn(&mut t);
    let rows = transcript(&t);
    let open = step(&rows, 1);
    assert_eq!(open["status"], "failed", "{open}");
    assert!(open.to_string().contains("terminal.run to deny"), "{open}");
    assert!(t.service().ids().is_empty());
    // prompt (the default): the first call asks, "Allow for this session" lets the rest run.
    std::fs::write(&policy, r#"{"version": 1}"#).unwrap();
    run_agent(&mut t);
    t.w.wait("the permission prompt", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).prompt.is_some())
    });
    let p =
        t.w.shell
            .read_with(&t.w.vcx, |s, cx| s.agents().window.read(cx).prompt.clone())
            .unwrap();
    assert_eq!(p.class, "dangerous");
    assert!(p.session && p.can_persist, "{p:?}");
    assert!(
        p.reason
            .as_deref()
            .unwrap()
            .contains("terminal.run: prompt")
    );
    t.w.shell
        .update(&mut t.w.vcx, |s, cx| {
            s.agents_answer(
                p.request,
                crate::shell::agents::window::Decision::AlwaysAllow,
                cx,
            )
        })
        .unwrap();
    wait_turn(&mut t);
    let rows = transcript(&t);
    for n in 1..=4 {
        assert_eq!(
            step(&rows, n)["status"],
            "completed",
            "step {n}: {}",
            step(&rows, n)
        );
    }
    // Asked once: the open was the person's answer, the sends ran on the session's grant at class execute.
    let note = |n: usize| {
        step(&rows, n)["note"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    assert!(
        note(1).contains("Allowed by you") && note(1).contains("for this session"),
        "{}",
        note(1)
    );
    for n in 2..=3 {
        assert!(!note(n).contains("Allowed by you"), "{}", note(n));
        assert!(
            note(n).contains("eludite.terminal.send (execute)"),
            "{}",
            note(n)
        );
    }
    assert!(
        step(&rows, 2)["debug"]
            .as_str()
            .unwrap()
            .starts_with("Typed `echo one`")
    );
    assert!(step(&rows, 4)["debug"].as_str().unwrap().contains("two"));
    // Nothing was written to the policy file.
    assert_eq!(
        std::fs::read_to_string(&policy).unwrap(),
        r#"{"version": 1}"#
    );
}

fn run_agent(t: &mut Tw) {
    t.w.shell
        .update(&mut t.w.vcx, |s, cx| {
            s.agents.last_stop = None;
            s.agents_start(Some("Scripted"), true, cx)?;
            s.agents_prompt("go", cx)
        })
        .unwrap();
}

fn wait_turn(t: &mut Tw) {
    t.w.wait("the turn's end", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.agents().last_stop.is_some()
                && s.agents().state != crate::shell::agents::window::StateKind::Running
        })
    });
}

fn transcript(t: &Tw) -> Value {
    t.w.shell.read_with(&t.w.vcx, |s, cx| {
        s.agents().window.read(cx).transcript.to_json()
    })
}

fn step(rows: &Value, n: usize) -> Value {
    let id = format!("toolu_fake_step_{n}");
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["tool_call"]["id"] == id)
        .map(|r| r["tool_call"].clone())
        .unwrap_or(Value::Null)
}

/// Budgets (brief 0041): `cat` of a 100 MB file keeps the terminal element's frame under 8 ms at p99 and ends within
/// twice the time of reading the file through a PTY with nothing drawn; 10,000 lines of scrollback stay under 20 MB.
#[gpui::test]
fn cat_of_a_large_file_keeps_frames_short_and_the_scrollback_small(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    let size_mb: usize = std::env::var("ELUDITE_TERMINAL_CAT_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let file = t.home.path().join("big.txt");
    {
        use std::io::Write as _;
        let mut f = std::io::BufWriter::new(std::fs::File::create(&file).unwrap());
        let line = "0123456789 the quick brown fox jumps over the lazy dog 0123456789 ABCDEFGHIJ\n";
        let n = size_mb * 1024 * 1024 / line.len();
        for _ in 0..n {
            f.write_all(line.as_bytes()).unwrap();
        }
    }
    let path = file.to_string_lossy().into_owned();
    // The plain terminal: the same PTY and emulator, nothing drawn.
    let plain = {
        let home = t.home.path().to_path_buf();
        let started = Instant::now();
        let term = eludite_terminal::Terminal::spawn(
            eludite_terminal::Options {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), format!("cat '{path}'")],
                env: Default::default(),
                cwd: Some(home),
                cols: 120,
                rows: 30,
                scrollback: 10_000,
            },
            Arc::new(|_| {}),
        )
        .unwrap();
        let r = term.wait(
            &eludite_terminal::WaitFor {
                exit: true,
                ..Default::default()
            },
            Duration::from_secs(300),
            &|| false,
        );
        assert_eq!(r.matched, eludite_terminal::Matched::Exit);
        started.elapsed()
    };
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    let view = t.view(&id);
    let frames_before = view.read_with(&t.w.vcx, |v, _| v.frame_times().len());
    let terminal = t.terminal(&id);
    let started = Instant::now();
    // The marker is printed as `cat-done` but typed as `"cat""-done"`, so the echoed command line never matches it.
    terminal.write(format!("cat '{path}'; echo \"cat\"\"-done\"\r"));
    let done = |text: &str| text.lines().any(|l| l == "cat-done");
    // Draw frames as the window would while it runs.
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        t.w.vcx.run_until_parked();
        let (text, _, _) = terminal.since(terminal.mark().saturating_sub(64), 64);
        if done(&text) {
            break;
        }
        assert!(Instant::now() < deadline, "cat never finished");
        std::thread::sleep(Duration::from_millis(16));
    }
    let drawn = started.elapsed();
    t.wait_drawn(&id, "the end", done);
    let mut frames: Vec<Duration> =
        view.read_with(&t.w.vcx, |v, _| v.frame_times()[frames_before..].to_vec());
    frames.sort();
    let p99 = frames[(frames.len() * 99 / 100).min(frames.len() - 1)];
    eprintln!(
        "timing: cat of {size_mb} MB: {} frames, frame p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms; done in {:.2} s \
         drawn vs {:.2} s plain ({:.2}x)",
        frames.len(),
        frames[frames.len() / 2].as_secs_f64() * 1e3,
        p99.as_secs_f64() * 1e3,
        frames.last().unwrap().as_secs_f64() * 1e3,
        drawn.as_secs_f64(),
        plain.as_secs_f64(),
        drawn.as_secs_f64() / plain.as_secs_f64()
    );
    super::tests::assert_budget(
        "terminal frame p99 during cat",
        p99,
        Duration::from_millis(8),
    );
    super::tests::assert_budget(
        "cat drawn vs plain",
        drawn,
        plain * 2 + Duration::from_millis(500),
    );
    // The scrollback is full (10,000 lines): its memory. alacritty_terminal keeps a cell per column per line, 24
    // bytes each (its own size assertion), so the budget holds for an 80-column terminal and grows with the width.
    let (lines, history) = terminal.all_lines();
    assert!(history >= 9_000, "{history} lines of history");
    let cols = terminal.size().0 as usize;
    let cell = 24;
    let bytes = |cols: usize| lines.len() * cols * cell;
    eprintln!(
        "memory: {} lines of scrollback and screen: {:.1} MB of cells at {cols} columns (this view), {:.1} MB at 80, \
         {:.1} MB at 120",
        lines.len(),
        bytes(cols) as f64 / 1e6,
        bytes(80) as f64 / 1e6,
        bytes(120) as f64 / 1e6
    );
    assert!(
        bytes(80) < 20 * 1024 * 1024,
        "{} bytes at 80 columns",
        bytes(80)
    );
}

/// The marks of a terminal's transcript are what the schemas call them: bytes of plain text.
#[gpui::test]
fn marks_are_bytes_of_plain_text_and_clear_keeps_them(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    let sent = t
        .agent(
            cmds::SEND,
            json!({"text": "printf '\\033[31mred\\033[0m\\n'"}),
        )
        .unwrap();
    let mark = sent["mark"].as_u64().unwrap();
    let w = t.agent(cmds::WAIT, json!({"mark": mark})).unwrap();
    assert!(w["text"].as_str().unwrap().contains("\nred\n"), "{w}");
    assert_eq!(w["integration"], false);
    let cleared = t.agent(cmds::CLEAR, json!({})).unwrap();
    assert!(cleared["mark"].as_u64().unwrap() >= w["mark"].as_u64().unwrap());
    let r = t
        .agent(cmds::READ, json!({"mode": "since", "mark": mark}))
        .unwrap();
    assert!(
        r["text"].as_str().unwrap().contains("red"),
        "marks survive Clear"
    );
    let resized = t
        .agent(cmds::RESIZE, json!({"cols": 90, "rows": 20}))
        .unwrap();
    assert_eq!(resized["cols"], 90);
    let refused = t
        .agent(cmds::CLOSE, json!({"terminal": "term99"}))
        .unwrap_err();
    assert!(refused.contains("eludite.terminal.list"), "{refused}");
    let _ = PathBuf::new();
}

/// Tools > Options has the Terminal page from the schema, and its settings reach the views (font size, copy on
/// select, the bell) and new terminals (the scrollback, the integration).
#[gpui::test]
fn the_options_page_and_the_terminal_settings_apply(cx: &mut TestAppContext) {
    let mut t = setup_terminal(cx);
    t.run_ui("eludite.view.show", json!({"id": ids::TERMINAL}));
    let id = t.wait_terminals(1)[0].clone();
    t.wait_screen(&id, "the prompt", |s| s.ends_with('$'));
    t.run_ui("eludite.tools.options", json!({"section": "Terminal"}));
    let section = t.w.shell.read_with(&t.w.vcx, |s, cx| {
        s.options.as_ref().map(|o| o.read(cx).section().to_owned())
    });
    assert_eq!(section.as_deref(), Some("Terminal"));
    for (key, value) in [
        ("terminal.fontSize", json!(20)),
        ("terminal.copyOnSelect", json!(true)),
        ("terminal.bell", json!("none")),
        ("terminal.scrollback", json!(500)),
    ] {
        t.w.commands
            .invoke("eludite.settings.set", json!({"key": key, "value": value}))
            .unwrap();
    }
    let view = t.view(&id);
    t.w.wait("the view's settings", |w| {
        view.read_with(&w.vcx, |v, _| v.grid_size().0 > 0)
            && w.shell.read_with(&w.vcx, |s, _| {
                s.terminal_ui().view_settings().is_some_and(|v| {
                    v.copy_on_select && !v.visual_bell && v.font_size == gpui::px(20.)
                })
            })
    });
    // The bell rings without a flash now.
    t.terminal(&id).write("printf '\\a'\r");
    t.w.vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(200));
    t.w.vcx.run_until_parked();
    assert!(!view.read_with(&t.w.vcx, |v, _| v.flashing()));
    // A new terminal keeps 500 lines.
    t.agent(cmds::OPEN, json!({})).unwrap();
    let ids = t.wait_terminals(2);
    let fresh = t.terminal(&ids[1]);
    t.wait_screen(&ids[1], "its prompt", |s| s.ends_with('$'));
    fresh.write("seq 1 2000\r");
    // The last number on a line of its own: the echoed command line also holds "2000".
    t.wait_screen(&ids[1], "the numbers", |s| s.lines().any(|l| l == "2000"));
    let (_, history) = fresh.all_lines();
    assert!((400..=500).contains(&history), "{history}");
}
