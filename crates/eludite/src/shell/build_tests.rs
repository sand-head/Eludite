//! Headless GPUI tests of building (brief 0017) against the in-process fake `eludite-host`: Ctrl+Shift+B streams the
//! build log into the Output window, which follows it and pauses when scrolled up; the Build menu is disabled while
//! building; the status bar's build slot; the build's diagnostics become Error List rows with click-through,
//! deduplicated against live ones and replaced by the next build's; cancel; a concurrent start refused; an agent
//! building from another thread and waiting for the result; build on save; the configuration dropdown.

use std::path::Path;
use std::time::Duration;

use eludite_commands::build::{self as build_commands, OutputSource};
use eludite_commands::diagnostics::{DIAGNOSTICS_LIST, RowSource};
use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, point, px};
use serde_json::{Value, json};

use super::build::{BUILD_SLOT, CONFIGURATION_BUTTON, toolbar_item_selector};
use super::documents::path_to_uri;
use super::error_list::row_selector;
use super::output::{BODY, PAUSED_LABEL};
use super::tests::{T, Ws, setup};

impl Ws {
    fn build_lines(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.output()
                .read(cx)
                .pane(OutputSource::Build)
                .tail(usize::MAX)
        })
    }

    fn build_status(&self) -> String {
        self.shell.read_with(&self.vcx, |s, _| {
            s.status().get(BUILD_SLOT).unwrap_or_default().to_owned()
        })
    }

    fn menu_enabled(&self, label: &str) -> bool {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.menu()
                .read(cx)
                .is_item_enabled("Build", label)
                .expect("a Build menu item")
        })
    }

    fn following(&self) -> bool {
        self.shell
            .read_with(&self.vcx, |s, cx| s.output().read(cx).following())
    }

    fn top_line(&self) -> usize {
        self.shell
            .read_with(&self.vcx, |s, cx| s.output().read(cx).top_line())
    }

    fn building(&self) -> bool {
        self.shell
            .read_with(&self.vcx, |s, _| s.builds().is_building())
    }

    fn wheel(&mut self, dy: f32) {
        let position = self.bounds(BODY).center();
        self.vcx.simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        self.vcx.run_until_parked();
    }

    /// Wait until the fake host has a build running.
    fn wait_build_started(&mut self) -> u64 {
        let fake = self.fake.clone();
        self.wait("the host to start a build", |_| {
            fake.running_build().is_some()
        });
        fake.running_build().unwrap()
    }

    fn listed(&self, args: Value) -> Vec<Value> {
        let c = self.commands.clone();
        // diagnostics.list is a read of shared rows: any thread.
        std::thread::spawn(move || c.invoke(DIAGNOSTICS_LIST, args).unwrap())
            .join()
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }
}

fn build_error(
    file: &Path,
    project: &Path,
    line: u32,
    column: u32,
    code: &str,
    msg: &str,
) -> Value {
    json!({"severity": "error", "code": code, "message": msg, "file": file, "line": line, "column": column,
           "project": project})
}

