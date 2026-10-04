//! Headless tests of brief 0048 against the fake host's NuGet service (`eludite-lsp`'s `fake_nuget.rs`): the Manage
//! NuGet Packages window opened from a project's context menu (scoped to it) and from the solution's (every project),
//! Browse with its 300 ms debounce, results and the detail pane, Install with the restore, the Output's Package Manager
//! lines and the Dependencies node updated, Installed with versions and the badge, Updates and Update, Consolidate
//! across two projects, Uninstall, a failed restore's Error List rows, the prerelease check box, a source failure row,
//! the Options page's Package Sources, the credential prompt for the person and its refusal for an agent, agents'
//! search, installed and install matching the window with install asking under the policy's `prompt`, nothing reaching
//! the host's NuGet service at startup, the Dependencies node with its transitive packages, and 500 results drawn
//! virtualized within the frame budget.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_commands::nuget::{self as cmds, Tab};
use eludite_commands::{Caller, CommandSpec, PermissionClass, with_caller};
use eludite_docking::ids;
use eludite_mcp::{CallContext, GateDecision, McpServer};
use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext};
use serde_json::{Value, json};

use super::explorer::{context_item_selector, row_selector};
use super::nuget::window::{self, Row, Scope};
use super::nuget::{AGENT_CANNOT_ANSWER, SEARCH_DEBOUNCE, credentials, sources_page};
use super::tests::{Ws, setup};

const GREETER: &str = "Eludite.Corpus.Greeter";
const LOGGING: &str = "Eludite.Corpus.Logging";

fn agent() -> Caller {
    Caller::Agent {
        agent: "test-agent".into(),
        call: 1,
        tool_call: None,
    }
}

struct N {
    w: Ws,
    app: String,
    lib: String,
}

/// The fake's sources, packages and projects: App with Greeter 1.0.0 (a known vulnerability) and a reference to Lib,
/// Lib with Greeter 1.1.0.
fn script(app: &str, lib: &str, extra_sources: Value) -> Value {
    let mut sources = vec![json!({"name": "corpus", "url": "/feed"})];
    sources.extend(extra_sources.as_array().cloned().unwrap_or_default());
    json!({
        "sources": sources,
        "packages": [
            {"source": "corpus", "id": GREETER, "versions": ["1.0.0", "1.1.0", "2.0.0-beta.1"],
             "description": "Greets people, for Eludite's NuGet tests.",
             "iconUrl": "https://icons.invalid/greeter.png",
             "dependencies": [{"id": LOGGING, "version": "[1.0.0, )"}],
             "vulnerable": {"version": "1.0.0", "severity": "high", "advisoryUrl": "https://github.com/advisories/GHSA-0048"},
             "deprecated": {"version": "1.0.0", "message": "Use 1.1.0"}},
            {"source": "corpus", "id": LOGGING, "versions": ["1.0.0"], "description": "Logs."}
        ],
        "projects": [
            {"path": app, "name": "App", "packages": [{"id": GREETER, "version": "1.0.0"}]},
            {"path": lib, "name": "Lib", "packages": [{"id": GREETER, "version": "1.1.0"}]}
        ]
    })
}

fn setup_nuget(cx: &mut TestAppContext) -> N {
    setup_sources(cx, json!([]))
}

fn setup_sources(cx: &mut TestAppContext, extra: Value) -> N {
    let w = setup(cx);
    let app = w.path("src/App/App.csproj").to_string_lossy().into_owned();
    let lib_path = w.path("src/Lib/Lib.csproj");
    std::fs::create_dir_all(lib_path.parent().unwrap()).unwrap();
    std::fs::write(&lib_path, "<Project Sdk=\"Microsoft.NET.Sdk\" />").unwrap();
    let shapes = w.path("src/Lib/Shapes.cs");
    std::fs::write(&shapes, "class Shapes { }\n").unwrap();
    let lib = lib_path.to_string_lossy().into_owned();
    let program = w.path("src/App/Program.cs").to_string_lossy().into_owned();
    w.fake.set_tree(json!([
        {"name": "App", "path": app, "kind": "sdk", "targetFrameworks": ["net10.0"],
         "files": [{"path": program, "itemType": "compile"}],
         "dependencies": {"restored": true, "packages": [], "projects": [{"name": "Lib", "path": lib}],
                          "frameworks": [{"name": "Microsoft.NETCore.App", "targetFramework": "net10.0"}]}},
        {"name": "Lib", "path": lib, "kind": "sdk", "targetFrameworks": ["net10.0"],
         "files": [{"path": shapes.to_string_lossy(), "itemType": "compile"}]}
    ]));
    w.fake.set_nuget(script(&app, &lib, extra));
    let mut n = N { w, app, lib };
    n.w.open_solution();
    n
}

