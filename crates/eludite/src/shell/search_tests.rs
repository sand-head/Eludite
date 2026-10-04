//! Headless tests of Find in Files and Replace in Files (brief 0042) on the test solution, with the real search
//! engine on the files on disk and the open documents' buffers: Ctrl+Shift+F opens the dialog with the editor's
//! selection, Enter searches the solution and Find Results 1 groups the matches by file under Visual Studio's count
//! line, F8 and Shift+F8 open the editor at a match with it selected, Append keeps the previous block, Find Results 2,
//! Stop during a slow search, an unsaved edit is found, a superseded search never draws, Replace All through the
//! review view (Accept All as one undo step per document, Reject, a file changed on disk since the search skipped
//! with a row), an agent's `find` and `replace`, `preview: false`, Replace Next and Skip File, the history, the
//! View menu's windows and the 10,000-match frame budget.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::search as cmds;
use eludite_commands::{Caller, with_caller, workspace};
use eludite_docking::ids;
use eludite_editor::EditorView;
use gpui::{Entity, TestAppContext};
use serde_json::{Value, json};

use super::search::dialog::{self, FindDialog, Mode};
use super::search::results::{self, FindResults, Row};
use super::search::{HISTORY, SearchService};
use super::tests::{Ws, setup};

fn agent_caller() -> Caller {
    Caller::Agent {
        agent: "Claude".into(),
        call: eludite_commands::next_call_id(),
        tool_call: None,
    }
}

struct Sw {
    w: Ws,
    _state: tempfile::TempDir,
}

/// The test solution, opened, with the dialog's history kept in a temporary folder.
fn setup_search(cx: &mut TestAppContext) -> Sw {
    let mut w = setup(cx);
    w.open_solution();
    let state = tempfile::tempdir().unwrap();
    let service = w
        .shell
        .read_with(&w.vcx, |s, _| s.search_ui().service.clone());
    service.set_state_dir(Some(state.path().to_path_buf()));
    Sw { w, _state: state }
}