#[gpui::test]
fn ctrl_shift_b_streams_output_follows_pauses_and_lists_errors(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    w.open_solution();
    assert!(w.menu_enabled("Build Solution"));
    assert!(!w.menu_enabled("Cancel"), "nothing to cancel yet");

    w.vcx.simulate_keystrokes("ctrl-shift-b");
    assert!(w.building());
    assert_eq!(w.build_status(), "Build started\u{2026}");
    assert!(!w.menu_enabled("Build Solution"), "disabled while building");
    assert!(!w.menu_enabled("Rebuild Solution"));
    assert!(!w.menu_enabled("Build Project"));
    assert!(w.menu_enabled("Cancel"));
    let id = w.wait_build_started();
    let start = &w.fake.received_params("eludite/build/start")[0];
    assert_eq!(start["target"], "build");
    assert_eq!(start["configuration"], "Debug");
    assert!(start.get("project").is_none(), "the whole solution");
    // The Output window came forward with the host's first line.
    w.wait("the first output line", |w| {
        w.build_lines()
            .first()
            .is_some_and(|l| l == "Build started...")
    });
    let output_state = w
        .controller
        .all_states()
        .into_iter()
        .find(|s| s.id == "output")
        .unwrap();
    assert!(output_state.active, "{output_state:?}");

    // Streamed output renders and the view follows it.
    for chunk in 0..10 {
        let text: String = (0..30)
            .map(|i| format!("  \u{1b}[32mline {}\u{1b}[0m\n", chunk * 30 + i))
            .collect();
        w.fake.build_output(&text);
    }
    w.fake.build_progress(1, 1, 0, 0);
    w.wait("300 streamed lines", |w| w.build_lines().len() == 302);
    assert_eq!(w.build_lines()[301], "  line 299", "ANSI colors stripped");
    w.wait("the progress", |w| {
        w.build_status() == "Building: 1 of 1 projects"
    });
    assert!(w.following());
    let bottom = w.top_line();
    assert!(bottom > 250, "scrolled to the end: top line {bottom}");

    // Scrolling up pauses auto-scroll; new output does not move the view.
    w.wheel(400.);
    assert!(!w.following());
    let paused_at = w.top_line();
    assert!(paused_at < bottom, "{paused_at} < {bottom}");
    w.bounds(PAUSED_LABEL);
    w.fake.build_output("one more\nand another\n");
    w.wait("two more lines", |w| w.build_lines().len() == 304);
    assert_eq!(w.top_line(), paused_at, "paused: the view stays");
    // Back at the end, it follows again.
    w.wheel(-100_000.);
    w.vcx.run_until_parked();
    assert!(w.following());
    w.fake.build_output("tail\n");
    w.wait("the tail line", |w| w.build_lines().len() == 305);
    assert!(w.top_line() > paused_at);

    // The build fails with an error in Program.cs: an Error List row from the build, with click-through.
    let program = w.path("src/App/Program.cs");
    let project = w.path("src/App/App.csproj");
    w.fake.finish_build(
        "failed",
        json!([build_error(&program, &project, 3, 17, "CS0103", "The name 'Mian' does not exist in the current context"),
               {"severity": "warning", "code": "CS0168", "message": "The variable 'e' is declared but never used",
                "file": program, "line": 2, "column": 5, "project": project}]),
    );
    w.wait("the build to finish", |w| !w.building());
    assert_eq!(w.build_status(), "Build failed: 1 error, 1 warning");
    assert!(w.menu_enabled("Build Solution"));
    assert!(!w.menu_enabled("Cancel"));
    let rows = w.error_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].code, "CS0103");
    assert_eq!((rows[0].line, rows[0].column), (3, 17));
    assert_eq!(rows[0].project.as_deref(), Some("App"));
    assert_eq!(rows[0].source, RowSource::Build);
    assert_eq!(rows[1].code, "CS0168");
    let error_list = w
        .controller
        .all_states()
        .into_iter()
        .find(|s| s.id == "error_list")
        .unwrap();
    assert!(error_list.active, "the Error List comes forward on errors");
    w.double_click(&row_selector(0));
    let view = w.editor(&program);
    w.wait("the caret at 3:17", |w| {
        let text = w.text(&view);
        let caret = w.caret(&view);
        let line_start = text[..caret].rfind('\n').map_or(0, |i| i + 1);
        text[..caret].matches('\n').count() == 2 && caret - line_start == 16
    });
    let timings = w.shell.read_with(&w.vcx, |s, _| s.builds().timings.clone());
    assert!(timings.rows_set.unwrap() >= timings.finished_received.unwrap());
    assert!(timings.first_output.unwrap() >= timings.requested.unwrap());
    let _ = id;
}

#[gpui::test]
fn build_rows_are_deduplicated_against_live_ones_and_replaced_by_the_next_build(
    cx: &mut gpui::TestAppContext,
) {
    let mut w = setup(cx);
    w.open_solution();
    let program = w.path("src/App/Program.cs");
    let project = w.path("src/App/App.csproj");
    // A live CS0103 at 3:17 (LSP 2:16).
    w.fake.publish_diagnostics(
        &path_to_uri(&program),
        1,
        json!([{"range": {"start": {"line": 2, "character": 16}, "end": {"line": 2, "character": 20}},
                "severity": 1, "code": "CS0103", "source": "csharp", "message": "The name 'Mian' does not exist"}]),
    );
    w.wait("the live row", |w| w.error_rows().len() == 1);

    w.vcx.simulate_keystrokes("ctrl-shift-b");
    w.wait_build_started();
    w.fake.finish_build(
        "failed",
        json!([build_error(&program, &project, 3, 17, "CS0103", "The name 'Mian' does not exist in the current context"),
               build_error(&program, &project, 4, 9, "CS1002", "; expected"),
               {"severity": "error", "code": "MSB3073", "message": "The command \"x\" exited with code 1.",
                "project": project}]),
    );
    w.wait("the build rows", |w| w.error_rows().len() == 3);
    let rows = w.error_rows();
    let codes: Vec<_> = rows.iter().map(|r| (r.code.as_str(), r.source)).collect();
    assert!(codes.contains(&("CS0103", RowSource::Both)), "{codes:?}");
    assert!(codes.contains(&("CS1002", RowSource::Build)), "{codes:?}");
    assert!(codes.contains(&("MSB3073", RowSource::Build)), "{codes:?}");
    // A diagnostic without a file points at its project file.
    let msb = rows.iter().find(|r| r.code == "MSB3073").unwrap();
    assert_eq!(msb.path, project);
    assert_eq!(msb.line, 1);

    // diagnostics.list carries the source and filters on it.
    let all = w.listed(json!({}));
    let sources: Vec<_> = all
        .iter()
        .map(|d| (d["code"].clone(), d["source"].clone()))
        .collect();
    assert!(
        sources.contains(&(json!("CS0103"), json!("both"))),
        "{sources:?}"
    );
    assert_eq!(w.listed(json!({"source": "build"})).len(), 3);
    let live = w.listed(json!({"source": "live"}));
    assert_eq!(live.len(), 1);
    assert_eq!(live[0]["code"], "CS0103");
    let build_errors = w.listed(json!({"source": "build", "severity": "error"}));
    let cs1002 = build_errors.iter().find(|d| d["code"] == "CS1002").unwrap();
    assert_eq!(cs1002["path"], "src/App/Program.cs");
    assert_eq!(cs1002["project"], "App");
    let msb = build_errors
        .iter()
        .find(|d| d["code"] == "MSB3073")
        .unwrap();
    assert_eq!(msb["path"], "src/App/App.csproj");

    // The next build replaces the build rows; the live one stays and is live only again.
    w.vcx.simulate_keystrokes("ctrl-shift-b");
    w.wait_build_started();
    w.fake.finish_build("succeeded", json!([]));
    w.wait("the build rows replaced", |w| w.error_rows().len() == 1);
    let rows = w.error_rows();
    assert_eq!(rows[0].code, "CS0103");
    assert_eq!(rows[0].source, RowSource::Live);
    assert_eq!(w.build_status(), "Build succeeded");
}