impl N {
    fn read<R>(&self, f: impl FnOnce(&window::NuGetWindow) -> R) -> R {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, cx| f(s.nuget().window.read(cx)))
    }

    fn update(
        &mut self,
        f: impl FnOnce(&mut window::NuGetWindow, &mut gpui::Context<window::NuGetWindow>),
    ) {
        self.w.shell.update(&mut self.w.vcx, |s, cx| {
            s.nuget().window.clone().update(cx, |w, cx| f(w, cx))
        });
        self.w.vcx.run_until_parked();
    }

    /// Run a command as the UI does.
    fn ui(&mut self, command: &str, args: Value) {
        let command = command.to_owned();
        self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.run(&command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
    }

    /// Invoke a command as an agent, from another thread.
    fn agent(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let commands = self.w.commands.clone();
        let command = command.to_owned();
        let t = std::thread::spawn(move || {
            with_caller(agent(), || commands.invoke(&command, args)).map_err(|e| e.to_string())
        });
        while !t.is_finished() {
            self.w.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        t.join().unwrap()
    }

    fn rows(&self) -> Vec<Row> {
        self.read(|w| w.rows().to_vec())
    }

    fn ids(&self) -> Vec<String> {
        self.rows()
            .iter()
            .filter_map(|r| r.id().map(str::to_owned))
            .collect()
    }

    fn wait_rows(&mut self, what: &str, f: impl Fn(&[Row]) -> bool) {
        self.w.wait(what, |w| {
            w.shell
                .read_with(&w.vcx, |s, cx| f(s.nuget().window.read(cx).rows()))
        });
    }

    /// Wait until no call of the window is running.
    fn wait_idle(&mut self) {
        self.w.wait("the window's calls", |w| {
            w.shell.read_with(&w.vcx, |s, cx| {
                s.nuget()
                    .window
                    .read(cx)
                    .status()
                    .is_none_or(|t| !t.ends_with('\u{2026}'))
            })
        });
    }

    fn open_tab(&mut self, tab: Tab) {
        self.w.click(&window::tab_selector(tab));
        self.wait_idle();
    }

    fn select(&mut self, id: &str) {
        let ix = self
            .rows()
            .iter()
            .position(|r| r.id() == Some(id))
            .unwrap_or_else(|| panic!("no row for {id}: {:?}", self.rows()));
        self.w.click(&window::row_selector(ix));
    }

    fn details(&self) -> Vec<String> {
        self.read(|w| w.details())
    }

    fn package_manager(&self) -> String {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            let pane = s
                .output
                .read(cx)
                .pane(eludite_commands::build::OutputSource::PackageManager);
            (0..pane.len())
                .filter_map(|i| pane.line(i).map(str::to_owned))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    fn fake_packages(&self, project: &str) -> Vec<(String, String)> {
        self.w
            .fake
            .nuget_projects()
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["path"] == project)
            .unwrap()["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["id"].as_str().unwrap().to_owned(),
                    p["version"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    fn right_click(&mut self, sel: &str) {
        let at = self.w.bounds(sel).center();
        self.w.vcx.simulate_event(MouseDownEvent {
            position: at,
            modifiers: Modifiers::none(),
            button: MouseButton::Right,
            click_count: 1,
            first_mouse: false,
        });
        self.w.vcx.run_until_parked();
    }

    fn type_query(&mut self, text: &str) {
        self.w.click(window::SEARCH_BOX);
        let keys: Vec<String> = text
            .chars()
            .map(|c| match c {
                ' ' => "space".to_owned(),
                '.' => "period".to_owned(),
                c => c.to_string(),
            })
            .collect();
        self.w.vcx.simulate_keystrokes(&keys.join(" "));
        self.w.vcx.run_until_parked();
    }

    fn searches(&self) -> Vec<Value> {
        self.w.fake.received_params("eludite/nuget/search")
    }
}

#[gpui::test]
fn nothing_reaches_the_nuget_service_at_startup_and_the_dependencies_node_shows_with_its_transitive_packages(
    cx: &mut TestAppContext,
) {
    let mut n = setup_nuget(cx);
    // The solution is open, its tree drawn: no NuGet call was made.
    std::thread::sleep(Duration::from_millis(50));
    n.w.vcx.run_until_parked();
    assert_eq!(n.w.fake.nuget_calls(), 0);
    assert!(
        n.w.fake
            .received()
            .iter()
            .all(|r| !r.method.starts_with("eludite/nuget/")),
        "no eludite/nuget/* request at startup"
    );
    // The Dependencies node: Frameworks, Packages (with what a package brings in) and Projects.
    let app = n.app.clone();
    for id in [
        app.clone(),
        format!("{app}|deps"),
        format!("{app}|deps|frameworks"),
        format!("{app}|deps|packages"),
        format!("{app}|deps|packages|{GREETER}"),
        format!("{app}|deps|projects"),
    ] {
        n.w.click(&format!("{}-toggle", row_selector(&id)));
    }
    let labels = n.w.row_labels();
    let at = labels
        .iter()
        .position(|l| l.trim() == "Dependencies")
        .unwrap();
    assert_eq!(
        labels[at..at + 8]
            .iter()
            .map(|l| l.trim().to_owned())
            .collect::<Vec<_>>(),
        [
            "Dependencies",
            "Frameworks",
            "Microsoft.NETCore.App",
            "Packages",
            "Eludite.Corpus.Greeter (1.0.0)",
            "Eludite.Corpus.Logging ([1.0.0, ))",
            "Projects",
            "Lib",
        ]
    );
    // Greeter 1.0.0 has a known vulnerability (NuGet Audit in the tree): the yellow glyph.
    let warned = n.w.shell.read_with(&n.w.vcx, |s, cx| {
        let e = s.explorer().read(cx);
        e.rows()
            .iter()
            .filter(|r| e.package_warning(r))
            .map(|r| r.label.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(warned, ["Eludite.Corpus.Greeter (1.0.0)"]);
    // eludite.workspace.tree has the same node for agents.
    let tree =
        n.w.commands
            .invoke("eludite.workspace.tree", json!({}))
            .unwrap();
    let app_tree = tree["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "App")
        .unwrap();
    assert_eq!(app_tree["dependency_tree"]["packages"][0]["id"], GREETER);
    assert_eq!(
        app_tree["dependency_tree"]["packages"][0]["vulnerable"],
        true
    );
    assert_eq!(
        app_tree["dependency_tree"]["packages"][0]["transitive"][0],
        "Eludite.Corpus.Logging/[1.0.0, )"
    );
    assert_eq!(app_tree["dependency_tree"]["projects"][0], n.lib);
    assert_eq!(n.w.fake.nuget_calls(), 0);
}

#[gpui::test]
fn the_window_opens_scoped_to_a_project_from_its_context_menu_and_to_every_project_from_the_solutions(
    cx: &mut TestAppContext,
) {
    let mut n = setup_nuget(cx);
    // The project's context menu: Manage NuGet Packages... scoped to App.
    n.right_click(&row_selector(&n.app));
    n.w.click(&context_item_selector("nuget"));
    n.wait_idle();
    assert_eq!(
        n.read(|w| w.scope().clone()),
        Scope::Project {
            name: "App".into(),
            path: n.app.clone()
        }
    );
    let docs = n.w.controller.snapshot().layout.documents;
    assert_eq!(docs.active.as_deref(), Some(ids::NUGET));
    assert_eq!(docs.get(ids::NUGET).unwrap().title, "NuGet: App");
    // Opening read the installed packages (and the sources for the dropdown), and searched Browse once.
    n.wait_rows("Browse's results", |r| !r.is_empty());
    assert_eq!(n.searches().len(), 1);
    assert_eq!(
        n.w.fake.received_params("eludite/nuget/installed")[0]["projects"],
        json!([n.app])
    );
    // The solution's context menu: Manage NuGet Packages for Solution..., every project.
    n.right_click(&row_selector(&n.w.path("App.slnx").to_string_lossy()));
    n.w.click(&context_item_selector("nuget_solution"));
    n.wait_idle();
    assert_eq!(n.read(|w| w.scope().clone()), Scope::Solution);
    let docs = n.w.controller.snapshot().layout.documents;
    assert_eq!(docs.get(ids::NUGET).unwrap().title, "NuGet - Solution");
    assert_eq!(docs.tabs.iter().filter(|t| t.id == ids::NUGET).count(), 1);
    assert_eq!(
        n.w.fake
            .received_params("eludite/nuget/installed")
            .last()
            .unwrap()
            .get("projects"),
        None
    );
    // Both are audited as the user's eludite.nuget.manage.
    assert!(n.w.audit().iter().filter(|c| *c == cmds::MANAGE).count() >= 2);
}

#[gpui::test]
fn browse_searches_after_the_debounce_with_results_the_detail_pane_prerelease_and_a_failed_source(
    cx: &mut TestAppContext,
) {
    let mut n = setup_sources(
        cx,
        json!([{"name": "down", "url": "https://down.invalid/v3/index.json", "failing": true}]),
    );
    n.w.fake.set_nuget_icon(
        "https://icons.invalid/greeter.png",
        "/tmp/eludite-test-greeter.png",
    );
    n.ui(cmds::MANAGE, json!({ "solution": true }));
    n.wait_idle();
    let before = n.searches().len();
    n.type_query("Greet");
    // Nothing until 300 ms after the last key; then one search with the whole text.
    n.w.vcx
        .executor()
        .advance_clock(SEARCH_DEBOUNCE - Duration::from_millis(1));
    n.w.vcx.run_until_parked();
    assert_eq!(n.searches().len(), before, "no search before the debounce");
    n.w.vcx.executor().advance_clock(Duration::from_millis(1));
    n.w.vcx.run_until_parked();
    n.w.wait("the debounced search", |w| {
        w.fake.received_params("eludite/nuget/search").len() == before + 1
    });
    assert_eq!(n.searches().last().unwrap()["query"], "Greet");
    n.wait_rows("the results", |r| r.iter().any(|x| x.id() == Some(GREETER)));
    // The failed source's error row first, then the package with the newest stable version.
    let rows = n.rows();
    assert!(
        matches!(&rows[0], Row::SourceError(name, why) if name == "down" && why.contains("Unable to load")),
        "{rows:?}"
    );
    assert!(
        matches!(&rows[1], Row::Package { id, version, .. } if id == GREETER && version == "1.1.0")
    );
    n.wait_idle();
    assert!(
        n.read(|w| w.status())
            .unwrap()
            .contains("1 package source(s) failed")
    );
    // The icon is fetched by the host for the rows that show.
    n.w.wait("the icon", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.nuget()
                .window
                .read(cx)
                .icon("https://icons.invalid/greeter.png")
                .is_some()
        })
    });
    // The detail pane: versions, the description, the projects with their versions.
    n.select(GREETER);
    let details = n.details();
    assert_eq!(details[0], GREETER);
    assert_eq!(details[1], "Version: 1.1.0");
    assert!(
        details.contains(&"Installed: App 1.0.0, Lib 1.1.0".to_owned()),
        "{details:?}"
    );
    assert!(details.contains(&"Greets people, for Eludite's NuGet tests.".to_owned()));
    assert!(
        details
            .iter()
            .any(|l| l
                .contains("High severity vulnerability: https://github.com/advisories/GHSA-0048")),
        "{details:?}"
    );
    // Include prerelease: searched again at once, the beta first.
    n.w.click(window::PRERELEASE);
    n.w.wait("the prerelease search", |w| {
        w.fake
            .received_params("eludite/nuget/search")
            .last()
            .is_some_and(|p| p["prerelease"] == true)
    });
    n.wait_rows("the beta", |r| {
        r.iter()
            .any(|x| matches!(x, Row::Package { version, .. } if version == "2.0.0-beta.1"))
    });
    n.w.click(window::VERSION_BUTTON);
    let versions = n.read(|w| w.versions(GREETER));
    assert_eq!(versions, ["2.0.0-beta.1", "1.1.0", "1.0.0"]);
    // The source dropdown: corpus only.
    n.w.click(window::SOURCE_BUTTON);
    n.w.click(&window::source_item_selector(1));
    n.w.wait("the corpus search", |w| {
        w.fake
            .received_params("eludite/nuget/search")
            .last()
            .is_some_and(|p| p["source"] == "corpus")
    });
    n.wait_rows("no error row", |r| {
        !r.is_empty() && r.iter().all(|x| !matches!(x, Row::SourceError(..)))
    });
}

#[gpui::test]
fn install_edits_restores_and_updates_the_output_and_the_tree(cx: &mut TestAppContext) {
    let mut n = setup_nuget(cx);
    n.ui(cmds::MANAGE, json!({ "project": "Lib" }));
    n.wait_rows("Browse's results", |r| {
        r.iter().any(|x| x.id() == Some(LOGGING))
    });
    let generation = n.w.fake.generation();
    n.select(LOGGING);
    assert_eq!(n.read(|w| w.actions()), (true, false, false, false));
    n.w.click(window::INSTALL);
    n.w.wait("the install", |w| {
        w.fake
            .received_params("eludite/nuget/change")
            .iter()
            .any(|p| p["action"] == "install")
    });
    n.wait_idle();
    let change =
        n.w.fake
            .received_params("eludite/nuget/change")
            .pop()
            .unwrap();
    assert_eq!(
        change["packages"],
        json!([{"id": LOGGING, "version": "1.0.0"}])
    );
    assert_eq!(change["projects"], json!([n.lib]));
    assert_eq!(
        change["restore"], true,
        "nuget.restoreOnChange is on by default"
    );
    assert_eq!(change["interactive"], true);
    assert_eq!(change["lockFiles"], "respect");
    assert!(
        n.fake_packages(&n.lib)
            .contains(&(LOGGING.to_owned(), "1.0.0".to_owned()))
    );
    assert_eq!(n.w.fake.generation(), generation + 1);
    // The Output window's Package Manager source shows what NuGet did.
    let output = n.package_manager();
    assert!(
        output.contains("Installing NuGet package Eludite.Corpus.Logging 1.0.0 in Lib."),
        "{output}"
    );
    assert!(output.contains("Restore succeeded"), "{output}");
    assert!(output.contains("========== Finished =========="));
    // The tree follows under the new generation: Lib's Packages node lists Logging.
    let lib = n.lib.clone();
    n.w.wait("the refreshed tree", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.explorer()
                .read(cx)
                .model()
                .and_then(|m| m.find(&format!("{lib}|deps|packages|{LOGGING}")).cloned())
                .is_some()
        })
    });
    // The window read Installed again: Logging is installed in Lib now.
    n.w.wait("Installed again", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.nuget()
                .window
                .read(cx)
                .placements(LOGGING)
                .iter()
                .any(|p| p.version.as_deref() == Some("1.0.0"))
        })
    });
    assert!(n.read(|w| w.status()).unwrap().contains("done"));
    // Audited with its arguments, the person's call too.
    let entry =
        n.w.commands
            .audit_log()
            .entries()
            .into_iter()
            .rev()
            .find(|e| e.command == cmds::INSTALL)
            .unwrap();
    assert_eq!(entry.arguments.as_ref().unwrap()["package"], LOGGING);
    assert_eq!(entry.arguments.as_ref().unwrap()["version"], "1.0.0");
}

