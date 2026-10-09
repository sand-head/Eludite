//! Headless tests of brief 0046 against the fixture server of `eludite-forge` (its GitHub and Tangled fixtures, served
//! on loopback; a transport that refuses anything else): the Pull Requests window lists and filters, the pull request
//! document's tabs, a review thread in the editor's margin and its reply, a review of two comments submitted as one,
//! Create Pull Request prefilled from the branch, the Git Changes link after a push, the status bar's pull request and
//! checks, the Issues window, a branch from an issue, a merge asked and refused while checks fail, the sign-in dialog's
//! device flow, an agent's write asking once per session and `deny`, a stale list and an offline one, nothing sent
//! before a window opens, a Tangled repository's capabilities, and the budgets.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::forge as cmds;
use eludite_commands::workspace;
use eludite_forge::credentials::{
    Credential, CredentialStore, Credentials, MemoryStore, Secret, SignInMethod,
};
use eludite_forge::http::{Cancel, Request, Response, Transport, UreqTransport, is_loopback};
use eludite_forge::replay::{Exchange, FixtureServer, Fixtures};
use eludite_forge::{Account, Family, ItemRef};
use eludite_git::git2;
use eludite_git::{GlobalConfig, WatchOptions};
use gpui::{Entity, TestAppContext};
use serde_json::{Value, json};

use super::forge::{
    self, ForgeSetup, PULL_SLOT, create, document, issues, margin, pull_tab, pulls, signin,
};
use super::git::changes;
use super::git::service::GitSetup;
use super::tests::assert_budget;
use super::tests::{Ws, setup_full};

/// The fake store's token: never in the cache, the audit or an output.
const TOKEN: &str = "test-token-0123456789-never-in-the-cache";

/// The real transport, refusing anything but loopback: these tests never reach a real forge.
struct LoopbackOnly(UreqTransport);

impl Transport for LoopbackOnly {
    fn send(&self, request: &Request, cancel: &Cancel) -> eludite_forge::Result<Response> {
        assert!(
            is_loopback(&request.url),
            "a shell test tried to reach {}",
            request.url
        );
        self.0.send(request, cancel)
    }
}

fn testdata(forge: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../forge/testdata")
        .join(forge)
}

/// Which fixtures, which forge, and the workspace repository's remote.
struct Spec {
    forge: &'static str,
    family: Family,
    host: &'static str,
    remote: &'static str,
    login: Option<&'static str>,
}

const GITHUB: Spec = Spec {
    forge: "github",
    family: Family::GitHub,
    host: "github.test",
    remote: "https://github.test/octo-org/hello-world.git",
    login: Some("octocat"),
};

const TANGLED: Spec = Spec {
    forge: "tangled",
    family: Family::Tangled,
    host: "tangled.org",
    remote: "https://tangled.sh/@tangled.org/core",
    login: None,
};

/// A shell over a workspace repository on `feature/login` (one commit ahead of `origin/main`), its forge on the
/// fixture server.
struct F {
    w: Ws,
    server: FixtureServer,
    bare: PathBuf,
}

fn signature() -> git2::Signature<'static> {
    git2::Signature::now("Test", "test@example.com").unwrap()
}

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

fn login_rs(changed: bool) -> String {
    (1..=30)
        .map(|i| {
            if changed && i == 14 {
                "    post(\"/session\", form);\n".to_owned()
            } else {
                format!("// line {i}\n")
            }
        })
        .collect()
}

fn forge_setup(cx: &mut TestAppContext, spec: &Spec) -> F {
    forge_setup_with(cx, spec, None)
}

fn forge_setup_with(
    cx: &mut TestAppContext,
    spec: &Spec,
    agents: Option<super::agents::AgentsSetup>,
) -> F {
    let w = setup_full(cx, |_| {}, agents);
    // Room for the right dock's windows beside the document.
    w.vcx
        .simulate_resize(gpui::size(gpui::px(1600.), gpui::px(900.)));
    w.vcx.run_until_parked();
    let root = w.dir.path().to_path_buf();
    w.shell.read_with(&w.vcx, |s, _| {
        s.git().service.set_setup(GitSetup {
            global: GlobalConfig::Files(vec![]),
            watch: WatchOptions {
                poll: Duration::from_millis(10),
                debounce: Duration::from_millis(20),
                rescan: Duration::from_millis(100),
            },
            state_dir: Some(root.join("git-state")),
        })
    });
    // The repository: main, its remote-tracking branch, and feature/login one commit ahead.
    let bare = root.join("remote.git");
    git2::Repository::init_bare(&bare).unwrap();
    let r = git2::Repository::init(&root).unwrap();
    r.set_head("refs/heads/main").unwrap();
    {
        let mut c = r.config().unwrap();
        c.set_str("user.name", "Test").unwrap();
        c.set_str("user.email", "test@example.com").unwrap();
    }
    std::fs::write(
        root.join(".gitignore"),
        "user-config/\ngit-state/\nremote.git/\n.eludite/\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/login.rs"), login_rs(false)).unwrap();
    let main = commit_all(&r, "Initial commit");
    r.remote("origin", spec.remote).unwrap();
    // A second remote on disk takes the tests' pushes; the forge is origin's.
    r.remote("disk", bare.to_str().unwrap()).unwrap();
    r.reference("refs/remotes/origin/main", main, true, "test")
        .unwrap();
    r.branch("feature/login", &r.find_commit(main).unwrap(), false)
        .unwrap();
    r.set_head("refs/heads/feature/login").unwrap();
    std::fs::write(root.join("src/login.rs"), login_rs(true)).unwrap();
    commit_all(&r, "Add the login form\n\nIt posts to /session.");

    // The forge: the fixture server, a store in memory, signed in when the spec says.
    let fixtures = Fixtures::load(&testdata(spec.forge)).expect("fixtures load");
    let server = FixtureServer::start(fixtures).unwrap();
    let store = MemoryStore::new();
    if let Some(login) = spec.login {
        store
            .set(
                spec.host,
                &Credential {
                    family: spec.family,
                    token: Secret::new(TOKEN),
                    refresh: None,
                    method: SignInMethod::Token,
                    account: Some(Account {
                        login: login.into(),
                        name: None,
                        url: None,
                    }),
                    did: None,
                    pds: None,
                    basic: false,
                },
            )
            .unwrap();
    }
    w.shell.read_with(&w.vcx, |s, _| {
        s.forge().service.set_setup(ForgeSetup {
            transport: Arc::new(LoopbackOnly(UreqTransport::default())),
            credentials: Credentials::new(Box::new(store), None),
        })
    });
    let mut f = F { w, server, bare };
    f.set_setting(
        "forge.hosts",
        json!([{"host": spec.host, "family": spec.family.as_str(), "api": f.server.base()}]),
    );
    f.w.wait("the forge hosts setting", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            !s.forge().service.hub().config().hosts.is_empty()
        })
    });
    f.w.open_solution();
    f.w.wait("the repository's status", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.git()
                .status
                .as_ref()
                .is_some_and(|st| st.branch.as_deref() == Some("feature/login"))
        })
    });
    f
}

