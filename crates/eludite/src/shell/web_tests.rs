//! Headless GPUI tests of brief 0050, the web language stack, against `eludite-lsp`'s scripted `FakeServer`s standing
//! in for typescript-language-server, vscode-eslint's server and the HTML, CSS and JSON servers, through the real
//! `servers.json` registrations: nothing starts until a web document opens; a `.ts` file is served by TypeScript and
//! ESLint together (each with the file's `languageId`), with completion, Quick Info, Go To Definition, Find All
//! References, rename and both servers' diagnostics (ESLint's with its rule id) through the shared paths; ESLint's
//! fix runs its command on ESLint and the edit is one undo step; Format Document runs the project's Prettier (a
//! script in the project's `node_modules`), then the server's formatting, and a failing formatter leaves the document
//! and writes an Output line; format on save formats before writing; Emmet expands on Tab in HTML and CSS and not when
//! `editor.emmet` is off; a server that is not found says `not found (run tools/web-servers/fetch.sh)` while
//! highlighting stays; a `.cshtml` view and a `.razor` component get Razor highlighting (brief 0056) and the HTML
//! server's completion; the JSON server gets the schema associations pushed with the cached schemas. The real servers'
//! test is at the end, gated on `ELUDITE_WEB_SERVERS` (`tools/web-servers/fetch.sh`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::build::OutputSource;
use eludite_commands::workspace;
use eludite_commands::{CommandRegistry, builtins, view};
use eludite_docking::{DockController, DockLayout, ToolWindowRegistry};
use eludite_editor::EditorView;
use eludite_editor::syntax::HighlightKind;
use eludite_lsp::fake::FakeReply;
use eludite_lsp::fake_server::FakeServer;
use eludite_ui::{Theme, bind_keymap, vs_keymap};
use gpui::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::Shell;
use super::documents::{DIAGNOSTICS_LAYER, normalize_path, path_to_uri};
use super::session::HostLaunch;

const T: Duration = Duration::from_secs(20);

const MAIN_TS: &str = "import { setupCounter } from \"./counter\";\n\nlet count: number = \"two\";\nsetupCounter(count);\n";
const LEGACY_JS: &str = "let total = 1;\nconsole.log(total);\n";
const INDEX_HTML: &str = "<!doctype html>\n<html>\n  <body>\n    \n  </body>\n  <script>\n    const n = 1;\n  </script>\n</html>\n";
const SITE_CSS: &str = ".card {\n  \n}\n";
const INDEX_CSHTML: &str =
    "@model Demo.Home\n<div class=\"greeting\">\n  <h1>@Model.Name</h1>\n  \n</div>\n";
const COUNTER_RAZOR: &str = "@page \"/counter\"\n<div class=\"counter\">\n  <h1>Count: @count</h1>\n  \n</div>\n\
@code {\n    private int count;\n}\n";

/// The fake Prettier: a shell script installed as the project's `prettier` package (its `bin`), so `auto` finds it
/// where it finds the real one. It squeezes runs of spaces, fails on `BROKEN`, and answers `--version`.
#[cfg(unix)]
const FAKE_PRETTIER: &str = "#!/bin/sh\n\
if [ \"$1\" = \"--version\" ]; then echo 3.9.9-fake; exit 0; fi\n\
[ \"$1\" = \"--stdin-filepath\" ] || { echo \"expected --stdin-filepath\" >&2; exit 2; }\n\
input=$(cat)\n\
case \"$input\" in *BROKEN*) echo \"[error] $2: SyntaxError: Unexpected token (1:5)\" >&2; exit 2;; esac\n\
printf '%s\\n' \"$input\" | sed -e 's/  */ /g'\n";

/// TypeScript-like capabilities, as typescript-language-server advertises them.
fn ts_capabilities() -> Value {
    json!({
        "textDocumentSync": {"openClose": true, "change": 2, "save": {}},
        "completionProvider": {"triggerCharacters": [".", "\"", "'", "/", "@", "<"], "resolveProvider": true},
        "hoverProvider": true,
        "signatureHelpProvider": {"triggerCharacters": ["(", ",", "<"]},
        "definitionProvider": true,
        "referencesProvider": true,
        "renameProvider": {"prepareProvider": true},
        "codeActionProvider": {"resolveProvider": true},
        "documentFormattingProvider": true,
        "executeCommandProvider": {"commands": ["_typescript.applyRefactoring"]}
    })
}

/// vscode-eslint's server's capabilities.
fn eslint_capabilities() -> Value {
    json!({
        "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
        "codeActionProvider": {"codeActionKinds": ["quickfix", "source.fixAll.eslint"]},
        "executeCommandProvider": {"commands": ["eslint.applySingleFix", "eslint.applySameFixes",
            "eslint.applyAllFixes", "eslint.applyDisableLine", "eslint.applyDisableFile", "eslint.openRuleDoc"]}
    })
}

/// The HTML, CSS and JSON servers' capabilities.
fn markup_capabilities() -> Value {
    json!({
        "textDocumentSync": {"openClose": true, "change": 2},
        "completionProvider": {"triggerCharacters": [".", ":", "<", "\"", "=", "/"], "resolveProvider": false},
        "hoverProvider": true,
        "documentFormattingProvider": true
    })
}

struct Web {
    shell: Entity<Shell>,
    vcx: VisualTestContext,
    commands: Arc<CommandRegistry>,
    ts: FakeServer,
    eslint: FakeServer,
    html: FakeServer,
    json: FakeServer,
    dir: TempDir,
    cache: TempDir,
    _config: TempDir,
}

/// A web project on disk (package.json, tsconfig.json, an ESLint flat config, TypeScript 5.9.3 in its
/// `node_modules`, a fake Prettier on Unix, the files above) and the fake servers behind the real registrations.
/// The CSS server has no fake: it is looked for, and not found (the web servers' cache is an empty folder).
fn setup(cx: &mut TestAppContext, script: impl FnOnce(&Web)) -> Web {
    cx.executor().allow_parking();
    let dir = tempfile::Builder::new()
        .prefix("eludite-web-")
        .tempdir()
        .unwrap();
    let cache = tempfile::Builder::new()
        .prefix("eludite-web-cache-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
    };
    write(
        "package.json",
        "{\"name\": \"web\", \"private\": true, \"type\": \"module\"}\n",
    );
    write(
        "tsconfig.json",
        "{\n  // Strict.\n  \"compilerOptions\": {\"strict\": true}\n}\n",
    );
    write(
        "eslint.config.js",
        "export default [{ rules: { \"prefer-const\": \"error\" } }];\n",
    );
    write(
        "node_modules/typescript/package.json",
        "{\"name\": \"typescript\", \"version\": \"5.9.3\"}",
    );
    write("node_modules/typescript/lib/tsserver.js", "");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        write(
            "node_modules/prettier/package.json",
            "{\"name\": \"prettier\", \"version\": \"3.9.9-fake\", \"bin\": {\"prettier\": \"bin/prettier.sh\"}}",
        );
        write("node_modules/prettier/bin/prettier.sh", FAKE_PRETTIER);
        let p = root.join("node_modules/prettier/bin/prettier.sh");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    write("src/main.ts", MAIN_TS);
    write("src/legacy.js", LEGACY_JS);
    write("index.html", INDEX_HTML);
    write("site.css", SITE_CSS);
    write("Views/Home/Index.cshtml", INDEX_CSHTML);
    write("Components/Pages/Counter.razor", COUNTER_RAZOR);
    write("appsettings.json", "{\n  \"Logging\": {}\n}\n");
    let cache_path = cache.path().to_path_buf();
    launch(cx, dir, cache, Some(cache_path), true, script)
}