impl Sw {
    fn service(&self) -> Arc<SearchService> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.search_ui().service.clone())
    }

    fn window(&self, n: u8) -> Entity<FindResults> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.search_ui().window(n).clone())
    }

    fn dialog(&self) -> Option<Entity<FindDialog>> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.search_ui().dialog.clone())
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.w.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    /// Invoke `command` from another thread as `caller`, running the UI while it waits.
    fn call(&mut self, caller: Caller, command: &str, args: Value) -> Result<Value, String> {
        let commands = self.w.commands.clone();
        let command = command.to_owned();
        let t = std::thread::spawn(move || {
            with_caller(caller, || commands.invoke(&command, args)).map_err(|e| e.to_string())
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        while !t.is_finished() {
            assert!(Instant::now() < deadline, "the call did not return");
            self.w.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        let out = t.join().unwrap();
        self.w.vcx.run_until_parked();
        out
    }

    fn agent(&mut self, command: &str, args: Value) -> Result<Value, String> {
        self.call(agent_caller(), command, args)
    }

    /// Run a command as the person through the shell (as a menu item or a button does).
    fn run(&mut self, command: &str, args: Value) {
        let command = command.to_owned();
        self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.run(&command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
    }

    /// Wait until window `n` shows a finished search.
    fn wait_done(&mut self, n: u8) {
        let win = self.window(n);
        self.w.wait("the search to finish", |w| {
            win.read_with(&w.vcx, |r, _| {
                !r.blocks.is_empty() && r.blocks.iter().all(|b| !b.running)
            })
        });
    }

    /// Run a search for window `n` as the person and wait until it is drawn and finished.
    fn search(&mut self, n: u8, args: Value) {
        let before: Vec<u64> = self.window(n).read_with(&self.w.vcx, |r, _| {
            r.blocks.iter().map(|b| b.search_id).collect()
        });
        self.run(cmds::FIND, args);
        let win = self.window(n);
        self.w.wait("the new search to finish", |w| {
            win.read_with(&w.vcx, |r, _| {
                r.blocks.iter().any(|b| !before.contains(&b.search_id))
                    && r.blocks.iter().all(|b| !b.running)
            })
        });
    }

    fn summary(&self, n: u8, block: usize) -> String {
        self.window(n)
            .read_with(&self.w.vcx, |r, _| r.blocks[block].summary())
    }

    /// The files of window `n`'s blocks, as displayed.
    fn files(&self, n: u8) -> Vec<Vec<String>> {
        self.window(n).read_with(&self.w.vcx, |r, _| {
            r.blocks
                .iter()
                .map(|b| b.files.iter().map(|f| f.display.clone()).collect())
                .collect()
        })
    }

    fn type_text(&mut self, text: &str) {
        let keys: Vec<String> = text
            .chars()
            .map(|c| match c {
                ' ' => "space".to_owned(),
                '.' => "..".to_owned(),
                c => c.to_string(),
            })
            .collect();
        for k in keys {
            if k == ".." {
                self.w.vcx.simulate_input(".");
            } else {
                self.w.vcx.simulate_keystrokes(&k);
            }
        }
        self.w.vcx.run_until_parked();
    }

    fn editor_text(&self, view: &Entity<EditorView>) -> String {
        self.w.text(view)
    }

    fn selection_text(&self, view: &Entity<EditorView>) -> String {
        view.read_with(&self.w.vcx, |v, _| {
            let e = v.editor();
            e.buffer().text_for_range(e.primary_selection().range())
        })
    }

    fn pending_changes(&self) -> Vec<u64> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.changes_for(None, None))
    }
}

#[gpui::test]
fn ctrl_shift_f_opens_the_dialog_with_the_selection_and_enter_searches_the_solution(
    cx: &mut TestAppContext,
) {
    let mut s = setup_search(cx);
    s.write(
        "src/App/Models/Customer.cs",
        "class Customer\n{\n    Order First;\n    Order Last;\n}\n",
    );
    let (program, view) = s.w.open_program();
    // Select `Main` in the editor: Ctrl+Shift+F preloads it.
    view.update(&mut s.w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            let at = e.text().find("Main").unwrap();
            e.set_selections(
                vec![eludite_editor::SelectionRange {
                    tail: at,
                    head: at + 4,
                }],
                0,
            );
        })
    });
    s.w.vcx.simulate_keystrokes("ctrl-shift-f");
    s.w.vcx.run_until_parked();
    let d = s.dialog().expect("the dialog opens");
    let (mode, query) = d.read_with(&s.w.vcx, |d, _| (d.mode, d.query.clone()));
    assert_eq!((mode, query.as_str()), (Mode::Find, "Main"));
    assert!(s.w.vcx.debug_bounds(dialog::DIALOG).is_some(), "drawn");
    // Typing replaces the selected query; Enter is Find All over the Entire Solution.
    s.type_text("Order");
    assert_eq!(d.read_with(&s.w.vcx, |d, _| d.query.clone()), "Order");
    s.w.vcx.simulate_keystrokes("enter");
    s.wait_done(1);
    // Find Results 1 is shown, grouped by file in path order, under the count line.
    assert!(
        s.w.vcx.debug_bounds("find-results-1").is_some(),
        "Find Results 1 is shown"
    );
    assert_eq!(
        s.files(1),
        [["src/App/Models/Customer.cs", "src/App/Models/Order.cs"]]
    );
    let summary = s.summary(1, 0);
    assert_eq!(
        summary,
        format!(
            "Find all \"Order\", Subfolders, Find Results 1, Entire Solution, \"*.*\" \u{2014} Matching lines: 3 \
             Matching files: 2 Total files searched: {}",
            s.window(1).read_with(&s.w.vcx, |r, _| r.blocks[0]
                .counts
                .as_ref()
                .unwrap()
                .files_searched)
        )
    );
    let rows = s.window(1).read_with(&s.w.vcx, |r, _| r.rows().to_vec());
    assert!(matches!(rows[0], Row::Header(0)));
    assert!(matches!(rows[1], Row::File(0, 0)));
    assert!(matches!(rows[2], Row::Line(0, 0, 0, 0)));
    assert!(matches!(rows.last(), Some(Row::Footer(0))));
    // The rows are drawn, the match highlighted.
    assert!(
        s.w.vcx
            .debug_bounds(results::row_selector(1, 2).leak())
            .is_some()
    );
    // The history remembers the query.
    assert_eq!(
        s.w.shell.read_with(&s.w.vcx, |sh, _| sh.search_history()),
        ["Order"]
    );
    // F8 opens the editor at the first match with the match selected; F8 again the next; Shift+F8 back.
    s.w.vcx.simulate_keystrokes("escape");
    s.w.vcx.run_until_parked();
    assert!(s.dialog().is_none(), "Escape closes the dialog");
    s.w.vcx.simulate_keystrokes("f8");
    s.w.vcx.run_until_parked();
    let customer = s.w.path("src/App/Models/Customer.cs");
    let cview = s.w.editor(&customer);
    let customer_id = super::documents::normalize_path(&customer)
        .to_string_lossy()
        .into_owned();
    s.w.wait("the selection", |w| {
        w.controller.active_document().as_deref() == Some(customer_id.as_str())
    });
    let sel = cview.read_with(&s.w.vcx, |v, _| {
        let e = v.editor();
        (
            e.buffer().text_for_range(e.primary_selection().range()),
            e.buffer()
                .offset_to_point(e.primary_selection().start())
                .row,
        )
    });
    assert_eq!(sel, ("Order".to_owned(), 2));
    s.w.vcx.simulate_keystrokes("f8");
    s.w.vcx.run_until_parked();
    let row = cview.read_with(&s.w.vcx, |v, _| {
        let e = v.editor();
        e.buffer()
            .offset_to_point(e.primary_selection().start())
            .row
    });
    assert_eq!(row, 3, "the second match");
    assert_eq!(
        s.window(1).read_with(&s.w.vcx, |r, _| r.selected()),
        Some(1)
    );
    s.w.vcx.simulate_keystrokes("shift-f8");
    s.w.vcx.run_until_parked();
    assert_eq!(
        s.window(1).read_with(&s.w.vcx, |r, _| r.selected()),
        Some(0)
    );
    // A double-click on a result opens it there (the window's rows are a click away).
    s.w.vcx.simulate_keystrokes("shift-f8");
    s.w.vcx.run_until_parked();
    assert_eq!(
        s.window(1).read_with(&s.w.vcx, |r, _| r.selected()),
        Some(2),
        "wraps around"
    );
    let rows = s.window(1).read_with(&s.w.vcx, |r, _| r.rows().to_vec());
    let second = rows
        .iter()
        .position(|r| matches!(r, Row::Line(0, 0, 1, 1)))
        .unwrap();
    s.w.double_click(&results::row_selector(1, second));
    assert_eq!(
        s.window(1).read_with(&s.w.vcx, |r, _| r.selected()),
        Some(1)
    );
    let row = cview.read_with(&s.w.vcx, |v, _| {
        let e = v.editor();
        e.buffer()
            .offset_to_point(e.primary_selection().start())
            .row
    });
    assert_eq!(row, 3);
    assert_eq!(s.selection_text(&cview), "Order");
    let _ = program;
    // The audit records the person's commands.
    let audit = s.w.audit();
    assert!(audit.contains(&cmds::FIND.to_owned()));
    assert!(audit.contains(&cmds::RESULTS.to_owned()));
}