impl F {
    /// Set a setting as the person, from another thread (the settings bus answers there).
    fn set_setting(&mut self, key: &str, value: Value) {
        let commands = self.w.commands.clone();
        let key = key.to_owned();
        let t = std::thread::spawn(move || {
            commands.invoke(
                eludite_commands::settings::SET,
                json!({"key": key, "value": value}),
            )
        });
        while !t.is_finished() {
            self.w.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        t.join().unwrap().unwrap();
    }

    /// A command from the UI (a button, the menu, a key).
    fn run(&mut self, command: &str, args: Value) {
        self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.run(command, args, window, cx)
        });
        self.w.vcx.run_until_parked();
    }

    fn show(&mut self, id: &str) {
        let _ = self
            .w
            .commands
            .invoke("eludite.view.hide", json!({ "id": "properties" }));
        self.w
            .commands
            .invoke("eludite.view.show", json!({ "id": id }))
            .unwrap();
        self.w.vcx.run_until_parked();
    }

    fn pulls(&self) -> Entity<pulls::PullsWindow> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.forge().pulls.clone())
    }

    fn issues(&self) -> Entity<issues::IssuesWindow> {
        self.w
            .shell
            .read_with(&self.w.vcx, |s, _| s.forge().issues.clone())
    }

    fn titles(&self) -> Vec<String> {
        self.pulls().read_with(&self.w.vcx, |p, _| p.titles())
    }

    fn wait_titles(&mut self, want: &[&str]) {
        let want: Vec<String> = want.iter().map(|s| (*s).to_owned()).collect();
        let pulls = self.pulls();
        self.w.wait("the pull request list", |w| {
            pulls.read_with(&w.vcx, |p, _| p.titles() == want && !p.loading)
        });
    }

    fn document(&self, tab: &str) -> Option<Entity<document::PullDocument>> {
        self.w.shell.read_with(&self.w.vcx, |s, _| {
            s.forge().pull_documents.get(tab).cloned()
        })
    }

    fn wait_document(&mut self, number: u64) -> Entity<document::PullDocument> {
        let tab = pull_tab(&number.to_string());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.w.vcx.run_until_parked();
            let (done, state) = self.w.shell.read_with(&self.w.vcx, |s, cx| {
                let d = s.forge().pull_documents.get(&tab).map(|d| d.read(cx));
                (
                    d.is_some_and(|d| d.pull.is_some()),
                    format!(
                        "open: {}, message: {:?}, status: {:?}",
                        d.is_some(),
                        d.and_then(|d| d.message.clone()),
                        s.status().get(eludite_ui::slots::STATE)
                    ),
                )
            });
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the pull request document: {state}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        self.document(&tab).unwrap()
    }

    fn wait_message(&mut self, doc: &Entity<document::PullDocument>, what: &str, has: &str) {
        let doc = doc.clone();
        let has = has.to_owned();
        self.w.wait(what, |w| {
            doc.read_with(&w.vcx, |d, _| {
                d.message.as_ref().is_some_and(|(m, _)| m.contains(&has))
            })
        });
    }

    fn status(&self, slot: &str) -> String {
        self.w.shell.read_with(&self.w.vcx, |s, _| {
            s.status().get(slot).unwrap_or_default().to_owned()
        })
    }

    fn seen(&self, method: &str, path_end: &str) -> Vec<eludite_forge::replay::Seen> {
        self.server
            .fixtures
            .seen()
            .into_iter()
            .filter(|x| {
                x.method == method && x.target.split('?').next().unwrap().ends_with(path_end)
            })
            .collect()
    }

    fn open_file(&mut self, rel: &str) -> PathBuf {
        let path = self.w.path(rel);
        self.run(
            workspace::FILE_OPEN,
            json!({"path": path.to_string_lossy()}),
        );
        let p = path.clone();
        self.w.wait("the editor", |w| {
            w.shell.read_with(&w.vcx, |s, _| s.editor(&p).is_some())
        });
        path
    }

    fn marks(&self, path: &Path) -> Option<Entity<margin::ThreadMarks>> {
        let id = super::documents::normalize_path(path)
            .to_string_lossy()
            .into_owned();
        self.w.shell.read_with(&self.w.vcx, |s, _| {
            s.forge().margins.borrow().get(&id).cloned()
        })
    }

    fn repository(&self) -> eludite_forge::Repository {
        self.w.shell.read_with(&self.w.vcx, |s, _| {
            forge::detect_repository(&s.forge().service).unwrap()
        })
    }
}

/// The p99 of `frames`.
fn p99(mut frames: Vec<Duration>) -> Duration {
    frames.sort();
    frames[(frames.len() * 99).div_ceil(100) - 1]
}