#[gpui::test]
fn installed_updates_consolidate_and_uninstall(cx: &mut TestAppContext) {
    let mut n = setup_nuget(cx);
    n.ui(
        cmds::MANAGE,
        json!({ "solution": true, "tab": "installed" }),
    );
    n.wait_rows("Installed", |r| !r.is_empty());
    // Installed: Greeter with both versions and the badge (App's 1.0.0 is vulnerable and deprecated).
    let rows = n.rows();
    assert!(
        matches!(&rows[..], [Row::Package { id, detail, warning: true, .. }] if id == GREETER && detail == "Installed: 1.0.0, 1.1.0"),
        "{rows:?}"
    );
    // The sources' data came with it (metadata): the deprecation line in the detail pane.
    n.select(GREETER);
    assert!(
        n.details()
            .iter()
            .any(|l| l.contains("1.0.0 is deprecated")),
        "{:?}",
        n.details()
    );
    // Consolidate: Greeter on 1.0.0 and 1.1.0, 1.1.0 by default.
    n.open_tab(Tab::Consolidate);
    let rows = n.rows();
    assert!(
        matches!(&rows[..], [Row::Package { id, version, .. }] if id == GREETER && version == "1.1.0")
    );
    n.select(GREETER);
    assert_eq!(
        n.read(|w| w.selected().map(str::to_owned)),
        Some(GREETER.to_owned())
    );
    assert_eq!(
        n.read(|w| w.checked_projects().len()),
        2,
        "both projects checked at first"
    );
    n.w.click(&window::project_check_selector("Lib"));
    assert_eq!(n.read(|w| w.target_projects()), [n.app.clone()]);
    assert_eq!(n.read(|w| w.actions()), (false, false, false, true));
    n.w.click(window::CONSOLIDATE);
    n.w.wait("the consolidation", |w| {
        w.fake
            .received_params("eludite/nuget/change")
            .iter()
            .any(|p| p["packages"][0]["version"] == "1.1.0")
    });
    n.wait_idle();
    assert!(
        n.fake_packages(&n.app)
            .contains(&(GREETER.to_owned(), "1.1.0".to_owned()))
    );
    n.w.wait("nothing left to consolidate", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.nuget().window.read(cx).consolidate_rows().is_empty()
        })
    });
    // Updates: Greeter 1.1.0 to 2.0.0-beta.1 with prerelease; Select all, Update.
    n.w.click(window::PRERELEASE);
    n.open_tab(Tab::Updates);
    n.wait_rows("the updates", |r| r.iter().any(|x| x.id() == Some(GREETER)));
    n.w.click(window::SELECT_ALL);
    assert_eq!(
        n.read(|w| w.checked_updates().iter().cloned().collect::<Vec<_>>()),
        [GREETER]
    );
    n.w.click(window::UPDATE_SELECTED);
    n.w.wait("the update", |w| {
        w.fake
            .received_params("eludite/nuget/change")
            .iter()
            .any(|p| p["action"] == "update" && p["packages"][0]["version"] == "2.0.0-beta.1")
    });
    n.wait_idle();
    assert!(
        n.fake_packages(&n.app)
            .contains(&(GREETER.to_owned(), "2.0.0-beta.1".to_owned()))
    );
    assert!(
        n.fake_packages(&n.lib)
            .contains(&(GREETER.to_owned(), "2.0.0-beta.1".to_owned()))
    );
    n.wait_rows("no updates left", |r| r.is_empty());
    // Uninstall from Lib only (App unchecked).
    n.open_tab(Tab::Installed);
    n.select(GREETER);
    n.w.click(&window::project_check_selector("App"));
    n.w.click(window::UNINSTALL);
    n.w.wait("the uninstall", |w| {
        w.fake
            .received_params("eludite/nuget/change")
            .iter()
            .any(|p| p["action"] == "uninstall")
    });
    n.wait_idle();
    assert!(n.fake_packages(&n.lib).is_empty());
    assert_eq!(n.fake_packages(&n.app).len(), 1);
}

