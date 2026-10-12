//! Brief 0070's headless tests: the unsaved-changes question on quit, Save All, save before build, reload after an
//! outside change, encodings through the shell, the tab well's menu and keys, the IDE Navigator's MRU order, Reopen
//! Closed Tab, documents restored after a restart, bookmarks through edits and a restart, New File and Save As.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eludite_commands::documents as cmd;
use eludite_commands::settings::SettingScope;
use eludite_commands::{with_caller, workspace};
use eludite_editor::EditorView;
use gpui::{Entity, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext};
use serde_json::{Value, json};

use super::tests::{Ws, setup, setup_services};

fn agent() -> eludite_commands::Caller {
    eludite_commands::Caller::Agent {
        agent: "test-agent".into(),
        call: 1,
        tool_call: None,
    }
}

impl Ws {
    /// Run a command as the UI does (questions and dialogs first).
    fn d_ui(&mut self, command: &str, args: Value) {
        let command = command.to_owned();
        self.shell.update_in(&mut self.vcx, |s, window, cx| {
            s.run(&command, args, window, cx)
        });
        self.vcx.run_until_parked();
    }

    /// Invoke a command on the bus from the UI thread, returning its result.
    fn d_invoke_ui(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let command = command.to_owned();
        let out = self.shell.update_in(&mut self.vcx, |s, window, cx| {
            s.invoke(&command, args, window, cx)
        });
        self.vcx.run_until_parked();
        out.map_err(|e| e.to_string())
    }

    /// Invoke a command as an agent, from another thread.
    fn d_agent(&mut self, command: &str, args: Value) -> Result<Value, String> {
        let commands = self.commands.clone();
        let command = command.to_owned();
        let t = std::thread::spawn(move || {
            with_caller(agent(), || commands.invoke(&command, args)).map_err(|e| e.to_string())
        });
        let deadline = std::time::Instant::now() + super::tests::hang_bound();
        while !t.is_finished() {
            assert!(std::time::Instant::now() < deadline, "the agent timed out");
            self.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        t.join().unwrap()
    }

    fn d_open(&mut self, path: &Path) -> Entity<EditorView> {
        self.d_ui(
            workspace::FILE_OPEN,
            json!({ "path": path.to_string_lossy() }),
        );
        self.editor(path)
    }

    fn d_type_at_start(&mut self, view: &Entity<EditorView>, text: &str) {
        let text = text.to_owned();
        view.update(&mut self.vcx, |v, cx| {
            v.update_editor(cx, |e| {
                e.set_caret(0);
                e.insert(&text);
            })
        });
        self.vcx.run_until_parked();
    }

    fn d_set_setting(&mut self, key: &str, value: Value) {
        self.shell.update(&mut self.vcx, |s, _| {
            s.settings.set(key, value, SettingScope::User).unwrap();
        });
        self.vcx.run_until_parked();
    }

    fn d_quits(&self) -> u32 {
        self.shell.read_with(&self.vcx, |s, _| s.docs.quits)
    }

    fn d_should_close(&mut self) -> bool {
        let close = self.shell.update_in(&mut self.vcx, |s, window, cx| {
            s.should_close_window(window, cx)
        });
        self.vcx.run_until_parked();
        close
    }

    fn d_poll_disk(&mut self) {
        self.shell
            .update_in(&mut self.vcx, |s, window, cx| s.reload_poll(window, cx));
        self.vcx.run_until_parked();
    }

    fn d_tabs(&self) -> Vec<String> {
        self.controller
            .layout()
            .documents
            .tabs
            .iter()
            .map(|t| t.id.clone())
            .collect()
    }

    fn d_active(&self) -> Option<String> {
        self.controller.active_document()
    }

    fn d_caret_line(&self, view: &Entity<EditorView>) -> u32 {
        view.read_with(&self.vcx, |v, _| v.editor().primary_head().row + 1)
    }

    fn d_mouse_down(&mut self, sel: &str, button: MouseButton) {
        let position = self.settled_bounds(sel).center();
        self.vcx.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::none(),
            button,
            click_count: 1,
            first_mouse: false,
        });
        self.vcx.simulate_event(MouseUpEvent {
            position,
            modifiers: Modifiers::none(),
            button,
            click_count: 1,
        });
        self.vcx.run_until_parked();
    }
}