#[gpui::test]
fn nothing_reaches_the_forge_before_a_window_opens_then_the_list_filters_and_a_pull_request_opens_with_its_tabs(
    cx: &mut TestAppContext,
) {
    let mut f = forge_setup(cx, &GITHUB);
    f.w.vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(100));
    f.w.vcx.run_until_parked();
    assert_eq!(
        f.server.fixtures.count(),
        0,
        "nothing reached the forge: not at startup, not with the workspace open"
    );
    assert_eq!(
        f.status(PULL_SLOT),
        "",
        "the status bar reads the cache only"
    );

    f.show("pull_requests");
    f.wait_titles(&[
        "Add the login form",
        "Fix the typo in the README",
        "Draft: rework the session store",
    ]);
    assert!(f.server.fixtures.count() > 0);
    // The repository's forge, as detected.
    let detected = f
        .pulls()
        .read_with(&f.w.vcx, |p, _| p.detected.clone())
        .unwrap();
    assert_eq!(detected.family, Family::GitHub);
    assert_eq!(detected.account.as_deref(), Some("octocat"));

    // Mine: the two of octocat.
    f.w.click(&pulls::filter_selector("mine"));
    f.wait_titles(&["Add the login form", "Fix the typo in the README"]);
    // The search box.
    let pulls = f.pulls();
    pulls.update(&mut f.w.vcx, |p, cx| p.search("typo", cx));
    f.wait_titles(&["Fix the typo in the README"]);
    pulls.update(&mut f.w.vcx, |p, cx| p.search("", cx));
    f.w.click(&pulls::filter_selector("all"));
    f.wait_titles(&[
        "Add the login form",
        "Fix the typo in the README",
        "Draft: rework the session store",
    ]);

    // A double-click opens the document.
    f.w.double_click(&pulls::row_selector(0));
    let doc = f.wait_document(12);
    assert_eq!(
        f.w.controller.active_document().as_deref(),
        Some(pull_tab("12").as_str())
    );
    doc.read_with(&f.w.vcx, |d, _| {
        let p = d.pull.as_ref().unwrap();
        assert_eq!(p.summary.title, "Add the login form");
        assert_eq!(p.commits.len(), 2);
        assert_eq!(p.files.len(), 3);
        assert_eq!(p.threads.len(), 2);
        assert_eq!(p.check_items.len(), 3);
    });
    for (tab, row) in [
        (document::Tab::Commits, "forge-pr-commit-1"),
        (document::Tab::Files, "forge-pr-file-2"),
        (document::Tab::Threads, "forge-pr-thread-1"),
        (document::Tab::Checks, "forge-pr-check-2"),
    ] {
        f.w.click(&document::tab_selector(tab));
        assert!(
            f.w.vcx
                .debug_bounds(Box::leak(row.to_owned().into_boxed_str()))
                .is_some(),
            "{row} is drawn on {tab:?}"
        );
    }
    // The checks tab's log opens as a document.
    f.w.click(&document::check_log_selector(doc.read_with(
        &f.w.vcx,
        |d, _| {
            d.pull
                .as_ref()
                .unwrap()
                .check_items
                .iter()
                .position(|c| c.name == "test")
                .unwrap()
        },
    )));
    f.w.wait("the check's log", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.forge().documents.borrow().iter().any(|(id, v)| {
                id.starts_with(forge::LOG_PREFIX)
                    && v.clone()
                        .downcast::<super::forge::log::CheckLog>()
                        .is_ok_and(|l| {
                            let l = l.read(cx);
                            l.id == "run:5002"
                                && l.lines.iter().any(|x| x.contains("exit code 101"))
                        })
            })
        })
    });
}

#[gpui::test]
fn the_status_bar_shows_the_branchs_pull_request_and_checks_and_opens_it(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    f.show("pull_requests");
    f.wait_titles(&[
        "Add the login form",
        "Fix the typo in the README",
        "Draft: rework the session store",
    ]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        f.w.vcx.run_until_parked();
        let (slot, info) = f.w.shell.read_with(&f.w.vcx, |s, cx| {
            (
                s.status().get(PULL_SLOT).unwrap_or_default().to_owned(),
                format!(
                    "branch {:?}, items {:?}",
                    s.git().status.as_ref().and_then(|st| st.branch.clone()),
                    s.forge()
                        .pulls
                        .read(cx)
                        .items
                        .iter()
                        .map(|p| (p["head"].clone(), p["checks"].clone()))
                        .collect::<Vec<_>>()
                ),
            )
        });
        if slot.starts_with("\u{21C4} #12 \u{2717} ") && slot.ends_with("/3") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the status bar's pull request: {slot:?} {info}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    f.w.click(&eludite_ui::status::slot_selector(PULL_SLOT));
    let doc = f.wait_document(12);
    assert_eq!(
        f.w.controller.active_document().as_deref(),
        Some(pull_tab("12").as_str())
    );
    // A comment on the conversation from the document.
    doc.update(&mut f.w.vcx, |d, cx| d.type_comment("Looks good", cx));
    f.w.click(document::COMMENT);
    f.wait_message(&doc, "the comment", "done");
    let posted = f.seen("POST", "/issues/12/comments");
    assert_eq!(posted.len(), 1);
    assert!(posted[0].body.contains("Looks good"));
    assert!(
        doc.read_with(&f.w.vcx, |d, _| d.comment.is_empty()),
        "the box clears"
    );
}

#[gpui::test]
fn the_sign_in_dialog_takes_a_token_and_keeps_it_out_of_the_dialog(cx: &mut TestAppContext) {
    let mut f = forge_setup(
        cx,
        &Spec {
            login: None,
            ..GITHUB
        },
    );
    f.run(
        cmds::AUTH,
        json!({"action": "sign_in", "host": "github.test"}),
    );
    let dialog =
        f.w.shell
            .read_with(&f.w.vcx, |s, _| s.forge().signin.clone())
            .expect("Git > Sign in to Forge... opens the dialog");
    f.w.click(&signin::method_selector("token"));
    dialog.update(&mut f.w.vcx, |d, cx| d.type_token("ghp_pasted", None, cx));
    assert!(
        f.w.vcx.debug_bounds(signin::TOKEN_BOX).is_some(),
        "the token box is drawn (as bullets)"
    );
    f.w.click(signin::SIGN_IN);
    f.w.wait("signed in", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.forge().signin.is_none())
    });
    let checked = f.seen("GET", "/user");
    assert_eq!(
        checked.last().unwrap().authorization.as_deref(),
        Some("Bearer ghp_pasted")
    );
    let status =
        f.w.commands
            .invoke(cmds::AUTH, json!({"action": "status"}))
            .unwrap();
    assert_eq!(status["signed_in"], true);
    assert_eq!(status["account"]["login"], "octocat");
    let audit = serde_json::to_string(
        &f.w.commands
            .audit_log()
            .entries()
            .into_iter()
            .map(|e| e.arguments)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(
        !audit.contains("ghp_pasted"),
        "the token is not audited: {audit}"
    );
}

