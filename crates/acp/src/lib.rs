//! ACP (Agent Client Protocol) client (PLAN.md D3, 5.2).
//!
//! Eludite hosts any ACP-speaking agent in its Agents tool window. Agents run as
//! child processes speaking JSON-RPC 2.0, newline-delimited, over stdio; Claude
//! Code (through its ACP adapter, [`default_agents`]) is the first one: the
//! native `eludite-claude-acp` adapter when it is installed (brief 0006), else
//! the Node adapter through `npx`.
//!
//! Public API: [`AgentDescriptor`], [`default_agents`] and
//! [`find_native_claude_adapter`] (what to launch),
//! [`AcpClient`] with [`ClientEvent`] (the connection: `initialize`,
//! `session/new`, `session/prompt`, `session/cancel`, streamed
//! `session/update`, `session/request_permission`), [`protocol`] (the typed
//! subset of ACP v1), and [`fake_agent`] (a scripted agent for tests and
//! benchmarks, also built as the `eludite-fake-acp-agent` binary).
//!
//! The wire types are hand-written instead of using the official
//! `agent-client-protocol` crate (Apache-2.0): that crate brings an async
//! runtime stack (`async-io`, `futures`, `schemars`) the shell does not use
//! elsewhere, and Eludite needs only a small, tolerant subset (brief 0005 report).

mod client;
pub mod fake_agent;
pub mod protocol;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use client::{AcpClient, AcpError, ClientEvent, EventSink};
pub use eludite_protocol::jsonrpc::{
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
/// tested. It reuses the user's existing Claude Code login; Eludite reads no key.
pub const CLAUDE_ADAPTER_PACKAGE: &str = "@agentclientprotocol/claude-agent-acp@0.85.0";

/// Set by Claude Code in its own child processes. If Eludite was started from a
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

/// The native Claude Code adapter's executable name (brief 0006): a Rust
/// binary that drives the user's `claude` CLI directly, with no Node.
pub const NATIVE_CLAUDE_ADAPTER: &str = if cfg!(windows) {
    "eludite-claude-acp.exe"
} else {
    "eludite-claude-acp"
};

/// Environment variable naming the native adapter explicitly (the configured
/// path, until Eludite has settings).
pub const NATIVE_CLAUDE_ADAPTER_ENV: &str = "ELUDITE_CLAUDE_ACP";

/// Where to look for the native adapter: the configured path, the directory
/// of the IDE's own executable (where it ships), then `PATH`.
#[derive(Debug, Clone, Default)]
pub struct AdapterSearch {
    pub configured: Option<PathBuf>,
    pub exe_dir: Option<PathBuf>,
    pub path: Option<OsString>,
}

impl AdapterSearch {
    /// From `$ELUDITE_CLAUDE_ACP`, the current executable and `$PATH`.
    pub fn from_env() -> Self {
        Self {
            configured: std::env::var_os(NATIVE_CLAUDE_ADAPTER_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            exe_dir: std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_path_buf)),
            path: std::env::var_os("PATH"),
        }
    }
}

