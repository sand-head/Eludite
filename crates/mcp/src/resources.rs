//! The guides an agent can read as MCP resources (brief 0027; `protocol/schemas/mcp-resource.json`): `resources/list`
//! lists them, `resources/read` serves one by its uri, and `resources/templates/list` is empty. The texts are compiled
//! in from `docs/agents/`, so a guide always matches the commands of the build that serves it.

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

/// Every guide, in the order `resources/list` gives them.
pub const GUIDES: [Guide; 1] = [DEBUGGING];

/// The MIME type of every guide.
pub const MIME: &str = "text/markdown";

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

/// `resources/list`'s result.
pub fn list() -> Value {
    json!({ "resources": GUIDES.iter().map(Guide::descriptor).collect::<Vec<_>>() })
}

/// `resources/read`'s result for `uri`, or `None` when no guide has it.
pub fn read(uri: &str) -> Option<Value> {
    let g = find(uri)?;
    Some(json!({ "contents": [{ "uri": g.uri, "mimeType": MIME, "text": g.text }] }))
}
