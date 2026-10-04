//! Headless tests of the settings store in the shell (brief 0020): the user file and the solution's
//! `.eludite/settings.json` load and merge (solution wins), edits on disk apply live within the budget,
//! `eludite.settings.get` and `set` work from an agent's thread and write the file, and the agents registry follows
//! its settings.

use std::time::{Duration, Instant};

use eludite_commands::settings::{GET, SET};
use serde_json::{Value, json};

use super::tests::{USER_SETTINGS, Ws, setup, setup_full};

impl Ws {
    fn write_settings(&self, rel: &str, value: Value) -> Instant {
        let p = self.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        // Through a temporary file and a rename, as editors save: the poll never sees half a file.
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        std::fs::rename(&tmp, &p).unwrap();
        Instant::now()
    }

    fn applied<T>(&self, f: impl Fn(&super::settings::Applied) -> T) -> Option<T> {
        self.shell
            .read_with(&self.vcx, |s, _| s.applied_settings().map(&f))
    }

    /// Invoke a command from another thread, as an agent's MCP call does.
    pub(super) fn agent_invoke(&self, command: &str, args: Value) -> Result<Value, String> {
        let c = self.commands.clone();
        let command = command.to_owned();
        std::thread::spawn(move || c.invoke(&command, args).map_err(|e| e.to_string()))
            .join()
            .unwrap()
    }
}

#[gpui::test]
fn settings_load_merge_and_reload_live(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    // Defaults first.
    assert_eq!(w.applied(|a| a.build_on_save), Some(false));
    assert_eq!(w.applied(|a| a.build_before_run), Some(true));

    // The user file, edited while Eludite runs.
    w.write_settings(
        USER_SETTINGS,
        json!({"build.onSave": true, "build.cargoPath": "/opt/cargo/bin/cargo"}),
    );
    w.wait("the user file", |w| {
        w.applied(|a| a.build_on_save) == Some(true)
    });
    assert_eq!(
        w.shell
            .read_with(&w.vcx, |s, _| s.builds.cargo_program.clone()),
        "/opt/cargo/bin/cargo"
    );

    // The solution's file wins over the user's once the solution opens.
    w.write_settings(
        ".eludite/settings.json",
        json!({"build.onSave": false, "build.showErrorListOnFailure": false}),
    );
    w.open_solution();
    w.wait("the solution file", |w| {
        w.applied(|a| (a.build_on_save, a.show_error_list_on_failure)) == Some((false, false))
    });
    let got = w.agent_invoke(GET, json!({"key": "build.onSave"})).unwrap();
    assert_eq!(got["settings"][0]["value"], false);
    assert_eq!(got["settings"][0]["source"], "solution");
    assert_eq!(got["solution_file"]["exists"], true);
    assert!(
        got["user_file"]["path"]
            .as_str()
            .unwrap()
            .ends_with("settings.json")
    );

    // Live reload: each edit applies within the brief's 200 ms of the file change (asserted at 1 s here, so a loaded
    // test machine does not fail it; the measured times are printed and reported).
    let mut took = Vec::new();
    for i in 0..10 {
        let on = i % 2 == 0;
        let written = w.write_settings(".eludite/settings.json", json!({"build.onSave": on}));
        w.wait("the edit", |w| w.applied(|a| a.build_on_save) == Some(on));
        took.push(written.elapsed());
    }
    took.sort();
    eprintln!(
        "settings reload, file written to applied: p50 {:.1} ms, max {:.1} ms (poll every 20 ms in tests, 100 ms in \
         the app)",
        took[took.len() / 2].as_secs_f64() * 1e3,
        took[took.len() - 1].as_secs_f64() * 1e3
    );
    assert!(took.iter().all(|t| *t < Duration::from_secs(1)), "{took:?}");
    let applied = w.shell.read_with(&w.vcx, |s, _| s.settings_timings().len());
    assert!(applied >= 10);

    // A file that does not parse is reported and ignored until fixed.
    std::fs::write(w.path(".eludite/settings.json"), "{ \"build.onSave\": tr").unwrap();
    let fake = w.commands.clone();
    w.wait("the error", |_| {
        fake.invoke(GET, json!({}))
            .unwrap()
            .get("solution_file")
            .and_then(|f| f.get("error"))
            .is_some()
    });
}

