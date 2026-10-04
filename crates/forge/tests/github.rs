//! GitHub against its fixtures (`testdata/github/`, synthesized from the pinned API descriptions: api.github.com was
//! not reachable where brief 0046 ran), through the real `ureq` transport and the loopback fixture server.

mod support;

use eludite_forge::Family;
use serde_json::json;
use support::{Setup, setup};

const REMOTE: &str = "git@github.test:octo-org/hello-world.git";

fn gh() -> Setup {
    setup(
        "github",
        Family::GitHub,
        "github.test",
        "",
        REMOTE,
        Some("octocat"),
    )
}

#[test]
fn detection_and_capabilities() {
    let s = gh();
    let v = s.ok("eludite.forge.detect", json!({}));
    assert_eq!(v["repository"]["family"], "github");
    assert_eq!(v["repository"]["owner"], "octo-org");
    assert_eq!(v["repository"]["name"], "hello-world");
    assert_eq!(v["signed_in"], true);
    assert_eq!(v["account"]["login"], "octocat");
    let c = &v["capabilities"];
    for k in [
        "pull_requests",
        "pending_reviews",
        "thread_resolution",
        "draft_pull_requests",
        "checks",
        "check_logs",
        "check_rerun",
        "branch_link",
    ] {
        assert_eq!(c[k], true, "{k}");
    }
    assert_eq!(c["merge_methods"], json!(["merge", "squash", "rebase"]));
    assert_eq!(
        s.fixtures().count(),
        0,
        "detection reads nothing from the forge"
    );
}