#[gpui::test]
fn a_thread_shows_in_the_margin_on_the_checked_out_branch_and_a_reply_posts(
    cx: &mut TestAppContext,
) {
    let mut f = forge_setup(cx, &GITHUB);
    f.run(cmds::PULL, json!({"number": 12}));
    f.wait_document(12);
    // feature/login is the pull request's head: its threads mark the open editors.
    let path = f.open_file("src/login.rs");
    f.w.wait("the thread's mark", |w| {
        let id = super::documents::normalize_path(&path)
            .to_string_lossy()
            .into_owned();
        w.shell.read_with(&w.vcx, |s, cx| {
            s.forge()
                .margins
                .borrow()
                .get(&id)
                .is_some_and(|m| m.read(cx).marks.len() == 1)
        })
    });
    let marks = f.marks(&path).unwrap();
    marks.read_with(&f.w.vcx, |m, _| {
        assert_eq!(m.marks[0].0, 14);
        assert_eq!(m.marks[0].1.comments.len(), 2);
        assert_eq!(
            m.target.as_ref().map(|t| t.1.as_str()),
            Some("src/login.rs")
        );
    });
    f.w.click(&margin::mark_selector(0));
    assert!(
        f.w.vcx.debug_bounds(margin::POPOVER).is_some(),
        "the thread opens"
    );
    marks.update(&mut f.w.vcx, |m, cx| m.type_reply("Done", cx));
    f.w.click(margin::REPLY);
    f.w.wait("the reply", |_| true);
    let doc = f.document(&pull_tab("12")).unwrap();
    f.wait_message(&doc, "the reply's answer", "done");
    let replies = f.seen("POST", "/pulls/12/comments/100/replies");
    assert_eq!(replies.len(), 1, "the reply posted to its thread");
    assert!(replies[0].body.contains("Done"), "{}", replies[0].body);
    assert_eq!(
        replies[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[gpui::test]
fn a_review_collects_two_comments_and_submits_them_as_one(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    f.run(cmds::PULL, json!({"number": 12}));
    let doc = f.wait_document(12);
    f.w.click(document::START_REVIEW);
    f.wait_message(&doc, "the review started", "pending review is open");
    let before = f.server.fixtures.count();
    let path = f.open_file("src/login.rs");
    let marks = f.marks(&path).unwrap();
    // A reply to the thread, into the review.
    f.w.click(&margin::mark_selector(0));
    marks.update(&mut f.w.vcx, |m, cx| m.type_reply("First", cx));
    f.w.click(margin::ADD_TO_REVIEW);
    f.w.wait("one pending comment", |w| {
        doc.read_with(&w.vcx, |d, _| d.pending == 1)
    });
    // A new comment on the caret's line, into the review.
    f.w.click(margin::NEW_COMMENT);
    marks.read_with(&f.w.vcx, |m, _| {
        assert!(matches!(m.open, Some(margin::Open::New(_))), "{:?}", m.open)
    });
    marks.update(&mut f.w.vcx, |m, cx| m.type_reply("Second", cx));
    f.w.click(margin::ADD_TO_REVIEW);
    f.w.wait("two pending comments", |w| {
        doc.read_with(&w.vcx, |d, _| d.pending == 2)
    });
    // (GraphQL reads threads by POST.)
    let posted = f
        .server
        .fixtures
        .seen()
        .into_iter()
        .skip(before)
        .filter(|x| x.method != "GET" && x.target != "/graphql")
        .count();
    assert_eq!(posted, 0, "pending comments send nothing");
    // Submit: one call with both.
    let tab = pull_tab("12");
    f.w.controller.open_document(&tab, "Pull request #12");
    f.w.vcx.run_until_parked();
    f.w.click(&document::submit_selector("approve"));
    f.wait_message(
        &doc,
        "the review submitted",
        "review submitted with 2 comment(s)",
    );
    let reviews = f.seen("POST", "/pulls/12/reviews");
    assert_eq!(reviews.len(), 1, "one call");
    assert!(
        reviews[0].body.contains("First")
            && reviews[0].body.contains("Second")
            && reviews[0].body.contains("\"path\":\"src/login.rs\""),
        "{}",
        reviews[0].body
    );
    f.w.wait("the review's count cleared", |w| {
        doc.read_with(&w.vcx, |d, _| d.pending == 0)
    });
}

#[gpui::test]
fn create_pull_request_after_a_push_prefills_from_the_branch_and_creates(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    f.show("git_changes");
    let link = |f: &F| {
        f.w.shell.read_with(&f.w.vcx, |s, cx| {
            s.git().changes.read(cx).pull_request_link()
        })
    };
    assert!(!link(&f), "no link before a push");
    // Push (to the bare repository on disk): the link appears.
    f.run("eludite.git.push", json!({"remote": "disk"}));
    f.w.wait("the push and the link", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.git().changes.read(cx).pull_request_link())
    });
    assert!(
        git2::Repository::open_bare(&f.bare)
            .unwrap()
            .find_reference("refs/heads/feature/login")
            .is_ok()
    );
    assert_eq!(
        f.server.fixtures.count(),
        0,
        "a push reads nothing from the forge"
    );
    f.w.click(changes::PULL_REQUEST_LINK);
    let form =
        f.w.shell
            .read_with(&f.w.vcx, |s, _| s.forge().create.clone())
            .unwrap();
    f.w.wait("the form prefilled", |w| {
        form.read_with(&w.vcx, |c, _| c.prefilled)
    });
    form.read_with(&f.w.vcx, |c, _| {
        assert_eq!(c.head.as_deref(), Some("feature/login"));
        assert_eq!(c.base, "main");
        assert_eq!(c.title, "Add the login form", "the single commit's title");
        assert_eq!(c.body, "It posts to /session.", "the commit's body");
        assert!(c.capabilities.pull_create && c.capabilities.draft_pull_requests);
    });
    assert_eq!(
        f.w.controller.active_document().as_deref(),
        Some(forge::CREATE_TAB)
    );
    serve_pull_14(&f);
    f.w.click(create::CREATE);
    let doc = f.wait_document(14);
    doc.read_with(&f.w.vcx, |d, _| assert_eq!(d.item, ItemRef::Number(14)));
    let created = f.seen("POST", "/repos/octo-org/hello-world/pulls");
    assert_eq!(created.len(), 1);
    assert!(
        created[0].body.contains("\"base\":\"main\""),
        "{}",
        created[0].body
    );
    assert!(
        f.w.shell
            .read_with(&f.w.vcx, |s, _| s.forge().create.is_none()),
        "the form closes"
    );
    assert!(!link(&f), "the link goes once the pull request exists");
}

#[gpui::test]
fn the_issues_window_lists_creates_comments_and_makes_a_branch(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    f.show("issues");
    let w = f.issues();
    f.w.wait("the issue list", |x| {
        w.read_with(&x.vcx, |i, _| {
            i.titles()
                == [
                    "Login accepts an empty password",
                    "Document the session store",
                ]
        })
    });
    // Select one: it loads, and a comment posts on it.
    f.w.click(&issues::row_selector(0));
    f.w.wait("the issue", |x| {
        w.read_with(&x.vcx, |i, _| {
            i.issue.as_ref().is_some_and(|v| v["milestone"] == "v1.0")
        })
    });
    w.update(&mut f.w.vcx, |i, cx| {
        i.type_into(issues::Field::Comment, "Taking this.", cx)
    });
    f.w.click(issues::COMMENT);
    f.w.wait("the comment", |x| {
        w.read_with(&x.vcx, |i, _| {
            i.message
                .as_ref()
                .is_some_and(|(m, failed)| !failed && m.contains("done"))
        })
    });
    assert_eq!(f.seen("POST", "/issues/42/comments").len(), 1);
    // Create.
    w.update(&mut f.w.vcx, |i, cx| {
        i.type_into(issues::Field::NewTitle, "Rate-limit the login endpoint", cx)
    });
    f.w.click(issues::NEW_CREATE);
    f.w.wait("the issue created", |x| {
        w.read_with(&x.vcx, |i, _| {
            i.message
                .as_ref()
                .is_some_and(|(m, failed)| !failed && m.starts_with("Created "))
        })
    });
    // A branch from the issue: created in the repository, checked out, and linked on the forge.
    f.w.click(&issues::row_selector(0));
    f.w.click(issues::BRANCH);
    f.w.wait("the branch", |x| {
        x.shell.read_with(&x.vcx, |s, _| {
            s.git().status.as_ref().is_some_and(|st| {
                st.branch.as_deref() == Some("issue/42-login-accepts-an-empty-password")
            })
        })
    });
    assert!(
        f.seen("POST", "/graphql")
            .iter()
            .any(|x| x.body.contains("createLinkedBranch")),
        "linked to the issue"
    );
}

#[gpui::test]
fn a_merge_asks_first_and_is_refused_while_checks_fail(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    f.run(cmds::PULL, json!({"number": 12}));
    let doc = f.wait_document(12);
    f.w.click(&document::method_selector("squash"));
    f.w.click(document::MERGE);
    assert!(f.w.vcx.has_pending_prompt(), "a merge asks");
    // No: nothing is sent.
    f.w.vcx.simulate_prompt_answer("No");
    f.w.vcx.run_until_parked();
    assert!(f.seen("PUT", "/pulls/12/merge").is_empty());
    // Yes: refused, the checks are failing.
    f.w.click(document::MERGE);
    f.w.vcx.simulate_prompt_answer("Yes");
    f.wait_message(&doc, "the refusal", "checks are failing");
    assert!(
        f.seen("PUT", "/pulls/12/merge").is_empty(),
        "nothing reached the forge's merge"
    );
}

#[gpui::test]
fn the_sign_in_dialogs_device_flow_shows_the_code_and_completes(cx: &mut TestAppContext) {
    let mut f = forge_setup(
        cx,
        &Spec {
            login: None,
            ..GITHUB
        },
    );
    f.set_setting("forge.githubClientId", json!("Iv1.test"));
    f.w.wait("the client id", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.forge().service.hub().config().github_client_id.is_some()
        })
    });
    // Git > Sign in to Forge...: the windows know the host once one opened.
    f.show("pull_requests");
    f.w.wait("not signed in", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.forge()
                .pulls
                .read(cx)
                .detected
                .as_ref()
                .is_some_and(|d| !d.signed_in)
        })
    });
    f.w.click(pulls::SIGN_IN);
    let dialog =
        f.w.shell
            .read_with(&f.w.vcx, |s, _| s.forge().signin.clone())
            .expect("the dialog");
    dialog.read_with(&f.w.vcx, |d, _| {
        assert_eq!(d.host, "github.test");
        assert_eq!(d.method, "device");
        assert_eq!(
            signin::methods(Family::GitHub)
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            ["device", "token", "cli"]
        );
    });
    f.w.click(signin::SIGN_IN);
    f.w.wait("the device code", |w| {
        dialog.read_with(&w.vcx, |d, _| d.device.is_some())
    });
    dialog.read_with(&f.w.vcx, |d, _| {
        let (code, url) = d.device.clone().unwrap();
        assert_eq!(code, "WDJB-MJHT");
        assert_eq!(url, "https://github.com/login/device");
    });
    assert!(f.w.vcx.debug_bounds(signin::CODE).is_some());
    // The fixture answers pending once, then the token: the dialog closes, the windows read signed in.
    f.w.wait("signed in", |w| {
        w.shell.read_with(&w.vcx, |s, _| s.forge().signin.is_none())
    });
    f.w.wait("the windows signed in", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.forge()
                .pulls
                .read(cx)
                .detected
                .as_ref()
                .is_some_and(|d| d.signed_in && d.account.as_deref() == Some("octocat"))
        })
    });
    let out =
        f.w.commands
            .invoke(cmds::AUTH, json!({"action": "status"}))
            .unwrap();
    assert_eq!(out["method"], "device");
    assert!(!out.to_string().contains("gho_devicetoken"));
}

