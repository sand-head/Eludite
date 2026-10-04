//! Headless GPUI tests of the project property pages, launch profiles and the configuration selection (brief 0049)
//! against the in-process fake `eludite-host` (its brief 0049 service): the pages open from Workspace, the menu and
//! `eludite.view.show`, show values with their sources, an edit marks the tab dirty and Save writes through the host
//! and moves the generation on, closing asks, the Configuration and Platform lists change the values and route the
//! write, All Configurations confirms, the inherited banner's Override, the Debug page edits a profile and the Debug
//! toolbar's choice is what F5 uses, the toolbar lists select and persist and the build uses the selection, Configuration
//! Manager edits the mapping, a project the mapping skips gets Visual Studio's Output line, the Target Framework list
//! drives a start and a test run, and an agent's `properties` and `set_property` match the pages.

use std::path::Path;
use std::time::{Duration, Instant};

use eludite_commands::build::OutputSource;
use eludite_commands::project::properties as props;
use eludite_lsp::fake::FakeHost;
use gpui::{Entity, TestAppContext};
use serde_json::{Value, json};

use super::build::{CONFIGURATION_BUTTON, PLATFORM_BUTTON, toolbar_item_selector};
use super::configuration_manager as cm;
use super::explorer::row_selector;
use super::project_properties::pages::{self, PropertyPages};
use super::project_properties::tab_id;
use super::tests::{Ws, setup, setup_debug};
use super::toolbar::{FRAMEWORK_BUTTON, PROFILE_BUTTON};

struct Pw {
    w: Ws,
    project: String,
    lib: String,
}

fn script(fake: &FakeHost, project: &str, lib: &str) {
    fake.set_project_properties(
        project,
        FakeHost::sample_project_properties(project, &["net10.0"]),
    );
    fake.set_launch_profiles(
        project,
        json!([
            {"name": "App", "commandName": "Project", "commandLineArgs": "--verbose",
             "environmentVariables": [{"name": "ZETA", "value": "1"}, {"name": "ALPHA", "value": "2"}]},
            {"name": "Other", "commandName": "Project", "environmentVariables": []}
        ]),
    );
    let mapping = |sc: &str, sp: &str, c: &str, build: bool| {
        json!({"solutionConfiguration": sc, "solutionPlatform": sp, "configuration": c, "platform": "Any CPU",
               "build": build})
    };
    let rows = |build_staging: bool| {
        let mut v = Vec::new();
        for sc in ["Debug", "Release", "Staging"] {
            for sp in ["Any CPU", "x64"] {
                let c = if sc == "Staging" { "Release" } else { sc };
                v.push(mapping(sc, sp, c, sc != "Staging" || build_staging));
            }
        }
        v
    };
    fake.set_solution_configurations(json!({
        "format": "slnx", "configurations": ["Debug", "Release", "Staging"], "platforms": ["Any CPU", "x64"],
        "projects": [
            {"name": "App", "path": project, "configurations": ["Debug", "Release"], "platforms": ["Any CPU"],
             "mappings": rows(true)},
            {"name": "Lib", "path": lib, "configurations": ["Debug", "Release"], "platforms": ["Any CPU"],
             "mappings": rows(false)}
        ]
    }));
}

fn ready(w: Ws) -> Pw {
    let project = w.path("src/App/App.csproj").to_string_lossy().into_owned();
    let lib = w.path("src/Lib/Lib.csproj").to_string_lossy().into_owned();
    std::fs::create_dir_all(w.path("src/Lib")).unwrap();
    std::fs::write(&lib, "<Project Sdk=\"Microsoft.NET.Sdk\" />").unwrap();
    script(&w.fake, &project, &lib);
    Pw { w, project, lib }
}

fn start(cx: &mut TestAppContext) -> Pw {
    ready(setup(cx))
}