#[gpui::test]
fn append_keeps_the_previous_block_and_find_results_2_is_its_own(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    s.search(1, json!({"query": "class Program", "results_window": 1}));
    s.search(
        1,
        json!({"query": "class Order", "results_window": 1, "append": true}),
    );
    let blocks = s.window(1).read_with(&s.w.vcx, |r, _| r.blocks.len());
    assert_eq!(blocks, 2, "Append keeps the previous block");
    let rows = s.window(1).read_with(&s.w.vcx, |r, _| r.rows().to_vec());
    assert!(rows.contains(&Row::Separator));
    assert_eq!(
        s.files(1),
        [vec!["src/App/Program.cs"], vec!["src/App/Models/Order.cs"]]
    );
    // Without Append the window starts over.
    s.search(1, json!({"query": "partial", "results_window": 1}));
    assert_eq!(s.files(1), [vec!["src/App/Default.aspx.cs"]]);
    // Find Results 2 is its own window, shown by the search, and F8 follows the last search.
    s.search(2, json!({"query": "Page", "results_window": 2}));
    assert_eq!(s.files(2), [vec!["src/App/Default.aspx"]]);
    assert_eq!(s.files(1), [vec!["src/App/Default.aspx.cs"]]);
    assert_eq!(
        s.w.shell.read_with(&s.w.vcx, |sh, _| sh.search_ui().active),
        2
    );
    assert!(s.summary(2, 0).contains("Find Results 2"));
    s.w.vcx.simulate_keystrokes("f8");
    s.w.vcx.run_until_parked();
    let aspx = s.w.path("src/App/Default.aspx");
    s.w.editor(&aspx);
    // Clear empties a window.
    s.run(cmds::RESULTS, json!({"results_window": 2, "clear": true}));
    assert!(s.window(2).read_with(&s.w.vcx, |r, _| r.blocks.is_empty()));
    // The View menu's items show the windows.
    s.run("eludite.view.show", json!({"id": ids::FIND_RESULTS_2}));
    assert!(s.w.vcx.debug_bounds("find-results-2").is_some());
}

