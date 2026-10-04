//! The project property pages, launch profiles and the configuration and platform selectors (brief 0049):
//! `eludite.project.properties` (read; with `open`, Project > Properties), `eludite.project.set_property` (execute),
//! `eludite.project.launch_profiles` (read), `eludite.project.set_launch_profile` (execute; `select` picks the Debug
//! toolbar's profile), and in [`crate::solution`] `eludite.solution.configurations` (read),
//! `eludite.solution.select_configuration` (edit_buffer: the per-solution selection, as Set as Startup Project) and
//! `eludite.solution.set_configuration` (execute: Configuration Manager). The shell implements [`PropertiesTarget`]
//! and answers with [`PropertiesOutput`].

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const PROPERTIES: &str = "eludite.project.properties";
pub const SET_PROPERTY: &str = "eludite.project.set_property";
pub const LAUNCH_PROFILES: &str = "eludite.project.launch_profiles";
pub const SET_LAUNCH_PROFILE: &str = "eludite.project.set_launch_profile";

/// The project commands of brief 0049 (the solution ones are [`crate::solution::CONFIGURATION_COMMANDS`]).
pub const ALL: [&str; 4] = [
    PROPERTIES,
    SET_PROPERTY,
    LAUNCH_PROFILES,
    SET_LAUNCH_PROFILE,
];

/// The pages, as the commands name them.
pub const PAGES: [&str; 8] = [
    "application",
    "build",
    "package",
    "debug",
    "code_analysis",
    "resources",
    "settings",
    "signing",
];

/// `eludite.project.properties` input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertiesInput {
    pub project: Option<String>,
    pub configuration: Option<String>,
    pub platform: Option<String>,
    pub framework: Option<String>,
    pub page: Option<String>,
    #[serde(default)]
    pub open: bool,
}

/// A property value as `set_property` takes it: text, a bool's on or off, or null (remove the element).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyInputValue {
    Text(String),
    Bool(bool),
    Remove,
}

/// `eludite.project.set_property` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetPropertyInput {
    pub project: Option<String>,
    pub property: String,
    pub value: PropertyInputValue,
    pub configuration: Option<String>,
    pub platform: Option<String>,
    pub framework: Option<String>,
    pub all_configurations: bool,
    pub override_inherited: bool,
}

/// What `set_launch_profile` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAction {
    #[default]
    Set,
    Create,
    Rename,
    Delete,
    /// The Debug toolbar's choice: the profile F5 uses (no file is written).
    Select,
}

/// `eludite.project.set_launch_profile` input. `values` keeps absent and null apart (null removes a member).
#[derive(Debug, Clone, PartialEq)]
pub struct SetLaunchProfileInput {
    pub project: Option<String>,
    pub profile: String,
    pub action: ProfileAction,
    pub new_name: Option<String>,
    pub values: Option<serde_json::Map<String, Value>>,
}

/// `eludite.solution.select_configuration` input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectConfigurationInput {
    pub configuration: Option<String>,
    pub platform: Option<String>,
    pub framework: Option<String>,
    pub project: Option<String>,
}

/// One cell of Configuration Manager's grid (`eludite.solution.set_configuration`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingCell {
    pub project: String,
    pub solution_configuration: Option<String>,
    pub solution_platform: Option<String>,
    pub configuration: Option<String>,
    pub platform: Option<String>,
    pub build: Option<bool>,
}

/// A parsed, validated request of brief 0049's commands.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertiesRequest {
    Properties(PropertiesInput),
    SetProperty(SetPropertyInput),
    LaunchProfiles {
        project: Option<String>,
    },
    SetLaunchProfile(SetLaunchProfileInput),
    Configurations,
    SelectConfiguration(SelectConfigurationInput),
    /// `None`: from the UI, the Configuration Manager dialog opens.
    SetConfiguration {
        mappings: Option<Vec<MappingCell>>,
    },
}

impl PropertiesRequest {
    pub fn command(&self) -> &'static str {
        match self {
            PropertiesRequest::Properties(_) => PROPERTIES,
            PropertiesRequest::SetProperty(_) => SET_PROPERTY,
            PropertiesRequest::LaunchProfiles { .. } => LAUNCH_PROFILES,
            PropertiesRequest::SetLaunchProfile(_) => SET_LAUNCH_PROFILE,
            PropertiesRequest::Configurations => crate::solution::CONFIGURATIONS,
            PropertiesRequest::SelectConfiguration(_) => crate::solution::SELECT_CONFIGURATION,
            PropertiesRequest::SetConfiguration { .. } => crate::solution::SET_CONFIGURATION,
        }
    }
}