#[gpui::test]
fn an_agents_write_asks_once_per_session_and_deny_refuses(cx: &mut TestAppContext) {
    let steps = json!([
        {"tool": "eludite-forge-pull_comment", "arguments": {"number": 12, "body": "Fixed in the next push."}},
        {"tool": "eludite-forge-pull_comment", "arguments": {"number": 12, "body": "Done", "reply_to": "100"}},
        {"tool": "eludite-forge-pull", "arguments": {"number": 12, "threads": "unresolved"}}
    ]);
    let agents = super::agents::tests::fake_agents(vec![(
        "Scripted".into(),
        vec![
            "--scenario".into(),
            "script".into(),
            "--script".into(),
            steps.to_string(),
        ],
    )]);
    let mut f = forge_setup_with(cx, &GITHUB, Some(agents));
    let policy = eludite_commands::policy::AgentPolicy::path_for(f.w.dir.path());
    std::fs::create_dir_all(policy.parent().unwrap()).unwrap();
    // deny: the write is refused, naming the policy; nothing is posted.
    std::fs::write(&policy, r#"{"version": 1, "forge": {"write": "deny"}}"#).unwrap();
    run_agent(&mut f);
    wait_turn(&mut f);
    let rows = transcript(&f);
    let first = step(&rows, 1);
    assert_eq!(first["status"], "failed", "{first}");
    assert!(first.to_string().contains("forge.write"), "{first}");
    assert!(f.seen("POST", "/issues/12/comments").is_empty());
    // prompt (the default): the first write asks, "Allow for this session" lets the second run without asking.
    std::fs::write(&policy, r#"{"version": 1}"#).unwrap();
    run_agent(&mut f);
    f.w.wait("the permission prompt", |w| {
        w.shell
            .read_with(&w.vcx, |s, cx| s.agents().window.read(cx).prompt.is_some())
    });
    let p =
        f.w.shell
            .read_with(&f.w.vcx, |s, cx| s.agents().window.read(cx).prompt.clone())
            .unwrap();
    assert!(p.session, "{p:?}");
    assert!(
        p.reason
            .as_deref()
            .unwrap_or_default()
            .contains("forge.write: prompt"),
        "{p:?}"
    );
    f.w.shell
        .update(&mut f.w.vcx, |s, cx| {
            s.agents_answer(
                p.request,
                crate::shell::agents::window::Decision::AlwaysAllow,
                cx,
            )
        })
        .unwrap();
    wait_turn(&mut f);
    let rows = transcript(&f);
    for n in 1..=3 {
        assert_eq!(
            step(&rows, n)["status"],
            "completed",
            "step {n}: {}",
            step(&rows, n)
        );
    }
    assert!(
        !step(&rows, 2)["note"]
            .as_str()
            .unwrap_or_default()
            .contains("Allowed by you"),
        "asked once"
    );
    assert_eq!(f.seen("POST", "/issues/12/comments").len(), 1);
    assert_eq!(f.seen("POST", "/pulls/12/comments/100/replies").len(), 1);
    // The audit keeps no body and no token.
    let audit = serde_json::to_string(
        &f.w.commands
            .audit_log()
            .entries()
            .into_iter()
            .filter(|e| e.command == cmds::PULL_COMMENT)
            .map(|e| e.arguments)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!audit.contains("Fixed in the next push."), "{audit}");
    assert!(audit.contains("body_length"), "{audit}");
    assert!(!audit.contains(TOKEN));
}

fn run_agent(f: &mut F) {
    f.w.shell
        .update(&mut f.w.vcx, |s, cx| {
            s.agents.last_stop = None;
            s.agents_start(Some("Scripted"), true, cx)?;
            s.agents_prompt("go", cx)
        })
        .unwrap();
}

fn wait_turn(f: &mut F) {
    f.w.wait("the turn's end", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.agents().last_stop.is_some()
                && s.agents().state != crate::shell::agents::window::StateKind::Running
        })
    });
}