/// The shell on project `dir`, with the fake servers behind the registrations (`fakes`) or the real ones from the
/// web servers' cache `servers` (the folder `tools/web-servers/fetch.sh` printed).
fn launch(
    cx: &mut TestAppContext,
    dir: TempDir,
    cache: TempDir,
    servers: Option<PathBuf>,
    fakes: bool,
    script: impl FnOnce(&Web),
) -> Web {
    cx.executor().allow_parking();
    let config = tempfile::tempdir().unwrap();
    let ts = FakeServer::new();
    ts.set_capabilities(ts_capabilities());
    let eslint = FakeServer::new();
    eslint.set_capabilities(eslint_capabilities());
    let html = FakeServer::new();
    html.set_capabilities(markup_capabilities());
    let json = FakeServer::new();
    json.set_capabilities(markup_capabilities());

    let tools = ToolWindowRegistry::vs_default();
    let controller = DockController::new(DockLayout::default_vs(&tools), tools);
    let mut commands = builtins::default_registry();
    view::register(&mut commands, Arc::new(controller.clone())).unwrap();
    let mut services = super::register_workspace(
        &mut commands,
        HostLaunch::Missing("no host in the web tests".into()),
        crate::settings::SettingsSetup::isolated(Some(config.path().join("settings.json"))),
    );
    services.agents = super::agents::AgentsSetup {
        registry: Some(Vec::new()),
        ..super::agents::AgentsSetup::from_env()
    };
    if fakes {
        for (id, fake) in [
            ("typescript", &ts),
            ("eslint", &eslint),
            ("html", &html),
            ("json", &json),
        ] {
            services
                .launches
                .in_process
                .insert(id.into(), fake.connector());
        }
    }
    // The web servers' cache: with the fakes an empty folder (the JSON schemas' URLs name it; the CSS server is not
    // in it), else the fetched one.
    services.launches.cache = Some(servers);
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
                    Theme::dark(),
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
    let w = Web {
        shell,
        vcx,
        commands,
        ts,
        eslint,
        html,
        json,
        dir,
        cache,
        _config: config,
    };
    script(&w);
    w
}

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

impl Web {
    fn path(&self, rel: &str) -> PathBuf {
        normalize_path(&self.dir.path().join(rel))
    }

    fn uri(&self, rel: &str) -> String {
        path_to_uri(&self.path(rel))
    }

    fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + eludite_test_support::hang_bound(T);
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

    /// `eludite.settings.set`, then wait until the shell applied it.
    fn set(&mut self, key: &str, value: Value) {
        let c = self.commands.clone();
        let args = json!({"key": key, "value": value});
        std::thread::spawn(move || c.invoke(eludite_commands::settings::SET, args).unwrap())
            .join()
            .unwrap();
        let key = key.to_owned();
        self.wait("the setting to apply", |w| {
            w.shell.read_with(&w.vcx, |s, _| {
                let l = &s.launches;
                match key.as_str() {
                    "editor.formatter" => l.formatter == value.as_str().unwrap(),
                    "editor.emmet" => l.emmet == value.as_bool().unwrap(),
                    k if k.starts_with("editor.formatOnSave.") => {
                        l.format_on_save.get(&k["editor.formatOnSave.".len()..])
                            == value.as_bool().as_ref()
                    }
                    k => l.settings.get(k).map(String::as_str) == value.as_str(),
                }
            })
        });
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

    fn slot(&self, id: &str) -> String {
        self.shell.read_with(&self.vcx, |s, _| {
            s.status()
                .get(&format!("language_server:{id}"))
                .unwrap_or_default()
                .to_owned()
        })
    }

    fn text(&self, view: &Entity<EditorView>) -> String {
        view.read_with(&self.vcx, |v, _| v.editor().text())
    }

    fn language_server_lines(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.output()
                .read(cx)
                .pane(OutputSource::LanguageServers)
                .tail(usize::MAX)
        })
    }

    fn format_done(&mut self, rel: &str) -> eludite_commands::workspace::FormatDocumentOutput {
        let id = self.path(rel).to_string_lossy().into_owned();
        self.wait("Format Document", |w| {
            w.shell.read_with(&w.vcx, |s, _| {
                s.format_result(&id)
                    .is_some_and(|o| o.state == eludite_commands::workspace::FormatState::Done)
            })
        });
        self.shell
            .read_with(&self.vcx, |s, _| s.format_result(&id).cloned())
            .unwrap()
    }

    fn type_at_end_of_line(&mut self, view: &Entity<EditorView>, line: usize, text: &str) {
        view.update(&mut self.vcx, |v, cx| {
            v.update_editor(cx, |e| {
                let full = e.text();
                let start: usize = full.split_inclusive('\n').take(line).map(str::len).sum();
                let end = start + full[start..].find('\n').unwrap_or(full.len() - start);
                e.set_caret(end);
                e.insert(text);
            })
        });
        self.vcx.run_until_parked();
    }

    fn focus(&mut self, view: &Entity<EditorView>) {
        let view = view.clone();
        self.vcx
            .update(|window, cx| view.focus_handle(cx).focus(window, cx));
        self.vcx.run_until_parked();
    }
}

