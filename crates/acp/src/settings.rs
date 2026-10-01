//! The agent registry the Agents window offers: the built-in Claude Code adapters (native first, then npx, see
//! [`crate::default_agents`]) followed by the ACP agents the user adds in `agents.json`
//! (`protocol/schemas/agents-settings.json`) in Eludite's config directory.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{AdapterSearch, AgentDescriptor, CLAUDE_SESSION_ENV, default_agents_with};

/// The settings file's name in Eludite's config directory.
pub const SETTINGS_FILE: &str = "agents.json";

/// Where a registry entry came from (`agents-state.output.json`'s `source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    Native,
    Npx,
    Settings,
}

impl AgentSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentSource::Native => "native",
            AgentSource::Npx => "npx",
            AgentSource::Settings => "settings",
        }
    }
}

/// One agent the window can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredAgent {
    pub descriptor: AgentDescriptor,
    pub source: AgentSource,
}

impl RegisteredAgent {
    pub fn name(&self) -> &str {
        &self.descriptor.name
    }

    /// The command line, for display.
    pub fn command_line(&self) -> String {
        std::iter::once(self.descriptor.command.as_str())
            .chain(self.descriptor.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `agents.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<ConfiguredAgent>,
}

/// One user-added agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredAgent {
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl AgentSettings {
    /// Read `path`; a missing file is the empty settings, a malformed one an error naming the file.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// The built-in agents found by `search`, then the configured ones (a configured agent with a built-in's name
/// replaces it, so the user can override the adapter's command). With `settings.default` naming one, it moves
/// first.
pub fn registry(search: &AdapterSearch, settings: &AgentSettings) -> Vec<RegisteredAgent> {
    let mut out: Vec<RegisteredAgent> = default_agents_with(search)
        .into_iter()
        .map(|descriptor| RegisteredAgent {
            source: if descriptor.command == "npx" {
                AgentSource::Npx
            } else {
                AgentSource::Native
            },
            descriptor,
        })
        .collect();
    for c in &settings.agents {
        let entry = RegisteredAgent {
            descriptor: AgentDescriptor {
                name: c.name.clone(),
                command: c.command.clone(),
                args: c.args.clone(),
                env: c.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                // A Claude Code session's variables never leak into a hosted agent.
                env_remove: CLAUDE_SESSION_ENV.iter().map(|s| (*s).to_owned()).collect(),
            },
            source: AgentSource::Settings,
        };
        match out.iter_mut().find(|a| a.name() == c.name) {
            Some(existing) => *existing = entry,
            None => out.push(entry),
        }
    }
    if let Some(d) = &settings.default
        && let Some(ix) = out.iter().position(|a| a.name() == d)
    {
        let a = out.remove(ix);
        out.insert(0, a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_add_and_override_agents() {
        let dir = std::env::temp_dir().join(format!("eludite-acp-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE);
        assert_eq!(
            AgentSettings::load(&path).unwrap(),
            AgentSettings::default()
        );
        std::fs::write(
            &path,
            r#"{"default": "Gemini", "agents": [
                {"name": "Gemini", "command": "gemini", "args": ["--experimental-acp"], "env": {"A": "1"}},
                {"name": "Claude Code (Node adapter)", "command": "/opt/node/bin/npx", "args": ["-y", "x"]}
            ]}"#,
        )
        .unwrap();
        let settings = AgentSettings::load(&path).unwrap();
        let agents = registry(&AdapterSearch::default(), &settings);
        let names: Vec<_> = agents.iter().map(|a| a.name().to_owned()).collect();
        assert_eq!(names, ["Gemini", "Claude Code (Node adapter)"]);
        assert_eq!(agents[0].source, AgentSource::Settings);
        assert_eq!(agents[0].command_line(), "gemini --experimental-acp");
        assert_eq!(agents[0].descriptor.env, [("A".into(), "1".into())]);
        assert!(
            agents[0]
                .descriptor
                .env_remove
                .iter()
                .any(|v| v == "CLAUDECODE")
        );
        assert_eq!(
            agents[1].descriptor.command, "/opt/node/bin/npx",
            "overridden"
        );
        std::fs::write(&path, r#"{"agents": [{"name": "x"}]}"#).unwrap();
        assert!(
            AgentSettings::load(&path)
                .unwrap_err()
                .contains("agents.json")
        );
        let plain = registry(&AdapterSearch::default(), &AgentSettings::default());
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].source, AgentSource::Npx);
    }
}
