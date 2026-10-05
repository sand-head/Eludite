//! The updater end to end against a loopback server: check, download, verify, stage, the staged build found on the
//! next start, a cancellation mid-download, a corrupt archive, a development build that never asks the network.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use eludite_update::build::Platform;
use eludite_update::download::Source;
use eludite_update::http::UreqTransport;
use eludite_update::stage::Stage;
use eludite_update::test_support::{self as support, Server, install, list, publish};
use eludite_update::updater::{Config, Mode, Setup, State, Updater};
use eludite_update::{Build, Channel};

const REPO: &str = "sand-head/Eludite";
const T: Duration = Duration::from_secs(30);

fn setup(
    server: &Server,
    install_dir: &std::path::Path,
    state_file: Option<std::path::PathBuf>,
) -> Setup {
    Setup {
        install_dir: install_dir.to_owned(),
        platform: Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        },
        build: Build::read(install_dir),
        state_file,
        source: Source {
            api: server.base(),
            repository: REPO.into(),
        },
        transport: Arc::new(UreqTransport::new(Duration::from_secs(20))),
    }
}

#[test]
fn check_download_stage_and_find_it_again_on_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::serve(dir.path());
    let base = server.base();
    let big = vec![7u8; 3 * 1024 * 1024];
    let r1 = publish(
        dir.path(),
        &base,
        "unstable-20261005.9",
        "linux",
        "x86_64",
        &[("eludite", b"nine", 0o755)],
    );
    let r2 = publish(
        dir.path(),
        &base,
        "unstable-20261006.3",
        "linux",
        "x86_64",
        &[("eludite", b"new", 0o755), ("cef/libcef.so", &big, 0o644)],
    );
    list(dir.path(), REPO, &[r1, r2]);
    let install_dir = install(dir.path(), "unstable-20261005.9", "linux", "x86_64");
    let state_file = dir.path().join("state/updates.json");
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = {
        let seen = seen.clone();
        Arc::new(move |s: &eludite_update::Status| {
            seen.lock().unwrap().push(s.state.kind().to_owned())
        })
    };
    let updater = Updater::new(
        setup(&server, &install_dir, Some(state_file.clone())),
        Config {
            channel: Channel::Unstable,
            mode: Mode::Notify,
        },
        Some(listener),
    );
    let s = updater.status();
    assert!(s.enabled, "{s:?}");
    assert_eq!(s.state, State::Idle);
    assert!(server.paths().is_empty(), "nothing is asked before a check");
    assert!(updater.due(Duration::from_secs(1)), "never checked");

    updater.check(false);
    let s = updater.wait(T);
    assert!(!s.busy);
    let State::Available { update } = &s.state else {
        panic!("{:?}", s.state)
    };
    assert_eq!(update.tag, "unstable-20261006.3");
    assert_eq!(update.version, "0.1.0");
    assert!(s.last_check.is_some());
    assert!(!updater.due(Duration::from_secs(3600)), "just checked");
    assert!(state_file.is_file(), "the last check and the list are kept");

    // A second check sends the ETag and gets a 304.
    updater.check(false);
    updater.wait(T);
    let reqs = server.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[1].1.contains_key("if-none-match"), "{reqs:?}");
    assert!(reqs[0].0.starts_with("/repos/sand-head/Eludite/releases"));
    assert!(matches!(updater.status().state, State::Available { .. }));

    updater.download();
    let s = updater.wait(T);
    let State::Ready { update, staged } = &s.state else {
        panic!("{:?}", s.state)
    };
    assert_eq!(update.tag, "unstable-20261006.3");
    assert_eq!(staged.build.build.as_str(), "20261006.3");
    assert_eq!(
        std::fs::read(staged.layout.join("eludite")).unwrap(),
        b"new"
    );
    assert_eq!(
        std::fs::metadata(staged.layout.join("cef/libcef.so"))
            .unwrap()
            .len(),
        big.len() as u64
    );
    assert!(
        staged
            .layout
            .starts_with(install_dir.join(".eludite-update"))
    );
    assert_eq!(s.writable, Some(true));
    let kinds = seen.lock().unwrap().clone();
    assert!(kinds.contains(&"downloading".to_owned()), "{kinds:?}");
    assert!(kinds.contains(&"unpacking".to_owned()), "{kinds:?}");
    assert!(
        !updater.due(Duration::ZERO),
        "a staged build waits for the restart"
    );

    // A new updater (the next start) finds the staged build without a network call.
    let before = server.paths().len();
    let again = Updater::new(
        setup(&server, &install_dir, Some(state_file.clone())),
        Config {
            channel: Channel::Unstable,
            mode: Mode::Download,
        },
        None,
    );
    let s = again.status();
    assert!(
        matches!(&s.state, State::Ready { update, .. } if update.tag == "unstable-20261006.3"),
        "{:?}",
        s.state
    );
    assert!(s.last_check.is_some(), "kept across starts");
    assert_eq!(server.paths().len(), before);
    assert_eq!(
        Stage::new(&install_dir).staged().unwrap().tag,
        "unstable-20261006.3"
    );
}