#[test]
fn pulls_list_filters_and_pages() {
    let s = gh();
    let all = s.ok("eludite.forge.pulls", json!({"max": 2}));
    let numbers: Vec<_> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [12, 13]);
    assert_eq!(all["stale"], false);
    let cursor = all["next_cursor"].as_str().expect("a next page").to_owned();
    let next = s.ok("eludite.forge.pulls", json!({"max": 2, "cursor": cursor}));
    assert_eq!(next["items"][0]["number"], 15);
    assert!(next.get("next_cursor").is_none());
    let mine = s.ok("eludite.forge.pulls", json!({"filter": "mine", "max": 10}));
    assert!(
        mine["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["author"] == "octocat")
    );
    let review = s.ok(
        "eludite.forge.pulls",
        json!({"filter": "review_requested", "max": 10}),
    );
    let r: Vec<_> = review["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["number"].as_u64().unwrap())
        .collect();
    assert_eq!(r, [15], "only #15 asks octocat for a review");
    assert_eq!(review["items"][0]["review_requested"], true);
    let text = s.ok("eludite.forge.pulls", json!({"text": "typo", "max": 10}));
    assert_eq!(text["items"].as_array().unwrap().len(), 1);
    let merged = s.ok("eludite.forge.pulls", json!({"state": "merged"}));
    assert_eq!(merged["items"][0]["state"], "merged");
}

#[test]
fn one_pull_request_with_threads_commits_files_checks_and_mergeability() {
    let s = gh();
    let p = s.ok("eludite.forge.pull", json!({"number": 12}));
    assert_eq!(p["title"], "Add the login form");
    assert_eq!(p["head"], "feature/login");
    assert_eq!(p["commits"].as_array().unwrap().len(), 2);
    let files = p["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[2]["status"], "renamed");
    assert_eq!(files[2]["old_path"], "docs/auth.md");
    let threads = p["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["path"], "src/login.rs");
    assert_eq!(threads[0]["line"], 14);
    assert_eq!(
        threads[0]["comments"].as_array().unwrap().len(),
        2,
        "the reply joins its thread"
    );
    assert_eq!(threads[0]["resolved"], false);
    assert_eq!(threads[1]["resolved"], true, "GraphQL's resolution");
    assert_eq!(p["unresolved_threads"], 1);
    assert_eq!(p["conversation"].as_array().unwrap().len(), 1);
    let states: Vec<_> = p["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["state"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(states, ["changes_requested", "requested"]);
    let checks = p["check_items"].as_array().unwrap();
    assert_eq!(checks.len(), 3, "two check runs and a status");
    assert_eq!(p["checks"], "failure");
    assert_eq!(p["mergeable"]["state"], "checks_failing");
    assert_eq!(
        p["mergeable"]["methods"],
        json!(["merge", "squash"]),
        "the repository disallows rebase"
    );
    let unresolved = s.ok(
        "eludite.forge.pull",
        json!({"number": 12, "threads": "unresolved"}),
    );
    assert_eq!(unresolved["threads"].as_array().unwrap().len(), 1);
    assert_eq!(unresolved["threads"][0]["id"], "100");
}

#[test]
fn create_comment_reply_and_review_with_pending_comments() {
    let s = gh();
    let created = s.ok(
        "eludite.forge.pull_create",
        json!({"reviewers": ["hubot"], "labels": ["enhancement"]}),
    );
    assert_eq!(created["number"], 14);
    assert_eq!(created["head"], "feature/login");
    assert_eq!(created["base"], "main");
    let post = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.method == "POST" && x.target.ends_with("/pulls"))
        .unwrap();
    assert!(
        post.body.contains("\"title\":\"Add the login form\""),
        "the single commit's title: {}",
        post.body
    );
    assert!(
        post.body.contains("It posts to /session."),
        "the commit's body: {}",
        post.body
    );
    assert_eq!(
        post.authorization.as_deref(),
        Some(format!("Bearer {}", support::TOKEN).as_str())
    );

    let c = s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 12, "body": "Looks good"}),
    );
    assert_eq!(c["id"], "3001");
    let line = s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 12, "body": "Rename this", "path": "src/login.rs", "line": 20}),
    );
    assert_eq!(line["thread"], "3002");
    let sent = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target.ends_with("/pulls/12/comments") && x.method == "POST")
        .unwrap();
    assert!(
        sent.body
            .contains(&format!("\"commit_id\":\"{}\"", "1".repeat(40))),
        "{}",
        sent.body
    );
    let reply = s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 12, "body": "Done", "reply_to": "100"}),
    );
    assert_eq!(reply["thread"], "100");

    let before = s.fixtures().count();
    s.ok(
        "eludite.forge.pull_review",
        json!({"number": 12, "action": "start"}),
    );
    let a = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 12, "action": "add", "body": "First", "path": "src/login.rs", "line": 3}),
    );
    assert_eq!(a["pending_comments"], 1);
    let b = s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 12, "body": "Second", "path": "src/main.rs", "line": 2, "pending": true}),
    );
    assert_eq!(b["pending"], true);
    assert_eq!(
        s.fixtures().count() - before,
        0,
        "starting a review and adding pending comments sends nothing"
    );
    let p = s.ok("eludite.forge.pull", json!({"number": 12}));
    assert_eq!(p["pending_review"]["comments"], 2);
    let submitted = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 12, "action": "submit", "event": "approve", "body": "LGTM"}),
    );
    assert_eq!(submitted["state"], "approved");
    assert_eq!(submitted["pending_comments"], 0);
    let reviews: Vec<_> = s
        .fixtures()
        .seen()
        .into_iter()
        .filter(|x| x.target.ends_with("/pulls/12/reviews") && x.method == "POST")
        .collect();
    assert_eq!(reviews.len(), 1, "one call");
    assert!(
        reviews[0].body.contains("First") && reviews[0].body.contains("Second"),
        "{}",
        reviews[0].body
    );
    let requested = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 12, "action": "request"}),
    );
    assert_eq!(
        requested["requested"],
        json!(["hubot"]),
        "the previous reviewers"
    );
}

