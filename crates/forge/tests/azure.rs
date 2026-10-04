//! Azure DevOps against fixtures recorded from dev.azure.com (dnceng-public/public, public, no token: a completed pull
//! request, its threads and commits) and synthesized ones (an active pull request with iterations, votes and policies,
//! work items, the writes).

mod support;

use eludite_forge::Family;
use serde_json::json;
use support::{Setup, setup};

const REMOTE: &str =
    "https://dnceng-public@dev.azure.com/dnceng-public/public/_git/dotnet-public-wiki";

fn az() -> Setup {
    setup(
        "azure",
        Family::AzureDevOps,
        "dev.azure.com",
        "/dnceng-public",
        REMOTE,
        Some("octocat@example.invalid"),
    )
}

#[test]
fn a_recorded_completed_pull_request() {
    let s = az();
    let d = s.ok("eludite.forge.detect", json!({}));
    assert_eq!(d["repository"]["project"], "public");
    assert_eq!(d["capabilities"]["votes"], true);
    assert_eq!(d["capabilities"]["pending_reviews"], false);
    let p = s.ok("eludite.forge.pull", json!({"number": 5}));
    assert_eq!(p["state"], "merged");
    assert_eq!(p["head"], "invBootstrap");
    assert!(!p["commits"].as_array().unwrap().is_empty());
    assert!(
        p["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["state"] == "requested"),
        "vote 0"
    );
    assert_eq!(p["mergeable"]["state"], "merged");
    let auth = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.target.contains("/pullrequests/5"))
        .unwrap();
    assert!(
        auth.authorization.unwrap().starts_with("Basic "),
        "a personal access token goes as Basic"
    );
    assert!(auth.target.contains("api-version=7.1"), "{}", auth.target);
}

#[test]
fn votes_iterations_policies_and_auto_complete() {
    let s = az();
    let list = s.ok("eludite.forge.pulls", json!({}));
    assert_eq!(list["items"][0]["number"], 17);
    let p = s.ok("eludite.forge.pull", json!({"number": 17}));
    assert_eq!(p["iterations"], 2);
    let files: Vec<_> = p["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        files,
        ["src/Login.cs", "src/Program.cs"],
        "folders left out"
    );
    let states: Vec<_> = p["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["state"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(states, ["approved", "waiting_for_author", "requested"]);
    let threads = p["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["path"], "src/Login.cs");
    assert_eq!(threads[0]["line"], 12);
    assert_eq!(threads[1]["resolved"], true, "fixed");
    assert_eq!(
        p["conversation"].as_array().unwrap().len(),
        1,
        "system comments left out"
    );
    let checks = p["check_items"].as_array().unwrap();
    let build = checks
        .iter()
        .find(|c| c["id"] == "build:4242")
        .expect("the build validation policy's build");
    assert_eq!(build["name"], "PR build");
    assert_eq!(build["required"], true);
    assert_eq!(build["conclusion"], "failure");
    assert_eq!(build["duration_seconds"], 480);
    assert!(
        checks
            .iter()
            .any(|c| c["kind"] == "policy" && c["name"] == "Minimum number of reviewers")
    );
    assert_eq!(p["mergeable"]["state"], "checks_failing");
    let refused = s
        .run(
            "eludite.forge.pull_merge",
            json!({"number": 17, "method": "squash"}),
        )
        .unwrap_err();
    assert!(refused.message.contains("checks are failing"), "{refused}");
    let auto = s.ok(
        "eludite.forge.pull_merge",
        json!({"number": 17, "method": "squash", "when_checks_pass": true}),
    );
    assert_eq!(auto["auto_merge"], true);
    let patch = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.method == "PATCH" && x.body.contains("autoCompleteSetBy"))
        .unwrap();
    assert!(
        patch.body.contains("\"mergeStrategy\":\"squash\""),
        "{}",
        patch.body
    );
    let done = s.ok(
        "eludite.forge.pull_merge",
        json!({"number": 17, "method": "rebase_merge", "force": true}),
    );
    assert_eq!(done["merged"], true);
    let log = s.ok("eludite.forge.check_log", json!({"id": "build:4242"}));
    assert!(log["text"].as_str().unwrap().contains("exit code 101"));
    s.ok("eludite.forge.check_rerun", json!({"id": "build:4242"}));
    s.ok(
        "eludite.forge.check_rerun",
        json!({"id": "policy:ev-build"}),
    );

    // No pending reviews: each comment posts at once, the submit is the vote.
    s.ok(
        "eludite.forge.pull_review",
        json!({"number": 17, "action": "start"}),
    );
    let a = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 17, "action": "add", "body": "One", "path": "src/Login.cs", "line": 4}),
    );
    assert_eq!(a["submitted"], true);
    let thread = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target.contains("/pullrequests/17/threads?"))
        .unwrap();
    assert!(
        thread.body.contains("\"filePath\":\"/src/Login.cs\"")
            && thread.body.contains("rightFileStart"),
        "{}",
        thread.body
    );
    let v = s.ok(
        "eludite.forge.pull_review",
        json!({"number": 17, "action": "submit", "event": "approve"}),
    );
    assert_eq!(v["state"], "approved");
    let vote = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.method == "PUT" && x.target.contains("/reviewers/"))
        .unwrap();
    assert!(vote.body.contains("\"vote\":10"), "{}", vote.body);
    s.ok(
        "eludite.forge.pull_comment",
        json!({"number": 17, "body": "Re", "reply_to": "31"}),
    );
    assert_eq!(
        s.ok(
            "eludite.forge.thread_resolve",
            json!({"number": 17, "thread": "31"})
        )["resolved"],
        true
    );
    assert_eq!(
        s.ok("eludite.forge.pull_close", json!({"number": 17}))["state"],
        "closed"
    );
    assert_eq!(
        s.ok(
            "eludite.forge.pull_ready",
            json!({"number": 17, "draft": true})
        )["draft"],
        true
    );
    let created = s.ok("eludite.forge.pull_create", json!({}));
    assert_eq!(created["number"], 18);
    let c = s.ok("eludite.forge.pull_checkout", json!({"number": 17}));
    assert_eq!(c["ref"], "refs/heads/feature/login", "the source branch");
}

