//! ACP (Agent Client Protocol) client (PLAN.md D3, 5.2).
//!
//! Eludite hosts any ACP-speaking agent in its Agents tool window. Agents run as
//! child processes speaking JSON-RPC 2.0, newline-delimited, over stdio; Claude
//! Code (through its ACP adapter, [`default_agents`]) is the first one: the
//! native `eludite-claude-acp` adapter when it is installed (brief 0006), else
//! the Node adapter through `npx`.
//!
//! Public API: [`AgentDescriptor`], [`default_agents`] and
//! [`find_native_claude_adapter`] (what to launch), [`find_native_openai_adapter`],
//! [`provider_agent`], [`with_provider_key`] and [`provider_models_launch`] (an
//! OpenAI-compatible server run through `eludite-openai-acp`, brief 0060),
//! [`settings`] (the registry the Agents window offers: the built-in adapters,
//! the servers in `agents.json`'s `providers`, then the agents the user adds
//! there), [`session`] (one agent session with its lifecycle,
//! streaming, permission requests and cancellation, all on background threads),
//! [`AcpClient`] with [`ClientEvent`] (the connection underneath: `initialize`,
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
pub mod session;
pub mod settings;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use client::{AcpClient, AcpError, ClientEvent, EventSink};
pub use eludite_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};
pub use session::{
    AgentSession, AgentState, LoginMethod, PermissionPolicy, PolicyAnswer, SessionConfig,
    SessionEvent, SessionSink,
};
pub use settings::{AgentSettings, AgentSource, ProviderModel, ProviderSettings, RegisteredAgent};

/// How to launch an ACP agent. Its `Debug` hides the values of variables whose names hold `KEY`, `TOKEN`, `SECRET`
/// or `PASSWORD` (a provider's key rides in [`OPENAI_API_KEY_ENV`]).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl std::fmt::Debug for AgentDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env: Vec<(&str, &str)> = self
            .env
            .iter()
            .map(|(k, v)| {
                let upper = k.to_ascii_uppercase();
                let secret = ["KEY", "TOKEN", "SECRET", "PASSWORD"]
                    .iter()
                    .any(|s| upper.contains(s));
                (k.as_str(), if secret { "<redacted>" } else { v.as_str() })
            })
            .collect();
        f.debug_struct("AgentDescriptor")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env", &env)
            .field("env_remove", &self.env_remove)
            .finish()
    }
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

/// The OpenAI-compatible adapter's executable name (brief 0060): an ACP agent over any server that speaks the
/// OpenAI Chat Completions API, with the IDE's MCP tools as its only tools.
pub const NATIVE_OPENAI_ADAPTER: &str = if cfg!(windows) {
    "eludite-openai-acp.exe"
} else {
    "eludite-openai-acp"
};

/// Environment variable naming the OpenAI-compatible adapter explicitly.
pub const NATIVE_OPENAI_ADAPTER_ENV: &str = "ELUDITE_OPENAI_ACP";

/// The variable a provider agent reads its key from, set per launch from the credential store
/// ([`with_provider_key`]), never written to a file.
pub const OPENAI_API_KEY_ENV: &str = "ELUDITE_OPENAI_API_KEY";

/// Keys a provider agent never inherits from the IDE's environment: the person's shell key is not picked up
/// silently, and only the store's key reaches the agent.
pub const INHERITED_KEY_ENV: &[&str] = &["OPENAI_API_KEY", OPENAI_API_KEY_ENV];

/// The message of a provider entry when the adapter is not installed (the entry's state is `error`).
pub const OPENAI_ADAPTER_NOT_FOUND: &str = "eludite-openai-acp was not found";

/// Where to look for the native adapters: the configured path, the directory
/// of the IDE's own executable (where they ship), then `PATH`.
#[derive(Debug, Clone, Default)]
pub struct AdapterSearch {
    /// The Claude Code adapter's configured path (`$ELUDITE_CLAUDE_ACP` or `agents.claudeCodeAdapterPath`).
    pub configured: Option<PathBuf>,
    pub exe_dir: Option<PathBuf>,
    pub path: Option<OsString>,
    /// The OpenAI-compatible adapter's configured path (`$ELUDITE_OPENAI_ACP`).
    pub openai_configured: Option<PathBuf>,
}