/// Nothing is launched at startup; the first `.ts` document starts TypeScript and ESLint (the project has an ESLint
/// configuration), each with the file's `languageId`, TypeScript on the project's own TypeScript (in its status bar
/// slot and `tsserver.path`), ESLint with its settings and the workspace folder; a `.js` file goes to both as
/// `javascript`; HTML starts only the HTML server.
#[gpui::test]
fn nothing_starts_until_a_web_document_opens_then_typescript_and_eslint_serve_it(
    cx: &mut TestAppContext,
) {
    let mut w = setup(cx, |_| {});
    w.vcx.run_until_parked();
    let started = w.shell.read_with(&w.vcx, |s, _| s.generic_servers().len());
    assert_eq!(started, 0, "no language server at startup");
    for fake in [&w.ts, &w.eslint, &w.html, &w.json] {
        assert_eq!(fake.connections(), 0);
    }

    w.open("src/main.ts");
    let uri = w.uri("src/main.ts");
    for (name, fake) in [("TypeScript", &w.ts), ("ESLint", &w.eslint)] {
        let open = fake
            .wait_for("textDocument/didOpen", T, |p| {
                p["textDocument"]["uri"] == uri
            })
            .unwrap_or_else(|| panic!("{name} got the document"));
        assert_eq!(
            open.params["textDocument"]["languageId"], "typescript",
            "{name}"
        );
        assert_eq!(open.params["textDocument"]["text"], MAIN_TS, "{name}");
    }
    let init = w.ts.wait_for("initialize", T, |_| true).unwrap();
    let root = normalize_path(w.dir.path());
    assert_eq!(init.params["rootUri"], json!(path_to_uri(&root)));
    assert_eq!(
        init.params["initializationOptions"]["tsserver"]["path"],
        json!(
            root.join("node_modules")
                .join("typescript")
                .join("lib")
                .to_string_lossy()
        )
    );
    assert_eq!(init.params["initializationOptions"]["hostInfo"], "eludite");
    w.wait("TypeScript ready on the project's TypeScript", |w| {
        w.slot("typescript").contains("TypeScript 5.9.3, project")
    });
    assert!(
        w.slot("typescript").starts_with("TypeScript: ready"),
        "{}",
        w.slot("typescript")
    );
    w.wait("ESLint ready", |w| {
        w.slot("eslint").starts_with("ESLint: ready")
    });
    // ESLint asks for its settings with an empty section: the whole object, with the workspace folder filled in.
    let eslint = w.eslint.clone();
    let config = std::thread::spawn(move || {
        eslint.request_client(
            "workspace/configuration",
            json!({"items": [{"scopeUri": "file:///x", "section": ""}]}),
            T,
        )
    });
    let config = loop {
        w.vcx.run_until_parked();
        if config.is_finished() {
            break config.join().unwrap().unwrap()["result"].clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(config[0]["validate"], "on");
    assert_eq!(
        config[0]["workspaceFolder"]["uri"],
        json!(path_to_uri(&root))
    );
    assert_eq!(
        config[0]["workspaceFolder"]["name"],
        json!(root.file_name().unwrap().to_string_lossy())
    );

    // A JavaScript file: both servers again (the same processes), as `javascript`.
    w.open("src/legacy.js");
    let js = w.uri("src/legacy.js");
    for fake in [&w.ts, &w.eslint] {
        let open = fake
            .wait_for("textDocument/didOpen", T, |p| {
                p["textDocument"]["uri"] == js
            })
            .unwrap();
        assert_eq!(open.params["textDocument"]["languageId"], "javascript");
        assert_eq!(fake.connections(), 1);
    }
    // HTML: the HTML server only.
    w.open("index.html");
    assert!(
        w.html
            .wait_for(
                "textDocument/didOpen",
                T,
                |p| p["textDocument"]["languageId"] == "html"
            )
            .is_some()
    );
    assert!(
        w.ts.received_params("textDocument/didOpen")
            .iter()
            .all(|p| p["textDocument"]["languageId"] != "html")
    );
    let keys: Vec<String> = w.shell.read_with(&w.vcx, |s, _| {
        s.generic_servers()
            .keys()
            .map(|k| k.split('|').next().unwrap().to_owned())
            .collect()
    });
    assert_eq!(keys.len(), 3, "{keys:?}");
}

/// With `languageServers.eslint` off, a `.ts` file has TypeScript only; `on` runs ESLint without a configuration.
#[gpui::test]
fn the_eslint_setting_decides_whether_eslint_runs(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    w.set("languageServers.eslint", json!("off"));
    w.open("src/main.ts");
    w.wait("TypeScript", |w| {
        w.slot("typescript").starts_with("TypeScript: ready")
    });
    assert_eq!(w.eslint.connections(), 0);
    assert_eq!(w.slot("eslint"), "");
    let doc_servers = w.shell.read_with(&w.vcx, |s, _| {
        s.documents
            .values()
            .map(|d| d.servers.len())
            .collect::<Vec<_>>()
    });
    assert_eq!(doc_servers, [1]);
}

/// Completion, Quick Info, Go To Definition, Find All References and rename through the shared paths in a `.ts`
/// file; TypeScript's and ESLint's diagnostics shown together, ESLint's with its rule id.
#[gpui::test]
fn ts_editor_features_and_both_servers_diagnostics(cx: &mut TestAppContext) {
    let mut w = setup(cx, |w| {
        w.ts.respond("textDocument/completion", |p| {
            let line = p["position"]["line"].as_u64().unwrap() as u32;
            let ch = p["position"]["character"].as_u64().unwrap() as u32;
            FakeReply::Result(json!({"isIncomplete": false, "items": [
                {"label": "setupCounter", "kind": 3, "sortText": "a", "textEdit": {"range": range(line, 0, ch), "newText": "setupCounter"}},
                {"label": "setTimeout", "kind": 3, "sortText": "b"}]}))
        });
        w.ts.respond("textDocument/hover", |_| {
            FakeReply::Result(json!({"contents": {"kind": "markdown", "value": "```typescript\nlet count: number\n```"}}))
        });
        w.ts.respond("textDocument/definition", |p| {
            FakeReply::Result(json!({"uri": p["textDocument"]["uri"], "range": range(2, 4, 9)}))
        });
        w.ts.respond("textDocument/references", |p| {
            let uri = p["textDocument"]["uri"].clone();
            FakeReply::Result(json!([{"uri": uri, "range": range(2, 4, 9)}, {"uri": uri, "range": range(3, 13, 18)}]))
        });
        w.ts.respond("textDocument/prepareRename", |_| {
            FakeReply::Result(range(2, 4, 9))
        });
        w.ts.respond("textDocument/rename", |p| {
            let uri = p["textDocument"]["uri"].as_str().unwrap().to_owned();
            FakeReply::Result(json!({"changes": {uri: [
                {"range": range(2, 4, 9), "newText": "total"}, {"range": range(3, 13, 18), "newText": "total"}]}}))
        });
        w.ts.diagnose_on_open(
            "main.ts",
            json!([{"range": range(2, 4, 9), "severity": 1, "code": 2322, "source": "ts",
                    "message": "Type 'string' is not assignable to type 'number'."}]),
        );
        w.eslint.diagnose_on_open(
            "main.ts",
            json!([{"range": range(2, 4, 9), "severity": 1, "code": "prefer-const", "source": "eslint",
                    "message": "'count' is never reassigned. Use 'const' instead."}]),
        );
    });
    let path = w.path("src/main.ts").to_string_lossy().into_owned();
    let view = w.open("src/main.ts");
    w.wait("TypeScript ready", |w| {
        w.slot("typescript").starts_with("TypeScript: ready")
    });

    // Both servers' diagnostics: two squiggles and two live Error List rows, TypeScript's then ESLint's.
    w.wait("both servers' diagnostics", |w| {
        view.read_with(&w.vcx, |v, _| v.decorations(DIAGNOSTICS_LAYER).len()) == 2
    });
    w.wait("the Error List rows", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.error_list().read(cx).rows().len())
            == 2
    });
    let codes: Vec<String> = w.shell.read_with(&w.vcx, |s, cx| {
        s.error_list()
            .read(cx)
            .rows()
            .iter()
            .map(|r| r.code.clone())
            .collect()
    });
    assert!(
        codes.contains(&"2322".to_owned()) && codes.contains(&"prefer-const".to_owned()),
        "{codes:?}"
    );
    let listed = w
        .commands
        .invoke(eludite_commands::diagnostics::DIAGNOSTICS_LIST, json!({}))
        .unwrap();
    let text = listed.to_string();
    assert!(
        text.contains("prefer-const") && text.contains("2322"),
        "{text}"
    );

    // Completion: TypeScript's items (ESLint offers none and is not asked).
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(MAIN_TS.find("setupCounter(count)").unwrap() + 3)
        })
    });
    w.run_cmd(
        workspace::EDITOR_COMPLETE,
        json!({"path": path, "line": 4, "column": 4}),
    );
    w.wait("the completion list", |w| {
        view.read_with(&w.vcx, |v, _| {
            v.completion()
                .is_some_and(|c| c.visible && c.items.iter().any(|i| i.0 == "setupCounter"))
        })
    });
    assert!(
        w.eslint
            .received_params("textDocument/completion")
            .is_empty()
    );
    // Quick Info.
    w.run_cmd(
        workspace::EDITOR_HOVER,
        json!({"path": path, "line": 3, "column": 6}),
    );
    w.wait("Quick Info", |w| {
        view.read_with(&w.vcx, |v, _| v.hover()).is_some_and(|h| {
            h.visible
                && h.text
                    .as_deref()
                    .is_some_and(|t| t.contains("let count: number"))
        })
    });
    // Go To Definition.
    w.run_cmd(
        workspace::EDITOR_GO_TO_DEFINITION,
        json!({"path": path, "line": 4, "column": 15}),
    );
    w.wait("the caret at the definition", |w| {
        view.read_with(&w.vcx, |v, _| v.editor().primary_selection().head)
            == MAIN_TS.find("count:").unwrap()
    });
    // Find All References.
    w.run_cmd(
        workspace::EDITOR_FIND_REFERENCES,
        json!({"path": path, "line": 3, "column": 6}),
    );
    w.wait("the references", |w| {
        !w.ts.received_params("textDocument/references").is_empty()
    });
    // Rename through the applier.
    w.run_cmd(
        workspace::EDITOR_RENAME,
        json!({"path": path, "line": 3, "column": 6, "new_name": "total", "apply": true}),
    );
    w.wait("the rename", |w| {
        w.text(&view).contains("let total: number")
    });
    assert!(w.text(&view).contains("setupCounter(total)"));
}

