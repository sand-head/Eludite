//! The per-solution permission policy for hosted agents (PLAN.md 5.3; `protocol/schemas/agents-policy.json`): a
//! committable file, `.eludite/agents-policy.json` beside the solution, deciding what an agent may do without asking.
//!
//! - Class **read** always runs; it is not configurable.
//! - **edit_buffer**: `review` (the default) holds the agent's edits as pending changes; `accept` applies them at once.
//! - **execute**: `prompt` (the default), `allow` or `deny`, unless a rule matches.
//! - **dangerous**: `prompt` (the default) or `deny`, unless a rule matches.
//! - **Rules** name a tool (as the agent names it; an Eludite MCP tool also by its bare name) and optionally the start
//!   of its shell command. Always Allow in a permission prompt adds one, and the file is rewritten with sorted keys
//!   so it diffs cleanly in review.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PermissionClass;

/// The policy file, relative to the solution's folder.
pub const POLICY_FILE: &str = ".eludite/agents-policy.json";

/// The prefix Claude-style agents give MCP tools of the `eludite` server.
const MCP_PREFIX: &str = "mcp__eludite__";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditPolicy {
    #[default]
    Review,
    Accept,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutePolicy {
    #[default]
    Prompt,
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DangerousPolicy {
    #[default]
    Prompt,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    Allow,
    Deny,
}

/// One rule of the policy file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_prefix: Option<String>,
    pub decision: RuleDecision,
}

impl PolicyRule {
    fn matches(&self, tool: &str, input: &Value) -> bool {
        let named = self.tool == tool
            || tool
                .strip_prefix(MCP_PREFIX)
                .is_some_and(|bare| bare == self.tool)
            || self
                .tool
                .strip_prefix(MCP_PREFIX)
                .is_some_and(|bare| bare == tool);
        named
            && self.command_prefix.as_deref().is_none_or(|prefix| {
                shell_command(input).is_some_and(|c| c.trim_start().starts_with(prefix))
            })
    }
}

/// The shell command a tool call carries (`command`, as Claude's Bash and ACP terminals name it).
pub fn shell_command(input: &Value) -> Option<&str> {
    input.get("command").and_then(Value::as_str)
}

/// `agents-policy.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPolicy {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_buffer: Option<EditPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execute: Option<ExecutePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangerous: Option<DangerousPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PolicyRule>,
}

impl Default for AgentPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            edit_buffer: None,
            execute: None,
            dangerous: None,
            rules: Vec::new(),
        }
    }
}

/// What the policy says about one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Run it, with the reason shown in the transcript.
    Allow(String),
    Deny(String),
    /// Ask the user.
    Ask,
    /// An edit: hold it as a pending change for review.
    Review,
}

impl AgentPolicy {
    /// The policy file of the solution in `solution_dir`.
    pub fn path_for(solution_dir: &Path) -> PathBuf {
        solution_dir.join(POLICY_FILE)
    }

    /// Read `path`; a missing file is the defaults, a malformed or newer one an error naming the file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let p: Self =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if p.version != 1 {
            return Err(format!(
                "{}: version {} is not supported (1 is)",
                path.display(),
                p.version
            ));
        }
        Ok(p)
    }

    /// Write `path` (creating `.eludite/`) atomically: a temporary file beside it, then a rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Keys sorted, so the file diffs cleanly (whether or not serde_json preserves insertion order in this build).
        let value = sorted(serde_json::to_value(self).map_err(std::io::Error::other)?);
        let mut text = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
        text.push('\n');
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Decide a call of `tool` (as the agent names it) of class `class` with `input`.
    pub fn decide(&self, class: PermissionClass, tool: &str, input: &Value) -> Verdict {
        let rule = || self.rules.iter().find(|r| r.matches(tool, input));
        match class {
            PermissionClass::Read => Verdict::Allow(format!("{tool} is class read")),
            PermissionClass::EditBuffer => match self.edit_buffer.unwrap_or_default() {
                EditPolicy::Review => Verdict::Review,
                EditPolicy::Accept => Verdict::Allow("the policy accepts edits".into()),
            },
            PermissionClass::Execute | PermissionClass::Dangerous => {
                if let Some(r) = rule() {
                    let why = format!(
                        "the solution's policy rule for {}{}",
                        r.tool,
                        r.command_prefix
                            .as_deref()
                            .map(|p| format!(" `{p}`"))
                            .unwrap_or_default()
                    );
                    return match r.decision {
                        RuleDecision::Allow => Verdict::Allow(why),
                        RuleDecision::Deny => Verdict::Deny(why),
                    };
                }
                let class_name = class.as_str();
                let deny = || Verdict::Deny(format!("the solution's policy denies {class_name}"));
                if class == PermissionClass::Execute {
                    match self.execute.unwrap_or_default() {
                        ExecutePolicy::Prompt => Verdict::Ask,
                        ExecutePolicy::Allow => {
                            Verdict::Allow("the solution's policy allows execute".into())
                        }
                        ExecutePolicy::Deny => deny(),
                    }
                } else {
                    match self.dangerous.unwrap_or_default() {
                        DangerousPolicy::Prompt => Verdict::Ask,
                        DangerousPolicy::Deny => deny(),
                    }
                }
            }
        }
    }

    /// Always Allow: add a rule for this call (its exact shell command when it has one) unless one exists. Returns
    /// the rule.
    pub fn allow_always(&mut self, tool: &str, input: &Value) -> PolicyRule {
        let rule = PolicyRule {
            tool: tool.strip_prefix(MCP_PREFIX).unwrap_or(tool).to_owned(),
            command_prefix: shell_command(input).map(|c| c.trim().to_owned()),
            decision: RuleDecision::Allow,
        };
        if !self.rules.contains(&rule) {
            self.rules.push(rule.clone());
        }
        rule
    }
}

