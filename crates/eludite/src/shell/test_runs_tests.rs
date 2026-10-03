//! Headless GPUI tests of brief 0035's Test Explorer: against the fake `eludite-host` (`eludite/test/*` scripted by
//! the test) the window opens with the tree, Run All streams results into its rows, the Error List and the status
//! bar, Run Failed Tests and Repeat Last Run rerun the right tests, the search box filters, Cancel ends a run, an
//! agent discovers, runs with `wait_ms` and reads the same results, stale updates are dropped and a restarted host's
//! status is replayed, and Debug Test starts a session that breaks at the test's first line (the fake adapter); with
//! the real `cargo` on the corpus package, Rust tests are listed and run with libtest's output parsed, and Debug Test
//! breaks in one under the real lldb-dap when it is installed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_commands::build::OutputSource;
use eludite_commands::diagnostics::RowSource;
use eludite_commands::test as cmds;
use eludite_commands::workspace;
use eludite_dap::fake::{self, FakeHandle, FakeProgram, FakeStep, FakeVar};
use eludite_docking::ids;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::debug::DebugSetup;
use super::debug::state::Mode;
use super::documents::normalize_path;
use super::test_runs::{Phase, RunState, TESTS_SLOT};
use super::tests::{Ws, setup_debug};
use super::tests_window::{self, RowKind};

const CALCULATOR_CS: &str = "namespace Corpus.Tests\n{\n    public class CalculatorTests\n    {\n        [Fact]\n        public void Adds()\n        {\n            Assert.Equal(5, 2 + 3);\n        }\n\n        [Fact]\n        public void Subtracts()\n        {\n            Assert.Equal(1, 2 - 3);\n        }\n    }\n}\n";

/// The Test Explorer over the fake host (and, for Debug Test, the fake adapter).
struct Tw {
    w: Ws,
    fake: Arc<Mutex<Option<FakeHandle>>>,
    /// The MTP and VSTest containers' ids.
    mtp: String,
    vstest: String,
}

