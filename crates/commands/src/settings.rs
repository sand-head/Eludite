//! Settings (brief 0020, PLAN.md 4.12): the settings schema and the `eludite.settings.get`, `eludite.settings.set`
//! and `eludite.tools.options` commands.
//!
//! [`SettingsSchema`] is read from `protocol/schemas/settings.json`, the schema of the settings file: every setting's
//! key, type, default, description, Options dialog section and label, and the environment variable that overrides
//! it. Defaults come from there and nowhere else, and the Options dialog is generated from it. The store itself (the
//! files, their merge and live reload) lives in the shell, which implements [`SettingsTarget`]; this module parses
//! the commands' input, validates a value against its setting, and serializes the outputs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

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
    /// The item schema of a list (to validate entries).
    items: Option<Value>,
    max_items: Option<usize>,
}

impl SettingSpec {
    /// Check `value` against the setting's type.
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        let ok = match &self.kind {
            SettingKind::Bool => value.is_boolean(),
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
                            check_object(schema, item)
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
                    SettingKind::Text | SettingKind::Path => "a string".to_owned(),
                    SettingKind::Enum { values, .. } => format!("one of {}", values.join(", ")),
                    SettingKind::List => "a list".to_owned(),
                }
            ))
        }
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

/// A small check of an object against an object schema: `required`, `additionalProperties: false`, and string,
/// array-of-strings and string-map members.
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
    Environment,
}

/// Which file `eludite.settings.set` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingScope {
    #[default]
    User,
    Solution,
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
    pub settings: Vec<SettingRow>,
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
    #[serde(default)]
    scope: SettingScope,
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

/// Parse a settings command's input. A `set` must name a known key with a value of its type (or null).
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
            Ok(SettingsRequest::Set {
                key: i.key,
                value: i.value,
                scope: i.scope,
            })
        }
        OPTIONS => {
            let i: OptionsInput = input(value)?;
            if let Some(s) = &i.section
                && !schema.sections.contains(s)
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
                "build.beforeRun",
                "build.onSave",
                "build.showOutputOnStart",
                "build.showErrorListOnFailure",
                "build.cargoPath",
                "debugger.netcoredbgPath",
                "debugger.monoPrefix",
                "debugger.monoAdapterPath",
                "languageServers.rustAnalyzerPath",
                "agents.default",
                "agents.claudeCodeAdapterPath",
                "agents.custom",
            ]
        );
        assert_eq!(s.get("build.onSave").unwrap().default, json!(false));
        assert_eq!(s.get("build.beforeRun").unwrap().default, json!(true));
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
        assert_eq!(s.section("Debugging > General").count(), 3);
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
    }
}
