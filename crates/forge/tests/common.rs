//! What every family shares, against the fixture server: the version probes, conditional requests answered 304 from
//! the cache, rate limits, a refused token, cancellation mid-page, stale reads refreshed in the background (and
//! dropped when the generation moved on), and the refresh budget.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_forge::cache::Cache;
use eludite_forge::credentials::{Credentials, MemoryStore};
use eludite_forge::http::{Cancel, Transport, UreqTransport};
use eludite_forge::hub::{Hub, HubEvent};
use eludite_forge::replay::{Exchange, FixtureServer, Fixtures};
use eludite_forge::{ErrorKind, Family, ItemRef, PullQuery};
use serde_json::json;
use support::{FakeGit, setup};

fn probe_hub(server: &FixtureServer) -> Arc<Hub> {
    let hub = Hub::new(
        Arc::new(support::LoopbackOnly(UreqTransport::default())),
        Credentials::new(Box::new(MemoryStore::new()), None),
    );
    let base = server.base();
    hub.set_probe_base(Some(Arc::new(move |_host: &str| base.clone())));
    hub
}

#[test]
fn unknown_hosts_are_detected_by_their_version_probe() {
    // gitlab.com answers /api/v4/version with 401 and its JSON message without a token: still GitLab.
    let f = Fixtures::new();
    f.push(Exchange::json(
        "GET",
        "/api/v4/version",
        401,
        json!({"message": "401 Unauthorized"}),
    ));
    let server = FixtureServer::start(f).unwrap();
    let hub = probe_hub(&server);
    let r = hub.detect("git@git.example.org:team/app.git", true);
    assert_eq!(r.family, Family::GitLab);
    assert_eq!(r.api, format!("{}/api/v4", server.base()));
    hub.detect("https://git.example.org/team/other.git", true);
    assert_eq!(server.fixtures.count(), 1, "one probe per host");
    assert_eq!(
        hub.detect("https://git.example.org/team/app.git", false)
            .family,
        Family::None,
        "no probe, no guess"
    );

    // A Forgejo instance: its version names gitea's, or /api/forgejo/v1/version answers.
    let f = Fixtures::new();
    f.push(Exchange::json(
        "GET",
        "/api/v1/version",
        200,
        json!({"version": "11.0.3+gitea-1.22.0"}),
    ));
    let server = FixtureServer::start(f).unwrap();
    assert_eq!(
        probe_hub(&server)
            .detect("https://forge.example.net/a/b", true)
            .family,
        Family::Forgejo
    );
    let f = Fixtures::new();
    f.push(Exchange::json(
        "GET",
        "/api/v1/version",
        200,
        json!({"version": "8.0.0"}),
    ));
    f.push(Exchange::json(
        "GET",
        "/api/forgejo/v1/version",
        200,
        json!({"version": "8.0.0"}),
    ));
    let server = FixtureServer::start(f).unwrap();
    assert_eq!(
        probe_hub(&server)
            .detect("https://forge.example.net/a/b", true)
            .family,
        Family::Forgejo
    );
    // Gitea: /api/v1/version answers, /api/forgejo does not.
    let f = Fixtures::new();
    f.push(Exchange::json(
        "GET",
        "/api/v1/version",
        200,
        json!({"version": "1.24.2"}),
    ));
    let server = FixtureServer::start(f).unwrap();
    assert_eq!(
        probe_hub(&server)
            .detect("https://gitea.example.net/a/b", true)
            .family,
        Family::Gitea
    );
    // GitHub Enterprise Server: /api/v3/meta.
    let f = Fixtures::new();
    f.push(Exchange::json(
        "GET",
        "/api/v3/meta",
        200,
        json!({"installed_version": "3.14.0"}),
    ));
    let server = FixtureServer::start(f).unwrap();
    let r = probe_hub(&server).detect("https://ghe.example.com/team/app", true);
    assert_eq!(r.family, Family::GitHub);
    // Nothing answers: no forge, and the windows say so.
    let server = FixtureServer::start(Fixtures::new()).unwrap();
    let hub = probe_hub(&server);
    let git = FakeGit::new("https://nothing.example.com/a/b.git");
    let d = eludite_forge::ops::run(&hub, &git, "eludite.forge.detect", &json!({})).unwrap();
    assert_eq!(d["repository"]["family"], "none");
    assert_eq!(
        d["message"],
        "No supported forge for https://nothing.example.com/a/b.git"
    );
    let e = eludite_forge::ops::run(&hub, &git, "eludite.forge.pulls", &json!({})).unwrap_err();
    assert!(e.message.starts_with("No supported forge for"), "{e}");
}