impl AdapterSearch {
    /// From `$ELUDITE_CLAUDE_ACP`, `$ELUDITE_OPENAI_ACP`, the current executable and `$PATH`.
    pub fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            configured: var(NATIVE_CLAUDE_ADAPTER_ENV),
            openai_configured: var(NATIVE_OPENAI_ADAPTER_ENV),
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

/// `exe`: the configured path (if it exists), beside the IDE's executable, then on `PATH`.
fn find_adapter(
    search: &AdapterSearch,
    configured: Option<&PathBuf>,
    exe: &str,
) -> Option<PathBuf> {
    if let Some(p) = configured
        && is_executable_file(p)
    {
        return Some(p.clone());
    }
    if let Some(dir) = &search.exe_dir {
        let p = dir.join(exe);
        if is_executable_file(&p) {
            return Some(p);
        }
    }
    std::env::split_paths(search.path.as_deref()?)
        .map(|d| d.join(exe))
        .find(|p| is_executable_file(p))
}

/// Find the native adapter: the configured path (if it exists), beside the
/// IDE's executable, then on `PATH`.
pub fn find_native_claude_adapter(search: &AdapterSearch) -> Option<PathBuf> {
    find_adapter(search, search.configured.as_ref(), NATIVE_CLAUDE_ADAPTER)
}

/// Find `eludite-openai-acp` (brief 0060) like the Claude adapter: `openai_configured` (`$ELUDITE_OPENAI_ACP`),
/// beside the IDE's executable, then on `PATH`.
pub fn find_native_openai_adapter(search: &AdapterSearch) -> Option<PathBuf> {
    find_adapter(
        search,
        search.openai_configured.as_ref(),
        NATIVE_OPENAI_ADAPTER,
    )
}

/// The adapter's arguments for `provider`'s server: `--base-url`, `--header NAME=VALUE`, and `--catalog` (the
/// provider's `models` as inline JSON) when it has one.
fn provider_server_args(provider: &ProviderSettings) -> Vec<String> {
    let mut args = vec!["--base-url".to_owned(), provider.base_url.clone()];
    for (k, v) in &provider.headers {
        args.push("--header".into());
        args.push(format!("{k}={v}"));
    }
    if !provider.models.is_empty() {
        args.push("--catalog".into());
        args.push(serde_json::to_string(&provider.models).expect("models serialize"));
    }
    args
}

/// An OpenAI-compatible server as an agent: `adapter --base-url URL [--model M] [--header NAME=VALUE]... [--tools
/// core|all] [--catalog JSON]`. No key: [`with_provider_key`] adds it at launch. The inherited `OPENAI_API_KEY`
/// (and [`OPENAI_API_KEY_ENV`]) and a Claude Code session's variables are removed from its environment.
pub fn provider_agent(adapter: &Path, provider: &ProviderSettings) -> AgentDescriptor {
    let mut args = provider_server_args(provider);
    if let Some(m) = &provider.default_model {
        args.splice(2..2, ["--model".to_owned(), m.clone()]);
    }
    if let Some(t) = &provider.tools {
        args.push("--tools".into());
        args.push(t.clone());
    }
    AgentDescriptor {
        name: provider.name.clone(),
        command: adapter.to_string_lossy().into_owned(),
        args,
        env: Vec::new(),
        env_remove: provider_env_remove(),
    }
}

fn provider_env_remove() -> Vec<String> {
    CLAUDE_SESSION_ENV
        .iter()
        .chain(INHERITED_KEY_ENV)
        .map(|s| (*s).to_owned())
        .collect()
}

/// `eludite-openai-acp models ...` for `provider` (what `eludite.agents.provider_models` runs, off the UI thread):
/// it prints `agents-provider-models.output.json` on stdout. The key is added with [`with_provider_key`].
pub fn provider_models_launch(adapter: &Path, provider: &ProviderSettings) -> AgentDescriptor {
    let mut args = vec!["models".to_owned()];
    args.extend(provider_server_args(provider));
    AgentDescriptor {
        name: provider.name.clone(),
        command: adapter.to_string_lossy().into_owned(),
        args,
        env: Vec::new(),
        env_remove: provider_env_remove(),
    }
}

/// `launch` with `key` (from the credential store) in [`OPENAI_API_KEY_ENV`]; no key or an empty one: none set,
/// so the adapter sends no `Authorization` header.
pub fn with_provider_key(mut launch: AgentDescriptor, key: Option<&str>) -> AgentDescriptor {
    launch.env.retain(|(k, _)| k != OPENAI_API_KEY_ENV);
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        launch
            .env
            .push((OPENAI_API_KEY_ENV.to_owned(), k.to_owned()));
    }
    launch
}

