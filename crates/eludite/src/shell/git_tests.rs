//! Headless tests of brief 0040: the Git Changes and Git Repository windows over a temporary repository (built with
//! git2, its identity in its own config, never this checkout), the status bar, the Workspace and tab glyphs, Compare
//! with Unmodified, the change margin, the confirmations, branches, a merge conflict, Fetch and Push against a bare
//! repository on disk, the generation rule, agents' calls and the drafts. Brief 0045: the credential prompt against
//! `git http-backend` behind basic authentication (the test server of `crates/git/tests/support/server.rs`), its
//! refusal for agents, the session's memory, the warning line while `http.sslVerify` is off and a refused
//! certificate naming its host.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_commands::git as cmds;
use eludite_commands::{Caller, CommandSpec, PermissionClass, with_caller};
use eludite_git::git2;
use eludite_git::{GlobalConfig, WatchOptions};
use eludite_mcp::{CallContext, GateDecision, McpServer};
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::explorer::row_selector;
use super::git::changes::{self, Group};
use super::git::credentials;
use super::git::service::{AGENT_CANNOT_ANSWER, GitSetup};
use super::git::{INCOMING_SLOT, OUTGOING_SLOT, PENDING_SLOT, compare, gutter, repository};
use super::tests::{Ws, setup};

const PROGRAM: &str = super::tests::PROGRAM;

/// A call the gate was asked about: the command, its class and why it was raised.
type Asked = (String, PermissionClass, Option<String>);

fn agent() -> Caller {
    Caller::Agent {
        agent: "test-agent".into(),
        call: 1,
        tool_call: None,
    }
}

/// The git half of a test: the workspace's repository opened with git2.
struct G {
    w: Ws,
}

fn signature() -> git2::Signature<'static> {
    git2::Signature::now("Test", "test@example.com").unwrap()
}

fn identity(r: &git2::Repository) {
    let mut c = r.config().unwrap();
    c.set_str("user.name", "Test").unwrap();
    c.set_str("user.email", "test@example.com").unwrap();
}

/// Stage everything in `r` and commit it with git2.
fn commit_all(r: &git2::Repository, message: &str) -> git2::Oid {
    let mut index = r.index().unwrap();
    index
        .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.update_all(["*"], None).unwrap();
    index.write().unwrap();
    let tree = r.find_tree(index.write_tree().unwrap()).unwrap();
    let parents: Vec<git2::Commit<'_>> = r
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .into_iter()
        .collect();
    let refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
    r.commit(
        Some("HEAD"),
        &signature(),
        &signature(),
        message,
        &tree,
        &refs,
    )
    .unwrap()
}

/// The shell with an isolated git setup (no global config, a fast watcher, drafts in the temporary folder).
fn shell(cx: &mut TestAppContext) -> Ws {
    let w = setup(cx);
    let state = w.dir.path().join("git-state");
    w.shell.read_with(&w.vcx, |s, _| {
        s.git().service.set_setup(GitSetup {
            global: GlobalConfig::Files(vec![]),
            watch: WatchOptions {
                poll: Duration::from_millis(10),
                debounce: Duration::from_millis(20),
                rescan: Duration::from_millis(100),
            },
            state_dir: Some(state),
        })
    });
    w
}

/// A repository in the workspace folder with everything committed on `main`, and the solution open.
fn setup_git(cx: &mut TestAppContext) -> G {
    let w = shell(cx);
    let r = git2::Repository::init(w.dir.path()).unwrap();
    r.set_head("refs/heads/main").unwrap();
    identity(&r);
    std::fs::write(
        w.dir.path().join(".gitignore"),
        "user-config/\ngit-state/\nremote.git/\nclone/\n",
    )
    .unwrap();
    commit_all(&r, "Initial commit");
    let mut g = G { w };
    g.w.open_solution();
    g.wait_ready();
    g
}

impl G {
    fn repo(&self) -> git2::Repository {
        git2::Repository::open(self.w.dir.path()).unwrap()
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.w.path(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.w.path(rel)).unwrap()
    }

    fn wait_ready(&mut self) {
        self.w.wait("the repository's status", |w| {
            w.shell.read_with(&w.vcx, |s, _| s.git().status.is_some())
        });
    }

    fn generation(&self) -> u64 {
        self.w.shell.read_with(&self.w.vcx, |s, _| s.git().shown)
    }

    /// Wait until the status on screen satisfies `f`.
    fn wait_status(&mut self, what: &str, f: impl Fn(&eludite_git::Status) -> bool) {
        self.w.wait(what, |w| {
            w.shell
                .read_with(&w.vcx, |s, _| s.git().status.as_deref().is_some_and(&f))
        });
    }

    /// The Git Changes window's group.
    fn group(&self, group: Group) -> Vec<String> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, cx| s.git().changes.read(cx).group(group))
    }

    fn info(&self) -> Option<(String, bool)> {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            s.git().changes.read(cx).info().cloned()
        })
    }

    fn slot(&self, id: &str) -> String {
        self.w.shell.read_with(&self.w.vcx, |s, _| {
            s.status().get(id).unwrap_or_default().to_owned()
        })
    }

    fn show(&mut self, id: &str) {
        // Properties shares the right dock with Git Changes; out of the way, the list has room.
        let _ = self
            .w
            .commands
            .invoke("eludite.view.hide", json!({ "id": "properties" }));
        self.w
            .commands
            .invoke("eludite.view.show", json!({ "id": id }))
            .unwrap();
        if id == "git_repository" {
            // Docked at the bottom: room for the history.
            self.w
                .commands
                .invoke(
                    "eludite.view.resize",
                    json!({ "side": "bottom", "size": 420 }),
                )
                .unwrap();
        }
        self.w.vcx.run_until_parked();
    }

    /// Run a command as the UI does (the shell's `run`, off the UI thread for git).
    fn ui(&mut self, command: &str, args: Value) {
        let (command, args) = (command.to_owned(), args);
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

    fn type_message(&mut self, text: &str) {
        self.w.click(changes::MESSAGE_BOX);
        let keys: Vec<String> = text
            .chars()
            .map(|c| match c {
                ' ' => "space".to_owned(),
                c => c.to_string(),
            })
            .collect();
        self.w.vcx.simulate_keystrokes(&keys.join(" "));
        self.w.vcx.run_until_parked();
    }

    fn message(&self) -> String {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            s.git().changes.read(cx).message().to_owned()
        })
    }

    fn head_message(&self) -> String {
        self.repo()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message()
            .unwrap()
            .to_owned()
    }
}