fn transcript(f: &F) -> Value {
    f.w.shell.read_with(&f.w.vcx, |s, cx| {
        s.agents().window.read(cx).transcript.to_json()
    })
}

fn step(rows: &Value, n: usize) -> Value {
    let id = format!("toolu_fake_step_{n}");
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["tool_call"]["id"] == id)
        .map(|r| r["tool_call"].clone())
        .unwrap_or(Value::Null)
}

/// The created pull request #14 as the forge then answers it: #13's fixtures, renumbered, on feature/login.
fn serve_pull_14(f: &F) {
    let dir = testdata("github").join("synthesized");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    for p in names {
        let text = std::fs::read_to_string(&p).unwrap();
        let Ok(mut x) = serde_json::from_str::<Exchange>(&text) else {
            continue;
        };
        let ours = x.request.method == "GET"
            && (x.request.path.ends_with("/pulls/13")
                || x.request.path.contains("/pulls/13/")
                || x.request.path.contains("/issues/13/"));
        if !ours {
            continue;
        }
        x.request.path = x.request.path.replace("/13", "/14");
        let body = serde_json::to_string(&x.response.body)
            .unwrap()
            .replace("\"number\":13", "\"number\":14")
            .replace("\"ref\":\"fix/typo\"", "\"ref\":\"feature/login\"");
        x.response.body = serde_json::from_str(&body).unwrap();
        f.server.fixtures.push_front(x);
    }
}

/// The (titles, banner) states a window drew, in order.
type Drawn = std::rc::Rc<std::cell::RefCell<Vec<(Vec<String>, Option<String>)>>>;