#[gpui::test]
fn a_failed_restore_leaves_the_edit_and_error_list_rows_with_click_through(
    cx: &mut TestAppContext,
) {
    let mut n = setup_nuget(cx);
    let lib = n.lib.clone();
    n.w.fake.set_nuget_restore_failure(Some(json!({
        "severity": "error", "code": "NU1102", "file": lib,
        "message": "Unable to find package Eludite.Corpus.Greeter with version (>= 7.0.0)"
    })));
    let out = n
        .agent(
            cmds::UPDATE,
            json!({ "package": GREETER, "version": "1.1.0", "project": "App" }),
        )
        .unwrap();
    // The edit stays; the answer and the Error List carry the restore's error at the project file.
    assert_eq!(out["restore"]["result"], "failed");
    assert_eq!(out["restore"]["errors"], 1);
    assert!(
        n.fake_packages(&n.app)
            .contains(&(GREETER.to_owned(), "1.1.0".to_owned()))
    );
    n.w.wait("the NuGet row", |w| {
        w.error_rows().iter().any(|r| {
            r.code == "NU1102" && r.source == eludite_commands::diagnostics::RowSource::NuGet
        })
    });
    let row =
        n.w.error_rows()
            .into_iter()
            .find(|r| r.code == "NU1102")
            .unwrap();
    assert_eq!(row.path, PathBuf::from(&lib));
    assert_eq!(row.project.as_deref(), Some("Lib"));
    let listed =
        n.w.commands
            .invoke(
                eludite_commands::diagnostics::DIAGNOSTICS_LIST,
                json!({ "source": "nuget" }),
            )
            .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["source"], "nuget");
    // A restore that succeeds clears them.
    n.w.fake.set_nuget_restore_failure(None);
    n.agent(cmds::RESTORE, json!({})).unwrap();
    n.w.wait("no NuGet rows", |w| {
        w.error_rows().iter().all(|r| r.code != "NU1102")
    });
}