#[gpui::test]
fn the_window_lists_the_groups_and_stage_moves_a_file(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    g.show("git_changes");
    assert!(g.group(Group::Changes).is_empty());
    g.write("src/App/Program.cs", "class Program { }\n");
    g.write("src/App/New.cs", "class New { }\n");
    std::fs::remove_file(g.w.path("src/App/Default.aspx")).unwrap();
    g.wait_status("three changes", |s| s.changed_files() == 3);
    g.w.vcx.run_until_parked();
    assert_eq!(
        g.group(Group::Changes),
        [
            "src/App/Default.aspx",
            "src/App/New.cs",
            "src/App/Program.cs"
        ]
    );
    assert!(g.group(Group::Staged).is_empty());
    // The rows carry the glyphs: modified, untracked, deleted.
    let glyphs = g.w.shell.read_with(&g.w.vcx, |s, cx| {
        s.git()
            .changes
            .read(cx)
            .model
            .changes
            .iter()
            .map(|f| f.glyph)
            .collect::<Vec<_>>()
    });
    use eludite_git::FileGlyph::*;
    assert_eq!(glyphs, [Deleted, Untracked, Modified]);
    // Stage (+) on Program.cs moves it to Staged Changes.
    g.w.click(&changes::button_selector("stage", "src/App/Program.cs"));
    g.wait_status("Program.cs staged", |s| s.staged.len() == 1);
    g.w.vcx.run_until_parked();
    assert_eq!(g.group(Group::Staged), ["src/App/Program.cs"]);
    assert_eq!(
        g.group(Group::Changes),
        ["src/App/Default.aspx", "src/App/New.cs"]
    );
    assert!(g.w.audit().contains(&cmds::STAGE.to_owned()));
    // Unstage (-) moves it back; Stage All stages everything.
    g.w.click(&changes::button_selector("unstage", "src/App/Program.cs"));
    g.wait_status("unstaged", |s| s.staged.is_empty());
    g.w.vcx.run_until_parked();
    g.w.click(changes::STAGE_ALL);
    g.wait_status("all staged", |s| s.staged.len() == 3);
    g.w.vcx.run_until_parked();
    assert!(g.group(Group::Changes).is_empty());
    assert_eq!(g.group(Group::Staged).len(), 3);
}

#[gpui::test]
fn commit_all_commits_with_the_message_clears_the_box_and_amend_rewrites(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    g.show("git_changes");
    g.write("src/App/Program.cs", "class Program { int x; }\n");
    g.wait_status("a change", |s| s.unstaged.len() == 1);
    g.type_message("add a field");
    assert_eq!(g.message(), "add a field");
    // Nothing staged: the button is Commit All.
    g.w.click(changes::COMMIT);
    g.wait_status("committed", |s| s.unstaged.is_empty());
    g.w.vcx.run_until_parked();
    assert_eq!(g.head_message(), "add a field\n");
    assert_eq!(g.message(), "", "the box is cleared");
    let info = g.info().unwrap();
    assert!(info.0.starts_with("Commit ") && !info.1, "{info:?}");
    // Amend: a change staged, the message replaced, the commit rewritten in place.
    let parent = g
        .repo()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .parent_id(0)
        .unwrap();
    g.write("src/App/Models/Order.cs", "class Order { int id; }\n");
    g.wait_status("another change", |s| s.unstaged.len() == 1);
    g.w.click(changes::STAGE_ALL);
    g.wait_status("staged", |s| s.staged.len() == 1);
    g.w.click(changes::AMEND);
    g.type_message("add fields");
    g.w.click(changes::COMMIT);
    g.wait_status("amended", |s| s.staged.is_empty());
    g.w.vcx.run_until_parked();
    let repo = g.repo();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.message(), Some("add fields\n"));
    assert_eq!(
        head.parent_id(0).unwrap(),
        parent,
        "the commit was replaced"
    );
    assert!(
        repo.find_tree(head.tree_id())
            .unwrap()
            .get_path(Path::new("src/App/Models/Order.cs"))
            .is_ok()
    );
    // An empty message is refused in the window.
    g.w.click(changes::COMMIT);
    assert_eq!(g.info().unwrap(), ("Enter a commit message".into(), true));
}

#[gpui::test]
fn undo_changes_asks_then_restores_and_reset_hard_asks(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    g.show("git_changes");
    g.write("src/App/Program.cs", "broken\n");
    g.wait_status("a change", |s| s.unstaged.len() == 1);
    g.w.vcx.run_until_parked();
    g.w.click(&changes::button_selector("undo", "src/App/Program.cs"));
    assert!(g.w.vcx.has_pending_prompt(), "Undo Changes asks first");
    g.w.vcx.simulate_prompt_answer("No");
    g.w.vcx.run_until_parked();
    assert_eq!(g.read("src/App/Program.cs"), "broken\n");
    g.w.click(&changes::button_selector("undo", "src/App/Program.cs"));
    g.w.vcx.simulate_prompt_answer("Yes");
    g.wait_status("restored", |s| s.unstaged.is_empty());
    assert_eq!(g.read("src/App/Program.cs"), PROGRAM);
    // Reset > Delete Changes asks too.
    g.write("src/App/Program.cs", "again\n");
    g.wait_status("a change", |s| s.unstaged.len() == 1);
    g.ui(cmds::RESET, json!({ "revision": "HEAD", "mode": "hard" }));
    assert!(g.w.vcx.has_pending_prompt());
    g.w.vcx.simulate_prompt_answer("Yes");
    g.wait_status("reset", |s| s.unstaged.is_empty());
    assert_eq!(g.read("src/App/Program.cs"), PROGRAM);
}

#[gpui::test]
fn the_status_bar_shows_the_branch_and_counts_and_the_glyphs_follow_the_file(
    cx: &mut TestAppContext,
) {
    let mut g = setup_git(cx);
    assert_eq!(g.slot(eludite_ui::slots::BRANCH), "\u{2387} main");
    assert_eq!(g.slot(PENDING_SLOT), "");
    // Open Program.cs: its tab has no glyph while it is unchanged.
    let project = g.w.path("src/App/App.csproj");
    g.w.click(&format!(
        "{}-toggle",
        row_selector(&project.to_string_lossy())
    ));
    let program = g.w.path("src/App/Program.cs");
    let row = row_selector(&format!("{}|Program.cs", project.to_string_lossy()));
    g.w.double_click(&row);
    let view = g.w.editor(&program);
    let id = program.to_string_lossy().into_owned();
    let badge = |g: &G| g.w.controller.snapshot().badges.get(&id).cloned();
    assert_eq!(badge(&g), None);
    let explorer_glyph = |g: &G| {
        g.w.shell.read_with(&g.w.vcx, |s, cx| {
            let e = s.explorer().read(cx);
            e.rows()
                .iter()
                .find(|r| r.label == "Program.cs")
                .and_then(|r| e.git_glyph(r))
        })
    };
    assert_eq!(explorer_glyph(&g), None);
    // Edit and save: the file is modified in the Workspace window, on its tab and in the status bar's count.
    view.update(&mut g.w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.insert("// edited\n"))
    });
    g.ui(
        eludite_commands::workspace::EDITOR_SAVE,
        json!({ "path": id }),
    );
    g.wait_status("modified", |s| s.unstaged.len() == 1);
    g.w.vcx.run_until_parked();
    assert_eq!(explorer_glyph(&g), Some(eludite_git::FileGlyph::Modified));
    assert_eq!(
        badge(&g),
        Some(("\u{2713}".into(), eludite_git::FileGlyph::Modified.color()))
    );
    assert_eq!(g.slot(PENDING_SLOT), "\u{270E} 1");
    // A new file shows the untracked glyph; staging it, the added one.
    g.write("src/App/Models/Item.cs", "class Item { }\n");
    g.wait_status("untracked", |s| s.untracked.len() == 1);
    g.agent(cmds::STAGE, json!({ "paths": ["src/App/Models/Item.cs"] }))
        .unwrap();
    g.wait_status("added", |s| s.staged.len() == 1);
    g.w.vcx.run_until_parked();
    let glyphs = g.w.shell.read_with(&g.w.vcx, |s, _| s.git().glyphs.clone());
    assert_eq!(
        glyphs.get("src/App/Models/Item.cs"),
        Some(eludite_git::FileGlyph::Added)
    );
    assert_eq!(g.slot(PENDING_SLOT), "\u{270E} 2");
    // Commit All clears the glyphs.
    g.agent(cmds::COMMIT, json!({ "message": "edit", "all": true }))
        .unwrap();
    g.wait_status("clean", |s| s.changed_files() == 0);
    g.w.vcx.run_until_parked();
    assert_eq!(badge(&g), None);
    assert_eq!(explorer_glyph(&g), None);
    assert_eq!(g.slot(PENDING_SLOT), "");
    // No upstream: no arrows yet.
    assert_eq!(g.slot(INCOMING_SLOT), "");
    assert_eq!(g.slot(OUTGOING_SLOT), "\u{2191} Publish");
}

