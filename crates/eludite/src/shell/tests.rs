//! Headless GPUI tests of opening a solution end to end (brief 0012), against the in-process fake `eludite-host`
//! from `eludite-lsp` (feature `fake`): the command bus opens the solution, Workspace shows the host's tree,
//! a double-click opens a file in an editor, edits reach the host as a debounced `didChange`, injected diagnostics
//! become squiggles and Error List rows with click-through, save, undo, close with the unsaved-changes question, an
//! agent's commands from another thread, and a host stalled for 5 s that leaves the editor responsive.
//!
//! The host, the session worker and the syntax thread are real threads, so these tests allow parking and poll with
//! a timeout, as the editor's own tests do.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::diagnostics::DIAGNOSTICS_LIST;
use eludite_commands::{CommandRegistry, builtins, view, workspace};
use eludite_docking::{DockController, DockLayout, ToolWindowRegistry};
use eludite_editor::EditorView;
use eludite_lsp::fake::FakeHost;
use eludite_ui::{Theme, bind_keymap, vs_keymap};
use gpui::{
    AppContext as _, Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    TestAppContext, VisualTestContext, px, size,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::documents::{DIAGNOSTICS_LAYER, DIDCHANGE_DEBOUNCE, normalize_path, path_to_uri};
use super::explorer::row_selector;
use super::session::HostLaunch;
use super::{SOLUTION_SLOT, Shell};

pub(super) const T: Duration = Duration::from_secs(10);

pub(super) const PROGRAM: &str = "class Program\n{\n    static void Main() { }\n}\n";

pub(super) struct Ws {
    pub shell: Entity<Shell>,
    pub vcx: VisualTestContext,
    pub commands: Arc<CommandRegistry>,
    pub controller: DockController,
    pub fake: FakeHost,
    pub dir: TempDir,
}

/// A one-project solution on disk, its tree in the fake host, and the shell.
pub(super) fn setup(cx: &mut TestAppContext) -> Ws {
    setup_with(cx, |_| {})
}

/// As [`setup`], scripting the fake host before the shell starts it.
pub(super) fn setup_with(cx: &mut TestAppContext, script: impl FnOnce(&FakeHost)) -> Ws {
    setup_full(cx, script, None)
}

/// As [`setup_with`], with the Agents window's agents (none by default: tests never search the machine for agents).
pub(super) fn setup_full(
    cx: &mut TestAppContext,
    script: impl FnOnce(&FakeHost),
    agents: Option<super::agents::AgentsSetup>,
) -> Ws {
    setup_debug(cx, script, agents, None)
}

/// As [`setup_full`], with how debugging sessions reach their adapter (by default none: tests never search the machine
/// for netcoredbg, and breakpoints do not persist).
pub(super) fn setup_debug(
    cx: &mut TestAppContext,
    script: impl FnOnce(&FakeHost),
    agents: Option<super::agents::AgentsSetup>,
    debug: Option<super::debug::DebugSetup>,
) -> Ws {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    };
    write(
        "App.slnx",
        "<Solution><Project Path=\"src/App/App.csproj\" /></Solution>",
    );
    let project = write(
        "src/App/App.csproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\" />",
    );
    let program = write("src/App/Program.cs", PROGRAM);
    let order = write("src/App/Models/Order.cs", "class Order { }\n");
    let aspx = write("src/App/Default.aspx", "<%@ Page %>\n");
    let code_behind = write("src/App/Default.aspx.cs", "partial class Default { }\n");
    let fake = FakeHost::new();
    script(&fake);
    fake.set_tree(json!([{
        "name": "App", "path": project, "kind": "sdk", "targetFrameworks": ["net10.0"],
        "files": [
            {"path": program, "itemType": "compile"},
            {"path": order, "itemType": "compile"},
            {"path": code_behind, "itemType": "compile", "dependentUpon": aspx},
            {"path": aspx, "itemType": "content"}
        ]
    }]));

    let tools = ToolWindowRegistry::vs_default();
    let controller = DockController::new(DockLayout::default_vs(&tools), tools);
    let mut commands = builtins::default_registry();
    view::register(&mut commands, Arc::new(controller.clone())).unwrap();
    let mut services = Some(super::register_workspace(
        &mut commands,
        HostLaunch::InProcess(fake.connector()),
    ));
    if let Some(s) = services.as_mut() {
        s.agents = agents.unwrap_or_else(|| super::agents::AgentsSetup {
            registry: Some(Vec::new()),
            ..super::agents::AgentsSetup::from_env()
        });
        s.debug = debug.unwrap_or_else(|| super::debug::DebugSetup {
            connect: None,
            search: eludite_dap::discovery::AdapterSearch::default(),
            store_dir: None,
            dotnet: "dotnet".into(),
        });
    }
    let commands = Arc::new(commands);
    let window = cx.update(|cx| {
        bind_keymap(cx, &vs_keymap());
        crate::app::bind_editor_keys(cx);
        cx.open_window(Default::default(), |window, cx| {
            let shell = cx.new(|cx| {
                Shell::new(
                    commands.clone(),
                    controller.clone(),
                    Theme::vs_dark(),
                    None,
                    services.take().unwrap(),
                    window,
                    cx,
                )
            });
            shell.focus_handle(cx).focus(window, cx);
            shell
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(1280.), px(800.)));
    vcx.run_until_parked();
    let shell = window.root(&mut vcx).unwrap();
    Ws {
        shell,
        vcx,
        commands,
        controller,
        fake,
        dir,
    }
}

impl Ws {
    pub(super) fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// Run the UI until `done` holds, letting the host and worker threads run in real time.
    pub(super) fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + T;
        loop {
            self.vcx.run_until_parked();
            if done(self) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(super) fn audit(&self) -> Vec<String> {
        self.commands
            .audit_log()
            .entries()
            .into_iter()
            .map(|e| e.command)
            .collect()
    }

    pub(super) fn bounds(&mut self, sel: &str) -> gpui::Bounds<gpui::Pixels> {
        let sel: &'static str = Box::leak(sel.to_owned().into_boxed_str());
        self.vcx
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
    }

    pub(super) fn click(&mut self, sel: &str) {
        let c = self.bounds(sel).center();
        self.vcx.simulate_click(c, Modifiers::none());
        self.vcx.run_until_parked();
    }

    pub(super) fn double_click(&mut self, sel: &str) {
        let position = self.bounds(sel).center();
        for click_count in [1, 2] {
            self.vcx.simulate_event(MouseDownEvent {
                position,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
                first_mouse: false,
            });
            self.vcx.simulate_event(MouseUpEvent {
                position,
                modifiers: Modifiers::none(),
                button: MouseButton::Left,
                click_count,
            });
        }
        self.vcx.run_until_parked();
    }

    pub(super) fn open_solution(&mut self) {
        let sln = self.path("App.slnx");
        let out = self
            .commands
            .invoke(
                workspace::SOLUTION_OPEN,
                json!({"path": sln.to_string_lossy()}),
            )
            .unwrap();
        assert_eq!(
            out,
            json!({"path": sln.to_string_lossy(), "state": "loading"})
        );
        self.wait("the Workspace tree", |w| {
            w.shell
                .read_with(&w.vcx, |s, cx| s.explorer().read(cx).model().is_some())
        });
    }

    pub(super) fn row_labels(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.explorer()
                .read(cx)
                .rows()
                .iter()
                .map(|r| format!("{}{}", "  ".repeat(r.depth), r.label))
                .collect()
        })
    }

    pub(super) fn editor(&mut self, path: &Path) -> Entity<EditorView> {
        let path = path.to_path_buf();
        self.wait("the editor", |w| {
            w.shell.read_with(&w.vcx, |s, _| s.editor(&path).is_some())
        });
        self.shell
            .read_with(&self.vcx, |s, _| s.editor(&path))
            .unwrap()
    }

    pub(super) fn text(&self, view: &Entity<EditorView>) -> String {
        view.read_with(&self.vcx, |v, _| v.editor().text())
    }

    pub(super) fn caret(&self, view: &Entity<EditorView>) -> usize {
        view.read_with(&self.vcx, |v, _| v.editor().primary_selection().head)
    }

    pub(super) fn error_rows(&self) -> Vec<super::error_list::ErrorRow> {
        self.shell
            .read_with(&self.vcx, |s, cx| s.error_list().read(cx).rows().to_vec())
    }

    pub(super) fn dirty(&self, id: &str) -> bool {
        self.controller.snapshot().dirty.contains(id)
    }

    /// Open the solution, expand the project and double-click Program.cs.
    pub(super) fn open_program(&mut self) -> (PathBuf, Entity<EditorView>) {
        self.open_solution();
        let project = self.path("src/App/App.csproj");
        self.click(&format!(
            "{}-toggle",
            row_selector(&project.to_string_lossy())
        ));
        let program = self.path("src/App/Program.cs");
        let row = row_selector(&format!("{}|Program.cs", project.to_string_lossy()));
        self.double_click(&row);
        let view = self.editor(&program);
        (program, view)
    }
}

fn diagnostic(line: u32, start: u32, end: u32, severity: u8, code: &str, message: &str) -> Value {
    json!({"range": {"start": {"line": line, "character": start}, "end": {"line": line, "character": end}},
           "severity": severity, "code": code, "source": "csharp", "message": message})
}

#[gpui::test]
fn open_solution_edit_diagnostics_and_error_list(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.open_solution();

    // The tree: solution expanded, project collapsed (Visual Studio's first view of a solution).
    assert_eq!(
        w.row_labels(),
        ["Solution 'App' (1 of 1 project)", "  App (net10.0)"]
    );
    let status = w.shell.read_with(&w.vcx, |s, _| {
        s.status().get(SOLUTION_SLOT).map(str::to_owned)
    });
    assert!(
        status
            .as_deref()
            .unwrap_or_default()
            .starts_with("App.slnx: 1 project loaded"),
        "{status:?}"
    );
    let project = w.path("src/App/App.csproj");
    w.click(&format!(
        "{}-toggle",
        row_selector(&project.to_string_lossy())
    ));
    let aspx_row = row_selector(&format!("{}|Default.aspx", project.to_string_lossy()));
    w.click(&format!("{aspx_row}-toggle"));
    assert_eq!(
        w.row_labels(),
        [
            "Solution 'App' (1 of 1 project)",
            "  App (net10.0)",
            "    Models",
            "    Default.aspx",
            "      Default.aspx.cs",
            "    Program.cs",
        ]
    );
    // Double-clicking a folder expands it.
    w.double_click(&row_selector(&format!(
        "{}|Models/",
        project.to_string_lossy()
    )));
    assert!(w.row_labels().contains(&"      Order.cs".to_owned()));

    // Double-click opens the file through the bus; the host gets didOpen.
    let program = w.path("src/App/Program.cs");
    let id = normalize_path(&program).to_string_lossy().into_owned();
    let uri = path_to_uri(&program);
    w.double_click(&row_selector(&format!(
        "{}|Program.cs",
        project.to_string_lossy()
    )));
    let view = w.editor(&program);
    assert_eq!(w.text(&view), PROGRAM);
    assert!(w.audit().contains(&workspace::FILE_OPEN.to_owned()));
    assert!(w.vcx.debug_bounds("doc-tab-".to_owned().leak()).is_none());
    assert_eq!(w.controller.active_document().as_deref(), Some(id.as_str()));
    let opened = w
        .fake
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .expect("didOpen");
    assert_eq!(opened.params["textDocument"]["languageId"], "csharp");
    assert_eq!(opened.params["textDocument"]["version"], 1);
    assert_eq!(opened.params["textDocument"]["text"], PROGRAM);

    // An edit marks the tab dirty at once; didChange waits for the debounce. (Typed identifier characters also
    // trigger completion, which sends the text at once so the request sees it; brief 0013. This edit is made the
    // way an agent or a paste does, which triggers nothing.)
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.insert("int y;"))
    });
    w.vcx.run_until_parked();
    assert!(w.text(&view).starts_with("int y;class Program"));
    assert!(w.dirty(&id));
    w.vcx
        .executor()
        .advance_clock(DIDCHANGE_DEBOUNCE - Duration::from_millis(1));
    w.vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        w.fake.received_params("textDocument/didChange").is_empty(),
        "no didChange before the debounce expires"
    );
    w.vcx.executor().advance_clock(Duration::from_millis(1));
    w.vcx.run_until_parked();
    let change = w
        .fake
        .wait_for("textDocument/didChange", T, |_| true)
        .expect("didChange after the debounce");
    assert_eq!(
        change.params,
        json!({"textDocument": {"uri": uri, "version": 2},
               "contentChanges": [{"range": {"start": {"line": 0, "character": 0},
                                             "end": {"line": 0, "character": 0}},
                                   "text": "int y;"}]})
    );

    // Diagnostics for version 2: squiggles and Error List rows.
    w.fake.publish_diagnostics(
        &uri,
        2,
        json!([
            diagnostic(
                0,
                6,
                11,
                1,
                "CS0116",
                "A namespace cannot directly contain members such as fields"
            ),
            diagnostic(
                2,
                16,
                20,
                2,
                "CS0028",
                "'Program.Main()' has the wrong signature to be an entry point"
            ),
            diagnostic(1, 0, 1, 4, "IDE0055", "Fix formatting")
        ]),
    );
    w.wait("the Error List rows", |w| w.error_rows().len() == 2);
    let rows = w.error_rows();
    assert_eq!(rows[0].code, "CS0116");
    assert_eq!(rows[0].project.as_deref(), Some("App"));
    assert_eq!(rows[0].file, "Program.cs");
    assert_eq!((rows[0].line, rows[0].column), (1, 7));
    assert_eq!(rows[1].code, "CS0028");
    let header = w
        .shell
        .read_with(&w.vcx, |s, cx| s.error_list().read(cx).header());
    assert!(
        header.contains("1 Error") && header.contains("1 Warning") && header.contains("0 Messages"),
        "{header}"
    );
    let squiggles = view.read_with(&w.vcx, |v, _| {
        let b = v.editor().buffer();
        v.decorations(DIAGNOSTICS_LAYER)
            .iter()
            .map(|d| {
                b.text_for_range(
                    b.offset_for_anchor(&d.range.start)..b.offset_for_anchor(&d.range.end),
                )
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(squiggles, ["class", "Main"], "hints are not drawn");

    // diagnostics.list reads the same rows (the MCP tool of brief 0005).
    let listed = w.commands.invoke(DIAGNOSTICS_LIST, json!({})).unwrap();
    assert_eq!(listed[0]["path"], "src/App/Program.cs");
    assert_eq!(listed[0]["project"], "App");
    assert_eq!(listed[0]["code"], "CS0116");
    assert_eq!(listed[0]["line"], 1);
    assert_eq!(
        w.commands
            .invoke(DIAGNOSTICS_LIST, json!({"severity": "warning"}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // A result for an older version is dropped.
    w.fake.publish_diagnostics(&uri, 1, json!([]));
    std::thread::sleep(Duration::from_millis(50));
    w.vcx.run_until_parked();
    assert_eq!(w.error_rows().len(), 2);

    // Click-through: double-click the warning row; the caret goes to line 3, column 17.
    w.vcx.simulate_keystrokes("ctrl-home");
    assert_eq!(w.caret(&view), 0);
    w.double_click(&super::error_list::row_selector(1));
    let expected = PROGRAM.find("Main").unwrap() + "int y;".len();
    assert_eq!(w.caret(&view), expected);
    let last = w
        .commands
        .audit_log()
        .entries()
        .last()
        .unwrap()
        .command
        .clone();
    assert_eq!(last, workspace::FILE_OPEN);

    // Ctrl+S saves through eludite.editor.save and tells the host.
    w.vcx.simulate_keystrokes("ctrl-s");
    assert!(!w.dirty(&id));
    assert!(
        std::fs::read_to_string(&program)
            .unwrap()
            .starts_with("int y;class")
    );
    assert!(w.audit().contains(&workspace::EDITOR_SAVE.to_owned()));
    w.fake
        .wait_for("textDocument/didSave", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .expect("didSave");

    // Ctrl+Z goes through eludite.editor.undo.
    w.vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(w.text(&view), PROGRAM);
    assert!(w.dirty(&id));
    assert_eq!(
        w.audit().last().map(String::as_str),
        Some(workspace::EDITOR_UNDO)
    );

    // Closing the dirty tab asks first; Don't Save closes it and the host gets didClose.
    w.click(&format!("doc-close-{id}"));
    assert!(w.vcx.has_pending_prompt());
    assert!(w.controller.layout().documents.get(&id).is_some());
    w.vcx.simulate_prompt_answer("Don't Save");
    w.vcx.run_until_parked();
    assert!(w.controller.layout().documents.get(&id).is_none());
    w.fake
        .wait_for("textDocument/didClose", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .expect("didClose");
    assert!(
        w.error_rows().is_empty(),
        "rows of a closed document go away"
    );
    assert!(
        std::fs::read_to_string(&program)
            .unwrap()
            .starts_with("int y;"),
        "not saved"
    );

    // Close the solution.
    let out = w
        .commands
        .invoke(workspace::SOLUTION_CLOSE, json!({}))
        .unwrap();
    assert_eq!(out["closed"], true);
    w.wait("the explorer to clear", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.explorer().read(cx).model().is_none())
    });
}

#[gpui::test]
fn ui_stays_responsive_while_the_host_stalls_for_5_s(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    let uri = path_to_uri(&program);
    w.fake
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .expect("didOpen");

    let stall = Duration::from_secs(5);
    w.fake.stall_for(stall);
    let t0 = Instant::now();
    let mut worst = Duration::ZERO;
    for i in 0..20 {
        let t = Instant::now();
        w.vcx.simulate_input("a");
        w.vcx
            .executor()
            .advance_clock(DIDCHANGE_DEBOUNCE + Duration::from_millis(5));
        w.vcx.run_until_parked();
        // The bus stays responsive too: an agent's read, and another open of the solution.
        if i % 5 == 0 {
            w.commands.invoke(DIAGNOSTICS_LIST, json!({})).unwrap();
        }
        worst = worst.max(t.elapsed());
    }
    let busy = t0.elapsed();
    assert!(w.text(&view).starts_with(&"a".repeat(20)));
    assert!(
        busy < Duration::from_secs(2),
        "20 edits took {busy:?} while the host stalled"
    );
    assert!(
        worst < Duration::from_millis(500),
        "worst edit took {worst:?}"
    );
    assert!(
        t0.elapsed() < stall - Duration::from_millis(500),
        "still inside the stall"
    );
    assert!(
        w.fake.received_params("textDocument/didChange").is_empty(),
        "the host really was stalled"
    );
    // The edits arrive once the host recovers.
    let last = w
        .fake
        .wait_for("textDocument/didChange", stall + T, |p| {
            p["textDocument"]["version"].as_i64() >= Some(21)
        })
        .expect("the last didChange after the stall");
    assert!(last.at >= t0 + stall - Duration::from_millis(100));
    eprintln!("20 edits during a 5 s host stall: total {busy:?}, worst {worst:?}");
}

#[gpui::test]
fn agents_open_save_and_find_from_another_thread(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.open_solution();
    let program = w.path("src/App/Program.cs");
    let commands = w.commands.clone();
    let agent = std::thread::spawn(move || {
        let open = commands
            .invoke(
                workspace::FILE_OPEN,
                json!({"path": "src/App/Program.cs", "line": 3, "column": 5}),
            )
            .unwrap();
        let find = commands
            .invoke(workspace::EDITOR_FIND, json!({"query": "main"}))
            .unwrap();
        let missing = commands.invoke(workspace::EDITOR_SAVE, json!({"path": "/nope.cs"}));
        (open, find, missing)
    });
    let deadline = Instant::now() + T;
    while !agent.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the agent's commands did not finish"
        );
        w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    let (open, find, missing) = agent.join().unwrap();
    assert_eq!(
        open,
        json!({"path": normalize_path(&program).to_string_lossy(), "already_open": false})
    );
    // Relative paths resolve against the solution directory; the file is opened at line 3.
    let view = w.editor(&program);
    // Find selected "Main" (case-insensitive) on line 3, column 17.
    assert_eq!(
        find,
        json!({"path": normalize_path(&program).to_string_lossy(), "found": true, "line": 3, "column": 17})
    );
    assert_eq!(
        view.read_with(&w.vcx, |v, _| v.editor().selected_text()),
        "Main"
    );
    assert!(missing.is_err());
}

#[gpui::test]
fn file_open_solution_menu_uses_the_path_prompt(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.click("menu-File");
    w.click("menu-item-File-Open Project/Solution...");
    assert!(w.vcx.did_prompt_for_paths());
    let sln = w.path("App.slnx");
    w.vcx
        .simulate_path_prompt_response(move |_| Some(vec![sln]));
    w.wait("the tree", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.explorer().read(cx).model().is_some())
    });
    assert_eq!(
        w.audit()
            .iter()
            .filter(|c| c.as_str() == workspace::SOLUTION_OPEN)
            .count(),
        1,
        "the menu item opened the dialog, the chosen path went through the bus once"
    );
    assert_eq!(w.fake.received_params("eludite/solution/tree").len(), 1);
    // Ctrl+Shift+O is the same command.
    w.vcx.simulate_keystrokes("ctrl-shift-o");
    assert!(w.vcx.did_prompt_for_paths());
    w.vcx.simulate_path_prompt_response(|_| None);
}

#[gpui::test]
fn missing_host_is_reported_not_fatal(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let tools = ToolWindowRegistry::vs_default();
    let controller = DockController::new(DockLayout::default_vs(&tools), tools);
    let mut commands = builtins::default_registry();
    view::register(&mut commands, Arc::new(controller.clone())).unwrap();
    let mut services = Some(super::register_workspace(
        &mut commands,
        HostLaunch::Missing("eludite-host not found".into()),
    ));
    let commands = Arc::new(commands);
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            cx.new(|cx| {
                Shell::new(
                    commands.clone(),
                    controller.clone(),
                    Theme::vs_dark(),
                    None,
                    services.take().unwrap(),
                    window,
                    cx,
                )
            })
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.run_until_parked();
    let shell = window.root(&mut vcx).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sln = dir.path().join("A.sln");
    std::fs::write(&sln, "").unwrap();
    assert!(
        commands
            .invoke(workspace::SOLUTION_OPEN, json!({"path": "/nope/A.sln"}))
            .is_err()
    );
    assert!(
        commands
            .invoke(
                workspace::SOLUTION_OPEN,
                json!({"path": dir.path().join("A.txt").to_string_lossy()})
            )
            .is_err()
    );
    commands
        .invoke(
            workspace::SOLUTION_OPEN,
            json!({"path": sln.to_string_lossy()}),
        )
        .unwrap();
    let deadline = Instant::now() + T;
    loop {
        vcx.run_until_parked();
        let status = shell.read_with(&vcx, |s, _| {
            s.status().get(SOLUTION_SLOT).map(str::to_owned)
        });
        if status.as_deref() == Some("eludite-host not found") {
            break;
        }
        assert!(Instant::now() < deadline, "{status:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