/// The fake host's catalogue: an MTP container (three tests, one a data row with a trait) and a VSTest container (one
/// test), its program built, the source file on disk, and the fake adapter whose program steps through `Adds`.
fn setup(cx: &mut TestAppContext) -> Tw {
    let fake: Arc<Mutex<Option<FakeHandle>>> = Arc::default();
    let dir: Arc<Mutex<Option<PathBuf>>> = Arc::default();
    let (f, d) = (fake.clone(), dir.clone());
    let debug = DebugSetup {
        connect: Some(Arc::new(move || {
            let root = d.lock().unwrap().clone().expect("the solution folder");
            let source = normalize_path(&root.join("tests/Corpus.Tests/CalculatorTests.cs"))
                .to_string_lossy()
                .into_owned();
            let v = FakeVar::new;
            let program = FakeProgram {
                steps: vec![
                    FakeStep::new(
                        &source,
                        8,
                        "Corpus.Tests.CalculatorTests.Adds()",
                        0,
                        vec![v("this", "{CalculatorTests}", "CalculatorTests")],
                    ),
                    FakeStep::new(
                        &source,
                        14,
                        "Corpus.Tests.CalculatorTests.Subtracts()",
                        0,
                        vec![],
                    ),
                ],
                run_at_start: true,
                exit_at_end: Some(0),
                extra_capabilities: json!({"supportsFunctionBreakpoints": true}),
                ..FakeProgram::default()
            };
            let (conn, handle) = fake::connect(program);
            *f.lock().unwrap() = Some(handle);
            Ok(conn)
        })),
        search: eludite_dap::discovery::AdapterSearch::default(),
        mono: eludite_dap::discovery::MonoSearch::default(),
        mono_adapter: eludite_dap::discovery::MonoAdapterSearch::default(),
        platform: eludite_dap::launch::Platform::current(),
        store_dir: Some(tempfile::tempdir().unwrap().keep()),
        dotnet: "dotnet".into(),
    };
    let w = setup_debug(cx, |_| {}, None, Some(debug));
    let root = w.dir.path().to_path_buf();
    *dir.lock().unwrap() = Some(root.clone());
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    };
    let project = write(
        "tests/Corpus.Tests/Corpus.Tests.csproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>",
    );
    let source = write("tests/Corpus.Tests/CalculatorTests.cs", CALCULATOR_CS);
    let program = write("tests/Corpus.Tests/bin/Debug/net10.0/Corpus.Tests.dll", "");
    let legacy = write(
        "tests/Legacy.Tests/Legacy.Tests.csproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\" />",
    );
    let mtp = format!("{project}|net10.0");
    let vstest = format!("{legacy}|net10.0");
    let item = |id: &str, method: &str, line: u32| {
        json!({"id": id, "displayName": format!("Corpus.Tests.CalculatorTests.{method}"),
               "fullyQualifiedName": format!("Corpus.Tests.CalculatorTests.{method}"),
               "namespace": "Corpus.Tests", "className": "CalculatorTests", "method": method,
               "source": source, "line": line})
    };
    w.fake.set_tests(json!([
        {"container": {"id": mtp, "name": "Corpus.Tests", "project": project, "targetFramework": "net10.0",
                       "protocol": "mtp", "runtime": "dotnet", "program": program},
         "tests": [
            item("u-adds", "Adds", 5),
            item("u-subtracts", "Subtracts", 11),
            {"id": "u-row", "displayName": "Corpus.Tests.CalculatorTests.AddsPairs(a: 1)",
             "fullyQualifiedName": "Corpus.Tests.CalculatorTests.AddsPairs", "namespace": "Corpus.Tests",
             "className": "CalculatorTests", "method": "AddsPairs", "traits": [{"name": "Category", "value": "Math"}]}
         ]},
        {"container": {"id": vstest, "name": "Legacy.Tests", "project": legacy, "targetFramework": "net10.0",
                       "protocol": "vstest", "runtime": "dotnet"},
         "tests": [{"id": "v-greets", "displayName": "Legacy.Tests.GreeterTests.Greets",
                    "fullyQualifiedName": "Legacy.Tests.GreeterTests.Greets", "namespace": "Legacy.Tests",
                    "className": "GreeterTests", "method": "Greets"}]}
    ]));
    w.fake.set_test_outcomes(json!({
        "u-subtracts": {"outcome": "failed", "durationMs": 3.5, "message": "Assert.Equal() Failure: Values differ\nExpected: 1\nActual:   -1",
                        "stackTrace": format!("   at Xunit.Assert.Equal() in /_/src/Assert.cs:line 40\n   at Corpus.Tests.CalculatorTests.Subtracts() in {source}:line 14")},
        "u-row": {"outcome": "skipped", "message": "Rows come later"},
        "v-greets": {"outcome": "passed", "output": "Hello from VSTest\n"}
    }));
    let mut t = Tw {
        w,
        fake,
        mtp,
        vstest,
    };
    // Debug Test launches what is built (build before run has its own tests, brief 0020).
    t.w.commands
        .invoke(
            eludite_commands::settings::SET,
            json!({"key": "build.beforeRun", "value": false}),
        )
        .unwrap();
    t.w.wait("build before run off", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| !s.builds().build_before_run)
    });
    t
}

impl Tw {
    /// Run a command from the UI (the test thread is the UI thread), as a key, a menu or the window does.
    fn cmd(&mut self, command: &str, args: Value) -> Result<Value, eludite_commands::CommandError> {
        let r = self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.invoke(command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
        r
    }

    /// Run a command from another thread (an agent) while the UI runs.
    fn agent(
        &mut self,
        command: &'static str,
        args: Value,
    ) -> Result<Value, eludite_commands::CommandError> {
        let c = self.w.commands.clone();
        let h = std::thread::spawn(move || c.invoke(command, args));
        self.w.wait(command, |_| h.is_finished());
        h.join().unwrap()
    }

    fn rows(&self) -> Vec<String> {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            s.tests_window()
                .read(cx)
                .rows()
                .iter()
                .map(|r| format!("{}{} {:?}", "  ".repeat(r.depth), r.label, r.glyph))
                .collect()
        })
    }

    fn wait_phase(&mut self, phase: Phase) {
        self.w.wait(&format!("discovery {phase:?}"), |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.test_runs().phase == phase)
        });
    }

    fn wait_run_done(&mut self, run: u64) -> RunState {
        self.w.wait(&format!("run {run} done"), |w| {
            w.shell.read_with(&w.vcx, |s, _| {
                s.test_runs().run(run).is_some_and(|r| r.state.done())
            })
        });
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.test_runs().run(run).unwrap().state)
    }

    fn status(&self) -> String {
        self.w.shell.read_with(&self.w.vcx, |s, _| {
            s.status().get(TESTS_SLOT).unwrap_or_default().to_owned()
        })
    }

    fn tests_output(&self) -> Vec<String> {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            s.output()
                .read(cx)
                .pane(OutputSource::Tests)
                .tail(usize::MAX)
        })
    }

    fn run_params(&self) -> Vec<Value> {
        self.w.fake.received_params("eludite/test/run")
    }
}