#[gpui::test]
fn compare_with_unmodified_opens_two_panes_with_the_hunks_and_f8_moves(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    // A 5,000-line file with two changes.
    let old: String = (1..=5000).map(|i| format!("line {i}\n")).collect();
    g.write("src/App/Big.cs", &old);
    commit_all(&g.repo(), "big");
    let new = old
        .replace("line 10\n", "line ten\n")
        .replace("line 4000\n", "line 4000\nan added line\n");
    g.write("src/App/Big.cs", &new);
    g.wait_status("a change", |s| s.unstaged.len() == 1);
    let started = Instant::now();
    g.ui(cmds::DIFF, json!({ "path": "src/App/Big.cs" }));
    let id = compare::compare_tab("src/App/Big.cs", "index", false);
    g.w.wait("the compare document", |w| {
        w.controller.snapshot().layout.documents.get(&id).is_some()
    });
    let opened = started.elapsed();
    eprintln!(
        "timing: Compare with Unmodified of a 5,000-line file opened in {:.1} ms",
        opened.as_secs_f64() * 1e3
    );
    let view =
        g.w.shell
            .read_with(&g.w.vcx, |s, _| {
                s.git().documents.borrow().get(&id).cloned()
            })
            .unwrap()
            .downcast::<compare::CompareView>()
            .unwrap();
    let (hunks, rows) = view.read_with(&g.w.vcx, |v, _| (v.hunks().to_vec(), v.rows().to_vec()));
    assert_eq!(hunks.len(), 2);
    let first = &rows[hunks[0]];
    assert_eq!(first.left.as_ref().unwrap().text, "line 10");
    assert_eq!(first.right.as_ref().unwrap().text, "line ten");
    let second = &rows[hunks[1]];
    assert_eq!(second.left, None);
    assert_eq!(second.right.as_ref().unwrap().text, "an added line");
    assert_eq!(rows.len(), 5001);
    // F8 goes to the next difference, Shift+F8 back.
    let title =
        g.w.controller
            .snapshot()
            .layout
            .documents
            .get(&id)
            .unwrap()
            .title
            .clone();
    assert_eq!(title, "Big.cs vs. Big.cs (index)");
    g.w.vcx.simulate_keystrokes("f8");
    assert_eq!(view.read_with(&g.w.vcx, |v, _| v.current()), Some(0));
    g.w.vcx.simulate_keystrokes("f8");
    assert_eq!(view.read_with(&g.w.vcx, |v, _| v.current()), Some(1));
    g.w.vcx.simulate_keystrokes("shift-f8");
    assert_eq!(view.read_with(&g.w.vcx, |v, _| v.current()), Some(0));
    // The command's own answer, as an agent reads it: the hunks with context, capped.
    let out = g
        .agent(
            cmds::DIFF,
            json!({ "path": "src/App/Big.cs", "context": 1 }),
        )
        .unwrap();
    assert_eq!(out["hunks"].as_array().unwrap().len(), 2);
    assert_eq!(
        (out["added"].as_u64(), out["removed"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(out["new_lines"], 5001);
    assert_eq!(out["hunks"][0]["lines"].as_array().unwrap().len(), 4);
    let capped = g
        .agent(
            cmds::DIFF,
            json!({ "path": "src/App/Big.cs", "max_lines": 1 }),
        )
        .unwrap();
    assert_eq!(
        (capped["omitted"].as_u64(), capped["truncated"].as_bool()),
        (Some(2), Some(true))
    );
    assert_budget(
        "Compare with Unmodified of a 5,000-line file",
        opened,
        Duration::from_millis(100),
    );
}

/// `measured` under `limit`, unless the machine is overloaded (other agents build beside these tests).
pub(super) fn assert_budget(what: &str, measured: Duration, limit: Duration) {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok());
    match load {
        Some(l) if l > cores => eprintln!(
            "timing: {what} {:.2} ms not asserted against {:.0} ms: load average {l:.1} on {cores:.0} cores",
            measured.as_secs_f64() * 1e3,
            limit.as_secs_f64() * 1e3
        ),
        _ => assert!(
            measured < limit,
            "{what}: {measured:?} is not under {limit:?}"
        ),
    }
}

#[gpui::test]
fn ctrl_d_in_the_workspace_and_the_context_menu_compare_and_blame(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    g.write("src/App/Program.cs", "class Program { }\n");
    g.wait_status("a change", |s| s.unstaged.len() == 1);
    let project = g.w.path("src/App/App.csproj");
    g.w.click(&format!(
        "{}-toggle",
        row_selector(&project.to_string_lossy())
    ));
    let row = row_selector(&format!("{}|Program.cs", project.to_string_lossy()));
    g.w.click(&row);
    g.w.vcx.simulate_keystrokes("ctrl-d");
    let id = compare::compare_tab("src/App/Program.cs", "index", false);
    g.w.wait("Ctrl+D's compare document", |w| {
        w.controller.snapshot().layout.documents.get(&id).is_some()
    });
    // The context menu has Blame, which opens the annotated file.
    let at = g.w.bounds(&row).center();
    g.w.vcx.simulate_event(gpui::MouseDownEvent {
        position: at,
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Right,
        click_count: 1,
        first_mouse: false,
    });
    g.w.vcx.run_until_parked();
    g.w.click(&super::explorer::context_item_selector("blame"));
    let blame = compare::blame_tab("src/App/Program.cs");
    g.w.wait("the blame document", |w| {
        w.controller
            .snapshot()
            .layout
            .documents
            .get(&blame)
            .is_some()
    });
    let view =
        g.w.shell
            .read_with(&g.w.vcx, |s, _| {
                s.git().documents.borrow().get(&blame).cloned()
            })
            .unwrap()
            .downcast::<compare::BlameView>()
            .unwrap();
    let lines = view.read_with(&g.w.vcx, |v, _| v.blame.lines.len());
    assert_eq!(lines, PROGRAM.lines().count());
}

#[gpui::test]
fn the_change_margin_follows_edits_and_clears_after_a_stage(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    let (program, view) = {
        let project = g.w.path("src/App/App.csproj");
        g.w.click(&format!(
            "{}-toggle",
            row_selector(&project.to_string_lossy())
        ));
        let program = g.w.path("src/App/Program.cs");
        let row = row_selector(&format!("{}|Program.cs", project.to_string_lossy()));
        g.w.double_click(&row);
        let view = g.w.editor(&program);
        (program, view)
    };
    let id = program.to_string_lossy().into_owned();
    let marks = |g: &G| {
        g.w.shell.read_with(&g.w.vcx, |s, cx| {
            s.git()
                .margins
                .borrow()
                .get(&id)
                .map(|m| m.read(cx).marks.clone())
                .unwrap_or_default()
        })
    };
    g.w.vcx.run_until_parked();
    assert!(marks(&g).is_empty());
    view.update(&mut g.w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.insert("// new\n"))
    });
    g.w.vcx.run_until_parked();
    assert!(marks(&g).is_empty(), "not before the editor is idle");
    let started = Instant::now();
    g.w.vcx.executor().advance_clock(gutter::IDLE);
    g.w.wait("the margin", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git()
                .margins
                .borrow()
                .get(&id)
                .is_some_and(|m| !m.read(cx).marks.is_empty())
        })
    });
    let compute = started.elapsed();
    eprintln!(
        "timing: change margin {:.0} ms idle + {:.1} ms to read the index and diff",
        gutter::IDLE.as_secs_f64() * 1e3,
        compute.as_secs_f64() * 1e3
    );
    assert_eq!(
        marks(&g),
        [gutter::Mark {
            row: 0,
            len: 1,
            kind: gutter::MarkKind::Added
        }]
    );
    assert_budget(
        "the change margin after an edit",
        gutter::IDLE + compute,
        Duration::from_millis(200),
    );
    // Save and stage: the index has the line, the margin clears.
    g.ui(
        eludite_commands::workspace::EDITOR_SAVE,
        json!({ "path": id }),
    );
    g.wait_status("modified", |s| s.unstaged.len() == 1);
    g.agent(cmds::STAGE, json!({ "paths": ["src/App/Program.cs"] }))
        .unwrap();
    g.wait_status("staged", |s| s.staged.len() == 1);
    g.w.wait("the margin cleared", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git()
                .margins
                .borrow()
                .get(&id)
                .is_some_and(|m| m.read(cx).marks.is_empty())
        })
    });
}

