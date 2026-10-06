//! Settings (brief 0020, PLAN.md 4.12): the settings schema and the `eludite.settings.get`, `eludite.settings.set`
//! and `eludite.tools.options` commands.
//!
//! [`SettingsSchema`] is read from `protocol/schemas/settings.json`, the schema of the settings file: every setting's
//! key, type, default, description, Options dialog section and label, and the environment variable that overrides
//! it. Defaults come from there and nowhere else, and the Options dialog is generated from it. The store itself (the
//! files, their merge and live reload) lives in the shell, which implements [`SettingsTarget`]; this module parses
//! the commands' input, validates a value against its setting, and serializes the outputs.
//!
//! Where a setting's effective value comes from is decided here too ([`SettingSpec::resolve`]), so the rule is in one
//! place: the environment variable, then the solution's file, then the user file, then the default; except that a
//! `user-workspace` setting (brief 0047's [`SettingScope::UserWorkspace`], `browser.allowNoSandbox`) is read from the
//! person's own state for the workspace instead of the solution's file, whose value of it is ignored and reported
//! ([`SettingsSchema::ignored_in_solution`]), so a committed file can never set it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

/// The Options page that is not generated from settings (brief 0048): the package sources of the NuGet.config chain,
/// edited through `eludite.nuget.sources` (Tools > NuGet Package Manager > Package Sources...).
pub const PACKAGE_SOURCES_PAGE: &str = "NuGet Package Manager > Package Sources";

pub const GET: &str = "eludite.settings.get";
pub const SET: &str = "eludite.settings.set";
/// Tools > Options: opens the dialog (the UI's; agents use `get` and `set`).
pub const OPTIONS: &str = "eludite.tools.options";

pub const ALL: [&str; 3] = [GET, SET, OPTIONS];

/// The settings file's schema (`protocol/schemas/settings.json`).
pub const FILE_SCHEMA: &str = include_str!("../../../protocol/schemas/settings.json");

/// How a setting's value is typed and edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    /// A string edited as text.
    Text,
    /// A string naming a file, edited with a path picker.
    Path,
    /// One of these strings, edited with a choice; the labels are what the dialog shows.
    Enum {
        values: Vec<String>,
        labels: Vec<String>,
    },
    /// A list of agent entries (edited in the file).
    List,
    /// A whole number from `min` to `max`, edited as text (brief 0040's `git.autoFetchMinutes`).
    Integer {
        min: i64,
        max: i64,
    },
}

/// One setting of the schema.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    pub key: String,
    pub kind: SettingKind,
    pub default: Value,
    pub description: String,
    pub section: String,
    pub label: String,
    /// The environment variable that overrides the files.
    pub env: Option<String>,
    /// Which file the Options dialog writes it in (`x-eludite-scope`): the user's, the workspace's for a setting that
    /// belongs to the workspace, or the person's own state for the workspace (brief 0047's `browser.allowNoSandbox`).
    pub scope: SettingScope,
    /// The item schema of a list (to validate entries).
    items: Option<Value>,
    max_items: Option<usize>,
}

impl SettingSpec {
    /// Check `value` against the setting's type.
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        let ok = match &self.kind {
            SettingKind::Bool => value.is_boolean(),
            SettingKind::Integer { min, max } => {
                value.as_i64().is_some_and(|n| (*min..=*max).contains(&n))
            }
            SettingKind::Text | SettingKind::Path => value.is_string(),
            SettingKind::Enum { values, .. } => value
                .as_str()
                .is_some_and(|v| values.iter().any(|x| x == v)),
            SettingKind::List => match value.as_array() {
                Some(items) => {
                    if let Some(max) = self.max_items
                        && items.len() > max
                    {
                        return Err(format!("{} takes at most {max} entries", self.key));
                    }
                    for (i, item) in items.iter().enumerate() {
                        if let Some(schema) = &self.items {
                            check_item(schema, item)
                                .map_err(|e| format!("{}[{i}]: {e}", self.key))?;
                        }
                    }
                    true
                }
                None => false,
            },
        };
        if ok {
            Ok(())
        } else {
            Err(format!(
                "{} takes {}, not {value}",
                self.key,
                match &self.kind {
                    SettingKind::Bool => "true or false".to_owned(),
                    SettingKind::Integer { min, max } =>
                        format!("a whole number from {min} to {max}"),
                    SettingKind::Text | SettingKind::Path => "a string".to_owned(),
                    SettingKind::Enum { values, .. } => format!("one of {}", values.join(", ")),
                    SettingKind::List => "a list".to_owned(),
                }
            ))
        }
    }

    /// The effective value and where it came from, given the setting's value in each place (each already read from
    /// its file; a value of the wrong type counts as absent). The environment variable wins, then the person's state
    /// for the workspace (only for a `user-workspace` setting) or the solution's file (for every other one), then
    /// the user file, then the default. A `user-workspace` setting never takes the solution file's value, and no other
    /// setting takes a value from the person's state for the workspace.
    pub fn resolve(&self, layers: SettingLayers<'_>) -> (Value, SettingSource) {
        if let Some(v) = layers.env.and_then(|t| self.parse_env(t)) {
            return (v, SettingSource::Environment);
        }
        let valid = |v: Option<&Value>| v.filter(|v| self.validate(v).is_ok()).cloned();
        let workspace = if self.scope == SettingScope::UserWorkspace {
            valid(layers.user_workspace).map(|v| (v, SettingSource::UserWorkspace))
        } else {
            valid(layers.solution).map(|v| (v, SettingSource::Solution))
        };
        workspace
            .or_else(|| valid(layers.user).map(|v| (v, SettingSource::User)))
            .unwrap_or_else(|| (self.default.clone(), SettingSource::Default))
    }

    /// The value from an environment variable's text (`1`/`true`/`0`/`false` for a switch), or `None` when it does
    /// not parse.
    pub fn parse_env(&self, text: &str) -> Option<Value> {
        match &self.kind {
            SettingKind::Bool => match text.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Some(Value::Bool(true)),
                "0" | "false" | "no" | "off" | "" => Some(Value::Bool(false)),
                _ => None,
            },
            SettingKind::Text | SettingKind::Path => Some(Value::String(text.to_owned())),
            SettingKind::Integer { .. } => text
                .trim()
                .parse::<i64>()
                .ok()
                .map(Value::from)
                .filter(|v| self.validate(v).is_ok()),
            SettingKind::Enum { values, .. } => values
                .iter()
                .any(|v| v == text)
                .then(|| Value::String(text.to_owned())),
            SettingKind::List => serde_json::from_str(text)
                .ok()
                .filter(|v| self.validate(v).is_ok()),
        }
    }
}

