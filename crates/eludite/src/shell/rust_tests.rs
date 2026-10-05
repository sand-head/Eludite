//! Headless GPUI tests of brief 0019 against the in-process fake `eludite-host` and the scripted fake generic server
//! (`eludite-lsp`'s `FakeServer` in rust-analyzer's place, through the real registration): File > Open Folder on a
//! mixed repository shows the .NET solution and the Cargo workspace side by side; a `.rs` document goes to the
//! generic server rooted at the Cargo workspace root and a `.cs` one to the host; the server's work-done progress and
//! status reach the status bar; its diagnostics become squiggles and live Error List rows; completion, Quick Info,
//! Go To Definition, rename and code actions run through the shared paths; a crash restarts it and replays the
//! documents; a Cargo build's recorded JSON becomes Error List rows deduplicated against the live ones, with
//! click-through; a real `cargo build` streams into Output; cancel kills cargo.

#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::build::{self as build_commands, OutputSource};
use eludite_commands::diagnostics::RowSource;
use eludite_commands::workspace;
use eludite_commands::workspace_tree::WORKSPACE_TREE;
use eludite_commands::{CommandRegistry, builtins, view};
use eludite_docking::{DockController, DockLayout, ToolWindowRegistry};
use eludite_editor::EditorView;
use eludite_lsp::fake::{FakeHost, FakeReply};
use eludite_lsp::fake_server::FakeServer;
use eludite_ui::{Theme, bind_keymap, vs_keymap};
use eludite_workspace::explorer::NodeKind;
use gpui::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::Shell;
use super::build::BUILD_SLOT;
use super::documents::{DIAGNOSTICS_LAYER, normalize_path, path_to_uri};
#[cfg(unix)]
use super::error_list::row_selector;
use super::session::HostLaunch;
use super::tests::T;

const MAIN: &str = "fn main() {\n    let n: u32 = \"2\";\n    println!(\"{n}\");\n}\n";
const PROGRAM: &str = "class Program\n{\n    static void Main() { }\n}\n";
const SLOT: &str = "language_server:rust-analyzer";

struct Rs {
    shell: Entity<Shell>,
    vcx: VisualTestContext,
    commands: Arc<CommandRegistry>,
    controller: DockController,
    host: FakeHost,
    ra: FakeServer,
    dir: TempDir,
}

/// A mixed repository on disk (a one-project solution and a one-member Cargo workspace), the fake host with the
/// solution's tree, and the fake generic server standing in for rust-analyzer.
fn setup(cx: &mut TestAppContext, script: impl FnOnce(&FakeServer)) -> Rs {
    cx.executor().allow_parking();
    // Not a dot-folder, as a real checkout is not (and as rust-analyzer needs).
    let dir = tempfile::Builder::new()
        .prefix("eludite-mixed-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\"]\nresolver = \"3\"\n",
    );
    write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write("app/src/main.rs", MAIN);
    write(
        "App.slnx",
        "<Solution><Project Path=\"src/App/App.csproj\" /></Solution>",
    );
    let project = write(
        "src/App/App.csproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\" />",
    );
    let program = write("src/App/Program.cs", PROGRAM);
    write("docs/README.md", "# docs\n");
    let host = FakeHost::new();
    host.set_tree(json!([{
        "name": "App", "path": project, "kind": "sdk", "targetFrameworks": ["net10.0"],
        "files": [{"path": program, "itemType": "compile"}]
    }]));
    let ra = FakeServer::new();
    script(&ra);

    let tools = ToolWindowRegistry::vs_default();
    let controller = DockController::new(DockLayout::default_vs(&tools), tools);
    let mut commands = builtins::default_registry();
    view::register(&mut commands, Arc::new(controller.clone())).unwrap();
    let mut services = super::register_workspace(
        &mut commands,
        HostLaunch::InProcess(host.connector()),
        // A user settings file, so a test can set a path (`set_cargo`) through the settings command.
        crate::settings::SettingsSetup::isolated(Some(
            dir.path().join(super::tests::USER_SETTINGS),
        )),
    );
    services.agents = super::agents::AgentsSetup {
        registry: Some(Vec::new()),
        ..super::agents::AgentsSetup::from_env()
    };
    services
        .launches
        .in_process
        .insert("rust-analyzer".into(), ra.connector());
    let mut services = Some(services);
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
    Rs {
        shell,
        vcx,
        commands,
        controller,
        host,
        ra,
        dir,
    }
}

