//! The guides an agent can read as MCP resources (brief 0027; `protocol/schemas/mcp-resource.json`): `resources/list`
//! lists them, `resources/read` serves one by its uri, and `resources/templates/list` is empty. The texts are compiled
//! in from `docs/agents/`, so a guide always matches the commands of the build that serves it.
//!
//! The guides: [`DEBUGGING`] (brief 0027), [`GIT`] (brief 0040), [`TERMINAL`] (brief 0041) and [`FORGE`] (brief
//! 0046).
//!
//! Besides the guides, [`GIT_STATUS_URI`] (brief 0040) is live: the repository's status as `eludite.git.status`
//! answers it, read through the command bus as the agent on each `resources/read`, and listed while that command is
//! registered. So is [`FORGE_PULL_URI`] (brief 0046): the current branch's pull request with its unresolved review
//! threads, read as the agent through `eludite.git.status`, `eludite.forge.pulls` and `eludite.forge.pull`.

use eludite_commands::{Caller, CommandRegistry, with_caller};
use serde_json::{Value, json};

/// The JSON-RPC error code of `resources/read` for a uri no resource has (MCP's "resource not found").
pub const RESOURCE_NOT_FOUND: i64 = -32002;

/// One guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guide {
    pub uri: &'static str,
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub text: &'static str,
}

/// How an agent should drive Eludite's debugger (`docs/agents/debugging.md`).
pub const DEBUGGING: Guide = Guide {
    uri: "eludite://guides/debugging",
    name: "debugging",
    title: "Debugging with Eludite: a guide for agents",
    description: "Read before driving the debugger (eludite.debug.*): snapshot before acting, run_until and trace over \
                  single steps, `stop` on every resuming call, output by cursor, what `interrupted_by: \"user\"` means, \
                  and what the debug policy and Allow Agents to Drive refuse.",
    text: include_str!("../../../docs/agents/debugging.md"),
};

/// How an agent reads the status, stages and commits, and what needs permission (`docs/agents/git.md`).
pub const GIT: Guide = Guide {
    uri: "eludite://guides/git",
    name: "git",
    title: "Git in Eludite: a guide for agents",
    description: "Read before using eludite.git.*: read the status first, diff before staging, stage and commit, \
                  branches, conflicts and sync, and what the git policy asks about or refuses (any `force`).",
    text: include_str!("../../../docs/agents/git.md"),
};

/// How an agent runs commands in the integrated terminal and what needs permission (`docs/agents/terminal.md`,
/// brief 0041).
pub const TERMINAL: Guide = Guide {
    uri: "eludite://guides/terminal",
    name: "terminal",
    title: "The terminal in Eludite: a guide for agents",
    description: "Read before using eludite.terminal.*: send a command and wait for its prompt with the mark, read the \
                  screen or the output since a mark, what `interrupted_by: \"user\"` means, and what the terminal policy \
                  (terminal.run) asks about.",
    text: include_str!("../../../docs/agents/terminal.md"),
};

/// How an agent reads and answers the review comments on its pull request, and what needs permission
/// (`docs/agents/forge.md`, brief 0046).
pub const FORGE: Guide = Guide {
    uri: "eludite://guides/forge",
    name: "forge",
    title: "Pull requests and issues in Eludite: a guide for agents",
    description: "Read before using eludite.forge.*: the review loop in four calls (read eludite://forge/pull/current, \
                  fix, reply to each thread and resolve it, request a re-review), reading from the cache, and what the \
                  forge policy asks about (writes), refuses (merges by default) and never allows (signing in).",
    text: include_str!("../../../docs/agents/forge.md"),
};

/// Every guide, in the order `resources/list` gives them.
pub const GUIDES: [Guide; 4] = [DEBUGGING, GIT, TERMINAL, FORGE];

/// The MIME type of every guide.
pub const MIME: &str = "text/markdown";

/// The repository's status (brief 0040), live.
pub const GIT_STATUS_URI: &str = "eludite://git/status";
/// Its MIME type.
pub const JSON_MIME: &str = "application/json";
/// The command that answers it.
const GIT_STATUS_COMMAND: &str = "eludite.git.status";

/// The current branch's pull request (brief 0046), live.
pub const FORGE_PULL_URI: &str = "eludite://forge/pull/current";
const FORGE_PULLS_COMMAND: &str = "eludite.forge.pulls";
const FORGE_PULL_COMMAND: &str = "eludite.forge.pull";