/// An hour-old cached list, as a closed IDE leaves it.
fn put_old_list(f: &F) {
    let repo = f.repository();
    let key = eludite_forge::ops::pulls_key(Default::default(), Default::default(), None, 50, None);
    let hub =
        f.w.shell
            .read_with(&f.w.vcx, |s, _| s.forge().service.hub());
    hub.cache().unwrap().put_at(
        &repo.cache_key(),
        &key,
        &json!({"items": [{"id": "12", "number": 12, "title": "Old title", "state": "open", "author": "octocat", "head": "feature/login", "base": "main", "url": "u"}]}),
        eludite_forge::util::now_secs() - 3600,
    );
}

#[gpui::test]
fn a_stale_list_shows_the_cache_with_its_age_then_updates(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    put_old_list(&f);
    // Every state the window draws, in order (the refresh may land within one run of the test's executor).
    let pulls = f.pulls();
    let seen: Drawn = Default::default();
    let record = seen.clone();
    f.w.vcx.update(|_, cx| {
        cx.observe(&pulls, move |p, cx| {
            let p = p.read(cx);
            let state = (p.titles(), p.provenance.banner(false));
            let mut r = record.borrow_mut();
            if r.last() != Some(&state) {
                r.push(state);
            }
        })
        .detach()
    });
    f.show("pull_requests");
    f.w.wait("the cached list", |_| {
        seen.borrow()
            .iter()
            .any(|(t, _)| t == &["Old title".to_owned()])
    });
    let banner = seen
        .borrow()
        .iter()
        .find(|(t, _)| t == &["Old title".to_owned()])
        .and_then(|(_, b)| b.clone());
    assert!(
        banner.as_deref().is_some_and(
            |b| b.starts_with("Showing cached results from ") && b.contains("1 hour ago")
        ),
        "{banner:?}"
    );
    // The refresh lands: the list updates and the banner goes.
    f.wait_titles(&[
        "Add the login form",
        "Fix the typo in the README",
        "Draft: rework the session store",
    ]);
    assert_eq!(
        pulls.read_with(&f.w.vcx, |p, _| p.provenance.banner(false)),
        None
    );
    // And it does not refresh again by itself.
    let sent = f.server.fixtures.count();
    std::thread::sleep(Duration::from_millis(200));
    f.w.vcx.run_until_parked();
    assert_eq!(f.server.fixtures.count(), sent);
}

#[gpui::test]
fn offline_the_window_shows_the_cache_and_the_reason(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    put_old_list(&f);
    f.server.fixtures.push_front(Exchange::json(
        "GET",
        "/repos/octo-org/hello-world/pulls",
        503,
        json!({"message": "Service unavailable"}),
    ));
    f.show("pull_requests");
    let pulls = f.pulls();
    f.w.wait("the reason", |w| {
        pulls.read_with(&w.vcx, |p, _| {
            p.provenance
                .error
                .as_deref()
                .is_some_and(|e| e.contains("503"))
        })
    });
    assert_eq!(f.titles(), ["Old title"], "the cache stays");
    let banner = pulls
        .read_with(&f.w.vcx, |p, _| p.provenance.banner(false))
        .unwrap();
    assert!(
        banner.contains("1 hour ago") && banner.contains("503"),
        "{banner}"
    );
    assert!(f.w.vcx.debug_bounds(pulls::BANNER).is_some());
}

#[gpui::test]
fn a_tangled_repository_shows_only_what_its_fixtures_support(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &TANGLED);
    // The recorded list asked for 3; the window asks for 50 (one page).
    let text = std::fs::read_to_string(
        testdata("tangled").join("recorded/003-get-xrpc-sh_tangled_repo_listPulls.json"),
    )
    .unwrap();
    let mut x: Exchange = serde_json::from_str(&text).unwrap();
    x.request.query.insert("limit".into(), "50".into());
    if let Some(o) = x.response.body.as_object_mut() {
        o.remove("cursor");
    }
    f.server.fixtures.push_front(x);
    f.show("pull_requests");
    let pulls = f.pulls();
    f.w.wait("the Tangled list", |w| {
        pulls.read_with(&w.vcx, |p, _| p.items.len() == 3 && !p.loading)
    });
    let (d, first) = pulls.read_with(&f.w.vcx, |p, _| {
        (p.detected.clone().unwrap(), p.items[0].clone())
    });
    assert_eq!(d.family, Family::Tangled);
    assert!(first.get("number").is_none(), "no numbers on Tangled");
    let c = &d.capabilities;
    assert!(c.pull_requests && c.issues);
    assert!(
        !c.pull_create
            && !c.reviews
            && !c.review_threads
            && !c.checks
            && c.merge_methods.is_empty()
    );
    // Its pull request: no review bar, no threads or checks tab, no merge.
    let id = "at://did:plc:xasnlahkri4ewmbuzly2rlc5/sh.tangled.repo.pull/3mwqc5pt6dc5d";
    f.run(cmds::PULL, json!({"id": id}));
    let tab = pull_tab(id);
    f.w.wait("the Tangled pull request", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.forge()
                .pull_documents
                .get(&tab)
                .is_some_and(|d| d.read(cx).pull.is_some())
        })
    });
    for absent in [
        document::START_REVIEW,
        document::MERGE,
        "forge-pr-tab-threads",
        "forge-pr-tab-checks",
    ] {
        assert!(
            f.w.vcx.debug_bounds(absent).is_none(),
            "{absent} is not offered on Tangled"
        );
    }
    assert!(f.w.vcx.debug_bounds("forge-pr-tab-files").is_some());
    // Create Pull Request says why not.
    f.run(cmds::PULL_CREATE, json!({}));
    let form =
        f.w.shell
            .read_with(&f.w.vcx, |s, _| s.forge().create.clone())
            .unwrap();
    f.w.wait("the form", |w| form.read_with(&w.vcx, |c, _| c.prefilled));
    assert!(
        form.read_with(&f.w.vcx, |c, _| c.message.clone())
            .unwrap()
            .contains("does not support creating pull requests")
    );
}