#[gpui::test]
fn stop_during_a_slow_search_cancels_and_the_window_says_so(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    for i in 0..300 {
        s.write(&format!("slow/f{i:03}.txt"), "needle\n");
    }
    // Every file's overlay lookup sleeps: a slow search.
    s.service()
        .set_slow_overlay(Some(Duration::from_millis(20)));
    s.run(cmds::FIND, json!({"query": "needle", "results_window": 1}));
    let win = s.window(1);
    s.w.wait("some results", |w| {
        win.read_with(&w.vcx, |r, _| {
            r.blocks
                .first()
                .is_some_and(|b| b.running && !b.files.is_empty())
        })
    });
    assert!(s.service().running_in(1).is_some());
    let stop = Instant::now();
    s.w.click(&results::stop_selector(1));
    s.wait_done(1);
    let took = stop.elapsed();
    let (canceled, found, footer) = win.read_with(&s.w.vcx, |r, _| {
        let b = &r.blocks[0];
        (
            b.counts.as_ref().unwrap().canceled,
            b.files.len(),
            b.footer().unwrap(),
        )
    });
    assert!(canceled);
    assert!(found < 300, "stopped before the end: {found}");
    assert!(footer.contains("stopped"), "{footer}");
    eprintln!(
        "timing: Stop to the search's end {:.1} ms (20 ms per file, 4 threads)",
        took.as_secs_f64() * 1e3
    );
    super::git_tests::assert_budget("Stop to the search's end", took, Duration::from_millis(50));
    assert!(s.service().running_in(1).is_none());
}

#[gpui::test]
fn an_unsaved_edit_in_an_open_document_is_found_once(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    let (program, view) = s.w.open_program();
    view.update(&mut s.w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(0);
            e.insert("// Unsaved marker\n")
        })
    });
    s.w.vcx.run_until_parked();
    let out = s
        .agent(cmds::FIND, json!({"query": "unsaved marker"}))
        .unwrap();
    assert_eq!(out["total"], 1);
    assert_eq!(out["files"][0]["path"], "src/App/Program.cs");
    assert_eq!(out["files"][0]["open"], true);
    assert_eq!(out["files"][0]["matches"][0]["line"], 1);
    // The disk does not have it; the unsaved text counts once.
    assert!(
        !std::fs::read_to_string(&program)
            .unwrap()
            .contains("Unsaved")
    );
    let out = s
        .agent(cmds::FIND, json!({"query": "class Program"}))
        .unwrap();
    assert_eq!(out["total"], 1);
    // An agent's search without `results_window` is answered only.
    assert!(s.window(1).read_with(&s.w.vcx, |r, _| r.blocks.is_empty()));
    // Current Document and All Open Documents search the buffer.
    let out = s
        .agent(
            cmds::FIND,
            json!({"query": "marker", "scope": "document", "path": "src/App/Program.cs"}),
        )
        .unwrap();
    assert_eq!(out["total"], 1);
    let out = s
        .agent(
            cmds::FIND,
            json!({"query": "marker", "scope": "open_documents"}),
        )
        .unwrap();
    assert_eq!(out["files"].as_array().unwrap().len(), 1);
    // The project scope by name, and its folder.
    let out = s
        .agent(
            cmds::FIND,
            json!({"query": "Order", "scope": "project", "project": "App"}),
        )
        .unwrap();
    assert_eq!(out["files"][0]["path"], "src/App/Models/Order.cs");
    assert!(
        s.agent(
            cmds::FIND,
            json!({"query": "x", "scope": "project", "project": "Nope"})
        )
        .unwrap_err()
        .contains("no project `Nope`")
    );
    let out = s
        .agent(
            cmds::FIND,
            json!({"query": "class", "scope": "folder", "path": "src/App/Models"}),
        )
        .unwrap();
    assert_eq!(out["files"].as_array().unwrap().len(), 1);
    // A bad regular expression is refused before anything is searched.
    assert!(
        s.agent(cmds::FIND, json!({"query": "(", "regex": true}))
            .unwrap_err()
            .contains("not valid")
    );
    // The answer follows the schema and pages through eludite.search.results by id.
    let out = s
        .agent(cmds::FIND, json!({"query": "class", "context_lines": 1}))
        .unwrap();
    let id = out["search_id"].as_u64().unwrap();
    let page = s
        .agent(cmds::RESULTS, json!({"search_id": id, "limit": 2}))
        .unwrap();
    assert_eq!(page["matches"].as_array().unwrap().len(), 2);
    assert_eq!(page["next_offset"], 2);
    assert_eq!(page["total"], out["matching_lines"]);
}