#[test]
fn work_items_are_the_issues() {
    let s = az();
    let list = s.ok("eludite.forge.issues", json!({"filter": "assigned"}));
    let items = list["items"].as_array().unwrap();
    assert_eq!(items[0]["type"], "Bug");
    assert_eq!(items[0]["labels"], json!(["auth", "security"]));
    assert_eq!(items[1]["type"], "Task");
    let wiql = s
        .fixtures()
        .seen()
        .into_iter()
        .find(|x| x.target.contains("/wit/wiql"))
        .unwrap();
    assert!(
        wiql.body.contains("[System.AssignedTo] = @Me"),
        "{}",
        wiql.body
    );
    let i = s.ok("eludite.forge.issue", json!({"number": 101}));
    assert_eq!(i["body"], "The form submits with an empty password.");
    assert_eq!(i["fields"]["Priority"], "2");
    assert_eq!(i["fields"]["ReproSteps"], "Open\nSubmit");
    assert_eq!(i["conversation"][0]["body"], "Reproduced on main.");
    let created = s.ok(
        "eludite.forge.issue_create",
        json!({"title": "Rate-limit the login endpoint"}),
    );
    assert_eq!(created["number"], 103, "the process's Bug type");
    let u = s.ok(
        "eludite.forge.issue_update",
        json!({"number": 101, "state": "closed"}),
    );
    assert_eq!(u["state"], "closed");
    let patch = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.method == "PATCH" && x.target.contains("/workitems/101"))
        .unwrap();
    assert!(
        patch.body.contains("\"value\":\"Closed\""),
        "the type's Completed state: {}",
        patch.body
    );
    s.ok(
        "eludite.forge.issue_comment",
        json!({"number": 101, "body": "On it."}),
    );
    let b = s.ok("eludite.forge.branch_from_issue", json!({"number": 101}));
    assert_eq!(b["linked"], true);
    let link = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.method == "PATCH" && x.target.contains("/workitems/101"))
        .unwrap();
    assert!(link.body.contains("vstfs:///Git/Ref/cbb18261-c48f-4abb-8651-8cdcb5474649%2F2459d599-fdb2-4d28-9810-daeec061cf90%2FGBissue"), "{}", link.body);
}

#[test]
fn the_device_code_flow() {
    let s = setup(
        "azure",
        Family::AzureDevOps,
        "dev.azure.com",
        "/dnceng-public",
        REMOTE,
        None,
    );
    let mut c = s.hub.config();
    c.azure_application_id = Some("00000000-0000-0000-0000-00000000test".into());
    s.hub.set_config(c);
    let started = s.ok(
        "eludite.forge.auth",
        json!({"action": "sign_in", "method": "device"}),
    );
    assert_eq!(started["device"]["user_code"], "AZDO-5678");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while s.ok("eludite.forge.auth", json!({"action": "status"}))["signed_in"] != true {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    s.ok("eludite.forge.pull", json!({"number": 17}));
    let bearer = s
        .fixtures()
        .seen()
        .into_iter()
        .rfind(|x| x.target.contains("/pullrequests/17"))
        .unwrap();
    assert!(
        bearer.authorization.unwrap().starts_with("Bearer "),
        "an OAuth token goes as Bearer"
    );
}
