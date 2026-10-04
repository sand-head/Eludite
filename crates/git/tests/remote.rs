//! Fetch, pull and push against a bare repository on disk (no network), with the ahead and behind counts; then the
//! same repository served on loopback by `git http-backend` behind basic authentication, over http and over https
//! with a self-signed certificate (brief 0045, `support/server.rs`).

use std::path::Path;

use eludite_git::branches::MergeOutcome;
use eludite_git::commit::CommitOptions;
use eludite_git::{Cancel, ErrorKind, GlobalConfig, Repo, SessionCredentials, UserPass, git2};

#[path = "support/server.rs"]
mod server;
use server::GitHttp;

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

fn up(username: &str, password: &str) -> UserPass {
    UserPass {
        username: username.into(),
        password: password.into(),
    }
}

/// `repo`'s `origin` moved to `url`, and `repo` offering `session`'s credentials.
fn served(repo: Repo, url: &str, session: &SessionCredentials) -> Repo {
    repo.repository()
        .unwrap()
        .remote_set_url("origin", url)
        .unwrap();
    repo.with_credentials(session.clone())
}

fn fetch(repo: &Repo) -> eludite_git::Result<eludite_git::remote::Fetched> {
    repo.fetch(None, false, &Cancel::new(), &mut |_| {})
}

fn push(repo: &Repo) -> eludite_git::Result<eludite_git::remote::Pushed> {
    repo.push(None, None, false, false, &Cancel::new(), &mut |_| {})
}