#[gpui::test]
fn the_options_page_lists_and_edits_the_package_sources(cx: &mut TestAppContext) {
    let mut n = setup_nuget(cx);
    n.ui(
        eludite_commands::settings::OPTIONS,
        json!({ "section": eludite_commands::settings::PACKAGE_SOURCES_PAGE }),
    );
    let page =
        n.w.shell
            .read_with(&n.w.vcx, |s, _| s.nuget().sources_page.clone());
    n.w.wait("the sources", |w| {
        page.read_with(&w.vcx, |p, _| p.sources().is_some())
    });
    assert_eq!(
        n.w.shell.read_with(&n.w.vcx, |s, cx| s
            .options
            .as_ref()
            .unwrap()
            .read(cx)
            .section()
            .to_owned()),
        eludite_commands::settings::PACKAGE_SOURCES_PAGE
    );
    // Add a source: the boxes, then Add.
    page.update(&mut n.w.vcx, |p, cx| {
        p.set_boxes("mine", "https://mine.invalid/v3/index.json", cx)
    });
    n.w.click(sources_page::ADD);
    n.w.wait("the added source", |w| {
        page.read_with(&w.vcx, |p, _| {
            p.sources()
                .is_some_and(|s| s.sources.iter().any(|x| x.name == "mine"))
        })
    });
    assert!(
        n.w.fake
            .nuget_sources()
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "mine")
    );
    // Disable it with its check box, then remove it.
    n.w.click(&sources_page::enabled_selector("mine"));
    n.w.wait("the disabled source", |w| {
        page.read_with(&w.vcx, |p, _| {
            p.sources()
                .is_some_and(|s| s.sources.iter().any(|x| x.name == "mine" && !x.enabled))
        })
    });
    n.w.click(&sources_page::source_selector("mine"));
    n.w.click(sources_page::REMOVE);
    n.w.wait("the removed source", |w| {
        page.read_with(&w.vcx, |p, _| {
            p.sources()
                .is_some_and(|s| !s.sources.iter().any(|x| x.name == "mine"))
        })
    });
    let actions: Vec<Value> =
        n.w.fake
            .received_params("eludite/nuget/sources")
            .iter()
            .map(|p| p["action"].clone())
            .collect();
    assert_eq!(
        actions,
        [
            json!("list"),
            json!("add"),
            json!("disable"),
            json!("remove")
        ]
    );
}

