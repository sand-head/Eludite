//! The forge commands (brief 0046): `eludite.forge.*`, the Pull Requests and Issues windows' actions and the agents'
//! review loop, on GitHub, GitLab, Azure DevOps, Forgejo and Gitea, and Tangled.
//!
//! The schemas are `protocol/schemas/forge-*.json` (checked in first, CLAUDE.md invariant 4). This module checks
//! each command's input against its typed shape (unknown members refused, `number` or `id` where one item is named),
//! declares the classes and registers the escalation hooks that apply the solution policy's `forge` object
//! ([`crate::policy::ForgePolicy`]); the shell implements [`ForgeCommands`] over `eludite-forge` and answers the
//! output JSON, on the invoking thread (the UI invokes these off its own thread, agents from theirs). The audit keeps
//! an agent's arguments through [`ForgeCommands::audit_arguments`]: never a token, a body's length rather than the
//! body, and the forge and repository.
//!
//! Classes: the reads (`detect`, `auth` `status`, `pulls`, `pull`, `issues`, `issue`, `checks`, `check_log`,
//! `refresh`) are read, refused for agents under `forge.read: deny`; the writes (`pull_create`, `pull_update`,
//! `pull_comment`, `pull_review`, `pull_ready`, `thread_resolve`, `issue_create`, `issue_comment`, `issue_update`,
//! `branch_from_issue`, `check_rerun`) are execute, raised for agents by `forge.write` (`prompt` by default: the first
//! write of an agent session asks, "Allow for this session" holds); `pull_checkout` is execute (it changes the
//! working tree; the policy's `execute` decides); `pull_merge` and `pull_close` are dangerous, refused for agents by
//! `forge.merge: deny` (the default) and asked under `prompt`, a `force` always refused; `auth`'s `sign_in`,
//! `sign_out` and `cancel` are refused for agents outright.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::policy::{FORGE_SIGN_IN_REFUSED, FORGE_WRITE_GRANT, PolicyView};
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, Escalation, EscalationHook,
    PermissionClass,
};

pub const DETECT: &str = "eludite.forge.detect";
pub const AUTH: &str = "eludite.forge.auth";
pub const PULLS: &str = "eludite.forge.pulls";
pub const PULL: &str = "eludite.forge.pull";
pub const PULL_CREATE: &str = "eludite.forge.pull_create";
pub const PULL_UPDATE: &str = "eludite.forge.pull_update";
pub const PULL_COMMENT: &str = "eludite.forge.pull_comment";
pub const PULL_REVIEW: &str = "eludite.forge.pull_review";
pub const PULL_CHECKOUT: &str = "eludite.forge.pull_checkout";
pub const PULL_MERGE: &str = "eludite.forge.pull_merge";
pub const PULL_CLOSE: &str = "eludite.forge.pull_close";
pub const PULL_READY: &str = "eludite.forge.pull_ready";
pub const THREAD_RESOLVE: &str = "eludite.forge.thread_resolve";
pub const ISSUES: &str = "eludite.forge.issues";
pub const ISSUE: &str = "eludite.forge.issue";
pub const ISSUE_CREATE: &str = "eludite.forge.issue_create";
pub const ISSUE_COMMENT: &str = "eludite.forge.issue_comment";
pub const ISSUE_UPDATE: &str = "eludite.forge.issue_update";
pub const BRANCH_FROM_ISSUE: &str = "eludite.forge.branch_from_issue";
pub const CHECKS: &str = "eludite.forge.checks";
pub const CHECK_LOG: &str = "eludite.forge.check_log";
pub const CHECK_RERUN: &str = "eludite.forge.check_rerun";
pub const REFRESH: &str = "eludite.forge.refresh";

pub const ALL: [&str; 23] = [
    DETECT,
    AUTH,
    PULLS,
    PULL,
    PULL_CREATE,
    PULL_UPDATE,
    PULL_COMMENT,
    PULL_REVIEW,
    PULL_CHECKOUT,
    PULL_MERGE,
    PULL_CLOSE,
    PULL_READY,
    THREAD_RESOLVE,
    ISSUES,
    ISSUE,
    ISSUE_CREATE,
    ISSUE_COMMENT,
    ISSUE_UPDATE,
    BRANCH_FROM_ISSUE,
    CHECKS,
    CHECK_LOG,
    CHECK_RERUN,
    REFRESH,
];