/// ESLint's quick fix is a command: Eludite runs it on ESLint (`workspace/executeCommand`), ESLint answers with
/// `workspace/applyEdit`, and the edit is one undo step. TypeScript's actions come first in the list.
#[gpui::test]
fn eslints_fix_runs_its_command_and_applies_as_one_undo_step(cx: &mut TestAppContext) {
    let mut w = setup(cx, |w| {
        w.ts.respond("textDocument/codeAction", |_| {
            FakeReply::Result(
                json!([{"title": "Infer type", "kind": "refactor.rewrite", "data": {"id": 1}}]),
            )
        });
        w.eslint.respond("textDocument/codeAction", |p| {
            let uri = p["textDocument"]["uri"].clone();
            FakeReply::Result(json!([
                {"title": "Fix this prefer-const problem", "kind": "quickfix", "isPreferred": true,
                 "command": {"title": "Fix this prefer-const problem", "command": "eslint.applySingleFix",
                             "arguments": [{"uri": uri, "version": 1, "ruleId": "prefer-const"}]}},
                {"title": "Fix all auto-fixable problems", "kind": "quickfix",
                 "command": {"title": "Fix all auto-fixable problems", "command": "eslint.applyAllFixes",
                             "arguments": [{"uri": uri, "version": 1}]}}]))
        });
        // The fix: ESLint asks the client to apply the edit, then answers the command.
        let eslint = w.eslint.clone();
        let uri = w.uri("src/legacy.js");
        w.eslint.respond("workspace/executeCommand", move |p| {
            let (eslint, uri) = (eslint.clone(), uri.clone());
            if p["command"] == "eslint.applySingleFix" {
                std::thread::spawn(move || {
                    eslint.request_client(
                        "workspace/applyEdit",
                        json!({"label": "Fix", "edit": {"changes": {uri: [{"range": range(0, 0, 3), "newText": "const"}]}}}),
                        T,
                    )
                });
            }
            FakeReply::After(Duration::from_millis(100), Value::Null)
        });
        w.eslint.diagnose_on_open(
            "legacy.js",
            json!([{"range": range(0, 4, 9), "severity": 1, "code": "prefer-const", "source": "eslint",
                    "message": "'total' is never reassigned. Use 'const' instead."}]),
        );
    });
    let path = w.path("src/legacy.js").to_string_lossy().into_owned();
    let view = w.open("src/legacy.js");
    w.wait("ESLint ready", |w| {
        w.slot("eslint").starts_with("ESLint: ready")
    });
    w.wait("ESLint's diagnostic", |w| {
        view.read_with(&w.vcx, |v, _| v.decorations(DIAGNOSTICS_LAYER).len()) == 1
    });
    let listed = w.run_cmd(
        workspace::EDITOR_CODE_ACTIONS,
        json!({"path": path, "line": 1, "column": 6}),
    );
    let _ = listed;
    w.wait("the light bulb menu", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.code_action_menu()
                .is_some_and(|m| m.read(cx).list().actions.len() == 3)
        })
    });
    let titles: Vec<String> = w.shell.read_with(&w.vcx, |s, cx| {
        s.code_action_menu()
            .unwrap()
            .read(cx)
            .list()
            .actions
            .iter()
            .map(|a| a.title.clone())
            .collect()
    });
    assert!(
        titles.contains(&"Fix all auto-fixable problems".to_owned()),
        "{titles:?}"
    );
    let fix = titles
        .iter()
        .position(|t| t == "Fix this prefer-const problem")
        .unwrap();
    // The light bulb's context carried ESLint's diagnostic.
    let asked = w.eslint.received_params("textDocument/codeAction");
    assert_eq!(
        asked[0]["context"]["diagnostics"][0]["code"],
        "prefer-const"
    );
    w.run_cmd(workspace::EDITOR_APPLY_CODE_ACTION, json!({"index": fix}));
    w.wait("the fix", |w| w.text(&view).starts_with("const total = 1;"));
    let exec = w.eslint.received_params("workspace/executeCommand");
    assert_eq!(exec[0]["command"], "eslint.applySingleFix");
    assert_eq!(exec[0]["arguments"][0]["ruleId"], "prefer-const");
    assert!(w.ts.received_params("workspace/executeCommand").is_empty());
    // One undo step.
    w.run_cmd(workspace::EDITOR_UNDO, json!({"path": path}));
    assert_eq!(w.text(&view), LEGACY_JS);
}