fn id(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[gpui::test]
fn quitting_with_unsaved_changes_asks_save_dont_save_or_cancel(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    w.d_type_at_start(&view, "// one\n");
    assert!(w.dirty(&id(&program)));

    // Cancel: the window stays, the edit too.
    assert!(!w.d_should_close(), "dirty documents block the close");
    assert!(w.vcx.has_pending_prompt());
    w.vcx.simulate_prompt_answer("Cancel");
    w.vcx.run_until_parked();
    assert_eq!(w.d_quits(), 0);
    assert!(w.dirty(&id(&program)));
    assert_eq!(
        std::fs::read_to_string(&program).unwrap(),
        super::tests::PROGRAM
    );

    // An agent never sees the question: without `save` it is told what is unsaved.
    let out = w.d_agent(cmd::EXIT, json!({})).unwrap();
    assert_eq!(out["exiting"], false);
    assert_eq!(out["dirty"], json!([id(&program)]));
    assert_eq!(w.d_quits(), 0);

    // Save: the file is written, then Eludite quits.
    assert!(!w.d_should_close());
    w.vcx.simulate_prompt_answer("Save");
    w.vcx.run_until_parked();
    assert_eq!(w.d_quits(), 1);
    assert!(
        std::fs::read_to_string(&program)
            .unwrap()
            .starts_with("// one\n")
    );
    assert!(!w.dirty(&id(&program)));

    // Don't Save: nothing is written.
    w.shell.update(&mut w.vcx, |s, _| s.docs.quitting = false);
    w.d_type_at_start(&view, "// two\n");
    assert!(!w.d_should_close());
    w.vcx.simulate_prompt_answer("Don't Save");
    w.vcx.run_until_parked();
    assert_eq!(w.d_quits(), 2);
    assert!(
        !std::fs::read_to_string(&program)
            .unwrap()
            .contains("// two")
    );

    // A clean window closes at once.
    w.shell.update(&mut w.vcx, |s, _| s.docs.quitting = false);
    w.d_ui(
        workspace::FILE_CLOSE,
        json!({"path": id(&program), "save": "discard"}),
    );
    assert!(w.d_should_close());
    assert!(!w.vcx.has_pending_prompt());
}

#[gpui::test]
fn save_all_writes_every_dirty_buffer(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let (program, a) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    let b = w.d_open(&order);
    w.d_type_at_start(&a, "// a\n");
    w.d_type_at_start(&b, "// b\n");
    let listed = w
        .d_agent(cmd::DOCUMENTS, json!({"dirty_only": true}))
        .unwrap();
    let paths: Vec<&str> = listed["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths.len(), 2, "{listed}");
    assert!(paths.contains(&id(&program).as_str()) && paths.contains(&id(&order).as_str()));
    assert_eq!(listed["documents"][0]["encoding"], "utf-8");
    assert_eq!(listed["documents"][0]["kind"], "text");

    let out = w.d_agent(cmd::SAVE_ALL, json!({})).unwrap();
    assert_eq!(out["saved"].as_array().unwrap().len(), 2, "{out}");
    assert!(
        std::fs::read_to_string(&program)
            .unwrap()
            .starts_with("// a\n")
    );
    assert!(
        std::fs::read_to_string(&order)
            .unwrap()
            .starts_with("// b\n")
    );
    assert!(!w.dirty(&id(&program)) && !w.dirty(&id(&order)));
    let listed = w
        .d_agent(cmd::DOCUMENTS, json!({"dirty_only": true}))
        .unwrap();
    assert_eq!(listed["documents"], json!([]));
}

#[gpui::test]
fn a_build_saves_dirty_documents_first_unless_the_setting_is_off(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    w.d_set_setting("build.saveBeforeBuild", json!(false));
    w.d_type_at_start(&view, "// off\n");
    w.d_ui(eludite_commands::build::SOLUTION, json!({}));
    assert!(
        !std::fs::read_to_string(&program)
            .unwrap()
            .contains("// off"),
        "the setting is off: the build reads the disk as it is"
    );
    w.d_ui(eludite_commands::build::CANCEL, json!({}));
    w.d_set_setting("build.saveBeforeBuild", json!(true));
    w.d_type_at_start(&view, "// on\n");
    w.d_ui(eludite_commands::build::SOLUTION, json!({}));
    let text = std::fs::read_to_string(&program).unwrap();
    assert!(text.starts_with("// on\n// off\n"), "{text}");
    assert!(!w.dirty(&id(&program)));
}

#[gpui::test]
fn an_outside_change_reloads_a_clean_buffer_and_asks_for_a_dirty_one(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    // The caret on `static void Main`, line 3.
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            let at = e.text().find("static").unwrap();
            e.set_caret(at);
        })
    });
    // An agent adds a using at the top.
    std::fs::write(
        &program,
        format!("using System;\n{}", super::tests::PROGRAM),
    )
    .unwrap();
    w.d_poll_disk();
    w.wait("the reload", |w| w.text(&view).starts_with("using System;"));
    assert!(!w.dirty(&id(&program)), "a reload is the file's text");
    assert_eq!(w.d_caret_line(&view), 4, "the caret stays on its text");
    let caret = w.caret(&view);
    assert!(w.text(&view)[caret..].starts_with("static"));

    // A dirty buffer asks; No keeps the edit, and the next save overwrites the outside version.
    w.d_type_at_start(&view, "// mine\n");
    std::fs::write(&program, "class Theirs { }\n").unwrap();
    w.d_poll_disk();
    assert!(w.vcx.has_pending_prompt(), "File Modification Detected");
    w.vcx.simulate_prompt_answer("No");
    w.vcx.run_until_parked();
    assert!(w.text(&view).starts_with("// mine\n"));
    w.d_poll_disk();
    assert!(
        !w.vcx.has_pending_prompt(),
        "No is remembered for that version"
    );
    w.d_ui(workspace::EDITOR_SAVE, json!({}));
    assert!(
        std::fs::read_to_string(&program)
            .unwrap()
            .starts_with("// mine\n")
    );

    // Yes reloads the outside version, dropping the edit.
    w.d_type_at_start(&view, "// again\n");
    std::fs::write(&program, "class Theirs2 { }\n").unwrap();
    w.d_poll_disk();
    w.vcx.simulate_prompt_answer("Yes");
    w.wait("the reload", |w| w.text(&view) == "class Theirs2 { }\n");
    assert!(!w.dirty(&id(&program)));

    // A save never overwrites a newer version unasked: an agent is refused.
    w.d_type_at_start(&view, "// x\n");
    std::fs::write(&program, "class Newer { int x; }\n").unwrap();
    let err = w
        .d_agent(workspace::EDITOR_SAVE, json!({"path": id(&program)}))
        .unwrap_err();
    assert!(err.contains("changed on disk"), "{err}");

    // A deleted file marks its tab.
    std::fs::remove_file(&program).unwrap();
    w.d_poll_disk();
    if w.vcx.has_pending_prompt() {
        w.vcx.simulate_prompt_answer("No");
        w.vcx.run_until_parked();
    }
    w.d_poll_disk();
    assert!(w.controller.snapshot().deleted.contains(&id(&program)));
    let docs = w.d_agent(cmd::DOCUMENTS, json!({})).unwrap();
    let row = docs["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["path"] == id(&program))
        .unwrap()
        .clone();
    assert_eq!(row["deleted"], true);
    // Saving writes it again.
    w.d_ui(workspace::EDITOR_SAVE, json!({"path": id(&program)}));
    assert!(program.is_file());
    assert!(!w.controller.snapshot().deleted.contains(&id(&program)));
}

