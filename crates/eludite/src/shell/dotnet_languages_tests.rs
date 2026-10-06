//! Headless GPUI tests of brief 0063, Visual Basic and F# through the shared language-server paths, against the
//! in-process fake `eludite-host` and `eludite-lsp`'s scripted `FakeServer` standing in for FsAutoComplete, through
//! the real `servers.json` registrations: a `.vb` document is the host's (Roslyn), opened with `languageId` `vb`,
//! with its diagnostics as squiggles and no generic server started; a `.fs` under a folder with an `.fsproj` starts
//! the `fsautocomplete` registration rooted at that folder (the glob root marker) with `AutomaticWorkspaceInit`,
//! opens the document as `fsharp`, and completion and a diagnostic arrive through the shared paths; an `.fsx` outside
//! any project is rooted at the solution's folder; a server that is not found says
//! `not found (run tools/fsautocomplete/fetch.sh)` while highlighting stays. The real server's test is
//! `crates/lsp/tests/real_fsautocomplete.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::build::OutputSource;
use eludite_commands::workspace;
use eludite_commands::{builtins, view};
use eludite_docking::{DockController, DockLayout, ToolWindowRegistry};
use eludite_editor::EditorView;
use eludite_lsp::fake::FakeReply;
use eludite_lsp::fake_server::FakeServer;
use eludite_ui::{Theme, bind_keymap, vs_keymap};
use gpui::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::Shell;
use super::documents::{DIAGNOSTICS_LAYER, normalize_path, path_to_uri};
use super::servers::ServerKey;
use super::session::HostLaunch;
use super::tests::T;

const MODULE_VB: &str = "Module Program\n    Sub Main()\n        Dim n As Integer = \"two\"\n        Console.WriteLine(n)\n    End Sub\nEnd Module\n";
const SHAPES_FS: &str = "module Shapes\n\nlet area (w: int) (h: int) = w * h\n\nlet total: int = \"two\"\n\nlet report () =\n    printfn \"%d\" (ar)\n";
const BUILD_FSX: &str = "#r \"nuget: Fake.Core.Target\"\n\nlet build () = printfn \"building\"\n";

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

/// FsAutoComplete's capabilities, as it advertises them.
fn fsac_capabilities() -> Value {
    json!({
        "textDocumentSync": {"openClose": true, "change": 1, "save": {"includeText": true}},
        "completionProvider": {"triggerCharacters": [".", "'"], "resolveProvider": true},
        "hoverProvider": true,
        "definitionProvider": true,
        "referencesProvider": true,
        "renameProvider": {"prepareProvider": true},
        "codeLensProvider": {"resolveProvider": true},
        "documentFormattingProvider": true
    })
}

/// A `.vb` document goes to the host, as the `roslyn` registration's second glob: `didOpen` with `languageId` `vb`,
/// the host's diagnostics as squiggles and Error List rows, and no generic server.
#[gpui::test]
fn a_vb_document_is_served_by_the_host_as_vb(cx: &mut TestAppContext) {
    let mut w = super::tests::setup_with(cx, |_| {});
    w.open_solution();
    let module = w.dir.path().join("src/Lib/Program.vb");
    std::fs::create_dir_all(module.parent().unwrap()).unwrap();
    std::fs::write(&module, MODULE_VB).unwrap();
    let module = normalize_path(&module);
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(
                workspace::FILE_OPEN,
                json!({"path": module.to_string_lossy()}),
                window,
                cx,
            )
        })
        .unwrap();
    let view = w.editor(&module);
    let uri = path_to_uri(&module);
    let opened = w
        .fake
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .expect("the host got the document");
    assert_eq!(opened.params["textDocument"]["languageId"], "vb");
    assert_eq!(opened.params["textDocument"]["text"], MODULE_VB);
    let id = module.to_string_lossy().into_owned();
    let (key, generic) = w
        .shell
        .read_with(&w.vcx, |s, _| (s.doc_key(&id), s.generic_servers().len()));
    assert_eq!(key, ServerKey::Host);
    assert_eq!(generic, 0, "no generic server for Visual Basic");
    let language = view.read_with(&w.vcx, |v, _| v.language().map(|l| l.id()));
    assert_eq!(language, Some("vb"));
    // Roslyn's diagnostics for it reach the editor through the host's path.
    w.fake.publish_diagnostics(
        &uri,
        1,
        json!([{"range": range(2, 27, 32), "severity": 1, "code": "BC30512", "source": "vb",
                "message": "Option Strict On disallows implicit conversions from 'String' to 'Integer'."}]),
    );
    w.wait("the squiggle", |w| {
        view.read_with(&w.vcx, |v, _| v.decorations(DIAGNOSTICS_LAYER).len()) == 1
    });
    w.wait("the Error List row", |w| {
        w.error_rows().iter().any(|r| r.code == "BC30512")
    });
    assert_eq!(
        w.shell.read_with(&w.vcx, |s, _| s
            .status()
            .get("language_server:roslyn")
            .map(str::to_owned)),
        None,
        "the host has no generic server slot"
    );
}