/// Format Document (Ctrl+K, Ctrl+D): the project's Prettier (`auto` finds it in its `node_modules`) on the
/// document's text, one undo step; `server` asks the language server; a failing formatter leaves the document and
/// writes an Output line.
#[cfg(unix)]
#[gpui::test]
fn format_document_runs_the_projects_prettier_then_the_server(cx: &mut TestAppContext) {
    let mut w = setup(cx, |w| {
        w.ts.respond("textDocument/formatting", |_| {
            FakeReply::Result(
                json!([{"range": range(0, 0, 0), "newText": "// formatted by the server\n"}]),
            )
        });
    });
    let path = w.path("src/main.ts").to_string_lossy().into_owned();
    let view = w.open("src/main.ts");
    w.wait("TypeScript ready", |w| {
        w.slot("typescript").starts_with("TypeScript: ready")
    });
    w.type_at_end_of_line(&view, 3, "   let   spaced   =   1;");
    let messy = w.text(&view);
    // Ctrl+K, Ctrl+D.
    w.focus(&view);
    w.vcx.simulate_keystrokes("ctrl-k ctrl-d");
    let out = w.format_done("src/main.ts");
    assert_eq!(
        (
            out.formatter.as_str(),
            out.version.as_deref(),
            out.applied,
            out.edits
        ),
        ("prettier", Some("3.9.9-fake"), true, 1)
    );
    assert!(
        w.text(&view).contains(" let spaced = 1;"),
        "{}",
        w.text(&view)
    );
    assert!(!w.text(&view).contains("   "));
    // One undo step back to the unformatted text.
    w.run_cmd(workspace::EDITOR_UNDO, json!({"path": path}));
    assert_eq!(w.text(&view), messy);
    // The language server's formatting.
    w.set("editor.formatter", json!("server"));
    let out = w.run_cmd(workspace::EDITOR_FORMAT_DOCUMENT, json!({"path": path}));
    assert_eq!(out["state"], "formatting");
    let out = w.format_done("src/main.ts");
    assert_eq!(
        (out.formatter.as_str(), out.server.as_deref(), out.applied),
        ("server", Some("typescript"), true)
    );
    assert!(w.text(&view).starts_with("// formatted by the server\n"));
    // A formatter that fails: the document stays, the Output window says why.
    w.set("editor.formatter", json!("prettier"));
    w.type_at_end_of_line(&view, 0, " BROKEN");
    let before = w.text(&view);
    w.run_cmd(workspace::EDITOR_FORMAT_DOCUMENT, json!({"path": path}));
    let out = w.format_done("src/main.ts");
    assert!(!out.applied);
    assert!(
        out.message.as_deref().unwrap().contains("SyntaxError"),
        "{out:?}"
    );
    assert_eq!(w.text(&view), before);
    assert!(
        w.language_server_lines()
            .iter()
            .any(|l| l.contains("Format Document") && l.contains("SyntaxError"))
    );
}

/// Format on save (`editor.formatOnSave.typescript`): Ctrl+S formats, then writes the formatted text.
#[cfg(unix)]
#[gpui::test]
fn format_on_save_formats_before_the_file_is_written(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    w.set("editor.formatOnSave.typescript", json!(true));
    let path = w.path("src/main.ts");
    let view = w.open("src/main.ts");
    w.type_at_end_of_line(&view, 3, "   const   a   =   2;");
    let out = w.run_cmd(
        workspace::EDITOR_SAVE,
        json!({"path": path.to_string_lossy()}),
    );
    assert_eq!(out["path"], json!(path.to_string_lossy()));
    w.wait("the formatted file on disk", |_| {
        std::fs::read_to_string(&path).is_ok_and(|t| t.contains(" const a = 2;"))
    });
    let dirty = w
        .shell
        .read_with(&w.vcx, |s, _| s.documents[&*path.to_string_lossy()].dirty);
    assert!(!dirty);
    // Off: the file is written as typed.
    w.set("editor.formatOnSave.typescript", json!(false));
    w.type_at_end_of_line(&view, 0, "   // as   typed");
    w.run_cmd(
        workspace::EDITOR_SAVE,
        json!({"path": path.to_string_lossy()}),
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("   // as   typed")
    );
}

/// Emmet: Tab expands an abbreviation in HTML and CSS (no completion item selected); with `editor.emmet` off, Tab
/// indents; in TypeScript, Tab indents.
#[gpui::test]
fn emmet_expands_on_tab_in_html_and_css(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    let view = w.open("index.html");
    w.type_at_end_of_line(&view, 3, "ul>li.item$*2");
    w.focus(&view);
    w.vcx.simulate_keystrokes("tab");
    let text = w.text(&view);
    assert!(
        text.contains("    <ul>\n        <li class=\"item1\"></li>\n        <li class=\"item2\"></li>\n    </ul>\n"),
        "{text}"
    );
    // The caret is inside the first item: typing goes there.
    w.vcx.simulate_input("one");
    assert!(w.text(&view).contains("<li class=\"item1\">one</li>"));
    // One undo step back to the abbreviation (typing aside).
    let css = w.open("site.css");
    w.type_at_end_of_line(&css, 1, "m10-20");
    w.focus(&css);
    w.vcx.simulate_keystrokes("tab");
    assert_eq!(w.text(&css), ".card {\n  margin: 10px 20px;\n}\n");
    // A plain word is not an abbreviation.
    w.type_at_end_of_line(&css, 1, " hello");
    w.vcx.simulate_keystrokes("tab");
    assert!(w.text(&css).contains("hello"), "{}", w.text(&css));
    // Off: Tab indents.
    w.set("editor.emmet", json!(false));
    w.type_at_end_of_line(&view, 3, "p");
    w.focus(&view);
    w.vcx.simulate_keystrokes("tab");
    assert!(!w.text(&view).contains("<p></p>"));
    // TypeScript has no Emmet.
    w.set("editor.emmet", json!(true));
    let ts = w.open("src/main.ts");
    w.type_at_end_of_line(&ts, 1, "div");
    w.focus(&ts);
    w.vcx.simulate_keystrokes("tab");
    assert!(!w.text(&ts).contains("<div>"));
}

