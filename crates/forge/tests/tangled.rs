//! Tangled against fixtures recorded from its appview (api.tangled.org, the tangled.org/core repository, public) and
//! the PLC directory, with synthesized sign-in and record writes: what its lexicons expose (pull requests and issues
//! read, comments, states, issues created) and only that.

mod support;

use eludite_forge::Family;
use serde_json::json;
use support::setup;

const REMOTE: &str = "https://tangled.sh/@tangled.org/core";
const PULL: &str = "at://did:plc:xasnlahkri4ewmbuzly2rlc5/sh.tangled.repo.pull/3mwqc5pt6dc5d";

#[test]
fn a_tangled_repository_shows_only_what_its_lexicons_expose() {
    let s = setup("tangled", Family::Tangled, "tangled.org", "", REMOTE, None);
    let d = s.ok("eludite.forge.detect", json!({}));
    assert_eq!(d["repository"]["family"], "tangled");
    assert_eq!(
        d["repository"]["host"], "tangled.org",
        "tangled.sh is tangled.org now"
    );
    let c = &d["capabilities"];
    for k in [
        "pull_requests",
        "issues",
        "issue_create",
        "branch_from_issue",
    ] {
        assert_eq!(c[k], true, "{k}");
    }
    for k in [
        "pull_create",
        "reviews",
        "review_threads",
        "thread_resolution",
        "draft_pull_requests",
        "labels",
        "milestones",
        "assignees",
        "checks",
    ] {
        assert_eq!(c[k], false, "{k}");
    }
    assert_eq!(c["merge_methods"], json!([]));

    let page = s.ok("eludite.forge.pulls", json!({"max": 3, "refresh": true}));
    let first = &page["items"][0];
    assert!(first.get("number").is_none(), "no numbers on Tangled");
    assert!(first["id"].as_str().unwrap().starts_with("at://"));
    assert!(
        !first["author"].as_str().unwrap().starts_with("did:"),
        "the author's handle from the PLC directory"
    );
    let p = s.ok("eludite.forge.pull", json!({"id": PULL}));
    assert_eq!(p["state"], "merged");
    assert_eq!(p["head"], "sl/static-mock");
    assert!(
        !p["files"].as_array().unwrap().is_empty(),
        "files from sh.tangled.repo.compare"
    );
    assert!(!p["commits"].as_array().unwrap().is_empty());
    assert_eq!(p["threads"], json!([]), "no line-anchored threads");
    assert_eq!(
        p["conversation"][0]["body"],
        "straight forward and makes sense, lgtm"
    );
    let issues = s.ok("eludite.forge.issues", json!({"max": 3, "refresh": true}));
    let id = issues["items"][0]["id"].as_str().unwrap().to_owned();
    s.ok("eludite.forge.issue", json!({"id": id}));
    assert_eq!(
        s.ok("eludite.forge.checks", json!({"id": PULL}))["state"],
        "none"
    );

    let e = s
        .run(
            "eludite.forge.pull_review",
            json!({"id": PULL, "action": "start"}),
        )
        .unwrap_err();
    assert!(e.message.contains("does not support reviews"), "{e}");
    let e = s.run("eludite.forge.pull_create", json!({})).unwrap_err();
    assert!(
        e.message
            .contains("does not support creating pull requests"),
        "{e}"
    );
    let e = s
        .run("eludite.forge.pull", json!({"number": 3}))
        .unwrap_err();
    assert!(e.message.contains("no numbers"), "{e}");
    let c = s.ok("eludite.forge.pull_checkout", json!({"id": PULL}));
    assert_eq!(c["branch"], "pr/3mwqc5pt6dc5d");
    assert_eq!(c["ref"], "refs/heads/sl/static-mock");
}

#[test]
fn an_app_password_session_writes_records() {
    let s = setup("tangled", Family::Tangled, "tangled.org", "", REMOTE, None);
    let refused = s.run("eludite.forge.auth", json!({"action": "sign_in", "method": "app_password", "user": "octocat.example", "token": "wrong"}));
    assert!(
        refused
            .unwrap_err()
            .message
            .contains("refused the app password")
    );
    let signed = s.ok("eludite.forge.auth", json!({"action": "sign_in", "method": "app_password", "user": "octocat.example", "token": "app-pass"}));
    assert_eq!(signed["account"]["login"], "octocat.example");
    let c = s.ok(
        "eludite.forge.pull_comment",
        json!({"id": PULL, "body": "Thanks!"}),
    );
    assert!(
        c["id"]
            .as_str()
            .unwrap()
            .starts_with("at://did:plc:testuser/")
    );
    let write = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target.ends_with("createRecord"))
        .unwrap();
    assert_eq!(
        write.authorization.as_deref(),
        Some("Bearer access-jwt-test")
    );
    assert!(
        write
            .body
            .contains("\"collection\":\"sh.tangled.feed.comment\"")
            && write.body.contains("\"pullRoundIdx\":0"),
        "{}",
        write.body
    );
    let e = s
        .run(
            "eludite.forge.pull_comment",
            json!({"id": PULL, "body": "x", "path": "a", "line": 1}),
        )
        .unwrap_err();
    assert!(e.message.contains("comments on a line"), "{e}");
    assert_eq!(
        s.ok("eludite.forge.pull_close", json!({"id": PULL}))["state"],
        "closed"
    );
    let status = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target.ends_with("createRecord"))
        .unwrap();
    assert!(
        status.body.contains("sh.tangled.repo.pull.status.closed"),
        "{}",
        status.body
    );
    let i = s.ok(
        "eludite.forge.issue_create",
        json!({"title": "Rate-limit the login endpoint"}),
    );
    assert_eq!(
        i["id"],
        "at://did:plc:testuser/sh.tangled.repo.issue/3newissue"
    );
    let issues = s.ok("eludite.forge.issues", json!({"max": 3}));
    let id = issues["items"][0]["id"].as_str().unwrap().to_owned();
    let b = s.ok("eludite.forge.branch_from_issue", json!({"id": id}));
    assert_eq!(b["linked"], true, "a comment links the branch");
    assert!(b["branch"].as_str().unwrap().starts_with("issue/"));
    let closed = s.ok(
        "eludite.forge.issue_update",
        json!({"id": id, "state": "closed"}),
    );
    assert_eq!(closed["state"], "closed");
    let e = s
        .run(
            "eludite.forge.issue_update",
            json!({"id": id, "labels": ["bug"]}),
        )
        .unwrap_err();
    assert!(e.message.contains("does not support"), "{e}");
}