#[test]
fn the_newest_installed_is_up_to_date_and_download_mode_checks_then_stages() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::serve(dir.path());
    let base = server.base();
    let r = publish(
        dir.path(),
        &base,
        "unstable-20261006.3",
        "linux",
        "x86_64",
        &[("eludite", b"new", 0o755)],
    );
    list(dir.path(), REPO, &[r]);
    let install_dir = install(dir.path(), "unstable-20261006.3", "linux", "x86_64");
    let updater = Updater::new(
        setup(&server, &install_dir, None),
        Config {
            channel: Channel::Unstable,
            mode: Mode::Download,
        },
        None,
    );
    updater.check(true);
    let s = updater.wait(T);
    assert_eq!(
        s.state,
        State::UpToDate {
            tag: "unstable-20261006.3".into()
        }
    );

    // An older install, checked with `then_download`, ends staged in one job.
    let older = install(
        &dir.path().join("older"),
        "unstable-20261005.1",
        "linux",
        "x86_64",
    );
    let updater = Updater::new(
        setup(&server, &older, None),
        Config {
            channel: Channel::Unstable,
            mode: Mode::Download,
        },
        None,
    );
    updater.check(true);
    let s = updater.wait(T);
    assert!(matches!(s.state, State::Ready { .. }), "{:?}", s.state);
}

#[test]
fn a_development_build_is_disabled_and_never_asks() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::serve(dir.path());
    let install_dir = dir.path().join("target/debug");
    std::fs::create_dir_all(&install_dir).unwrap();
    let updater = Updater::new(setup(&server, &install_dir, None), Config::default(), None);
    let s = updater.status();
    assert!(!s.enabled);
    assert!(
        s.reason.as_deref().unwrap().contains("development build"),
        "{s:?}"
    );
    assert!(!updater.due(Duration::ZERO));
    updater.check(true);
    updater.download();
    let s = updater.wait(T);
    assert_eq!(s.state, State::Idle);
    assert!(server.paths().is_empty());
}

#[test]
fn a_corrupt_archive_is_refused_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::serve(dir.path());
    let base = server.base();
    let r = publish(
        dir.path(),
        &base,
        "unstable-20261006.3",
        "linux",
        "x86_64",
        &[("eludite", b"new", 0o755)],
    );
    list(dir.path(), REPO, &[r]);
    // Tamper with the archive after its digest was listed.
    let archive = dir
        .path()
        .join("releases/unstable-20261006.3/eludite-0.1.0-linux-x86_64.tar.gz");
    let mut bytes = std::fs::read(&archive).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&archive, bytes).unwrap();
    let install_dir = install(dir.path(), "unstable-20261005.1", "linux", "x86_64");
    let updater = Updater::new(
        setup(&server, &install_dir, None),
        Config {
            channel: Channel::Unstable,
            mode: Mode::Notify,
        },
        None,
    );
    updater.download();
    let s = updater.wait(T);
    let State::Failed {
        error,
        message,
        update,
        ..
    } = &s.state
    else {
        panic!("{:?}", s.state)
    };
    assert_eq!(error, "verification");
    assert!(message.contains("SHA256SUMS"), "{message}");
    assert_eq!(update.as_ref().unwrap().tag, "unstable-20261006.3");
    let stage = Stage::new(&install_dir);
    assert!(
        std::fs::read_dir(stage.dir_of("unstable-20261006.3"))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "no archive kept"
    );
    assert!(stage.staged().is_none());
}