#[gpui::test]
fn the_window_opens_with_the_tree_and_run_all_streams_results_into_rows_the_error_list_and_the_status_bar(
    cx: &mut TestAppContext,
) {
    let mut t = setup(cx);
    t.w.open_solution();
    // Test > Test Explorer (Ctrl+E, T): the window shows and discovers the outputs as they are (no build).
    t.w.vcx.simulate_keystrokes("ctrl-e t");
    t.wait_phase(Phase::Ready);
    assert!(t.w.audit().contains(&cmds::EXPLORER.to_owned()));
    assert!(t.w.fake.received_params("eludite/build/start").is_empty());
    assert_eq!(
        t.w.controller
            .all_states()
            .iter()
            .find(|s| s.id == ids::TEST_EXPLORER)
            .map(|s| s.state),
        Some(eludite_commands::view::WindowState::Docked)
    );
    assert_eq!(
        t.rows(),
        [
            "Corpus.Tests NotRun",
            "  Corpus.Tests NotRun",
            "    CalculatorTests NotRun",
            "      Adds NotRun",
            "      AddsPairs(a: 1) NotRun",
            "      Subtracts NotRun",
            "Legacy.Tests NotRun",
            "  Legacy.Tests NotRun",
            "    GreeterTests NotRun",
            "      Greets NotRun"
        ]
    );
    assert!(
        t.tests_output()
            .iter()
            .any(|l| l.contains("Discovering Corpus.Tests"))
    );

    // Run All (the window's button): one host run of both containers, every test.
    t.w.click(tests_window::RUN_ALL);
    let run =
        t.w.shell
            .read_with(&t.w.vcx, |s, _| s.test_runs().last_run().unwrap().id);
    assert_eq!(t.wait_run_done(run), RunState::Failed);
    let params = t.run_params();
    assert_eq!(params.len(), 1);
    let containers = params[0]["containers"].as_array().unwrap();
    assert_eq!(containers.len(), 2);
    assert!(
        containers.iter().all(|c| c["tests"].is_null()),
        "{params:?}"
    );
    assert_eq!(
        t.rows(),
        [
            "Corpus.Tests Failed",
            "  Corpus.Tests Failed",
            "    CalculatorTests Failed",
            "      Adds Passed",
            "      AddsPairs(a: 1) Skipped",
            "      Subtracts Failed",
            "Legacy.Tests Passed",
            "  Legacy.Tests Passed",
            "    GreeterTests Passed",
            "      Greets Passed"
        ]
    );
    assert!(
        t.status()
            .starts_with("Tests: 2 passed, 1 failed, 1 skipped ("),
        "{}",
        t.status()
    );
    // The failure is an Error List row at its first stack frame in the project, with click-through.
    let rows = t.w.error_rows();
    let row = rows
        .iter()
        .find(|r| r.source == RowSource::Test)
        .expect("a test row");
    assert_eq!(row.line, 14);
    assert_eq!(row.file, "CalculatorTests.cs");
    assert!(row.message.starts_with(
        "Test failed: Corpus.Tests.CalculatorTests.Subtracts: Assert.Equal() Failure"
    ));
    assert_eq!(row.project.as_deref(), Some("Corpus.Tests"));
    let listed = t
        .agent(
            eludite_commands::diagnostics::DIAGNOSTICS_LIST,
            json!({"source": "test"}),
        )
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    // The detail pane shows the selected test's failure.
    let subtracts = t.w.shell.read_with(&t.w.vcx, |s, cx| {
        s.tests_window()
            .read(cx)
            .rows()
            .iter()
            .position(|r| r.label == "Subtracts")
            .unwrap()
    });
    t.w.click(&tests_window::row_selector(subtracts));
    let details =
        t.w.shell
            .read_with(&t.w.vcx, |s, cx| s.tests_window().read(cx).details());
    assert_eq!(details[0], "Corpus.Tests.CalculatorTests.Subtracts");
    assert_eq!(details[1], "Failed (4 ms)");
    assert!(details.iter().any(|l| l.contains("Expected: 1")));
    // A double-click opens the test's source at its line.
    t.w.double_click(&tests_window::row_selector(subtracts));
    let source = t.w.path("tests/Corpus.Tests/CalculatorTests.cs");
    t.w.editor(&normalize_path(&source));

    // Run Failed Tests: the one failure, by id.
    let out = t.cmd(cmds::RUN, json!({"failed_only": true})).unwrap();
    let again = out["run"].as_u64().unwrap();
    t.wait_run_done(again);
    let last = t.run_params().pop().unwrap();
    assert_eq!(
        last["containers"],
        json!([{"id": t.mtp, "tests": ["u-subtracts"]}])
    );
    // Repeat Last Run (Ctrl+R, L): the same set again.
    t.w.vcx.simulate_keystrokes("ctrl-r l");
    t.w.wait("the repeated run", |w| {
        w.fake.received_params("eludite/test/run").len() == 3
    });
    assert_eq!(
        t.run_params().pop().unwrap()["containers"],
        last["containers"]
    );
    let repeat =
        t.w.shell
            .read_with(&t.w.vcx, |s, _| s.test_runs().last_run().unwrap().id);
    t.wait_run_done(repeat);
    // The search box filters through eludite.test.explorer.
    t.cmd(cmds::EXPLORER, json!({"filter": "Outcome:Failed"}))
        .unwrap();
    assert_eq!(t.rows().last().unwrap(), "      Subtracts Failed");
    assert_eq!(t.rows().len(), 4);
    t.cmd(cmds::EXPLORER, json!({"filter": "Trait:Category=Math"}))
        .unwrap();
    assert_eq!(t.rows().last().unwrap(), "      AddsPairs(a: 1) Skipped");
    t.cmd(cmds::EXPLORER, json!({"filter": ""})).unwrap();
    assert_eq!(t.rows().len(), 10);
    // The Output window's Tests source has the runs.
    assert!(
        t.tests_output()
            .iter()
            .any(|l| l.contains("========== Tests: 2 passed, 1 failed"))
    );
    let timings =
        t.w.shell
            .read_with(&t.w.vcx, |s, _| s.test_runs().timings.clone());
    let shown = timings.first_result_shown.unwrap() - timings.first_result_received.unwrap();
    eprintln!(
        "timing: first result row {:.2} ms after it was received; tree built in {:.2} ms",
        shown.as_secs_f64() * 1e3,
        timings.tree_built.unwrap().as_secs_f64() * 1e3
    );
    assert!(shown < Duration::from_millis(100), "{shown:?}");
}

