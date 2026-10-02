//! Headless GPUI tests of run and debug (brief 0018) against the scripted fake debug adapter from `eludite-dap`
//! (feature `fake`) and the fake `eludite-host`: F9 and the margin toggle breakpoints that persist per solution, F5
//! launches and breaks at a breakpoint with the windows populated, F10, F11 and Shift+F11 move the execution point,
//! the Call Stack selects a frame, Watch and data tips evaluate, variables expand lazily, conditional and hit-count
//! breakpoints, Shift+F5 tears the session down, stale and concurrent commands are refused, an adapter crash ends
//! the session, Ctrl+F5 runs without the debugger, the UI never waits on a stalled adapter, and an agent drives a
//! session from the bus reading the same state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_commands::debug as cmds;
use eludite_commands::{Caller, with_caller};
use eludite_dap::fake::{self, FakeHandle, FakeProgram, FakeStep, FakeVar};
use eludite_docking::ids;
use eludite_editor::{BreakpointGlyph, EditorEvent, ExecutionKind};
use gpui::{Modifiers, TestAppContext};
use serde_json::{Value, json};

use super::super::tests::{T, Ws, setup_debug};
use super::DebugSetup;
use super::state::Mode;
use crate::shell::documents::normalize_path;

const PROGRAM_CS: &str = "class Program\n{\n    static void Main()\n    {\n        var x = 1;\n        var y = Calc.Add(x, 2);\n        var order = new Order();\n        System.Console.WriteLine(y);\n    }\n}\n";
const CALC_CS: &str = "class Calc\n{\n    public static int Add(int a, int b)\n    {\n        var sum = a + b;\n        return sum;\n    }\n}\n";

/// The fake adapter's program over the test solution's files: Main (lines 5 to 8) calls Calc.Add (lines 5 and 6).
fn program(dir: &Path) -> FakeProgram {
    let p = |rel: &str| {
        normalize_path(&dir.join(rel))
            .to_string_lossy()
            .into_owned()
    };
    let (main, calc) = (p("src/App/Program.cs"), p("src/App/Calc.cs"));
    let v = FakeVar::new;
    let order = v("order", "{Order}", "Order")
        .with_children(vec![v("Id", "7", "int"), v("Name", "\"A\"", "string")]);
    FakeProgram {
        steps: vec![
            FakeStep::new(&main, 5, "Program.Main()", 0, vec![v("x", "0", "int")]),
            FakeStep::new(&main, 6, "Program.Main()", 0, vec![v("x", "1", "int")]),
            FakeStep::new(
                &calc,
                5,
                "Calc.Add(int, int)",
                1,
                vec![v("a", "1", "int"), v("b", "2", "int"), v("sum", "0", "int")],
            ),
            FakeStep::new(
                &calc,
                6,
                "Calc.Add(int, int)",
                1,
                vec![v("a", "1", "int"), v("b", "2", "int"), v("sum", "3", "int")],
            ),
            FakeStep::new(
                &main,
                7,
                "Program.Main()",
                0,
                vec![v("x", "1", "int"), v("y", "3", "int")],
            ),
            FakeStep::new(
                &main,
                8,
                "Program.Main()",
                0,
                vec![v("x", "1", "int"), v("y", "3", "int"), order],
            ),
        ],
        other_threads: vec![(2, ".NET TP Worker".into())],
        output_at_start: vec!["listening\n".into()],
        ..FakeProgram::default()
    }
}

fn state_of(w: &Ws) -> Value {
    w.shell.read_with(&w.vcx, |s, _| {
        serde_json::to_value(s.debugger().model.state()).unwrap()
    })
}

struct Dbg {
    w: Ws,
    /// The fake of the latest session.
    fake: Arc<Mutex<Option<FakeHandle>>>,
    store: PathBuf,
}

/// The test solution as an executable project that is built, the debugger reaching the fake adapter, and
/// breakpoints persisting under a temporary directory.
fn setup(cx: &mut TestAppContext) -> Dbg {
    setup_with(cx, |_| {})
}

fn setup_with(
    cx: &mut TestAppContext,
    tweak: impl Fn(&mut FakeProgram) + Send + Sync + 'static,
) -> Dbg {
    setup_dotnet(cx, tweak, "dotnet")
}