#[gpui::test]
fn a_superseded_search_never_draws(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    for i in 0..60 {
        s.write(&format!("slow/a{i:02}.txt"), "alpha\n");
    }
    s.write("slow/beta.txt", "beta\n");
    s.service()
        .set_slow_overlay(Some(Duration::from_millis(15)));
    s.run(cmds::FIND, json!({"query": "alpha", "results_window": 1}));
    let win = s.window(1);
    s.w.wait("the first search's results", |w| {
        win.read_with(&w.vcx, |r, _| {
            r.blocks.first().is_some_and(|b| !b.files.is_empty())
        })
    });
    let first = win.read_with(&s.w.vcx, |r, _| r.blocks[0].search_id);
    s.service().set_slow_overlay(None);
    s.search(1, json!({"query": "beta", "results_window": 1}));
    // Let the first search's last files arrive: they are dropped.
    std::thread::sleep(Duration::from_millis(100));
    s.w.vcx.run_until_parked();
    let (ids, files) = win.read_with(&s.w.vcx, |r, _| {
        (
            r.blocks.iter().map(|b| b.search_id).collect::<Vec<_>>(),
            r.blocks
                .iter()
                .flat_map(|b| b.files.iter().map(|f| f.display.clone()))
                .collect::<Vec<_>>(),
        )
    });
    assert!(!ids.contains(&first), "the superseded block is gone");
    assert_eq!(files, ["slow/beta.txt"]);
    // A late file of the first search is refused.
    let late = s.w.path("slow/a00.txt");
    let refused = win.update(&mut s.w.vcx, |r, cx| {
        r.add_files(
            first,
            vec![eludite_search::FileMatches {
                path: late,
                open: false,
                lines: vec![],
                fingerprint: 0,
                encoding: Default::default(),
            }],
            cx,
        )
    });
    assert!(!refused);
}

/// Program.cs open, Order.cs open, Customer.cs closed: Replace `Order` with `Purchase`.
fn replace_fixture(s: &mut Sw) -> (Entity<EditorView>, Entity<EditorView>, PathBuf) {
    let customer = s.write(
        "src/App/Models/Customer.cs",
        "class Customer\n{\n    Order First; Order Second;\n}\n",
    );
    let (_, program) = s.w.open_program();
    program.update(&mut s.w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(0);
            e.insert("// Order, Order\n")
        })
    });
    let order = s.w.path("src/App/Models/Order.cs");
    s.run(
        workspace::FILE_OPEN,
        json!({"path": order.to_string_lossy()}),
    );
    let order = s.w.editor(&order);
    (program, order, customer)
}

#[gpui::test]
fn replace_all_with_preview_holds_changes_and_accept_all_is_one_undo_step_per_document(
    cx: &mut TestAppContext,
) {
    let mut s = setup_search(cx);
    let (program, order, customer) = replace_fixture(&mut s);
    let before_program = s.editor_text(&program);
    // The dialog's Replace All: Ctrl+Shift+H, the query, Tab, the replacement, the button.
    s.w.vcx.simulate_keystrokes("ctrl-shift-h");
    s.w.vcx.run_until_parked();
    let d = s.dialog().unwrap();
    assert_eq!(d.read_with(&s.w.vcx, |d, _| d.mode), Mode::Replace);
    s.w.vcx.simulate_keystrokes("ctrl-a");
    s.type_text("Order");
    s.w.vcx.simulate_keystrokes("tab");
    s.type_text("Purchase");
    d.update(&mut s.w.vcx, |d, cx| {
        d.match_case = true;
        d.whole_word = true;
        cx.notify();
    });
    s.w.vcx.run_until_parked();
    s.w.click(dialog::REPLACE_ALL);
    s.w.wait("the pending changes", |w| {
        w.shell
            .read_with(&w.vcx, |sh, _| sh.changes_for(None, None).len() == 3)
    });
    // Nothing is applied yet; the review view opened; the window lists the replaced lines.
    assert_eq!(s.editor_text(&program), before_program);
    assert!(
        std::fs::read_to_string(&customer)
            .unwrap()
            .contains("Order First")
    );
    s.wait_done(1);
    assert!(s.summary(1, 0).starts_with(
        "Replace all \"Order\", \"Purchase\", Match case, Whole word, Subfolders, Keep modified files open"
    ));
    let tabs: Vec<String> =
        s.w.controller
            .snapshot()
            .layout
            .documents
            .tabs
            .iter()
            .map(|t| t.id.clone())
            .collect();
    assert!(
        tabs.iter().any(|d| d.starts_with("agent-change:")),
        "a review view is open: {tabs:?}"
    );
    // Accept All (the Agents window's button runs eludite.agents.review with `all`).
    s.run(
        eludite_commands::agents::REVIEW,
        json!({"all": true, "decision": "accept"}),
    );
    let customer_doc = customer.clone();
    s.w.wait("every change applied", |w| {
        w.shell
            .read_with(&w.vcx, |sh, _| sh.changes_for(None, None).is_empty())
            && w.shell
                .read_with(&w.vcx, |sh, _| sh.editor(&customer_doc).is_some())
            && w.text(&program).contains("Purchase, Purchase")
    });
    assert_eq!(
        s.editor_text(&order),
        "class Purchase { }\n",
        "Order.cs's buffer"
    );
    // Keep modified files open: the closed file opened, edited in its buffer, not saved.
    let cview = s.w.editor(&customer);
    s.w.wait("Customer.cs edited", |w| {
        w.text(&cview).contains("Purchase First")
    });
    assert!(
        std::fs::read_to_string(&customer)
            .unwrap()
            .contains("Order First"),
        "not saved"
    );
    assert!(s.w.dirty(&super::documents::normalize_path(&customer).to_string_lossy()));
    // One undo step per document: Ctrl+Z takes Program.cs's two replacements back at once.
    s.run(
        workspace::EDITOR_UNDO,
        json!({"path": s.w.path("src/App/Program.cs").to_string_lossy()}),
    );
    assert_eq!(s.editor_text(&program), before_program);
    s.run(
        workspace::EDITOR_UNDO,
        json!({"path": customer.to_string_lossy()}),
    );
    assert!(s.editor_text(&cview).contains("Order First; Order Second"));
}