impl Pw {
    fn open(&mut self) {
        // Tall enough for the longest page without scrolling.
        self.w
            .vcx
            .simulate_resize(gpui::size(gpui::px(1400.), gpui::px(1700.)));
        self.w.open_solution();
        self.w.wait("the solution configurations", |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.properties.configurations.is_some())
        });
        self.settled_generation();
    }

    /// Wait until the shell has seen the host's current generation (its `eludite/solution/status` can arrive after
    /// the tree and the answers that `open` waits for), and return it.
    fn settled_generation(&mut self) -> u64 {
        let fake = self.w.fake.clone();
        self.w.wait("the shell to see the host's generation", |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.generation == fake.generation())
        });
        self.generation()
    }

    /// Wait until a save that reloaded the solution has fully landed: the shell is on `generation` (the host sends the
    /// reload's `eludite/solution/status` before the `setProperty` answer, and the shell takes them from different
    /// channels, so either can be seen first), the tab is clean, and the pages show values evaluated under
    /// `generation` (of `configuration`, when given), not the ones from before the write.
    fn wait_reloaded(
        &mut self,
        generation: u64,
        configuration: Option<&str>,
    ) -> Entity<PropertyPages> {
        let project = self.project.clone();
        let tab = tab_id(&project);
        let configuration = configuration.map(str::to_owned);
        self.w.wait("the save and the reload", |w| {
            !w.dirty(&tab)
                && w.shell.read_with(&w.vcx, |s, cx| {
                    s.generation == generation
                        && s.property_pages(&project).is_some_and(|p| {
                            let p = p.read(cx);
                            !p.loading
                                && p.result.as_ref().is_some_and(|r| {
                                    r.generation == generation
                                        && configuration
                                            .as_ref()
                                            .is_none_or(|c| &r.configuration == c)
                                })
                        })
                })
        });
        self.pages().unwrap()
    }

    fn pages(&self) -> Option<Entity<PropertyPages>> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.property_pages(&self.project))
    }

    /// Wait until the pages show values (of `configuration`, when given).
    fn wait_values(&mut self, configuration: Option<&str>) -> Entity<PropertyPages> {
        let project = self.project.clone();
        let configuration = configuration.map(str::to_owned);
        self.w.wait("the property pages' values", |w| {
            w.shell.read_with(&w.vcx, |s, cx| {
                s.property_pages(&project).is_some_and(|p| {
                    let p = p.read(cx);
                    !p.loading
                        && p.result.as_ref().is_some_and(|r| {
                            configuration.as_ref().is_none_or(|c| &r.configuration == c)
                        })
                })
            })
        });
        self.pages().unwrap()
    }

    fn shown(&self, name: &str) -> String {
        self.pages()
            .unwrap()
            .read_with(&self.w.vcx, |p, _| p.shown_value(name).unwrap_or_default())
    }

    fn note(&self, name: &str) -> String {
        self.pages()
            .unwrap()
            .read_with(&self.w.vcx, |p, _| p.source_note(name).unwrap_or_default())
    }

    /// Click a text box, type, Enter.
    fn type_into(&mut self, selector: &str, text: &str) {
        self.w.click(selector);
        let pages = self.pages().unwrap();
        pages.update(&mut self.w.vcx, |p, cx| p.type_text(text, cx));
        self.w.vcx.simulate_keystrokes("enter");
        self.w.vcx.run_until_parked();
    }

    fn generation(&self) -> u64 {
        self.w.shell.read_with(&self.w.vcx, |s, _| s.generation)
    }

    fn ui(&mut self, command: &str, args: Value) -> Result<Value, eludite_commands::CommandError> {
        let r = self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.invoke(command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
        r
    }

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

    fn set_property_edits(&self) -> Vec<Value> {
        self.w
            .fake
            .received_params("eludite/project/setProperty")
            .into_iter()
            .map(|p| p["edits"].clone())
            .collect()
    }

    fn wait_set_properties(&mut self, n: usize) {
        let fake = self.w.fake.clone();
        self.w.wait("eludite/project/setProperty", |_| {
            fake.received_params("eludite/project/setProperty").len() >= n
        });
    }
}