fn setup_dotnet(
    cx: &mut TestAppContext,
    tweak: impl Fn(&mut FakeProgram) + Send + Sync + 'static,
    dotnet: &str,
) -> Dbg {
    let store = tempfile::tempdir().unwrap().keep();
    let fake: Arc<Mutex<Option<FakeHandle>>> = Arc::default();
    let dir: Arc<Mutex<Option<PathBuf>>> = Arc::default();
    let (f, d) = (fake.clone(), dir.clone());
    let setup = DebugSetup {
        connect: Some(Arc::new(move || {
            let root = d.lock().unwrap().clone().expect("the solution folder");
            let mut p = program(&root);
            tweak(&mut p);
            let (conn, handle) = fake::connect(p);
            *f.lock().unwrap() = Some(handle);
            Ok(conn)
        })),
        search: eludite_dap::discovery::AdapterSearch::default(),
        store_dir: Some(store.clone()),
        dotnet: dotnet.to_owned(),
    };
    let w = setup_debug(cx, |_| {}, None, Some(setup));
    *dir.lock().unwrap() = Some(w.dir.path().to_path_buf());
    let write = |rel: &str, text: &str| {
        let p = w.dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(
        "src/App/App.csproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>",
    );
    write("src/App/Program.cs", PROGRAM_CS);
    write("src/App/Calc.cs", CALC_CS);
    write("src/App/bin/Debug/net10.0/App.dll", "");
    Dbg { w, fake, store }
}

impl Dbg {
    fn fake(&self) -> FakeHandle {
        self.fake
            .lock()
            .unwrap()
            .clone()
            .expect("a session started")
    }

    /// Run a command from the UI (the test thread is the UI thread), as a key or a window does.
    fn cmd(&mut self, command: &str, args: Value) -> Result<Value, eludite_commands::CommandError> {
        let r = self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.invoke(command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
        r
    }

    fn state(&mut self) -> Value {
        self.cmd(cmds::STATE, json!({})).unwrap()
    }

    fn mode(&self) -> Mode {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.debugger().model.mode)
    }

    fn wait_mode(&mut self, mode: Mode) {
        self.w.wait(&format!("mode {mode:?}"), |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.debugger().model.mode == mode)
        });
    }

    /// Wait for break mode with the locals loaded.
    fn wait_break(&mut self, stop: u64) {
        self.w.wait(&format!("break {stop}"), |w| {
            w.shell.read_with(&w.vcx, |s, _| {
                let m = &s.debugger().model;
                m.mode == Mode::Break && m.stop == stop && !m.locals_loading
            })
        });
    }

    fn open(&mut self, rel: &str, line: u32) -> gpui::Entity<eludite_editor::EditorView> {
        self.cmd(
            eludite_commands::workspace::FILE_OPEN,
            json!({"path": rel, "line": line}),
        )
        .unwrap();
        let path = self.w.path(rel);
        self.w.editor(&path)
    }

    fn locals(&self) -> Vec<(String, String)> {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            s.debugger()
                .windows
                .locals
                .read(cx)
                .rows()
                .iter()
                .map(|r| {
                    (
                        format!("{}{}", "  ".repeat(r.depth), r.name),
                        r.value.clone(),
                    )
                })
                .collect()
        })
    }

    fn exec(
        &self,
        view: &gpui::Entity<eludite_editor::EditorView>,
    ) -> Option<(u32, ExecutionKind)> {
        view.read_with(&self.w.vcx, |v, _| v.execution_point())
    }

    fn top_line(&mut self) -> (String, u64) {
        let s = self.state();
        (
            format!(
                "{}:{}",
                Path::new(s["frames"][0]["path"].as_str().unwrap_or_default())
                    .file_name()
                    .unwrap()
                    .to_string_lossy(),
                s["frames"][0]["line"]
            ),
            s["stop"].as_u64().unwrap(),
        )
    }

    /// Start under the fake, run once to the first breakpoint.
    fn start_and_break(&mut self) {
        self.w.vcx.simulate_keystrokes("f5");
        self.wait_mode(Mode::Running);
        self.fake().trigger();
        self.wait_break(1);
    }
}