/// One setting's raw value in each place, for [`SettingSpec::resolve`]: the override variable's text and the
/// value in each file (absent: `None`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SettingLayers<'a> {
    pub env: Option<&'a str>,
    pub user: Option<&'a Value>,
    pub user_workspace: Option<&'a Value>,
    pub solution: Option<&'a Value>,
}

/// A small check of an object against an object schema: `required`, `additionalProperties: false`, enums, and string,
/// array-of-strings and string-map members.
/// A list entry: a string (search.excludes' globs, brief 0042) or an object.
fn check_item(schema: &Value, value: &Value) -> Result<(), String> {
    if schema["type"] == "string" {
        let min = schema["minLength"].as_u64().unwrap_or(0);
        return match value.as_str() {
            Some(s) if s.len() as u64 >= min => Ok(()),
            Some(_) => Err(format!("expected at least {min} characters")),
            None => Err("expected a string".into()),
        };
    }
    check_object(schema, value)
}

fn check_object(schema: &Value, value: &Value) -> Result<(), String> {
    let Some(obj) = value.as_object() else {
        return Err("expected an object".into());
    };
    for r in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !obj.contains_key(r) {
            return Err(format!("missing {r}"));
        }
    }
    let props = schema["properties"].as_object();
    for (k, v) in obj {
        let Some(p) = props.and_then(|p| p.get(k)) else {
            if schema["additionalProperties"] == Value::Bool(false) {
                return Err(format!("unknown member {k}"));
            }
            continue;
        };
        if let Some(values) = p["enum"].as_array()
            && !values.contains(v)
        {
            return Err(format!(
                "{k} is not one of {}",
                Value::Array(values.clone())
            ));
        }
        let ok = match p["type"].as_str() {
            Some("string") => v
                .as_str()
                .is_some_and(|s| s.len() as u64 >= p["minLength"].as_u64().unwrap_or(0)),
            Some("array") => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
            Some("object") => v
                .as_object()
                .is_some_and(|m| m.values().all(Value::is_string)),
            _ => true,
        };
        if !ok {
            return Err(format!("{k} has the wrong type"));
        }
    }
    Ok(())
}

/// The settings schema: the settings in file order and the Options dialog's sections in order.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsSchema {
    pub settings: Vec<SettingSpec>,
    pub sections: Vec<String>,
}

impl SettingsSchema {
    /// The schema checked in at `protocol/schemas/settings.json`.
    pub fn builtin() -> Self {
        Self::parse(FILE_SCHEMA).expect("protocol/schemas/settings.json is a valid settings schema")
    }