#[gpui::test]
fn an_agent_discovers_runs_with_wait_and_reads_the_results_the_window_shows(
    cx: &mut TestAppContext,
) {
    let mut t = setup(cx);
    t.w.open_solution();
    // Discover: builds first (never discovered under this generation), then answers the tree.
    let c = t.w.commands.clone();
    let h = std::thread::spawn(move || c.invoke(cmds::DISCOVER, json!({"max_items": 2})));
    t.w.wait("the build", |w| {
        !w.fake.received_params("eludite/build/start").is_empty()
    });
    t.w.fake.finish_build("succeeded", json!([]));
    t.w.wait("the discovery", |_| h.is_finished());
    let found = h.join().unwrap().unwrap();
    assert_eq!(found["state"], "ready");
    assert_eq!(found["counts"], json!({"projects": 2, "tests": 4}));
    assert_eq!(found["tests"].as_array().unwrap().len(), 2);
    assert_eq!(found["truncated"], true);
    assert_eq!(
        found["tests"][0]["group"],
        json!(["Corpus.Tests", "CalculatorTests"])
    );
    assert_eq!(found["projects"][0]["protocol"], "mtp");
    assert_eq!(found["projects"][1]["protocol"], "vstest");
    let next = t
        .agent(cmds::DISCOVER, json!({"cursor": found["next_cursor"]}))
        .unwrap();
    assert_eq!(next["tests"].as_array().unwrap().len(), 2);
    assert_eq!(next["tests"][1]["traits"], Value::Null);
    let pairs = &found["tests"][1];
    assert_eq!(pairs["traits"], json!(["Category=Math"]));
    // Kept: no second build or discovery.
    assert_eq!(t.w.fake.test_discoveries(), 1);

    // Run with a filter and wait: the summary and the failure's message.
    let out = t
        .agent(
            cmds::RUN,
            json!({"filter": "CalculatorTests", "wait_ms": 10000}),
        )
        .unwrap();
    assert_eq!(out["state"], "failed");
    assert_eq!(out["tests"], 3);
    assert_eq!(out["summary"]["failed"], 1);
    assert_eq!(out["summary"]["skipped"], 1);
    assert_eq!(
        out["failed"][0]["name"],
        "Corpus.Tests.CalculatorTests.Subtracts"
    );
    assert_eq!(out["failed"][0]["line"], 14);
    assert!(
        out["failed"][0]["message"]
            .as_str()
            .unwrap()
            .contains("Actual:   -1")
    );
    // Results: the same data the window shows, with output.
    let results = t
        .agent(cmds::RESULTS, json!({"outcome": "skipped"}))
        .unwrap();
    assert_eq!(results["total"], 1);
    assert_eq!(results["results"][0]["message"], "Rows come later");
    let all = t
        .agent(cmds::RESULTS, json!({"max_output_chars": 5}))
        .unwrap();
    assert_eq!(all["summary"]["total"], 3);
    let run = t
        .agent(
            cmds::RUN,
            json!({"project": "Legacy.Tests", "wait_ms": 10000}),
        )
        .unwrap();
    assert_eq!(run["state"], "passed");
    let greets = t
        .agent(cmds::RESULTS, json!({"max_output_chars": 5}))
        .unwrap();
    assert_eq!(greets["results"][0]["output"], "Hello");
    assert_eq!(greets["results"][0]["truncated"], true);
    let window_row = t.rows().last().unwrap().clone();
    assert_eq!(window_row, "      Greets Passed");
    // Bad input and an unknown project are refused.
    assert!(t.agent(cmds::RUN, json!({"ids": ["nope"]})).is_err());
    assert!(
        t.agent(cmds::RUN, json!({"project": "Nope"}))
            .is_ok_and(|v| v["state"] == "failed")
    );
    assert!(t.agent(cmds::RESULTS, json!({"run": 99})).is_err());
}