#[test]
fn a_conditional_request_answered_304_reads_the_kept_body() {
    let s = setup(
        "github",
        Family::GitHub,
        "github.test",
        "",
        "git@github.test:octo-org/hello-world.git",
        Some("octocat"),
    );
    let first = s.ok("eludite.forge.pulls", json!({"max": 2, "refresh": true}));
    let again = s.ok("eludite.forge.pulls", json!({"max": 2, "refresh": true}));
    assert_eq!(first["items"], again["items"]);
    let seen: Vec<_> = s
        .fixtures()
        .seen()
        .into_iter()
        .filter(|x| x.target.contains("/pulls?"))
        .collect();
    assert_eq!(seen[0].status, 200);
    assert_eq!(seen[1].if_none_match.as_deref(), Some("\"pulls-open-v1\""));
    assert_eq!(seen[1].status, 304);
}

#[test]
fn a_rate_limit_answers_with_its_reset_time_and_stops_sending() {
    let s = setup(
        "github",
        Family::GitHub,
        "github.test",
        "",
        "git@github.test:octo-org/hello-world.git",
        Some("octocat"),
    );
    s.fixtures().push_front(
        Exchange::json(
            "GET",
            "/repos/octo-org/hello-world/issues",
            403,
            json!({"message": "API rate limit exceeded"}),
        )
        .with_header("X-RateLimit-Remaining", "0")
        .with_header("X-RateLimit-Reset", "4102444800"),
    );
    let e = s.run("eludite.forge.issues", json!({})).unwrap_err();
    assert_eq!(e.kind, ErrorKind::RateLimited);
    assert_eq!(e.reset_at.as_deref(), Some("2100-01-01T00:00:00Z"));
    assert!(e.message.contains("2100-01-01T00:00:00Z"), "{e}");
    let before = s.fixtures().count();
    let e = s
        .run("eludite.forge.pull", json!({"number": 12}))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::RateLimited);
    assert_eq!(
        s.fixtures().count(),
        before,
        "an exhausted limit is honored without sending"
    );
}

#[test]
fn a_refused_token_is_sign_in_required() {
    let s = setup(
        "github",
        Family::GitHub,
        "github.test",
        "",
        "git@github.test:octo-org/hello-world.git",
        Some("octocat"),
    );
    s.fixtures().push_front(Exchange::json(
        "GET",
        "/repos/octo-org/hello-world/pulls/12",
        401,
        json!({"message": "Bad credentials"}),
    ));
    let e = s
        .run("eludite.forge.pull", json!({"number": 12}))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::SignInRequired);
    assert_eq!(e.host.as_deref(), Some("github.test"));
    assert!(
        e.message.starts_with("sign_in_required: github.test"),
        "{e}"
    );
}

/// Cancels its `Cancel` after `n` requests.
struct CancelAfter {
    inner: UreqTransport,
    n: std::sync::atomic::AtomicUsize,
    cancel: Cancel,
}

impl Transport for CancelAfter {
    fn send(
        &self,
        r: &eludite_forge::http::Request,
        c: &Cancel,
    ) -> eludite_forge::Result<eludite_forge::http::Response> {
        let out = self.inner.send(r, c);
        if self.n.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            self.cancel.cancel();
        }
        out
    }
}