#[test]
fn a_cancel_mid_download_cleans_up_and_a_missing_list_is_an_http_error() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::serve(dir.path());
    let base = server.base();
    let big = support::noise(2 * 1024 * 1024);
    // The archive's name holds `slow` through the tag so the server paces it.
    let r = publish(
        dir.path(),
        &base,
        "unstable-20261006.3",
        "linux",
        "x86_64",
        &[("eludite", b"new", 0o755), ("blob", &big, 0o644)],
    );
    let r = serde_json::from_str::<serde_json::Value>(
        &r.to_string()
            .replace("/releases/unstable-20261006.3/", "/releases/slow/"),
    )
    .unwrap();
    std::fs::rename(
        dir.path().join("releases/unstable-20261006.3"),
        dir.path().join("releases/slow"),
    )
    .unwrap();
    list(dir.path(), REPO, &[r]);
    server
        .slow_ms
        .store(50, std::sync::atomic::Ordering::SeqCst);
    let install_dir = install(dir.path(), "unstable-20261005.1", "linux", "x86_64");
    let updater = Updater::new(
        setup(&server, &install_dir, None),
        Config {
            channel: Channel::Unstable,
            mode: Mode::Notify,
        },
        None,
    );
    updater.download();
    let started = std::time::Instant::now();
    while !matches!(updater.status().state, State::Downloading { received, .. } if received > 0) {
        assert!(started.elapsed() < T, "{:?}", updater.status().state);
        std::thread::sleep(Duration::from_millis(5));
    }
    updater.cancel();
    let s = updater.wait(T);
    assert!(
        matches!(&s.state, State::Failed { error, .. } if error == "canceled"),
        "{:?}",
        s.state
    );
    let stage = Stage::new(&install_dir);
    assert!(!stage.dir_of("unstable-20261006.3").exists());
    assert!(stage.staged().is_none());

    // No release list at all: an HTTP error naming the status.
    std::fs::remove_file(dir.path().join("repos").join(REPO).join("releases")).unwrap();
    updater.check(false);
    let s = updater.wait(T);
    assert!(
        matches!(&s.state, State::Failed { error, message, .. } if error == "http" && message.contains("404")),
        "{:?}",
        s.state
    );
}

#[test]
fn switching_the_channel_forgets_the_last_check() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::serve(dir.path());
    let base = server.base();
    let r = publish(
        dir.path(),
        &base,
        "unstable-20261006.3",
        "linux",
        "x86_64",
        &[("eludite", b"new", 0o755)],
    );
    list(dir.path(), REPO, &[r]);
    let install_dir = install(dir.path(), "unstable-20261005.1", "linux", "x86_64");
    let updater = Updater::new(
        setup(&server, &install_dir, None),
        Config {
            channel: Channel::Unstable,
            mode: Mode::Off,
        },
        None,
    );
    assert!(!updater.due(Duration::ZERO), "off never checks on its own");
    updater.check(false);
    let s = updater.wait(T);
    assert!(matches!(s.state, State::Available { .. }));
    updater.set_config(Config {
        channel: Channel::Unstable,
        mode: Mode::Notify,
    });
    assert_eq!(updater.status().mode, Mode::Notify);
    assert!(
        matches!(updater.status().state, State::Available { .. }),
        "the same channel keeps its answer"
    );
}
