//! ACP (Agent Client Protocol) client (PLAN.md D3, 5.2).
//!
//! Niello hosts any ACP-speaking agent in its Agents tool window. Agents run as
//! child processes speaking JSON-RPC 2.0, newline-delimited, over stdio; Claude
//! Code (through its ACP adapter, [`default_agents`]) is the first one.
//!
//! Public API: [`AgentDescriptor`] and [`default_agents`] (what to launch),
//! [`AcpClient`] with [`ClientEvent`] (the connection: `initialize`,
//! `session/new`, `session/prompt`, `session/cancel`, streamed
//! `session/update`, `session/request_permission`), [`protocol`] (the typed
//! subset of ACP v1), and [`fake_agent`] (a scripted agent for tests and
//! benchmarks, also built as the `niello-fake-acp-agent` binary).
//!
//! The wire types are hand-written instead of using the official
//! `agent-client-protocol` crate (Apache-2.0): that crate brings an async
//! runtime stack (`async-io`, `futures`, `schemars`) the shell does not use
//! elsewhere, and Niello needs only a small, tolerant subset (brief 0005 report).

mod client;
pub mod fake_agent;
pub mod protocol;

use serde::{Deserialize, Serialize};

pub use client::{AcpClient, AcpError, ClientEvent, EventSink};
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
    /// Extra environment for the agent process.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Variables removed from the inherited environment.
    #[serde(default)]
    pub env_remove: Vec<String>,
}

/// The Claude Code ACP adapter verified by brief 0005, pinned to the version
/// tested. It reuses the user's existing Claude Code login; Niello reads no key.
pub const CLAUDE_ADAPTER_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp@0.85.0";

/// Set by Claude Code in its own child processes. If Niello was started from a
/// Claude Code terminal they would leak into the agent and make it think it is
/// nested; older adapters refuse to start ("cannot be launched inside another
/// Claude Code session").
pub const CLAUDE_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
];

/// Agents offered out of the box.
pub fn default_agents() -> Vec<AgentDescriptor> {
    vec![AgentDescriptor {
        name: "Claude Code".into(),
        command: "npx".into(),
        args: vec!["-y".into(), CLAUDE_ADAPTER_PACKAGE.into()],
        env: Vec::new(),
        env_remove: CLAUDE_SESSION_ENV.iter().map(|s| (*s).to_owned()).collect(),
    }]
}

impl AgentDescriptor {
    /// The command line a user runs for a terminal auth method: the agent's
    /// own command plus the method's `args`.
    pub fn terminal_auth_command(&self, method: &protocol::AuthMethod) -> String {
        std::iter::once(self.command.as_str())
            .chain(self.args.iter().map(String::as_str))
            .chain(method.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_is_default() {
        let agents = default_agents();
        assert_eq!(agents[0].name, "Claude Code");
        assert_eq!(agents[0].command, "npx");
        // Verified by brief 0005: the official adapter, pinned.
        assert_eq!(
            agents[0].args,
            ["-y", "@agentclientprotocol/claude-agent-acp@0.85.0"]
        );
        assert!(agents[0].env_remove.iter().any(|v| v == "CLAUDECODE"));
        assert!(
            agents[0].env.is_empty(),
            "no key or token is passed to the agent"
        );
    }

    #[test]
    fn terminal_auth_command_appends_method_args() {
        let m = protocol::AuthMethod {
            id: "claude-ai-login".into(),
            name: "Claude Subscription".into(),
            description: None,
            kind: Some("terminal".into()),
            args: vec![
                "--cli".into(),
                "auth".into(),
                "login".into(),
                "--claudeai".into(),
            ],
        };
        assert_eq!(
            default_agents()[0].terminal_auth_command(&m),
            "npx -y @agentclientprotocol/claude-agent-acp@0.85.0 --cli auth login --claudeai"
        );
    }
}