#[gpui::test]
fn pages_open_from_workspace_show_sources_save_through_the_host_and_ask_on_close(
    cx: &mut TestAppContext,
) {
    let mut p = start(cx);
    p.open();
    let generation = p.generation();

    // A double-click on the project opens its pages as a document tab named after it.
    p.w.double_click(&row_selector(&p.project));
    let pages = p.wait_values(Some("Debug"));
    let tab = tab_id(&p.project);
    assert_eq!(
        p.w.controller.active_document().as_deref(),
        Some(tab.as_str())
    );
    assert!(p.w.audit().contains(&props::PROPERTIES.to_owned()));
    let timings =
        p.w.shell
            .read_with(&p.w.vcx, |s, _| s.properties_timings().clone());
    let open_ms = (timings.shown.unwrap() - timings.opened.unwrap()).as_secs_f64() * 1e3;
    eprintln!("timing: property pages open to values shown {open_ms:.1} ms (fake host)");
    assert!(open_ms < 150.0, "{open_ms} ms");

    // Values with their sources.
    assert_eq!(p.shown("OutputType"), "Exe");
    assert_eq!(p.note("OutputType"), "From the project file, line 3");
    assert_eq!(p.shown("LangVersion"), "12.0");
    assert_eq!(
        p.note("LangVersion"),
        "Inherited from Directory.Build.props"
    );
    assert_eq!(p.note("Nullable"), "Default");
    // Resources, Settings and Signing say they are not there yet.
    p.w.click(&pages::page_selector("resources"));
    let page = pages.read_with(&p.w.vcx, |v, _| v.page.clone());
    assert_eq!(page, "resources");
    p.w.click(&pages::page_selector("application"));

    // An edit marks the tab dirty; Ctrl+S writes it through the host, which reloads: the generation moves on.
    p.type_into(&pages::property_selector("AssemblyName"), "Renamed");
    assert_eq!(p.shown("AssemblyName"), "AppRenamed");
    assert!(p.w.dirty(&tab));
    assert_eq!(p.note("AssemblyName"), "Changed (not saved)");
    p.w.vcx.simulate_keystrokes("ctrl-s");
    p.wait_set_properties(1);
    assert_eq!(
        p.set_property_edits()[0],
        json!([{"name": "AssemblyName", "value": "AppRenamed"}])
    );
    p.wait_reloaded(generation + 1, None);
    assert_eq!(p.shown("AssemblyName"), "AppRenamed");
    assert_eq!(p.note("AssemblyName"), "From the project file, line 5");
    let fake = p.w.fake.clone();
    p.w.wait("the tree of the new generation", |_| {
        fake.received_params("eludite/solution/tree").len() >= 2
    });

    // Closing dirty pages asks; Don't Save closes them and writes nothing.
    p.type_into(&pages::property_selector("AssemblyName"), "X");
    assert!(p.w.dirty(&tab));
    p.w.shell.update_in(&mut p.w.vcx, |s, window, cx| {
        s.run(
            eludite_commands::workspace::FILE_CLOSE,
            json!({"path": tab}),
            window,
            cx,
        )
    });
    assert!(p.w.vcx.has_pending_prompt(), "closing dirty pages asks");
    p.w.vcx.simulate_prompt_answer("Don't Save");
    p.w.vcx.run_until_parked();
    assert!(p.pages().is_none());
    assert!(p.w.controller.layout().documents.get(&tab).is_none());
    assert_eq!(p.set_property_edits().len(), 1);

    // Project > Properties opens the selected project's pages again; so does View's project_properties.
    p.w.shell.update_in(&mut p.w.vcx, |s, window, cx| {
        s.run(props::PROPERTIES, json!({"open": true}), window, cx)
    });
    assert!(p.pages().is_some());
    p.w.shell.update_in(&mut p.w.vcx, |s, window, cx| {
        s.run(
            eludite_commands::workspace::FILE_CLOSE,
            json!({"path": tab}),
            window,
            cx,
        )
    });
    assert!(p.pages().is_none());
    p.w.commands
        .invoke("eludite.view.show", json!({"id": "project_properties"}))
        .unwrap();
    let project = p.project.clone();
    p.w.wait("View's project_properties to open the pages", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.property_pages(&project).is_some())
    });
    assert!(
        p.w.controller
            .layout()
            .documents
            .get("project_properties")
            .is_none()
    );
}

#[gpui::test]
fn configuration_lists_route_the_write_all_configurations_confirms_and_override_writes_inherited(
    cx: &mut TestAppContext,
) {
    let mut p = start(cx);
    p.open();
    p.ui(
        props::PROPERTIES,
        json!({"project": "App", "open": true, "page": "build"}),
    )
    .unwrap();
    let pages = p.wait_values(Some("Debug"));
    assert_eq!(pages.read_with(&p.w.vcx, |v, _| v.page.clone()), "build");
    assert_eq!(p.shown("DefineConstants"), "TRACE;DEBUG");

    // Release: the values are evaluated for it, and a value entered goes to Release's group.
    p.w.click(&pages::configuration_selector(1));
    p.wait_values(Some("Release"));
    let asked = p.w.fake.received_params("eludite/project/properties");
    assert_eq!(asked.last().unwrap()["configuration"], "Release");
    assert_eq!(asked.last().unwrap()["platform"], "AnyCPU");
    p.type_into(
        &pages::property_selector("DefineConstants"),
        ";RELEASE_ONLY",
    );
    let generation = p.settled_generation();
    p.w.click(pages::SAVE);
    p.wait_set_properties(1);
    assert_eq!(
        p.set_property_edits()[0],
        json!([{"name": "DefineConstants", "value": "TRACE;DEBUG;RELEASE_ONLY", "configuration": "Release",
                "platform": "AnyCPU"}])
    );
    p.wait_reloaded(generation + 1, Some("Release"));
    assert_eq!(
        p.note("DefineConstants").split(", under").next(),
        Some("From the project file")
    );
    assert_eq!(
        p.w.fake
            .project_property(&p.project, "DefineConstants", Some(("Release", "AnyCPU"))),
        Some("TRACE;DEBUG;RELEASE_ONLY".into())
    );

    // All Configurations on a property that differs per configuration asks first, then removes the conditioned ones.
    p.w.click(&pages::configuration_selector(2));
    p.w.vcx.run_until_parked();
    p.type_into(&pages::property_selector("DefineConstants"), ";ALL");
    assert!(p.w.vcx.has_pending_prompt(), "All Configurations asks");
    p.w.vcx.simulate_prompt_answer("Set for All Configurations");
    p.w.vcx.run_until_parked();
    p.w.vcx.simulate_keystrokes("ctrl-s");
    p.wait_set_properties(2);
    let edit = &p.set_property_edits()[1][0];
    assert_eq!(edit["allConfigurations"], true);
    assert!(edit.get("configuration").is_none());
    let fake = p.w.fake.clone();
    let project = p.project.clone();
    p.w.wait("the conditioned value removed", |_| {
        fake.project_property(&project, "DefineConstants", Some(("Release", "AnyCPU")))
            .is_none()
    });

    // The inherited banner: read-only until Override, then written in the project with `override`.
    p.w.click(&pages::page_selector("application"));
    p.w.vcx.run_until_parked();
    let lang = p.shown("LangVersion");
    assert_eq!(lang, "12.0");
    p.w.click(&pages::choice_selector("LangVersion", 2));
    let dirty = p.pages().unwrap().read_with(&p.w.vcx, |v, _| v.is_dirty());
    assert!(!dirty, "an inherited value is read-only before Override");
    p.w.click(&pages::override_selector("LangVersion"));
    p.w.click(&pages::choice_selector("LangVersion", 2));
    assert_eq!(
        p.shown("LangVersion"),
        "13.0",
        "choice 2 at {:?}, override at {:?}, dirty {}",
        p.w.bounds(&pages::choice_selector("LangVersion", 2)),
        p.w.bounds(&pages::override_selector("LangVersion")),
        p.pages().unwrap().read_with(&p.w.vcx, |v, _| v.is_dirty())
    );
    p.w.vcx.simulate_keystrokes("ctrl-s");
    p.wait_set_properties(3);
    assert_eq!(
        p.set_property_edits()[2],
        json!([{"name": "LangVersion", "value": "13.0", "override": true}])
    );
    let fake = p.w.fake.clone();
    let project = p.project.clone();
    p.w.wait("the override written", |_| {
        fake.project_property(&project, "LangVersion", None) == Some("13.0".into())
    });
}

