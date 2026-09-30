//! ACP (Agent Client Protocol) client (PLAN.md D3, 5.2).
//!
//! Niello hosts any ACP-speaking agent in its Agents tool window. Agents run as
//! child processes speaking JSON-RPC 2.0 over stdio; Claude Code (via its ACP
//! adapter) is the first one wired up.

use serde::{Deserialize, Serialize};

pub use niello_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};

/// How to launch an ACP agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDescriptor {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Agents offered out of the box.
pub fn default_agents() -> Vec<AgentDescriptor> {
    vec![AgentDescriptor {
        name: "Claude Code".into(),
        command: "npx".into(),
        args: vec!["claude-code-acp".into()],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_is_default() {
        let agents = default_agents();
        assert_eq!(agents[0].name, "Claude Code");
        assert_eq!(agents[0].command, "npx");
        assert_eq!(agents[0].args, ["claude-code-acp"]);
    }
}
