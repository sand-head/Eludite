//! Brief 0062's Workspace window: the icon each row leads with, the change glyphs beside it, nothing listed twice,
//! the search box ("Search Workspace (Ctrl+;)") and `eludite.workspace.search`. The shell tests run against the
//! in-process fake `eludite-host`, as brief 0012's do.

// Matched ranges are byte ranges of a label: a one-range list is a list, not a range to collect.
#![allow(clippy::single_range_in_vec_init)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eludite_commands::workspace::{self, WORKSPACE_SEARCH};
use eludite_commands::workspace_tree::WORKSPACE_TREE;
use eludite_lsp::host::{TreeItemType, TreeProjectKind};
use eludite_ui::Icon;
use eludite_workspace::cargo::TargetKind;
use eludite_workspace::explorer::{DependencyGroup, FileType, NodeKind, Row, file_type_of};
use gpui::{Context, IntoElement, Render, TestAppContext, Window, div};
use serde_json::{Value, json};

use super::super::tests::{Ws, setup, setup_with};
use super::*;

fn row(kind: NodeKind, label: &str, path: Option<&str>, expanded: bool) -> Row {
    Row {
        id: label.into(),
        label: label.into(),
        depth: 1,
        kind,
        path: path.map(PathBuf::from),
        has_children: expanded,
        expanded,
    }
}

fn file(name: &str) -> Row {
    row(
        NodeKind::File {
            item_type: TreeItemType::Compile,
        },
        name,
        Some(&format!("/w/{name}")),
        false,
    )
}

#[test]
fn every_node_kind_has_its_icon() {
    let project = |web, error: Option<&str>| NodeKind::Project {
        kind: TreeProjectKind::Sdk,
        web,
        error: error.map(str::to_owned),
    };
    let cases = [
        (
            row(NodeKind::Solution, "Solution 'E'", None, true),
            false,
            Icon::Solution,
        ),
        (
            row(NodeKind::FolderRoot, "eludite", None, true),
            false,
            Icon::Workspace,
        ),
        (
            row(NodeKind::CargoWorkspace, "Cargo", None, true),
            false,
            Icon::CargoWorkspace,
        ),
        (
            row(NodeKind::CargoPackage { kinds: vec![] }, "app", None, false),
            false,
            Icon::CargoPackage,
        ),
        (
            row(project(false, None), "App", None, false),
            false,
            Icon::ProjectCSharp,
        ),
        (
            row(project(false, None), "App.Tests", None, false),
            true,
            Icon::ProjectCSharpTest,
        ),
        (
            row(project(true, None), "Web", None, false),
            false,
            Icon::ProjectWeb,
        ),
        (
            row(project(false, Some("bad xml")), "Broken", None, false),
            true,
            Icon::ProjectUnavailable,
        ),
        (
            row(NodeKind::Folder, "src", None, false),
            false,
            Icon::Folder,
        ),
        (
            row(NodeKind::Folder, "src", None, true),
            false,
            Icon::FolderOpen,
        ),
        (
            row(NodeKind::Dependencies, "Dependencies", None, false),
            false,
            Icon::Dependencies,
        ),
        (
            row(
                NodeKind::DependencyGroup {
                    group: DependencyGroup::Frameworks,
                },
                "Frameworks",
                None,
                false,
            ),
            false,
            Icon::Frameworks,
        ),
        (
            row(
                NodeKind::DependencyGroup {
                    group: DependencyGroup::Packages,
                },
                "Packages",
                None,
                false,
            ),
            false,
            Icon::Packages,
        ),
        (
            row(
                NodeKind::DependencyGroup {
                    group: DependencyGroup::Projects,
                },
                "Projects",
                None,
                false,
            ),
            false,
            Icon::ProjectReference,
        ),
        (
            row(
                NodeKind::Package {
                    id: "xunit".into(),
                    version: None,
                    transitive: false,
                    warning: false,
                },
                "xunit",
                None,
                false,
            ),
            false,
            Icon::Package,
        ),
        (
            row(
                NodeKind::Package {
                    id: "x".into(),
                    version: None,
                    transitive: true,
                    warning: false,
                },
                "x",
                None,
                false,
            ),
            false,
            Icon::PackageTransitive,
        ),
        (
            row(NodeKind::Framework, "Microsoft.NETCore.App", None, false),
            false,
            Icon::Framework,
        ),
        (
            row(NodeKind::ProjectReference, "Shared", None, false),
            false,
            Icon::ProjectCSharp,
        ),
        (
            row(NodeKind::CargoTargets, "Targets", None, false),
            false,
            Icon::CargoTargets,
        ),
    ];
    for (r, test, want) in cases {
        assert_eq!(row_icon(&r, test), want, "{}", r.label);
    }
    for (kind, want) in [
        (TargetKind::Bin, Icon::CargoTargetBin),
        (TargetKind::CustomBuild, Icon::CargoTargetBin),
        (TargetKind::Lib, Icon::CargoTargetLib),
        (TargetKind::ProcMacro, Icon::CargoTargetLib),
        (TargetKind::Test, Icon::CargoTargetTest),
        (TargetKind::Example, Icon::CargoTargetExample),
        (TargetKind::Bench, Icon::CargoTargetBench),
    ] {
        let r = row(
            NodeKind::CargoTarget { kind },
            "t",
            Some("/w/src/main.rs"),
            false,
        );
        assert_eq!(row_icon(&r, false), want, "{kind:?}");
    }
    // Every file type has an icon of its own, except that every other file shares the plain page.
    let types = [
        FileType::CSharp,
        FileType::Rust,
        FileType::Json,
        FileType::Markdown,
        FileType::Xml,
        FileType::Solution,
        FileType::Toml,
        FileType::TypeScript,
        FileType::JavaScript,
        FileType::Html,
        FileType::Css,
        FileType::Razor,
        FileType::Cshtml,
        FileType::Aspx,
        FileType::Image,
        FileType::Text,
        FileType::Shell,
        FileType::Yaml,
        FileType::Lock,
        FileType::Other,
    ];
    let icons: HashSet<Icon> = types.iter().map(|t| file_icon(*t)).collect();
    assert_eq!(icons.len(), types.len());
}