fn diag(line: u32, start: u32, end: u32, code: &str, message: &str) -> Value {
    json!({"range": {"start": {"line": line, "character": start}, "end": {"line": line, "character": end}},
           "severity": 1, "code": code, "source": "rust-analyzer", "message": message})
}

impl Rs {
    fn path(&self, rel: &str) -> PathBuf {
        normalize_path(&self.dir.path().join(rel))
    }

    fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
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

    /// Run a command from the UI thread, as a key or menu item does.
    fn run_cmd(&mut self, command: &str, args: Value) -> Value {
        self.shell
            .update_in(&mut self.vcx, |s, window, cx| {
                s.invoke(command, args, window, cx)
            })
            .unwrap_or_else(|e| panic!("{command}: {e}"))
    }

    fn open_folder(&mut self) -> Value {
        let root = self.dir.path().to_string_lossy().into_owned();
        let out = self.run_cmd(workspace::WORKSPACE_OPEN_FOLDER, json!({ "path": root }));
        self.wait("the folder's parts", |w| {
            let labels = w.top_labels();
            labels
                .iter()
                .any(|l| l.starts_with("Cargo workspace") && l.ends_with("(1 member)"))
                && labels
                    .iter()
                    .any(|l| l == "Solution 'App' (1 of 1 project)")
        });
        out
    }

    /// The labels of the root's children.
    fn top_labels(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.explorer()
                .read(cx)
                .model()
                .map(|m| m.root.children.iter().map(|c| c.label.clone()).collect())
                .unwrap_or_default()
        })
    }

    fn open(&mut self, rel: &str) -> Entity<EditorView> {
        let path = self.path(rel);
        self.run_cmd(
            workspace::FILE_OPEN,
            json!({"path": path.to_string_lossy()}),
        );
        self.wait("the editor", |w| {
            w.shell.read_with(&w.vcx, |s, _| s.editor(&path).is_some())
        });
        self.shell
            .read_with(&self.vcx, |s, _| s.editor(&path))
            .unwrap()
    }

    fn slot(&self) -> String {
        self.shell.read_with(&self.vcx, |s, _| {
            s.status().get(SLOT).unwrap_or_default().to_owned()
        })
    }

    fn error_rows(&self) -> Vec<super::error_list::ErrorRow> {
        self.shell
            .read_with(&self.vcx, |s, cx| s.error_list().read(cx).rows().to_vec())
    }

    fn squiggles(&self, view: &Entity<EditorView>) -> Vec<String> {
        view.read_with(&self.vcx, |v, _| {
            let b = v.editor().buffer();
            v.decorations(DIAGNOSTICS_LAYER)
                .iter()
                .map(|d| {
                    b.text_for_range(
                        b.offset_for_anchor(&d.range.start)..b.offset_for_anchor(&d.range.end),
                    )
                })
                .collect()
        })
    }

    fn generation(&self) -> u64 {
        self.shell.read_with(&self.vcx, |s, _| {
            s.generic_servers()
                .values()
                .next()
                .map_or(0, |g| g.generation)
        })
    }

    fn building(&self) -> bool {
        self.shell
            .read_with(&self.vcx, |s, _| s.builds().is_building())
    }

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

    /// The cargo to run, through the settings store (`build.cargoPath`) as a person would set it: the settings pass
    /// applies the store's value and would put `cargo` back over one poked into the shell.
    fn set_cargo(&mut self, program: impl Into<std::ffi::OsString>) {
        let program = program.into();
        self.commands
            .invoke(
                eludite_commands::settings::SET,
                json!({"key": "build.cargoPath", "value": program.to_string_lossy()}),
            )
            .unwrap();
        self.wait("the cargo path", |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.builds.cargo_program == program)
        });
    }
}