struct Fs {
    shell: Entity<Shell>,
    vcx: VisualTestContext,
    fsac: FakeServer,
    dir: TempDir,
    _config: TempDir,
}

/// A solution folder on disk with an F# project (`src/Lib/Lib.fsproj`, a source file two folders down) and a script
/// outside any project, and the shell with the fake FsAutoComplete behind the real registration when `fake` is set,
/// else the registration located for real.
fn setup(cx: &mut TestAppContext, fake: bool, script: impl FnOnce(&FakeServer)) -> Fs {
    cx.executor().allow_parking();
    let dir = tempfile::Builder::new()
        .prefix("eludite-fsharp-")
        .tempdir()
        .unwrap();
    let config = tempfile::tempdir().unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
    };
    write(
        "App.sln",
        "Microsoft Visual Studio Solution File, Format Version 12.00\n",
    );
    write(
        "src/Lib/Lib.fsproj",
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup><ItemGroup><Compile Include=\"Types/Shapes.fs\" /></ItemGroup></Project>",
    );
    write("src/Lib/Types/Shapes.fs", SHAPES_FS);
    write("scripts/build.fsx", BUILD_FSX);
    let fsac = FakeServer::new();
    fsac.set_capabilities(fsac_capabilities());
    script(&fsac);

    let tools = ToolWindowRegistry::vs_default();
    let controller = DockController::new(DockLayout::default_vs(&tools), tools);
    let mut commands = builtins::default_registry();
    view::register(&mut commands, Arc::new(controller.clone())).unwrap();
    let mut services = super::register_workspace(
        &mut commands,
        HostLaunch::Missing("no host in the F# tests".into()),
        crate::settings::SettingsSetup::isolated(Some(config.path().join("settings.json"))),
    );
    services.agents = super::agents::AgentsSetup {
        registry: Some(Vec::new()),
        ..super::agents::AgentsSetup::from_env()
    };
    if fake {
        services
            .launches
            .in_process
            .insert("fsautocomplete".into(), fsac.connector());
    }
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
    Fs {
        shell,
        vcx,
        fsac,
        dir,
        _config: config,
    }
}

impl Fs {
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

    fn run_cmd(&mut self, command: &str, args: Value) -> Value {
        self.shell
            .update_in(&mut self.vcx, |s, window, cx| {
                s.invoke(command, args, window, cx)
            })
            .unwrap_or_else(|e| panic!("{command}: {e}"))
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
            s.status()
                .get("language_server:fsautocomplete")
                .unwrap_or_default()
                .to_owned()
        })
    }

    fn language_server_lines(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.output()
                .read(cx)
                .pane(OutputSource::LanguageServers)
                .tail(usize::MAX)
        })
    }

    /// The roots of the running `fsautocomplete` servers, as their keys name them.
    fn roots(&self) -> Vec<PathBuf> {
        self.shell.read_with(&self.vcx, |s, _| {
            s.generic_servers()
                .keys()
                .filter_map(|k| k.strip_prefix("fsautocomplete|"))
                .map(PathBuf::from)
                .collect()
        })
    }
}