/// A server that is not found (no fake, nothing in the cache, the project or PATH) says so in its slot with the fetch
/// command, the reason goes to the Output window, and the document still has tree-sitter highlighting.
#[gpui::test]
fn a_server_that_is_not_found_names_the_fetch_command(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    let view = w.open("site.css");
    w.wait("the CSS server's state", |w| {
        w.slot("css").contains("not found")
    });
    assert_eq!(
        w.slot("css"),
        "CSS: not found (run tools/web-servers/fetch.sh)"
    );
    assert!(
        w.language_server_lines()
            .iter()
            .any(|l| l.contains("vscode-css-language-server not found"))
    );
    let language = view.read_with(&w.vcx, |v, _| v.language().map(|l| l.id()));
    assert_eq!(language, Some("css"));
    // Typing still works, with the syntax completion fallback.
    w.type_at_end_of_line(&view, 1, "color: red;");
    assert!(w.text(&view).contains("color: red;"));
}

/// A Razor view (`.cshtml`) is highlighted by the Razor grammar (brief 0056), its markup served by the HTML server.
#[gpui::test]
fn a_cshtml_view_gets_razor_highlighting_and_the_html_servers_completion(cx: &mut TestAppContext) {
    razor_highlighting_and_html_completion(cx, "Views/Home/Index.cshtml", &[]);
}

/// A Blazor component (`.razor`) is highlighted by the Razor grammar, its `@code` block's C# by the C# query, and its
/// markup is served by the HTML server as a `.cshtml` view's is.
#[gpui::test]
fn a_razor_component_gets_razor_highlighting_and_the_html_servers_completion(
    cx: &mut TestAppContext,
) {
    razor_highlighting_and_html_completion(
        cx,
        "Components/Pages/Counter.razor",
        &[
            (5, 0, HighlightKind::Keyword, "@code"),
            (5, 1, HighlightKind::Keyword, "@code"),
            (6, 4, HighlightKind::Keyword, "private"),
            (6, 12, HighlightKind::TypeBuiltin, "int"),
        ],
    );
}

/// Open the Razor document `rel` (its first row a directive, its second `<div class=...>`, its third `  <h1>`, its
/// fourth blank): the editor's language is Razor, with a keyword at the directive's `@`, a tag at `div` and `h1`, an
/// attribute name at `class` and the `more` kinds at their rows and columns; the HTML server gets it with `languageId`
/// `html` and completes `<s` typed on the blank row.
fn razor_highlighting_and_html_completion(
    cx: &mut TestAppContext,
    rel: &str,
    more: &[(u32, u32, HighlightKind, &str)],
) {
    let mut w = setup(cx, |w| {
        w.html.respond("textDocument/completion", |_| {
            FakeReply::Result(
                json!([{"label": "section", "kind": 10}, {"label": "span", "kind": 10}]),
            )
        });
    });
    let path = w.path(rel).to_string_lossy().into_owned();
    let file = rel.rsplit('/').next().unwrap().to_owned();
    let view = w.open(rel);
    let open = w
        .html
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"]
                .as_str()
                .is_some_and(|u| u.ends_with(&file))
        })
        .unwrap();
    assert_eq!(open.params["textDocument"]["languageId"], "html");
    let language = view.read_with(&w.vcx, |v, _| v.language().map(|l| l.id()));
    assert_eq!(language, Some("razor"));
    w.wait("the Razor highlights", |w| {
        view.read_with(&w.vcx, |v, _| v.highlights_complete())
    });
    let kind = |w: &Web, row, column| {
        view.read_with(&w.vcx, |v, _| {
            v.highlights()
                .kind_at(eludite_editor::text::Point::new(row, column))
        })
    };
    assert_eq!(
        kind(&w, 0, 0),
        Some(HighlightKind::Keyword),
        "the directive"
    );
    assert_eq!(
        kind(&w, 0, 1),
        Some(HighlightKind::Keyword),
        "the directive"
    );
    assert_eq!(kind(&w, 1, 1), Some(HighlightKind::Tag), "div");
    assert_eq!(kind(&w, 1, 5), Some(HighlightKind::AttributeName), "class");
    assert_eq!(kind(&w, 2, 3), Some(HighlightKind::Tag), "h1");
    for &(row, column, want, what) in more {
        assert_eq!(kind(&w, row, column), Some(want), "{what}");
    }
    w.wait("HTML ready", |w| w.slot("html").starts_with("HTML: ready"));
    w.type_at_end_of_line(&view, 3, "<s");
    w.run_cmd(
        workspace::EDITOR_COMPLETE,
        json!({"path": path, "line": 4, "column": 5}),
    );
    w.wait("the HTML server's list", |w| {
        view.read_with(&w.vcx, |v, _| {
            v.completion()
                .is_some_and(|c| c.visible && c.items.iter().any(|i| i.0 == "section"))
        })
    });
}

/// The JSON server gets its settings pushed after `initialized`: the SchemaStore associations at the cached schema
/// files, and file-only schema loading (no network).
#[gpui::test]
fn the_json_server_gets_the_cached_schema_associations(cx: &mut TestAppContext) {
    let mut w = setup(cx, |_| {});
    w.open("appsettings.json");
    let init = w.json.wait_for("initialize", T, |_| true).unwrap();
    assert_eq!(
        init.params["initializationOptions"]["handledSchemaProtocols"],
        json!(["file"])
    );
    let pushed = w
        .json
        .wait_for("workspace/didChangeConfiguration", T, |_| true)
        .unwrap();
    let schemas = pushed.params["settings"]["json"]["schemas"]
        .as_array()
        .unwrap()
        .clone();
    let cache_uri = path_to_uri(w.cache.path());
    let appsettings = schemas
        .iter()
        .find(|s| {
            s["fileMatch"]
                .as_array()
                .unwrap()
                .contains(&json!("appsettings.json"))
        })
        .unwrap();
    assert_eq!(
        appsettings["url"],
        json!(format!(
            "{}/schemas/appsettings.json",
            cache_uri.trim_end_matches('/')
        ))
    );
    let open = w
        .json
        .wait_for("textDocument/didOpen", T, |_| true)
        .unwrap();
    assert_eq!(open.params["textDocument"]["languageId"], "json");
    // tsconfig.json is JSON with comments.
    w.open("tsconfig.json");
    assert!(
        w.json
            .wait_for(
                "textDocument/didOpen",
                T,
                |p| p["textDocument"]["languageId"] == "jsonc"
            )
            .is_some()
    );
}