#[test]
fn files_get_their_icon_by_extension_case_insensitively() {
    let cases = [
        ("Program.cs", Icon::FileCs),
        ("LEGACY.CS", Icon::FileCs),
        ("main.rs", Icon::FileRs),
        ("appsettings.json", Icon::FileJson),
        ("README.md", Icon::FileMd),
        ("Eludite.Host.csproj", Icon::FileXml),
        ("Directory.Build.props", Icon::FileXml),
        ("Sdk.targets", Icon::FileXml),
        ("App.config", Icon::FileXml),
        ("Strings.resx", Icon::FileXml),
        ("Eludite.slnx", Icon::FileSln),
        ("Cargo.toml", Icon::FileToml),
        ("app.ts", Icon::FileTs),
        ("site.JS", Icon::FileJs),
        ("index.html", Icon::FileHtml),
        ("site.css", Icon::FileCss),
        ("Counter.razor", Icon::FileRazor),
        ("Index.cshtml", Icon::FileCshtml),
        ("Default.aspx", Icon::FileAspx),
        ("logo.png", Icon::FileImage),
        ("favicon.ICO", Icon::FileImage),
        ("notes.txt", Icon::FileText),
        ("build.sh", Icon::FileShell),
        ("fetch.ps1", Icon::FileShell),
        ("ci.yml", Icon::FileYaml),
        ("Cargo.lock", Icon::FileLock),
        ("packages.lock.json", Icon::FileLock),
        ("LICENSE", Icon::File),
    ];
    assert!(cases.len() >= 20);
    for (name, want) in cases {
        assert_eq!(row_icon(&file(name), false), want, "{name}");
        assert_eq!(file_icon(file_type_of(name)), want, "{name}");
    }
}

#[test]
fn matches_are_case_insensitive_every_word_must_match_and_ranges_merge() {
    let w = |q: &str| super::words(q);
    assert_eq!(
        match_ranges("HostRpcTarget.cs", &w("host rpc")),
        Some(vec![0..7])
    );
    assert_eq!(
        match_ranges("HostRpcTarget.cs", &w("TARGET host")),
        Some(vec![0..4, 7..13])
    );
    assert_eq!(match_ranges("HostRpcTarget.cs", &w("host lsp")), None);
    assert_eq!(match_ranges("Übersicht.md", &w("über")), Some(vec![0..5]));
    assert_eq!(match_ranges("anything", &w("   ")), None);
}