#[gpui::test]
fn cancel_ends_a_held_run_and_stale_updates_are_dropped_and_a_restarted_host_is_replayed(
    cx: &mut TestAppContext,
) {
    let mut t = setup(cx);
    t.w.open_solution();
    t.cmd(cmds::DISCOVER, json!({"rebuild": false})).unwrap();
    t.wait_phase(Phase::Ready);
    t.w.fake.set_hold_test_runs(true);
    let out = t.cmd(cmds::RUN, json!({})).unwrap();
    assert_eq!(out["state"], "running");
    let run = out["run"].as_u64().unwrap();
    t.w.wait("the host run", |w| w.fake.running_test_run().is_some());
    // A second run is refused while one goes.
    assert!(t.cmd(cmds::RUN, json!({})).is_err());
    t.w.fake
        .test_results(&t.mtp, json!([{"id": "u-adds", "outcome": "running"}]));
    t.w.wait("the running row", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.test_runs().tests.iter().any(|x| {
                x.result
                    .as_ref()
                    .is_some_and(|r| r.outcome == cmds::Outcome::Running)
            })
        })
    });
    // An update of an older generation is never shown (CLAUDE.md invariant 12).
    let generation = t.w.fake.generation();
    t.w.fake.notify(
        "eludite/test/update",
        json!({"runId": 1, "generation": generation.saturating_sub(1), "seq": 0, "kind": "output", "text": "STALE\n"}),
    );
    // The host restarts: its status replays a result sent while the shell was away.
    t.w.fake.set_test_survives_restart(true);
    t.w.fake.test_results_unsent(
        &t.mtp,
        json!([{"id": "u-adds", "outcome": "passed", "durationMs": 2.0}]),
    );
    t.w.shell.read_with(&t.w.vcx, |s, _| s.session.kill());
    t.w.wait("the replay", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.test_runs()
                .run(run)
                .and_then(|r| r.results.values().next().cloned())
                .is_some_and(|r| r.outcome == cmds::Outcome::Passed)
        })
    });
    assert!(!t.tests_output().iter().any(|l| l == "STALE"));
    // Cancel (the window's button, Test > Test Explorer shown): the host cancels, the run ends canceled with the rest not run.
    t.cmd(cmds::EXPLORER, json!({})).unwrap();
    t.w.click(tests_window::CANCEL);
    assert_eq!(t.wait_run_done(run), RunState::Canceled);
    assert!(!t.w.fake.received_params("eludite/test/cancel").is_empty());
    let results = t.agent(cmds::RESULTS, json!({"run": run})).unwrap();
    assert_eq!(results["state"], "canceled");
    assert_eq!(results["summary"]["passed"], 1);
    assert_eq!(results["summary"]["not_run"], 3);
    assert!(t.status().ends_with("(canceled)"), "{}", t.status());
    assert_eq!(
        t.agent(cmds::CANCEL, json!({})).unwrap(),
        json!({"canceled": false})
    );
}