#[gpui::test]
fn checkout_of_a_dirty_tree_is_refused_and_new_branch_creates_and_switches(
    cx: &mut TestAppContext,
) {
    let mut g = setup_git(cx);
    // A branch where Program.cs differs.
    let r = g.repo();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("other", &head, false).unwrap();
    r.set_head("refs/heads/other").unwrap();
    g.write("src/App/Program.cs", "class Other { }\n");
    commit_all(&r, "other");
    r.set_head("refs/heads/main").unwrap();
    r.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    g.wait_status("on main", |s| {
        s.branch.as_deref() == Some("main") && s.changed_files() == 0
    });
    g.show("git_changes");
    g.write("src/App/Program.cs", "local change\n");
    g.wait_status("dirty", |s| s.unstaged.len() == 1);
    g.ui(cmds::CHECKOUT, json!({ "name": "other" }));
    g.w.wait("the refusal", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.git().changes.read(cx).info().is_some())
    });
    let (info, error) = g.info().unwrap();
    assert!(error);
    assert!(
        info.contains("would be overwritten by checkout: src/App/Program.cs"),
        "{info}"
    );
    assert_eq!(g.read("src/App/Program.cs"), "local change\n");
    // New Branch (Git > New Branch... focuses the Git Repository window's box): creates and switches.
    g.w.commands
        .invoke(cmds::DISCARD, json!({ "all": true }))
        .unwrap();
    g.wait_status("clean", |s| s.changed_files() == 0);
    g.ui(cmds::CHECKOUT, json!({}));
    g.w.vcx.simulate_keystrokes("f e a t u r e");
    g.w.vcx.run_until_parked();
    let typed = g.w.shell.read_with(&g.w.vcx, |s, cx| {
        s.git().repository.read(cx).new_branch_text().to_owned()
    });
    assert_eq!(typed, "feature");
    g.w.click(repository::NEW_BRANCH_CREATE);
    g.wait_status("on feature", |s| s.branch.as_deref() == Some("feature"));
    assert_eq!(g.slot(eludite_ui::slots::BRANCH), "\u{2387} feature");
    assert!(
        g.repo()
            .find_branch("feature", git2::BranchType::Local)
            .is_ok()
    );
}

#[gpui::test]
fn a_merge_conflict_opens_the_file_with_markers_and_staging_resolves_it(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    let r = g.repo();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("theirs", &head, false).unwrap();
    r.set_head("refs/heads/theirs").unwrap();
    g.write("src/App/Program.cs", "class Theirs { }\n");
    commit_all(&r, "theirs");
    r.set_head("refs/heads/main").unwrap();
    r.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    g.write("src/App/Program.cs", "class Ours { }\n");
    commit_all(&r, "ours");
    g.wait_status("on main, clean", |s| {
        s.changed_files() == 0 && s.branch.as_deref() == Some("main")
    });
    // Merge from the Git Repository window: the branch, then Merge into Current Branch.
    g.show("git_repository");
    g.w.wait("the branches", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().repository.read(cx).branches.is_some()
        })
    });
    g.w.click(&repository::branch_selector("theirs"));
    g.w.click(repository::MERGE);
    let program = g.w.path("src/App/Program.cs");
    let view = g.w.editor(&program);
    let text = g.w.text(&view);
    assert!(
        text.contains("<<<<<<<") && text.contains("Ours") && text.contains("Theirs"),
        "{text}"
    );
    g.wait_status("conflicted", |s| s.conflicted == ["src/App/Program.cs"]);
    g.show("git_changes");
    assert_eq!(g.group(Group::Merge), ["src/App/Program.cs"]);
    // The margin shows the sides.
    g.w.vcx.executor().advance_clock(gutter::IDLE);
    let id = program.to_string_lossy().into_owned();
    g.w.wait("the conflict's sides", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().margins.borrow().get(&id).is_some_and(|m| {
                let k: Vec<_> = m.read(cx).marks.iter().map(|m| m.kind).collect();
                k == [gutter::MarkKind::Ours, gutter::MarkKind::Theirs]
            })
        })
    });
    // Resolve in the editor, save, stage: resolved; commit ends the merge.
    view.update(&mut g.w.vcx, |v, cx| {
        v.update_editor(cx, |e| {
            e.select_all();
            e.insert("class Both { }\n")
        })
    });
    g.ui(
        eludite_commands::workspace::EDITOR_SAVE,
        json!({ "path": id }),
    );
    g.w.vcx.run_until_parked();
    g.w.click(&changes::button_selector("stage", "src/App/Program.cs"));
    g.wait_status("resolved", |s| {
        s.conflicted.is_empty() && s.staged.len() == 1
    });
    g.type_message("merge theirs");
    g.w.click(changes::COMMIT);
    g.wait_status("merged", |s| {
        s.operation.is_none() && s.changed_files() == 0
    });
    assert_eq!(
        g.repo()
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_count(),
        2
    );
}

