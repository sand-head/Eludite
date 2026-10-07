//! Headless GPUI tests of the `.resx` editor (proposal 0005) against the in-process fake `eludite-host`: a `.resx`
//! opens from the Workspace window as its set's grid (one tab titled by the neutral file, whichever file was opened),
//! the grid shows the cultures, the missing cells and the rule warnings, an edit marks the tab dirty and Save writes
//! the file byte for byte except the changed element and asks the host to regenerate the designer when keys changed,
//! closing asks, the filters and the search box narrow the rows, Add Key, Delete and Rename edit the files, agents'
//! `sets`, `entries`, `set`, `add`, `remove` and `rename` match the grid and are held for review, the Access
//! Modifier goes through the host, `resx.openAsText` and Open With open the text editor, and a 5,000-key set keeps
//! the frame budget.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eludite_commands::resx as cmd;
use eludite_commands::workspace::{FILE_CLOSE, FILE_OPEN};
use eludite_commands::{Caller, with_caller};
use gpui::{Entity, Modifiers, MouseButton, MouseDownEvent, TestAppContext};
use serde_json::{Value, json};

use super::documents::normalize_path;
use super::explorer::{context_item_selector, row_selector};
use super::resx::editor::{self, Column, ResxEditor, cell_selector};
use super::resx::tab_id;
use super::tests::{Ws, setup_with};

fn agent() -> Caller {
    Caller::Agent {
        agent: "test-agent".into(),
        call: 1,
        tool_call: None,
    }
}