#[gpui::test]
fn f9_and_the_margin_toggle_breakpoints_that_persist_per_solution(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    d.w.open_solution();
    let view = d.open("src/App/Program.cs", 6);
    // F9 at the caret's line.
    d.w.vcx.simulate_keystrokes("f9");
    assert!(d.w.audit().contains(&cmds::TOGGLE_BREAKPOINT.to_owned()));
    assert_eq!(
        view.read_with(&d.w.vcx, |v, _| v.breakpoint_glyphs()),
        [(5, BreakpointGlyph::Enabled)]
    );
    // A click in the margin of line 8 adds one there; clicking again removes it.
    let at = |d: &mut Dbg, row| {
        view.read_with(&d.w.vcx, |v, _| v.breakpoint_margin_point(row))
            .expect("row painted")
    };
    let p = at(&mut d, 7);
    d.w.vcx.simulate_click(p, Modifiers::none());
    let s = d.state();
    let lines: Vec<u64> = s["breakpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, [6, 8]);
    d.w.vcx.simulate_click(p, Modifiers::none());
    assert_eq!(d.state()["breakpoints"].as_array().unwrap().len(), 1);
    // Typing lines above moves it with its text.
    view.update(&mut d.w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(0);
            e.insert("// a\n");
        })
    });
    d.w.vcx.run_until_parked();
    assert_eq!(d.state()["breakpoints"][0]["line"], 7);
    // It is saved per solution, off the UI thread.
    let file =
        eludite_docking::LayoutStore::new(d.store.clone()).solution_path(&d.w.path("App.slnx"));
    d.w.wait("the breakpoints file", |_| {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("\"line\": 7"))
    });
    // A conditional, disabled one through the command the Breakpoints window runs.
    d.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"action": "set", "path": "src/App/Calc.cs", "line": 5, "condition": "a == 1",
                   "hit_condition": ">=2", "enabled": false}),
    )
    .unwrap();
    d.w.wait("the second breakpoint saved", |_| {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("a == 1") && t.contains(">=2"))
    });

    // A new window on the same solution loads them.
    let mut e = setup(&mut d.w.vcx.cx);
    // The same solution in another folder: the saved paths are moved there.
    let mut saved: super::state::Persisted =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    for b in &mut saved.breakpoints {
        let rel = Path::new(&b.path)
            .strip_prefix(d.w.dir.path())
            .unwrap()
            .to_path_buf();
        b.path = normalize_path(&e.w.dir.path().join(rel))
            .to_string_lossy()
            .into_owned();
    }
    let text = serde_json::to_string(&saved).unwrap();
    let file2 =
        eludite_docking::LayoutStore::new(e.store.clone()).solution_path(&e.w.path("App.slnx"));
    eludite_docking::persist::write_atomic(&file2, &text).unwrap();
    e.w.open_solution();
    e.w.wait("breakpoints loaded", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger().model.breakpoints.all().len() == 2
        })
    });
    let calc = e.open("src/App/Calc.cs", 1);
    assert_eq!(
        calc.read_with(&e.w.vcx, |v, _| v.breakpoint_glyphs()),
        [(4, BreakpointGlyph::Disabled)]
    );
    let s = e.state();
    assert_eq!(s["breakpoints"][0]["condition"], "a == 1");
    assert_eq!(s["breakpoints"][0]["hit_condition"], ">=2");
    assert_eq!(s["breakpoints"][0]["enabled"], false);
    // The Breakpoints window lists them; its Delete All deletes them.
    e.cmd("eludite.view.show", json!({"id": ids::BREAKPOINTS}))
        .unwrap();
    e.w.vcx.run_until_parked();
    let rows = e.w.shell.read_with(&e.w.vcx, |s, cx| {
        s.debugger().windows.breakpoints.read(cx).rows().len()
    });
    assert_eq!(rows, 2);
    e.w.click("debug-bp-delete-all");
    assert!(e.state()["breakpoints"].as_array().unwrap().is_empty());
    assert!(
        calc.read_with(&e.w.vcx, |v, _| v.breakpoint_glyphs())
            .is_empty()
    );
}