#[gpui::test]
fn reject_leaves_a_file_untouched_and_a_file_changed_on_disk_is_skipped_with_a_row(
    cx: &mut TestAppContext,
) {
    let mut s = setup_search(cx);
    let a = s.write("src/App/A.cs", "// token one\n");
    let b = s.write("src/App/B.cs", "// token two\n");
    s.run(
        cmds::REPLACE,
        json!({"query": "token", "replacement": "word", "keep_open": false, "results_window": 1}),
    );
    s.w.wait("two pending changes", |w| {
        w.shell
            .read_with(&w.vcx, |sh, _| sh.changes_for(None, None).len() == 2)
    });
    let ids = s.pending_changes();
    let id_of = |s: &Sw, p: &Path| {
        let p = super::documents::normalize_path(p);
        s.w.shell.read_with(&s.w.vcx, |sh, _| {
            sh.changes_for(None, Some(&p.to_string_lossy()))
        })[0]
    };
    let (ia, ib) = (id_of(&s, &a), id_of(&s, &b));
    assert_eq!(ids.len(), 2);
    // Reject A: untouched.
    s.run(
        eludite_commands::agents::REVIEW,
        json!({"change": ia, "decision": "reject"}),
    );
    // B changes on disk after the search; accepting it refuses, with a row in the results.
    std::fs::write(&b, "// token two, edited elsewhere\n").unwrap();
    s.run(
        eludite_commands::agents::REVIEW,
        json!({"change": ib, "decision": "accept"}),
    );
    let win = s.window(1);
    s.w.wait("the skipped row", |w| {
        win.read_with(&w.vcx, |r, _| {
            r.blocks.iter().any(|b| !b.skipped.is_empty())
        })
    });
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "// token one\n");
    assert_eq!(
        std::fs::read_to_string(&b).unwrap(),
        "// token two, edited elsewhere\n",
        "never overwritten blindly"
    );
    let skipped = win.read_with(&s.w.vcx, |r, _| r.blocks[0].skipped.clone());
    assert_eq!(skipped[0].0, "src/App/B.cs");
    assert!(skipped[0].1.contains("changed on disk"), "{skipped:?}");
    let rows = win.read_with(&s.w.vcx, |r, _| r.rows().to_vec());
    assert!(rows.iter().any(|r| matches!(r, Row::Skipped(0, 0))));
    let state = s.w.shell.read_with(&s.w.vcx, |sh, _| {
        sh.agents
            .changes
            .get(&ib)
            .map(|c| c.state.label().to_owned())
    });
    assert_eq!(state.as_deref(), Some("failed"));
    // Unchanged and accepted with Keep modified files open off: written directly.
    s.run(
        cmds::REPLACE,
        json!({"query": "token", "replacement": "word", "keep_open": false}),
    );
    s.w.wait("the new changes", |w| {
        w.shell
            .read_with(&w.vcx, |sh, _| sh.changes_for(None, None).len() == 2)
    });
    s.run(
        eludite_commands::agents::REVIEW,
        json!({"all": true, "decision": "accept"}),
    );
    s.w.wait("written", |_| {
        std::fs::read_to_string(&a).unwrap() == "// word one\n"
    });
    assert!(
        s.w.shell
            .read_with(&s.w.vcx, |sh, _| sh.editor(&a).is_none()),
        "not opened"
    );
}

