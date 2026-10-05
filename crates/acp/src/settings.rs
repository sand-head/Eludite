//! The agent registry the Agents window offers: the built-in Claude Code adapters (native first, then npx, see
//! [`crate::default_agents`]), then the OpenAI-compatible servers in `agents.json`'s `providers`
//! ([`ProviderSettings`], each run through `eludite-openai-acp`, brief 0059), then the ACP agents the user adds in
//! `agents.json` (`protocol/schemas/agents-settings.json`) in Eludite's config directory. A provider's key is never
//! in the file: the credential store keeps it under [`provider_credential_key`].

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{
    AdapterSearch, AgentDescriptor, CLAUDE_SESSION_ENV, OPENAI_ADAPTER_NOT_FOUND,
    default_agents_with, find_native_openai_adapter, provider_agent,
};

/// The settings file's name in Eludite's config directory.
pub const SETTINGS_FILE: &str = "agents.json";

/// Where a registry entry came from (`agents-state.output.json`'s `source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    Native,
    Npx,
    /// An OpenAI-compatible server from `providers`, run through `eludite-openai-acp` (brief 0059).
    Provider,
    Settings,
}

impl AgentSource {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentSource::Native => "native",
            AgentSource::Npx => "npx",
            AgentSource::Provider => "provider",
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

    /// Why the entry cannot start, before trying: a provider whose adapter was not found
    /// ([`OPENAI_ADAPTER_NOT_FOUND`]; its descriptor's command is empty). The window shows it in the `error` state.
    pub fn launch_error(&self) -> Option<&'static str> {
        (self.source == AgentSource::Provider && self.descriptor.command.is_empty())
            .then_some(OPENAI_ADAPTER_NOT_FOUND)
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
    /// OpenAI-compatible servers (brief 0059).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderSettings>,
}

/// The credential store's key for a provider's API key: `provider:<name>`.
pub fn provider_credential_key(name: &str) -> String {
    format!("provider:{name}")
}

/// One model of a provider's catalog (`{id, name?, contextWindow?}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// One OpenAI-compatible server (`agents-settings.json`'s `providers`). Its key is in the credential store under
/// [`ProviderSettings::credential_key`], never here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSettings {
    pub name: String,
    /// The API root ending in the version segment (`http://localhost:8080/v1`).
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    /// Extra request headers; never `Authorization`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// The person's catalog, for a server that does not list its models.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ProviderModel>,
    /// `core` or `all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
}

impl ProviderSettings {
    /// `provider:<name>`.
    pub fn credential_key(&self) -> String {
        provider_credential_key(&self.name)
    }
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

    /// Write `path` (pretty JSON, through a temporary file and a rename), creating its directory.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())? + "\n";
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Add `provider`, or replace the one with its name (keeping its place). True when one was replaced.
    pub fn set_provider(&mut self, provider: ProviderSettings) -> bool {
        match self.providers.iter_mut().find(|p| p.name == provider.name) {
            Some(p) => {
                *p = provider;
                true
            }
            None => {
                self.providers.push(provider);
                false
            }
        }
    }

    /// Remove the provider named `name`.
    pub fn remove_provider(&mut self, name: &str) -> Option<ProviderSettings> {
        let ix = self.providers.iter().position(|p| p.name == name)?;
        Some(self.providers.remove(ix))
    }
}