#[gpui::test]
fn the_debug_page_edits_a_profile_and_the_debug_toolbars_choice_is_what_f5_uses(
    cx: &mut TestAppContext,
) {
    let mut p = start(cx);
    p.open();
    p.ui(
        props::PROPERTIES,
        json!({"project": "App", "open": true, "page": "debug"}),
    )
    .unwrap();
    let project = p.project.clone();
    p.w.wait("the launch profiles", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.property_pages(&project)
                .is_some_and(|v| v.read(cx).launch.is_some())
        })
    });
    // The Debug page edits the selected profile at once (eludite.project.set_launch_profile).
    p.type_into(
        &pages::profile_field_selector("command_line_args"),
        " --fast",
    );
    let fake = p.w.fake.clone();
    p.w.wait("eludite/project/setLaunchProfile", |_| {
        !fake
            .received_params("eludite/project/setLaunchProfile")
            .is_empty()
    });
    let sent = &p.w.fake.received_params("eludite/project/setLaunchProfile")[0];
    assert_eq!(sent["action"], "set");
    assert_eq!(sent["profile"], "App");
    assert_eq!(
        sent["values"],
        json!({"commandLineArgs": "--verbose --fast"})
    );
    // The environment table keeps its order; a new variable goes last.
    p.type_into(&pages::profile_field_selector("env_add"), "NEW=3");
    p.w.wait("the second edit", |_| {
        fake.received_params("eludite/project/setLaunchProfile")
            .len()
            >= 2
    });
    assert_eq!(
        p.w.fake.received_params("eludite/project/setLaunchProfile")[1]["values"]["environmentVariables"],
        json!([{"name": "ZETA", "value": "1"}, {"name": "ALPHA", "value": "2"}, {"name": "NEW", "value": "3"}])
    );
    // New creates "Profile 1".
    p.w.click(pages::PROFILE_NEW);
    p.w.wait("New", |_| {
        fake.received_params("eludite/project/setLaunchProfile")
            .len()
            >= 3
    });
    assert_eq!(
        p.w.fake.received_params("eludite/project/setLaunchProfile")[2]["action"],
        "create"
    );
    p.w.wait("three profiles", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.properties
                .launch
                .get(&project)
                .is_some_and(|l| l.profiles.len() == 3)
        })
    });

    // The Debug toolbar's list picks the profile F5 uses, kept for the solution; the agents' view agrees.
    p.w.click(PROFILE_BUTTON);
    p.w.click(&toolbar_item_selector("profile", 1));
    let out = p
        .agent(props::LAUNCH_PROFILES, json!({"project": "App"}))
        .unwrap();
    assert_eq!(out["selected"], "Other");
    assert_eq!(out["profiles"][0]["command_line_args"], "--verbose --fast");
    let choice = p.w.shell.update(&mut p.w.vcx, |s, _| s.launch_choice());
    assert_eq!(
        choice.profiles.get(&p.project).map(String::as_str),
        Some("Other")
    );
    assert_eq!(choice.configuration, "Debug");
    // An unknown profile is refused.
    assert!(
        p.agent(
            props::SET_LAUNCH_PROFILE,
            json!({"project": "App", "profile": "Nope", "action": "select"})
        )
        .is_err()
    );
}