#[gpui::test]
fn the_credential_prompt_asks_the_person_and_an_agent_is_refused(cx: &mut TestAppContext) {
    let mut n = setup_sources(
        cx,
        json!([{"name": "private", "url": "https://feed.invalid:8443/v3/index.json", "private": true}]),
    );
    let mut s = script(
        &n.app,
        &n.lib,
        json!([{"name": "private", "url": "https://feed.invalid:8443/v3/index.json", "private": true}]),
    );
    s["credentials"] = json!(["alice", "s3cret"]);
    s["packages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source": "private", "id": "Private.Package", "versions": ["3.0.0"]}));
    n.w.fake.set_nuget(s);
    // An agent: refused with credentials_required and the host; no prompt.
    let e = n
        .agent(
            cmds::SEARCH,
            json!({ "query": "Private", "source": "private" }),
        )
        .unwrap_err();
    assert!(e.contains("credentials_required: feed.invalid:8443"), "{e}");
    assert!(e.contains(AGENT_CANNOT_ANSWER), "{e}");
    assert!(
        n.w.shell
            .read_with(&n.w.vcx, |s, _| s.nuget().prompt.is_none())
    );
    // The person's search: the prompt, for that source and host.
    n.ui(
        cmds::MANAGE,
        json!({ "solution": true, "query": "Private" }),
    );
    n.w.wait("the prompt", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.nuget().prompt.is_some())
    });
    let prompt = n.w.shell.read_with(&n.w.vcx, |s, _| {
        s.nuget().prompt.as_ref().unwrap().1.clone()
    });
    prompt.read_with(&n.w.vcx, |p, _| {
        assert_eq!(p.source(), "private");
        assert_eq!(p.host(), "feed.invalid:8443");
        assert!(!p.refused());
    });
    n.w.click(credentials::USER_BOX);
    n.w.vcx
        .simulate_keystrokes("a l i c e tab s 3 c r e t enter");
    n.w.vcx.run_until_parked();
    n.wait_rows("the private package", |r| {
        r.iter().any(|x| x.id() == Some("Private.Package"))
    });
    assert!(
        n.w.shell
            .read_with(&n.w.vcx, |s, _| s.nuget().prompt.is_none())
    );
    // Kept for the session: the agent's search now gets through.
    let out = n
        .agent(
            cmds::SEARCH,
            json!({ "query": "Private", "source": "private" }),
        )
        .unwrap();
    assert_eq!(out["results"][0]["id"], "Private.Package");
}