#[test]
fn checkout_merge_close_ready_and_resolve() {
    let s = gh();
    let c = s.ok("eludite.forge.pull_checkout", json!({"number": 12}));
    assert_eq!(c["branch"], "pr/12");
    assert_eq!(c["ref"], "refs/pull/12/head");
    assert_eq!(
        s.git.checkouts.lock().unwrap()[0],
        (None, "refs/pull/12/head".to_owned(), "pr/12".to_owned())
    );

    let refused = s
        .run(
            "eludite.forge.pull_merge",
            json!({"number": 12, "method": "squash"}),
        )
        .unwrap_err();
    assert!(refused.message.contains("checks are failing"), "{refused}");
    let wrong = s
        .run(
            "eludite.forge.pull_merge",
            json!({"number": 13, "method": "rebase"}),
        )
        .unwrap_err();
    assert!(wrong.message.contains("does not allow `rebase`"), "{wrong}");
    let merged = s.ok(
        "eludite.forge.pull_merge",
        json!({"number": 13, "method": "merge"}),
    );
    assert_eq!(merged["merged"], true);
    assert_eq!(merged["sha"], "9".repeat(40));
    let auto = s.ok(
        "eludite.forge.pull_merge",
        json!({"number": 13, "method": "squash", "when_checks_pass": true}),
    );
    assert_eq!(auto["auto_merge"], true);
    let forced = s.run(
        "eludite.forge.pull_merge",
        json!({"number": 12, "method": "merge", "force": true}),
    );
    assert!(
        forced.is_err(),
        "the fixture server has no merge for #12: the forge was asked"
    );
    assert!(
        s.fixtures()
            .seen()
            .iter()
            .any(|x| x.method == "PUT" && x.target.ends_with("/pulls/12/merge"))
    );

    assert_eq!(
        s.ok("eludite.forge.pull_close", json!({"number": 12}))["state"],
        "closed"
    );
    assert_eq!(
        s.ok(
            "eludite.forge.pull_close",
            json!({"number": 12, "reopen": true})
        )["state"],
        "open"
    );
    assert_eq!(
        s.ok("eludite.forge.pull_ready", json!({"number": 15}))["draft"],
        false
    );
    assert_eq!(
        s.ok(
            "eludite.forge.pull_ready",
            json!({"number": 15, "draft": true})
        )["draft"],
        true
    );
    let u = s.ok(
        "eludite.forge.pull_update",
        json!({"number": 12, "title": "Add the login form (v2)"}),
    );
    assert_eq!(u["title"], "Add the login form (v2)");
    let r = s.ok(
        "eludite.forge.thread_resolve",
        json!({"number": 12, "thread": "100"}),
    );
    assert_eq!(r["resolved"], true);
    let gq = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target == "/graphql")
        .unwrap();
    assert!(
        gq.body.contains("PRRT_100"),
        "the thread's node id: {}",
        gq.body
    );
    assert_eq!(
        s.ok(
            "eludite.forge.thread_resolve",
            json!({"number": 12, "thread": "100", "resolved": false})
        )["resolved"],
        false
    );
}

#[test]
fn issues_and_branch_from_issue() {
    let s = gh();
    let list = s.ok("eludite.forge.issues", json!({}));
    let numbers: Vec<_> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [42, 41], "the pull request is not an issue");
    let bugs = s.ok("eludite.forge.issues", json!({"labels": ["bug"]}));
    assert_eq!(bugs["items"].as_array().unwrap().len(), 1);
    let i = s.ok("eludite.forge.issue", json!({"number": 42}));
    assert_eq!(i["milestone"], "v1.0");
    assert_eq!(i["conversation"].as_array().unwrap().len(), 1);
    let created = s.ok("eludite.forge.issue_create", json!({"title": "Rate-limit the login endpoint", "milestone": "v1.0", "labels": ["security"]}));
    assert_eq!(created["number"], 43);
    let post = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.method == "POST" && x.target.ends_with("/issues"))
        .unwrap();
    assert!(
        post.body.contains("\"milestone\":1"),
        "the milestone's number: {}",
        post.body
    );
    s.ok(
        "eludite.forge.issue_comment",
        json!({"number": 42, "body": "Taking this."}),
    );
    let u = s.ok(
        "eludite.forge.issue_update",
        json!({"number": 42, "state": "closed", "labels": ["bug", "security"]}),
    );
    assert_eq!(u["state"], "closed");
    let b = s.ok("eludite.forge.branch_from_issue", json!({"number": 42}));
    assert_eq!(b["branch"], "issue/42-login-accepts-an-empty-password");
    assert_eq!(b["linked"], true);
    assert_eq!(
        s.git.branches.lock().unwrap()[0].0,
        "issue/42-login-accepts-an-empty-password"
    );
    let gq = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target == "/graphql")
        .unwrap();
    assert!(
        gq.body.contains("createLinkedBranch") && gq.body.contains("I_kwDO42"),
        "{}",
        gq.body
    );
}