#[gpui::test]
fn utf16_and_windows_1252_files_round_trip_through_the_editor(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let text = "// caf\u{e9}\r\nclass C { }\r\n";
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    let cp1252 = b"// caf\xE9 \x80\r\nclass D { }\r\n".to_vec();
    let a = w.path("src/App/Wide.cs");
    let b = w.path("src/App/Legacy.sql");
    std::fs::write(&a, &utf16).unwrap();
    std::fs::write(&b, &cp1252).unwrap();
    for (path, bytes, encoding, status) in [
        (&a, &utf16, "utf-16le", "UTF-16 LE"),
        (&b, &cp1252, "windows-1252", "Windows-1252"),
    ] {
        let view = w.d_open(path);
        assert!(w.text(&view).contains("caf\u{e9}"), "decoded");
        let docs = w.d_invoke_ui(cmd::DOCUMENTS, json!({})).unwrap();
        let row = docs["documents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["path"] == id(path))
            .unwrap()
            .clone();
        assert_eq!(row["encoding"], encoding);
        assert_eq!(row["line_ending"], "crlf");
        assert_eq!(
            w.shell.read_with(&w.vcx, |s, _| s
                .status
                .get(eludite_ui::slots::ENCODING)
                .map(str::to_owned)),
            Some(status.to_owned())
        );
        w.d_ui(workspace::EDITOR_SAVE, json!({"path": id(path)}));
        assert_eq!(&std::fs::read(path).unwrap(), bytes, "byte for byte");
    }
    // Advanced Save Options: UTF-8 with LF from now on; the document is dirty until saved.
    let out = w
        .d_invoke_ui(
            cmd::SET_ENCODING,
            json!({"path": id(&b), "encoding": "utf-8", "line_ending": "lf"}),
        )
        .unwrap();
    assert_eq!(out["dirty"], true);
    w.d_ui(workspace::EDITOR_SAVE, json!({"path": id(&b)}));
    assert_eq!(
        std::fs::read(&b).unwrap(),
        "// caf\u{e9} \u{20ac}\nclass D { }\n".as_bytes()
    );
    assert!(
        w.d_invoke_ui(cmd::SET_ENCODING, json!({"encoding": "nope"}))
            .is_err()
    );
}

#[gpui::test]
fn the_tab_menu_closes_pins_and_copies(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let (program, _) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    w.d_open(&order);
    let aspx = w.path("src/App/Default.aspx.cs");
    w.d_open(&aspx);

    // Pin Tab from the menu: the tab moves to the left.
    w.d_mouse_down(&format!("doc-tab-{}", id(&aspx)), MouseButton::Right);
    w.click("doc-menu-pin");
    assert_eq!(w.d_tabs()[0], id(&aspx));
    // Copy Full Path.
    w.d_mouse_down(&format!("doc-tab-{}", id(&order)), MouseButton::Right);
    w.click("doc-menu-copy-path");
    assert_eq!(
        w.shell.read_with(&w.vcx, |s, _| s.docs.copied.clone()),
        Some(id(&order))
    );
    // Open Containing Folder.
    w.d_mouse_down(&format!("doc-tab-{}", id(&order)), MouseButton::Right);
    w.click("doc-menu-open-folder");
    assert_eq!(
        w.opened.lock().unwrap().last().cloned(),
        order.parent().map(Path::to_path_buf)
    );
    // Close All But Pinned keeps the pinned tab.
    w.d_mouse_down(&format!("doc-tab-{}", id(&order)), MouseButton::Right);
    w.click("doc-menu-close-unpinned");
    assert_eq!(w.d_tabs(), [id(&aspx)]);
    // Reopen Closed Tab (Ctrl+Shift+T) brings back the last one closed.
    w.vcx.simulate_keystrokes("ctrl-shift-t");
    w.wait("the reopened tab", |w| w.d_tabs().len() == 2);
    assert!(w.d_tabs().contains(&id(&order)));
    w.d_open(&program);
    // Close All But This.
    w.d_mouse_down(&format!("doc-tab-{}", id(&program)), MouseButton::Right);
    w.click("doc-menu-close-others");
    assert_eq!(w.d_tabs(), [id(&program)]);
    // Middle click closes.
    w.d_mouse_down(&format!("doc-tab-{}", id(&program)), MouseButton::Middle);
    assert!(w.d_tabs().is_empty());
}

#[gpui::test]
fn ctrl_tab_walks_the_documents_in_most_recently_used_order(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let (program, _) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    w.d_open(&order);
    let aspx = w.path("src/App/Default.aspx.cs");
    w.d_open(&aspx);
    w.d_open(&program);
    // MRU: Program.cs, Default.aspx.cs, Order.cs.
    let out = w.d_agent(cmd::NAVIGATOR, json!({})).unwrap();
    let docs: Vec<&str> = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "document")
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(&docs[..3], [id(&program), id(&aspx), id(&order)]);
    // Ctrl+Tab, release Ctrl: the previous document.
    w.vcx.simulate_keystrokes("ctrl-tab");
    assert!(
        w.shell.read_with(&w.vcx, |s, cx| s
            .dock()
            .read(cx)
            .navigator_selection()
            .is_some()),
        "the navigator shows while Ctrl is held"
    );
    w.vcx.simulate_modifiers_change(Modifiers::none());
    w.vcx.run_until_parked();
    assert_eq!(w.d_active(), Some(id(&aspx)));
    // Ctrl+Tab twice before releasing: two back.
    w.vcx.simulate_keystrokes("ctrl-tab ctrl-tab");
    w.vcx.simulate_modifiers_change(Modifiers::none());
    w.vcx.run_until_parked();
    assert_eq!(w.d_active(), Some(id(&order)));
    // Ctrl+Alt+PgDn: the next tab in tab order, wrapping.
    let tabs = w.d_tabs();
    let at = tabs
        .iter()
        .position(|t| Some(t) == w.d_active().as_ref())
        .unwrap();
    w.vcx.simulate_keystrokes("ctrl-alt-pagedown");
    assert_eq!(w.d_active(), Some(tabs[(at + 1) % tabs.len()].clone()));
}