#[gpui::test]
fn an_agents_replace_with_preview_answers_pending_changes_and_review_applies_them(
    cx: &mut TestAppContext,
) {
    let mut s = setup_search(cx);
    let c = s.write(
        "src/App/C.cs",
        "var total = items.Count();\nvar n = xs.Count();\n",
    );
    let out = s
        .agent(
            cmds::REPLACE,
            json!({"query": "(\\w+)\\.Count\\(\\)", "regex": true, "replacement": "$1.Length", "keep_open": false}),
        )
        .unwrap();
    assert_eq!(out["state"], "pending", "{out}");
    assert_eq!(out["replacements"], 2);
    assert_eq!(out["files"][0]["path"], "src/App/C.cs");
    let change = out["files"][0]["change"].as_u64().unwrap();
    assert!(
        out["message"]
            .as_str()
            .unwrap()
            .contains("eludite.agents.review")
    );
    // Nothing written until it is accepted.
    assert!(std::fs::read_to_string(&c).unwrap().contains("Count()"));
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../protocol/schemas/search-replace.output.json"
    ))
    .unwrap();
    for k in out.as_object().unwrap().keys() {
        assert!(schema["properties"].get(k).is_some(), "{k}");
    }
    let review = s
        .agent(
            eludite_commands::agents::REVIEW,
            json!({"change": change, "decision": "accept"}),
        )
        .unwrap();
    assert_eq!(review["changes"][0]["id"], change);
    s.w.wait("written", |_| {
        std::fs::read_to_string(&c).unwrap() == "var total = items.Length;\nvar n = xs.Length;\n"
    });
    // preview: false applies at once: the open document as one undo step, the closed file written.
    let (_, program) = s.w.open_program();
    let d = s.write("src/App/D.cs", "Main Main\n");
    let out = s
        .agent(
            cmds::REPLACE,
            json!({"query": "Main", "replacement": "Start", "case_sensitive": true, "preview": false}),
        )
        .unwrap();
    assert_eq!(out["state"], "applied", "{out}");
    assert!(s.editor_text(&program).contains("static void Start()"));
    assert_eq!(std::fs::read_to_string(&d).unwrap(), "Start Start\n");
    assert!(s.pending_changes().is_empty());
    // The class: preview is held for review (edit); preview: false is execute.
    let class = |s: &Sw, v: Value| s.w.commands.classify(cmds::REPLACE, &v).unwrap().class;
    assert_eq!(
        class(&s, json!({"query": "a", "replacement": "b"})),
        eludite_commands::PermissionClass::EditBuffer
    );
    assert_eq!(
        class(
            &s,
            json!({"query": "a", "replacement": "b", "preview": false})
        ),
        eludite_commands::PermissionClass::Execute
    );
    // An agent cannot open the dialog.
    assert!(
        s.agent(cmds::FIND, json!({}))
            .unwrap_err()
            .contains("query")
    );
}

#[gpui::test]
fn replace_next_and_skip_file_step_through_the_results(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    s.write("src/App/E.cs", "x = old;\ny = old;\n");
    s.write("src/App/F.cs", "z = old;\n");
    s.w.vcx.simulate_keystrokes("ctrl-shift-h");
    s.w.vcx.run_until_parked();
    s.type_text("old");
    s.w.vcx.simulate_keystrokes("tab");
    s.type_text("new");
    // Replace Next without results finds first; then it selects the first match; then replaces it and moves on.
    s.w.click(dialog::REPLACE_NEXT);
    s.wait_done(1);
    s.w.click(dialog::REPLACE_NEXT);
    let e = s.w.path("src/App/E.cs");
    let view = s.w.editor(&e);
    s.w.wait("the first match selected", |w| {
        view.read_with(&w.vcx, |v, _| !v.editor().primary_selection().is_empty())
    });
    assert_eq!(s.selection_text(&view), "old");
    s.w.click(dialog::REPLACE_NEXT);
    s.w.wait("replaced", |w| w.text(&view).starts_with("x = new;"));
    assert_eq!(s.editor_text(&view), "x = new;\ny = old;\n");
    assert_eq!(s.selection_text(&view), "old", "the next match is selected");
    // Skip File: the next file's first match.
    s.w.click(dialog::SKIP_FILE);
    let f = s.w.path("src/App/F.cs");
    let fview = s.w.editor(&f);
    s.w.wait("F.cs selected", |w| {
        fview.read_with(&w.vcx, |v, _| !v.editor().primary_selection().is_empty())
    });
    assert_eq!(s.selection_text(&fview), "old");
    assert_eq!(
        s.editor_text(&view),
        "x = new;\ny = old;\n",
        "skipped, not replaced"
    );
}