/// One page of `properties`' output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRow {
    pub id: String,
    pub title: String,
    /// `ready`, `launch_profiles` or `not_yet`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One property of `properties`' output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyRow {
    pub name: String,
    pub page: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    pub per_configuration: bool,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// `project`, `conditioned`, `inherited` or `default`.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_from: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
}

/// `project-properties.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertiesOutput {
    pub project: String,
    pub path: String,
    pub kind: String,
    pub configuration: String,
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    pub configurations: Vec<String>,
    pub platforms: Vec<String>,
    pub frameworks: Vec<String>,
    pub pages: Vec<PageRow>,
    pub properties: Vec<PropertyRow>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// `project-set-property.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPropertyOutput {
    pub project: String,
    pub path: String,
    pub property: String,
    /// `written`, `removed`, `unchanged`, `inherited` or `pending`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_from: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_conditions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub generation: u64,
}

/// One environment variable, in the file's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvRow {
    pub name: String,
    pub value: String,
}

/// One launch profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRow {
    pub name: String,
    pub command_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_line_args: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    pub environment_variables: Vec<EnvRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_browser: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dotnet_run_messages: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_reload_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_path: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
}

/// `project-launch-profiles.output.json` and `project-set-launch-profile.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchProfilesOutput {
    pub project: String,
    pub path: String,
    pub file: String,
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    pub profiles: Vec<ProfileRow>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// The active solution configuration and platform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRow {
    pub configuration: String,
    pub platform: String,
}

/// One row of Configuration Manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingRow {
    pub solution_configuration: String,
    pub solution_platform: String,
    pub configuration: String,
    pub platform: String,
    pub build: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deploy: bool,
}

/// One project of `configurations`' output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfigurationsRow {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configurations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frameworks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub mappings: Vec<MappingRow>,
}

/// `solution-configurations.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationsOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `sln`, `slnx`, `project` or `cargo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub configurations: Vec<String>,
    pub platforms: Vec<String>,
    pub active: SelectionRow,
    pub projects: Vec<ProjectConfigurationsRow>,
}

/// `solution-select-configuration.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectOutput {
    pub configuration: String,
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
}

/// `solution-set-configuration.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetConfigurationOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub written: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dialog: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    pub projects: Vec<ProjectConfigurationsRow>,
}

/// What the shell answers.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertiesOutputs {
    Properties(Box<PropertiesOutput>),
    SetProperty(SetPropertyOutput),
    LaunchProfiles(LaunchProfilesOutput),
    Configurations(ConfigurationsOutput),
    Select(SelectOutput),
    SetConfiguration(SetConfigurationOutput),
}

impl PropertiesOutputs {
    pub fn to_json(&self) -> Value {
        match self {
            PropertiesOutputs::Properties(o) => serde_json::to_value(o),
            PropertiesOutputs::SetProperty(o) => serde_json::to_value(o),
            PropertiesOutputs::LaunchProfiles(o) => serde_json::to_value(o),
            PropertiesOutputs::Configurations(o) => serde_json::to_value(o),
            PropertiesOutputs::Select(o) => serde_json::to_value(o),
            PropertiesOutputs::SetConfiguration(o) => serde_json::to_value(o),
        }
        .expect("the property command outputs serialize")
    }
}