/// A tree of `n` files under one folder, for the search's budget and its background path.
fn big_model(n: usize) -> SolutionModel {
    use eludite_lsp::host::{SolutionTree, TreeFile, TreeProject};
    let files = (0..n)
        .map(|i| TreeFile {
            path: format!("/big/App/F{}/File{i}.cs", i / 100),
            item_type: TreeItemType::Compile,
            dependent_upon: None,
            link: None,
        })
        .collect();
    SolutionModel::from_tree(&SolutionTree {
        generation: 1,
        path: Some("/big/Big.slnx".into()),
        projects: vec![TreeProject {
            name: "App".into(),
            path: "/big/App/App.csproj".into(),
            kind: TreeProjectKind::Sdk,
            web: false,
            target_frameworks: vec!["net10.0".into()],
            files,
            error: None,
            dependencies: None,
        }],
    })
    .unwrap()
}

#[test]
fn a_search_over_20000_rows_fits_the_budget() {
    let (all, parents) = SearchIndex::of(Some(&big_model(20_000)));
    assert!(all.len() > 20_000);
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let hits = search_rows(&all, &parents, "file1999 cs");
        best = best.min(t.elapsed());
        assert_eq!(
            hits.matched.len(),
            11,
            "File1999.cs and File19990..19999.cs"
        );
    }
    eprintln!("search over {} rows: {best:?}", all.len());
    // The budget is 10 ms per keystroke in a release build; this debug build gets ten times that.
    let budget = if cfg!(debug_assertions) { 100 } else { 10 };
    assert!(best < Duration::from_millis(budget), "{best:?}");
}

impl Ws {
    fn se_read<R>(&self, f: impl FnOnce(&SolutionExplorer, &gpui::App) -> R) -> R {
        self.shell
            .read_with(&self.vcx, |s, cx| f(s.explorer().read(cx), cx))
    }

    /// Link `eludite.workspace.search` to the window and bind Ctrl+;, as `app.rs` does.
    fn se_link_search(&mut self) {
        let link = register_search(&self.commands);
        self.shell.update_in(&mut self.vcx, |s, window, cx| {
            let explorer = s.explorer().clone();
            explorer.update(cx, |e, cx| e.link_search(link, window, cx));
        });
        self.vcx.update(|_, cx| bind_keys(cx));
        self.vcx.run_until_parked();
    }

    fn se_type(&mut self, text: &str) {
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

    fn se_box_focused(&mut self) -> bool {
        let input = self.se_read(|e, _| e.search_input().clone());
        self.vcx
            .update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window))
    }
}

/// The Eludite.slnx layout (`src/Eludite.Host` with `Rpc/HostRpcTarget.cs`, `tests/Eludite.Host.Tests`, `debuggers`)
/// in `<tmp>/dotnet`, and the fake host's tree for it (real paths, as MSBuild reports them).
fn eludite_folder(w: &Ws) -> PathBuf {
    let root = w.path("dotnet");
    let write = |rel: &str, text: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    };
    write(
        "Eludite.slnx",
        "<Solution><Project Path=\"src/Eludite.Host/Eludite.Host.csproj\" /></Solution>",
    );
    let host = write("src/Eludite.Host/Eludite.Host.csproj", "<Project />");
    let program = write("src/Eludite.Host/Program.cs", "class P { }\n");
    let rpc = write("src/Eludite.Host/Rpc/HostRpcTarget.cs", "class T { }\n");
    let tests = write(
        "tests/Eludite.Host.Tests/Eludite.Host.Tests.csproj",
        "<Project />",
    );
    let test = write(
        "tests/Eludite.Host.Tests/HostRpcTargetTests.cs",
        "class X { }\n",
    );
    let mono = write(
        "debuggers/Eludite.Debugger.Mono/Eludite.Debugger.Mono.csproj",
        "<Project />",
    );
    let dispatcher = write(
        "debuggers/Eludite.Debugger.Mono/Dispatcher.cs",
        "class D { }\n",
    );
    write("Directory.Build.props", "<Project />");
    let real = |p: &str| {
        std::fs::canonicalize(p)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    w.fake.set_tree(json!([
        {"name": "Eludite.Host", "path": real(&host), "kind": "sdk", "targetFrameworks": ["net10.0"],
         "files": [{"path": real(&program), "itemType": "compile"}, {"path": real(&rpc), "itemType": "compile"}]},
        {"name": "Eludite.Host.Tests", "path": real(&tests), "kind": "sdk", "targetFrameworks": ["net10.0"],
         "files": [{"path": real(&test), "itemType": "compile"}],
         "dependencies": {"restored": true, "frameworks": [], "projects": [],
                          "packages": [{"id": "xunit.v3", "version": "3.0.0"}]}},
        {"name": "Eludite.Debugger.Mono", "path": real(&mono), "kind": "sdk", "targetFrameworks": ["net472"],
         "files": [{"path": real(&dispatcher), "itemType": "compile"}]}
    ]));
    root
}