#[gpui::test]
fn open_folder_shows_the_solution_and_the_cargo_workspace_side_by_side(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    let out = w.open_folder();
    let root = normalize_path(w.dir.path());
    assert_eq!(out["root"], json!(root.to_string_lossy()));
    assert_eq!(out["state"], "loading");
    assert_eq!(out["solution"], json!(w.path("App.slnx").to_string_lossy()));
    assert_eq!(
        out["cargo_manifest"],
        json!(w.path("Cargo.toml").to_string_lossy())
    );
    // The host loads the solution as File > Open > Project/Solution would.
    assert!(
        w.host
            .wait_for("eludite/solution/open", T, |p| p["path"]
                .as_str()
                .is_some_and(|s| s.ends_with("App.slnx")))
            .is_some()
    );
    w.wait("the folder listing", |w| {
        w.top_labels().contains(&"docs".to_owned())
    });
    let (labels, kinds) = w.shell.read_with(&w.vcx, |s, cx| {
        let m = s.explorer().read(cx).model().cloned().unwrap();
        (
            m.visible_rows(&m.default_expanded())
                .into_iter()
                .map(|r| format!("{}{}", "  ".repeat(r.depth), r.label))
                .collect::<Vec<_>>(),
            m.root
                .children
                .iter()
                .map(|c| c.kind.clone())
                .collect::<Vec<_>>(),
        )
    });
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(labels[0], name);
    assert_eq!(labels[1], "  Solution 'App' (1 of 1 project)");
    assert_eq!(labels[2], "    App (net10.0)");
    assert_eq!(labels[3], format!("  Cargo workspace '{name}' (1 member)"));
    assert_eq!(labels[4], "    app (bin)");
    assert!(labels.contains(&"    Cargo.toml".to_owned()), "{labels:?}");
    assert!(labels.contains(&"  docs".to_owned()), "{labels:?}");
    assert!(matches!(kinds[0], NodeKind::Solution));
    assert!(matches!(kinds[1], NodeKind::CargoWorkspace));
    // The solution's and the package's files are not listed again under the folder.
    assert!(!labels.iter().any(|l| l.trim() == "src"), "{labels:?}");

    // Agents see every project, whatever its build system.
    w.wait("the workspace tree to load", |w| {
        w.commands.invoke(WORKSPACE_TREE, json!({})).unwrap()["state"] == "loaded"
    });
    let tree = w.commands.invoke(WORKSPACE_TREE, json!({})).unwrap();
    let kinds: Vec<&str> = tree["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["csproj", "cargo"]);
    let app = &tree["projects"][1];
    assert_eq!(app["name"], "app");
    assert_eq!(app["targets"][0]["kind"], "bin");
    assert!(
        app["files"]
            .as_array()
            .unwrap()
            .contains(&json!(w.path("app/src/main.rs").to_string_lossy()))
    );
}

#[gpui::test]
fn rust_documents_go_to_the_generic_server_and_its_state_reaches_the_status_bar(
    cx: &mut TestAppContext,
) {
    let mut w = setup(cx, |ra| ra.set_quiescent_after_init(false));
    w.open_folder();
    let main = w.path("app/src/main.rs");
    let uri = path_to_uri(&main);
    let view = w.open("app/src/main.rs");

    // Launched from the registration, rooted at the Cargo workspace root, plain LSP.
    let init = w.ra.wait_for("initialize", T, |_| true).unwrap();
    assert_eq!(
        init.params["rootUri"],
        json!(path_to_uri(&normalize_path(w.dir.path())))
    );
    assert_eq!(
        init.params["initializationOptions"]["cargo"]["targetDir"],
        true
    );
    let open =
        w.ra.wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .unwrap();
    assert_eq!(open.params["textDocument"]["languageId"], "rust");
    assert_eq!(open.params["textDocument"]["text"], MAIN);
    // Progress, then ready.
    w.wait("indexing progress", |w| {
        w.slot() == "rust-analyzer: Indexing 1/2 (core) 50%"
    });
    w.ra.finish_indexing();
    w.wait("ready", |w| w.slot() == "rust-analyzer: ready (0.0.0-fake)");

    // Edits reach it as an incremental didChange; the host never hears of the Rust file.
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(MAIN.find("2\"").unwrap());
            e.insert("4");
        })
    });
    w.vcx.run_until_parked();
    w.vcx
        .executor()
        .advance_clock(super::documents::DIDCHANGE_DEBOUNCE + Duration::from_millis(5));
    let change =
        w.ra.wait_for("textDocument/didChange", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .unwrap();
    assert_eq!(change.params["contentChanges"][0]["text"], "4");
    assert!(
        w.host
            .received_params("textDocument/didOpen")
            .iter()
            .all(|p| p["textDocument"]["uri"] != uri)
    );

    // A C# file of the solution goes to the host, not to the generic server.
    let program = w.path("src/App/Program.cs");
    w.open("src/App/Program.cs");
    let cs = path_to_uri(&program);
    assert!(
        w.host
            .wait_for("textDocument/didOpen", T, |p| p["textDocument"]["uri"]
                == cs)
            .is_some()
    );
    assert!(
        w.ra.received_params("textDocument/didOpen")
            .iter()
            .all(|p| p["textDocument"]["uri"] != cs)
    );
}