/// F5's launch resolves the toolbar's profile, configuration and framework (`resolve_launch` with the choice).
#[test]
fn the_launch_follows_the_toolbars_profile_configuration_and_framework() {
    let t = tempfile::tempdir().unwrap();
    let project = t.path().join("Multi/Multi.csproj");
    let write = |p: &Path, s: &str| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    };
    write(
        &project,
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFrameworks>net8.0;net10.0</TargetFrameworks></PropertyGroup></Project>",
    );
    write(&t.path().join("Multi/bin/Release/net10.0/Multi.dll"), "");
    write(
        &t.path().join("Multi/Properties/launchSettings.json"),
        r#"{"profiles": {"First": {"commandName": "Project"}, "Other": {"commandName": "Project", "commandLineArgs": "--other"}}}"#,
    );
    let p = project.to_string_lossy().into_owned();
    let choice = super::debug::LaunchChoice {
        configuration: "Debug".into(),
        configurations: [(p.clone(), "Release".to_owned())].into(),
        frameworks: [(p.clone(), "net10.0".to_owned())].into(),
        profiles: [(p.clone(), "Other".to_owned())].into(),
        framework: None,
    };
    let c = super::debug::resolve_launch(
        None,
        None,
        std::slice::from_ref(&project),
        None,
        Some(&project),
        &choice,
    )
    .unwrap();
    assert!(
        c.program.ends_with("bin/Release/net10.0/Multi.dll"),
        "{:?}",
        c.program
    );
    assert_eq!(c.profile.as_deref(), Some("Other"));
    assert_eq!(c.args, ["--other"]);
    // eludite.debug.start's profile wins; a profile the file lost falls back to the first Project profile.
    let c = super::debug::resolve_launch(
        None,
        Some("First"),
        std::slice::from_ref(&project),
        None,
        Some(&project),
        &choice,
    )
    .unwrap();
    assert_eq!(c.profile.as_deref(), Some("First"));
    let gone = super::debug::LaunchChoice {
        profiles: [(p, "Deleted".to_owned())].into(),
        ..choice
    };
    let c = super::debug::resolve_launch(
        None,
        None,
        std::slice::from_ref(&project),
        None,
        Some(&project),
        &gone,
    )
    .unwrap();
    assert_eq!(c.profile.as_deref(), Some("First"));
}