#[gpui::test]
fn documents_come_back_with_their_carets_after_a_restart(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let state_root = state.path().to_path_buf();
    let mut w = setup_services(
        cx,
        |_| {},
        None,
        None,
        move |s| {
            s.documents_state_root = Some(state_root);
        },
    );
    let (program, view) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    w.d_open(&order);
    w.d_open(&program);
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            let at = e.text().find("static").unwrap();
            e.set_caret(at);
        })
    });
    w.d_ui(cmd::PIN_TAB, json!({"path": id(&order)}));
    w.shell.update(&mut w.vcx, |s, cx| s.session_save_now(cx));
    let root: PathBuf = w
        .shell
        .read_with(&w.vcx, |s, _| s.workspace_root())
        .unwrap();
    let file = crate::settings::workspace_state_dir(state.path(), &root).join("documents.json");
    assert!(file.is_file(), "{}", file.display());

    // "Restart": every document closes, then the workspace's session is restored.
    w.d_ui(cmd::CLOSE_ALL, json!({"save": "discard"}));
    assert!(w.d_tabs().is_empty());
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.session_restore(&root, window, cx)
    });
    w.wait("the restored tabs", |w| w.d_tabs().len() == 2);
    assert_eq!(w.d_tabs(), [id(&order), id(&program)], "the pin is kept");
    assert_eq!(w.d_active(), Some(id(&program)));
    let view = w.editor(&program);
    w.wait("the caret", |w| w.d_caret_line(&view) == 3);
    // The other tab is read only when shown.
    assert!(
        w.shell.read_with(&w.vcx, |s, _| s.editor(&order).is_none()),
        "lazy until shown"
    );
    w.d_ui(eludite_commands::view::SHOW, json!({"id": id(&order)}));
    w.editor(&order);
}