#[gpui::test]
fn cancel_from_the_menu_and_a_concurrent_start_is_refused(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    w.open_solution();
    // Build > Rebuild Solution from the menu.
    w.click("menu-Build");
    w.click("menu-item-Build-Rebuild Solution");
    w.wait_build_started();
    assert_eq!(
        w.fake.received_params("eludite/build/start")[0]["target"],
        "rebuild"
    );
    assert_eq!(w.build_status(), "Rebuild All started\u{2026}");

    // A second start while building is refused in the shell; the host never sees it.
    w.vcx.simulate_keystrokes("ctrl-shift-b");
    w.vcx.run_until_parked();
    let state = w.shell.read_with(&w.vcx, |s, _| {
        s.status()
            .get(eludite_ui::slots::STATE)
            .unwrap_or_default()
            .to_owned()
    });
    assert!(state.contains("a build is already running"), "{state}");
    assert_eq!(w.fake.received_params("eludite/build/start").len(), 1);

    // Build > Cancel: the host's canceled result ends it.
    w.click("menu-Build");
    w.click("menu-item-Build-Cancel");
    w.wait("the build to end canceled", |w| !w.building());
    assert_eq!(w.fake.received_params("eludite/build/cancel").len(), 1);
    assert_eq!(w.build_status(), "Rebuild All canceled");
    assert!(w.build_lines().contains(&"Build canceled.".to_owned()));
    assert!(w.menu_enabled("Rebuild Solution"));
}