fn is_executable_file(p: &Path) -> bool {
    let Ok(meta) = p.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Find the native adapter: the configured path (if it exists), beside the
/// IDE's executable, then on `PATH`.
pub fn find_native_claude_adapter(search: &AdapterSearch) -> Option<PathBuf> {
    if let Some(p) = &search.configured
        && is_executable_file(p)
    {
        return Some(p.clone());
    }
    if let Some(dir) = &search.exe_dir {
        let p = dir.join(NATIVE_CLAUDE_ADAPTER);
        if is_executable_file(&p) {
            return Some(p);
        }
    }
    std::env::split_paths(search.path.as_deref()?)
        .map(|d| d.join(NATIVE_CLAUDE_ADAPTER))
        .find(|p| is_executable_file(p))
}

/// Claude Code through the native adapter at `path`.
pub fn native_claude_agent(path: &Path) -> AgentDescriptor {
    AgentDescriptor {
        name: "Claude Code".into(),
        command: path.to_string_lossy().into_owned(),
        args: Vec::new(),
        env: Vec::new(),
        env_remove: CLAUDE_SESSION_ENV.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// Claude Code through the Node adapter, `npx -y` with the pinned version
/// (brief 0005). Needs Node.
pub fn npx_claude_agent() -> AgentDescriptor {
    AgentDescriptor {
        name: "Claude Code (Node adapter)".into(),
        command: "npx".into(),
        args: vec!["-y".into(), CLAUDE_ADAPTER_PACKAGE.into()],
        env: Vec::new(),
        env_remove: CLAUDE_SESSION_ENV.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// Agents offered out of the box, preferred first: the native Claude Code
/// adapter when it is found (see [`find_native_claude_adapter`]), then the
/// npx adapter as the fallback.
pub fn default_agents() -> Vec<AgentDescriptor> {
    default_agents_with(&AdapterSearch::from_env())
}

/// [`default_agents`] with an explicit search (for tests and settings).
pub fn default_agents_with(search: &AdapterSearch) -> Vec<AgentDescriptor> {
    find_native_claude_adapter(search)
        .map(|p| native_claude_agent(&p))
        .into_iter()
        .chain(std::iter::once(npx_claude_agent()))
        .collect()
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

    fn temp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("eludite-acp-agents-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_exe(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(NATIVE_CLAUDE_ADAPTER);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    #[test]
    fn npx_adapter_is_the_fallback() {
        let root = temp_dir("fallback");
        let agents = default_agents_with(&AdapterSearch {
            configured: Some(root.join("missing")),
            exe_dir: Some(root.clone()),
            path: Some(root.clone().into()),
        });
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0], npx_claude_agent());
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
        assert!(default_agents_with(&AdapterSearch::default()) == vec![npx_claude_agent()]);
    }

    #[test]
    fn native_adapter_is_preferred_when_found() {
        let root = temp_dir("native");
        let on_path = make_exe(&root.join("pathdir"));
        let beside = make_exe(&root.join("ide"));
        let configured = make_exe(&root.join("configured"));
        let path = std::env::join_paths([root.join("empty"), root.join("pathdir")]).unwrap();
        let search = |configured: Option<PathBuf>, exe_dir: Option<PathBuf>| AdapterSearch {
            configured,
            exe_dir,
            path: Some(path.clone()),
        };

        // Order: configured path, beside the IDE, then PATH.
        let all = search(Some(configured.clone()), Some(root.join("ide")));
        assert_eq!(find_native_claude_adapter(&all), Some(configured.clone()));
        let beside_first = search(Some(root.join("missing")), Some(root.join("ide")));
        assert_eq!(
            find_native_claude_adapter(&beside_first),
            Some(beside.clone())
        );
        let path_only = search(None, Some(root.join("empty")));
        assert_eq!(
            find_native_claude_adapter(&path_only),
            Some(on_path.clone())
        );

        let agents = default_agents_with(&beside_first);
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].name, "Claude Code");
        assert_eq!(agents[0].command, beside.to_string_lossy());
        assert!(agents[0].args.is_empty());
        assert!(agents[0].env.is_empty(), "no key or token is passed");
        assert!(agents[0].env_remove.iter().any(|v| v == "CLAUDECODE"));
        assert_eq!(agents[1], npx_claude_agent(), "npx stays as the fallback");
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_files_are_skipped() {
        let root = temp_dir("noexec");
        std::fs::write(root.join(NATIVE_CLAUDE_ADAPTER), "x").unwrap();
        let s = AdapterSearch {
            configured: None,
            exe_dir: Some(root.clone()),
            path: Some(root.clone().into()),
        };
        assert_eq!(find_native_claude_adapter(&s), None);
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
            npx_claude_agent().terminal_auth_command(&m),
            "npx -y @agentclientprotocol/claude-agent-acp@0.85.0 --cli auth login --claudeai"
        );
        // The native adapter's login method is `auth login` on its own command.
        let native = protocol::AuthMethod {
            id: "claude-login".into(),
            name: "Log in to Claude Code".into(),
            description: None,
            kind: Some("terminal".into()),
            args: vec!["auth".into(), "login".into()],
        };
        assert_eq!(
            native_claude_agent(Path::new("/opt/eludite/eludite-claude-acp"))
                .terminal_auth_command(&native),
            "/opt/eludite/eludite-claude-acp auth login"
        );
    }
}