#[gpui::test]
fn bookmarks_follow_edits_and_survive_a_restart(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let state_root = state.path().to_path_buf();
    let mut w = setup_services(
        cx,
        |_| {},
        None,
        None,
        move |s| {
            s.documents_state_root = Some(state_root);
        },
    );
    let (program, view) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    let out = w
        .d_agent(
            cmd::BOOKMARK_TOGGLE,
            json!({"path": id(&program), "line": 3}),
        )
        .unwrap();
    assert_eq!(out["added"], true);
    w.d_agent(
        cmd::BOOKMARK_TOGGLE,
        json!({"path": id(&program), "line": 1}),
    )
    .unwrap();
    w.d_open(&order);
    // Ctrl+K, Ctrl+K on Order.cs's first line.
    w.vcx.simulate_keystrokes("ctrl-k ctrl-k");
    w.vcx.run_until_parked();
    let list = w.d_agent(cmd::BOOKMARK_LIST, json!({})).unwrap();
    assert_eq!(list["bookmarks"].as_array().unwrap().len(), 3, "{list}");
    assert_eq!(
        view.read_with(&w.vcx, |v, _| v.bookmark_rows()),
        [(0, true), (2, true)],
        "drawn in the margin"
    );
    // An edit above moves them with their lines.
    w.d_type_at_start(&view, "// new\n");
    let list = w.d_agent(cmd::BOOKMARK_LIST, json!({})).unwrap();
    let lines: Vec<u64> = list["bookmarks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["path"] == id(&program))
        .map(|b| b["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, [4, 2]);
    // Next Bookmark walks the window's order across documents.
    w.d_ui(cmd::BOOKMARK_NEXT, json!({}));
    let first = w.d_active();
    w.d_ui(cmd::BOOKMARK_NEXT, json!({}));
    w.d_ui(cmd::BOOKMARK_NEXT, json!({}));
    w.d_ui(cmd::BOOKMARK_NEXT, json!({}));
    assert_eq!(w.d_active(), first, "wraps after three");
    // Disabled bookmarks are skipped; rename.
    let bid = list["bookmarks"][0]["id"].as_u64().unwrap();
    let row = w
        .d_agent(
            cmd::BOOKMARK_UPDATE,
            json!({"id": bid, "name": "Main", "enabled": false}),
        )
        .unwrap();
    assert_eq!(row["name"], "Main");
    assert_eq!(row["enabled"], false);
    // Restart: write, forget, read back.
    w.shell.update(&mut w.vcx, |s, cx| s.session_save_now(cx));
    let root: PathBuf = w
        .shell
        .read_with(&w.vcx, |s, _| s.workspace_root())
        .unwrap();
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.bookmarks_load(&root, window, cx)
    });
    w.wait("the bookmarks read back", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.bookmarks.list.len()) == 3
    });
    let list = w.d_agent(cmd::BOOKMARK_LIST, json!({})).unwrap();
    let main = list["bookmarks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["name"] == "Main")
        .unwrap()
        .clone();
    assert_eq!(main["line"], 4);
    assert_eq!(main["enabled"], false);
    // Clear Bookmarks.
    let out = w.d_agent(cmd::BOOKMARK_CLEAR, json!({})).unwrap();
    assert_eq!(out["removed"], 3);
    assert!(view.read_with(&w.vcx, |v, _| v.bookmark_rows()).is_empty());
}