/// The commands that write to the forge, under `forge.write`.
pub const WRITES: [&str; 11] = [
    PULL_CREATE,
    PULL_UPDATE,
    PULL_COMMENT,
    PULL_REVIEW,
    PULL_READY,
    THREAD_RESOLVE,
    ISSUE_CREATE,
    ISSUE_COMMENT,
    ISSUE_UPDATE,
    BRANCH_FROM_ISSUE,
    CHECK_RERUN,
];

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    macro_rules! s {
        ($name:literal) => {
            (
                include_str!(concat!(
                    "../../../protocol/schemas/forge-",
                    $name,
                    ".input.json"
                )),
                include_str!(concat!(
                    "../../../protocol/schemas/forge-",
                    $name,
                    ".output.json"
                )),
            )
        };
    }
    let (title, (input, output), class) = match id {
        DETECT => ("Forge: Detect", s!("detect"), Read),
        AUTH => ("Forge: Sign In", s!("auth"), Read),
        PULLS => ("Forge: Pull Requests", s!("pulls"), Read),
        PULL => ("Forge: Open Pull Request", s!("pull"), Read),
        PULL_CREATE => ("Forge: Create Pull Request", s!("pull-create"), Execute),
        PULL_UPDATE => ("Forge: Edit Pull Request", s!("pull-update"), Execute),
        PULL_COMMENT => (
            "Forge: Comment on Pull Request",
            s!("pull-comment"),
            Execute,
        ),
        PULL_REVIEW => ("Forge: Review Pull Request", s!("pull-review"), Execute),
        PULL_CHECKOUT => (
            "Forge: Check Out Pull Request",
            s!("pull-checkout"),
            Execute,
        ),
        PULL_MERGE => ("Forge: Merge Pull Request", s!("pull-merge"), Dangerous),
        PULL_CLOSE => ("Forge: Close Pull Request", s!("pull-close"), Dangerous),
        PULL_READY => ("Forge: Mark Ready for Review", s!("pull-ready"), Execute),
        THREAD_RESOLVE => ("Forge: Resolve Thread", s!("thread-resolve"), Execute),
        ISSUES => ("Forge: Issues", s!("issues"), Read),
        ISSUE => ("Forge: Open Issue", s!("issue"), Read),
        ISSUE_CREATE => ("Forge: New Issue", s!("issue-create"), Execute),
        ISSUE_COMMENT => ("Forge: Comment on Issue", s!("issue-comment"), Execute),
        ISSUE_UPDATE => ("Forge: Edit Issue", s!("issue-update"), Execute),
        BRANCH_FROM_ISSUE => (
            "Forge: Create Branch from Issue",
            s!("branch-from-issue"),
            Execute,
        ),
        CHECKS => ("Forge: Checks", s!("checks"), Read),
        CHECK_LOG => ("Forge: Check Log", s!("check-log"), Read),
        CHECK_RERUN => ("Forge: Rerun Check", s!("check-rerun"), Execute),
        REFRESH => ("Forge: Refresh", s!("refresh"), Read),
        other => unreachable!("not a forge command: {other}"),
    };
    (title, input, output, class)
}

pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// What the shell implements over `eludite-forge`.
pub trait ForgeCommands: Send + Sync {
    /// Run `id` with `input` (checked against its shape) and answer its output JSON.
    fn apply(&self, id: &'static str, input: Value) -> Result<Value, CommandError>;
    /// What the audit keeps of an agent's `input` for `id`.
    fn audit_arguments(&self, id: &str, input: &Value) -> Value;
}

// ----- Input shapes (the schemas' `properties`, unknown members refused) -----

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct DetectIn {
    remote: Option<String>,
    url: Option<String>,
    probe: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum AuthAction {
    Status,
    SignIn,
    SignOut,
    Cancel,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum Method {
    Device,
    Token,
    AppPassword,
    Cli,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AuthIn {
    action: AuthAction,
    host: Option<String>,
    method: Option<Method>,
    token: Option<String>,
    user: Option<String>,
    allow_file_store: Option<bool>,
    remote: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ListIn {
    filter: Option<String>,
    state: Option<String>,
    text: Option<String>,
    labels: Option<Vec<String>>,
    max: Option<u64>,
    cursor: Option<String>,
    refresh: Option<bool>,
    remote: Option<String>,
}

/// The members every command naming one item may carry, and its own.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ItemIn {
    number: Option<u64>,
    id: Option<String>,
    remote: Option<String>,
    refresh: Option<bool>,
    threads: Option<String>,
    title: Option<String>,
    body: Option<String>,
    draft: Option<bool>,
    base: Option<String>,
    path: Option<String>,
    line: Option<u64>,
    start_line: Option<u64>,
    side: Option<String>,
    reply_to: Option<String>,
    pending: Option<bool>,
    action: Option<String>,
    event: Option<String>,
    reviewers: Option<Vec<String>>,
    branch: Option<String>,
    method: Option<String>,
    message: Option<String>,
    delete_branch: Option<bool>,
    when_checks_pass: Option<bool>,
    force: Option<bool>,
    reopen: Option<bool>,
    thread: Option<String>,
    resolved: Option<bool>,
    state: Option<String>,
    labels: Option<Vec<String>>,
    assignees: Option<Vec<String>>,
    milestone: Option<String>,
    name: Option<String>,
    checkout: Option<bool>,
    link: Option<bool>,
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    what: Option<Vec<String>>,
    head: Option<String>,
    max_bytes: Option<u64>,
    offset: Option<u64>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// The members of `id`'s input schema.
fn members(id: &str) -> Vec<String> {
    spec(id)
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// Check `input` for command `id`: its shape (unknown members refused, by the schema's own list), the item it names,
/// the enums and bounds the windows and agents most often get wrong.
pub fn parse(id: &'static str, input: Value) -> Result<Value, CommandError> {
    let invalid = |e: String| CommandError::InvalidInput(format!("{id}: {e}"));
    let obj = match &input {
        Value::Object(o) => o,
        Value::Null => return parse(id, Value::Object(Default::default())),
        _ => return Err(invalid("the input is a JSON object".into())),
    };
    let allowed = members(id);
    if let Some(k) = obj.keys().find(|k| !allowed.contains(k)) {
        return Err(invalid(format!("unknown member `{k}`")));
    }
    match id {
        DETECT => serde_json::from_value::<DetectIn>(input.clone()).map(drop),
        AUTH => serde_json::from_value::<AuthIn>(input.clone()).map(drop),
        PULLS | ISSUES => serde_json::from_value::<ListIn>(input.clone()).map(drop),
        _ => serde_json::from_value::<ItemIn>(input.clone()).map(drop),
    }
    .map_err(|e| invalid(e.to_string()))?;
    let enum_of = |k: &str, allowed: &[&str]| -> Result<(), CommandError> {
        match obj.get(k).and_then(Value::as_str) {
            Some(v) if !allowed.contains(&v) => Err(invalid(format!(
                "`{k}` is one of {}, not `{v}`",
                allowed.join(", ")
            ))),
            _ => Ok(()),
        }
    };
    if let Some(m) = obj.get("max").and_then(Value::as_u64)
        && !(1..=200).contains(&m)
    {
        return Err(invalid("`max` is from 1 to 200".into()));
    }
    match id {
        PULLS => {
            enum_of("filter", &["all", "mine", "review_requested"])?;
            enum_of("state", &["open", "closed", "merged", "all"])?;
        }
        ISSUES => {
            enum_of("filter", &["all", "mine", "assigned"])?;
            enum_of("state", &["open", "closed", "all"])?;
        }
        PULL => enum_of("threads", &["all", "unresolved", "none"])?,
        PULL_REVIEW => {
            enum_of("action", &["start", "add", "submit", "discard", "request"])?;
            enum_of("event", &["approve", "request_changes", "comment"])?;
            if obj.get("action").is_none() {
                return Err(invalid("`action` is required".into()));
            }
        }
        PULL_MERGE => {
            enum_of(
                "method",
                &[
                    "merge",
                    "squash",
                    "rebase",
                    "rebase_merge",
                    "fast_forward",
                    "semi_linear",
                ],
            )?;
            if obj.get("method").is_none() {
                return Err(invalid("`method` is required".into()));
            }
        }
        ISSUE_UPDATE => enum_of("state", &["open", "closed"])?,
        PULL_COMMENT | ISSUE_COMMENT
            if obj
                .get("body")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty) =>
        {
            return Err(invalid("`body` is required".into()));
        }
        _ => {}
    }
    if let Some(s) = obj.get("side") {
        enum_of("side", &["right", "left"])
            .map_err(|_| invalid(format!("`side` is right or left, not {s}")))?;
    }
    let names_item = matches!(
        id,
        PULL | PULL_UPDATE
            | PULL_COMMENT
            | PULL_REVIEW
            | PULL_CHECKOUT
            | PULL_MERGE
            | PULL_CLOSE
            | PULL_READY
            | THREAD_RESOLVE
            | ISSUE
            | ISSUE_COMMENT
            | ISSUE_UPDATE
            | BRANCH_FROM_ISSUE
    );
    if names_item && obj.get("number").is_none() && obj.get("id").is_none() {
        return Err(invalid(
            "give `number` (or `id`, as a list answered it)".into(),
        ));
    }
    Ok(input)
}

fn flag(input: &Value, k: &str) -> bool {
    input.get(k).and_then(Value::as_bool) == Some(true)
}

/// The escalation hook of forge command `id`: the policy's `forge` object.
pub fn escalation(id: &'static str) -> Option<EscalationHook> {
    if id == PULL_CHECKOUT {
        return None;
    }
    Some(Arc::new(move |input: &Value, view: &PolicyView| {
        let forge = view.forge();
        match id {
            AUTH => match input.get("action").and_then(Value::as_str) {
                Some("status") | None => forge.decide_read(),
                Some(_) => Some(Escalation::Refuse(FORGE_SIGN_IN_REFUSED.into())),
            },
            PULL_MERGE => Some(forge.decide_merge(flag(input, "force"))),
            PULL_CLOSE => Some(forge.decide_merge(false)),
            w if WRITES.contains(&w) => {
                Some(forge.decide_write(view.session_granted(FORGE_WRITE_GRANT)))
            }
            _ => forge.decide_read(),
        }
    }))
}

/// Register every forge command, applying them to `target`, with their escalation hooks and audit redaction.
pub fn register(registry: &CommandRegistry, target: Arc<dyn ForgeCommands>) {
    for id in ALL {
        let t = target.clone();
        let redact = target.clone();
        registry.replace_with_redaction(
            spec(id),
            escalation(id),
            Arc::new(move |input: &Value| redact.audit_arguments(id, input)),
            move |input| {
                let input = parse(id, input)?;
                t.apply(id, input)
            },
        );
    }
}

/// The escalation a hook gives, for tests of other crates.
pub fn decide(id: &'static str, input: &Value, view: &PolicyView) -> Option<Escalation> {
    escalation(id).and_then(|h| h(input, view))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_member_of_every_item_schema_passes_the_shape_check() {
        for id in ALL {
            if [DETECT, AUTH, PULLS, ISSUES].contains(&id) {
                continue;
            }
            for k in members(id) {
                let one = serde_json::Map::from_iter([(k.clone(), Value::Null)]);
                assert!(
                    serde_json::from_value::<ItemIn>(Value::Object(one)).is_ok(),
                    "{id}: `{k}` is in the schema but refused"
                );
            }
        }
    }
    use crate::policy::{
        AgentPolicy, AlwaysAllow, ForgeMergePolicy, ForgePolicy, ForgeReadPolicy, PolicySnapshot,
        RunPolicy,
    };
    use crate::{Caller, with_caller};
    use serde_json::json;
    use std::sync::Mutex;

    fn view(forge: Option<ForgePolicy>, grants: &[&str]) -> PolicyView {
        PolicyView::of(PolicySnapshot {
            policy: AgentPolicy {
                forge,
                ..Default::default()
            },
            session_grants: grants.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        })
    }

    #[test]
    fn every_schema_parses_and_names_its_command() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
            assert_eq!(
                s.escalates().is_some(),
                WRITES.contains(&id) || matches!(id, PULL_MERGE | PULL_CLOSE | AUTH),
                "{id}: x-eludite-escalates documents the hook"
            );
        }
        let files = std::fs::read_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/schemas"
        ))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("forge-"))
        .count();
        assert_eq!(
            files,
            ALL.len() * 2,
            "two schemas per command, no stray one"
        );
    }