/// The built-in agents found by `search`, then the providers (each through `eludite-openai-acp` found by `search`;
/// without it, an entry with an empty command and [`RegisteredAgent::launch_error`]), then the configured agents
/// (a provider or configured agent with an earlier entry's name replaces it, so the user can override the adapter's
/// command). With `settings.default` naming one, it moves first.
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
    let adapter = if settings.providers.is_empty() {
        None
    } else {
        find_native_openai_adapter(search)
    };
    for p in &settings.providers {
        let descriptor = match &adapter {
            Some(path) => provider_agent(path, p),
            None => AgentDescriptor {
                command: String::new(),
                ..provider_agent(Path::new(""), p)
            },
        };
        let entry = RegisteredAgent {
            descriptor,
            source: AgentSource::Provider,
        };
        match out.iter_mut().find(|a| a.name() == p.name) {
            Some(existing) => *existing = entry,
            None => out.push(entry),
        }
    }
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
    #[test]
    fn providers_come_after_the_built_ins_and_before_the_configured_agents() {
        let dir =
            std::env::temp_dir().join(format!("eludite-acp-providers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(SETTINGS_FILE);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"{"agents": [{"name": "Gemini", "command": "gemini"}],
                "providers": [{"name": "llama.cpp", "baseUrl": "http://localhost:8080/v1", "models": [{"id": "q", "contextWindow": 16384}]},
                              {"name": "OpenRouter", "baseUrl": "https://openrouter.ai/api/v1", "defaultModel": "x/y",
                               "headers": {"X-Title": "Eludite"}, "tools": "all"}]}"#,
        )
        .unwrap();
        let mut settings = AgentSettings::load(&path).unwrap();
        assert_eq!(settings.providers[0].models[0].context_window, Some(16_384));
        assert_eq!(
            settings.providers[1].credential_key(),
            "provider:OpenRouter"
        );

        // Without the adapter: the entries are there, in the error state's message.
        let agents = registry(&AdapterSearch::default(), &settings);
        let names: Vec<_> = agents.iter().map(|a| a.name().to_owned()).collect();
        assert_eq!(
            names,
            [
                "Claude Code (Node adapter)",
                "llama.cpp",
                "OpenRouter",
                "Gemini"
            ]
        );
        assert_eq!(agents[1].source, AgentSource::Provider);
        assert_eq!(
            agents[1].launch_error(),
            Some("eludite-openai-acp was not found")
        );
        assert_eq!(agents[3].launch_error(), None);
        assert_eq!(AgentSource::Provider.as_str(), "provider");

        // With it beside the executable: the launch line.
        let ide = dir.join("ide");
        std::fs::create_dir_all(&ide).unwrap();
        let exe = ide.join(crate::NATIVE_OPENAI_ADAPTER);
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let search = AdapterSearch {
            exe_dir: Some(ide),
            ..AdapterSearch::default()
        };
        let agents = registry(&search, &settings);
        assert_eq!(agents[2].launch_error(), None);
        assert_eq!(agents[2].descriptor.command, exe.to_string_lossy());
        assert_eq!(
            agents[2].descriptor.args,
            [
                "--base-url",
                "https://openrouter.ai/api/v1",
                "--model",
                "x/y",
                "--header",
                "X-Title=Eludite",
                "--tools",
                "all"
            ]
        );
        assert!(
            agents[2].descriptor.env.is_empty(),
            "no key in the registry"
        );
        assert!(
            agents[2]
                .descriptor
                .env_remove
                .iter()
                .any(|v| v == "OPENAI_API_KEY")
        );

        // Edits round-trip, and the key is never part of the file.
        let replaced = settings.set_provider(ProviderSettings {
            name: "llama.cpp".into(),
            base_url: "http://127.0.0.1:8081/v1".into(),
            ..ProviderSettings::default()
        });
        assert!(replaced);
        assert!(!settings.set_provider(ProviderSettings {
            name: "Ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            ..ProviderSettings::default()
        }));
        assert_eq!(
            settings.remove_provider("OpenRouter").unwrap().name,
            "OpenRouter"
        );
        assert!(settings.remove_provider("OpenRouter").is_none());
        settings.save(&path).unwrap();
        let again = AgentSettings::load(&path).unwrap();
        assert_eq!(again, settings);
        assert_eq!(again.providers[0].base_url, "http://127.0.0.1:8081/v1");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("apiKey") && text.contains("baseUrl"),
            "{text}"
        );
        std::fs::write(
            &path,
            r#"{"providers": [{"name": "x", "baseUrl": "http://h/v1", "apiKey": "k"}]}"#,
        )
        .unwrap();
        assert!(
            AgentSettings::load(&path).is_err(),
            "a key in the file is refused"
        );
    }
}