/// One of a document's servers crashing (ESLint) restarts it with the document replayed, as brief 0019's generic
/// server does, while the other one's diagnostics stay and an answer computed before the restart is dropped.
#[gpui::test]
fn a_crashed_eslint_restarts_and_typescripts_diagnostics_stay(cx: &mut TestAppContext) {
    let mut w = setup(cx, |w| {
        w.ts.diagnose_on_open(
            "main.ts",
            json!([{"range": range(2, 4, 9), "severity": 1, "code": 2322, "source": "ts",
                    "message": "Type 'string' is not assignable to type 'number'."}]),
        );
        w.eslint.diagnose_on_open(
            "main.ts",
            json!([{"range": range(2, 0, 3), "severity": 1, "code": "prefer-const", "source": "eslint",
                    "message": "'count' is never reassigned. Use 'const' instead."}]),
        );
    });
    let uri = w.uri("src/main.ts");
    let view = w.open("src/main.ts");
    w.wait("both servers' diagnostics", |w| {
        view.read_with(&w.vcx, |v, _| v.decorations(DIAGNOSTICS_LAYER).len()) == 2
    });
    let generation = |w: &Web| {
        let id = w.path("src/main.ts").to_string_lossy().into_owned();
        w.shell.read_with(&w.vcx, |s, _| s.doc_generation(&id))
    };
    let before = generation(&w);
    w.eslint.crash();
    w.wait("ESLint restarted", |w| w.eslint.connections() == 2);
    // Replayed: the document reaches the new ESLint process, and its diagnostics come back.
    w.eslint
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .unwrap();
    w.wait("the document's generation moved", |w| {
        generation(w) > before
    });
    w.wait("both servers' diagnostics again", |w| {
        view.read_with(&w.vcx, |v, _| v.decorations(DIAGNOSTICS_LAYER).len()) == 2
    });
    assert_eq!(w.ts.connections(), 1, "TypeScript kept running");
}

/// Copy `from` into `to`, leaving out `node_modules`.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let name = e.file_name();
        if name == "node_modules" {
            continue;
        }
        let target = to.join(&name);
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).unwrap();
        }
    }
}

impl Web {
    /// Run `f` (an agent's bus call, which waits for the UI thread) on its own thread while the UI runs.
    fn off_thread<R: Send + 'static>(&mut self, f: impl FnOnce() -> R + Send + 'static) -> R {
        let t = std::thread::spawn(f);
        let deadline = Instant::now() + eludite_test_support::hang_bound(T);
        loop {
            self.vcx.run_until_parked();
            if t.is_finished() {
                return t.join().unwrap();
            }
            assert!(Instant::now() < deadline, "the bus call did not answer");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn agent(&mut self, command: &'static str, args: Value) -> Value {
        let c = self.commands.clone();
        self.off_thread(move || c.invoke(command, args))
            .unwrap_or_else(|e| panic!("{command}: {e}"))
    }

    fn rows_with(&self, code: &str) -> Vec<super::error_list::ErrorRow> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.error_list()
                .read(cx)
                .rows()
                .iter()
                .filter(|r| r.code == code)
                .cloned()
                .collect()
        })
    }

    /// A completion request through the document's session (fanned out to its servers), as the editor sends it:
    /// the labels and how long the servers took.
    fn complete(&mut self, rel: &str, line: u32, character: u32) -> (Vec<String>, Duration) {
        let id = self.path(rel).to_string_lossy().into_owned();
        let uri = self.uri(rel);
        let session = self.shell.read_with(&self.vcx, |s, _| s.session_for(&id));
        let (_, rx) =
            session.request::<eludite_lsp::lsp::Completion>(eludite_lsp::lsp::CompletionParams {
                text_document: eludite_lsp::lsp::TextDocumentIdentifier { uri },
                position: eludite_lsp::lsp::Position { line, character },
                context: Some(eludite_lsp::lsp::CompletionContext {
                    trigger_kind: 1,
                    trigger_character: None,
                }),
            });
        let started = Instant::now();
        let reply = self.off_thread(move || futures::executor::block_on(rx).unwrap());
        let took = reply.sent.map_or(started.elapsed(), |s| reply.received - s);
        let labels = reply
            .result
            .unwrap_or_else(|e| panic!("completion: {e:?}"))
            .map(|r| r.items().iter().map(|i| i.label.clone()).collect())
            .unwrap_or_default();
        (labels, took)
    }
}