/// The pull request resource's `resources/list` entry.
pub fn forge_pull_descriptor() -> Value {
    json!({
        "uri": FORGE_PULL_URI,
        "name": "forge-pull-current",
        "title": "The current branch's pull request",
        "description": "The open pull request (merge request on GitLab) whose head is the checked-out branch, as \
                        eludite.forge.pull answers it with `threads: unresolved`: the review threads you must still \
                        answer, with their file, line and comments. `{\"pull\": null, \"branch\": ...}` when the \
                        branch has none.",
        "mimeType": JSON_MIME,
    })
}

/// The current branch's pull request, read through `registry` as the caller in effect.
fn current_pull(registry: &CommandRegistry) -> Result<Value, String> {
    let status = registry
        .invoke(GIT_STATUS_COMMAND, json!({}))
        .map_err(|e| e.to_string())?;
    let Some(branch) = status
        .get("branch")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(
            json!({"pull": null, "message": "HEAD is detached: no branch, no pull request"}),
        );
    };
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        let mut input = json!({"state": "open", "max": 100});
        if let Some(c) = &cursor {
            input["cursor"] = json!(c);
        }
        let page = registry
            .invoke(FORGE_PULLS_COMMAND, input)
            .map_err(|e| e.to_string())?;
        if let Some(p) = page["items"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["head"] == branch.as_str())
        {
            let item = match p["number"].as_u64() {
                Some(n) => json!({"number": n, "threads": "unresolved"}),
                None => json!({"id": p["id"], "threads": "unresolved"}),
            };
            return registry
                .invoke(FORGE_PULL_COMMAND, item)
                .map_err(|e| e.to_string());
        }
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => break,
        }
    }
    Ok(json!({"pull": null, "branch": branch}))
}

/// The status resource's `resources/list` entry.
pub fn git_status_descriptor() -> Value {
    json!({
        "uri": GIT_STATUS_URI,
        "name": "git-status",
        "title": "Git status",
        "description": "The workspace repository's status, as eludite.git.status answers it: the branch, its \
                        upstream with the incoming and outgoing counts, the staged and unstaged changes, untracked \
                        and conflicted files and its `generation`.",
        "mimeType": JSON_MIME,
    })
}

impl Guide {
    /// Its `resources/list` entry.
    pub fn descriptor(&self) -> Value {
        json!({
            "uri": self.uri,
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "mimeType": MIME,
            "size": self.text.len(),
        })
    }
}

/// The guide at `uri`.
pub fn find(uri: &str) -> Option<&'static Guide> {
    GUIDES.iter().find(|g| g.uri == uri)
}

/// `resources/list`'s result: the guides, and the status while `registry` has `eludite.git.status`.
pub fn list(registry: &CommandRegistry) -> Value {
    let mut resources: Vec<Value> = GUIDES.iter().map(Guide::descriptor).collect();
    if registry.lookup(GIT_STATUS_COMMAND).is_some() {
        resources.push(git_status_descriptor());
        if registry.lookup(FORGE_PULL_COMMAND).is_some() {
            resources.push(forge_pull_descriptor());
        }
    }
    json!({ "resources": resources })
}

/// `resources/read`'s result for `uri`: a guide, or the status read through `registry` as `caller`. `None` when no
/// resource has the uri; `Err` when the status could not be read.
pub fn read(
    uri: &str,
    registry: &CommandRegistry,
    caller: Caller,
) -> Option<Result<Value, String>> {
    if uri == GIT_STATUS_URI && registry.lookup(GIT_STATUS_COMMAND).is_some() {
        let status = with_caller(caller, || registry.invoke(GIT_STATUS_COMMAND, json!({})));
        return Some(match status {
            Ok(v) => Ok(json!({ "contents": [{
                "uri": GIT_STATUS_URI,
                "mimeType": JSON_MIME,
                "text": serde_json::to_string_pretty(&v).expect("serializes"),
            }] })),
            Err(e) => Err(e.to_string()),
        });
    }
    if uri == FORGE_PULL_URI
        && registry.lookup(FORGE_PULL_COMMAND).is_some()
        && registry.lookup(GIT_STATUS_COMMAND).is_some()
    {
        let pull = with_caller(caller, || current_pull(registry));
        return Some(pull.map(|v| {
            json!({ "contents": [{
                "uri": FORGE_PULL_URI,
                "mimeType": JSON_MIME,
                "text": serde_json::to_string_pretty(&v).expect("serializes"),
            }] })
        }));
    }
    let g = find(uri)?;
    Some(Ok(
        json!({ "contents": [{ "uri": g.uri, "mimeType": MIME, "text": g.text }] }),
    ))
}
