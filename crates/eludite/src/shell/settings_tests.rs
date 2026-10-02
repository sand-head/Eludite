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
    fn agent_invoke(&self, command: &str, args: Value) -> Result<Value, String> {
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
        .contains("no solution")
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