#[gpui::test]
fn f5_breaks_with_windows_populated_steps_move_the_line_and_stop_tears_down(
    cx: &mut TestAppContext,
) {
    let mut d = setup(cx);
    d.w.open_solution();
    let program = d.open("src/App/Program.cs", 6);
    d.w.vcx.simulate_keystrokes("f9");
    d.w.vcx.simulate_keystrokes("f5");
    assert!(matches!(d.mode(), Mode::Launching | Mode::Running));
    // The Debug layout: Locals, Watch and Call Stack shown, Call Stack in its own group at the bottom.
    let states = d.w.controller.all_states();
    let shown = |id: &str| {
        states
            .iter()
            .any(|s| s.id == id && s.state == eludite_commands::view::WindowState::Docked)
    };
    assert!(
        shown(ids::LOCALS)
            && shown(ids::WATCH)
            && shown(ids::CALL_STACK)
            && shown(ids::DEBUG_CONSOLE)
    );
    d.wait_mode(Mode::Running);
    let fake = d.fake();
    // The handshake in DAP's order, the breakpoint sent before configurationDone, with the project's program.
    assert_eq!(
        fake.commands()[..5],
        [
            "initialize",
            "launch",
            "setBreakpoints",
            "setExceptionBreakpoints",
            "configurationDone"
        ]
    );
    let launch = fake.last("launch").unwrap();
    assert!(launch["program"].as_str().unwrap().ends_with("App.dll"));
    assert_eq!(
        fake.last("setBreakpoints").unwrap()["breakpoints"][0]["line"],
        6
    );
    assert_eq!(
        fake.last("setExceptionBreakpoints").unwrap()["filters"],
        json!(["user-unhandled"])
    );
    d.w.wait("the breakpoint bound", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger()
                .model
                .breakpoints
                .all()
                .iter()
                .all(|b| b.verified)
        })
    });

    // The program runs and breaks at line 6: every window has the stop.
    fake.trigger();
    d.wait_break(1);
    let s = d.state();
    assert_eq!(s["mode"], "break");
    assert_eq!(s["stopped"]["reason"], "breakpoint");
    assert_eq!(s["frames"][0]["name"], "Program.Main()");
    assert_eq!(s["frames"][0]["line"], 6);
    assert_eq!(
        s["locals"],
        json!([{"name": "x", "value": "1", "type": "int", "reference": 0, "evaluate_name": "x"}])
    );
    assert_eq!(s["threads"].as_array().unwrap().len(), 2);
    assert_eq!(s["breakpoints"][0]["hits"], 1);
    assert_eq!(d.locals(), [("x".to_owned(), "1".to_owned())]);
    assert_eq!(d.exec(&program), Some((5, ExecutionKind::Current)));
    let (stack, threads) = d.w.shell.read_with(&d.w.vcx, |s, cx| {
        let w = &s.debugger().windows;
        (
            w.call_stack.read(cx).rows().to_vec(),
            w.threads.read(cx).rows().to_vec(),
        )
    });
    assert_eq!(stack[0].name, "Program.Main()");
    assert_eq!(stack[0].location, "Program.cs, line 6");
    assert_eq!(stack[1].name, "[Native Frames]");
    assert!(
        threads
            .iter()
            .any(|t| t.current && t.location == "Program.Main()")
    );
    let status = d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.status().get(super::DEBUG_SLOT).map(str::to_owned)
    });
    assert_eq!(
        status.as_deref(),
        Some("Debugging: App (break: breakpoint, Program.cs line 6)")
    );
    let console = d.w.shell.read_with(&d.w.vcx, |s, cx| {
        s.debugger()
            .windows
            .console
            .read(cx)
            .lines()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
    });
    assert!(console.contains(&"listening".to_owned()), "{console:?}");

    // Watch: add through the window's box; an unknown name shows the debugger's error.
    d.w.commands
        .invoke("eludite.view.show", json!({"id": ids::WATCH}))
        .unwrap();
    d.w.vcx.run_until_parked();
    d.w.click("debug-watch-input");
    d.w.vcx.simulate_keystrokes("x enter");
    d.cmd(cmds::WATCH, json!({"add": "nope"})).unwrap();
    d.w.wait("watch values", |w| {
        state_of(w)["watches"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x.get("value").is_some() || x.get("error").is_some())
    });
    let s = d.state();
    assert_eq!(
        s["watches"][0],
        json!({"expression": "x", "value": "1", "type": "int", "reference": 0})
    );
    assert!(
        s["watches"][1]["error"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    // F11 enters Calc.Add: the file opens at the statement, the stack has both frames.
    d.w.vcx.simulate_keystrokes("f11");
    d.wait_break(2);
    assert_eq!(d.top_line(), ("Calc.cs:5".to_owned(), 2));
    let calc = d.w.editor(&d.w.path("src/App/Calc.cs"));
    assert_eq!(d.exec(&calc), Some((4, ExecutionKind::Current)));
    assert_eq!(d.exec(&program), None);
    assert_eq!(
        d.locals(),
        [("a", "1"), ("b", "2"), ("sum", "0")].map(|(a, b)| (a.to_owned(), b.to_owned()))
    );
    // The Call Stack: clicking the caller selects its frame; Locals and the watch follow, the arrow turns green.
    d.w.commands
        .invoke("eludite.view.show", json!({"id": ids::CALL_STACK}))
        .unwrap();
    d.w.vcx.run_until_parked();
    d.w.click("debug-callstack-row-1");
    d.w.wait("the caller's locals", |w| {
        state_of(w)["locals"][0]["name"] == "x"
    });
    assert_eq!(d.state()["frame"], 1);
    assert_eq!(d.exec(&program), Some((5, ExecutionKind::Frame)));
    assert!(d.w.audit().contains(&cmds::SELECT_FRAME.to_owned()));
    // Shift+F11 returns to Main at line 7; F10 steps to line 8.
    d.w.vcx.simulate_keystrokes("shift-f11");
    d.wait_break(3);
    assert_eq!(d.top_line(), ("Program.cs:7".to_owned(), 3));
    assert_eq!(d.exec(&program), Some((6, ExecutionKind::Current)));
    d.w.vcx.simulate_keystrokes("f10");
    d.wait_break(4);
    assert_eq!(d.top_line(), ("Program.cs:8".to_owned(), 4));
    assert_eq!(d.exec(&program), Some((7, ExecutionKind::Current)));
    let timings =
        d.w.shell
            .read_with(&d.w.vcx, |s, _| s.debugger().timings.clone());
    assert_eq!(timings.steps.len(), 3);
    assert!(timings.start.is_some() && timings.first_break.is_some());

    // Locals expand lazily: `order`'s members are fetched when it is expanded.
    let before = d
        .fake()
        .commands()
        .iter()
        .filter(|c| *c == "variables")
        .count();
    d.w.commands
        .invoke("eludite.view.show", json!({"id": ids::LOCALS}))
        .unwrap();
    d.w.vcx.run_until_parked();
    let order_row = d.locals().iter().position(|(n, _)| n == "order").unwrap();
    d.w.click(&format!("debug-locals-toggle-{order_row}"));
    d.w.wait("order's members", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.debugger().windows.locals.read(cx).rows().len() == 5
        })
    });
    assert!(
        d.fake()
            .commands()
            .iter()
            .filter(|c| *c == "variables")
            .count()
            > before
    );
    assert_eq!(d.locals()[3], ("  Id".to_owned(), "7".to_owned()));

    // A data tip: the member chain under the mouse, evaluated in the stopped frame.
    let text = program.read_with(&d.w.vcx, |v, _| v.editor().text());
    let offset = text.find("order = new").unwrap() + 1;
    program.update(&mut d.w.vcx, |_, cx| {
        cx.emit(EditorEvent::HoverTriggered { offset })
    });
    d.w.wait("the data tip", |w| {
        program
            .read_with(&w.vcx, |v, _| v.hover().and_then(|h| h.text))
            .is_some()
    });
    assert_eq!(
        program
            .read_with(&d.w.vcx, |v, _| v.hover().and_then(|h| h.text))
            .as_deref(),
        Some("order = {Order}  (Order)")
    );
    assert_eq!(d.fake().last("evaluate").unwrap()["context"], "hover");

    // Shift+F5: the session ends, the windows and the margin forget the stop.
    d.w.vcx.simulate_keystrokes("shift-f5");
    d.wait_mode(Mode::Design);
    assert!(d.fake().commands().contains(&"disconnect".to_owned()));
    let s = d.state();
    assert!(
        s["frames"].as_array().unwrap().is_empty() && s["locals"].as_array().unwrap().is_empty()
    );
    assert_eq!(d.exec(&program), None);
    assert!(d.locals().is_empty());
    assert!(
        d.state()["console"]["tail"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().contains("has exited with code 0"))
    );
    assert_eq!(
        program.read_with(&d.w.vcx, |v, _| v.breakpoint_glyphs()),
        [(5, BreakpointGlyph::Enabled)]
    );
}