#[gpui::test]
fn generic_diagnostics_become_squiggles_and_live_error_list_rows(cx: &mut TestAppContext) {
    let mut w = setup(cx, |ra| {
        ra.diagnose_on_open(
            "app/src/main.rs",
            json!([diag(1, 17, 20, "E0308", "mismatched types")]),
        );
    });
    w.open_folder();
    let view = w.open("app/src/main.rs");
    w.wait("the squiggle", |w| !w.squiggles(&view).is_empty());
    assert_eq!(w.squiggles(&view), ["\"2\""]);
    let rows = w.error_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].code, "E0308");
    assert_eq!(rows[0].project.as_deref(), Some("app"));
    assert_eq!((rows[0].line, rows[0].column), (2, 18));
    assert_eq!(rows[0].source, RowSource::Live);
    // Generic diagnostics survive the solution's generation changing (they are not the host's).
    w.wait("the solution to load", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.generation) >= 1
    });
    assert_eq!(w.error_rows().len(), 1);
    let timings = w.shell.read_with(&w.vcx, |s, _| {
        s.generic_servers().values().next().unwrap().timings.clone()
    });
    assert!(timings.first_nonempty_diagnostics.unwrap() >= timings.requested.unwrap());
}

#[gpui::test]
fn completion_quick_info_definition_rename_and_code_actions_use_the_shared_paths(
    cx: &mut TestAppContext,
) {
    let mut w = setup(cx, |ra| {
        ra.respond("textDocument/completion", |p| {
            let line = p["position"]["line"].clone();
            let character = p["position"]["character"].clone();
            FakeReply::Result(json!({"isIncomplete": false, "items": [
                {"label": "println!", "kind": 3, "sortText": "a",
                 "textEdit": {"range": {"start": {"line": line, "character": 4}, "end": {"line": line, "character": character}},
                              "newText": "println!"}},
                {"label": "panic!", "kind": 3, "sortText": "b"}]}))
        });
        ra.respond("textDocument/hover", |_| {
            FakeReply::Result(
                json!({"contents": {"kind": "markdown", "value": "```rust\nlet n: u32\n```"}}),
            )
        });
        ra.respond("textDocument/definition", |p| {
            FakeReply::Result(json!({"uri": p["textDocument"]["uri"],
                "range": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 9}}}))
        });
        ra.respond("textDocument/prepareRename", |_| {
            FakeReply::Result(
                json!({"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 9}}),
            )
        });
        ra.respond("textDocument/rename", |p| {
            let uri = p["textDocument"]["uri"].clone();
            FakeReply::Result(json!({"changes": {uri.as_str().unwrap(): [
                {"range": {"start": {"line": 1, "character": 8}, "end": {"line": 1, "character": 9}}, "newText": "count"},
                {"range": {"start": {"line": 2, "character": 15}, "end": {"line": 2, "character": 16}}, "newText": "count"}]}}))
        });
        ra.respond("textDocument/codeAction", |p| {
            let uri = p["textDocument"]["uri"].clone();
            FakeReply::Result(json!([{"title": "Add `#[allow(unused)]`", "kind": "quickfix",
                "edit": {"changes": {uri.as_str().unwrap(): [
                    {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                     "newText": "#[allow(unused)]\n"}]}}}]))
        });
    });
    w.open_folder();
    let main = w.path("app/src/main.rs");
    let path = main.to_string_lossy().into_owned();
    let view = w.open("app/src/main.rs");
    w.wait("ready", |w| w.slot().starts_with("rust-analyzer: ready"));

    // Completion through the shared popup: the server's items, from a request without Eludite's generation.
    let line3 = MAIN.find("    println").unwrap() + 4;
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(line3))
    });
    let out = w.run_cmd(
        workspace::EDITOR_COMPLETE,
        json!({"path": path, "line": 3, "column": 5}),
    );
    let _ = out;
    w.wait("the completion list", |w| {
        view.read_with(&w.vcx, |v, _| {
            v.completion()
                .is_some_and(|c| c.visible && c.items.iter().any(|i| i.0 == "println!"))
        })
    });
    let request =
        w.ra.wait_for("textDocument/completion", T, |_| true)
            .unwrap();
    assert!(request.params.get("eluditeGeneration").is_none());

    // Quick Info.
    w.run_cmd(
        workspace::EDITOR_HOVER,
        json!({"path": path, "line": 2, "column": 9}),
    );
    w.wait("Quick Info", |w| {
        view.read_with(&w.vcx, |v, _| v.hover()).is_some_and(|h| {
            h.visible && h.text.as_deref().is_some_and(|t| t.contains("let n: u32"))
        })
    });

    // Go To Definition moves the caret.
    w.run_cmd(
        workspace::EDITOR_GO_TO_DEFINITION,
        json!({"path": path, "line": 3, "column": 16}),
    );
    w.wait("the caret at the definition", |w| {
        view.read_with(&w.vcx, |v, _| v.editor().primary_selection().head)
            == MAIN.find("n: u32").unwrap()
    });

    // Rename applies the server's WorkspaceEdit with the shared applier.
    w.run_cmd(
        workspace::EDITOR_RENAME,
        json!({"path": path, "line": 2, "column": 9, "new_name": "count", "apply": true}),
    );
    w.wait("the rename", |w| {
        view.read_with(&w.vcx, |v, _| v.editor().text())
            .contains("let count: u32")
    });
    assert!(
        view.read_with(&w.vcx, |v, _| v.editor().text())
            .contains("println!(\"{count}\")")
    );

    // A code action (Ctrl+.), applied with the shared applier.
    w.run_cmd(
        workspace::EDITOR_CODE_ACTIONS,
        json!({"path": path, "line": 1, "column": 1}),
    );
    w.wait("the light bulb menu", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.code_action_menu()
                .is_some_and(|m| !m.read(cx).list().actions.is_empty())
        })
    });
    w.run_cmd(workspace::EDITOR_APPLY_CODE_ACTION, json!({"index": 0}));
    w.wait("the code action's edit", |w| {
        view.read_with(&w.vcx, |v, _| v.editor().text())
            .starts_with("#[allow(unused)]\nfn main()")
    });
    for method in [
        "textDocument/hover",
        "textDocument/definition",
        "textDocument/prepareRename",
        "textDocument/rename",
        "textDocument/codeAction",
    ] {
        assert!(!w.ra.received_params(method).is_empty(), "{method}");
    }
    // None of it went to the host.
    assert!(w.host.received_params("textDocument/rename").is_empty());
}