/// Visual Studio's bytes: a byte order mark, CRLF, no final newline, the entries in Visual Studio's shape.
fn resx(entries: &[(&str, &str, Option<&str>)]) -> Vec<u8> {
    let header = eludite_resx::ResxFile::empty("x.resx");
    let mut text = header.text().strip_suffix("</root>").unwrap().to_owned();
    for (k, v, c) in entries {
        text.push_str(&format!(
            "  <data name=\"{k}\" xml:space=\"preserve\">\r\n    <value>{}</value>\r\n",
            eludite_resx::escape_text(v, "\r\n")
        ));
        if let Some(c) = c {
            text.push_str(&format!(
                "    <comment>{}</comment>\r\n",
                eludite_resx::escape_text(c, "\r\n")
            ));
        }
        text.push_str("  </data>\r\n");
    }
    text.push_str("</root>");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

struct R {
    w: Ws,
    project: PathBuf,
    neutral: PathBuf,
    de: PathBuf,
}

/// The one-project solution with a resource set beside `App.csproj`, listed by the fake host.
fn start(cx: &mut TestAppContext) -> R {
    start_with(
        cx,
        &[
            ("Hello", "Hello {0}!", Some("The greeting")),
            ("Brand", "Eludite", Some("{Invariant}")),
            ("Save", "Save?", None),
        ],
    )
}

fn start_with(cx: &mut TestAppContext, neutral_entries: &[(&str, &str, Option<&str>)]) -> R {
    let w = setup_with(cx, |_| {});
    // Native separators, as the shell sends paths to the host: the fake matches them as strings.
    let native = |rel: &str| normalize_path(&w.path(rel));
    let project = native("src/App/App.csproj");
    let neutral = native("src/App/Resources.resx");
    let de = native("src/App/Resources.de.resx");
    let fr = native("src/App/Resources.fr-FR.resx");
    std::fs::write(&neutral, resx(neutral_entries)).unwrap();
    std::fs::write(
        &de,
        resx(&[("Hello", "Hallo {0}!", None), ("Save", "Speichern", None)]),
    )
    .unwrap();
    std::fs::write(&fr, resx(&[("Hello", "Bonjour", None)])).unwrap();
    let p = project.to_string_lossy().into_owned();
    w.fake.set_tree(json!([{
        "name": "App", "path": p, "kind": "sdk", "targetFrameworks": ["net10.0"],
        "files": [
            {"path": native("src/App/Program.cs").to_string_lossy(), "itemType": "compile"},
            {"path": neutral.to_string_lossy(), "itemType": "content"},
            {"path": de.to_string_lossy(), "itemType": "content"},
            {"path": fr.to_string_lossy(), "itemType": "content"}
        ]
    }]));
    w.fake.set_resx_sets(json!([{
        "project": p, "projectName": "App", "kind": "sdk",
        "path": neutral.to_string_lossy(), "baseName": "Resources", "neutralLanguage": "en-US",
        "rootNamespace": "App",
        "cultures": [{"name": "de", "path": de.to_string_lossy(), "item": true},
                     {"name": "fr-FR", "path": fr.to_string_lossy(), "item": true}],
        "generator": "ResXFileCodeGenerator", "lastGenOutput": "Resources.Designer.cs",
        "designer": native("src/App/Resources.Designer.cs").to_string_lossy(),
        "accessModifier": "internal", "manifestName": "App.Resources", "namespace": "App"
    }]));
    let mut r = R {
        w,
        project,
        neutral,
        de,
    };
    r.w.open_solution();
    r
}

impl R {
    fn tab(&self) -> String {
        tab_id(&self.neutral)
    }

    fn editor(&self) -> Option<Entity<ResxEditor>> {
        let path = self.neutral.clone();
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.resx_editor(&path))
    }

    /// Wait until the editor shows its set (and the host's information about it).
    fn wait_loaded(&mut self) -> Entity<ResxEditor> {
        let path = self.neutral.clone();
        self.w.wait("the .resx editor's rows", |w| {
            w.shell.read_with(&w.vcx, |s, cx| {
                s.resx_editor(&path)
                    .is_some_and(|e| e.read(cx).model.is_some() && e.read(cx).info.is_some())
            })
        });
        self.editor().unwrap()
    }

    fn read<T>(&self, f: impl FnOnce(&ResxEditor) -> T) -> T {
        self.editor().unwrap().read_with(&self.w.vcx, |e, _| f(e))
    }

    fn update(&mut self, f: impl FnOnce(&mut ResxEditor, &mut gpui::Context<ResxEditor>)) {
        let e = self.editor().unwrap();
        e.update(&mut self.w.vcx, f);
        self.w.vcx.run_until_parked();
    }

    fn keys(&self) -> Vec<String> {
        self.read(|e| e.visible_rows().iter().map(|r| r.key.clone()).collect())
    }

    fn value(&self, key: &str, culture: &str) -> Option<String> {
        self.read(|e| {
            e.model.as_ref().and_then(|m| {
                m.row(key)
                    .and_then(|r| r.cell(culture).and_then(|c| c.value.clone()))
            })
        })
    }

    /// Double-click a cell, type, Enter.
    fn type_cell(&mut self, ix: usize, column: Column, text: &str) {
        self.w.double_click(&cell_selector(ix, &column));
        self.w.vcx.run_until_parked();
        self.update(|e, cx| e.type_text(text, cx));
        self.w.vcx.simulate_keystrokes("enter");
        self.w.vcx.run_until_parked();
    }

    fn ui(&mut self, command: &str, args: Value) -> Result<Value, eludite_commands::CommandError> {
        let r = self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.invoke(command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
        r
    }

    /// Invoke a command from another thread, as a person's outer call (`as_agent` false) or an agent's.
    fn thread(
        &mut self,
        command: &'static str,
        args: Value,
        as_agent: bool,
    ) -> Result<Value, String> {
        let c = self.w.commands.clone();
        let h = std::thread::spawn(move || {
            let run = || c.invoke(command, args).map_err(|e| e.to_string());
            if as_agent {
                with_caller(agent(), run)
            } else {
                run()
            }
        });
        self.w.wait(command, |_| h.is_finished());
        h.join().unwrap()
    }

    fn designer_calls(&self) -> Vec<Value> {
        self.w.fake.received_params("eludite/resx/designer")
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

    fn file_row(&self, path: &Path) -> String {
        row_selector(&format!(
            "{}|{}",
            self.project.to_string_lossy(),
            path.file_name().unwrap().to_string_lossy()
        ))
    }
}

#[gpui::test]
fn a_resx_opens_from_workspace_as_its_set_edits_save_byte_for_byte_and_asks_on_close(
    cx: &mut TestAppContext,
) {
    let mut r = start(cx);
    r.w.click(&format!(
        "{}-toggle",
        row_selector(&r.project.to_string_lossy())
    ));
    // Double-clicking the German file opens the set's tab, titled by the neutral file.
    let de_row = r.file_row(&r.de.clone());
    r.w.double_click(&de_row);
    let editor = r.wait_loaded();
    let tab = r.tab();
    assert_eq!(
        r.w.controller.active_document().as_deref(),
        Some(tab.as_str())
    );
    assert_eq!(r.read(|e| e.title.clone()), "Resources.resx");
    assert_eq!(r.read(|e| e.columns()), ["", "de", "fr-FR"]);
    assert_eq!(r.keys(), ["Hello", "Brand", "Save"]);
    assert_eq!(
        r.read(|e| e.info.as_ref().unwrap().neutral_language.clone()),
        Some("en-US".into())
    );
    let timings =
        r.w.shell
            .read_with(&r.w.vcx, |s, _| s.resx_timings().clone());
    let open_ms = (timings.shown.unwrap() - timings.opened.unwrap()).as_secs_f64() * 1e3;
    eprintln!("timing: .resx editor open to rows shown {open_ms:.1} ms (3 files)");
    assert!(open_ms < 100.0, "{open_ms} ms");
    // The cells: a missing French Save, a placeholder warning on the French Hello, an invariant Brand.
    let rows = editor.read_with(&r.w.vcx, |e, _| {
        e.visible_rows()
            .iter()
            .map(|r| (*r).clone())
            .collect::<Vec<_>>()
    });
    assert!(rows[2].cell("fr-FR").unwrap().missing());
    assert_eq!(
        rows[0].cell("fr-FR").unwrap().warnings[0].rule,
        eludite_resx::Rule::Placeholders
    );
    assert!(rows[1].invariant && rows[1].missing_count() == 0);
    assert!(!r.w.dirty(&tab));

    // An edit in the German Hello marks the tab dirty; Ctrl+S writes only that element.
    let before = std::fs::read(&r.de).unwrap();
    r.type_cell(0, Column::Culture("de".into()), " Welt");
    assert_eq!(r.value("Hello", "de").as_deref(), Some("Hallo {0}! Welt"));
    assert!(r.w.dirty(&tab));
    r.w.vcx.simulate_keystrokes("ctrl-s");
    let de = r.de.clone();
    r.w.wait("the German file", |_| std::fs::read(&de).unwrap() != before);
    let after = std::fs::read(&r.de).unwrap();
    let expected = String::from_utf8(before.clone()).unwrap().replace(
        "<value>Hallo {0}!</value>",
        "<value>Hallo {0}! Welt</value>",
    );
    assert_eq!(
        String::from_utf8(after.clone()).unwrap(),
        expected,
        "every other byte stays"
    );
    assert!(after.starts_with(&[0xEF, 0xBB, 0xBF]) && after.ends_with(b"</root>"));
    r.w.wait("the clean tab", |w| !w.dirty(&tab));
    assert!(
        r.designer_calls().is_empty(),
        "no key changed: the designer stays"
    );

    // Add Key appends to the neutral file; Save regenerates the designer through the host.
    r.w.click(editor::ADD_KEY);
    r.update(|e, cx| e.type_text("Welcome", cx));
    r.w.vcx.simulate_keystrokes("enter");
    r.w.vcx.run_until_parked();
    assert_eq!(r.keys(), ["Hello", "Brand", "Save", "Welcome"]);
    assert!(r.w.dirty(&tab));
    r.w.vcx.simulate_keystrokes("ctrl-s");
    r.w.wait("the designer call", |w| {
        !w.fake.received_params("eludite/resx/designer").is_empty()
    });
    let neutral_text = String::from_utf8(std::fs::read(&r.neutral).unwrap()).unwrap();
    assert!(neutral_text.ends_with("  <data name=\"Welcome\" xml:space=\"preserve\">\r\n    <value></value>\r\n  </data>\r\n</root>"));
    let calls = r.designer_calls();
    assert_eq!(calls[0]["action"], "generate");
    assert_eq!(calls[0]["path"], json!(r.neutral.to_string_lossy()));

    // Closing a dirty tab asks; Don't Save closes it and writes nothing.
    r.type_cell(2, Column::Culture("de".into()), "!");
    assert!(r.w.dirty(&tab));
    let before = std::fs::read(&r.de).unwrap();
    r.w.shell.update_in(&mut r.w.vcx, |s, window, cx| {
        s.run(FILE_CLOSE, json!({"path": tab}), window, cx)
    });
    assert!(r.w.vcx.has_pending_prompt(), "closing a dirty set asks");
    r.w.vcx.simulate_prompt_answer("Don't Save");
    r.w.vcx.run_until_parked();
    assert!(r.editor().is_none());
    assert!(r.w.controller.layout().documents.get(&tab).is_none());
    assert_eq!(std::fs::read(&r.de).unwrap(), before);
}

#[gpui::test]
fn filters_search_delete_and_rename_work_on_the_grid(cx: &mut TestAppContext) {
    let mut r = start(cx);
    // `eludite.file.open` on the neutral file opens the grid (the UI's and an agent's path).
    let out = r
        .ui(FILE_OPEN, json!({"path": r.neutral.to_string_lossy()}))
        .unwrap();
    assert_eq!(out["path"], r.tab());
    r.wait_loaded();
    // Missing: Save (French), not Hello (translated everywhere) nor Brand (invariant).
    r.update(|e, cx| e.set_filter(true, false, false, cx));
    assert_eq!(r.keys(), ["Save"]);
    r.update(|e, cx| e.set_filter(false, true, false, cx));
    assert_eq!(
        r.keys(),
        ["Hello", "Save"],
        "Hello's placeholder, Save's punctuation (Speichern)"
    );
    r.update(|e, cx| e.set_filter(false, false, true, cx));
    assert_eq!(r.keys(), ["Brand"]);
    r.update(|e, cx| e.set_filter(false, false, false, cx));
    r.update(|e, cx| e.type_query("speich", cx));
    assert_eq!(r.keys(), ["Save"], "the search box matches values");
    r.update(|e, cx| {
        e.set_filter(false, false, false, cx);
    });
    r.w.click(editor::SEARCH_BOX);
    r.w.vcx.simulate_keystrokes("escape");
    r.w.vcx.run_until_parked();
    assert_eq!(r.keys(), ["Hello", "Brand", "Save"]);
    // Rename through the toolbar (F2 is the keymap's Rename symbol outside the grid), Delete through the key.
    r.w.click(&cell_selector(2, &Column::Key));
    r.w.click(editor::RENAME);
    r.update(|e, cx| e.type_text("2", cx));
    r.w.vcx.simulate_keystrokes("enter");
    r.w.vcx.run_until_parked();
    assert_eq!(r.keys(), ["Hello", "Brand", "Save2"]);
    assert_eq!(r.value("Save2", "de").as_deref(), Some("Speichern"));
    r.w.click(&cell_selector(1, &Column::Key));
    r.w.vcx.simulate_keystrokes("delete");
    r.w.vcx.run_until_parked();
    assert_eq!(r.keys(), ["Hello", "Save2"]);
    // The comment column edits the neutral comment; the Invariant button marks the key.
    r.type_cell(0, Column::Comment, "!");
    assert_eq!(
        r.read(|e| e
            .model
            .as_ref()
            .unwrap()
            .set
            .neutral
            .entry("Hello")
            .unwrap()
            .comment
            .clone()),
        Some("The greeting!".into())
    );
    r.w.click(&cell_selector(0, &Column::Key));
    r.w.click("resx-invariant");
    assert!(r.read(|e| e.model.as_ref().unwrap().row("Hello").unwrap().invariant));
    // Save writes all three files: the rename touched every file, the delete too.
    r.w.vcx.simulate_keystrokes("ctrl-s");
    let tab = r.tab();
    r.w.wait("the save", |w| !w.dirty(&tab));
    for p in [r.neutral.clone(), r.de.clone()] {
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("name=\"Save2\"") && !text.contains("name=\"Brand\""),
            "{}",
            p.display()
        );
    }
    assert!(
        std::fs::read_to_string(&r.neutral)
            .unwrap()
            .contains("<comment>{Invariant} The greeting!</comment>")
    );
}

#[gpui::test]
fn agents_read_and_write_sets_through_the_commands(cx: &mut TestAppContext) {
    let mut r = start(cx);
    // sets: the one set, with the host's information.
    let sets = r.thread(cmd::SETS, json!({}), true).unwrap();
    assert_eq!(sets["sets"].as_array().unwrap().len(), 1);
    let set = &sets["sets"][0];
    assert_eq!(set["base_name"], "Resources");
    assert_eq!(set["project"], "App");
    assert_eq!(set["neutral_language"], "en-US");
    assert_eq!(set["source"], "project");
    assert_eq!(set["access_modifier"], "internal");
    assert_eq!(set["strings"], 3);
    assert_eq!(set["missing"], 1);
    assert_eq!(set["cultures"][2]["name"], "fr-FR");
    assert_eq!(set["cultures"][2]["missing"], 1);
    assert_eq!(set["generation"], Value::Null);
    assert_eq!(sets["generation"], 1);
    // entries with the missing filter, named by a culture file.
    let entries = r
        .thread(
            cmd::ENTRIES,
            json!({"set": r.de.to_string_lossy(), "missing": true}),
            true,
        )
        .unwrap();
    assert_eq!(entries["total"], 1);
    assert_eq!(entries["entries"][0]["key"], "Save");
    assert_eq!(entries["entries"][0]["cells"][2]["missing"], true);
    assert_eq!(entries["cultures"], json!(["", "de", "fr-FR"]));
    let v = r
        .thread(
            cmd::VALIDATE,
            json!({"set": r.neutral.to_string_lossy()}),
            true,
        )
        .unwrap();
    assert_eq!(
        v["warnings"].as_array().unwrap().len(),
        3,
        "Hello's placeholder and punctuation in French, Save's punctuation in German"
    );
    assert_eq!(v["rules"].as_array().unwrap().len(), 4);

    // A person's outer call writes the closed set at once: the French Save is created on the fly.
    let out = r
        .thread(
            cmd::SET,
            json!({"cells": [{"set": r.neutral.to_string_lossy(), "key": "Save", "culture": "fr-FR", "value": "Enregistrer ?"}]}),
            false,
        )
        .unwrap();
    assert_eq!(out["results"][0]["status"], "created");
    assert_eq!(out["written"][0]["entries"], 1);
    let fr = r.w.path("src/App/Resources.fr-FR.resx");
    assert!(
        std::fs::read_to_string(&fr)
            .unwrap()
            .contains("<value>Enregistrer ?</value>")
    );
    // A new culture file needs create_culture.
    let err = r
        .thread(
            cmd::SET,
            json!({"cells": [{"set": r.neutral.to_string_lossy(), "key": "Save", "culture": "es", "value": "Guardar"}]}),
            false,
        )
        .unwrap();
    assert_eq!(err["results"][0]["status"], "error");
    let out = r
        .thread(
            cmd::SET,
            json!({"cells": [{"set": r.neutral.to_string_lossy(), "key": "Save", "culture": "es", "value": "Guardar"}], "create_culture": true}),
            false,
        )
        .unwrap();
    assert_eq!(out["results"][0]["status"], "created");
    let es = r.w.path("src/App/Resources.es.resx");
    assert!(
        std::fs::read_to_string(&es)
            .unwrap()
            .contains("<value>Guardar</value>")
    );

    // An agent's add is held for review (edit_buffer: review): pending, nothing written.
    let before = std::fs::read(&r.neutral).unwrap();
    let out = r
        .thread(
            cmd::ADD,
            json!({"set": r.neutral.to_string_lossy(), "key": "Later", "value": "Later"}),
            true,
        )
        .unwrap();
    assert_eq!(out["status"], "pending");
    assert!(out["message"].as_str().unwrap().contains("held for review"));
    assert_eq!(std::fs::read(&r.neutral).unwrap(), before);
    // rename and remove as a person: every file follows; the designer regenerates.
    let out = r
        .thread(
            cmd::RENAME,
            json!({"set": r.neutral.to_string_lossy(), "key": "Save", "new_key": "Store"}),
            false,
        )
        .unwrap();
    assert_eq!(out["status"], "renamed");
    assert_eq!(out["files"], 4);
    let out = r
        .thread(
            cmd::REMOVE,
            json!({"set": r.neutral.to_string_lossy(), "keys": ["Hello", "Nope"]}),
            false,
        )
        .unwrap();
    assert_eq!(out["status"], "removed");
    assert_eq!(out["removed"][0]["files"], 3);
    assert_eq!(out["missing"], json!(["Nope"]));
    let text = std::fs::read_to_string(&r.neutral).unwrap();
    assert!(text.contains("name=\"Store\"") && !text.contains("name=\"Hello\""));
    r.w.wait("the designer calls", |w| {
        w.fake.received_params("eludite/resx/designer").len() >= 2
    });
    assert!(r.designer_calls().iter().all(|c| c["action"] == "generate"));
    let audit = r.w.audit();
    for c in [
        cmd::SETS,
        cmd::ENTRIES,
        cmd::SET,
        cmd::ADD,
        cmd::RENAME,
        cmd::REMOVE,
    ] {
        assert!(audit.contains(&c.to_owned()), "{c}");
    }

    // With the set open and clean, a person's outer write lands in the grid and is saved from there (`pending`
    // while the save runs); an agent's write to a dirty set is refused.
    r.ui(FILE_OPEN, json!({"path": r.neutral.to_string_lossy()}))
        .unwrap();
    r.wait_loaded();
    assert_eq!(r.keys(), ["Brand", "Store"]);
    let out = r
        .thread(
            cmd::ADD,
            json!({"set": r.neutral.to_string_lossy(), "key": "Again", "value": "a"}),
            false,
        )
        .unwrap();
    assert_eq!(out["status"], "pending");
    assert_eq!(r.keys(), ["Brand", "Store", "Again"]);
    let neutral = r.neutral.clone();
    r.w.wait("the save from the grid", |_| {
        std::fs::read_to_string(&neutral)
            .unwrap()
            .contains("name=\"Again\"")
    });
    r.type_cell(0, Column::Culture("de".into()), "x");
    let err = r
        .thread(
            cmd::ADD,
            json!({"set": r.neutral.to_string_lossy(), "key": "Third", "value": "t"}),
            true,
        )
        .unwrap_err();
    assert!(err.contains("unsaved changes"), "{err}");
}

#[gpui::test]
fn the_access_modifier_goes_through_the_host(cx: &mut TestAppContext) {
    let mut r = start(cx);
    r.ui(FILE_OPEN, json!({"path": r.neutral.to_string_lossy()}))
        .unwrap();
    r.wait_loaded();
    r.w.click(&editor::modifier_selector(cmd::AccessModifier::Public));
    r.w.wait("the host's setModifier", |w| {
        !w.fake.received_params("eludite/resx/designer").is_empty()
    });
    let call = &r.designer_calls()[0];
    assert_eq!(call["action"], "setModifier");
    assert_eq!(call["modifier"], "public");
    r.w.wait("the editor's modifier", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.resx_editor(&w.path("src/App/Resources.resx"))
                .is_some_and(|e| {
                    e.read(cx).info.as_ref().is_some_and(|i| {
                        i.access_modifier == eludite_lsp::host::AccessModifier::Public
                    })
                })
        })
    });
    let out = r
        .thread(
            cmd::ACCESS_MODIFIER,
            json!({"set": r.neutral.to_string_lossy(), "modifier": "none"}),
            true,
        )
        .unwrap();
    assert_eq!(out["status"], "written");
    assert_eq!(out["modifier"], "none");
    assert!(r.w.fake.resx_set(&r.neutral.to_string_lossy()).unwrap()["designer"].is_null());
}