#[gpui::test]
fn stale_and_concurrent_commands_are_refused_not_queued(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    d.w.open_solution();
    d.open("src/App/Program.cs", 6);
    d.w.vcx.simulate_keystrokes("f9");
    // Nothing to step in design mode.
    let e = d.cmd(cmds::STEP_OVER, json!({})).unwrap_err().to_string();
    assert!(
        e.contains("not in break mode") && e.contains("design"),
        "{e}"
    );
    d.start_and_break();
    // Starting again is refused while a session runs.
    let e = d.cmd(cmds::START, json!({})).unwrap_err().to_string();
    assert!(e.contains("already break"), "{e}");
    // Two steps issued back to back (two drivers): the first runs the debuggee, the second is refused at once.
    let (first, second) = d.w.shell.update_in(&mut d.w.vcx, |s, window, cx| {
        (
            s.invoke(cmds::STEP_OVER, json!({}), window, cx),
            s.invoke(cmds::STEP_INTO, json!({"stop": 1}), window, cx),
        )
    });
    assert_eq!(first.unwrap()["mode"], "running");
    let e = second.unwrap_err().to_string();
    assert!(
        e.contains("stale:") && e.contains("step into") && e.contains("running"),
        "{e}"
    );
    d.wait_break(2);
    // A command quoting the stop it saw is refused once another driver moved the debuggee on.
    let e = d
        .cmd(cmds::STEP_OVER, json!({"stop": 1}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("stale:") && e.contains("now stop 2"), "{e}");
    let e = d
        .cmd(cmds::EVALUATE, json!({"expression": "x", "stop": 1}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("stale"), "{e}");
    // Only one step reached the adapter.
    assert_eq!(
        d.fake()
            .commands()
            .iter()
            .filter(|c| *c == "next" || *c == "stepIn")
            .count(),
        1
    );
    // F5 in break mode is Continue.
    d.w.vcx.simulate_keystrokes("f5");
    assert!(d.w.audit().contains(&cmds::CONTINUE.to_owned()));
    d.wait_mode(Mode::Running);
    let e = d.cmd(cmds::CONTINUE, json!({})).unwrap_err().to_string();
    assert!(e.contains("cannot continue"), "{e}");
    // Editing breakpoints is always allowed, and reaches the running session.
    d.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"path": "src/App/Calc.cs", "line": 6}),
    )
    .unwrap();
    assert!(d.fake().wait_for("setBreakpoints", 2, T));
    d.cmd(cmds::STOP, json!({})).unwrap();
    let e = d.cmd(cmds::STOP, json!({})).unwrap_err().to_string();
    assert!(
        e.contains("already stopping") || e.contains("no debugging session"),
        "{e}"
    );
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn conditions_hit_counts_run_to_cursor_and_exceptions(cx: &mut TestAppContext) {
    let mut d = setup_with(cx, |p| {
        // A handled exception at the end of the run.
        let mut throws = p.steps[5].clone();
        throws.line = 9;
        throws.throws = Some(eludite_dap::fake::FakeThrow {
            exception: "System.InvalidOperationException".into(),
            message: "boom".into(),
            handled: true,
        });
        p.steps.push(throws);
    });
    d.w.open_solution();
    let calc = "src/App/Calc.cs";
    // Calc.Add's first line breaks only on the second hit; line 6 only when sum == 4 (never).
    d.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"action": "set", "path": calc, "line": 5, "hit_condition": "2"}),
    )
    .unwrap();
    d.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"action": "set", "path": calc, "line": 6, "condition": "sum == 4"}),
    )
    .unwrap();
    d.w.vcx.simulate_keystrokes("f5");
    d.wait_mode(Mode::Running);
    let fake = d.fake();
    let sent = fake.last("setBreakpoints").unwrap();
    assert_eq!(sent["breakpoints"][1]["condition"], "sum == 4");
    // netcoredbg does not support hitCondition: the shell counts.
    assert!(sent["breakpoints"][0].get("hitCondition").is_none());
    // First run: the hit is counted and the stop never shown.
    fake.trigger();
    d.w.wait("the first hit", |w| {
        state_of(w)["breakpoints"][0]["hits"] == 1
    });
    d.w.wait("the run resumed", |_| {
        fake.commands().iter().filter(|c| *c == "continue").count() == 1
    });
    assert_eq!(d.mode(), Mode::Running);
    assert_eq!(d.state()["stop"], 0);
    // Second run: the second hit breaks.
    fake.trigger();
    d.wait_break(1);
    assert_eq!(d.top_line(), ("Calc.cs:5".to_owned(), 1));
    assert_eq!(d.state()["breakpoints"][0]["hits"], 2);
    // Run To Cursor to Program.cs line 8: a one-shot breakpoint, gone after the break.
    let program = d.open("src/App/Program.cs", 8);
    d.w.vcx.simulate_keystrokes("ctrl-f10");
    d.wait_break(2);
    assert_eq!(d.top_line(), ("Program.cs:8".to_owned(), 2));
    assert_eq!(d.exec(&program), Some((7, ExecutionKind::Current)));
    let last = fake.last("setBreakpoints").unwrap();
    assert!(last["breakpoints"].as_array().unwrap().is_empty(), "{last}");
    assert_eq!(d.state()["breakpoints"].as_array().unwrap().len(), 2);
    // Exception Settings: break when thrown (first chance), through the window.
    d.w.commands
        .invoke("eludite.view.show", json!({"id": ids::EXCEPTION_SETTINGS}))
        .unwrap();
    d.w.vcx.run_until_parked();
    d.w.click("debug-exc-thrown");
    assert_eq!(d.state()["exceptions"]["break_when_thrown"], true);
    assert_eq!(
        fake.last("setExceptionBreakpoints").unwrap()["filters"],
        json!(["all", "user-unhandled"])
    );
    d.cmd(cmds::CONTINUE, json!({})).unwrap();
    d.wait_break(3);
    let s = d.state();
    assert_eq!(s["stopped"]["reason"], "exception");
    assert_eq!(
        s["stopped"]["exception"]["id"],
        "System.InvalidOperationException"
    );
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn an_adapter_crash_ends_the_session_and_a_stalled_one_never_blocks_the_ui(
    cx: &mut TestAppContext,
) {
    let mut d = setup(cx);
    d.w.open_solution();
    d.open("src/App/Program.cs", 6);
    d.w.vcx.simulate_keystrokes("f9");
    d.start_and_break();
    // The adapter hangs for 2 s: stepping, the bus and the windows answer meanwhile.
    d.fake().stall(Duration::from_secs(2));
    let t = Instant::now();
    d.w.vcx.simulate_keystrokes("f10");
    for _ in 0..20 {
        d.state();
        d.cmd(cmds::WATCH, json!({"add": "x"})).unwrap();
        d.w.vcx.run_until_parked();
    }
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(d.mode(), Mode::Running);
    d.wait_break(2);
    // Then it crashes: the session ends with a message, nothing is left behind.
    d.fake().crash();
    d.wait_mode(Mode::Design);
    let s = d.state();
    assert!(
        s["message"]
            .as_str()
            .unwrap()
            .contains("exited unexpectedly"),
        "{s}"
    );
    assert!(s["frames"].as_array().unwrap().is_empty());
    // A new session starts cleanly with a new generation.
    d.w.vcx.simulate_keystrokes("f5");
    d.wait_mode(Mode::Running);
    assert_eq!(d.state()["generation"], 2);
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
    // Without an adapter, F5 says how to get one.
    let mut e = super::super::tests::setup(&mut d.w.vcx.cx);
    e.open_solution();
    std::fs::write(
        e.path("src/App/App.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>",
    )
    .unwrap();
    std::fs::create_dir_all(e.path("src/App/bin/Debug/net10.0")).unwrap();
    std::fs::write(e.path("src/App/bin/Debug/net10.0/App.dll"), "").unwrap();
    e.vcx.simulate_keystrokes("f5");
    e.wait("the launch failure", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger().model.mode == Mode::Design && s.debugger().model.message.is_some()
        })
    });
    let m = e
        .shell
        .read_with(&e.vcx, |s, _| s.debugger().model.message.clone().unwrap());
    assert!(m.contains("netcoredbg was not found"), "{m}");
}