/// Run a [`provider_models_launch`] and parse what it prints (`{models, listing, message?}`). Blocks for up to the
/// adapter's 30 s listing timeout: call it off the UI thread.
pub fn run_provider_models(launch: &AgentDescriptor) -> Result<serde_json::Value, String> {
    let mut cmd = std::process::Command::new(&launch.command);
    cmd.args(&launch.args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    for var in &launch.env_remove {
        cmd.env_remove(var);
    }
    for (k, v) in &launch.env {
        cmd.env(k, v);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("could not run {}: {e}", launch.command))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "{} failed: {}",
            launch.command,
            err.lines().next().unwrap_or("no output")
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("the model list is not JSON: {e}"))
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
        make_named(dir, NATIVE_CLAUDE_ADAPTER, "#!/bin/sh\n")
    }

    fn make_named(dir: &Path, name: &str, script: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, script).unwrap();
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
            openai_configured: None,
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
            openai_configured: None,
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
            openai_configured: None,
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
    fn llama() -> ProviderSettings {
        ProviderSettings {
            name: "llama.cpp".into(),
            base_url: "http://localhost:8080/v1".into(),
            default_model: Some("qwen3-8b".into()),
            headers: [("X-Title".to_owned(), "Eludite".to_owned())].into(),
            models: vec![ProviderModel {
                id: "qwen3-8b".into(),
                name: Some("Qwen3 8B".into()),
                context_window: Some(16_384),
            }],
            tools: Some("all".into()),
        }
    }

    #[test]
    fn the_openai_adapter_is_found_like_the_claude_one() {
        let root = temp_dir("openai");
        let beside = make_named(&root.join("ide"), NATIVE_OPENAI_ADAPTER, "#!/bin/sh\n");
        let configured = make_named(
            &root.join("configured"),
            NATIVE_OPENAI_ADAPTER,
            "#!/bin/sh\n",
        );
        let on_path = make_named(&root.join("pathdir"), NATIVE_OPENAI_ADAPTER, "#!/bin/sh\n");
        let mut s = AdapterSearch {
            configured: None,
            exe_dir: Some(root.join("ide")),
            path: Some(root.join("pathdir").into()),
            openai_configured: Some(configured.clone()),
        };
        assert_eq!(find_native_openai_adapter(&s), Some(configured));
        s.openai_configured = Some(root.join("missing"));
        assert_eq!(find_native_openai_adapter(&s), Some(beside));
        s.exe_dir = None;
        assert_eq!(find_native_openai_adapter(&s), Some(on_path));
        assert_eq!(
            find_native_claude_adapter(&s),
            None,
            "each adapter by its own name"
        );
        assert_eq!(find_native_openai_adapter(&AdapterSearch::default()), None);
    }

    #[test]
    fn a_provider_launches_the_adapter_with_its_arguments_and_key() {
        let d = provider_agent(Path::new("/opt/eludite/eludite-openai-acp"), &llama());
        assert_eq!(d.name, "llama.cpp");
        assert_eq!(
            d.args[..6],
            [
                "--base-url",
                "http://localhost:8080/v1",
                "--model",
                "qwen3-8b",
                "--header",
                "X-Title=Eludite"
            ]
        );
        assert_eq!(d.args[6], "--catalog");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&d.args[7]).unwrap(),
            serde_json::json!([{"id": "qwen3-8b", "name": "Qwen3 8B", "contextWindow": 16384}])
        );
        assert_eq!(d.args[8..], ["--tools", "all"]);
        assert!(d.env.is_empty(), "the key is added per launch");
        for v in ["OPENAI_API_KEY", OPENAI_API_KEY_ENV, "CLAUDECODE"] {
            assert!(d.env_remove.iter().any(|r| r == v), "{v}");
        }
        let keyed = with_provider_key(d.clone(), Some("sk-local-secret"));
        assert_eq!(
            keyed.env,
            [(OPENAI_API_KEY_ENV.to_owned(), "sk-local-secret".to_owned())]
        );
        assert!(
            !format!("{keyed:?}").contains("sk-local-secret"),
            "Debug hides the key"
        );
        assert!(with_provider_key(keyed, Some("")).env.is_empty());
        let plain = ProviderSettings {
            name: "ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            ..ProviderSettings::default()
        };
        assert_eq!(
            provider_agent(Path::new("a"), &plain).args,
            ["--base-url", "http://localhost:11434/v1"]
        );
        let m = provider_models_launch(Path::new("a"), &llama());
        assert_eq!(
            m.args[..3],
            ["models", "--base-url", "http://localhost:8080/v1"]
        );
        assert!(!m.args.iter().any(|a| a == "--model" || a == "--tools"));
    }

    #[cfg(unix)]
    #[test]
    fn provider_models_runs_the_adapter_and_reads_its_json() {
        let root = temp_dir("models");
        let script = format!(
            "#!/bin/sh\n[ \"$1\" = models ] || exit 2\n[ -z \"$OPENAI_API_KEY\" ] || exit 3\n\
             echo '{{\"models\":[{{\"id\":\"m\"}}],\"listing\":\"server\",\"key\":\"'\"${OPENAI_API_KEY_ENV}\"'\"}}'\n"
        );
        let exe = make_named(&root, NATIVE_OPENAI_ADAPTER, &script);
        let launch = with_provider_key(provider_models_launch(&exe, &llama()), Some("k1"));
        let v = run_provider_models(&launch).unwrap();
        assert_eq!(v["listing"], "server");
        assert_eq!(v["key"], "k1", "the key reaches the adapter's environment");
        let fail = make_named(
            &root.join("f"),
            NATIVE_OPENAI_ADAPTER,
            "#!/bin/sh\necho boom >&2\nexit 1\n",
        );
        let e = run_provider_models(&provider_models_launch(&fail, &llama())).unwrap_err();
        assert!(e.contains("boom"), "{e}");
    }
}