/// `v` with every object's keys inserted in sorted order.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCHEMA: &str = include_str!("../../../protocol/schemas/agents-policy.json");

    #[test]
    fn defaults_follow_plan_5_3() {
        let p = AgentPolicy::default();
        let none = json!({});
        assert!(matches!(
            p.decide(PermissionClass::Read, "x", &none),
            Verdict::Allow(_)
        ));
        assert_eq!(
            p.decide(PermissionClass::EditBuffer, "Write", &none),
            Verdict::Review
        );
        assert_eq!(
            p.decide(PermissionClass::Execute, "Bash", &none),
            Verdict::Ask
        );
        assert_eq!(
            p.decide(PermissionClass::Dangerous, "WebFetch", &none),
            Verdict::Ask
        );
    }

    #[test]
    fn rules_classes_and_always_allow_persist() {
        let dir = std::env::temp_dir().join(format!("eludite-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = AgentPolicy::path_for(&dir);
        assert_eq!(AgentPolicy::load(&path).unwrap(), AgentPolicy::default());

        let mut p = AgentPolicy::default();
        let build = json!({"command": "dotnet build Eludite.slnx"});
        let rule = p.allow_always("Bash", &build);
        assert_eq!(
            rule.command_prefix.as_deref(),
            Some("dotnet build Eludite.slnx")
        );
        p.allow_always("Bash", &build);
        assert_eq!(p.rules.len(), 1, "no duplicate rules");
        p.allow_always(
            "mcp__eludite__eludite-solution-open",
            &json!({"path": "/a.sln"}),
        );
        assert_eq!(p.rules[1].tool, "eludite-solution-open");
        p.save(&path).unwrap();

        let loaded = AgentPolicy::load(&path).unwrap();
        assert_eq!(loaded, p);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"));
        assert!(
            text.find("\"rules\"").unwrap() < text.find("\"version\"").unwrap(),
            "sorted keys"
        );
        // The file follows its schema (the subset this crate can check by hand).
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        for k in v.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }

        assert!(matches!(
            loaded.decide(PermissionClass::Execute, "Bash", &build),
            Verdict::Allow(r) if r.contains("dotnet build")
        ));
        assert_eq!(
            loaded.decide(
                PermissionClass::Execute,
                "Bash",
                &json!({"command": "rm -rf obj"})
            ),
            Verdict::Ask
        );
        assert!(matches!(
            loaded.decide(
                PermissionClass::Execute,
                "mcp__eludite__eludite-solution-open",
                &json!({})
            ),
            Verdict::Allow(_)
        ));

        std::fs::write(
            &path,
            r#"{"version": 1, "edit_buffer": "accept", "execute": "deny", "dangerous": "deny",
                "rules": [{"tool": "Bash", "command_prefix": "git push", "decision": "allow"}]}"#,
        )
        .unwrap();
        let strict = AgentPolicy::load(&path).unwrap();
        assert!(matches!(
            strict.decide(PermissionClass::EditBuffer, "Write", &json!({})),
            Verdict::Allow(_)
        ));
        assert!(matches!(
            strict.decide(PermissionClass::Execute, "Bash", &json!({"command": "ls"})),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            strict.decide(
                PermissionClass::Dangerous,
                "Bash",
                &json!({"command": "git push origin"})
            ),
            Verdict::Allow(_)
        ));
        std::fs::write(&path, r#"{"version": 2}"#).unwrap();
        assert!(AgentPolicy::load(&path).unwrap_err().contains("version 2"));
        std::fs::write(&path, r#"{"version": 1, "bogus": true}"#).unwrap();
        assert!(AgentPolicy::load(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