    #[test]
    fn classes() {
        use PermissionClass::*;
        for id in [
            DETECT, AUTH, PULLS, PULL, ISSUES, ISSUE, CHECKS, CHECK_LOG, REFRESH,
        ] {
            assert_eq!(spec(id).permission, Read, "{id}");
        }
        for id in WRITES.iter().chain([PULL_CHECKOUT].iter()) {
            assert_eq!(spec(id).permission, Execute, "{id}");
        }
        for id in [PULL_MERGE, PULL_CLOSE] {
            assert_eq!(spec(id).permission, Dangerous, "{id}");
        }
    }

    #[test]
    fn input_is_checked_before_the_shell_sees_it() {
        assert!(parse(PULLS, json!({"filter": "mine", "max": 20})).is_ok());
        assert!(parse(PULLS, json!({"filter": "everyone"})).is_err());
        assert!(parse(PULLS, json!({"max": 500})).is_err());
        assert!(parse(PULLS, json!({"surprise": 1})).is_err());
        assert!(parse(PULL, json!({})).is_err(), "number or id");
        assert!(
            parse(
                PULL,
                json!({"id": "at://did:plc:x/sh.tangled.repo.pull/abc"})
            )
            .is_ok()
        );
        assert!(parse(PULL_COMMENT, json!({"number": 3})).is_err(), "body");
        assert!(
            parse(
                PULL_COMMENT,
                json!({"number": 3, "body": "x", "side": "middle"})
            )
            .is_err()
        );
        assert!(parse(PULL_MERGE, json!({"number": 3})).is_err(), "method");
        assert!(parse(PULL_MERGE, json!({"number": 3, "method": "octopus"})).is_err());
        assert!(
            parse(
                PULL_REVIEW,
                json!({"number": 3, "action": "submit", "event": "approve"})
            )
            .is_ok()
        );
        assert!(
            parse(
                AUTH,
                json!({"action": "sign_in", "method": "token", "token": "t"})
            )
            .is_ok()
        );
        assert!(parse(AUTH, json!({"action": "steal"})).is_err());
        assert!(parse(CHECKS, json!({"ref": "main"})).is_ok());
        assert!(parse(REFRESH, json!({"what": ["pulls"]})).is_ok());
        assert!(parse(DETECT, Value::Null).is_ok());
    }

