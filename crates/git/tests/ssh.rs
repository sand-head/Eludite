//! Fetch and push over ssh against a loopback `sshd` (brief 0045): libgit2's ssh transport (the bundled libssh2),
//! the credential callback's key-file path (`~/.ssh/id_ed25519`, no agent), and the refusal of an unknown host key
//! naming the host.
//!
//! Runs only when `ELUDITE_TEST_SSHD` names an `sshd` binary by absolute path (`/usr/sbin/sshd`), with `ssh-keygen`
//! and `git` on `PATH`; it skips otherwise (the key selection is tested alone in `credentials.rs`). The test makes a
//! host key and a user key with `ssh-keygen`, starts `sshd` on a free loopback port with its own configuration
//! (only that key is authorized), and runs the git half in a child process of this test binary whose `HOME` is a
//! temporary folder, so libgit2 reads that folder's `.ssh` (and no ssh agent is reachable).

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use eludite_git::commit::CommitOptions;
use eludite_git::{Cancel, ErrorKind, GlobalConfig, Repo, git2};

/// Set in the child process: the remote's ssh url, its path on disk and the known_hosts line for the server.
const CHILD_URL: &str = "ELUDITE_TEST_SSH_URL";
const CHILD_REPO: &str = "ELUDITE_TEST_SSH_REPO";
const CHILD_KNOWN: &str = "ELUDITE_TEST_SSH_KNOWN_HOSTS";

struct Sshd(Child);

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn keygen(path: &Path) {
    let ok = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "eludite-test", "-f"])
        .arg(path)
        .stdout(Stdio::null())
        .status()
        .expect("ssh-keygen")
        .success();
    assert!(ok, "ssh-keygen failed");
}

fn on_path(program: &str, arg: &str) -> bool {
    Command::new(program)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

#[test]
fn fetch_and_push_over_ssh_with_a_key_file() {
    if std::env::var_os(CHILD_URL).is_some() {
        return child();
    }
    let Some(sshd) = std::env::var_os("ELUDITE_TEST_SSHD").filter(|s| !s.is_empty()) else {
        eprintln!(
            "skipped: set ELUDITE_TEST_SSHD to an sshd binary (absolute path) to run the ssh test"
        );
        return;
    };
    assert!(
        on_path("ssh-keygen", "-?") && on_path("git", "version"),
        "the ssh test needs ssh-keygen and git on PATH"
    );
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let home = t.join("home");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    keygen(&t.join("host_ed25519"));
    keygen(&home.join(".ssh/id_ed25519"));
    std::fs::copy(home.join(".ssh/id_ed25519.pub"), t.join("authorized_keys")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = t.join("sshd_config");
    std::fs::write(
        &config,
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {t}/host_ed25519\nPidFile {t}/sshd.pid\n\
             AuthorizedKeysFile {t}/authorized_keys\nStrictModes no\nUsePAM no\nPasswordAuthentication no\n\
             KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin prohibit-password\n\
             LogLevel ERROR\n",
            t = t.display()
        ),
    )
    .unwrap();
    let log = std::fs::File::create(t.join("sshd.log")).unwrap();
    let _sshd = Sshd(
        Command::new(&sshd)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("sshd"),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            Instant::now() < deadline,
            "sshd did not listen: {}",
            std::fs::read_to_string(t.join("sshd.log")).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let host_key = std::fs::read_to_string(t.join("host_ed25519.pub")).unwrap();
    let mut fields = host_key.split_whitespace();
    let known = format!(
        "[127.0.0.1]:{port} {} {}\n",
        fields.next().unwrap(),
        fields.next().unwrap()
    );
    let user = String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout)
        .unwrap()
        .trim()
        .to_owned();
    let bare = t.join("remote.git");
    let url = format!("ssh://{user}@127.0.0.1:{port}{}", bare.display());
    // The git half, with HOME in the temporary folder and no ssh agent.
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fetch_and_push_over_ssh_with_a_key_file",
            "--nocapture",
        ])
        .env("HOME", &home)
        .env_remove("SSH_AUTH_SOCK")
        .env(CHILD_URL, &url)
        .env(CHILD_REPO, &bare)
        .env(CHILD_KNOWN, &known)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}\nsshd: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        std::fs::read_to_string(t.join("sshd.log")).unwrap_or_default()
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("ssh: fetch and push over"),
        "the child ran the git half"
    );
}

fn open(path: &Path) -> Repo {
    let r = git2::Repository::open(path).unwrap();
    let mut c = r.config().unwrap();
    c.set_str("user.name", "Test").unwrap();
    c.set_str("user.email", "test@example.com").unwrap();
    Repo::open(path)
        .unwrap()
        .with_global_config(GlobalConfig::Files(vec![]))
}

fn commit(repo: &Repo, file: &str, message: &str) {
    std::fs::write(repo.workdir().join(file), message).unwrap();
    repo.commit(&CommitOptions {
        message: message.into(),
        all: true,
        ..Default::default()
    })
    .unwrap();
}

/// In the child: a bare remote at the url's path, seeded over `file://`, then a clone that fetches and pushes over
/// ssh.
fn child() {
    let url = std::env::var(CHILD_URL).unwrap();
    let known = std::env::var(CHILD_KNOWN).unwrap();
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let bare = std::path::PathBuf::from(std::env::var(CHILD_REPO).unwrap());
    let mut opts = git2::RepositoryInitOptions::new();
    opts.bare(true).initial_head("main");
    git2::Repository::init_opts(&bare, &opts).unwrap();
    let work = home.parent().unwrap().join("work");
    let seed = git2::Repository::init(&work).unwrap();
    seed.set_head("refs/heads/main").unwrap();
    seed.remote("origin", &format!("file://{}", bare.display()))
        .unwrap();
    let a = open(&work);
    commit(&a, "a.txt", "init");
    a.push(None, None, true, false, &Cancel::new(), &mut |_| {})
        .unwrap();
    a.repository()
        .unwrap()
        .remote_set_url("origin", &url)
        .unwrap();
    // No known_hosts entry: the host key is refused, naming the host.
    let e = a
        .fetch(None, false, &Cancel::new(), &mut |_| {})
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Certificate, "{e}");
    assert!(e.message.contains("ssh host key of 127.0.0.1"), "{e}");
    // Known: the key file authenticates; fetch and push go over ssh.
    std::fs::write(home.join(".ssh/known_hosts"), known).unwrap();
    a.fetch(None, false, &Cancel::new(), &mut |_| {})
        .unwrap_or_else(|e| panic!("fetch over ssh: {e}"));
    commit(&a, "b.txt", "over ssh");
    let pushed = a
        .push(None, None, false, false, &Cancel::new(), &mut |_| {})
        .unwrap_or_else(|e| panic!("push over ssh: {e}"));
    let remote = git2::Repository::open_bare(&bare).unwrap();
    assert_eq!(
        remote.refname_to_id("refs/heads/main").unwrap(),
        pushed.oid,
        "the remote has the commit pushed over ssh"
    );
    // A key the server does not authorize: the failure says what was tried and that the prompt cannot help.
    std::fs::rename(
        home.join(".ssh/id_ed25519"),
        home.join(".ssh/id_rsa_unused"),
    )
    .unwrap();
    let e = a
        .fetch(None, false, &Cancel::new(), &mut |_| {})
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Credentials, "{e}");
    assert!(e.message.contains("no key file in ~/.ssh"), "{e}");
    println!("ssh: fetch and push over {url} passed");
}