#[gpui::test]
fn an_agent_builds_from_another_thread_and_waits_for_the_result(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    w.open_solution();
    let program = w.path("src/App/Program.cs");
    let project = w.path("src/App/App.csproj");

    let c = w.commands.clone();
    let agent = std::thread::spawn(move || {
        c.invoke(
            build_commands::SOLUTION,
            json!({"configuration": "Release"}),
        )
    });
    w.wait_build_started();
    assert_eq!(
        w.fake.received_params("eludite/build/start")[0]["configuration"],
        "Release"
    );
    w.fake.build_output("  App -> /x/App.dll\n");
    std::thread::sleep(Duration::from_millis(50));
    w.vcx.run_until_parked();
    assert!(!agent.is_finished(), "the agent waits for the build");
    // The agent reads the log while it waits.
    let c = w.commands.clone();
    let show =
        std::thread::spawn(move || c.invoke(build_commands::OUTPUT_SHOW, json!({"tail": 5})));
    w.wait("the output read", |_| show.is_finished());
    let shown = show.join().unwrap().unwrap();
    assert_eq!(shown["source"], "build");
    assert_eq!(
        shown["tail"].as_array().unwrap().last().unwrap(),
        "  App -> /x/App.dll"
    );

    w.fake.finish_build(
        "failed",
        json!([build_error(
            &program,
            &project,
            3,
            17,
            "CS0103",
            "The name 'Mian' does not exist"
        )]),
    );
    w.wait("the agent's result", |_| agent.is_finished());
    let out = agent.join().unwrap().unwrap();
    assert_eq!(out["state"], "failed");
    assert_eq!(out["configuration"], "Release");
    assert_eq!(out["errors"], 1);
    assert_eq!(out["toolchain"], "dotnet");
    assert_eq!(out["projects"][0]["name"], "App");
    assert_eq!(out["diagnostics"][0]["path"], "src/App/Program.cs");
    assert_eq!(out["diagnostics"][0]["project"], "App");
    assert_eq!(out["diagnostics"][0]["line"], 3);
    // The selection followed the explicit configuration (the toolbar shows it).
    assert_eq!(
        w.shell
            .read_with(&w.vcx, |s, _| s.builds().configuration.clone()),
        "Release"
    );

    // `wait: false` returns as soon as the build starts; another agent call cancels it, and a waiting call ends
    // canceled.
    let c = w.commands.clone();
    let started =
        std::thread::spawn(move || c.invoke(build_commands::CLEAN, json!({"wait": false})));
    w.wait("the clean to start", |_| started.is_finished());
    assert_eq!(started.join().unwrap().unwrap()["state"], "running");
    w.wait_build_started();
    w.fake.finish_build("succeeded", json!([]));
    w.wait("the clean to finish", |w| !w.building());

    let c = w.commands.clone();
    let waiting = std::thread::spawn(move || c.invoke(build_commands::SOLUTION, json!({})));
    w.wait_build_started();
    let c = w.commands.clone();
    let cancel = std::thread::spawn(move || c.invoke(build_commands::CANCEL, json!({})));
    w.wait("the agent's build to end", |_| waiting.is_finished());
    assert_eq!(cancel.join().unwrap().unwrap()["canceled"], true);
    assert_eq!(waiting.join().unwrap().unwrap()["state"], "canceled");
    // The audit log records the agent's calls.
    let audit = w.audit();
    for c in [
        build_commands::SOLUTION,
        build_commands::CANCEL,
        build_commands::OUTPUT_SHOW,
    ] {
        assert!(audit.iter().any(|a| a == c), "{c} in {audit:?}");
    }
}

#[gpui::test]
fn build_on_save_is_off_by_default_and_builds_the_project_when_on(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    assert!(w.text(&view).starts_with("class"));
    let _ = program;
    w.vcx.simulate_input(" ");
    w.vcx.simulate_keystrokes("ctrl-s");
    w.vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(50));
    w.vcx.run_until_parked();
    assert!(w.fake.received_params("eludite/build/start").is_empty());

    w.shell
        .update(&mut w.vcx, |s, _| s.builds.build_on_save = true);
    w.vcx.simulate_input(" ");
    w.vcx.simulate_keystrokes("ctrl-s");
    w.wait_build_started();
    let start = &w.fake.received_params("eludite/build/start")[0];
    assert_eq!(
        Path::new(start["project"].as_str().unwrap()),
        w.path("src/App/App.csproj")
    );
    w.fake.finish_build("succeeded", json!([]));
    w.wait("the build to finish", |w| !w.building());
}

#[gpui::test]
fn the_configuration_dropdown_picks_release_and_the_host_log_has_its_own_source(
    cx: &mut gpui::TestAppContext,
) {
    let mut w = setup(cx);
    w.open_solution();
    w.click(CONFIGURATION_BUTTON);
    w.click(&toolbar_item_selector("configuration", 1));
    w.vcx.simulate_keystrokes("f6");
    w.wait_build_started();
    assert_eq!(
        w.fake.received_params("eludite/build/start")[0]["configuration"],
        "Release"
    );
    w.fake.finish_build("succeeded", json!([]));
    w.wait("the build to finish", |w| !w.building());

    // eludite-host's log lines go to the Host source; the dropdown switches to it through eludite.output.show.
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.on_session_event(
            super::session::SessionEvent::HostLog("[build] #1 succeeded".into()),
            window,
            cx,
        )
    });
    w.click(super::output::SOURCE_BUTTON);
    w.click(&super::output::source_item_selector(1));
    let (selected, host) = w.shell.read_with(&w.vcx, |s, cx| {
        let o = s.output().read(cx);
        (o.selected(), o.pane(OutputSource::Host).tail(1))
    });
    assert_eq!(selected, OutputSource::Host);
    assert_eq!(host, ["[build] #1 succeeded"]);
    // Clear All clears the selected source only.
    w.click(super::output::CLEAR_BUTTON);
    let (host, build) = w.shell.read_with(&w.vcx, |s, cx| {
        let o = s.output().read(cx);
        (
            o.pane(OutputSource::Host).len(),
            o.pane(OutputSource::Build).len(),
        )
    });
    assert_eq!(host, 0);
    assert!(build > 0);
    let _ = T;
}