/// A bare remote beside the workspace (`remote.git`), the workspace's `main` pushed to it as `origin/main`, and a
/// second clone (`clone`).
fn with_remote(g: &mut G) -> (PathBuf, git2::Repository) {
    let bare = g.w.path("remote.git");
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).initial_head("main");
    git2::Repository::init_opts(&bare, &opts).unwrap();
    let url = format!("file://{}", bare.display());
    g.repo().remote("origin", &url).unwrap();
    g.agent(cmds::PUSH, json!({ "set_upstream": true }))
        .unwrap_or_else(|e| panic!("{e}"));
    let other = git2::Repository::clone(&url, g.w.path("clone")).unwrap();
    identity(&other);
    g.wait_status("published", |s| {
        s.upstream.as_deref() == Some("origin/main")
    });
    (bare, other)
}

#[gpui::test]
fn fetch_updates_the_incoming_count(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    let (_, other) = with_remote(&mut g);
    assert_eq!(g.slot(INCOMING_SLOT), "\u{2193}0");
    assert_eq!(g.slot(OUTGOING_SLOT), "\u{2191}0");
    // Another clone pushes a commit.
    std::fs::write(other.workdir().unwrap().join("x.cs"), "x\n").unwrap();
    commit_all(&other, "from the other clone");
    let mut remote = other.find_remote("origin").unwrap();
    remote
        .push(&["refs/heads/main:refs/heads/main"], None)
        .unwrap();
    // The status bar's Fetch.
    g.w.click(&eludite_ui::slot_selector(super::git::FETCH_SLOT));
    g.wait_status("one incoming", |s| s.behind == 1);
    g.w.vcx.run_until_parked();
    assert_eq!(g.slot(INCOMING_SLOT), "\u{2193}1");
    // Pull (the incoming arrow) brings it in.
    g.w.click(&eludite_ui::slot_selector(INCOMING_SLOT));
    g.wait_status("pulled", |s| s.behind == 0);
    assert!(g.w.path("x.cs").exists());
}

#[gpui::test]
fn push_asks_for_an_agent_and_runs_for_the_user(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    let (bare, _) = with_remote(&mut g);
    g.write("src/App/Program.cs", "class Pushed { }\n");
    g.agent(cmds::COMMIT, json!({ "message": "to push", "all": true }))
        .unwrap();
    g.wait_status("one outgoing", |s| s.ahead == 1);
    let remote_head = || {
        git2::Repository::open_bare(&bare)
            .unwrap()
            .find_reference("refs/heads/main")
            .unwrap()
            .target()
            .unwrap()
    };
    let before = remote_head();
    // An agent's push goes through the MCP gate as dangerous (git.push: prompt); the gate here says no.
    let asked: Arc<Mutex<Vec<Asked>>> = Arc::default();
    let record = asked.clone();
    let server = McpServer::new(g.w.commands.clone())
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
    let call = |server: &McpServer, args: Value| {
        let s = server.clone();
        std::thread::spawn(move || {
            s.handle_line(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": {"name": "eludite-git-push", "arguments": args}})
                .to_string(),
            )
        })
        .join()
        .unwrap()
    };
    let reply = serde_json::to_value(call(&server, json!({})).unwrap()).unwrap();
    assert_eq!(reply["result"]["isError"], true);
    let asked_now = asked.lock().unwrap().clone();
    assert_eq!(asked_now.len(), 1, "the agent's push asked");
    assert_eq!(asked_now[0].1, PermissionClass::Dangerous);
    assert!(
        asked_now[0]
            .2
            .as_deref()
            .unwrap()
            .contains("git.push: prompt")
    );
    assert_eq!(remote_head(), before, "nothing was pushed");
    // A forced push is refused outright, without asking.
    let reply = serde_json::to_value(call(&server, json!({"force": true})).unwrap()).unwrap();
    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("force")
    );
    assert_eq!(
        asked.lock().unwrap().len(),
        1,
        "a force never reaches the gate"
    );
    // The user's Push runs at once.
    g.show("git_changes");
    g.w.click(changes::PUSH);
    g.wait_status("pushed", |s| s.ahead == 0);
    assert_ne!(remote_head(), before);
    assert_eq!(asked.lock().unwrap().len(), 1);
}

#[gpui::test]
fn a_stale_status_is_not_rendered_and_an_agents_status_matches_the_window(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    g.show("git_changes");
    g.write("src/App/New.cs", "class New { }\n");
    g.wait_status("untracked", |s| s.untracked.len() == 1);
    g.w.vcx.run_until_parked();
    let shown = g.generation();
    // An agent's answer is the window's.
    let out = g.agent(cmds::STATUS, json!({})).unwrap();
    assert_eq!(out["generation"], shown);
    assert_eq!(out["untracked"], json!(g.group(Group::Changes)));
    assert_eq!(out["branch"], "main");
    assert_eq!(out["state"], "ready");
    // An older status (an earlier generation, as a slow answer would bring) is dropped.
    let stale = Arc::new(eludite_git::Status {
        branch: Some("stale".into()),
        ..Default::default()
    });
    let drawn = g.w.shell.update(&mut g.w.vcx, |s, cx| {
        s.git_apply_status(shown - 1, stale.clone(), cx)
    });
    assert!(!drawn);
    assert_eq!(g.group(Group::Changes), ["src/App/New.cs"]);
    assert_eq!(g.slot(eludite_ui::slots::BRANCH), "\u{2387} main");
    // A newer one is drawn.
    let drawn = g.w.shell.update(&mut g.w.vcx, |s, cx| {
        s.git_apply_status(shown + 100, stale.clone(), cx)
    });
    assert!(drawn);
    assert_eq!(g.slot(eludite_ui::slots::BRANCH), "\u{2387} stale");
}