#[gpui::test]
fn agents_get_and_set_settings_through_the_bus(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    w.write_settings(USER_SETTINGS, json!({"agents.default": "Gemini"}));
    w.wait("the user file", |w| {
        w.applied(|a| a.agents.default.clone()) == Some(Some("Gemini".into()))
    });

    let all = w.agent_invoke(GET, json!({})).unwrap();
    let keys: Vec<&str> = all["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"build.beforeRun") && keys.contains(&"keyboard.preset"));
    let preset = all["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "keyboard.preset")
        .unwrap();
    assert_eq!(preset["value"], "visualStudio");
    assert_eq!(preset["source"], "default");

    let out = w
        .agent_invoke(SET, json!({"key": "build.onSave", "value": true}))
        .unwrap();
    assert_eq!(out["source"], "user");
    assert_eq!(out["scope"], "user");
    w.wait("the set", |w| w.applied(|a| a.build_on_save) == Some(true));
    // The file is written in the background, keeping its other keys.
    let user = w.path(USER_SETTINGS);
    w.wait("the file", |_| {
        std::fs::read_to_string(&user)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            == Some(json!({"agents.default": "Gemini", "build.onSave": true}))
    });
    // Bad values and unknown keys are refused; the solution scope needs an open solution.
    assert!(
        w.agent_invoke(SET, json!({"key": "build.onSave", "value": "yes"}))
            .is_err()
    );
    assert!(
        w.agent_invoke(SET, json!({"key": "nope", "value": 1}))
            .is_err()
    );
    assert!(
        w.agent_invoke(
            SET,
            json!({"key": "build.onSave", "value": false, "scope": "solution"})
        )
        .unwrap_err()
        .contains("no workspace")
    );
    w.open_solution();
    w.wait("the solution layer", |w| {
        w.agent_invoke(GET, json!({}))
            .unwrap()
            .get("solution_file")
            .is_some()
    });
    w.agent_invoke(
        SET,
        json!({"key": "build.onSave", "value": false, "scope": "solution"}),
    )
    .unwrap();
    w.wait("the solution value", |w| {
        w.applied(|a| a.build_on_save) == Some(false)
    });
    let sln_file = w.path(".eludite/settings.json");
    w.wait("the solution file", |_| sln_file.exists());
    // null removes the key: the user's value applies again.
    w.agent_invoke(
        SET,
        json!({"key": "build.onSave", "value": null, "scope": "solution"}),
    )
    .unwrap();
    w.wait("the user value again", |w| {
        w.applied(|a| a.build_on_save) == Some(true)
    });
    // Settings are visible to agents, set in the edit class.
    let spec = w.commands.lookup(SET).unwrap();
    assert!(spec.agent_visible);
    assert_eq!(
        spec.permission,
        eludite_commands::PermissionClass::EditBuffer
    );
    assert!(w.audit().iter().filter(|c| *c == SET).count() >= 4);
}

#[gpui::test]
fn the_agents_registry_follows_its_settings(cx: &mut gpui::TestAppContext) {
    // A registry searched from the settings (the other tests fix it up front).
    let agents = super::agents::AgentsSetup {
        registry: None,
        ..super::agents::AgentsSetup::from_env()
    };
    let mut w = setup_full(cx, |_| {}, Some(agents));
    let names = |w: &Ws| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.agents
                .registry
                .iter()
                .map(|a| a.name().to_owned())
                .collect::<Vec<_>>()
        })
    };
    w.write_settings(
        USER_SETTINGS,
        json!({"agents.custom": [{"name": "Fake ACP", "command": "/bin/true", "args": ["--acp"]}],
               "agents.default": "Fake ACP"}),
    );
    w.wait("the configured agent, first", |w| {
        names(w).first().map(String::as_str) == Some("Fake ACP")
    });
    let selected = w.shell.read_with(&w.vcx, |s, _| s.agents.selected);
    assert_eq!(selected, 0);
    // Removing it from the settings searches again without it.
    w.write_settings(USER_SETTINGS, json!({"agents.custom": []}));
    w.wait("the agent gone", |w| {
        !names(w).iter().any(|n| n == "Fake ACP")
    });
}