/// Budgets: a cached list within 50 ms of asking; the document of 100 files and 300 threads from the cache within
/// 200 ms, frame p99 under 8 ms while its lists scroll; both windows over a 500-pull-request cache under 40 MB more.
#[gpui::test]
fn budgets_of_the_cached_list_the_large_document_and_memory(cx: &mut TestAppContext) {
    let mut f = forge_setup(cx, &GITHUB);
    f.show("pull_requests");
    f.wait_titles(&[
        "Add the login form",
        "Fix the typo in the README",
        "Draft: rework the session store",
    ]);
    // The cached list (fresh: no refresh) from the window asking to drawn.
    let pulls = f.pulls();
    let mut opens = Vec::new();
    for _ in 0..5 {
        pulls.update(&mut f.w.vcx, |p, cx| {
            p.items.clear();
            cx.emit(pulls::PullsEvent::Load);
        });
        let t = Instant::now();
        f.w.wait("the cached list", |w| {
            pulls.read_with(&w.vcx, |p, _| p.items.len() == 3)
        });
        opens.push(t.elapsed());
    }
    let open = p99(opens);
    eprintln!(
        "timing: the cached list drawn {:.2} ms after asking",
        open.as_secs_f64() * 1e3
    );
    assert_budget("the cached list", open, Duration::from_millis(50));

    // A pull request of 100 files and 300 threads, from the cache.
    f.run(cmds::PULL, json!({"number": 12}));
    f.wait_document(12);
    let repo = f.repository();
    let hub =
        f.w.shell
            .read_with(&f.w.vcx, |s, _| s.forge().service.hub());
    let key = eludite_forge::ops::pull_key(&ItemRef::Number(12));
    let mut big: Value = hub.cached::<Value>(&repo, &key).unwrap().value;
    let file = big["files"][0].clone();
    big["files"] = Value::Array(
        (0..100)
            .map(|i| {
                let mut x = file.clone();
                x["path"] = json!(format!("src/file_{i:03}.rs"));
                x
            })
            .collect(),
    );
    let thread = big["threads"][0].clone();
    big["threads"] = Value::Array(
        (0..300)
            .map(|i| {
                let mut x = thread.clone();
                x["id"] = json!(format!("{}", 10_000 + i));
                x["path"] = json!(format!("src/file_{:03}.rs", i % 100));
                x
            })
            .collect(),
    );
    hub.store(&repo, &key, &big);
    let tab = pull_tab("12");
    f.w.vcx.update(|_, cx| {
        f.w.shell.update(cx, |s, _| {
            s.controller.close_document(&tab);
            s.forge_document_closed(&tab);
        })
    });
    f.w.vcx.run_until_parked();
    let sent = f.server.fixtures.count();
    let t = Instant::now();
    f.run(cmds::PULL, json!({"number": 12}));
    f.w.wait("the large document", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.forge()
                .pull_documents
                .get(&tab)
                .and_then(|d| {
                    d.read(cx)
                        .pull
                        .as_ref()
                        .map(|p| p.files.len() == 100 && p.threads.len() == 300)
                })
                .unwrap_or(false)
        })
    });
    let took = t.elapsed();
    eprintln!(
        "timing: the 100-file, 300-thread document from the cache in {:.2} ms",
        took.as_secs_f64() * 1e3
    );
    assert_budget(
        "the large document from the cache",
        took,
        Duration::from_millis(200),
    );
    assert_eq!(f.server.fixtures.count(), sent, "read from the cache");
    let doc = f.document(&tab).unwrap();
    // The best of three passes: other tests share the machine, and noise only lengthens frames.
    let mut passes = Vec::new();
    for _ in 0..3 {
        let mut frames = Vec::new();
        for (tab_kind, rows) in [
            (document::Tab::Files, 100usize),
            (document::Tab::Threads, 300),
        ] {
            doc.update(&mut f.w.vcx, |d, cx| d.select_tab(tab_kind, cx));
            for i in 0..40 {
                doc.update(&mut f.w.vcx, |d, cx| d.scroll_to((i * 37) % rows, cx));
                let took = f.w.vcx.update(|window, cx| {
                    window.refresh();
                    let t = Instant::now();
                    let _ = window.draw(cx);
                    t.elapsed()
                });
                frames.push(took);
            }
        }
        passes.push(p99(frames));
    }
    let frame = passes.into_iter().min().unwrap();
    eprintln!(
        "timing: scrolling the large document, frame p99 {:.2} ms",
        frame.as_secs_f64() * 1e3
    );
    assert_budget(
        "the large document's frame p99",
        frame,
        Duration::from_millis(8),
    );

    // Memory: both windows open over a 500-pull-request cache.
    let rss = || -> Option<u64> {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = s.lines().find(|l| l.starts_with("VmRSS:"))?;
        line.split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|k| k * 1024)
    };
    let key = eludite_forge::ops::pulls_key(Default::default(), Default::default(), None, 50, None);
    let summary = |i: u64| {
        json!({"id": i.to_string(), "number": i, "title": format!("Pull request {i}: a title of a usual length"), "state": "open",
               "author": "octocat", "head": format!("feature/branch-{i}"), "base": "main",
               "url": format!("https://github.test/octo-org/hello-world/pull/{i}"), "updated_at": "2026-10-04T05:00:00Z",
               "labels": ["enhancement"], "checks": "success"})
    };
    hub.store(
        &repo,
        &key,
        &json!({"items": (1..=500).map(summary).collect::<Vec<_>>()}),
    );
    if let Some(before) = rss() {
        f.show("issues");
        pulls.update(&mut f.w.vcx, |p, cx| {
            p.items.clear();
            cx.emit(pulls::PullsEvent::Load);
        });
        f.w.wait("the 500 pull requests", |w| {
            pulls.read_with(&w.vcx, |p, _| p.items.len() == 500)
        });
        f.show("pull_requests");
        f.w.vcx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        eludite_test_support::assert_memory_budget(
            "memory growth of both windows over a 500-pull-request cache",
            rss().unwrap().saturating_sub(before),
            40_000_000,
        );
    }
}
