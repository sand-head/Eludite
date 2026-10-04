//! Forgejo against fixtures recorded from codeberg.org (forgejo/forgejo, public, no token) and synthesized writes.

mod support;

use eludite_forge::Family;
use serde_json::json;
use support::{Setup, setup};

const REMOTE: &str = "https://codeberg.org/forgejo/forgejo.git";

fn fj() -> Setup {
    setup(
        "forgejo",
        Family::Forgejo,
        "codeberg.org",
        "/api/v1",
        REMOTE,
        Some("octocat"),
    )
}

#[test]
fn recorded_reads_from_codeberg() {
    let s = fj();
    let d = s.ok("eludite.forge.detect", json!({}));
    assert_eq!(d["repository"]["family"], "forgejo");
    assert_eq!(
        d["repository"]["web_url"],
        "https://codeberg.org/forgejo/forgejo"
    );
    assert_eq!(
        d["capabilities"]["thread_resolution"], false,
        "the API shows a resolver but cannot set one"
    );
    assert_eq!(d["capabilities"]["check_rerun"], false);
    let page = s.ok("eludite.forge.pulls", json!({"max": 3, "refresh": true}));
    assert_eq!(page["items"].as_array().unwrap().len(), 3);
    let next = s.ok(
        "eludite.forge.pulls",
        json!({"max": 3, "cursor": page["next_cursor"], "refresh": true}),
    );
    assert_eq!(next["items"].as_array().unwrap().len(), 3);
    assert_ne!(page["items"][0]["number"], next["items"][0]["number"]);

    let p = s.ok("eludite.forge.pull", json!({"number": 14628}));
    assert_eq!(p["state"], "merged");
    assert_eq!(p["files"][0]["status"], "modified", "Forgejo's `changed`");
    let threads = p["threads"].as_array().unwrap();
    assert!(threads.len() >= 3, "{threads:?}");
    let first = &threads[0];
    assert_eq!(first["path"], "services/repository/branch.go");
    assert_eq!(first["line"], 284);
    assert_eq!(first["resolved"], true, "the resolver on its first comment");
    assert!(
        first["comments"].as_array().unwrap().len() >= 4,
        "the replies on the same line join the thread"
    );
    assert_eq!(p["mergeable"]["state"], "merged");
    let reviews: Vec<_> = p["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["state"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        reviews.contains(&"approved".to_owned()) && reviews.contains(&"commented".to_owned()),
        "{reviews:?}"
    );

    let checks = s.ok("eludite.forge.checks", json!({"number": 14628}));
    let items = checks["items"].as_array().unwrap();
    let names: std::collections::HashSet<_> =
        items.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), items.len(), "the newest status per context");
    let job = items
        .iter()
        .find(|c| c["has_log"] == true)
        .expect("an Actions job");
    let log = s.ok(
        "eludite.forge.check_log",
        json!({"id": job["id"], "max_bytes": 4096}),
    );
    assert!(!log["text"].as_str().unwrap().is_empty());

    let issues = s.ok("eludite.forge.issues", json!({"max": 3, "refresh": true}));
    assert_eq!(issues["items"][0]["number"], 14684);
    let i = s.ok("eludite.forge.issue", json!({"number": 14684}));
    assert_eq!(i["number"], 14684);
    assert_eq!(s.fixtures().misses(), 0);
}

#[test]
fn writes_reviews_drafts_and_merge() {
    let s = fj();
    let created = s.ok(
        "eludite.forge.pull_create",
        json!({"draft": true, "labels": ["enhancement"], "base": "forgejo"}),
    );
    assert_eq!(created["number"], 77);
    let post = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.method == "POST" && x.target.ends_with("/pulls"))
        .unwrap();
    assert!(
        post.body.contains("\"title\":\"WIP: Add the login form\""),
        "a draft is the WIP prefix: {}",
        post.body
    );
    assert!(
        post.body.contains("\"labels\":[12]"),
        "labels by id: {}",
        post.body
    );
    assert_eq!(
        post.authorization.as_deref(),
        Some(format!("token {}", support::TOKEN).as_str())
    );

    s.ok(
        "eludite.forge.pull_review",
        json!({"number": 77, "action": "start"}),
    );
    s.ok(
        "eludite.forge.pull_review",
        json!({"number": 77, "action": "add", "body": "One", "path": "a.go", "line": 3}),
    );
    s.ok("eludite.forge.pull_review", json!({"number": 77, "action": "add", "body": "Two", "path": "b.go", "line": 9, "side": "left"}));
    let done = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 77, "action": "submit", "event": "approve"}),
    );
    assert_eq!(done["state"], "approved");
    let sent = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target.ends_with("/pulls/77/reviews"))
        .unwrap();
    assert!(
        sent.body.contains("\"new_position\":3") && sent.body.contains("\"old_position\":9"),
        "{}",
        sent.body
    );

    let ready = s.ok(
        "eludite.forge.pull_ready",
        json!({"number": 77, "draft": true}),
    );
    assert_eq!(ready["draft"], true);
    let e = s
        .run(
            "eludite.forge.thread_resolve",
            json!({"number": 77, "thread": "1"}),
        )
        .unwrap_err();
    assert!(e.message.contains("does not support resolving"), "{e}");
    let m = s.ok(
        "eludite.forge.pull_merge",
        json!({"number": 77, "method": "squash", "delete_branch": true}),
    );
    assert_eq!(m["method"], "squash");
    let merge = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.target.ends_with("/pulls/77/merge"))
        .unwrap();
    assert!(
        merge.body.contains("\"Do\":\"squash\"")
            && merge.body.contains("\"delete_branch_after_merge\":true"),
        "{}",
        merge.body
    );
    assert_eq!(
        s.ok("eludite.forge.pull_close", json!({"number": 77}))["state"],
        "closed"
    );

    let i = s.ok(
        "eludite.forge.issue_create",
        json!({"title": "Rate-limit the login endpoint", "labels": ["bug"]}),
    );
    assert_eq!(i["number"], 14700);
    s.ok(
        "eludite.forge.issue_comment",
        json!({"number": 14684, "body": "Taking this."}),
    );
    let u = s.ok(
        "eludite.forge.issue_update",
        json!({"number": 14684, "state": "closed", "labels": ["bug"]}),
    );
    assert_eq!(u["state"], "closed");
    let b = s.ok("eludite.forge.branch_from_issue", json!({"number": 14684}));
    assert_eq!(b["linked"], true);
    let link = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.method == "PATCH" && x.target.ends_with("/issues/14684"))
        .unwrap();
    assert!(
        link.body.contains("\"ref\":\"refs/heads/issue/14684-"),
        "Forgejo's branch reference: {}",
        link.body
    );
}
