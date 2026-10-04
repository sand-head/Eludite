//! The recorder (`tools/forge-corpus/record.sh` runs it): the read scenario against public repositories on the real
//! services, through [`RecordingTransport`], writing scrubbed fixtures into `testdata/<forge>/recorded/` (or
//! `$ELUDITE_FORGE_RECORD/<forge>/`). Runs only when `ELUDITE_FORGE_RECORD` is set; a family runs when it is named in
//! `ELUDITE_FORGE_RECORD_ONLY` (comma-separated) or that is unset. No token is needed: these repositories are public,
//! and request headers are never written.

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use eludite_forge::Family;
use eludite_forge::http::UreqTransport;
use eludite_forge::replay::RecordingTransport;
use serde_json::json;

struct Target {
    forge: &'static str,
    family: Family,
    remote: &'static str,
    pull: serde_json::Value,
}

fn targets() -> Vec<Target> {
    vec![
        Target {
            forge: "forgejo",
            family: Family::Forgejo,
            remote: "https://codeberg.org/forgejo/forgejo.git",
            pull: json!({"number": 14628}),
        },
        Target {
            forge: "gitlab",
            family: Family::GitLab,
            remote: "https://gitlab.com/gitlab-org/cli.git",
            pull: json!({"number": 3992}),
        },
        Target {
            forge: "azure",
            family: Family::AzureDevOps,
            remote: "https://dev.azure.com/dnceng-public/public/_git/dotnet-public-wiki",
            pull: json!({"number": 5}),
        },
        Target {
            forge: "tangled",
            family: Family::Tangled,
            remote: "https://tangled.org/@tangled.org/core",
            pull: json!({"id": "at://did:plc:xasnlahkri4ewmbuzly2rlc5/sh.tangled.repo.pull/3mwqc5pt6dc5d"}),
        },
        Target {
            forge: "github",
            family: Family::GitHub,
            remote: "https://github.com/cli/cli.git",
            pull: json!({"number": 1}),
        },
    ]
}

#[test]
fn record_public_fixtures() {
    let Some(root) = std::env::var_os("ELUDITE_FORGE_RECORD") else {
        eprintln!(
            "skipped: set ELUDITE_FORGE_RECORD (tools/forge-corpus/record.sh) to record fixtures"
        );
        return;
    };
    let only: Option<Vec<String>> = std::env::var("ELUDITE_FORGE_RECORD_ONLY")
        .ok()
        .map(|s| s.split(',').map(|x| x.trim().to_owned()).collect());
    let date = eludite_forge::util::rfc3339(eludite_forge::util::now_secs());
    for t in targets() {
        if only
            .as_ref()
            .is_some_and(|o| !o.iter().any(|x| x == t.forge))
        {
            continue;
        }
        let dir = if root.is_empty() {
            support::testdata(t.forge).join("recorded")
        } else {
            PathBuf::from(&root).join(t.forge)
        };
        let _ = std::fs::remove_dir_all(&dir);
        let host = eludite_forge::detect::detect(t.remote, &[], None).host;
        let rec = RecordingTransport::new(
            Arc::new(UreqTransport::default()),
            &dir,
            format!(
                "recorded from {host} on {} by tools/forge-corpus/record.sh",
                &date[..10]
            ),
        )
        .map_host("plc.directory", "/plc")
        .map_host("bsky.social", "");
        let rec = if t.family == Family::Tangled {
            rec.api_host("api.tangled.org")
        } else {
            rec
        };
        let (hub, git, _tmp) = support::live(t.family, &host, t.remote, Arc::new(rec), None);
        let answers = support::read_scenario(&hub, &git, &t.pull, None);
        eprintln!(
            "{}: {} answers recorded into {}",
            t.forge,
            answers.len(),
            dir.display()
        );
        for (id, v) in &answers {
            support::conforms(&format!("eludite.forge.{id}"), v);
        }
    }
}