    /// Parse a settings file schema.
    pub fn parse(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        // serde_json keeps object order (the `preserve_order` feature is off), so take the key order from the text.
        let props = root["properties"].as_object().ok_or("no properties")?;
        let mut keys: Vec<&String> = props.keys().collect();
        keys.sort_by_key(|k| text.find(&format!("\"{k}\"")).unwrap_or(usize::MAX));
        let mut settings = Vec::new();
        for key in keys {
            let p = &props[key.as_str()];
            let str_of = |name: &str| p[name].as_str().map(str::to_owned);
            let kind = if let Some(values) = p["enum"].as_array() {
                let values: Vec<String> = values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                let labels = p["x-eludite-enum-labels"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(|| values.clone());
                SettingKind::Enum { values, labels }
            } else {
                match (p["type"].as_str(), p["x-eludite-editor"].as_str()) {
                    (Some("boolean"), _) => SettingKind::Bool,
                    (Some("integer"), _) => SettingKind::Integer {
                        min: p["minimum"].as_i64().unwrap_or(i64::MIN),
                        max: p["maximum"].as_i64().unwrap_or(i64::MAX),
                    },
                    (Some("string"), Some("path")) => SettingKind::Path,
                    (Some("string"), _) => SettingKind::Text,
                    (Some("array"), _) => SettingKind::List,
                    (t, _) => return Err(format!("{key}: unsupported type {t:?}")),
                }
            };
            let default = p
                .get("default")
                .cloned()
                .ok_or_else(|| format!("{key} has no default"))?;
            let spec = SettingSpec {
                key: key.clone(),
                kind,
                default,
                description: str_of("description")
                    .ok_or_else(|| format!("{key} has no description"))?,
                section: str_of("x-eludite-section")
                    .ok_or_else(|| format!("{key} has no section"))?,
                label: str_of("x-eludite-label").ok_or_else(|| format!("{key} has no label"))?,
                env: str_of("x-eludite-env"),
                scope: match p["x-eludite-scope"].as_str() {
                    None | Some("user") => SettingScope::User,
                    Some("solution") => SettingScope::Solution,
                    Some("user-workspace") => SettingScope::UserWorkspace,
                    Some(other) => return Err(format!("{key}: unknown scope {other}")),
                },
                items: p.get("items").cloned(),
                max_items: p["maxItems"].as_u64().map(|n| n as usize),
            };
            spec.validate(&spec.default)
                .map_err(|e| format!("default: {e}"))?;
            settings.push(spec);
        }
        let sections: Vec<String> = root["x-eludite-sections"]
            .as_array()
            .ok_or("no x-eludite-sections")?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        if let Some(s) = settings.iter().find(|s| !sections.contains(&s.section)) {
            return Err(format!("{}: section {} is not listed", s.key, s.section));
        }
        Ok(Self { settings, sections })
    }

    pub fn get(&self, key: &str) -> Option<&SettingSpec> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// The keys among `keys` (those of the solution's `.eludite/settings.json`) that are the person's own
    /// (`user-workspace`): ignored in that file and reported, sorted.
    pub fn ignored_in_solution<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a String>,
    ) -> Vec<String> {
        let mut ignored: Vec<String> = keys
            .into_iter()
            .filter(|k| {
                self.get(k)
                    .is_some_and(|s| s.scope == SettingScope::UserWorkspace)
            })
            .cloned()
            .collect();
        ignored.sort();
        ignored.dedup();
        ignored
    }

    /// The settings of one section, in file order.
    pub fn section<'a>(&'a self, section: &'a str) -> impl Iterator<Item = &'a SettingSpec> + 'a {
        self.settings.iter().filter(move |s| s.section == section)
    }
}

/// Where a setting's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    Default,
    User,
    Solution,
    /// The person's own state for the workspace (brief 0047).
    #[serde(rename = "user-workspace")]
    UserWorkspace,
    Environment,
}

/// Which file `eludite.settings.set` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingScope {
    #[default]
    User,
    Solution,
    /// The person's own state for the open workspace (`<config dir>/eludite/workspaces/<folder>-<hash>/settings.json`,
    /// brief 0047), for the settings whose `x-eludite-scope` is `user-workspace` only.
    #[serde(rename = "user-workspace")]
    UserWorkspace,
}

/// `settings-get.output.json`'s file members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsFileInfo {
    pub path: String,
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One setting in `settings-get.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingRow {
    pub key: String,
    pub value: Value,
    pub default: Value,
    pub source: SettingSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    pub section: String,
    pub label: String,
    pub description: String,
}

/// `settings-get.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsGetOutput {
    pub user_file: SettingsFileInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution_file: Option<SettingsFileInfo>,
    /// The person's state for the open workspace (brief 0047).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_workspace_file: Option<SettingsFileInfo>,
    pub settings: Vec<SettingRow>,
    /// `user-workspace` keys found in the solution's file: ignored there (brief 0047).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_keys: Vec<String>,
}

/// `settings-set.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSetOutput {
    pub key: String,
    pub scope: SettingScope,
    pub path: String,
    pub value: Value,
    pub source: SettingSource,
}

/// `tools-options.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionsOutput {
    pub section: String,
}