/// Open `<tmp>/dotnet` through `..` (`<tmp>/dotnet/../dotnet`) and wait for the solution's projects.
fn open_eludite_folder(w: &mut Ws) -> PathBuf {
    let root = eludite_folder(w);
    let spelled = root.join("..").join("dotnet");
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(
                workspace::WORKSPACE_OPEN_FOLDER,
                json!({ "path": spelled.to_string_lossy() }),
                window,
                cx,
            )
        })
        .unwrap();
    w.wait("the solution and the listing", |w| {
        w.se_read(|e, _| {
            e.model().is_some_and(|m| {
                let labels: Vec<&str> = m.root.children.iter().map(|c| c.label.as_str()).collect();
                labels.contains(&"Solution 'Eludite' (3 of 3 projects)")
                    && labels.contains(&"Directory.Build.props")
            })
        })
    });
    root
}

fn all_rows(w: &Ws) -> Vec<Row> {
    w.se_read(|e, _| {
        let index = e.index.read().unwrap();
        index.all.as_ref().clone()
    })
}

#[gpui::test]
fn a_solution_folder_lists_each_project_once_under_the_solution(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    open_eludite_folder(&mut w);
    let rows = all_rows(&w);
    let labels: Vec<String> = rows
        .iter()
        .map(|r| format!("{}{}", "  ".repeat(r.depth), r.label))
        .collect();
    for name in [
        "Eludite.Host",
        "Eludite.Host.Tests",
        "Eludite.Debugger.Mono",
    ] {
        let projects: Vec<&Row> = rows
            .iter()
            .filter(|r| r.label.starts_with(&format!("{name} (")))
            .collect();
        assert_eq!(projects.len(), 1, "{name}: {labels:#?}");
        assert_eq!(projects[0].depth, 2, "{name} sits under the solution");
        assert!(
            !rows.iter().any(|r| r.label == format!("{name}.csproj")),
            "{name}.csproj again as a file: {labels:#?}"
        );
    }
    // No folder at the root holds only projects.
    for folder in ["src", "tests", "debuggers"] {
        assert!(
            !rows.iter().any(|r| r.depth == 1 && r.label == folder),
            "{labels:#?}"
        );
    }
    // The test project's icon has the flask; the others do not.
    w.se_read(|e, _| {
        let icon_of = |name: &str| {
            let r = rows.iter().find(|r| r.label.starts_with(name)).unwrap();
            row_icon(r, e.test_projects.contains(&r.id))
        };
        assert_eq!(icon_of("Eludite.Host.Tests ("), Icon::ProjectCSharpTest);
        assert_eq!(icon_of("Eludite.Host ("), Icon::ProjectCSharp);
    });

    // Agents see each project once too.
    w.wait("the workspace tree", |w| {
        w.commands.invoke(WORKSPACE_TREE, json!({})).unwrap()["state"] == "loaded"
    });
    let tree = w.commands.invoke(WORKSPACE_TREE, json!({})).unwrap();
    let mut names: Vec<&str> = tree["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Eludite.Debugger.Mono",
            "Eludite.Host",
            "Eludite.Host.Tests"
        ]
    );
}