#[gpui::test]
fn no_repository_shows_create_git_repository_and_init_creates_one(cx: &mut TestAppContext) {
    let mut w = shell(cx);
    if eludite_git::Repo::discover(w.dir.path()).unwrap().is_some() {
        eprintln!("skipped: the temporary folder is inside a repository on this machine");
        return;
    }
    w.open_solution();
    w.commands
        .invoke("eludite.view.show", json!({ "id": "git_changes" }))
        .unwrap();
    w.wait("Create Git Repository", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().changes.read(cx).model.repository == Some(false)
        })
    });
    w.vcx.run_until_parked();
    let out = with_caller(agent(), || w.commands.invoke(cmds::STATUS, json!({}))).unwrap();
    assert_eq!(out["state"], "none");
    w.click(changes::CREATE);
    w.wait("the new repository", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.git().status.is_some())
    });
    assert!(w.dir.path().join(".git").is_dir());
    let mut g = G { w };
    g.wait_status("everything untracked", |s| {
        s.branch.as_deref() == Some("main") && !s.untracked.is_empty()
    });
}

#[gpui::test]
fn the_draft_message_survives_a_reopen(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    g.show("git_changes");
    g.type_message("work in progress");
    g.w.vcx.executor().advance_clock(Duration::from_millis(400));
    let file = g.w.dir.path().join("git-state/drafts.json");
    g.w.wait("the draft on disk", |_| {
        std::fs::read_to_string(&file).is_ok_and(|t| t.contains("work in progress"))
    });
    // Close the workspace and open it again: the draft comes back.
    g.w.commands
        .invoke(eludite_commands::workspace::SOLUTION_CLOSE, json!({}))
        .unwrap();
    g.w.wait("closed", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.git().root.is_none())
    });
    g.w.shell.update(&mut g.w.vcx, |s, cx| {
        s.git
            .changes
            .update(cx, |c, cx| c.set_message(String::new(), cx))
    });
    g.w.open_solution();
    g.wait_ready();
    g.w.wait("the draft restored", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().changes.read(cx).message() == "work in progress"
        })
    });
}

#[gpui::test]
fn a_thousand_changed_files_draw_in_a_frame(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    for i in 0..1000 {
        g.write(&format!("gen/f{i:04}.cs"), "class F { }\n");
    }
    g.wait_status("1,000 files", |s| s.untracked.len() == 1000);
    g.show("git_changes");
    let mut frames = Vec::new();
    for _ in 0..20 {
        let took = g.w.vcx.update(|window, cx| {
            window.refresh();
            let t = Instant::now();
            let _ = window.draw(cx);
            t.elapsed()
        });
        frames.push(took);
    }
    frames.sort();
    let worst = *frames.last().unwrap();
    eprintln!(
        "timing: the Git Changes window with 1,000 changed files: frame median {:.2} ms, max {:.2} ms",
        frames[frames.len() / 2].as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3
    );
    assert_budget(
        "the slowest frame of 1,000 changed files",
        worst,
        Duration::from_millis(8),
    );
}

#[gpui::test]
fn status_answers_from_the_cache_on_ten_thousand_files(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    for d in 0..100 {
        for f in 0..100 {
            g.write(&format!("big/m{d:03}/f{f:03}.cs"), "class C { }\n");
        }
    }
    commit_all(&g.repo(), "10,000 files");
    for i in 0..100 {
        g.write(&format!("big/m{i:03}/f000.cs"), "changed\n");
    }
    g.wait_status("100 changes", |s| s.unstaged.len() == 100);
    let mut took = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        let out = g.agent(cmds::STATUS, json!({})).unwrap();
        took.push(t.elapsed());
        assert_eq!(out["totals"]["unstaged"], 100);
    }
    took.sort();
    let p95 = took[(took.len() * 95).div_ceil(100) - 1];
    eprintln!(
        "timing: eludite.git.status from the cache, 10,000 files, 100 changes: p95 {:.2} ms",
        p95.as_secs_f64() * 1e3
    );
    assert_budget("status p95", p95, Duration::from_millis(50));
}

#[gpui::test]
fn the_identity_comes_from_the_settings_and_its_refusal_names_them(cx: &mut TestAppContext) {
    use super::options::{section_selector, setting_selector};
    let mut w = shell(cx);
    // A repository with no identity in its config (and no global config: the setup's is empty).
    let r = git2::Repository::init(w.dir.path()).unwrap();
    r.set_head("refs/heads/main").unwrap();
    std::fs::write(
        w.dir.path().join(".gitignore"),
        "user-config/\ngit-state/\n",
    )
    .unwrap();
    commit_all(&r, "Initial commit");
    w.open_solution();
    let mut g = G { w };
    g.wait_ready();
    g.show("git_changes");
    g.write("src/App/Program.cs", "class Program { int x; }\n");
    g.wait_status("a change", |s| s.unstaged.len() == 1);
    g.type_message("needs an identity");
    g.w.click(changes::COMMIT);
    g.w.wait("the refusal", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.git().changes.read(cx).info().is_some())
    });
    let (info, error) = g.info().unwrap();
    assert!(error);
    assert!(
        info.contains("git config --global user.name") && info.contains("git.userName"),
        "{info}"
    );
    // Tools > Options > Source Control > Git Global Settings: the name, and the automatic fetch (a whole number).
    g.ui(eludite_commands::settings::OPTIONS, json!({}));
    let schema = eludite_commands::settings::SettingsSchema::builtin();
    let page = schema
        .sections
        .iter()
        .position(|s| s == "Source Control > Git Global Settings")
        .unwrap();
    g.w.click(&section_selector(page));
    g.w.click(&setting_selector("git.userName"));
    g.w.vcx.simulate_keystrokes("T e s t e r enter");
    g.w.click(&setting_selector("git.autoFetchMinutes"));
    g.w.vcx.simulate_keystrokes("1 5 enter");
    g.w.wait("the automatic fetch", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.git_auto_fetch_minutes() == Some(15))
    });
    let got = g
        .agent(
            eludite_commands::settings::GET,
            json!({ "key": "git.autoFetchMinutes" }),
        )
        .unwrap();
    assert_eq!(got["settings"][0]["value"], 15);
    g.agent(
        eludite_commands::settings::SET,
        json!({ "key": "git.userEmail", "value": "tester@example.com" }),
    )
    .unwrap();
    g.w.wait("the identity setting", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.git()
                .service
                .settings()
                .fallback
                .is_some_and(|f| f.email == "tester@example.com")
        })
    });
    g.w.click(changes::COMMIT);
    g.wait_status("committed", |s| s.unstaged.is_empty());
    let repo = g.repo();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.author().name(), Some("Tester"));
    assert_eq!(head.author().email(), Some("tester@example.com"));
    // 0 turns it off.
    g.agent(
        eludite_commands::settings::SET,
        json!({ "key": "git.autoFetchMinutes", "value": 0 }),
    )
    .unwrap();
    g.w.wait("no automatic fetch", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.git_auto_fetch_minutes().is_none())
    });
}