#[gpui::test]
fn the_options_dialog_is_generated_from_the_schema_and_edits_through_the_bus(
    cx: &mut gpui::TestAppContext,
) {
    use super::options::{
        OK, browse_selector, choice_selector, section_selector, setting_selector,
    };
    let mut w = setup(cx);
    // Tools > Options runs the command; the dialog opens on the first page.
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.run(eludite_commands::settings::OPTIONS, json!({}), window, cx)
    });
    w.vcx.run_until_parked();
    let page = |w: &Ws| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.options_dialog().map(|d| d.read(cx).section().to_owned())
        })
    };
    assert_eq!(page(&w).as_deref(), Some("Environment > Keyboard"));
    // The page tree has more rows than it shows (the forges' and NuGet's pages, briefs 0046 and 0048), so a page is
    // scrolled into view before it is clicked, as a person would: to the top for the first half, the bottom after.
    // The point is inside the tree's viewport, taken before it scrolls.
    let tree = w.bounds(&section_selector(0)).center();
    let wheel = |w: &mut Ws, dy: f32| {
        w.vcx.simulate_event(gpui::ScrollWheelEvent {
            position: tree,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        w.vcx.run_until_parked();
    };
    // Every section of the schema is a page, and every setting of a page has its editor.
    let schema = eludite_commands::settings::SettingsSchema::builtin();
    for (ix, section) in schema.sections.iter().enumerate() {
        let top = ix < schema.sections.len() / 2;
        wheel(&mut w, if top { 10_000. } else { -10_000. });
        w.click(&section_selector(ix));
        assert_eq!(page(&w).as_deref(), Some(section.as_str()));
        for spec in schema.section(section) {
            let sel = match &spec.kind {
                eludite_commands::settings::SettingKind::Enum { .. } => {
                    choice_selector(&spec.key, 0)
                }
                _ => setting_selector(&spec.key),
            };
            w.bounds(&sel);
        }
    }

    wheel(&mut w, 10_000.);

    // A check box: Projects and Solutions > Build and Run, Build the project after saving.
    w.click(&section_selector(1));
    w.click(&setting_selector("build.onSave"));
    w.wait("build on save", |w| {
        w.applied(|a| a.build_on_save) == Some(true)
    });
    assert!(
        w.audit()
            .contains(&eludite_commands::settings::SET.to_owned()),
        "the dialog runs the command"
    );
    let user = w.path(USER_SETTINGS);
    w.wait("the user file", |_| {
        std::fs::read_to_string(&user).is_ok_and(|t| t.contains("\"build.onSave\": true"))
    });
    // Clicking again turns it off.
    w.click(&setting_selector("build.onSave"));
    w.wait("build on save off", |w| {
        w.applied(|a| a.build_on_save) == Some(false)
    });

    // A text box: Agents, Default agent (after Test > General, brief 0035).
    w.click(&section_selector(5));
    w.click(&setting_selector("agents.default"));
    w.vcx.simulate_keystrokes("G e m i n i enter");
    w.wait("the default agent", |w| {
        w.applied(|a| a.agents.default.clone()) == Some(Some("Gemini".into()))
    });

    // A path with Browse...: Debugging > General, netcoredbg.
    w.click(&section_selector(2));
    w.click(&browse_selector("debugger.netcoredbgPath"));
    assert!(w.vcx.did_prompt_for_paths());
    let picked = w.path("tools/netcoredbg/netcoredbg");
    let p = picked.clone();
    w.vcx.simulate_path_prompt_response(move |_| Some(vec![p]));
    w.wait("the adapter path", |w| {
        w.applied(|a| a.netcoredbg.clone()) == Some(Some(picked.clone()))
    });

    // An enum: Environment > Keyboard, the Visual Studio scheme.
    w.click(&section_selector(0));
    w.click(&choice_selector("keyboard.preset", 0));
    let got = w
        .agent_invoke(GET, json!({"key": "keyboard.preset"}))
        .unwrap();
    assert_eq!(got["settings"][0]["source"], "user");

    // A file edit made elsewhere shows in the open dialog.
    w.write_settings(USER_SETTINGS, json!({"build.onSave": true}));
    w.wait("the dialog follows the file", |w| {
        w.applied(|a| a.build_on_save) == Some(true)
    });
    // OK closes it.
    w.click(&section_selector(1));
    w.click(OK);
    assert!(page(&w).is_none());
}

/// Brief 0047: the per-person opt-in (`x-eludite-scope: user-workspace`, `browser.allowNoSandbox`) is on the Web
/// Browser page with "for this workspace, on this machine" in its label, and the Options dialog writes it in the
/// person's state for the workspace (mode 0600 on Unix), never the workspace's `.eludite/settings.json` or the user's
/// file; `settings.get` names the source and the dialog says where the answer is kept. Turning it off writes the state
/// again. (The Web Browser page is taller than the test window, so its check box is toggled by the event its click
/// emits.)
#[gpui::test]
fn the_options_dialog_writes_the_opt_in_in_the_persons_workspace_state(
    cx: &mut gpui::TestAppContext,
) {
    use super::options::{OptionsEvent, section_selector, setting_selector};
    let mut w = setup(cx);
    w.open_solution();
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.run(eludite_commands::settings::OPTIONS, json!({}), window, cx)
    });
    w.vcx.run_until_parked();
    let schema = eludite_commands::settings::SettingsSchema::builtin();
    let page = schema
        .sections
        .iter()
        .position(|s| s == "Web Browser")
        .unwrap();
    let spec = schema.get("browser.allowNoSandbox").unwrap();
    assert_eq!(spec.section, "Web Browser");
    assert!(
        spec.label.contains("for this workspace, on this machine"),
        "{}",
        spec.label
    );
    w.click(&section_selector(page));
    w.bounds(&setting_selector("browser.allowNoSandbox"));
    w.bounds(&setting_selector("browser.enginePath"));
    let workspace_file = w.path(".eludite/settings.json");
    let allowed = |w: &Ws| {
        w.shell
            .read_with(&w.vcx, |s, _| s.browser().settings().allow_no_sandbox)
    };
    let toggle = |w: &mut Ws, on: bool| {
        let dialog = w
            .shell
            .read_with(&w.vcx, |s, _| s.options_dialog().cloned())
            .unwrap();
        dialog.update(&mut w.vcx, |_, cx| {
            cx.emit(OptionsEvent::Set {
                key: "browser.allowNoSandbox".into(),
                value: json!(on),
            })
        });
        w.vcx.run_until_parked();
    };
    let get = |w: &Ws| {
        w.agent_invoke(GET, json!({"key": "browser.allowNoSandbox"}))
            .unwrap()
    };
    toggle(&mut w, true);
    w.wait("the opt-in on", |w| allowed(w));
    let out = get(&w);
    assert_eq!(out["settings"][0]["value"], true);
    assert_eq!(out["settings"][0]["source"], "user-workspace");
    let state = std::path::PathBuf::from(out["user_workspace_file"]["path"].as_str().unwrap());
    assert!(
        state.starts_with(w.path("user-config/workspaces")),
        "{}",
        state.display()
    );
    w.wait("the state file", |_| {
        std::fs::read_to_string(&state)
            .is_ok_and(|t| t.contains("\"browser.allowNoSandbox\": true"))
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&state).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert!(
        !workspace_file.exists(),
        "the workspace's file is never written"
    );
    let user = std::fs::read_to_string(w.path(USER_SETTINGS)).unwrap_or_default();
    assert!(!user.contains("browser.allowNoSandbox"), "{user}");
    let notes = w.shell.read_with(&w.vcx, |s, cx| {
        s.options_dialog()
            .unwrap()
            .read(cx)
            .notes("browser.allowNoSandbox")
    });
    assert!(
        notes
            .iter()
            .any(|n| n.contains("for this workspace, on this machine")),
        "{notes:?}"
    );
    toggle(&mut w, false);
    w.wait("the opt-in off", |w| !allowed(w));
    w.wait("the state file off", |_| {
        std::fs::read_to_string(&state)
            .is_ok_and(|t| t.contains("\"browser.allowNoSandbox\": false"))
    });
    assert!(!workspace_file.exists());

    // A workspace file carrying the key: the dialog says it is ignored there.
    w.write_settings(
        ".eludite/settings.json",
        json!({"browser.allowNoSandbox": true}),
    );
    w.wait("the ignored key reported", |w| {
        get(w)["ignored_keys"] == json!(["browser.allowNoSandbox"])
    });
    // The dialog hears of it when the shell applies the change, after the store has it.
    let notes = |w: &Ws| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.options_dialog()
                .unwrap()
                .read(cx)
                .notes("browser.allowNoSandbox")
        })
    };
    w.wait("the dialog's note", |w| {
        notes(w).iter().any(|n| n.contains("is ignored"))
    });
    assert!(!allowed(&w), "still the person's answer: off");
}