#[gpui::test]
fn a_crash_restarts_the_generic_server_and_replays_its_documents(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    w.open_folder();
    let main = w.path("app/src/main.rs");
    let uri = path_to_uri(&main);
    w.open("app/src/main.rs");
    w.wait("ready", |w| w.slot().starts_with("rust-analyzer: ready"));
    assert_eq!(w.generation(), 1);
    w.ra.crash();
    let opened_twice = |w: &Rs| {
        w.ra.received_params("textDocument/didOpen")
            .into_iter()
            .filter(|p| p["textDocument"]["uri"] == uri)
            .count()
            == 2
    };
    w.wait("the restart", |w| {
        w.ra.connections() == 2 && w.generation() == 2 && opened_twice(w)
    });
    let opens: Vec<Value> =
        w.ra.received_params("textDocument/didOpen")
            .into_iter()
            .filter(|p| p["textDocument"]["uri"] == uri)
            .collect();
    assert_eq!(opens.len(), 2, "replayed after the restart");
    assert_eq!(w.ra.received_params("initialize").len(), 2);
    w.wait("ready again", |w| {
        w.slot().starts_with("rust-analyzer: ready")
    });
}

/// A `cargo` that prints the recorded JSON of a build with an error at `app/src/main.rs:2:18` and fails, as cargo
/// does; with `sleep`, it runs until killed.
#[cfg(unix)]
fn fake_cargo(dir: &Path, sleep: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let recorded = include_str!("testdata/cargo-build.jsonl");
    let root = normalize_path(dir).to_string_lossy().into_owned();
    // The recorded build's error moved to this workspace's main.rs, line 2, column 18.
    let error = recorded
        .lines()
        .find(|l| l.contains("mismatched types"))
        .unwrap()
        .replace("/w/ws", &root)
        .replace(
            "\"line_start\":2,\"line_end\":2,\"column_start\":30,\"column_end\":33",
            "\"line_start\":2,\"line_end\":2,\"column_start\":18,\"column_end\":21",
        );
    let fixture = dir.join("cargo-out.jsonl");
    std::fs::write(&fixture, format!("{error}\n")).unwrap();
    let script = dir.join("fake-cargo.sh");
    let body = if sleep {
        "#!/bin/sh\necho '   Compiling app v0.1.0' >&2\nsleep 30\n".to_owned()
    } else {
        format!(
            "#!/bin/sh\necho '   Compiling app v0.1.0' >&2\ncat '{}'\necho 'error: could not compile `app`' >&2\nexit 101\n",
            fixture.display()
        )
    };
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

#[cfg(unix)]
#[gpui::test]
fn cargo_rows_from_recorded_json_dedup_with_live_rows_and_click_through(cx: &mut TestAppContext) {
    let mut w = setup(cx, |ra| {
        ra.diagnose_on_open(
            "app/src/main.rs",
            json!([diag(1, 17, 20, "E0308", "mismatched types")]),
        );
    });
    let cargo = fake_cargo(w.dir.path(), false);
    w.set_cargo(cargo);
    w.open_folder();
    let view = w.open("app/src/main.rs");
    w.wait("the live row", |w| w.error_rows().len() == 1);

    // Ctrl+Shift+B with a Rust document active builds the Cargo workspace.
    let started = w.run_cmd(build_commands::SOLUTION, json!({}));
    assert_eq!(started["system"], "cargo");
    assert_eq!(started["state"], "running");
    w.wait("the build to finish", |w| !w.building());
    let lines = w.build_lines();
    assert!(lines[0].starts_with("Build started at"), "{lines:?}");
    assert!(lines[1].contains("--message-format=json-diagnostic-rendered-ansi"));
    assert!(lines.iter().any(|l| l.contains("Compiling app")));
    assert!(lines.iter().any(|l| l.contains("mismatched types")));
    assert!(
        lines
            .iter()
            .any(|l| l == "========== Build: 0 succeeded, 1 failed ==========")
    );
    assert_eq!(w.build_status(), "Build failed: 1 error, 0 warnings");
    // The build's row is the live one: one row, from both.
    let rows = w.error_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].source, RowSource::Both);
    assert_eq!(rows[0].project.as_deref(), Some("app"));
    // Without the live row it is a build row, with click-through.
    w.ra.publish_diagnostics(&path_to_uri(&w.path("app/src/main.rs")), None, json!([]));
    w.wait("the build row alone", |w| {
        w.error_rows()
            .first()
            .is_some_and(|r| r.source == RowSource::Build)
    });
    let rows = w.error_rows();
    assert_eq!((rows[0].line, rows[0].column), (2, 18));
    // Move the caret away, then double-click the row.
    view.update(&mut w.vcx, |v, cx| v.update_editor(cx, |e| e.set_caret(0)));
    let c = w
        .vcx
        .debug_bounds(Box::leak(row_selector(0).into_boxed_str()))
        .unwrap()
        .center();
    for click_count in [1, 2] {
        w.vcx.simulate_event(gpui::MouseDownEvent {
            position: c,
            modifiers: gpui::Modifiers::none(),
            button: gpui::MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        w.vcx.simulate_event(gpui::MouseUpEvent {
            position: c,
            modifiers: gpui::Modifiers::none(),
            button: gpui::MouseButton::Left,
            click_count,
        });
    }
    w.wait("the caret at 2:18", |w| {
        view.read_with(&w.vcx, |v, _| v.editor().primary_selection().head)
            == MAIN.find("\"2\"").unwrap()
    });
    let listed = w
        .commands
        .invoke(eludite_commands::diagnostics::DIAGNOSTICS_LIST, json!({}))
        .unwrap();
    // Paths relative to the open folder.
    assert_eq!(listed[0]["path"], "app/src/main.rs");
    assert_eq!(listed[0]["source"], "build");
}

#[cfg(unix)]
#[gpui::test]
fn cancel_kills_cargo(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    let cargo = fake_cargo(w.dir.path(), true);
    w.set_cargo(cargo);
    w.open_folder();
    w.run_cmd(build_commands::SOLUTION, json!({"system": "cargo"}));
    w.wait("cargo to start", |w| {
        w.build_lines().iter().any(|l| l.contains("Compiling app"))
    });
    let canceled = Instant::now();
    let out = w.run_cmd(build_commands::CANCEL, json!({}));
    assert_eq!(out["canceled"], true, "{out} with {:?}", w.build_lines());
    w.wait("the build to end", |w| !w.building());
    assert!(canceled.elapsed() < Duration::from_secs(5));
    assert_eq!(w.build_status(), "Build canceled");
    assert!(w.build_lines().iter().any(|l| l == "Build canceled."));
}

#[gpui::test]
fn a_real_cargo_build_streams_into_output_and_both_systems_build_with_no_active_document(
    cx: &mut TestAppContext,
) {
    let mut w = setup(cx, |_| {});
    // The cargo that runs these tests.
    if let Some(cargo) = std::env::var_os("CARGO") {
        w.set_cargo(cargo);
    }
    w.open_folder();
    w.wait("the solution to load", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.generation) >= 1
    });
    // No document is active: MSBuild (in the fake host), then cargo, into one Output pane.
    let started = w.run_cmd(build_commands::SOLUTION, json!({}));
    assert_eq!(started["system"], "all");
    w.wait("the host's build", |w| w.host.running_build().is_some());
    w.host.finish_build("succeeded", json!([]));
    w.wait("cargo's build", |w| {
        w.build_lines()
            .iter()
            .any(|l| l.contains("--message-format=json-diagnostic-rendered-ansi"))
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    while w.building() {
        assert!(Instant::now() < deadline, "{:?}", w.build_lines());
        w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    let lines = w.build_lines();
    assert!(
        lines.iter().any(|l| l.contains("Build started")),
        "{lines:?}"
    );
    // The real rustc found the type error in main.rs.
    assert_eq!(
        w.build_status(),
        "Build failed: 1 error, 0 warnings",
        "{lines:?}"
    );
    let rows = w.error_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].code, "E0308");
    assert_eq!((rows[0].line, rows[0].column), (2, 18));
    let result = w
        .shell
        .read_with(&w.vcx, |s, _| s.builds().last.clone())
        .unwrap();
    assert_eq!(result.summary.projects_failed, 1);
    assert!(result.projects.iter().any(|p| p.name == "App"));
    assert!(result.projects.iter().any(|p| p.name == "app"));
    let _ = (&w.controller, OutputSource::Build);
}