#[gpui::test]
fn the_git_menu_and_the_status_bar_run_their_commands(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    let labels =
        g.w.shell
            .read_with(&g.w.vcx, |s, cx| s.menu().read(cx).enabled_labels("Git"));
    assert_eq!(
        labels,
        [
            "Commit or Stash...",
            "Fetch",
            "Pull",
            "Push",
            "Sync",
            "New Branch...",
            "Manage Branches",
            "Open in File Explorer"
        ]
    );
    // Git > Open in File Explorer: the repository's folder.
    g.w.click("menu-Git");
    g.w.click("menu-item-Git-Open in File Explorer");
    let opened = g.w.opened.lock().unwrap().clone();
    assert_eq!(
        opened.last().map(|p| normalize(p)),
        Some(normalize(g.w.dir.path()))
    );
    // Git > Manage Branches: the Git Repository window with the branch and the history.
    g.w.click("menu-Git");
    g.w.click("menu-item-Git-Manage Branches");
    g.w.wait("the history", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| !s.git().repository.read(cx).log.is_empty())
    });
    let (branches, log) = g.w.shell.read_with(&g.w.vcx, |s, cx| {
        let r = s.git().repository.read(cx);
        (
            r.branches
                .as_ref()
                .unwrap()
                .branches
                .iter()
                .map(|b| b.name.clone())
                .collect::<Vec<_>>(),
            r.log.iter().map(|e| e.summary.clone()).collect::<Vec<_>>(),
        )
    });
    assert_eq!(branches, ["main"]);
    assert_eq!(log, ["Initial commit"]);
    // Ctrl+0, Ctrl+G shows Git Changes; the status bar's branch shows the Git Repository window.
    g.w.vcx.simulate_keystrokes("ctrl-0 ctrl-g");
    assert_eq!(
        g.w.controller.snapshot().active_tool.as_deref(),
        Some("git_changes")
    );
    g.w.click(&eludite_ui::slot_selector(eludite_ui::slots::BRANCH));
    assert_eq!(
        g.w.controller.snapshot().active_tool.as_deref(),
        Some("git_repository")
    );
    g.w.vcx.simulate_keystrokes("ctrl-0 ctrl-g");
    g.w.vcx.simulate_keystrokes("ctrl-0 ctrl-r");
    assert_eq!(
        g.w.controller.snapshot().active_tool.as_deref(),
        Some("git_repository")
    );
}

fn normalize(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

#[gpui::test]
fn stash_all_lists_the_stash_and_pop_and_drop_ask_or_run(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    g.show("git_changes");
    g.write("src/App/Program.cs", "class Stashed { }\n");
    g.write("src/App/New.cs", "class New { }\n");
    g.wait_status("two changes", |s| s.changed_files() == 2);
    g.type_message("half done");
    g.w.click(changes::STASH_ALL);
    g.wait_status("stashed", |s| s.changed_files() == 0 && s.stashes == 1);
    g.w.wait("the stash listed", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().changes.read(cx).group(Group::Stashes).len() == 1
        })
    });
    assert!(g.group(Group::Stashes)[0].contains("half done"));
    assert_eq!(g.read("src/App/Program.cs"), PROGRAM);
    // Pop brings the changes back and the stash goes.
    g.w.click(&changes::stash_selector("pop", 0));
    g.wait_status("popped", |s| s.changed_files() == 2 && s.stashes == 0);
    assert_eq!(g.read("src/App/Program.cs"), "class Stashed { }\n");
    // Drop asks.
    g.w.click(changes::STASH_ALL);
    g.wait_status("stashed again", |s| s.stashes == 1);
    g.w.wait("the stash listed", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().changes.read(cx).group(Group::Stashes).len() == 1
        })
    });
    g.w.click(&changes::stash_selector("drop", 0));
    assert!(g.w.vcx.has_pending_prompt());
    g.w.vcx.simulate_prompt_answer("Yes");
    g.wait_status("dropped", |s| s.stashes == 0);
}

#[gpui::test]
fn the_repository_window_cherry_picks_and_resets(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    let r = g.repo();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    r.branch("topic", &head, false).unwrap();
    r.set_head("refs/heads/topic").unwrap();
    g.write("src/App/Picked.cs", "class Picked { }\n");
    let picked = commit_all(&r, "the change to pick");
    r.set_head("refs/heads/main").unwrap();
    r.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    // Main moves on, so the pick is a new commit (not the same object again).
    g.write("src/App/Main.cs", "class Main { }\n");
    let moved = commit_all(&r, "main moves");
    g.wait_status("on main", |s| {
        s.branch.as_deref() == Some("main") && s.head == Some(moved)
    });
    g.show("git_repository");
    g.w.wait("the graph", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.git().repository.read(cx).log.len() == 3)
    });
    let to_pick = picked.to_string();
    g.w.wait("the commit to pick selected", |w| {
        let row = w.shell.read_with(&w.vcx, |s, cx| {
            let r = s.git().repository.read(cx);
            (!r.loading && !r.stale)
                .then(|| r.log.iter().position(|e| e.commit == to_pick))
                .flatten()
        });
        if let Some(ix) = row {
            w.click(&repository::commit_selector(ix));
        }
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().repository.read(cx).selected
                == Some(repository::Selection::Commit(to_pick.clone()))
        })
    });
    g.w.click(repository::CHERRY_PICK);
    g.wait_status("picked", |s| {
        s.changed_files() == 0 && s.head != Some(moved)
    });
    assert!(g.w.path("src/App/Picked.cs").exists());
    assert_eq!(g.head_message(), "the change to pick");
    // Reset > Keep Changes back to the first commit leaves the file as a change.
    g.w.wait("the new graph", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.git().repository.read(cx).log.len() == 4)
    });
    // Commits made in one second tie in the graph's order, which a reload may change: select by id.
    let initial = head.id().to_string();
    g.w.wait("the first commit selected", |w| {
        let first = w.shell.read_with(&w.vcx, |s, cx| {
            let r = s.git().repository.read(cx);
            (!r.loading && !r.stale)
                .then(|| r.log.iter().position(|e| e.commit == initial))
                .flatten()
        });
        if let Some(ix) = first {
            w.click(&repository::commit_selector(ix));
        }
        w.shell.read_with(&w.vcx, |s, cx| {
            s.git().repository.read(cx).selected
                == Some(repository::Selection::Commit(initial.clone()))
        })
    });
    g.w.click(repository::RESET_KEEP);
    g.wait_status("reset", |s| {
        s.untracked == ["src/App/Main.cs", "src/App/Picked.cs"] && s.head == Some(head.id())
    });
}

// ----- Brief 0045: credentials and certificates -----

#[path = "../../../git/tests/support/server.rs"]
mod git_server;

/// The workspace's `main` on `remote.git` (see [`with_remote`]), now served over http by `git http-backend`
/// demanding alice / s3cret; `None` (the test skips) without `git` on PATH.
fn with_http_remote(g: &mut G) -> Option<git_server::GitHttp> {
    with_remote(g);
    let server = git_server::GitHttp::start(g.w.dir.path(), "alice", "s3cret")?;
    g.repo()
        .remote_set_url("origin", &server.url("remote.git"))
        .unwrap();
    Some(server)
}