#[test]
fn cancellation_stops_between_pages() {
    let fixtures = Fixtures::load(&support::testdata("github")).unwrap();
    let server = FixtureServer::start(fixtures).unwrap();
    let cancel = Cancel::new();
    let t = Arc::new(CancelAfter {
        inner: UreqTransport::default(),
        n: 1.into(),
        cancel: cancel.clone(),
    });
    let hub = Hub::new(t, Credentials::new(Box::new(MemoryStore::new()), None));
    hub.set_config(eludite_forge::hub::HubConfig {
        hosts: vec![eludite_forge::detect::HostEntry {
            host: "github.test".into(),
            family: Family::GitHub,
            api: Some(server.base()),
        }],
        ..Default::default()
    });
    let repo = hub.detect("git@github.test:octo-org/hello-world.git", false);
    let forge = hub.forge(&repo, cancel).unwrap();
    let e = forge
        .pulls(&PullQuery {
            max: 10,
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Canceled);
    assert_eq!(
        server.fixtures.count(),
        1,
        "the second page was never asked for"
    );
}

#[test]
fn a_stale_read_answers_the_cache_then_refreshes_and_a_failed_refresh_says_why() {
    let s = setup(
        "github",
        Family::GitHub,
        "github.test",
        "",
        "git@github.test:octo-org/hello-world.git",
        Some("octocat"),
    );
    let repo = s
        .hub
        .detect("git@github.test:octo-org/hello-world.git", false);
    let key = eludite_forge::ops::pulls_key(Default::default(), Default::default(), None, 50, None);
    // A cached answer an hour old.
    s.cache().put_at(&repo.cache_key(), &key, &json!({"items": [{"id": "1", "title": "Old", "state": "open", "author": "a", "head": "h", "base": "b", "url": "u"}]}), eludite_forge::util::now_secs() - 3600);
    let (tx, rx) = std::sync::mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    s.hub.add_listener(Arc::new(move |e: &HubEvent| {
        let _ = tx.lock().unwrap().send(e.clone());
    }));
    s.hub.set_background(true);
    let stale = s.ok("eludite.forge.pulls", json!({}));
    assert_eq!(stale["stale"], true);
    assert!(stale["age_seconds"].as_u64().unwrap() >= 3600);
    assert_eq!(stale["items"][0]["title"], "Old");
    match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        HubEvent::Refreshed { key: k, .. } => assert_eq!(k, key),
        other => panic!("{other:?}"),
    }
    let fresh = s.hub.cached::<serde_json::Value>(&repo, &key).unwrap();
    assert_eq!(
        fresh.value["items"][0]["number"], 12,
        "the cache holds the refreshed answer"
    );

    // Offline: the refresh fails, the cache stays, the answer says why.
    s.fixtures().push_front(Exchange::json(
        "GET",
        "/repos/octo-org/hello-world/pulls",
        500,
        json!({"message": "down"}),
    ));
    let _ = s.ok("eludite.forge.pulls", json!({}));
    match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        HubEvent::RefreshFailed { error, .. } => {
            assert!(error.message.contains("HTTP 500"), "{error}")
        }
        other => panic!("{other:?}"),
    }
    let again = s.ok("eludite.forge.pulls", json!({}));
    assert_eq!(again["stale"], true);
    assert!(
        again["refresh_error"]["message"]
            .as_str()
            .unwrap()
            .contains("HTTP 500")
    );
    assert_eq!(again["items"][0]["number"], 12, "the cache is kept");
    let _ = rx.recv_timeout(Duration::from_secs(10));
    // With `refresh`, a failure answers the cache with the reason at once.
    let waited = s.ok("eludite.forge.pulls", json!({"refresh": true}));
    assert_eq!(waited["stale"], true);
    assert_eq!(waited["refresh_error"]["kind"], "other");
}