#[gpui::test]
fn a_modified_file_shows_its_change_glyph_and_an_unchanged_one_none(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.open_solution();
    let project = w.path("src/App/App.csproj");
    w.click(&format!(
        "{}-toggle",
        row_selector(&project.to_string_lossy())
    ));
    let status = eludite_git::Status {
        unstaged: vec![eludite_git::Change {
            path: "src/App/Program.cs".into(),
            kind: eludite_git::ChangeKind::Modified,
            old_path: None,
        }],
        ..Default::default()
    };
    let root = w.dir.path().to_path_buf();
    w.shell.update(&mut w.vcx, |s, cx| {
        s.explorer().clone().update(cx, |e, cx| {
            e.set_git(Some((root, Rc::new(status.glyphs()))), cx)
        })
    });
    w.vcx.run_until_parked();
    let p = project.to_string_lossy();
    let program = row_selector(&format!("{p}|Program.cs"));
    let order = row_selector(&format!("{p}|Models/"));
    // The glyph sits in the badge slot after the icon and before the label.
    let icon = w.bounds(&format!("{program}-icon"));
    let badge = w.bounds(&format!("{program}-badge"));
    assert!(
        badge.origin.x >= icon.origin.x + icon.size.width,
        "{icon:?} {badge:?}"
    );
    let glyph = w.se_read(|e, _| {
        e.rows()
            .iter()
            .find(|r| r.label == "Program.cs")
            .and_then(|r| e.git_glyph(r))
    });
    assert_eq!(glyph, Some(eludite_git::FileGlyph::Modified));
    // An unchanged row has its icon and no glyph: no lock for unchanged files.
    let _ = w.bounds(&format!("{order}-icon"));
    let leaked: &'static str = Box::leak(format!("{order}-badge").into_boxed_str());
    assert!(w.vcx.debug_bounds(leaked).is_none());
    let aspx = row_selector(&format!("{p}|Default.aspx"));
    let leaked: &'static str = Box::leak(format!("{aspx}-badge").into_boxed_str());
    assert!(w.vcx.debug_bounds(leaked).is_none());
}

#[gpui::test]
fn typing_host_rpc_shows_the_file_under_its_expanded_ancestors_and_escape_restores_the_tree(
    cx: &mut TestAppContext,
) {
    let mut w = setup(cx);
    open_eludite_folder(&mut w);
    let before_rows = w.row_labels();
    let before_expanded = w.se_read(|e, _| e.expanded().clone());
    assert!(!before_rows.iter().any(|l| l.contains("HostRpcTarget.cs")));

    w.click(SEARCH_BOX);
    assert!(w.se_box_focused());
    w.se_type("host rpc");
    // Nothing until typing pauses for the debounce.
    assert!(!w.se_read(|e, _| e.searching()));
    w.vcx.executor().advance_clock(SEARCH_DEBOUNCE);
    w.vcx.run_until_parked();
    assert!(w.se_read(|e, _| e.searching()));
    let labels = w.row_labels();
    let name = w.se_read(|e, _| e.model().unwrap().root.label.clone());
    assert_eq!(
        labels,
        [
            name,
            "  Solution 'Eludite' (3 of 3 projects)".into(),
            "    Eludite.Host (net10.0)".into(),
            "      Rpc".into(),
            "        HostRpcTarget.cs".into(),
            "    Eludite.Host.Tests (net10.0)".into(),
            "      HostRpcTargetTests.cs".into(),
        ],
    );
    let rows = w.se_read(|e, _| e.rows().to_vec());
    assert!(rows[..4].iter().all(|r| r.expanded), "ancestors expanded");
    let target = rows.iter().find(|r| r.label == "HostRpcTarget.cs").unwrap();
    // The matched characters are drawn bold: `HostRpc`.
    assert_eq!(
        w.se_read(|e, _| e.search_matches(&target.id).map(<[_]>::to_vec)),
        Some(vec![0..7])
    );
    let _ = w.bounds(&row_selector(&target.id));
    assert_eq!(w.se_read(|e, cx| e.query(cx)), "host rpc");

    // Escape clears the box, restores the expanded set from before and returns to the tree.
    w.vcx.simulate_keystrokes("escape");
    w.vcx.run_until_parked();
    assert!(!w.se_read(|e, _| e.searching()));
    assert_eq!(w.se_read(|e, cx| e.query(cx)), "");
    assert_eq!(w.se_read(|e, _| e.expanded().clone()), before_expanded);
    assert_eq!(w.row_labels(), before_rows);
    assert!(!w.se_box_focused());
    let tree_focused = w.vcx.update(|window, cx| {
        w.shell
            .read(cx)
            .explorer()
            .read(cx)
            .focus
            .is_focused(window)
    });
    assert!(tree_focused);
}