    #[test]
    fn the_forge_policy_applies_to_agents() {
        // Defaults: reads run, the first write asks (Allow for this session), merges and closes are refused.
        assert_eq!(decide(PULLS, &json!({}), &view(None, &[])), None);
        match decide(
            PULL_COMMENT,
            &json!({"number": 1, "body": "x"}),
            &view(None, &[]),
        ) {
            Some(Escalation::Raise {
                class,
                always_allow,
                ..
            }) => {
                assert_eq!(class, PermissionClass::Dangerous);
                assert_eq!(always_allow, AlwaysAllow::Session(FORGE_WRITE_GRANT.into()));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            decide(PULL_COMMENT, &json!({}), &view(None, &[FORGE_WRITE_GRANT])),
            Some(Escalation::Raise {
                class: PermissionClass::Execute,
                always_allow: AlwaysAllow::Granted,
                ..
            })
        ));
        assert!(matches!(
            decide(PULL_MERGE, &json!({"method": "merge"}), &view(None, &[])),
            Some(Escalation::Refuse(_))
        ));
        assert!(matches!(
            decide(PULL_CLOSE, &json!({}), &view(None, &[FORGE_WRITE_GRANT])),
            Some(Escalation::Refuse(_))
        ));
        let prompt = Some(ForgePolicy {
            merge: Some(ForgeMergePolicy::Prompt),
            ..Default::default()
        });
        assert!(matches!(
            decide(
                PULL_MERGE,
                &json!({"method": "merge"}),
                &view(prompt.clone(), &[])
            ),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        assert!(matches!(
            decide(
                PULL_MERGE,
                &json!({"method": "merge", "force": true}),
                &view(prompt, &[])
            ),
            Some(Escalation::Refuse(_))
        ));
        // Sign-in is refused outright; the status is a read.
        for action in ["sign_in", "sign_out", "cancel"] {
            assert_eq!(
                decide(AUTH, &json!({"action": action}), &view(None, &[])),
                Some(Escalation::Refuse(FORGE_SIGN_IN_REFUSED.into())),
                "{action}"
            );
        }
        assert_eq!(
            decide(AUTH, &json!({"action": "status"}), &view(None, &[])),
            None
        );
        // deny.
        let deny = Some(ForgePolicy {
            read: Some(ForgeReadPolicy::Deny),
            write: Some(RunPolicy::Deny),
            ..Default::default()
        });
        assert!(matches!(
            decide(ISSUES, &json!({}), &view(deny.clone(), &[])),
            Some(Escalation::Refuse(_))
        ));
        assert!(matches!(
            decide(ISSUE_CREATE, &json!({}), &view(deny, &[FORGE_WRITE_GRANT])),
            Some(Escalation::Refuse(_))
        ));
        assert_eq!(
            decide(PULL_CHECKOUT, &json!({}), &view(None, &[])),
            None,
            "the execute policy decides"
        );
    }

    struct Fake(Mutex<Vec<(String, Value)>>);

    impl ForgeCommands for Fake {
        fn apply(&self, id: &'static str, input: Value) -> Result<Value, CommandError> {
            self.0.lock().unwrap().push((id.to_owned(), input));
            Ok(json!({"signed_in": false}))
        }
        fn audit_arguments(&self, _id: &str, input: &Value) -> Value {
            let mut v = input.clone();
            if let Some(o) = v.as_object_mut() {
                if o.remove("token").is_some() {
                    o.insert("token".into(), json!("<hidden>"));
                }
                if let Some(b) = o.remove("body") {
                    o.insert(
                        "body_length".into(),
                        json!(b.as_str().map(str::len).unwrap_or(0)),
                    );
                }
            }
            v
        }
    }

    #[test]
    fn an_agents_sign_in_is_refused_and_audited_without_its_token() {
        let reg = CommandRegistry::new();
        let fake = Arc::new(Fake(Mutex::new(Vec::new())));
        register(&reg, fake.clone());
        let agent = Caller::Agent {
            agent: "claude".into(),
            call: 1,
            tool_call: None,
        };
        let r = with_caller(agent.clone(), || {
            reg.invoke(
                AUTH,
                json!({"action": "sign_in", "method": "token", "token": "ghp_secret"}),
            )
        });
        assert!(r.unwrap_err().to_string().contains("never signs in"));
        assert!(fake.0.lock().unwrap().is_empty(), "the handler never ran");
        let r = with_caller(agent, || {
            reg.invoke(PULL_COMMENT, json!({"number": 4, "body": "twelve chars"}))
        });
        assert!(r.is_ok());
        let entries = reg.audit_log().entries();
        let text = serde_json::to_string(&entries).unwrap();
        assert!(!text.contains("ghp_secret"), "{text}");
        assert!(!text.contains("twelve chars"), "{text}");
        assert!(text.contains("\"body_length\":12"), "{text}");
        // The person's own sign-in reaches the handler.
        assert!(
            reg.invoke(
                AUTH,
                json!({"action": "sign_in", "method": "token", "token": "t"})
            )
            .is_ok()
        );
    }
}