impl G {
    /// The open credential prompt's host and whether it says the last answer was refused.
    fn prompt(&self) -> Option<(String, bool)> {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            s.git().prompt.as_ref().map(|p| {
                let p = p.read(cx);
                (p.host().to_owned(), p.refused())
            })
        })
    }

    fn wait_prompt(&mut self, refused: bool) -> String {
        self.w.wait("the credential prompt", |w| {
            w.shell.read_with(&w.vcx, |s, cx| {
                s.git()
                    .prompt
                    .as_ref()
                    .is_some_and(|p| p.read(cx).refused() == refused)
            })
        });
        self.prompt().unwrap().0
    }

    /// Type a user name and a password into the prompt, tick "Remember for this session" if asked, then OK.
    fn answer(&mut self, user: &str, password: &str, remember: bool) {
        let keys = |t: &str| {
            t.chars()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };
        self.w
            .vcx
            .simulate_keystrokes(&format!("{} tab {}", keys(user), keys(password)));
        self.w.vcx.run_until_parked();
        if remember {
            self.w.click(credentials::REMEMBER);
        }
        self.w.click(credentials::OK);
    }

    fn wait_info(&mut self, what: &str, f: impl Fn(&str) -> bool) {
        self.w.wait(what, |w| {
            w.shell.read_with(&w.vcx, |s, cx| {
                s.git()
                    .changes
                    .read(cx)
                    .info()
                    .is_some_and(|(text, _)| f(text))
            })
        });
    }

    fn kept_hosts(&self) -> Vec<String> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.git().service.credentials().hosts())
    }
}

#[gpui::test]
fn the_credential_prompt_asks_the_user_and_an_agent_is_refused(cx: &mut TestAppContext) {
    let mut g = setup_git(cx);
    let Some(server) = with_http_remote(&mut g) else {
        return;
    };
    let host = server.host();
    // An agent's fetch: refused with credentials_required and the host, and no prompt for anyone.
    let e = g.agent(cmds::FETCH, json!({})).unwrap_err();
    assert!(
        e.contains(&format!("credentials_required: {host} asks")),
        "{e}"
    );
    assert!(e.contains(AGENT_CANNOT_ANSWER), "{e}");
    g.w.vcx.run_until_parked();
    assert_eq!(g.prompt(), None, "an agent never gets the prompt");
    // The person's Fetch: the prompt for the host.
    g.show("git_changes");
    g.w.click(changes::FETCH);
    assert_eq!(g.wait_prompt(false), host);
    let message = g.w.shell.read_with(&g.w.vcx, |s, cx| {
        s.git().prompt.as_ref().unwrap().read(cx).message()
    });
    assert!(
        message.starts_with(&format!("{host} asks for a user name")),
        "{message}"
    );
    // A wrong password: asked again, saying it was refused.
    g.answer("alice", "wrong", false);
    assert_eq!(g.wait_prompt(true), host);
    // The right one, for this transfer only: the fetch runs, then the answer is forgotten.
    g.answer("alice", "s3cret", false);
    g.wait_info("fetched", |t| t.starts_with("Fetched from origin"));
    assert_eq!(g.prompt(), None);
    assert!(g.kept_hosts().is_empty(), "a one-time answer is not kept");
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.user.as_deref() == Some("alice") && r.status == 200)
    );
    // Cancel: nothing runs, and the window says why.
    g.w.click(changes::FETCH);
    g.wait_prompt(false);
    g.w.click(credentials::CANCEL);
    assert_eq!(g.prompt(), None);
    g.wait_info("canceled", |t| {
        t == format!("Fetch canceled: no credentials for {host}")
    });
}

#[gpui::test]
fn a_remembered_credential_is_reused_in_the_session_and_forgotten_at_close(
    cx: &mut TestAppContext,
) {
    let mut g = setup_git(cx);
    let Some(server) = with_http_remote(&mut g) else {
        return;
    };
    let host = server.host();
    g.show("git_changes");
    g.w.click(changes::FETCH);
    g.wait_prompt(false);
    g.answer("alice", "s3cret", true);
    g.wait_info("fetched", |t| t.starts_with("Fetched from origin"));
    assert_eq!(g.kept_hosts(), [host.clone()]);
    // Reused within the session: the person's Push and an agent's fetch run without asking.
    g.write("src/App/Program.cs", "class Remembered { }\n");
    g.agent(
        cmds::COMMIT,
        json!({ "message": "remembered", "all": true }),
    )
    .unwrap();
    g.wait_status("one outgoing", |s| s.ahead == 1);
    g.w.click(changes::PUSH);
    g.wait_status("pushed", |s| s.ahead == 0);
    assert_eq!(
        g.prompt(),
        None,
        "no prompt while the credential is remembered"
    );
    g.agent(cmds::FETCH, json!({}))
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.status == 200 && r.target.contains("git-receive-pack"))
    );
    // Closing the workspace forgets it; opened again, Fetch asks again.
    g.w.commands
        .invoke(eludite_commands::workspace::SOLUTION_CLOSE, json!({}))
        .unwrap();
    g.w.wait("closed", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.git().root.is_none())
    });
    assert!(g.kept_hosts().is_empty(), "forgotten at close");
    g.w.open_solution();
    g.wait_ready();
    g.show("git_changes");
    g.w.click(changes::FETCH);
    assert_eq!(g.wait_prompt(false), host);
}

#[gpui::test]
fn the_warning_line_follows_ssl_verify_and_a_refused_certificate_names_its_host(
    cx: &mut TestAppContext,
) {
    let mut g = setup_git(cx);
    g.show("git_changes");
    let warning_shown = |g: &mut G| g.w.vcx.debug_bounds(changes::SSL_WARNING).is_some();
    assert!(!warning_shown(&mut g));
    // A self-signed https remote while sslVerify is on: refused, naming the host, with no prompt.
    with_remote(&mut g);
    if let Some(server) = git_server::GitHttp::start_tls(g.w.dir.path(), "alice", "s3cret") {
        g.repo()
            .remote_set_url("origin", &server.url("remote.git"))
            .unwrap();
        g.w.click(changes::FETCH);
        let host = server.host();
        g.wait_info("the refusal", |t| {
            t.starts_with(&format!("The certificate of {host} could not be verified"))
        });
        assert_eq!(g.prompt(), None);
        assert!(g.slot(eludite_ui::slots::STATE).contains(&host));
    }
    // http.sslVerify false in the repository's config: the warning line, until it is set back.
    g.repo()
        .config()
        .unwrap()
        .set_bool("http.sslVerify", false)
        .unwrap();
    g.wait_status("sslVerify off", |s| s.ssl_verify_off);
    g.w.vcx.run_until_parked();
    assert!(warning_shown(&mut g));
    assert!(g.w.shell.read_with(&g.w.vcx, |s, cx| {
        s.git().changes.read(cx).model.ssl_verify_off
    }));
    g.repo().config().unwrap().remove("http.sslVerify").unwrap();
    g.wait_status("sslVerify on", |s| !s.ssl_verify_off);
    g.w.vcx.run_until_parked();
    assert!(!warning_shown(&mut g));
}