/// An http remote that asks for a user name and password: nothing answers (credentials_required with the host, as
/// the shell's prompt needs it), a wrong answer is refused and says so, the right one fetches and pushes.
#[test]
fn an_http_remote_asking_for_credentials_takes_the_prompts_answer() {
    let (tmp, a, b) = setup();
    let Some(server) = GitHttp::start(tmp.path(), "alice", "s3cret") else {
        return;
    };
    let session = SessionCredentials::new();
    let a = served(a, &server.url("remote.git"), &session);
    let e = fetch(&a).unwrap_err();
    assert_eq!(e.kind, ErrorKind::CredentialsRequired, "{e}");
    assert_eq!(e.host.as_deref(), Some(server.host().as_str()));
    assert!(!e.refused);
    assert!(
        e.message.starts_with(&format!(
            "credentials_required: {} asks for a user name and a password or token",
            server.host()
        )),
        "{e}"
    );
    assert!(e.message.contains("git.credentialPrompt"), "{e}");
    let asked = server.requests();
    assert!(
        !asked.is_empty() && asked.iter().all(|r| r.status == 401),
        "{asked:?}"
    );
    // A wrong answer: refused.
    session.supply(&server.host(), up("alice", "wrong"), false);
    let e = fetch(&a).unwrap_err();
    assert_eq!(
        (e.kind, e.refused),
        (ErrorKind::CredentialsRequired, true),
        "{e}"
    );
    assert!(
        e.message.contains("refused the user name and password"),
        "{e}"
    );
    // The right one, remembered for the session: fetch, pull and push go as alice.
    session.forget_once(&server.host());
    session.supply(&server.host(), up("alice", "s3cret"), true);
    commit(&b, "b.cs", "b\n", "from b");
    b.push(None, None, false, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    let f = fetch(&a).unwrap();
    assert_eq!(f.updated, ["origin/main"]);
    assert_eq!(a.status(false).unwrap().behind, 1);
    assert!(matches!(
        a.pull(None, false, None, &Cancel::new(), &mut |_| {})
            .unwrap(),
        MergeOutcome::FastForward(_)
    ));
    commit(&a, "a2.cs", "a2\n", "from a over http");
    push(&a).unwrap();
    let s = a.status(false).unwrap();
    assert_eq!((s.ahead, s.behind), (0, 0));
    let log = server.requests();
    assert!(
        log.iter().any(|r| r.user.as_deref() == Some("alice")
            && r.status == 200
            && r.target.contains("git-receive-pack")),
        "the push went through http-backend as alice: {log:?}"
    );
}

/// The configured credential helper answers before the prompt's answer is tried (a wrong one is kept here: it is
/// never reached).
#[test]
fn the_configured_credential_helper_answers_before_the_prompt() {
    if !server::on_path("sh", "-c") && !cfg!(unix) {
        eprintln!("skipped: the test's helper is a shell function");
        return;
    }
    let (tmp, a, _) = setup();
    let Some(server) = GitHttp::start(tmp.path(), "alice", "s3cret") else {
        return;
    };
    let session = SessionCredentials::new();
    session.supply(&server.host(), up("alice", "wrong"), true);
    let a = served(a, &server.url("remote.git"), &session);
    a.repository()
        .unwrap()
        .config()
        .unwrap()
        .set_str(
            "credential.helper",
            "!f() { echo username=alice; echo password=s3cret; }; f",
        )
        .unwrap();
    fetch(&a).unwrap();
    assert!(
        server
            .requests()
            .iter()
            .any(|r| r.user.as_deref() == Some("alice") && r.status == 200)
    );
}

/// `http.proxy` carries an https transfer: a remote on a host that does not resolve is reached through a CONNECT
/// proxy on loopback, which tunnels to the TLS test server (libgit2 tunnels only https through a proxy).
#[test]
fn http_proxy_carries_an_https_transfer() {
    let (tmp, a, _) = setup();
    let Some(server) = GitHttp::start_tls(tmp.path(), "alice", "s3cret") else {
        return;
    };
    let proxy = server::ConnectProxy::start(server.port);
    let session = SessionCredentials::new();
    session.supply("git.example.invalid", up("alice", "s3cret"), true);
    let a = served(a, "https://git.example.invalid/remote.git", &session);
    let config = || a.repository().unwrap().config().unwrap();
    config().set_bool("http.sslVerify", false).unwrap();
    config()
        .set_str("http.proxy", &format!("http://127.0.0.1:{}", proxy.port))
        .unwrap();
    fetch(&a).unwrap();
    assert_eq!(
        proxy.connects().first().map(String::as_str),
        Some("git.example.invalid:443")
    );
    assert!(server.requests().iter().any(|r| r.status == 200));
    // An empty http.proxy turns it off (as in git): the host does not resolve.
    config().set_str("http.proxy", "").unwrap();
    let before = proxy.connects().len();
    assert!(fetch(&a).is_err());
    assert_eq!(proxy.connects().len(), before);
}

/// https with a self-signed certificate: refused, naming the host, while `http.sslVerify` is on; with it off (as git
/// honors it) fetch, pull and push go over TLS, and the credential callback answered the server's 401 there too.
#[test]
fn https_with_a_self_signed_certificate_needs_ssl_verify_off() {
    let (tmp, a, b) = setup();
    let Some(server) = GitHttp::start_tls(tmp.path(), "alice", "s3cret") else {
        return;
    };
    let session = SessionCredentials::new();
    session.supply(&server.host(), up("alice", "s3cret"), true);
    let a = served(a, &server.url("remote.git"), &session);
    assert!(server.url("remote.git").starts_with("https://"));
    let e = fetch(&a).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Certificate, "{e}");
    assert_eq!(e.host.as_deref(), Some(server.host().as_str()));
    assert!(
        e.message.starts_with(&format!(
            "The certificate of {} could not be verified",
            server.host()
        )),
        "{e}"
    );
    assert!(
        server.requests().is_empty(),
        "nothing was sent before the certificate"
    );
    assert!(!a.status(false).unwrap().ssl_verify_off);
    a.repository()
        .unwrap()
        .config()
        .unwrap()
        .set_bool("http.sslVerify", false)
        .unwrap();
    assert!(
        a.status(false).unwrap().ssl_verify_off,
        "the status says verification is off"
    );
    commit(&b, "b.cs", "b\n", "from b");
    b.push(None, None, false, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    assert_eq!(fetch(&a).unwrap().updated, ["origin/main"]);
    assert!(matches!(
        a.pull(None, false, None, &Cancel::new(), &mut |_| {})
            .unwrap(),
        MergeOutcome::FastForward(_)
    ));
    commit(&a, "tls.cs", "tls\n", "over https");
    push(&a).unwrap();
    assert_eq!(a.status(false).unwrap().ahead, 0);
    let log = server.requests();
    assert_eq!(log.first().map(|r| r.status), Some(401), "{log:?}");
    assert!(
        log.iter()
            .any(|r| r.status == 200 && r.target.contains("git-receive-pack")),
        "{log:?}"
    );
}