/// A parsed settings command.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsRequest {
    Get {
        key: Option<String>,
    },
    Set {
        key: String,
        value: Value,
        scope: SettingScope,
    },
    Options {
        section: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsOutput {
    Get(Box<SettingsGetOutput>),
    Set(SettingsSetOutput),
    Options(OptionsOutput),
}

impl SettingsOutput {
    pub fn to_json(&self) -> Value {
        match self {
            SettingsOutput::Get(o) => serde_json::to_value(o),
            SettingsOutput::Set(o) => serde_json::to_value(o),
            SettingsOutput::Options(o) => serde_json::to_value(o),
        }
        .expect("settings outputs serialize")
    }
}

/// What the shell implements: the store for `get` and `set` (any thread), and the dialog for `options`.
pub trait SettingsTarget: Send + Sync {
    fn apply(&self, request: SettingsRequest) -> Result<SettingsOutput, CommandError>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetInput {
    key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetInput {
    key: String,
    value: Value,
    /// Omitted: the setting's own scope (`x-eludite-scope`).
    scope: Option<SettingScope>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionsInput {
    section: Option<String>,
}

fn input<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, CommandError> {
    let input = if input.is_null() {
        Value::Object(Default::default())
    } else {
        input
    };
    serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

/// Parse a settings command's input. A `set` must name a known key with a value of its type (or null), in a file
/// that may hold it: without a scope, the setting's own; a `user-workspace` setting is never written in the solution's
/// file, where it would be ignored (null, which removes it from there, is allowed), and nothing else is written in
/// the person's state for the workspace.
pub fn parse(
    id: &str,
    value: Value,
    schema: &SettingsSchema,
) -> Result<SettingsRequest, CommandError> {
    let known = |key: &str| {
        schema.get(key).ok_or_else(|| {
            CommandError::InvalidInput(format!("{key} is not a setting (see eludite.settings.get)"))
        })
    };
    match id {
        GET => {
            let i: GetInput = input(value)?;
            if let Some(k) = &i.key {
                known(k)?;
            }
            Ok(SettingsRequest::Get { key: i.key })
        }
        SET => {
            let i: SetInput = input(value)?;
            let spec = known(&i.key)?;
            if !i.value.is_null() {
                spec.validate(&i.value)
                    .map_err(CommandError::InvalidInput)?;
            }
            let scope = i.scope.unwrap_or(spec.scope);
            let per_person = spec.scope == SettingScope::UserWorkspace;
            if per_person && scope == SettingScope::Solution && !i.value.is_null() {
                return Err(CommandError::InvalidInput(format!(
                    "{} is the person's own, kept in their state for the workspace (scope user-workspace, the \
                     default for it): a value in the workspace's .eludite/settings.json is ignored",
                    i.key
                )));
            }
            if !per_person && scope == SettingScope::UserWorkspace {
                return Err(CommandError::InvalidInput(format!(
                    "{} is not kept in the person's state for the workspace (scope user or solution)",
                    i.key
                )));
            }
            Ok(SettingsRequest::Set {
                key: i.key,
                value: i.value,
                scope,
            })
        }
        OPTIONS => {
            let i: OptionsInput = input(value)?;
            if let Some(s) = &i.section
                && !schema.sections.contains(s)
                && s != PACKAGE_SOURCES_PAGE
            {
                return Err(CommandError::InvalidInput(format!(
                    "no Options page {s}; the pages are {}",
                    schema.sections.join(", ")
                )));
            }
            Ok(SettingsRequest::Options { section: i.section })
        }
        other => Err(CommandError::InvalidInput(format!(
            "not a settings command: {other}"
        ))),
    }
}

pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission, agent_visible) = match id {
        GET => (
            "Settings: Get",
            include_str!("../../../protocol/schemas/settings-get.input.json"),
            include_str!("../../../protocol/schemas/settings-get.output.json"),
            PermissionClass::Read,
            true,
        ),
        // Changing a setting changes how the IDE builds and runs: the edit class, as the brief asks.
        SET => (
            "Settings: Set",
            include_str!("../../../protocol/schemas/settings-set.input.json"),
            include_str!("../../../protocol/schemas/settings-set.output.json"),
            PermissionClass::EditBuffer,
            true,
        ),
        OPTIONS => (
            "Tools: Options",
            include_str!("../../../protocol/schemas/tools-options.input.json"),
            include_str!("../../../protocol/schemas/tools-options.output.json"),
            PermissionClass::Read,
            false,
        ),
        other => unreachable!("not a settings command: {other}"),
    };
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible,
    }
}

/// Register the settings commands on `registry`, applied by `target`: `get` and `set` always, `options` when the
/// caller has the dialog (`with_options`).
pub fn register(
    registry: &CommandRegistry,
    schema: Arc<SettingsSchema>,
    target: Arc<dyn SettingsTarget>,
    with_options: bool,
) {
    for id in ALL.into_iter().filter(|id| with_options || *id != OPTIONS) {
        let target = target.clone();
        let schema = schema.clone();
        registry.replace(spec(id), move |value| {
            let request = parse(id, value, &schema)?;
            target.apply(request).map(|o| o.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_checked_in_schema_gives_every_setting_a_default_a_section_and_a_label() {
        let s = SettingsSchema::builtin();
        let keys: Vec<&str> = s.settings.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "keyboard.preset",
                "updates.channel",
                "updates.mode",
                "build.beforeRun",
                "build.onSave",
                "build.showOutputOnStart",
                "build.showErrorListOnFailure",
                "build.cargoPath",
                "debugger.netcoredbgPath",
                "debugger.monoPrefix",
                "debugger.monoAdapterPath",
                "debugger.lldbDapPath",
                "debugger.rustFormatters",
                "debugger.allowAgentsByDefault",
                "debugger.launchBrowser",
                "debugger.attachBrowser",
                "debugger.nodePath",
                "debugger.jsDebugPath",
                "languageServers.rustAnalyzerPath",
                "editor.codeLens",
                "editor.codeLens.references",
                "editor.codeLens.tests",
                "editor.languages.csharp.codeLens",
                "editor.languages.rust.codeLens",
                "editor.languages.typescript.codeLens",
                "editor.languages.javascript.codeLens",
                "editor.languages.vb.codeLens",
                "editor.languages.fsharp.codeLens",
                "test.runSettings",
                "test.parallel",
                "test.vstestConsolePath",
                "git.enabled",
                "git.autoFetchMinutes",
                "git.userName",
                "git.userEmail",
                "git.gpgSign",
                "agents.default",
                "agents.claudeCodeAdapterPath",
                "agents.custom",
                "browser.engine",
                "browser.enginePath",
                "browser.allowNoSandbox",
                "browser.useBuiltIn",
                "browser.homePage",
                "browser.showDevToolsTab",
                "browser.chromePath",
                "browser.headless",
                "browser.viewport",
                "terminal.defaultProfile",
                "terminal.profiles",
                "terminal.fontSize",
                "terminal.scrollback",
                "terminal.copyOnSelect",
                "terminal.bell",
                "terminal.inheritToolPaths",
                "terminal.shellIntegration",
                "search.excludes",
                "search.useGitignore",
                "search.maxFileSize",
                "search.followSymlinks",
                "forge.refreshSeconds",
                "forge.hosts",
                "forge.githubClientId",
                "forge.gitlabApplicationId",
                "forge.azureApplicationId",
                "nuget.includePrerelease",
                "nuget.restoreOnChange",
                "nuget.lockFiles",
                "editor.formatter",
                "editor.formatOnSave.css",
                "editor.emmet",
                "editor.formatOnSave.html",
                "languageServers.nodePath",
                "languageServers.typescriptPath",
                "languageServers.eslint",
                "editor.formatOnSave.typescript",
                "editor.formatOnSave.json",
            ]
        );
        // Brief 0052: CodeLens, with the text editor's pages; on unless the settings say otherwise. Brief 0057 adds
        // Visual Basic and F#.
        assert_eq!(
            s.section("Text Editor > All Languages > CodeLens").count(),
            9
        );
        assert_eq!(
            s.get("editor.languages.fsharp.codeLens").unwrap().default,
            json!("default")
        );
        assert_eq!(
            s.get("editor.languages.vb.codeLens").unwrap().label,
            "Visual Basic"
        );
        assert_eq!(s.get("editor.codeLens").unwrap().default, json!(true));
        let csharp = s.get("editor.languages.csharp.codeLens").unwrap();
        assert_eq!(csharp.default, json!("default"));
        assert!(csharp.validate(&json!("references")).is_ok());
        assert!(csharp.validate(&json!("sometimes")).is_err());
        // Brief 0048: NuGet Package Manager > General, last.
        // Brief 0050: the web languages' pages, after the earlier ones, so they keep their places.
        assert_eq!(
            s.sections[s.sections.len() - 5..],
            [
                "Text Editor > All Languages",
                "Text Editor > CSS",
                "Text Editor > HTML",
                "Text Editor > JavaScript/TypeScript",
                "Text Editor > JSON"
            ]
        );
        assert_eq!(s.section("Text Editor > JavaScript/TypeScript").count(), 4);
        let eslint = s.get("languageServers.eslint").unwrap();
        assert_eq!(eslint.default, json!("auto"));
        assert!(eslint.validate(&json!("off")).is_ok());
        assert!(eslint.validate(&json!("sometimes")).is_err());
        let formatter = s.get("editor.formatter").unwrap();
        assert_eq!(formatter.default, json!("auto"));
        assert!(formatter.validate(&json!("biome")).is_ok());
        assert_eq!(s.get("editor.emmet").unwrap().default, json!(true));
        assert_eq!(
            s.get("languageServers.nodePath").unwrap().env.as_deref(),
            Some("ELUDITE_NODE")
        );
        // Brief 0048: NuGet Package Manager > General, the last before brief 0050's pages.
        assert_eq!(
            s.sections.iter().rev().nth(5).map(String::as_str),
            Some("NuGet Package Manager > General")
        );
        assert_eq!(s.section("NuGet Package Manager > General").count(), 3);
        assert_eq!(s.get("nuget.restoreOnChange").unwrap().default, json!(true));
        assert_eq!(
            s.get("nuget.includePrerelease").unwrap().default,
            json!(false)
        );
        let lock = s.get("nuget.lockFiles").unwrap();
        assert_eq!(lock.default, json!("respect"));
        assert!(lock.validate(&json!("ignore")).is_ok());
        assert!(lock.validate(&json!("strict")).is_err());
        // Brief 0046: the forges' page, before NuGet's.
        assert_eq!(
            s.sections.iter().rev().nth(6).map(String::as_str),
            Some("Source Control > Forges")
        );
        assert_eq!(s.section("Source Control > Forges").count(), 5);
        let hosts = s.get("forge.hosts").unwrap();
        assert_eq!(hosts.kind, SettingKind::List);
        assert!(
            hosts
                .validate(&json!([{"host": "git.corp.example", "family": "github"}]))
                .is_ok()
        );
        assert!(
            hosts
                .validate(&json!([{"host": "git.corp.example", "family": "bitbucket"}]))
                .is_err()
        );
        assert_eq!(
            s.get("forge.refreshSeconds").unwrap().kind,
            SettingKind::Integer { min: 0, max: 3600 }
        );
        // Brief 0041: the Terminal page, after the earlier ones, so they keep their places.
        assert_eq!(s.section("Terminal").count(), 8);
        // Brief 0042: Find and Replace, after the earlier pages (brief 0055's Updates page sits right after it).
        assert_eq!(
            s.sections.iter().rev().nth(8).map(String::as_str),
            Some("Environment > Find and Replace")
        );
        assert_eq!(
            s.sections.iter().rev().nth(7).map(String::as_str),
            Some("Environment > Updates")
        );
        assert_eq!(s.section("Environment > Updates").count(), 2);
        assert_eq!(s.get("updates.mode").unwrap().default, json!("ask"));
        assert!(
            s.get("updates.mode")
                .unwrap()
                .validate(&json!("download"))
                .is_ok()
        );
        assert!(
            s.get("updates.mode")
                .unwrap()
                .validate(&json!("always"))
                .is_err()
        );
        assert_eq!(s.get("updates.channel").unwrap().default, json!("unstable"));
        assert!(
            s.get("updates.channel")
                .unwrap()
                .validate(&json!("stable"))
                .is_err()
        );
        assert_eq!(s.section("Environment > Find and Replace").count(), 4);
        assert_eq!(s.get("search.excludes").unwrap().kind, SettingKind::List);
        assert_eq!(
            s.get("search.excludes").unwrap().default,
            json!([
                "**/bin/**",
                "**/obj/**",
                "**/node_modules/**",
                "**/target/**",
                "**/.git/**"
            ])
        );
        let excludes = s.get("search.excludes").unwrap();
        assert!(excludes.validate(&json!(["**/Migrations/**"])).is_ok());
        assert!(excludes.validate(&json!([1])).is_err());
        assert!(excludes.validate(&json!([""])).is_err());
        assert_eq!(s.get("search.useGitignore").unwrap().default, json!(true));
        assert_eq!(s.get("search.maxFileSize").unwrap().default, json!(4));
        assert_eq!(
            s.get("search.followSymlinks").unwrap().default,
            json!(false)
        );
        assert_eq!(s.get("terminal.scrollback").unwrap().default, json!(10_000));
        assert_eq!(
            s.get("terminal.inheritToolPaths").unwrap().default,
            json!(true)
        );
        assert_eq!(
            s.get("terminal.shellIntegration").unwrap().default,
            json!(true)
        );
        assert_eq!(
            s.get("terminal.copyOnSelect").unwrap().default,
            json!(false)
        );
        assert_eq!(s.get("terminal.profiles").unwrap().kind, SettingKind::List);
        assert_eq!(s.get("build.onSave").unwrap().default, json!(false));
        assert_eq!(s.get("build.beforeRun").unwrap().default, json!(true));
        // Brief 0039: the opt-in is off by default; brief 0047: it is the person's, kept in their state for the
        // workspace, and its label says so. The engine's path is in the user's file.
        let allow = s.get("browser.allowNoSandbox").unwrap();
        assert_eq!(allow.default, json!(false));
        assert_eq!(allow.kind, SettingKind::Bool);
        assert_eq!(allow.scope, SettingScope::UserWorkspace);
        assert_eq!(allow.section, "Web Browser");
        assert!(
            allow.label.contains("for this workspace, on this machine"),
            "{}",
            allow.label
        );
        let engine_path = s.get("browser.enginePath").unwrap();
        assert_eq!(engine_path.kind, SettingKind::Path);
        assert_eq!(engine_path.scope, SettingScope::User);
        assert_eq!(
            engine_path.env, None,
            "the setting comes before ELUDITE_CHROMIUM"
        );
        assert!(
            s.settings
                .iter()
                .filter(|x| x.key != "browser.allowNoSandbox")
                .all(|x| x.scope == SettingScope::User)
        );
        // Brief 0037: F5 opens a web project's page, in the Web Browser window, by default.
        assert_eq!(s.get("browser.useBuiltIn").unwrap().default, json!(true));
        assert_eq!(
            s.get("debugger.launchBrowser").unwrap().default,
            json!(true)
        );
        assert_eq!(
            s.get("build.onSave").unwrap().env.as_deref(),
            Some("ELUDITE_BUILD_ON_SAVE")
        );
        assert_eq!(
            s.get("debugger.netcoredbgPath").unwrap().kind,
            SettingKind::Path
        );
        assert_eq!(s.get("agents.default").unwrap().kind, SettingKind::Text);
        assert_eq!(
            s.get("keyboard.preset").unwrap().kind,
            SettingKind::Enum {
                values: vec!["visualStudio".into()],
                labels: vec!["Visual Studio".into()]
            }
        );
        assert_eq!(s.sections[0], "Environment > Keyboard");
        assert!(
            s.settings
                .iter()
                .all(|x| !x.description.is_empty() && !x.label.is_empty())
        );
        assert_eq!(s.section("Debugging > General").count(), 10);
        // Brief 0038: F5 on a web project debugs its page too, by default; js-debug and Node are located.
        assert_eq!(
            s.get("debugger.attachBrowser").unwrap().default,
            json!(true)
        );
        assert_eq!(
            s.get("debugger.jsDebugPath").unwrap().env.as_deref(),
            Some("ELUDITE_JS_DEBUG")
        );
        assert_eq!(
            s.get("debugger.nodePath").unwrap().env.as_deref(),
            Some("ELUDITE_NODE")
        );
        assert_eq!(s.get("debugger.nodePath").unwrap().kind, SettingKind::Path);
        assert_eq!(
            s.get("debugger.allowAgentsByDefault").unwrap().default,
            json!(true)
        );
        assert_eq!(
            s.get("debugger.lldbDapPath").unwrap().env.as_deref(),
            Some("ELUDITE_LLDB_DAP")
        );
        assert_eq!(
            s.get("debugger.rustFormatters").unwrap().default,
            json!(true)
        );
        assert_eq!(
            s.get("debugger.monoPrefix").unwrap().env.as_deref(),
            Some("ELUDITE_MONO_PREFIX")
        );
        assert_eq!(
            s.get("debugger.monoAdapterPath").unwrap().env.as_deref(),
            Some("ELUDITE_DBG_MONO")
        );
    }

    #[test]
    fn values_are_checked_against_their_setting() {
        let s = SettingsSchema::builtin();
        let ok = |id, v: Value| parse(id, v, &s);
        assert_eq!(
            ok(SET, json!({"key": "build.onSave", "value": true})).unwrap(),
            SettingsRequest::Set {
                key: "build.onSave".into(),
                value: json!(true),
                scope: SettingScope::User
            }
        );
        assert!(ok(SET, json!({"key": "build.onSave", "value": "yes"})).is_err());
        assert!(ok(SET, json!({"key": "nope", "value": 1})).is_err());
        assert!(ok(SET, json!({"key": "keyboard.preset", "value": "emacs"})).is_err());
        // A whole number in its range (brief 0040).
        assert!(ok(SET, json!({"key": "git.autoFetchMinutes", "value": 15})).is_ok());
        assert!(ok(SET, json!({"key": "git.autoFetchMinutes", "value": -1})).is_err());
        assert!(ok(SET, json!({"key": "git.autoFetchMinutes", "value": "15"})).is_err());
        assert!(ok(SET, json!({"key": "git.autoFetchMinutes", "value": 1.5})).is_err());
        let minutes = s.get("git.autoFetchMinutes").unwrap();
        assert_eq!(minutes.default, json!(0));
        assert_eq!(minutes.parse_env(" 30 "), Some(json!(30)));
        assert_eq!(minutes.parse_env("2000"), None);
        // null removes the key from the file.
        assert!(
            ok(
                SET,
                json!({"key": "keyboard.preset", "value": null, "scope": "solution"})
            )
            .is_ok()
        );
        assert!(
            ok(SET, json!({"key": "build.onSave"})).is_err(),
            "value is required"
        );
        let agents =
            json!([{"name": "Gemini", "command": "gemini", "args": ["--acp"], "env": {"A": "1"}}]);
        assert!(ok(SET, json!({"key": "agents.custom", "value": agents})).is_ok());
        assert!(
            ok(
                SET,
                json!({"key": "agents.custom", "value": [{"name": "x"}]})
            )
            .is_err()
        );
        assert!(
            ok(
                SET,
                json!({"key": "agents.custom", "value": [{"name": "x", "command": "y", "z": 1}]})
            )
            .is_err()
        );
        assert_eq!(
            ok(GET, Value::Null).unwrap(),
            SettingsRequest::Get { key: None }
        );
        assert!(ok(GET, json!({"key": "nope"})).is_err());
        // Terminal profiles: a name and a command, args, env and a folder (brief 0041).
        let profiles = json!([{"name": "nu", "command": "nu", "args": ["-l"], "env": {"A": "1"}, "cwd": "src"}]);
        assert!(ok(SET, json!({"key": "terminal.profiles", "value": profiles})).is_ok());
        assert!(
            ok(
                SET,
                json!({"key": "terminal.profiles", "value": [{"name": "nu"}]})
            )
            .is_err()
        );
        assert!(ok(SET, json!({"key": "terminal.bell", "value": "audible"})).is_err());
        assert!(ok(SET, json!({"key": "terminal.fontSize", "value": 5})).is_err());
        assert!(ok(OPTIONS, json!({"section": "Debugging > General"})).is_ok());
        assert!(ok(OPTIONS, json!({"section": "Fonts and Colors"})).is_err());
        let b = s.get("build.onSave").unwrap();
        assert_eq!(b.parse_env("1"), Some(json!(true)));
        assert_eq!(b.parse_env("0"), Some(json!(false)));
        assert_eq!(b.parse_env("maybe"), None);
    }

    #[test]
    fn specs_follow_the_schemas() {
        for id in ALL {
            let spec = spec(id);
            assert_eq!(spec.input_schema["title"], format!("{id} input"));
            assert_eq!(spec.output_schema["title"], format!("{id} output"));
        }
        assert_eq!(spec(SET).permission, PermissionClass::EditBuffer);
        assert!(spec(GET).agent_visible && !spec(OPTIONS).agent_visible);
        // Outputs serialize to their schemas' members.
        let out = SettingsGetOutput {
            user_file: SettingsFileInfo {
                path: "/c/settings.json".into(),
                exists: false,
                error: None,
            },
            solution_file: None,
            user_workspace_file: Some(SettingsFileInfo {
                path: "/c/workspaces/App-0123456789abcdef/settings.json".into(),
                exists: true,
                error: None,
            }),
            settings: vec![SettingRow {
                key: "build.onSave".into(),
                value: json!(true),
                default: json!(false),
                source: SettingSource::Environment,
                env: Some("ELUDITE_BUILD_ON_SAVE".into()),
                section: "s".into(),
                label: "l".into(),
                description: "d".into(),
            }],
            ignored_keys: vec!["browser.allowNoSandbox".into()],
            unknown_keys: vec![],
        };
        let v = SettingsOutput::Get(Box::new(out)).to_json();
        let schema: Value = serde_json::from_str(include_str!(
            "../../../protocol/schemas/settings-get.output.json"
        ))
        .unwrap();
        for k in v.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        let row_props = &schema["properties"]["settings"]["items"]["properties"];
        for k in v["settings"][0].as_object().unwrap().keys() {
            assert!(row_props.get(k).is_some(), "{k}");
        }
        assert_eq!(v["settings"][0]["source"], "environment");
        // Brief 0047: the new scope and source spell as the schemas do.
        let enum_of = |schema: &Value| -> Vec<String> {
            schema["enum"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_owned())
                .collect()
        };
        assert!(
            enum_of(&row_props["source"]).contains(&"user-workspace".to_owned()),
            "settings-get's source"
        );
        assert_eq!(
            serde_json::to_value(SettingSource::UserWorkspace).unwrap(),
            "user-workspace"
        );
        let set_out: Value = serde_json::from_str(include_str!(
            "../../../protocol/schemas/settings-set.output.json"
        ))
        .unwrap();
        let set_in: Value = serde_json::from_str(include_str!(
            "../../../protocol/schemas/settings-set.input.json"
        ))
        .unwrap();
        for scope in [
            SettingScope::User,
            SettingScope::Solution,
            SettingScope::UserWorkspace,
        ] {
            let name = serde_json::to_value(scope).unwrap();
            let name = name.as_str().unwrap().to_owned();
            assert!(
                enum_of(&set_out["properties"]["scope"]).contains(&name),
                "{name}"
            );
            assert!(
                enum_of(&set_in["properties"]["scope"]).contains(&name),
                "{name}"
            );
        }
        assert!(enum_of(&set_out["properties"]["source"]).contains(&"user-workspace".to_owned()));
    }

    /// Brief 0047: a `user-workspace` setting is read from the person's state for the workspace, then the user file,
    /// then its default; the solution's file never sets it (its value there is reported ignored). Every other setting
    /// keeps brief 0020's order (environment, solution, user, default) and never reads the person's workspace state.
    #[test]
    fn a_user_workspace_setting_ignores_the_solution_file_and_merges_after_the_user_file() {
        let s = SettingsSchema::builtin();
        let allow = s.get("browser.allowNoSandbox").unwrap();
        let (t, f, yes) = (json!(true), json!(false), json!("yes"));
        let layers = |user, user_workspace, solution| SettingLayers {
            env: None,
            user,
            user_workspace,
            solution,
        };
        // The workspace's file alone: ignored, the default applies.
        assert_eq!(
            allow.resolve(layers(None, None, Some(&t))),
            (json!(false), SettingSource::Default)
        );
        // The user file under the person's workspace state; the solution file never counts.
        assert_eq!(
            allow.resolve(layers(Some(&t), None, Some(&f))),
            (json!(true), SettingSource::User)
        );
        assert_eq!(
            allow.resolve(layers(Some(&f), None, Some(&t))),
            (json!(false), SettingSource::User)
        );
        assert_eq!(
            allow.resolve(layers(Some(&f), Some(&t), Some(&f))),
            (json!(true), SettingSource::UserWorkspace)
        );
        assert_eq!(
            allow.resolve(layers(Some(&t), Some(&f), Some(&t))),
            (json!(false), SettingSource::UserWorkspace)
        );
        // A value of the wrong type in the state counts as absent.
        assert_eq!(
            allow.resolve(layers(None, Some(&yes), None)),
            (json!(false), SettingSource::Default)
        );
        // Another setting: the solution file wins over the user's, and the person's workspace state is not read.
        let on_save = s.get("build.onSave").unwrap();
        assert_eq!(
            on_save.resolve(layers(Some(&f), Some(&f), Some(&t))),
            (json!(true), SettingSource::Solution)
        );
        assert_eq!(
            on_save.resolve(layers(None, Some(&t), None)),
            (json!(false), SettingSource::Default)
        );
        assert_eq!(
            on_save.resolve(SettingLayers {
                env: Some("0"),
                ..layers(Some(&t), None, Some(&t))
            }),
            (json!(false), SettingSource::Environment)
        );
        // The report: only the per-person keys of the solution file, sorted, once each.
        let keys: Vec<String> = [
            "build.onSave",
            "browser.allowNoSandbox",
            "nope",
            "browser.allowNoSandbox",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(s.ignored_in_solution(&keys), ["browser.allowNoSandbox"]);
        assert!(s.ignored_in_solution(&keys[..1]).is_empty());
    }

    /// Brief 0047: `eludite.settings.set` without a scope writes a setting in its own file; the per-person opt-in is
    /// never written in the solution's file (removing it from there is allowed), and nothing else goes in the person's
    /// workspace state.
    #[test]
    fn set_writes_a_user_workspace_setting_only_in_the_persons_state() {
        let s = SettingsSchema::builtin();
        let set = |v: Value| parse(SET, v, &s);
        assert_eq!(
            set(json!({"key": "browser.allowNoSandbox", "value": true})).unwrap(),
            SettingsRequest::Set {
                key: "browser.allowNoSandbox".into(),
                value: json!(true),
                scope: SettingScope::UserWorkspace
            }
        );
        assert!(
            set(json!({"key": "browser.allowNoSandbox", "value": true, "scope": "user-workspace"}))
                .is_ok()
        );
        let refused =
            set(json!({"key": "browser.allowNoSandbox", "value": true, "scope": "solution"}))
                .unwrap_err()
                .to_string();
        assert!(refused.contains("ignored"), "{refused}");
        assert!(
            set(json!({"key": "browser.allowNoSandbox", "value": null, "scope": "solution"}))
                .is_ok(),
            "removing the ignored key from the workspace's file"
        );
        assert!(
            set(json!({"key": "browser.allowNoSandbox", "value": true, "scope": "user"})).is_ok()
        );
        assert!(
            set(json!({"key": "build.onSave", "value": true, "scope": "user-workspace"})).is_err()
        );
        assert!(
            set(json!({"key": "build.onSave", "value": true, "scope": "userWorkspace"})).is_err()
        );
    }
}