/// Nothing starts until an F# document opens; a `.fs` two folders under its `.fsproj` starts FsAutoComplete rooted at
/// the project's folder (the `*.fsproj` glob marker, not the solution above it) with `AutomaticWorkspaceInit`, gets
/// `didOpen` as `fsharp`, and completion and the server's diagnostic come through the shared paths; an `.fsx` outside
/// any project is a second instance rooted at the solution's folder.
#[gpui::test]
fn an_fs_document_starts_fsautocomplete_at_the_project_folder(cx: &mut TestAppContext) {
    let mut w = setup(cx, true, |fsac| {
        fsac.respond("textDocument/completion", |p| {
            let line = p["position"]["line"].as_u64().unwrap() as u32;
            let ch = p["position"]["character"].as_u64().unwrap() as u32;
            FakeReply::Result(json!({"isIncomplete": false, "items": [
                {"label": "area", "kind": 3, "sortText": "a", "detail": "val area: w: int -> h: int -> int",
                 "textEdit": {"range": range(line, ch - 2, ch), "newText": "area"}},
                {"label": "abs", "kind": 3, "sortText": "b"}]}))
        });
        fsac.diagnose_on_open(
            "Shapes.fs",
            json!([{"range": range(4, 17, 22), "severity": 1, "code": "FS0001", "source": "F# Compiler",
                    "message": "This expression was expected to have type 'int' but here has type 'string'"}]),
        );
    });
    w.vcx.run_until_parked();
    assert_eq!(
        w.shell.read_with(&w.vcx, |s, _| s.generic_servers().len()),
        0,
        "no language server at startup"
    );
    assert_eq!(w.fsac.connections(), 0);

    let view = w.open("src/Lib/Types/Shapes.fs");
    let path = w.path("src/Lib/Types/Shapes.fs");
    let uri = path_to_uri(&path);
    let init = w.fsac.wait_for("initialize", T, |_| true).unwrap();
    let project = w.path("src/Lib");
    assert_eq!(init.params["rootUri"], json!(path_to_uri(&project)));
    assert_eq!(
        init.params["initializationOptions"],
        json!({"AutomaticWorkspaceInit": true})
    );
    assert_eq!(w.roots(), std::slice::from_ref(&project));
    let opened = w
        .fsac
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .unwrap();
    assert_eq!(opened.params["textDocument"]["languageId"], "fsharp");
    assert_eq!(opened.params["textDocument"]["text"], SHAPES_FS);
    w.wait("FsAutoComplete ready", |w| {
        w.slot().starts_with("FsAutoComplete: ready")
    });
    let id = path.to_string_lossy().into_owned();
    let key = w.shell.read_with(&w.vcx, |s, _| s.doc_key(&id));
    assert_eq!(
        key,
        ServerKey::Generic(format!("fsautocomplete|{}", project.to_string_lossy()))
    );
    let language = view.read_with(&w.vcx, |v, _| v.language().map(|l| l.id()));
    assert_eq!(language, Some("fsharp"));

    // The server's diagnostic: a squiggle and a live Error List row with its code.
    w.wait("the diagnostic", |w| {
        view.read_with(&w.vcx, |v, _| v.decorations(DIAGNOSTICS_LAYER).len()) == 1
    });
    w.wait("the Error List row", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.error_list()
                .read(cx)
                .rows()
                .iter()
                .any(|r| r.code == "FS0001")
        })
    });
    // Completion after `(ar` on line 7.
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(SHAPES_FS.find("(ar)").unwrap() + 3))
    });
    w.run_cmd(
        workspace::EDITOR_COMPLETE,
        json!({"path": id, "line": 7, "column": 20}),
    );
    w.wait("the completion list", |w| {
        view.read_with(&w.vcx, |v, _| {
            v.completion()
                .is_some_and(|c| c.visible && c.items.iter().any(|i| i.0 == "area"))
        })
    });
    let asked = w.fsac.received_params("textDocument/completion");
    assert_eq!(asked.last().unwrap()["textDocument"]["uri"], uri);

    // A script outside any project: its own instance, rooted at the folder with the solution file.
    w.open("scripts/build.fsx");
    let fsx = path_to_uri(&w.path("scripts/build.fsx"));
    let opened = w
        .fsac
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == fsx
        })
        .unwrap();
    assert_eq!(opened.params["textDocument"]["languageId"], "fsharp");
    w.wait("the second instance", |w| w.fsac.connections() == 2);
    let mut roots = w.roots();
    roots.sort();
    let mut expected = vec![project, w.path("")];
    expected.sort();
    assert_eq!(roots, expected);
    let inits = w.fsac.received_params("initialize");
    assert!(
        inits
            .iter()
            .any(|p| p["rootUri"] == json!(path_to_uri(&w.path("")))),
        "{inits:?}"
    );
}

/// FsAutoComplete that is not found (no fake, nothing beside eludite, in `ELUDITE_FSAUTOCOMPLETE`, the pinned cache,
/// `~/.dotnet/tools` or on PATH) says so in its slot with its fetch script, the reason goes to the Output window, and
/// the document keeps its highlighting. Skips when a real FsAutoComplete is installed on this machine.
#[gpui::test]
fn fsautocomplete_that_is_not_found_names_its_fetch_script(cx: &mut TestAppContext) {
    let registry = eludite_lsp::ServerRegistry::builtin();
    let reg = registry.get("fsautocomplete").unwrap();
    let cache =
        registry.dotnet_tool_cache("fsautocomplete", eludite_lsp::node::home_dir().as_deref());
    if let Ok(found) = reg.locate(None, cache.as_deref(), &|| Err("not needed".into())) {
        eprintln!(
            "skipped: FsAutoComplete {} is installed here ({})",
            found.version, found.source
        );
        return;
    }
    let mut w = setup(cx, false, |_| {});
    let view = w.open("src/Lib/Types/Shapes.fs");
    w.wait("the server's state", |w| w.slot().contains("not found"));
    assert_eq!(
        w.slot(),
        "FsAutoComplete: not found (run tools/fsautocomplete/fetch.sh)"
    );
    w.wait("the Output line", |w| {
        w.language_server_lines()
            .iter()
            .any(|l| l.contains("fsautocomplete not found") && l.contains("ELUDITE_FSAUTOCOMPLETE"))
    });
    let language = view.read_with(&w.vcx, |v, _| v.language().map(|l| l.id()));
    assert_eq!(language, Some("fsharp"));
    assert_eq!(w.fsac.connections(), 0);
}