#[cfg(unix)]
#[gpui::test]
fn ctrl_f5_runs_without_the_debugger_and_shows_output(cx: &mut TestAppContext) {
    // A stand-in for `dotnet` that prints its first argument and fails.
    let bin = tempfile::tempdir().unwrap().keep();
    let script = bin.join("dotnet");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"ran $1\"\necho oops >&2\nexit 3\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut d = setup_dotnet(cx, |_| {}, &script.to_string_lossy());
    d.w.open_solution();
    d.w.vcx.simulate_keystrokes("ctrl-f5");
    assert!(d.w.audit().contains(&cmds::START.to_owned()));
    d.w.wait("the program exited", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger().model.mode == Mode::Design && s.debugger().model.console_total > 3
        })
    });
    let tail: Vec<String> = d.state()["console"]["tail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_owned())
        .collect();
    assert!(
        tail.iter()
            .any(|l| l.starts_with("ran ") && l.ends_with("App.dll")),
        "{tail:?}"
    );
    assert!(tail.contains(&"oops".to_owned()), "{tail:?}");
    assert!(
        tail.contains(&"The program has exited with code 3.".to_owned()),
        "{tail:?}"
    );
    assert!(d.fake.lock().unwrap().is_none(), "no adapter for Ctrl+F5");
    // The Debug Console was shown, not the debugger windows.
    let states = d.w.controller.all_states();
    let docked = |id: &str| {
        states
            .iter()
            .any(|s| s.id == id && s.state == eludite_commands::view::WindowState::Docked)
    };
    assert!(docked(ids::DEBUG_CONSOLE) && !docked(ids::LOCALS));
}