#[gpui::test]
fn toolbar_lists_select_persist_and_the_build_uses_the_selection_and_skips_unmapped_projects(
    cx: &mut TestAppContext,
) {
    let store = tempfile::tempdir().unwrap();
    let w = setup_debug(
        cx,
        |_| {},
        None,
        Some(super::debug::DebugSetup {
            connect: None,
            search: eludite_dap::discovery::AdapterSearch::default(),
            mono: eludite_dap::discovery::MonoSearch::default(),
            mono_adapter: eludite_dap::discovery::MonoAdapterSearch::default(),
            platform: eludite_dap::launch::Platform::current(),
            store_dir: Some(store.path().to_path_buf()),
            dotnet: "dotnet".into(),
            js: Default::default(),
        }),
    );
    let mut p = ready(w);
    p.open();
    let lists = p.w.shell.read_with(&p.w.vcx, |s, _| {
        (s.solution_configurations(), s.solution_platforms())
    });
    assert_eq!(lists.0, ["Debug", "Release", "Staging"]);
    assert_eq!(lists.1, ["Any CPU", "x64"]);

    // The lists end with Configuration Manager...; choosing selects through the command.
    p.w.click(CONFIGURATION_BUTTON);
    p.w.click(&toolbar_item_selector("configuration", 2));
    p.w.click(PLATFORM_BUTTON);
    p.w.click(&toolbar_item_selector("platform", 1));
    let selected = |w: &Ws| w.shell.read_with(&w.vcx, |s, _| s.active_selection());
    if selected(&p.w) != ("Staging".to_owned(), "x64".to_owned()) {
        let entries = p.w.commands.audit_log().entries();
        let selects: Vec<String> = entries
            .iter()
            .filter(|e| e.command == "eludite.solution.select_configuration")
            .map(|e| format!("{:?} {:?}", e.arguments, e.outcome))
            .collect();
        let lists = p.w.shell.read_with(&p.w.vcx, |s, _| {
            (
                s.solution_configurations(),
                s.solution_platforms(),
                s.properties.toolbar_menu,
            )
        });
        panic!(
            "selected {:?}; select_configuration calls {selects:?}; lists {lists:?}; bounds {:?} {:?}",
            selected(&p.w),
            p.w.vcx
                .debug_bounds(toolbar_item_selector("configuration", 2).leak()),
            p.w.vcx
                .debug_bounds(toolbar_item_selector("configuration", 0).leak()),
        );
    }
    assert!(
        p.w.audit()
            .iter()
            .filter(|c| *c == "eludite.solution.select_configuration")
            .count()
            >= 2
    );
    let fake = p.w.fake.clone();
    p.w.wait("the host told the selection", |_| {
        fake.received_params("eludite/solution/setConfiguration")
            .iter()
            .any(|x| x["select"] == json!({"configuration": "Staging", "platform": "x64"}))
    });

    // Kept per solution beside the breakpoints.
    let solution = p.w.path("App.slnx");
    let file = eludite_docking::LayoutStore::new(store.path().to_path_buf())
        .solution_path(&solution)
        .with_extension("configuration.json");
    p.w.wait("the selection saved", |_| {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("Staging") && t.contains("x64"))
    });

    // The build uses it, and says which project the mapping does not build.
    p.w.vcx.simulate_keystrokes("ctrl-shift-b");
    p.w.wait("the build started", |_| fake.running_build().is_some());
    let start = &p.w.fake.received_params("eludite/build/start")[0];
    assert_eq!(start["configuration"], "Staging");
    assert_eq!(start["platform"], "x64");
    let lines = p.w.shell.read_with(&p.w.vcx, |s, cx| {
        s.output()
            .read(cx)
            .pane(OutputSource::Build)
            .tail(usize::MAX)
    });
    assert!(
        lines
            .iter()
            .any(|l| l
                == "------ Skipped Build: Project: Lib, Configuration: Release Any CPU ------"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.starts_with("Project not selected to build for this solution configuration")));
    p.w.fake.finish_build("succeeded", json!([]));
    p.w.wait("the build finished", |w| {
        w.shell.read_with(&w.vcx, |s, _| !s.builds().is_building())
    });
    // A project build uses the configuration the selection maps the project to.
    p.ui(eludite_commands::build::PROJECT, json!({"project": "App"}))
        .unwrap();
    p.w.wait("the project build", |_| {
        fake.received_params("eludite/build/start").len() >= 2
    });
    let start = &p.w.fake.received_params("eludite/build/start")[1];
    assert_eq!(start["configuration"], "Release");
    assert!(
        start.get("platform").is_none(),
        "AnyCPU is the project's default"
    );
    p.w.fake.finish_build("succeeded", json!([]));
    p.w.wait("the build finished", |w| {
        w.shell.read_with(&w.vcx, |s, _| !s.builds().is_building())
    });

    // Reopening the solution brings the selection back.
    p.w.shell.update_in(&mut p.w.vcx, |s, window, cx| {
        s.builds.configuration = "Debug".into();
        s.builds.platform = None;
        s.properties.solution = None;
        let solution = s.solution.clone().unwrap();
        s.properties_solution_opened(&solution, window, cx);
    });
    p.w.wait("the selection restored", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.active_selection() == ("Staging".to_owned(), "x64".to_owned())
        })
    });
    // A configuration the solution does not have is refused.
    assert!(
        p.agent(
            "eludite.solution.select_configuration",
            json!({"configuration": "Nope"})
        )
        .is_err()
    );
}

#[gpui::test]
fn configuration_manager_edits_the_mapping_in_the_solution(cx: &mut TestAppContext) {
    let mut p = start(cx);
    p.open();
    let generation = p.generation();
    // Build > Configuration Manager...: the command without mappings opens the dialog.
    p.w.shell.update_in(&mut p.w.vcx, |s, window, cx| {
        s.run("eludite.solution.set_configuration", json!({}), window, cx)
    });
    let manager =
        p.w.shell
            .read_with(&p.w.vcx, |s, _| s.properties.manager.clone())
            .expect("the dialog is open");
    p.w.bounds(cm::DIALOG);
    assert_eq!(
        manager.read_with(&p.w.vcx, |m, _| m.cell(1)),
        Some(("Debug".into(), "Any CPU".into(), true))
    );
    // Clearing Lib's Build edits the solution file through the host, which reloads.
    p.w.click(&cm::build_selector(1));
    let fake = p.w.fake.clone();
    p.w.wait("the mapping edit", |_| {
        fake.received_params("eludite/solution/setConfiguration")
            .iter()
            .any(|x| x["mappings"].is_array())
    });
    let sent = fake
        .received_params("eludite/solution/setConfiguration")
        .into_iter()
        .find(|x| x["mappings"].is_array())
        .unwrap();
    assert_eq!(
        sent["mappings"],
        json!([{"project": p.lib, "solutionConfiguration": "Debug", "solutionPlatform": "Any CPU", "build": false}])
    );
    p.w.wait("the dialog's row updated", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.generation == generation + 1
                && s.properties
                    .manager
                    .as_ref()
                    .is_some_and(|m| m.read(cx).cell(1).is_some_and(|c| !c.2))
        })
    });
    // An agent sees the same mapping; its edit answers when the host has written it.
    let out = p
        .agent(
            "eludite.solution.set_configuration",
            json!({"mappings": [{"project": "Lib", "build": true}]}),
        )
        .unwrap();
    assert_eq!(out["written"], true);
    assert_eq!(out["projects"][1]["mappings"][0]["build"], true);
    let c = p
        .agent("eludite.solution.configurations", json!({}))
        .unwrap();
    assert_eq!(c["format"], "slnx");
    assert_eq!(c["configurations"], json!(["Debug", "Release", "Staging"]));
    p.w.click(cm::CLOSE);
    assert!(
        p.w.shell
            .read_with(&p.w.vcx, |s, _| s.properties.manager.is_none())
    );
}