/// What the shell implements, for the seven commands.
pub trait PropertiesTarget: Send + Sync {
    fn apply(&self, request: PropertiesRequest) -> Result<PropertiesOutputs, CommandError>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetPropertyIn {
    project: Option<String>,
    property: String,
    value: Value,
    configuration: Option<String>,
    platform: Option<String>,
    framework: Option<String>,
    #[serde(default)]
    all_configurations: bool,
    #[serde(default, rename = "override")]
    override_inherited: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProjectIn {
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetProfileIn {
    project: Option<String>,
    profile: String,
    #[serde(default)]
    action: ProfileAction,
    new_name: Option<String>,
    values: Option<serde_json::Map<String, Value>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SetConfigurationIn {
    mappings: Option<Vec<MappingCell>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct EmptyIn {}

/// The `values` members `set_launch_profile` edits (snake_case, as the input names them).
pub const PROFILE_MEMBERS: [&str; 10] = [
    "command_name",
    "command_line_args",
    "working_directory",
    "environment_variables",
    "launch_browser",
    "launch_url",
    "application_url",
    "dotnet_run_messages",
    "hot_reload_enabled",
    "executable_path",
];

fn bad(e: impl std::fmt::Display) -> CommandError {
    CommandError::InvalidInput(e.to_string())
}

fn input<T: serde::de::DeserializeOwned + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(bad)
}

fn non_empty(what: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(s) if s.trim().is_empty() => Err(bad(format!("`{what}` is empty"))),
        v => Ok(v),
    }
}

/// Parse and validate brief 0049's command `id` (project and solution commands alike).
pub fn parse(id: &str, value: Value) -> Result<PropertiesRequest, CommandError> {
    Ok(match id {
        PROPERTIES => {
            let i: PropertiesInput = input(value)?;
            if let Some(p) = &i.page
                && !PAGES.contains(&p.as_str())
            {
                return Err(bad(format!("`page` is one of {}", PAGES.join(", "))));
            }
            PropertiesRequest::Properties(PropertiesInput {
                project: non_empty("project", i.project)?,
                configuration: non_empty("configuration", i.configuration)?,
                platform: non_empty("platform", i.platform)?,
                framework: non_empty("framework", i.framework)?,
                ..i
            })
        }
        SET_PROPERTY => {
            let i: SetPropertyIn = serde_json::from_value(value).map_err(bad)?;
            if i.property.trim().is_empty() {
                return Err(bad("`property` is empty"));
            }
            let value = match i.value {
                Value::String(s) => PropertyInputValue::Text(s),
                Value::Bool(b) => PropertyInputValue::Bool(b),
                Value::Null => PropertyInputValue::Remove,
                other => {
                    return Err(bad(format!(
                        "`value` is text, true or false, or null, not {other}"
                    )));
                }
            };
            if i.all_configurations
                && (i.configuration.is_some() || i.platform.is_some() || i.framework.is_some())
            {
                return Err(bad(
                    "`all_configurations` writes the unconditioned value: not with `configuration`, `platform` or `framework`",
                ));
            }
            if i.platform.is_some() && i.configuration.is_none() && i.framework.is_none() {
                return Err(bad("`platform` goes with `configuration`"));
            }
            PropertiesRequest::SetProperty(SetPropertyInput {
                project: non_empty("project", i.project)?,
                property: i.property,
                value,
                configuration: non_empty("configuration", i.configuration)?,
                platform: non_empty("platform", i.platform)?,
                framework: non_empty("framework", i.framework)?,
                all_configurations: i.all_configurations,
                override_inherited: i.override_inherited,
            })
        }
        LAUNCH_PROFILES => {
            let i: ProjectIn = input(value)?;
            PropertiesRequest::LaunchProfiles {
                project: non_empty("project", i.project)?,
            }
        }
        SET_LAUNCH_PROFILE => {
            let i: SetProfileIn = serde_json::from_value(value).map_err(bad)?;
            if i.profile.trim().is_empty() {
                return Err(bad("`profile` is empty"));
            }
            if (i.action == ProfileAction::Rename) != i.new_name.is_some() {
                return Err(bad("`new_name` goes with `rename`, and `rename` needs it"));
            }
            if let Some(values) = &i.values {
                if matches!(
                    i.action,
                    ProfileAction::Delete | ProfileAction::Rename | ProfileAction::Select
                ) {
                    return Err(bad("`values` goes with `set` or `create`"));
                }
                for (k, v) in values {
                    if !PROFILE_MEMBERS.contains(&k.as_str()) {
                        return Err(bad(format!(
                            "`{k}` is not a launch profile member Eludite edits"
                        )));
                    }
                    let ok = match k.as_str() {
                        "environment_variables" => v.is_null() || v.is_array(),
                        "launch_browser" | "dotnet_run_messages" | "hot_reload_enabled" => {
                            v.is_null() || v.is_boolean()
                        }
                        _ => v.is_null() || v.is_string(),
                    };
                    if !ok {
                        return Err(bad(format!("`{k}` has the wrong type")));
                    }
                }
            }
            PropertiesRequest::SetLaunchProfile(SetLaunchProfileInput {
                project: non_empty("project", i.project)?,
                profile: i.profile,
                action: i.action,
                new_name: non_empty("new_name", i.new_name)?,
                values: i.values,
            })
        }
        crate::solution::CONFIGURATIONS => {
            let _: EmptyIn = input(value)?;
            PropertiesRequest::Configurations
        }
        crate::solution::SELECT_CONFIGURATION => {
            let i: SelectConfigurationInput = input(value)?;
            if i.configuration.is_none() && i.platform.is_none() && i.framework.is_none() {
                return Err(bad("give `configuration`, `platform` or `framework`"));
            }
            if i.project.is_some() && i.framework.is_none() {
                return Err(bad("`project` goes with `framework`"));
            }
            PropertiesRequest::SelectConfiguration(SelectConfigurationInput {
                configuration: non_empty("configuration", i.configuration)?,
                platform: non_empty("platform", i.platform)?,
                framework: non_empty("framework", i.framework)?,
                project: non_empty("project", i.project)?,
            })
        }
        crate::solution::SET_CONFIGURATION => {
            let i: SetConfigurationIn = input(value)?;
            if let Some(m) = &i.mappings {
                if m.is_empty() || m.len() > 1000 {
                    return Err(bad("`mappings` lists 1 to 1000 cells"));
                }
                if m.iter().any(|c| c.project.trim().is_empty()) {
                    return Err(bad("a mapping's `project` is empty"));
                }
            }
            PropertiesRequest::SetConfiguration {
                mappings: i.mappings,
            }
        }
        other => return Err(bad(format!("not a project properties command: {other}"))),
    })
}

pub fn spec(id: &str) -> CommandSpec {
    macro_rules! schema {
        ($name:literal) => {
            include_str!(concat!("../../../../protocol/schemas/", $name))
        };
    }
    let (title, input, output, permission) = match id {
        PROPERTIES => (
            "Project: Properties",
            schema!("project-properties.input.json"),
            schema!("project-properties.output.json"),
            PermissionClass::Read,
        ),
        // It writes the project file and reloads the solution.
        SET_PROPERTY => (
            "Project: Set Property",
            schema!("project-set-property.input.json"),
            schema!("project-set-property.output.json"),
            PermissionClass::Execute,
        ),
        LAUNCH_PROFILES => (
            "Project: Launch Profiles",
            schema!("project-launch-profiles.input.json"),
            schema!("project-launch-profiles.output.json"),
            PermissionClass::Read,
        ),
        // It writes launchSettings.json, which decides what F5 runs.
        SET_LAUNCH_PROFILE => (
            "Project: Edit Launch Profile",
            schema!("project-set-launch-profile.input.json"),
            schema!("project-set-launch-profile.output.json"),
            PermissionClass::Execute,
        ),
        crate::solution::CONFIGURATIONS => (
            "Solution: Configurations",
            schema!("solution-configurations.input.json"),
            schema!("solution-configurations.output.json"),
            PermissionClass::Read,
        ),
        // The per-solution selection, as Set as Startup Project: what F5, builds and tests use.
        crate::solution::SELECT_CONFIGURATION => (
            "Solution: Select Configuration",
            schema!("solution-select-configuration.input.json"),
            schema!("solution-select-configuration.output.json"),
            PermissionClass::EditBuffer,
        ),
        // Configuration Manager writes the solution file.
        crate::solution::SET_CONFIGURATION => (
            "Build: Configuration Manager",
            schema!("solution-set-configuration.input.json"),
            schema!("solution-set-configuration.output.json"),
            PermissionClass::Execute,
        ),
        other => unreachable!("not a project properties command: {other}"),
    };
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// Register `ids` (brief 0049's project or solution commands) on `registry`, answered by `target`.
pub fn register_ids(
    registry: &CommandRegistry,
    ids: &[&'static str],
    target: Arc<dyn PropertiesTarget>,
) {
    for &id in ids {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|o| o.to_json())
        });
    }
}

/// Register the four project commands (the solution ones: [`crate::solution::register_configurations`]).
pub fn register(registry: &CommandRegistry, target: Arc<dyn PropertiesTarget>) {
    register_ids(registry, &ALL, target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys_in_schema(value: &Value, schema: &Value) {
        for (k, v) in value.as_object().unwrap() {
            let s = &schema["properties"][k];
            assert!(!s.is_null(), "{k} is not in the schema");
            if let (Some(items), Some(arr)) = (s.get("items"), v.as_array())
                && items.get("properties").is_some()
            {
                for item in arr {
                    keys_in_schema(item, items);
                }
            }
        }
    }

    #[test]
    fn inputs_parse_and_are_validated() {
        let r = parse(
            PROPERTIES,
            json!({"project": "App", "open": true, "page": "build"}),
        )
        .unwrap();
        let PropertiesRequest::Properties(p) = r else {
            panic!()
        };
        assert!(p.open);
        assert_eq!(p.page.as_deref(), Some("build"));
        assert!(parse(PROPERTIES, json!({"page": "nope"})).is_err());
        assert!(parse(PROPERTIES, json!({"project": ""})).is_err());
        assert!(parse(PROPERTIES, Value::Null).is_ok());

        let r = parse(
            SET_PROPERTY,
            json!({"property": "Optimize", "value": true, "configuration": "Release", "platform": "Any CPU"}),
        )
        .unwrap();
        let PropertiesRequest::SetProperty(s) = r else {
            panic!()
        };
        assert_eq!(s.value, PropertyInputValue::Bool(true));
        assert_eq!(s.platform.as_deref(), Some("Any CPU"));
        let PropertiesRequest::SetProperty(s) = parse(
            SET_PROPERTY,
            json!({"property": "AssemblyName", "value": null, "override": true}),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            (s.value, s.override_inherited),
            (PropertyInputValue::Remove, true)
        );
        for bad in [
            json!({"property": "X"}),
            json!({"property": "", "value": "x"}),
            json!({"property": "X", "value": 3}),
            json!({"property": "X", "value": "x", "all_configurations": true, "configuration": "Debug"}),
            json!({"property": "X", "value": "x", "platform": "x64"}),
            json!({"property": "X", "value": "x", "other": 1}),
        ] {
            assert!(parse(SET_PROPERTY, bad.clone()).is_err(), "{bad}");
        }

        let PropertiesRequest::SetLaunchProfile(s) = parse(
            SET_LAUNCH_PROFILE,
            json!({"profile": "http", "values": {"command_line_args": "--x", "launch_url": null,
                   "environment_variables": [{"name": "A", "value": "1"}]}}),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(s.action, ProfileAction::Set);
        assert_eq!(s.values.unwrap().len(), 3);
        for bad in [
            json!({"profile": "http", "action": "rename"}),
            json!({"profile": "http", "new_name": "x"}),
            json!({"profile": "http", "values": {"unknown": "x"}}),
            json!({"profile": "http", "values": {"launch_browser": "yes"}}),
            json!({"profile": "http", "action": "delete", "values": {"launch_url": "x"}}),
        ] {
            assert!(parse(SET_LAUNCH_PROFILE, bad.clone()).is_err(), "{bad}");
        }
        assert!(
            parse(
                SET_LAUNCH_PROFILE,
                json!({"profile": "http", "action": "select"})
            )
            .is_ok()
        );

        assert_eq!(
            parse(crate::solution::CONFIGURATIONS, json!({})).unwrap(),
            PropertiesRequest::Configurations
        );
        assert!(parse(crate::solution::SELECT_CONFIGURATION, json!({})).is_err());
        assert!(
            parse(
                crate::solution::SELECT_CONFIGURATION,
                json!({"project": "App"})
            )
            .is_err()
        );
        assert!(
            parse(
                crate::solution::SELECT_CONFIGURATION,
                json!({"configuration": "Release"})
            )
            .is_ok()
        );
        assert_eq!(
            parse(crate::solution::SET_CONFIGURATION, json!({})).unwrap(),
            PropertiesRequest::SetConfiguration { mappings: None }
        );
        assert!(parse(crate::solution::SET_CONFIGURATION, json!({"mappings": []})).is_err());
        let PropertiesRequest::SetConfiguration { mappings: Some(m) } = parse(
            crate::solution::SET_CONFIGURATION,
            json!({"mappings": [{"project": "Lib", "build": true}]}),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(m[0].build, Some(true));
    }

    #[test]
    fn specs_and_outputs_follow_the_schemas() {
        let all: Vec<&str> = ALL
            .iter()
            .chain(crate::solution::CONFIGURATION_COMMANDS.iter())
            .copied()
            .collect();
        for id in &all {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert!(s.agent_visible);
        }
        assert_eq!(spec(PROPERTIES).permission, PermissionClass::Read);
        assert_eq!(spec(SET_PROPERTY).permission, PermissionClass::Execute);
        assert_eq!(
            spec(SET_LAUNCH_PROFILE).permission,
            PermissionClass::Execute
        );
        assert_eq!(
            spec(crate::solution::SELECT_CONFIGURATION).permission,
            PermissionClass::EditBuffer
        );
        let props = PropertiesOutputs::Properties(Box::new(PropertiesOutput {
            project: "App".into(),
            path: "/s/App/App.csproj".into(),
            kind: "sdk".into(),
            configuration: "Debug".into(),
            platform: "AnyCPU".into(),
            framework: Some("net8.0".into()),
            configurations: vec!["Debug".into()],
            platforms: vec!["AnyCPU".into()],
            frameworks: vec!["net8.0".into(), "net10.0".into()],
            pages: vec![PageRow {
                id: "resources".into(),
                title: "Resources".into(),
                state: "not_yet".into(),
                note: Some("x".into()),
            }],
            properties: vec![PropertyRow {
                name: "LangVersion".into(),
                page: "application".into(),
                section: Some("General".into()),
                label: "Language version".into(),
                kind: "enum".into(),
                values: vec!["latest".into()],
                per_configuration: false,
                value: "12.0".into(),
                raw: Some("12.0".into()),
                source: "inherited".into(),
                file: Some("/s/Directory.Build.props".into()),
                line: Some(4),
                condition: None,
                inherited_from: Some("/s/Directory.Build.props".into()),
                conditions: vec!["c".into()],
                read_only: true,
            }],
            generation: 2,
            opened: true,
            pending: true,
        }));
        keys_in_schema(&props.to_json(), &spec(PROPERTIES).output_schema);
        let set = PropertiesOutputs::SetProperty(SetPropertyOutput {
            project: "App".into(),
            path: "/s/App/App.csproj".into(),
            property: "Optimize".into(),
            status: "pending".into(),
            condition: Some("c".into()),
            line: Some(3),
            inherited_from: Some("x".into()),
            removed_conditions: vec!["c".into()],
            value: Some("true".into()),
            generation: 3,
        });
        let schema = spec(SET_PROPERTY).output_schema;
        keys_in_schema(&set.to_json(), &schema);
        assert!(
            schema["properties"]["status"]["enum"]
                .as_array()
                .unwrap()
                .contains(&json!("pending"))
        );
        let profiles = PropertiesOutputs::LaunchProfiles(LaunchProfilesOutput {
            project: "Web".into(),
            path: "/s/Web/Web.csproj".into(),
            file: "/s/Web/Properties/launchSettings.json".into(),
            exists: true,
            selected: Some("http".into()),
            profiles: vec![ProfileRow {
                name: "http".into(),
                command_name: "Project".into(),
                command_line_args: Some("a".into()),
                environment_variables: vec![EnvRow {
                    name: "A".into(),
                    value: "1".into(),
                }],
                launch_browser: Some(true),
                read_only: true,
                unknown: vec!["x".into()],
                ..ProfileRow::default()
            }],
            pending: true,
        });
        keys_in_schema(&profiles.to_json(), &spec(LAUNCH_PROFILES).output_schema);
        keys_in_schema(&profiles.to_json(), &spec(SET_LAUNCH_PROFILE).output_schema);
        let row = ProjectConfigurationsRow {
            name: "Lib".into(),
            path: "/s/Lib/Lib.csproj".into(),
            configurations: vec!["Debug".into()],
            platforms: vec!["Any CPU".into()],
            frameworks: vec!["net8.0".into()],
            framework: Some("net8.0".into()),
            profile: Some("Lib".into()),
            mappings: vec![MappingRow {
                solution_configuration: "Release".into(),
                solution_platform: "Any CPU".into(),
                configuration: "Release".into(),
                platform: "Any CPU".into(),
                build: false,
                deploy: true,
            }],
        };
        let configurations = PropertiesOutputs::Configurations(ConfigurationsOutput {
            path: Some("/s/S.sln".into()),
            format: Some("sln".into()),
            configurations: vec!["Debug".into()],
            platforms: vec!["Any CPU".into()],
            active: SelectionRow::default(),
            projects: vec![row.clone()],
        });
        keys_in_schema(
            &configurations.to_json(),
            &spec(crate::solution::CONFIGURATIONS).output_schema,
        );
        keys_in_schema(
            &PropertiesOutputs::Select(SelectOutput {
                configuration: "Release".into(),
                platform: "x64".into(),
                project: Some("App".into()),
                framework: Some("net8.0".into()),
            })
            .to_json(),
            &spec(crate::solution::SELECT_CONFIGURATION).output_schema,
        );
        keys_in_schema(
            &PropertiesOutputs::SetConfiguration(SetConfigurationOutput {
                path: Some("/s/S.sln".into()),
                written: true,
                dialog: true,
                pending: true,
                projects: vec![row],
            })
            .to_json(),
            &spec(crate::solution::SET_CONFIGURATION).output_schema,
        );
    }
}