/// A call the gate was asked about: the command, its class and why it was raised.
type Asked = (String, PermissionClass, Option<String>);

#[gpui::test]
fn an_agents_search_installed_and_install_match_the_window_and_install_asks(
    cx: &mut TestAppContext,
) {
    let mut n = setup_nuget(cx);
    n.ui(cmds::MANAGE, json!({ "solution": true }));
    n.wait_rows("Browse's results", |r| r.len() == 2);
    let window_ids = n.ids();
    let search = n.agent(cmds::SEARCH, json!({})).unwrap();
    let agent_ids: Vec<String> = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(agent_ids, window_ids);
    assert_eq!(search["results"][0]["versions"], json!(["1.1.0", "1.0.0"]));
    let interactive: Vec<Value> = n
        .searches()
        .iter()
        .map(|p| p["interactive"].clone())
        .collect();
    assert_eq!(
        interactive,
        [json!(true), json!(false)],
        "the person's search may prompt; the agent's not"
    );
    // Installed: the window's placements are the agent's answer.
    let installed = n.agent(cmds::INSTALLED, json!({})).unwrap();
    let app = installed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "App")
        .unwrap();
    assert_eq!(app["packages"][0]["version"], "1.0.0");
    assert_eq!(app["packages"][0]["vulnerabilities"][0]["severity"], "high");
    assert_eq!(app["packages"][0]["deprecated"], true);
    let placements = n.read(|w| w.placements(GREETER));
    assert_eq!(placements[0].version.as_deref(), Some("1.0.0"));
    assert!(placements[0].deprecated);
    // An agent's install goes through the MCP gate as dangerous (nuget.change: prompt); the gate says no.
    let asked: Arc<Mutex<Vec<Asked>>> = Arc::default();
    let record = asked.clone();
    let server = McpServer::new(n.w.commands.clone())
        .with_agent("test-agent")
        .with_permission_gate(Arc::new(
            move |spec: &CommandSpec, _: &Value, ctx: &CallContext| {
                record.lock().unwrap().push((
                    spec.id.to_string(),
                    ctx.class.class,
                    ctx.class.reason.clone(),
                ));
                GateDecision::Deny("the user said no".into())
            },
        ));
    let reply = std::thread::spawn(move || {
        server.handle_line(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": "eludite-nuget-install",
                               "arguments": {"package": LOGGING, "project": "App"}}})
            .to_string(),
        )
    })
    .join()
    .unwrap();
    let reply = serde_json::to_value(reply.unwrap()).unwrap();
    assert_eq!(reply["result"]["isError"], true);
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, PermissionClass::Dangerous);
    assert!(
        asked[0]
            .2
            .as_deref()
            .unwrap()
            .contains("nuget.change: prompt")
    );
    assert!(
        n.w.fake.received_params("eludite/nuget/change").is_empty(),
        "nothing changed"
    );
    // Allowed (the bus without the gate, as the policy's `allow` would): the install runs and the window follows.
    let out = n
        .agent(
            cmds::INSTALL,
            json!({ "package": LOGGING, "project": "App" }),
        )
        .unwrap();
    assert_eq!(
        out["packages"][0],
        json!({"id": LOGGING, "version": "1.0.0"})
    );
    assert_eq!(out["projects"], json!([n.app]));
    assert_eq!(out["restore"]["result"], "succeeded");
    assert_eq!(
        n.w.fake.received_params("eludite/nuget/change")[0]["interactive"],
        false
    );
    n.w.wait("the window's Installed", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.nuget()
                .window
                .read(cx)
                .placements(LOGGING)
                .iter()
                .any(|p| p.project == "App" && p.version.is_some())
        })
    });
}