#[gpui::test]
fn ctrl_semicolon_focuses_the_search_box_from_the_editor(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    w.se_link_search();
    let (_, view) = w.open_program();
    let editor_focused = w
        .vcx
        .update(|window, cx| view.read(cx).focus_handle(cx).is_focused(window));
    assert!(editor_focused, "the editor has the keys");
    w.vcx.simulate_keystrokes("ctrl-;");
    w.vcx.run_until_parked();
    assert!(w.se_box_focused(), "Ctrl+; puts the keys in the search box");
    assert!(
        w.audit().contains(&WORKSPACE_SEARCH.to_owned()),
        "Ctrl+; runs the command: {:?}",
        w.audit()
    );
    // Typing goes to the box, not the editor.
    w.se_type("order");
    assert_eq!(w.se_read(|e, cx| e.query(cx)), "order");
    assert_eq!(w.text(&view), super::super::tests::PROGRAM);
}

/// Checks `value` against the parts of `workspace-search.output.json` it uses.
fn conforms(value: &Value) {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../protocol/schemas/workspace-search.output.json"
    ))
    .unwrap();
    let obj = value.as_object().unwrap();
    for r in schema["required"].as_array().unwrap() {
        assert!(obj.contains_key(r.as_str().unwrap()), "missing {r}");
    }
    assert!(obj.keys().all(|k| schema["properties"].get(k).is_some()));
    let kinds = schema["properties"]["rows"]["items"]["properties"]["kind"]["enum"]
        .as_array()
        .unwrap();
    for row in value["rows"].as_array().unwrap() {
        assert!(row["name"].is_string());
        assert!(kinds.contains(&row["kind"]), "{row}");
        assert!(row.get("path").is_none_or(Value::is_string));
        assert!(
            row.as_object()
                .unwrap()
                .keys()
                .all(|k| ["path", "name", "kind"].contains(&k.as_str()))
        );
    }
    assert!(value["total"].is_u64());
    assert!(value["truncated"].is_boolean());
}