#[gpui::test]
fn the_history_keeps_twenty_queries_per_workspace(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    s.w.vcx.simulate_keystrokes("ctrl-shift-f");
    s.w.vcx.run_until_parked();
    for i in 0..(HISTORY + 3) {
        let d = s.dialog().unwrap();
        d.update(&mut s.w.vcx, |d, cx| {
            d.query = format!("q{i}");
            cx.notify();
        });
        s.w.click(dialog::QUERY_BOX);
        s.w.vcx.simulate_keystrokes("enter");
        s.wait_done(1);
    }
    let h = s.w.shell.read_with(&s.w.vcx, |sh, _| sh.search_history());
    assert_eq!(h.len(), HISTORY);
    assert_eq!(h[0], format!("q{}", HISTORY + 2), "the newest first");
    // The dialog lists them; choosing one fills Find what.
    s.w.click(dialog::HISTORY);
    s.w.click(&dialog::history_selector(1));
    assert_eq!(
        s.dialog()
            .unwrap()
            .read_with(&s.w.vcx, |d, _| d.query.clone()),
        format!("q{}", HISTORY + 1)
    );
    // Written off the UI thread.
    let dir = s.service().state_dir().unwrap();
    s.w.wait("the history file", |_| dir.join("history.json").exists());
    // The Look in list has the solution's project.
    s.w.click(dialog::LOOK_IN);
    let labels = s.dialog().unwrap().read_with(&s.w.vcx, |d, _| {
        d.look_in_choices()
            .iter()
            .map(|c| c.label())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        labels,
        [
            "Entire Solution",
            "Current Project",
            "Current Document",
            "All Open Documents",
            "Project: App",
            "Folder..."
        ]
    );
    // The regex help names the differences.
    s.w.click(dialog::REGEX_HELP);
    assert!(dialog::REGEX_NOTES.contains("lookahead"));
}

#[gpui::test]
fn ten_thousand_matches_draw_in_a_frame(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    for f in 0..100 {
        let text: String = (0..100)
            .map(|i| format!("    let hit_{i} = needle({i});\n"))
            .collect();
        s.write(&format!("many/f{f:03}.rs"), &text);
    }
    s.run(
        cmds::FIND,
        json!({"query": "needle", "results_window": 1, "max_results": 10000}),
    );
    s.wait_done(1);
    assert_eq!(
        s.window(1).read_with(&s.w.vcx, |r, _| r.match_count()),
        10_000
    );
    let mut frames = Vec::new();
    for i in 0..40 {
        if i % 10 == 0 {
            // Scroll somewhere else in the list.
            s.window(1).update(&mut s.w.vcx, |r, cx| {
                r.select(i * 250, cx);
            });
        }
        let took = s.w.vcx.update(|window, cx| {
            window.refresh();
            let t = Instant::now();
            let _ = window.draw(cx);
            t.elapsed()
        });
        frames.push(took);
    }
    frames.sort();
    let p99 = frames[(frames.len() * 99).div_ceil(100) - 1];
    eprintln!(
        "timing: Find Results with 10,000 matches: frame median {:.2} ms, p99 {:.2} ms",
        frames[frames.len() / 2].as_secs_f64() * 1e3,
        p99.as_secs_f64() * 1e3
    );
    super::git_tests::assert_budget("the 10,000-match frame p99", p99, Duration::from_millis(8));
}

#[gpui::test]
fn the_first_result_streams_before_the_search_ends(cx: &mut TestAppContext) {
    let mut s = setup_search(cx);
    for i in 0..200 {
        s.write(&format!("stream/f{i:03}.txt"), "needle\n");
    }
    // 25 ms per file: five seconds for the whole search, so the first file's draw lands while it still runs even on a
    // loaded CI runner (the overlay is lifted below once the first file is seen).
    s.service()
        .set_slow_overlay(Some(Duration::from_millis(25)));
    let started = Instant::now();
    s.run(cmds::FIND, json!({"query": "needle", "results_window": 1}));
    let win = s.window(1);
    let mut first = None;
    s.w.wait("the first file", |w| {
        let shown = win.read_with(&w.vcx, |r, _| {
            r.blocks.first().is_some_and(|b| !b.files.is_empty())
        });
        if shown && first.is_none() {
            first = Some(started.elapsed());
        }
        shown
    });
    let running = win.read_with(&s.w.vcx, |r, _| r.running().is_some());
    assert!(running, "the first file drew while the search ran");
    s.service().set_slow_overlay(None);
    s.wait_done(1);
    eprintln!(
        "timing: Enter to the first result drawn {:.1} ms (with 25 ms per file)",
        first.unwrap().as_secs_f64() * 1e3
    );
}