#[test]
fn checks_logs_and_reruns() {
    let s = gh();
    let c = s.ok("eludite.forge.checks", json!({"number": 12}));
    assert_eq!(c["state"], "failure");
    let items = c["items"].as_array().unwrap();
    let test = items.iter().find(|x| x["name"] == "test").unwrap();
    assert_eq!(test["conclusion"], "failure");
    assert_eq!(test["duration_seconds"], 300);
    assert_eq!(test["has_log"], true);
    assert_eq!(
        items.iter().find(|x| x["kind"] == "status").unwrap()["has_log"],
        false
    );
    let log = s.ok(
        "eludite.forge.check_log",
        json!({"id": "run:5002", "max_bytes": 100}),
    );
    assert_eq!(log["truncated"], true);
    assert_eq!(log["next_offset"], 100);
    let rest = s.ok(
        "eludite.forge.check_log",
        json!({"id": "run:5002", "offset": 100}),
    );
    assert!(rest["text"].as_str().unwrap().contains("exit code 101"));
    assert_eq!(rest["truncated"], false);
    let r = s.ok("eludite.forge.check_rerun", json!({"id": "run:5002"}));
    assert_eq!(r["rerun"], true);
    assert!(
        s.run("eludite.forge.check_log", json!({"id": "status:7001"}))
            .is_err(),
        "a status has no log"
    );
}

#[test]
fn sign_in_by_token_device_flow_and_cli() {
    let s = setup("github", Family::GitHub, "github.test", "", REMOTE, None);
    let anon = s.ok("eludite.forge.auth", json!({"action": "status"}));
    assert_eq!(anon["signed_in"], false);
    let e = s
        .run(
            "eludite.forge.pull_comment",
            json!({"number": 12, "body": "x"}),
        )
        .unwrap_err();
    assert!(
        e.message.starts_with("sign_in_required: github.test"),
        "{e}"
    );
    let signed = s.ok(
        "eludite.forge.auth",
        json!({"action": "sign_in", "method": "token", "token": "ghp_pasted"}),
    );
    assert_eq!(signed["signed_in"], true);
    assert_eq!(signed["account"]["login"], "octocat");
    assert_eq!(signed["store"], "memory");
    assert!(
        !signed.to_string().contains("ghp_pasted"),
        "never a secret in the output"
    );
    let checked = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.target == "/user")
        .unwrap();
    assert_eq!(checked.authorization.as_deref(), Some("Bearer ghp_pasted"));
    s.ok("eludite.forge.auth", json!({"action": "sign_out"}));
    assert_eq!(
        s.ok("eludite.forge.auth", json!({"action": "status"}))["signed_in"],
        false
    );

    // The device flow needs a client id: none is built in, the setting gives one.
    let none = s
        .run(
            "eludite.forge.auth",
            json!({"action": "sign_in", "method": "device"}),
        )
        .unwrap_err();
    assert!(none.message.contains("forge.githubClientId"), "{none}");
    let mut config = s.hub.config();
    config.github_client_id = Some("Iv1.test".into());
    s.hub.set_config(config);
    let started = s.ok(
        "eludite.forge.auth",
        json!({"action": "sign_in", "method": "device"}),
    );
    assert_eq!(started["pending"], true);
    assert_eq!(started["device"]["user_code"], "WDJB-MJHT");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let st = s.ok("eludite.forge.auth", json!({"action": "status"}));
        if st["signed_in"] == true {
            assert_eq!(st["method"], "device");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the device flow completes when the fixture answers"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let polls = s
        .fixtures()
        .seen()
        .into_iter()
        .filter(|x| x.target == "/login/oauth/access_token")
        .count();
    assert_eq!(polls, 2, "pending once, then the token");

    // The `gh` CLI's token.
    s.ok("eludite.forge.auth", json!({"action": "sign_out"}));
    let dir = tempfile::tempdir().unwrap();
    let gh = dir.path().join("gh");
    std::fs::write(&gh, "#!/bin/sh\n[ \"$1 $2 $3 $4\" = \"auth token --hostname github.test\" ] && echo ghp_from_cli\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        s.hub.set_cli(Family::GitHub, gh);
        let cli = s.ok(
            "eludite.forge.auth",
            json!({"action": "sign_in", "method": "cli"}),
        );
        assert_eq!(cli["signed_in"], true);
        assert_eq!(cli["method"], "cli");
        let checked = s
            .fixtures()
            .seen()
            .into_iter()
            .rfind(|x| x.target == "/user")
            .unwrap();
        assert_eq!(
            checked.authorization.as_deref(),
            Some("Bearer ghp_from_cli")
        );
    }
}

#[test]
fn the_cache_never_holds_the_token() {
    let s = gh();
    s.ok("eludite.forge.pulls", json!({}));
    s.ok("eludite.forge.pull", json!({"number": 12}));
    s.ok("eludite.forge.issues", json!({}));
    support::cache_has_no_token(&s.cache());
}