/// Brief 0050 with the real servers from `tools/web-servers/fetch.sh` (`ELUDITE_WEB_SERVERS`; skipped without it):
/// the Vite counter (corpus/web/vite-counter, `npm ci` for its own TypeScript, ESLint and Prettier) opened in the
/// headless shell: completion in `main.ts` from typescript-language-server on the project's TypeScript (cold and
/// warm), an agent fixing the type error (TS2552) and the ESLint violation (prefer-const) through `diagnostics.list`,
/// `code_actions` and `apply_code_action`, ESLint's diagnostics after a save, Prettier formatting a file (and a
/// 2,000-line one warm), the JSON server validating a `package.json` against the cached SchemaStore schema, and the
/// HTML server's completion in `index.html`. The timings are printed for the report (`REAL-WEB`).
#[gpui::test]
fn real_web_servers_serve_the_vite_counter(cx: &mut TestAppContext) {
    let Some(servers) = std::env::var_os("ELUDITE_WEB_SERVERS").map(PathBuf::from) else {
        eprintln!("skipped: ELUDITE_WEB_SERVERS is not set (tools/web-servers/fetch.sh prints it)");
        return;
    };
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/web");
    let dir = tempfile::Builder::new()
        .prefix("eludite-vite-")
        .tempdir()
        .unwrap();
    copy_tree(&corpus.join("vite-counter"), dir.path());
    copy_tree(
        &corpus.join("minimal-api/wwwroot/fixtures"),
        &dir.path().join("fixtures"),
    );
    // 2,000 lines for Prettier's warm budget.
    let mut big = String::new();
    for i in 0..1000 {
        big.push_str(&format!(
            "export   const   value{i}   =   {{ a:{i},b :'x' }}\n"
        ));
        big.push_str(&format!(
            "export function   f{i}( n:number ){{ return n+{i} }}\n"
        ));
    }
    std::fs::write(dir.path().join("src/big.ts"), &big).unwrap();
    let npm = std::process::Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
        .args([
            "ci",
            "--no-audit",
            "--no-fund",
            "--ignore-scripts",
            "--loglevel=error",
        ])
        .current_dir(dir.path())
        .output()
        .expect("npm runs (Node.js on PATH)");
    assert!(
        npm.status.success(),
        "npm ci: {}",
        String::from_utf8_lossy(&npm.stderr)
    );
    let cache = tempfile::tempdir().unwrap();
    let mut w = launch(cx, dir, cache, Some(servers), false, |_| {});
    let mut report = Vec::new();

    // TypeScript: completion in main.ts, on the project's TypeScript 5.9.3.
    let opened = Instant::now();
    w.open("src/main.ts");
    let line = "setupCounter(document.querySelector<HTMLButtonElement>(\"#counter\")!);";
    let at = line.find("querySelector").unwrap() as u32;
    let (labels, _) = w.complete("src/main.ts", 4, at);
    let cold = opened.elapsed();
    assert!(labels.iter().any(|l| l == "querySelector"), "{labels:?}");
    let mut warm = Vec::new();
    for _ in 0..20 {
        let (labels, took) = w.complete("src/main.ts", 4, at);
        assert!(labels.iter().any(|l| l == "querySelector"));
        warm.push(took);
    }
    warm.sort();
    let warm_p95 = warm[warm.len() * 95 / 100];
    report.push(format!(
        "first completion {cold:?} cold from open, warm p95 {warm_p95:?}"
    ));
    w.wait("TypeScript on the project's TypeScript", |w| {
        w.slot("typescript").contains("TypeScript 5.9.3, project")
    });
    report.push(w.slot("typescript"));
    report.push(w.slot("eslint"));

    // An agent fixes the type error: diagnostics.list, code_actions at the row, apply_code_action.
    let type_error = w.path("src/typeError.ts").to_string_lossy().into_owned();
    w.open("src/typeError.ts");
    w.wait("TS2552", |w| !w.rows_with("2552").is_empty());
    let listed = w.agent(eludite_commands::diagnostics::DIAGNOSTICS_LIST, json!({}));
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["code"] == "2552")
        .cloned()
        .unwrap_or_else(|| panic!("{listed}"));
    let actions = w.agent(
        workspace::EDITOR_CODE_ACTIONS,
        json!({"path": type_error, "line": row["line"], "column": row["column"]}),
    );
    let index = actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["title"]
                .as_str()
                .is_some_and(|t| t.contains("Change spelling to 'count'"))
        })
        .map(|a| a["index"].clone())
        .unwrap_or_else(|| panic!("{actions}"));
    let applied = w.agent(workspace::EDITOR_APPLY_CODE_ACTION, json!({"index": index}));
    assert_eq!(applied["state"], "applied", "{applied}");
    let fixed = w.shell.read_with(&w.vcx, |s, cx| {
        s.editor(&w.path("src/typeError.ts"))
            .unwrap()
            .read(cx)
            .editor()
            .text()
    });
    assert!(fixed.contains("= count * 2"), "{fixed}");

    // ESLint: the violation in lint.js, fixed by an agent through ESLint's command.
    let lint = w.path("src/lint.js").to_string_lossy().into_owned();
    let lint_view = w.open("src/lint.js");
    w.wait("ESLint's prefer-const", |w| {
        !w.rows_with("prefer-const").is_empty()
    });
    let row = w.rows_with("prefer-const")[0].clone();
    let actions = w.agent(
        workspace::EDITOR_CODE_ACTIONS,
        json!({"path": lint, "line": row.line, "column": row.column}),
    );
    let titles: Vec<String> = actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        titles.iter().any(|t| t == "Fix all auto-fixable problems"),
        "{titles:?}"
    );
    let index = actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Fix this prefer-const problem")
        .map(|a| a["index"].clone())
        .unwrap_or_else(|| panic!("{titles:?}"));
    let applied = w.agent(workspace::EDITOR_APPLY_CODE_ACTION, json!({"index": index}));
    assert_eq!(applied["state"], "applied", "{applied}");
    w.wait("the ESLint fix", |w| {
        w.text(&lint_view).contains("const total = 1;")
    });
    w.wait("the violation gone", |w| {
        w.rows_with("prefer-const").is_empty()
    });
    // A new violation, saved: ESLint's diagnostics after the save.
    w.type_at_end_of_line(&lint_view, 3, "\nlet again = 2;\nconsole.log(again);");
    w.run_cmd(workspace::EDITOR_SAVE, json!({"path": lint}));
    let saved = Instant::now();
    w.wait("ESLint's diagnostic after the save", |w| {
        !w.rows_with("prefer-const").is_empty()
    });
    let eslint_after_save = saved.elapsed();
    report.push(format!(
        "ESLint diagnostics {eslint_after_save:?} after a save"
    ));

    // Prettier (the project's 3.9.9 through Node.js).
    let unformatted = w.path("src/unformatted.ts").to_string_lossy().into_owned();
    let view = w.open("src/unformatted.ts");
    w.run_cmd(
        workspace::EDITOR_FORMAT_DOCUMENT,
        json!({"path": unformatted}),
    );
    let out = w.format_done("src/unformatted.ts");
    assert_eq!(
        (out.formatter.as_str(), out.version.as_deref(), out.applied),
        ("prettier", Some("3.9.9"), true),
        "{out:?}"
    );
    let text = w.text(&view);
    assert!(
        text.contains("export function label(name: string, count: number) {"),
        "{text}"
    );
    assert!(text.contains("export const values = [1, 2, 3];"), "{text}");
    // 2,000 lines, Prettier warm (it just ran): request to applied.
    let big_path = w.path("src/big.ts").to_string_lossy().into_owned();
    let big_view = w.open("src/big.ts");
    let mut times = Vec::new();
    for _ in 0..5 {
        w.run_cmd(workspace::EDITOR_FORMAT_DOCUMENT, json!({"path": big_path}));
        let out = w.format_done("src/big.ts");
        assert!(out.applied, "{out:?}");
        times.push(
            w.shell
                .read_with(&w.vcx, |s, _| *s.formatting.timings.last().unwrap()),
        );
        w.run_cmd(workspace::EDITOR_UNDO, json!({"path": big_path}));
        assert_eq!(w.text(&big_view), big);
    }
    report.push(format!(
        "Format Document on 2,000 lines with Prettier: {times:?}"
    ));

    // JSON: SchemaStore's package.json schema, from the cache, flags `"private": "yes"`.
    w.open("fixtures/package.json");
    w.wait("the schema's diagnostic", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.error_list()
                .read(cx)
                .rows()
                .iter()
                .any(|r| r.file == "package.json" && r.message.contains("boolean"))
        })
    });
    report.push(w.slot("json"));

    // HTML: completion in index.html's body.
    let html = w.open("index.html");
    w.type_at_end_of_line(&html, 7, "\n    <");
    let (labels, _) = w.complete("index.html", 8, 5);
    assert!(labels.iter().any(|l| l == "div"), "{labels:?}");
    report.push(w.slot("html"));
    for line in &report {
        eprintln!("REAL-WEB: {line}");
    }
    super::tests::assert_budget("cold completion", cold, Duration::from_secs(10));
}
