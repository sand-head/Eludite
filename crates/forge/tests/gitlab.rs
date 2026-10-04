//! GitLab against fixtures recorded from gitlab.com (gitlab-org/cli, public, no token) and synthesized ones for what
//! gitlab.com shows only to a signed-in reader (discussions, notes, job logs) and for the writes.

mod support;

use eludite_forge::Family;
use serde_json::json;
use support::{Setup, setup};

const REMOTE: &str = "https://gitlab.com/gitlab-org/cli.git";

fn gl() -> Setup {
    setup(
        "gitlab",
        Family::GitLab,
        "gitlab.com",
        "/api/v4",
        REMOTE,
        Some("octocat"),
    )
}

#[test]
fn merge_requests_with_discussions_approvals_and_pipelines() {
    let s = gl();
    let d = s.ok("eludite.forge.detect", json!({}));
    assert_eq!(d["repository"]["owner"], "gitlab-org");
    assert_eq!(d["capabilities"]["approvals"], true);
    assert_eq!(
        d["capabilities"]["merge_methods"],
        json!(["merge", "squash", "fast_forward", "semi_linear"])
    );
    let page = s.ok("eludite.forge.pulls", json!({"max": 3, "refresh": true}));
    assert_eq!(page["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        page["items"][0]["draft"], true,
        "the recorded draft merge request"
    );

    let p = s.ok("eludite.forge.pull", json!({"number": 3992}));
    assert_eq!(p["head"], "310-config-uri");
    assert!(!p["files"].as_array().unwrap().is_empty());
    assert!(
        p["files"][0]["additions"].as_u64().unwrap() > 0,
        "counted from the diff"
    );
    assert_eq!(p["approvals"]["required"], 2);
    assert_eq!(p["approvals"]["given"], 0);
    assert_eq!(p["mergeable"]["state"], "blocked", "requested_changes");
    let threads = p["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["id"], "d1a2b3");
    assert_eq!(threads[0]["line"], 118);
    assert_eq!(threads[0]["comments"].as_array().unwrap().len(), 2);
    assert_eq!(threads[1]["resolved"], true);
    assert_eq!(
        p["conversation"].as_array().unwrap().len(),
        1,
        "system notes are left out"
    );
    let checks = s.ok("eludite.forge.checks", json!({"number": 3992}));
    let items = checks["items"].as_array().unwrap();
    assert_eq!(items[0]["kind"], "pipeline");
    let manual = items
        .iter()
        .find(|c| c["name"].as_str().unwrap().ends_with("review-docs-deploy"))
        .unwrap();
    assert_eq!(manual["conclusion"], "action_required");
    let job = items
        .iter()
        .find(|c| c["name"].as_str().unwrap().ends_with("tests:integration"))
        .unwrap();
    assert_eq!(
        job["id"], "job:45049979:16914440454",
        "the fork's project runs the pipeline"
    );
    let log = s.ok("eludite.forge.check_log", json!({"id": job["id"]}));
    assert!(log["text"].as_str().unwrap().contains("FAILED"));
    let r = s.ok("eludite.forge.check_rerun", json!({"id": job["id"]}));
    assert!(r["message"].as_str().unwrap().contains("16914449999"));
    let issue = s.ok("eludite.forge.issue", json!({"number": 8577}));
    assert_eq!(issue["conversation"].as_array().unwrap().len(), 1);
}

#[test]
fn draft_notes_are_the_pending_review_and_merge_when_pipeline_succeeds() {
    let s = gl();
    s.ok(
        "eludite.forge.pull_review",
        json!({"number": 3992, "action": "start"}),
    );
    let a = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 3992, "action": "add", "body": "One", "path": "a.go", "line": 3}),
    );
    assert_eq!(a["pending_comments"], 1);
    s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 3992, "body": "Two", "reply_to": "d1a2b3", "pending": true}),
    );
    let drafts: Vec<_> = s
        .fixtures()
        .seen()
        .into_iter()
        .filter(|x| x.target.ends_with("/draft_notes") && x.method == "POST")
        .collect();
    assert_eq!(drafts.len(), 2, "kept on the forge as draft notes");
    assert!(
        drafts[0].body.contains("\"new_line\":3") && drafts[0].body.contains("\"base_sha\""),
        "{}",
        drafts[0].body
    );
    assert!(
        drafts[1]
            .body
            .contains("\"in_reply_to_discussion_id\":\"d1a2b3\""),
        "{}",
        drafts[1].body
    );
    let done = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 3992, "action": "submit", "event": "approve"}),
    );
    assert_eq!(done["state"], "approved");
    let seen = s.fixtures().seen();
    assert!(
        seen.iter()
            .any(|x| x.target.ends_with("/draft_notes/bulk_publish"))
    );
    assert!(
        seen.iter()
            .any(|x| x.target.ends_with("/merge_requests/3992/approve"))
    );

    let m = s.ok(
        "eludite.forge.pull_merge",
        json!({"number": 3992, "method": "squash", "when_checks_pass": true, "force": true}),
    );
    assert_eq!(m["auto_merge"], true);
    let put = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.target.ends_with("/merge_requests/3992/merge"))
        .unwrap();
    assert!(put.body.contains("\"squash\":true"), "{}", put.body);
    let refused = s
        .run(
            "eludite.forge.pull_merge",
            json!({"number": 3992, "method": "merge"}),
        )
        .unwrap_err();
    assert!(refused.message.contains("blocked"), "{refused}");
    let wrong = s
        .run(
            "eludite.forge.pull_merge",
            json!({"number": 3992, "method": "rebase"}),
        )
        .unwrap_err();
    assert!(wrong.message.contains("does not allow"), "{wrong}");

    s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 3992, "body": "Hi"}),
    );
    let line = s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 3992, "body": "Here", "path": "x.go", "line": 2}),
    );
    assert_eq!(line["thread"], "n3w");
    s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 3992, "body": "Re", "reply_to": "d1a2b3"}),
    );
    assert_eq!(
        s.ok(
            "eludite.forge.thread_resolve",
            json!({"number": 3992, "thread": "d1a2b3"})
        )["resolved"],
        true
    );
    let ready = s.ok(
        "eludite.forge.pull_ready",
        json!({"number": 3992, "draft": true}),
    );
    assert_eq!(ready["draft"], true);
    let put = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.method == "PUT" && x.target.ends_with("/merge_requests/3992"))
        .unwrap();
    assert!(put.body.contains("\"title\":\"Draft: "), "{}", put.body);
    assert_eq!(
        s.ok("eludite.forge.pull_close", json!({"number": 3992}))["state"],
        "closed"
    );
    let rq = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 3992, "action": "request", "reviewers": ["reviewer"]}),
    );
    assert_eq!(rq["requested"], json!(["reviewer"]));
    let created = s.ok("eludite.forge.pull_create", json!({}));
    assert_eq!(created["number"], 4000);
    assert_eq!(
        s.ok("eludite.forge.pull_checkout", json!({"number": 3992}))["ref"],
        "refs/merge-requests/3992/head"
    );
    let b = s.ok("eludite.forge.branch_from_issue", json!({"number": 8577}));
    assert_eq!(b["linked"], true, "a note on the issue");
    s.ok("eludite.forge.issue_create", json!({"title": "Rate-limit"}));
    assert_eq!(
        s.ok(
            "eludite.forge.issue_update",
            json!({"number": 8577, "state": "closed"})
        )["state"],
        "closed"
    );
    support::cache_has_no_token(&s.cache());
}

#[test]
fn the_device_flow_and_the_version_probe() {
    let s = setup(
        "gitlab",
        Family::GitLab,
        "gitlab.com",
        "/api/v4",
        REMOTE,
        None,
    );
    let mut c = s.hub.config();
    c.gitlab_application_id = Some("app-test".into());
    s.hub.set_config(c);
    let started = s.ok(
        "eludite.forge.auth",
        json!({"action": "sign_in", "method": "device"}),
    );
    assert_eq!(started["device"]["user_code"], "GLAB-1234");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while s.ok("eludite.forge.auth", json!({"action": "status"}))["signed_in"] != true {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