#[gpui::test]
fn the_target_framework_list_drives_a_start_and_a_test_run(cx: &mut TestAppContext) {
    let mut p = start(cx);
    let project = p.project.clone();
    let program =
        p.w.path("src/App/Program.cs")
            .to_string_lossy()
            .into_owned();
    p.w.fake.set_tree(json!([{
        "name": "App", "path": project, "kind": "sdk", "targetFrameworks": ["net8.0", "net10.0"],
        "files": [{"path": program, "itemType": "compile"}]
    }]));
    let item = |id: &str| {
        json!({"id": id, "displayName": format!("App.Tests.{id}"), "fullyQualifiedName": format!("App.Tests.{id}"),
               "namespace": "App", "className": "Tests", "method": id})
    };
    p.w.fake.set_tests(json!([
        {"container": {"id": format!("{project}|net8.0"), "name": "App (net8.0)", "project": project,
                       "targetFramework": "net8.0", "protocol": "mtp", "runtime": "dotnet"},
         "tests": [item("A8")]},
        {"container": {"id": format!("{project}|net10.0"), "name": "App (net10.0)", "project": project,
                       "targetFramework": "net10.0", "protocol": "mtp", "runtime": "dotnet"},
         "tests": [item("A10")]}
    ]));
    p.open();
    // The toolbar shows the startup project's frameworks; choosing net8.0 keeps it for F5 and Run Tests.
    p.w.click(FRAMEWORK_BUTTON);
    p.w.click(&toolbar_item_selector("framework", 0));
    let choice = p.w.shell.update(&mut p.w.vcx, |s, _| s.launch_choice());
    assert_eq!(
        choice.frameworks.get(&project).map(String::as_str),
        Some("net8.0")
    );
    // `debug.start`'s framework is the next launch's.
    p.w.shell.update(&mut p.w.vcx, |s, _| {
        s.properties.start_framework = Some("net10.0".into())
    });
    let choice = p.w.shell.update(&mut p.w.vcx, |s, _| s.launch_choice());
    assert_eq!(choice.framework.as_deref(), Some("net10.0"));
    assert!(
        p.w.shell
            .read_with(&p.w.vcx, |s, _| s.properties.start_framework.is_none())
    );

    // A test run runs the chosen framework's container only.
    p.ui(eludite_commands::test::EXPLORER, json!({})).unwrap();
    let out = p
        .agent(eludite_commands::test::RUN, json!({"wait_ms": 5000}))
        .unwrap();
    let _ = out;
    let fake = p.w.fake.clone();
    p.w.wait("the host's run", |_| {
        !fake.received_params("eludite/test/run").is_empty()
    });
    let run = &p.w.fake.received_params("eludite/test/run")[0];
    assert_eq!(
        run["containers"],
        json!([{"id": format!("{project}|net8.0")}])
    );
    p.w.wait("the run done", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.test_runs().current.is_none())
    });
    // An explicit framework wins.
    p.agent(
        eludite_commands::test::RUN,
        json!({"framework": "net10.0", "wait_ms": 5000}),
    )
    .unwrap();
    p.w.wait("the second run", |_| {
        fake.received_params("eludite/test/run").len() >= 2
    });
    assert_eq!(
        p.w.fake.received_params("eludite/test/run")[1]["containers"],
        json!([{"id": format!("{project}|net10.0")}])
    );
    // A framework the project does not list is refused by the toolbar's command.
    assert!(
        p.agent(
            "eludite.solution.select_configuration",
            json!({"framework": "net6.0", "project": "App"})
        )
        .is_err()
    );
}