#[gpui::test]
fn debug_test_starts_a_session_that_breaks_at_the_first_line_and_results_still_flow(
    cx: &mut TestAppContext,
) {
    let mut t = setup(cx);
    t.w.open_solution();
    t.cmd(cmds::DISCOVER, json!({"rebuild": false})).unwrap();
    t.wait_phase(Phase::Ready);
    let adds = format!("{}|u-adds", t.mtp);
    let started = Instant::now();
    let c = t.w.commands.clone();
    let id = adds.clone();
    let h = std::thread::spawn(move || {
        c.invoke(
            cmds::DEBUG,
            json!({"ids": [id], "wait_ms": 10000, "depth": 1}),
        )
    });
    t.w.wait("the debugged test", |_| h.is_finished());
    let out = h.join().unwrap().unwrap();
    let took = started.elapsed();
    eprintln!(
        "timing: Debug Test to the first stop {:.0} ms (fake host and adapter)",
        took.as_secs_f64() * 1e3
    );
    assert_eq!(out["project"], "Corpus.Tests");
    assert_eq!(out["tests"], json!([adds]));
    assert_eq!(out["breakpoint"], "Corpus.Tests.CalculatorTests.Adds");
    assert_eq!(out["summary"]["mode"], "break", "{out}");
    assert_eq!(out["summary"]["frames"]["rows"][0]["line"], 8, "{out}");
    assert_eq!(out["summary"]["stopped"]["reason"], "function breakpoint");
    // The host was asked for a debug run of the one test; the launch carried `--server --client-port`.
    let params = t.run_params().pop().unwrap();
    assert_eq!(params["debug"], true);
    assert_eq!(
        params["containers"],
        json!([{"id": t.mtp, "tests": ["u-adds"]}])
    );
    t.w.shell.read_with(&t.w.vcx, |s, _| {
        let m = &s.debugger().model;
        assert_eq!(m.mode, Mode::Break);
        // The temporary breakpoint is gone after its stop.
        assert!(m.breakpoints.function_breakpoints(false).1.is_empty());
    });
    let state = t.cmd(eludite_commands::debug::STATE, json!({})).unwrap();
    assert!(
        state["session"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "--client-port"),
        "{state}"
    );
    // Results still flow while the person steps; the run ends when the host says so.
    let run = out["run"].as_u64().unwrap();
    t.w.fake
        .test_results(&t.mtp, json!([{"id": "u-adds", "outcome": "passed"}]));
    t.cmd(eludite_commands::debug::CONTINUE, json!({})).unwrap();
    t.w.fake.finish_test_run("completed");
    assert_eq!(t.wait_run_done(run), RunState::Passed);
    // Debug of tests of two projects needs a project.
    let err = t
        .agent(cmds::DEBUG, json!({"filter": "Tests", "wait_ms": 2000}))
        .unwrap();
    assert_eq!(err["summary"]["mode"], "design", "{err}");
    assert!(
        err["summary"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("one project"),
        "{err}"
    );
    let _ = t.fake.lock().unwrap().is_some();
    let _ = &t.vstest;
}

/// The corpus Cargo package (`corpus/tests/rust`) copied beside the test solution.
fn copy_rust_corpus(to: &Path) {
    let from = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests/rust");
    for rel in ["Cargo.toml", "src/lib.rs", "tests/integration.rs"] {
        let dest = to.join(rel);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(from.join(rel), dest).unwrap();
    }
}

fn wait_long(w: &mut Ws, what: &str, mut done: impl FnMut(&mut Ws) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        w.vcx.run_until_parked();
        if done(w) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui::test]
fn cargo_tests_are_built_listed_and_run_with_libtest_parsed(cx: &mut TestAppContext) {
    let mut t = setup(cx);
    let rs = t.w.path("rs");
    copy_rust_corpus(&rs);
    t.cmd(
        workspace::WORKSPACE_OPEN_FOLDER,
        json!({"path": rs.to_string_lossy()}),
    )
    .unwrap();
    t.w.wait("the Cargo workspace", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.cargo_workspace().is_some())
    });
    // Discover: `cargo test --no-run` through the Cargo build path, then one listing per target.
    t.cmd(cmds::DISCOVER, json!({})).unwrap();
    wait_long(&mut t.w, "the Rust discovery", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.test_runs().phase == Phase::Ready)
    });
    let build = t.w.shell.read_with(&t.w.vcx, |s, cx| {
        s.output()
            .read(cx)
            .pane(OutputSource::Build)
            .tail(usize::MAX)
    });
    assert!(
        build.iter().any(|l| l.contains("test --no-run")),
        "{build:?}"
    );
    assert_eq!(
        t.rows(),
        [
            "corpus-tests NotRun",
            "  integration NotRun",
            "    adds_from_outside NotRun",
            "  tests NotRun",
            "    nested NotRun",
            "      adds_negatives NotRun",
            "    adds NotRun",
            "    divides NotRun",
            "    subtracts NotRun",
            "    waits NotRun",
            "    writes_output NotRun"
        ]
    );
    let found = t
        .agent(cmds::DISCOVER, json!({"project": "corpus-tests"}))
        .unwrap();
    assert_eq!(found["projects"][0]["protocol"], "cargo");
    let adds = found["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["full_name"] == "tests::adds")
        .unwrap()
        .clone();
    assert_eq!(adds["line"], 17, "{adds}");
    assert!(adds["source"].as_str().unwrap().ends_with("lib.rs"));
    let outside = found["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "adds_from_outside")
        .unwrap();
    assert_eq!(outside["full_name"], "integration::adds_from_outside");

    // Run All: one cargo per target, libtest's lines parsed.
    let started = Instant::now();
    let c = t.w.commands.clone();
    let h = std::thread::spawn(move || c.invoke(cmds::RUN, json!({"wait_ms": 240000})));
    wait_long(&mut t.w, "the Rust run", |_| h.is_finished());
    let out = h.join().unwrap().unwrap();
    eprintln!(
        "timing: Run All of the Rust corpus {:.0} ms",
        started.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(out["state"], "failed", "{out}");
    assert_eq!(out["summary"]["total"], 7);
    assert_eq!(out["summary"]["passed"], 5);
    assert_eq!(out["summary"]["failed"], 1);
    assert_eq!(out["summary"]["skipped"], 1);
    let failed = &out["failed"][0];
    assert_eq!(failed["name"], "tests::subtracts");
    assert!(
        failed["message"]
            .as_str()
            .unwrap()
            .starts_with("assertion `left == right` failed"),
        "{failed}"
    );
    assert_eq!(failed["line"], 25);
    assert!(failed["source"].as_str().unwrap().ends_with("src/lib.rs"));
    let skipped = t
        .agent(cmds::RESULTS, json!({"outcome": "skipped"}))
        .unwrap();
    assert_eq!(
        skipped["results"][0]["message"],
        "division is not written yet"
    );
    let printed = t
        .agent(cmds::RESULTS, json!({"outcome": "passed"}))
        .unwrap();
    let writes = printed["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "tests::writes_output")
        .unwrap();
    assert_eq!(writes["output"], "Hello from Rust\n");
    let row =
        t.w.error_rows()
            .into_iter()
            .find(|r| r.source == RowSource::Test)
            .unwrap();
    assert_eq!((row.file.as_str(), row.line), ("lib.rs", 25));
    // A filtered run: one exact test.
    let one = t
        .agent(
            cmds::RUN,
            json!({"filter": "tests::adds", "wait_ms": 120000}),
        )
        .unwrap();
    assert_eq!(one["state"], "passed", "{one}");
    assert_eq!(one["tests"], 1, "{one}");
    let lines = t.tests_output();
    assert!(
        lines.iter().any(|l| l.contains("--exact")
            && l.contains("tests::adds")
            && l.contains("--test-threads=1")),
        "{lines:?}"
    );

    // Cancel a slow test: cargo's process group is killed.
    let w = t.w.path("rs/src/lib.rs");
    let text = std::fs::read_to_string(&w).unwrap().replace(
        "std::env::var(\"CORPUS_SLOW_MS\").ok().and_then(|v| v.parse().ok())",
        "Some(60_000u64)",
    );
    std::fs::write(&w, text).unwrap();
    let out = t.cmd(cmds::RUN, json!({"filter": "tests::waits"})).unwrap();
    let run = out["run"].as_u64().unwrap();
    wait_long(&mut t.w, "the slow test running", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.output()
                .read(cx)
                .pane(OutputSource::Tests)
                .tail(5)
                .iter()
                .any(|l| l.contains("running 1 test"))
        })
    });
    let cancel_at = Instant::now();
    t.cmd(cmds::CANCEL, json!({})).unwrap();
    assert_eq!(t.wait_run_done(run), RunState::Canceled);
    assert!(cancel_at.elapsed() < Duration::from_secs(5));
}