#[gpui::test]
fn a_new_file_is_saved_through_save_as(cx: &mut TestAppContext) {
    let mut w = setup(cx);
    let out = w.d_agent(cmd::NEW, json!({"language": "csharp"})).unwrap();
    let untitled = out["path"].as_str().unwrap().to_owned();
    assert!(
        untitled.starts_with("untitled:") && untitled.ends_with(".cs"),
        "{untitled}"
    );
    assert_eq!(w.d_active(), Some(untitled.clone()));
    let view = w
        .shell
        .read_with(&w.vcx, |s, _| {
            s.documents.get(&untitled).map(|d| d.view.clone())
        })
        .unwrap();
    w.d_type_at_start(&view, "class New { }\n");
    // Ctrl+S on it is Save As: the dialog.
    w.d_ui(workspace::EDITOR_SAVE, json!({}));
    assert!(w.vcx.did_prompt_for_new_path());
    let target = w.path("src/App/New.cs");
    w.vcx.simulate_new_path_selection(|_| Some(target.clone()));
    w.vcx.run_until_parked();
    w.wait("the saved file", |_| target.is_file());
    let bytes = std::fs::read(&target).unwrap();
    assert!(
        bytes.starts_with(b"\xEF\xBB\xBFclass New"),
        "UTF-8 with a signature"
    );
    assert_eq!(
        w.d_active(),
        Some(id(&target)),
        "the tab now names the file"
    );
    assert!(!w.dirty(&id(&target)));
    // An agent's Save As refuses to replace a file unless told.
    let program = w.path("src/App/Program.cs");
    let err = w
        .d_agent(
            cmd::SAVE_AS,
            json!({"path": id(&target), "new_path": id(&program)}),
        )
        .unwrap_err();
    assert!(err.contains("exists"), "{err}");
    let copy = w.path("src/App/Copy.cs");
    let out = w
        .d_agent(
            cmd::SAVE_AS,
            json!({"path": id(&target), "new_path": id(&copy)}),
        )
        .unwrap();
    assert!(out["new_path"].as_str().unwrap().ends_with("Copy.cs"));
    assert!(copy.is_file());
    // The API other briefs call (the Workspace window's rename and delete).
    w.shell.update_in(&mut w.vcx, |s, window, cx| {
        assert!(s.open_document_paths().contains(&copy));
        let moved = copy.with_file_name("Moved.cs");
        assert!(s.rename_open_document(&copy, &moved, window, cx));
        assert!(s.open_document_paths().contains(&moved));
        assert!(s.close_document(&moved, true, cx));
        assert!(!s.open_document_paths().contains(&moved));
    });
}