#[gpui::test]
fn the_search_command_answers_its_schema_types_into_the_box_and_an_empty_query_clears(
    cx: &mut TestAppContext,
) {
    let mut w = setup(cx);
    w.se_link_search();
    open_eludite_folder(&mut w);
    let before = w.row_labels();
    // From an agent's thread.
    let commands = w.commands.clone();
    let out = std::thread::spawn(move || {
        commands.invoke(WORKSPACE_SEARCH, json!({ "query": "HOST rpc" }))
    })
    .join()
    .unwrap()
    .unwrap();
    conforms(&out);
    assert_eq!(out["query"], "HOST rpc");
    assert_eq!(out["total"], 2);
    let names: Vec<&str> = out["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["HostRpcTarget.cs", "HostRpcTargetTests.cs"]);
    assert_eq!(out["rows"][0]["kind"], "file");
    assert!(
        out["rows"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("HostRpcTarget.cs")
    );
    // The box shows the query and the tree the matches, at once (no debounce for the command).
    w.vcx.run_until_parked();
    assert_eq!(w.se_read(|e, cx| e.query(cx)), "HOST rpc");
    assert!(
        w.row_labels()
            .iter()
            .any(|l| l.trim() == "HostRpcTarget.cs")
    );
    // Without a query: the answer for the box's text, nothing changes.
    let again = w.commands.invoke(WORKSPACE_SEARCH, json!({})).unwrap();
    assert_eq!(again["total"], 2);
    // Projects, folders and the solution match by their names too.
    let projects = w
        .commands
        .invoke(WORKSPACE_SEARCH, json!({ "query": "eludite.host" }))
        .unwrap();
    conforms(&projects);
    let kinds: Vec<&str> = projects["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["project", "project"], "{projects}");
    // An empty query clears the box and restores the tree.
    let cleared = w
        .commands
        .invoke(WORKSPACE_SEARCH, json!({ "query": "" }))
        .unwrap();
    conforms(&cleared);
    assert_eq!(cleared["rows"], json!([]));
    assert_eq!(cleared["total"], 0);
    w.vcx.run_until_parked();
    assert_eq!(w.se_read(|e, cx| e.query(cx)), "");
    assert!(!w.se_read(|e, _| e.searching()));
    assert_eq!(w.row_labels(), before);
    assert!(
        w.commands
            .invoke(WORKSPACE_SEARCH, json!({ "query": 3 }))
            .is_err()
    );
}

#[gpui::test]
fn a_large_tree_searches_off_the_ui_thread_and_drops_stale_results(cx: &mut TestAppContext) {
    let mut w = setup_with(cx, |_| {});
    w.shell.update(&mut w.vcx, |s, cx| {
        s.explorer()
            .clone()
            .update(cx, |e, cx| e.set_model(big_model(SEARCH_INLINE_ROWS), cx))
    });
    w.vcx.run_until_parked();
    // Two queries in a row: the first one's result arrives after the second was asked for and is dropped.
    w.shell.update(&mut w.vcx, |s, cx| {
        s.explorer().clone().update(cx, |e, cx| {
            e.set_query("file1", cx);
            assert!(!e.searching(), "the search runs on a background task");
            e.set_query("file19999", cx);
        })
    });
    w.wait("the background search", |w| w.se_read(|e, _| e.searching()));
    w.vcx.run_until_parked();
    let labels = w.row_labels();
    assert_eq!(
        labels.last().map(|l| l.trim()),
        Some("File19999.cs"),
        "{labels:?}"
    );
    assert!(
        !labels.iter().any(|l| l.trim() == "File1.cs"),
        "the stale result is not shown"
    );
}

/// 60 tree rows, led by icons or by brief 0012's text glyphs.
struct SixtyRows {
    icons: bool,
}

impl Render for SixtyRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::vs_dark();
        div().flex().flex_col().children((0..60).map(|i| {
            let style = TreeRowStyle {
                depth: 2,
                disclosure: None,
                ..Default::default()
            };
            let label = format!("HostRpcTarget{i}.cs");
            let badge = (i % 3 == 0).then_some(("\u{2713}", 0xC5_86_86));
            if self.icons {
                let icon = file_icon(file_type_of(&label));
                tree_row_with_icon(
                    &t,
                    format!("row-{i}"),
                    Some((icon, icon.tint(&t))),
                    badge,
                    label,
                    &[0..7],
                    style,
                    |_, _, _| {},
                )
            } else {
                eludite_ui::tree_row_with_badge(
                    &t,
                    format!("row-{i}"),
                    Some("\u{2261}"),
                    badge,
                    label,
                    style,
                    |_, _, _| {},
                )
            }
        }))
    }
}

/// The budget: a frame of 60 rows with icons (and a search's bold matches) under 2 ms more than glyph rows. The
/// test platform does not rasterize SVGs (GPUI rasterizes each icon once per size and caches it in the atlas), so
/// this measures the elements and their layout.
#[gpui::test]
fn sixty_rows_with_icons_draw_within_2_ms_of_glyph_rows(cx: &mut TestAppContext) {
    let median = |icons: bool, cx: &mut TestAppContext| {
        let (_, vcx) = cx.add_window_view(|_, _| SixtyRows { icons });
        let mut frames: Vec<Duration> = (0..30)
            .map(|_| {
                vcx.update(|window, cx| {
                    window.refresh();
                    let t = Instant::now();
                    let _ = window.draw(cx);
                    t.elapsed()
                })
            })
            .collect();
        frames.sort();
        frames[frames.len() / 2]
    };
    let glyphs = median(false, cx);
    let icons = median(true, cx);
    eprintln!(
        "timing: 60 Workspace rows: glyphs {:.2} ms, icons {:.2} ms (median of 30 frames)",
        glyphs.as_secs_f64() * 1e3,
        icons.as_secs_f64() * 1e3
    );
    super::super::git_tests::assert_budget(
        "60 rows with icons over glyph rows",
        icons.saturating_sub(glyphs),
        Duration::from_millis(2),
    );
}