#[gpui::test]
fn debug_test_of_a_rust_test_breaks_at_its_first_line_under_lldb_dap(cx: &mut TestAppContext) {
    if eludite_dap::discovery::LldbSearch::from_env()
        .find(eludite_dap::launch::Platform::current())
        .is_err()
    {
        eprintln!("skipped: lldb-dap is not installed");
        return;
    }
    // The real adapter: no fake connector.
    let debug = DebugSetup {
        connect: None,
        search: eludite_dap::discovery::AdapterSearch::default(),
        mono: eludite_dap::discovery::MonoSearch::default(),
        mono_adapter: eludite_dap::discovery::MonoAdapterSearch::default(),
        platform: eludite_dap::launch::Platform::current(),
        store_dir: Some(tempfile::tempdir().unwrap().keep()),
        dotnet: "dotnet".into(),
    };
    // The native setup is the machine's (lldb-dap from ELUDITE_LLDB_DAP or PATH, the toolchain's formatters).
    let mut w = setup_debug(cx, |_| {}, None, Some(debug));
    let rs = w.path("rs");
    copy_rust_corpus(&rs);
    let r = w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.invoke(
            workspace::WORKSPACE_OPEN_FOLDER,
            json!({"path": rs.to_string_lossy()}),
            window,
            cx,
        )
    });
    r.unwrap();
    w.wait("the Cargo workspace", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.cargo_workspace().is_some())
    });
    let c = w.commands.clone();
    let h = std::thread::spawn(move || c.invoke(cmds::DISCOVER, json!({"wait_ms": 240000})));
    wait_long(&mut w, "the Rust discovery", |_| h.is_finished());
    h.join().unwrap().unwrap();
    // Warm: the test executable is built; Debug Test to the first stop.
    let mut took = Vec::new();
    for _ in 0..2 {
        let started = Instant::now();
        let c = w.commands.clone();
        let h = std::thread::spawn(move || {
            c.invoke(
                cmds::DEBUG,
                json!({"filter": "tests::adds", "project": "corpus-tests", "wait_ms": 120000}),
            )
        });
        wait_long(&mut w, "the debugged Rust test", |_| h.is_finished());
        let out = h.join().unwrap().unwrap();
        took.push(started.elapsed());
        assert_eq!(out["breakpoint"], "corpus_tests::tests::adds", "{out}");
        assert_eq!(out["summary"]["mode"], "break", "{out}");
        let frame = &out["summary"]["frames"]["rows"][0];
        assert!(
            frame["path"]
                .as_str()
                .unwrap_or_default()
                .ends_with("lib.rs"),
            "{out}"
        );
        assert_eq!(frame["line"], 18, "the first line of `adds`: {out}");
        let stop = w.shell.update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(eludite_commands::debug::STOP, json!({}), window, cx)
        });
        stop.unwrap();
        w.wait("the session ended", |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.debugger().model.mode == Mode::Design)
        });
        let run = out["run"].as_u64().unwrap();
        wait_long(&mut w, "the debugged run ended", |w| {
            w.shell.read_with(&w.vcx, |s, _| {
                s.test_runs().run(run).is_some_and(|r| r.state.done())
            })
        });
    }
    eprintln!(
        "timing: Debug Test (Rust, lldb-dap) to the first stop: {}",
        took.iter()
            .map(|d| format!("{:.0} ms", d.as_secs_f64() * 1e3))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert!(took[1] < Duration::from_secs(10), "{took:?}");
    let _ = RowKind::Group;
}