#[gpui::test]
fn an_agent_drives_a_session_from_the_bus_and_reads_the_same_state(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    d.w.open_solution();
    let commands = d.w.commands.clone();
    let agent = Caller::Agent {
        agent: "Test Agent".into(),
        call: eludite_commands::next_call_id(),
        tool_call: None,
    };
    let run_agent = |d: &mut Dbg, f: Box<dyn FnOnce() -> Value + Send>| -> Value {
        let a = agent.clone();
        let handle = std::thread::spawn(move || with_caller(a, f));
        let deadline = Instant::now() + T;
        while !handle.is_finished() {
            assert!(
                Instant::now() < deadline,
                "the agent's command did not finish"
            );
            d.w.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        handle.join().unwrap()
    };
    // The agent sets a breakpoint in Calc.Add and starts: it answers once the program runs.
    let c = commands.clone();
    let out = run_agent(
        &mut d,
        Box::new(move || {
            c.invoke(
                cmds::TOGGLE_BREAKPOINT,
                json!({"path": "src/App/Calc.cs", "line": 5}),
            )
            .unwrap();
            c.invoke(cmds::START, json!({})).unwrap()
        }),
    );
    assert_eq!(out["mode"], "running");
    assert_eq!(out["last_driver"], "agent:Test Agent");
    // The program is triggered (as a test run would); the agent reads the break.
    d.fake().trigger();
    d.wait_break(1);
    let c = commands.clone();
    let out = run_agent(
        &mut d,
        Box::new(move || {
            let state = c.invoke(cmds::STATE, json!({})).unwrap();
            let a = c
                .invoke(cmds::EVALUATE, json!({"expression": "a + b"}))
                .unwrap();
            let sum = c
                .invoke(cmds::EVALUATE, json!({"expression": "sum"}))
                .unwrap();
            // A step waits for the next break and answers with it.
            let stepped = c
                .invoke(cmds::STEP_OVER, json!({"stop": state["stop"]}))
                .unwrap();
            let back = c.invoke(cmds::STEP_OUT, json!({})).unwrap();
            let order = c.invoke(cmds::STEP_OVER, json!({})).unwrap();
            let expanded = c
                .invoke(
                    cmds::EVALUATE,
                    json!({"expression": "order", "expand": true}),
                )
                .unwrap();
            json!({"state": state, "a": a, "sum": sum, "stepped": stepped, "back": back, "order": order,
               "expanded": expanded})
        }),
    );
    assert_eq!(out["state"]["frames"][0]["name"], "Calc.Add(int, int)");
    assert_eq!(out["state"]["locals"][2]["name"], "sum");
    assert_eq!(out["a"]["state"], "failed");
    assert!(
        out["a"]["message"]
            .as_str()
            .unwrap()
            .contains("does not exist"),
        "{out}"
    );
    assert_eq!(out["sum"]["result"], "0");
    assert_eq!(out["sum"]["state"], "done");
    assert_eq!(out["stepped"]["mode"], "break");
    assert_eq!(out["stepped"]["frames"][0]["line"], 6);
    assert_eq!(out["stepped"]["locals"][2]["value"], "3");
    assert_eq!(out["stepped"]["stopped"]["driver"], "agent:Test Agent");
    assert_eq!(out["back"]["frames"][0]["line"], 7);
    assert_eq!(out["order"]["frames"][0]["line"], 8);
    assert_eq!(
        out["expanded"]["children"][1],
        json!({"name": "Name", "value": "\"A\"", "type": "string",
                                                       "reference": 0, "evaluate_name": "Name"})
    );
    // The user sees what the agent did: the windows show the same stop and the same locals.
    let program = d.w.editor(&d.w.path("src/App/Program.cs"));
    assert_eq!(d.exec(&program), Some((7, ExecutionKind::Current)));
    assert_eq!(d.locals()[2], ("order".to_owned(), "{Order}".to_owned()));
    // The human takes over: F10 is refused for the agent's stale stop afterwards.
    let stop = d.state()["stop"].as_u64().unwrap();
    d.w.vcx.simulate_keystrokes("f5");
    d.wait_mode(Mode::Running);
    let c = commands.clone();
    let out = run_agent(
        &mut d,
        Box::new(move || {
            let refused = c
                .invoke(cmds::STEP_OVER, json!({"stop": stop}))
                .unwrap_err()
                .to_string();
            let stopped = c.invoke(cmds::STOP, json!({})).unwrap();
            json!({"refused": refused, "stopped": stopped})
        }),
    );
    assert!(out["refused"].as_str().unwrap().contains("stale"), "{out}");
    assert_eq!(out["stopped"]["mode"], "design");
    // The audit log records the agent's commands as the agent's.
    let entries = d.w.commands.audit_log().entries();
    let by_agent = entries
        .iter()
        .filter(|e| {
            e.command.starts_with("eludite.debug.") && matches!(e.caller, Caller::Agent { .. })
        })
        .count();
    assert!(by_agent >= 10, "{by_agent}");
    // Every debug command is visible to agents.
    for id in cmds::ALL {
        assert!(d.w.commands.lookup(id).unwrap().agent_visible, "{id}");
    }
}