#[test]
fn a_refresh_of_an_older_generation_is_dropped() {
    let s = setup(
        "github",
        Family::GitHub,
        "github.test",
        "",
        "git@github.test:octo-org/hello-world.git",
        Some("octocat"),
    );
    s.server.set_delay(Duration::from_millis(30));
    let repo = s
        .hub
        .detect("git@github.test:octo-org/hello-world.git", false);
    let key = eludite_forge::ops::pull_key(&ItemRef::Number(12));
    s.cache().put_at(&repo.cache_key(), &key, &json!({"id": "12", "title": "Cached", "state": "open", "author": "a", "head": "h", "base": "b", "url": "u"}), 0);
    let (tx, rx) = std::sync::mpsc::channel::<HubEvent>();
    let tx = std::sync::Mutex::new(tx);
    s.hub.add_listener(Arc::new(move |e: &HubEvent| {
        let _ = tx.lock().unwrap().send(e.clone());
    }));
    s.hub.set_background(true);
    let a = s.ok("eludite.forge.pull", json!({"number": 12}));
    assert_eq!(a["title"], "Cached");
    s.hub.bump(); // the workspace or the account changed while the refresh ran
    let deadline = Instant::now() + Duration::from_secs(20);
    while s.hub.refreshing(&repo, &key) {
        assert!(Instant::now() < deadline, "the refresh ends");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(rx.try_recv().is_err(), "the stale answer is not announced");
    assert_eq!(
        s.hub
            .cached::<serde_json::Value>(&repo, &key)
            .unwrap()
            .value["title"],
        "Cached",
        "nor written"
    );
}

#[test]
fn nothing_reaches_the_forge_before_a_command_runs() {
    let s = setup(
        "github",
        Family::GitHub,
        "github.test",
        "",
        "git@github.test:octo-org/hello-world.git",
        Some("octocat"),
    );
    let _ = s
        .hub
        .detect("git@github.test:octo-org/hello-world.git", true);
    let _ = s.hub.cached::<serde_json::Value>(
        &s.hub
            .detect("git@github.test:octo-org/hello-world.git", false),
        "pulls:x",
    );
    assert_eq!(s.fixtures().count(), 0);
}

#[test]
fn a_refresh_of_fifty_pull_requests_with_20ms_per_request_is_under_2s() {
    // 50 pull requests on one page; every request waits 20 ms.
    let f = Fixtures::new();
    let pulls: Vec<_> = (1..=50)
        .map(|n| json!({"number": n, "title": format!("Change {n}"), "state": "open", "draft": false, "user": {"login": "octocat"},
            "head": {"ref": format!("b{n}"), "sha": "1".repeat(40)}, "base": {"ref": "main", "sha": "2".repeat(40)},
            "html_url": format!("https://github.test/o/r/pull/{n}"), "labels": [], "requested_reviewers": []}))
        .collect();
    f.push(Exchange::json("GET", "/repos/o/r/pulls", 200, json!(pulls)));
    let server = FixtureServer::start(f).unwrap();
    server.set_delay(Duration::from_millis(20));
    let hub = Hub::new(
        Arc::new(support::LoopbackOnly(UreqTransport::default())),
        Credentials::new(Box::new(MemoryStore::new()), None),
    );
    hub.set_config(eludite_forge::hub::HubConfig {
        hosts: vec![eludite_forge::detect::HostEntry {
            host: "github.test".into(),
            family: Family::GitHub,
            api: Some(server.base()),
        }],
        ..Default::default()
    });
    let dir = tempfile::tempdir().unwrap();
    hub.set_cache(Some(Cache::for_workspace(dir.path())));
    let git = FakeGit::new("https://github.test/o/r.git");
    let t = Instant::now();
    let v = eludite_forge::ops::run(&hub, &git, "eludite.forge.pulls", &json!({"refresh": true}))
        .unwrap();
    let took = t.elapsed();
    assert_eq!(v["items"].as_array().unwrap().len(), 50);
    eprintln!(
        "refresh of 50 pull requests, 20 ms per request: {:.1} ms",
        took.as_secs_f64() * 1000.0
    );
    assert!(took < Duration::from_secs(2), "{took:?}");
    let t = Instant::now();
    let cached = eludite_forge::ops::run(&hub, &git, "eludite.forge.pulls", &json!({})).unwrap();
    hub.set_background(false);
    eprintln!(
        "the same list from the cache: {:.2} ms",
        t.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(cached["stale"], true);
}