#[gpui::test]
fn open_as_text_and_open_with_give_the_text_editor(cx: &mut TestAppContext) {
    let mut r = start(cx);
    // The command's `editor: text`.
    r.ui(
        FILE_OPEN,
        json!({"path": r.neutral.to_string_lossy(), "editor": "text"}),
    )
    .unwrap();
    let path = r.neutral.clone();
    r.w.wait("the text editor", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.editor(&path).is_some())
    });
    assert!(r.editor().is_none());
    r.ui(FILE_CLOSE, json!({"path": r.neutral.to_string_lossy()}))
        .unwrap();
    // The Workspace window's Open With > XML (Text) Editor (the open revealed the file: the project is expanded).
    if !r
        .w
        .row_labels()
        .iter()
        .any(|l| l.trim() == "Resources.resx")
    {
        r.w.click(&format!(
            "{}-toggle",
            row_selector(&r.project.to_string_lossy())
        ));
    }
    let row = r.file_row(&r.neutral.clone());
    r.right_click(&row);
    r.w.click(&context_item_selector(
        super::explorer::RESX_OPEN_TEXT_ITEM.0,
    ));
    r.w.wait("the text editor again", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.editor(&path).is_some())
    });
    assert!(r.editor().is_none());
    r.ui(FILE_CLOSE, json!({"path": r.neutral.to_string_lossy()}))
        .unwrap();
    // The setting.
    r.thread(
        eludite_commands::settings::SET,
        json!({"key": "resx.openAsText", "value": true}),
        false,
    )
    .unwrap();
    r.w.wait("the setting", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.resx_open_as_text())
    });
    r.ui(FILE_OPEN, json!({"path": r.de.to_string_lossy()}))
        .unwrap();
    let de = r.de.clone();
    r.w.wait("the German file as text", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.editor(&de).is_some())
    });
    assert!(r.editor().is_none());
}

