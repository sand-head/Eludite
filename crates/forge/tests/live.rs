//! The real-service tests: the same scenarios as the fixture tests, against a throwaway repository on each service,
//! when its token is set; each family skips otherwise (with a line saying so). Read and write, so point them at a
//! repository made for it:
//!
//! - GitHub: `ELUDITE_GITHUB_TOKEN`, `ELUDITE_GITHUB_REPO` (`owner/name`; github.com).
//! - GitLab: `ELUDITE_GITLAB_TOKEN`, `ELUDITE_GITLAB_HOST` (`gitlab.com`), `ELUDITE_GITLAB_REPO` (`group/project`).
//! - Azure DevOps: `ELUDITE_AZDO_TOKEN`, `ELUDITE_AZDO_ORG`, `ELUDITE_AZDO_PROJECT`, `ELUDITE_AZDO_REPO`.
//! - Forgejo: `ELUDITE_FORGEJO_TOKEN`, `ELUDITE_FORGEJO_HOST` (`codeberg.org`), `ELUDITE_FORGEJO_REPO` (`owner/name`).
//! - Tangled: `ELUDITE_TANGLED_APP_PASSWORD`, `ELUDITE_TANGLED_HANDLE`, `ELUDITE_TANGLED_REPO` (`handle/name`).
//!
//! `ELUDITE_<FORGE>_PR`, the number (Tangled: the at:// id) of an open pull request in that repository, adds the
//! pull request scenario (read with threads, a comment, a review comment submitted as a comment review).

mod support;

use std::sync::Arc;

use eludite_forge::Family;
use eludite_forge::http::UreqTransport;
use eludite_forge::ops;
use serde_json::{Value, json};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn scenario(
    family: Family,
    remote: String,
    token: String,
    user: Option<String>,
    pr: Option<String>,
) {
    let host = eludite_forge::detect::detect(&remote, &[], None).host;
    let (hub, git, _dir) = support::live(
        family,
        &host,
        &remote,
        Arc::new(UreqTransport::default()),
        Some((&token, user.as_deref())),
    );
    let run = |id: &str, input: Value| -> Value {
        let v = ops::run(&hub, &git, &format!("eludite.forge.{id}"), &input)
            .unwrap_or_else(|e| panic!("{id} {input}: {e}"));
        support::conforms(&format!("eludite.forge.{id}"), &v);
        v
    };
    let d = run("detect", json!({}));
    assert_eq!(d["signed_in"], true);
    run("pulls", json!({"refresh": true, "max": 5}));
    if d["capabilities"]["issues"] == true {
        let created = run(
            "issue_create",
            json!({"title": "Eludite real-service test", "body": "Made by crates/forge/tests/live.rs; safe to close."}),
        );
        let item = match created["number"].as_u64() {
            Some(n) => json!({"number": n}),
            None => json!({"id": created["id"]}),
        };
        let mut c = item.clone();
        c["body"] = json!("A comment from the real-service test.");
        run("issue_comment", c);
        let mut u = item.clone();
        u["state"] = json!("closed");
        run("issue_update", u);
        run("issues", json!({"state": "all", "refresh": true, "max": 5}));
    }
    if let Some(pr) = pr {
        let item = match pr.parse::<u64>() {
            Ok(n) => json!({"number": n}),
            Err(_) => json!({"id": pr}),
        };
        let mut p = item.clone();
        p["refresh"] = json!(true);
        let pull = run("pull", p);
        let mut c = item.clone();
        c["body"] = json!("A comment from the real-service test.");
        run("pull_comment", c);
        if d["capabilities"]["reviews"] == true
            && let Some(f) = pull["files"].as_array().and_then(|a| {
                a.iter()
                    .find(|f| f["additions"].as_u64().unwrap_or(0) > 0 || f["status"] == "added")
            })
        {
            let mut a = item.clone();
            a["action"] = json!("add");
            a["body"] = json!("A review comment from the real-service test.");
            a["path"] = f["path"].clone();
            a["line"] = json!(1);
            run("pull_review", a);
            let mut s = item.clone();
            s["action"] = json!("submit");
            s["event"] = json!("comment");
            run("pull_review", s);
        }
        let mut ch = item.clone();
        ch["refresh"] = json!(true);
        run("checks", ch);
    }
    eprintln!(
        "{}: the real-service scenario passed against {remote}",
        family.display()
    );
}

#[test]
fn github_real_service() {
    let (Some(t), Some(r)) = (env("ELUDITE_GITHUB_TOKEN"), env("ELUDITE_GITHUB_REPO")) else {
        eprintln!("skipped: set ELUDITE_GITHUB_TOKEN and ELUDITE_GITHUB_REPO");
        return;
    };
    scenario(
        Family::GitHub,
        format!("https://github.com/{r}.git"),
        t,
        None,
        env("ELUDITE_GITHUB_PR"),
    );
}

#[test]
fn gitlab_real_service() {
    let (Some(t), Some(h), Some(r)) = (
        env("ELUDITE_GITLAB_TOKEN"),
        env("ELUDITE_GITLAB_HOST"),
        env("ELUDITE_GITLAB_REPO"),
    ) else {
        eprintln!("skipped: set ELUDITE_GITLAB_TOKEN, ELUDITE_GITLAB_HOST and ELUDITE_GITLAB_REPO");
        return;
    };
    scenario(
        Family::GitLab,
        format!("https://{h}/{r}.git"),
        t,
        None,
        env("ELUDITE_GITLAB_PR"),
    );
}

#[test]
fn azure_devops_real_service() {
    let (Some(t), Some(o), Some(p), Some(r)) = (
        env("ELUDITE_AZDO_TOKEN"),
        env("ELUDITE_AZDO_ORG"),
        env("ELUDITE_AZDO_PROJECT"),
        env("ELUDITE_AZDO_REPO"),
    ) else {
        eprintln!(
            "skipped: set ELUDITE_AZDO_TOKEN, ELUDITE_AZDO_ORG, ELUDITE_AZDO_PROJECT and ELUDITE_AZDO_REPO"
        );
        return;
    };
    scenario(
        Family::AzureDevOps,
        format!("https://dev.azure.com/{o}/{p}/_git/{r}"),
        t,
        None,
        env("ELUDITE_AZDO_PR"),
    );
}

#[test]
fn forgejo_real_service() {
    let (Some(t), Some(h), Some(r)) = (
        env("ELUDITE_FORGEJO_TOKEN"),
        env("ELUDITE_FORGEJO_HOST"),
        env("ELUDITE_FORGEJO_REPO"),
    ) else {
        eprintln!(
            "skipped: set ELUDITE_FORGEJO_TOKEN, ELUDITE_FORGEJO_HOST and ELUDITE_FORGEJO_REPO"
        );
        return;
    };
    scenario(
        Family::Forgejo,
        format!("https://{h}/{r}.git"),
        t,
        None,
        env("ELUDITE_FORGEJO_PR"),
    );
}

#[test]
fn tangled_real_service() {
    let (Some(t), Some(h), Some(r)) = (
        env("ELUDITE_TANGLED_APP_PASSWORD"),
        env("ELUDITE_TANGLED_HANDLE"),
        env("ELUDITE_TANGLED_REPO"),
    ) else {
        eprintln!(
            "skipped: set ELUDITE_TANGLED_APP_PASSWORD, ELUDITE_TANGLED_HANDLE and ELUDITE_TANGLED_REPO"
        );
        return;
    };
    scenario(
        Family::Tangled,
        format!("https://tangled.org/{r}"),
        t,
        Some(h),
        env("ELUDITE_TANGLED_PR"),
    );
}