#[gpui::test]
fn an_agents_properties_and_set_property_match_the_pages(cx: &mut TestAppContext) {
    let mut p = start(cx);
    p.open();
    let generation = p.generation();
    let started = Instant::now();
    let out = p
        .agent(
            props::PROPERTIES,
            json!({"project": "App", "configuration": "Release", "page": "build"}),
        )
        .unwrap();
    eprintln!(
        "timing: agent properties answered in {:.1} ms (fake host)",
        started.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(out["configuration"], "Release");
    assert!(
        out["properties"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["page"] == "build")
    );
    let names: Vec<&str> = out["properties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "DefineConstants",
            "Optimize",
            "TreatWarningsAsErrors",
            "OutputPath",
            "PreBuildEvent"
        ]
    );
    let all = p
        .agent(props::PROPERTIES, json!({"project": "App"}))
        .unwrap();
    let lang = all["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "LangVersion")
        .unwrap()
        .clone();
    assert_eq!(lang["source"], "inherited");
    assert!(
        lang["inherited_from"]
            .as_str()
            .unwrap()
            .ends_with("Directory.Build.props")
    );

    // set_property with a configuration follows the condition rule; the answer has the value after the reload.
    let set = p
        .agent(
            props::SET_PROPERTY,
            json!({"project": "App", "property": "Optimize", "value": true, "configuration": "Release"}),
        )
        .unwrap();
    assert_eq!(set["status"], "written");
    assert_eq!(
        set["condition"],
        "'$(Configuration)|$(Platform)'=='Release|AnyCPU'"
    );
    assert_eq!(set["value"], "true");
    assert_eq!(set["generation"], generation + 1);
    assert_eq!(
        p.set_property_edits()[0],
        json!([{"name": "Optimize", "value": "true", "configuration": "Release", "platform": "AnyCPU"}])
    );
    // The inherited rule: without override nothing is written.
    let inherited = p
        .agent(
            props::SET_PROPERTY,
            json!({"project": "App", "property": "LangVersion", "value": "13.0"}),
        )
        .unwrap();
    assert_eq!(inherited["status"], "inherited");
    assert!(
        inherited["inherited_from"]
            .as_str()
            .unwrap()
            .ends_with("Directory.Build.props")
    );

    // The pages show what the agent wrote.
    p.ui(
        props::PROPERTIES,
        json!({"project": "App", "open": true, "page": "build"}),
    )
    .unwrap();
    p.wait_values(None);
    p.w.click(&pages::configuration_selector(1));
    p.wait_values(Some("Release"));
    assert_eq!(p.shown("Optimize"), "true");
    assert!(
        p.note("Optimize")
            .starts_with("From the project file, under")
    );
    // From the UI thread the answer never waits: `pending`.
    let pending = p
        .ui(
            props::SET_PROPERTY,
            json!({"project": "App", "property": "Version", "value": "2.0.0"}),
        )
        .unwrap();
    assert_eq!(pending["status"], "pending");
    let audit = p.w.audit();
    assert!(audit.contains(&props::SET_PROPERTY.to_owned()));
    assert!(audit.contains(&props::PROPERTIES.to_owned()));
    let _ = Duration::ZERO;
}

/// While `--bounds-out` probes, the pages, the toolbar's lists and Configuration Manager record where their controls
/// are drawn (the Xvfb run, `crates/eludite/tools/project-properties-linux.sh`, clicks them there).
#[gpui::test]
fn probed_controls_record_their_bounds_for_the_xvfb_run(cx: &mut TestAppContext) {
    let mut p = start(cx);
    p.open();
    let probe: eludite_docking::Probe = Default::default();
    p.w.shell
        .update(&mut p.w.vcx, |s, cx| s.set_probe(Some(probe), cx));
    p.w.double_click(&row_selector(&p.project));
    p.wait_values(Some("Debug"));
    p.w.click(&pages::page_selector("build"));
    p.w.click(CONFIGURATION_BUTTON);
    p.w.vcx.run_until_parked();
    let drawn = |p: &mut Pw| {
        p.w.vcx.update(|window, _| window.refresh());
        p.w.vcx.run_until_parked();
        p.w.shell.read_with(&p.w.vcx, |s, _| {
            s.ui_bounds()
                .unwrap()
                .borrow()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let keys = drawn(&mut p);
    for k in [
        pages::SAVE.to_owned(),
        pages::page_selector("debug"),
        pages::property_selector("TreatWarningsAsErrors"),
        pages::property_selector("DefineConstants"),
        pages::configuration_selector(0),
        CONFIGURATION_BUTTON.to_owned(),
        PLATFORM_BUTTON.to_owned(),
        toolbar_item_selector("configuration", 0),
    ] {
        assert!(keys.contains(&k), "{k} not probed: {keys:?}");
    }
    // Configuration Manager... is the list's last entry.
    p.w.click(&toolbar_item_selector("configuration", 3));
    let keys = drawn(&mut p);
    for k in [cm::CLOSE.to_owned(), cm::build_selector(0)] {
        assert!(keys.contains(&k), "{k} not probed: {keys:?}");
    }
}