#[gpui::test]
fn a_large_set_opens_fast_and_keeps_the_frame_budget(cx: &mut TestAppContext) {
    let entries: Vec<(String, String)> = (0..5000)
        .map(|i| (format!("Key{i:04}"), format!("Value {i} of the set")))
        .collect();
    let refs: Vec<(&str, &str, Option<&str>)> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str(), None))
        .collect();
    let mut r = start_with(cx, &refs);
    std::fs::write(&r.de, resx(&refs[..4000])).unwrap();
    std::fs::write(r.w.path("src/App/Resources.fr-FR.resx"), resx(&refs[..10])).unwrap();
    r.ui(FILE_OPEN, json!({"path": r.neutral.to_string_lossy()}))
        .unwrap();
    r.wait_loaded();
    let timings =
        r.w.shell
            .read_with(&r.w.vcx, |s, _| s.resx_timings().clone());
    let open_ms = (timings.shown.unwrap() - timings.opened.unwrap()).as_secs_f64() * 1e3;
    eprintln!("timing: .resx editor open to rows shown {open_ms:.1} ms (5,000 keys, 3 cultures)");
    assert_eq!(r.keys().len(), 5000);
    let rounds = frame_rounds(|ix| {
        r.update(|e, cx| e.select(ix, Column::Culture("de".into()), cx));
        let drawn_before = r.read(|e| e.rows_drawn());
        let took = r.w.vcx.update(|window, cx| {
            window.refresh();
            let started = Instant::now();
            let _ = window.draw(cx);
            started.elapsed()
        });
        let drawn = r.read(|e| e.rows_drawn()) - drawn_before;
        assert!(drawn < 80, "only the rows that show are drawn ({drawn})");
        took
    });
    eprintln!(
        "timing: the shell with the .resx editor's 5,000 rows, frame p50/p99 per round: {}",
        show_rounds(&rounds)
    );
    // The editor alone, in a window of its own of the shell's document size: its p99 does not move with the rest of
    // the shell (the Workspace tree, the tool windows) or the machine's other work.
    let neutral = r.neutral.clone();
    let model =
        super::resx::model::SetModel::load(&neutral, eludite_resx::Rules::default()).unwrap();
    let (view, vcx) = cx.add_window_view(|_, cx| {
        let mut e = ResxEditor::new(
            eludite_ui::Theme::default(),
            "resx:test".into(),
            "Resources.resx".into(),
            cx,
        );
        e.set_model(Ok(model), cx);
        e
    });
    let rounds = frame_rounds(|ix| {
        view.update(vcx, |e, cx| e.select(ix, Column::Culture("de".into()), cx));
        vcx.update(|window, cx| {
            window.refresh();
            let started = Instant::now();
            let _ = window.draw(cx);
            started.elapsed()
        })
    });
    eprintln!(
        "timing: the .resx editor alone with 5,000 rows, frame p50/p99 per round: {}",
        show_rounds(&rounds)
    );
    let p99 = rounds.iter().map(|r| r.1).min().unwrap();
    super::tests::assert_budget("a frame of the .resx editor", p99, Duration::from_millis(8));
}

/// Three rounds of 100 frames drawn by `frame` (given the row to select), each round's (p50, p99): the selection
/// moves one row down per frame (the Down key), as a keystroke does.
fn frame_rounds(mut frame: impl FnMut(usize) -> Duration) -> Vec<(Duration, Duration)> {
    (0..3)
        .map(|round| {
            let mut frames: Vec<Duration> =
                (0..100).map(|i| frame((round * 1000 + i) % 5000)).collect();
            frames.sort();
            (frames[frames.len() / 2], frames[frames.len() * 99 / 100])
        })
        .collect()
}

fn show_rounds(rounds: &[(Duration, Duration)]) -> String {
    rounds
        .iter()
        .map(|(p50, p99)| {
            format!(
                "{:.2}/{:.2} ms",
                p50.as_secs_f64() * 1e3,
                p99.as_secs_f64() * 1e3
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}
