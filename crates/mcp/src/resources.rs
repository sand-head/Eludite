//! The guides an agent can read as MCP resources (brief 0027; `protocol/schemas/mcp-resource.json`): `resources/list`
//! lists them, `resources/read` serves one by its uri, and `resources/templates/list` is empty. The texts are compiled
//! in from `docs/agents/`, so a guide always matches the commands of the build that serves it.
//!
//! Besides the guides, [`GIT_STATUS_URI`] (brief 0040) is live: the repository's status as `eludite.git.status`
//! answers it, read through the command bus as the agent on each `resources/read`, and listed while that command is
//! registered.

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

/// Every guide, in the order `resources/list` gives them.
pub const GUIDES: [Guide; 2] = [DEBUGGING, GIT];

/// The MIME type of every guide.
pub const MIME: &str = "text/markdown";

/// The repository's status (brief 0040), live.
pub const GIT_STATUS_URI: &str = "eludite://git/status";
/// Its MIME type.
pub const JSON_MIME: &str = "application/json";
/// The command that answers it.
const GIT_STATUS_COMMAND: &str = "eludite.git.status";

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
    let g = find(uri)?;
    Some(Ok(
        json!({ "contents": [{ "uri": g.uri, "mimeType": MIME, "text": g.text }] }),
    ))
}