#[gpui::test]
fn five_hundred_results_are_drawn_virtualized_within_the_frame_budget(cx: &mut TestAppContext) {
    let mut n = setup_nuget(cx);
    let results: Vec<cmds::SearchRow> = (0..500)
        .map(|i| cmds::SearchRow {
            id: format!("Package.{i:03}"),
            version: "1.0.0".into(),
            versions: vec!["1.0.0".into()],
            description: Some(format!("Package number {i}, for the frame budget.")),
            authors: None,
            source: "corpus".into(),
            downloads: None,
            license: None,
            project_url: None,
            icon_url: None,
            vulnerabilities: Vec::new(),
            deprecated: i % 7 == 0,
        })
        .collect();
    n.ui(
        cmds::MANAGE,
        json!({ "solution": true, "tab": "installed" }),
    );
    n.wait_idle();
    n.update(|w, cx| {
        w.set_tab(Tab::Browse, cx);
        w.set_browse(
            cmds::SearchOutput {
                results,
                sources: vec![],
                truncated: false,
            },
            cx,
        );
    });
    assert_eq!(n.rows().len(), 500);
    let mut frames = Vec::new();
    for i in 0..60 {
        let drawn_before = n.read(|w| w.rows_drawn());
        let started = Instant::now();
        n.update(|w, cx| w.select(Some(format!("Package.{:03}", i * 8)), cx));
        n.w.vcx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        frames.push(started.elapsed());
        let drawn = n.read(|w| w.rows_drawn()) - drawn_before;
        assert!(drawn < 60, "only the rows that show are drawn ({drawn})");
    }
    frames.sort();
    let p99 = frames[frames.len() * 99 / 100];
    eprintln!(
        "timing: NuGet window with 500 results, frame p50 {:.2} ms, p99 {:.2} ms",
        frames[frames.len() / 2].as_secs_f64() * 1e3,
        p99.as_secs_f64() * 1e3
    );
    super::git_tests::assert_budget("a frame of the NuGet window", p99, Duration::from_millis(8));
}
