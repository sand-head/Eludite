//! Fetch, pull and push against a bare repository on disk (no network), with the ahead and behind counts.

use std::path::Path;

use eludite_git::branches::MergeOutcome;
use eludite_git::commit::CommitOptions;
use eludite_git::{Cancel, ErrorKind, GlobalConfig, Repo, git2};

fn clone_of(bare: &Path, at: &Path) -> Repo {
    let url = format!("file://{}", bare.display());
    let r = git2::Repository::clone(&url, at).unwrap();
    let mut c = r.config().unwrap();
    c.set_str("user.name", "Test").unwrap();
    c.set_str("user.email", "test@example.com").unwrap();
    Repo::open(at)
        .unwrap()
        .with_global_config(GlobalConfig::Files(vec![]))
}

fn commit(repo: &Repo, file: &str, text: &str, message: &str) {
    std::fs::write(repo.workdir().join(file), text).unwrap();
    repo.commit(&CommitOptions {
        message: message.into(),
        all: true,
        ..Default::default()
    })
    .unwrap();
}

/// A bare remote with one commit on `main`, and two clones of it.
fn setup() -> (tempfile::TempDir, Repo, Repo) {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("remote.git");
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).initial_head("main");
    git2::Repository::init_opts(&bare, &opts).unwrap();
    // Seed it from a first clone.
    let seed = {
        let r = git2::Repository::init(tmp.path().join("seed")).unwrap();
        r.set_head("refs/heads/main").unwrap();
        r.remote("origin", &format!("file://{}", bare.display()))
            .unwrap();
        let mut c = r.config().unwrap();
        c.set_str("user.name", "Seed").unwrap();
        c.set_str("user.email", "seed@example.com").unwrap();
        Repo::open(&tmp.path().join("seed"))
            .unwrap()
            .with_global_config(GlobalConfig::Files(vec![]))
    };
    commit(&seed, "a.cs", "one\ntwo\nthree\n", "init");
    let pushed = seed
        .push(None, None, true, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    assert_eq!(pushed.upstream.as_deref(), Some("origin/main"));
    let a = clone_of(&bare, &tmp.path().join("a"));
    let b = clone_of(&bare, &tmp.path().join("b"));
    (tmp, a, b)
}

#[test]
fn fetch_updates_the_incoming_count_and_pull_fast_forwards() {
    let (_tmp, a, b) = setup();
    let s = a.status(false).unwrap();
    assert_eq!(
        (s.upstream.as_deref(), s.ahead, s.behind),
        (Some("origin/main"), 0, 0)
    );
    commit(&b, "b.cs", "b\n", "from b");
    let mut reports = 0;
    let pushed = b
        .push(None, None, false, false, &Cancel::new(), &mut |_| {
            reports += 1
        })
        .unwrap();
    assert_eq!(
        (pushed.remote.as_str(), pushed.branch.as_str()),
        ("origin", "main")
    );
    // b is even with its upstream; a does not know yet.
    let s = b.status(false).unwrap();
    assert_eq!((s.ahead, s.behind), (0, 0));
    assert_eq!(a.status(false).unwrap().behind, 0);
    let f = a.fetch(None, false, &Cancel::new(), &mut |_| {}).unwrap();
    assert_eq!(f.remote, "origin");
    assert_eq!(f.updated, ["origin/main"]);
    assert!(f.received_objects > 0);
    assert_eq!(
        a.status(false).unwrap().behind,
        1,
        "Fetch updates the incoming count"
    );
    let out = a
        .pull(None, false, None, &Cancel::new(), &mut |_| {})
        .unwrap();
    assert!(matches!(out, MergeOutcome::FastForward(_)), "{out:?}");
    assert!(a.workdir().join("b.cs").exists());
    assert_eq!(a.status(false).unwrap().behind, 0);
    assert_eq!(
        a.pull(None, false, None, &Cancel::new(), &mut |_| {})
            .unwrap(),
        MergeOutcome::UpToDate
    );
}

#[test]
fn outgoing_commits_push_and_a_non_fast_forward_is_refused() {
    let (_tmp, a, b) = setup();
    commit(&a, "a.cs", "one\nA\nthree\n", "a's change");
    assert_eq!(a.status(false).unwrap().ahead, 1, "outgoing");
    commit(&b, "b.cs", "b\n", "b's change");
    b.push(None, None, false, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    // a's push would overwrite b's commit.
    let e = a
        .push(None, None, false, false, &Cancel::new(), &mut |_| {})
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(e.message.contains("non-fast-forward"), "{e}");
    // Pull merges (both moved), then the push goes.
    let out = a
        .pull(None, false, None, &Cancel::new(), &mut |_| {})
        .unwrap();
    assert!(matches!(out, MergeOutcome::Merged(_)), "{out:?}");
    a.push(None, None, false, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    let s = a.status(false).unwrap();
    assert_eq!((s.ahead, s.behind), (0, 0));
    // Force overwrites the remote.
    a.reset("HEAD~1", eludite_git::branches::ResetMode::Hard)
        .unwrap();
    assert!(
        a.push(None, None, false, false, &Cancel::new(), &mut |_| {})
            .is_err()
    );
    a.push(None, None, false, true, &Cancel::new(), &mut |_| {})
        .unwrap();
    b.fetch(None, false, &Cancel::new(), &mut |_| {}).unwrap();
    let s = b.status(false).unwrap();
    assert_eq!(
        (s.ahead, s.behind),
        (1, 1),
        "the remote lost b's commit and has a's instead"
    );
}

#[test]
fn a_new_branch_pushes_with_its_upstream_and_pull_conflicts_stop() {
    let (_tmp, a, b) = setup();
    a.checkout("feature", true, None, false).unwrap();
    commit(&a, "f.cs", "f\n", "feature");
    assert_eq!(a.status(false).unwrap().upstream, None);
    let p = a
        .push(None, None, true, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    assert_eq!(p.upstream.as_deref(), Some("origin/feature"));
    let s = a.status(false).unwrap();
    assert_eq!(
        (s.upstream.as_deref(), s.ahead),
        (Some("origin/feature"), 0)
    );
    // b checks out the remote branch as a tracking branch.
    b.fetch(None, false, &Cancel::new(), &mut |_| {}).unwrap();
    let c = b.checkout("origin/feature", false, None, false).unwrap();
    assert_eq!((c.branch.as_deref(), c.created), (Some("feature"), true));
    assert_eq!(
        b.status(false).unwrap().upstream.as_deref(),
        Some("origin/feature")
    );
    // Both change the same line: the pull stops with the conflict.
    commit(&a, "a.cs", "one\nFROM A\nthree\n", "a");
    a.push(None, None, false, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    commit(&b, "a.cs", "one\nFROM B\nthree\n", "b");
    let out = b
        .pull(None, false, None, &Cancel::new(), &mut |_| {})
        .unwrap();
    assert_eq!(
        out,
        MergeOutcome::Conflicts {
            paths: vec!["a.cs".into()],
            step: 0,
            steps: 0
        }
    );
    // A canceled push sends nothing.
    let cancel = Cancel::new();
    cancel.cancel();
    assert_eq!(
        a.push(None, None, false, false, &cancel, &mut |_| {})
            .unwrap_err()
            .kind,
        ErrorKind::Canceled
    );
    let e = a
        .fetch(Some("nowhere"), false, &Cancel::new(), &mut |_| {})
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::NotFound);
}
