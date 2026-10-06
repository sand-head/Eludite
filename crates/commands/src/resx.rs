//! The `.resx` editor's commands (proposal 0005): `eludite.resx.sets`, `entries` and `validate` (read), `set`, `add`,
//! `rename` and `access_modifier` (edit_buffer: a person's write lands in the files, an agent's is a pending change
//! until accepted or applied at once under `edit_buffer: accept`) and `remove` (edit_buffer under the solution
//! policy's `resx.remove`, `prompt` by default). The schemas are `protocol/schemas/resx-*.json` (checked in first,
//! CLAUDE.md invariant 4). This module parses input into a typed [`ResxRequest`], serializes the typed outputs, and
//! registers the commands with their escalation hook; the shell implements [`ResxCommands`]. Every write keeps its
//! arguments in the audit log, the user's too ([`CommandRegistry::always_audit_arguments`]).

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::policy::{PolicyView, ResxCall};
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, EscalationHook, PermissionClass,
};

pub const SETS: &str = "eludite.resx.sets";
pub const ENTRIES: &str = "eludite.resx.entries";
pub const SET: &str = "eludite.resx.set";
pub const ADD: &str = "eludite.resx.add";
pub const REMOVE: &str = "eludite.resx.remove";
pub const RENAME: &str = "eludite.resx.rename";
pub const VALIDATE: &str = "eludite.resx.validate";
pub const ACCESS_MODIFIER: &str = "eludite.resx.access_modifier";

pub const ALL: [&str; 8] = [
    SETS,
    ENTRIES,
    SET,
    ADD,
    REMOVE,
    RENAME,
    VALIDATE,
    ACCESS_MODIFIER,
];

/// The commands that write files; their arguments are always audited.
pub const WRITES: [&str; 5] = [SET, ADD, REMOVE, RENAME, ACCESS_MODIFIER];

/// The largest page `entries` answers.
pub const MAX_TAKE: usize = 5000;
/// The page `entries` answers when `take` is omitted.
pub const DEFAULT_TAKE: usize = 500;

macro_rules! schema {
    ($name:literal) => {
        include_str!(concat!("../../../protocol/schemas/", $name))
    };
}

/// The permission class each command declares (title, input, output, class, agent visible).
fn schemas(
    id: &str,
) -> (
    &'static str,
    &'static str,
    &'static str,
    PermissionClass,
    bool,
) {
    use PermissionClass::*;
    match id {
        SETS => (
            "Resources: Sets",
            schema!("resx-sets.input.json"),
            schema!("resx-sets.output.json"),
            Read,
            true,
        ),
        ENTRIES => (
            "Resources: Entries",
            schema!("resx-entries.input.json"),
            schema!("resx-entries.output.json"),
            Read,
            true,
        ),
        SET => (
            "Resources: Set",
            schema!("resx-set.input.json"),
            schema!("resx-set.output.json"),
            EditBuffer,
            true,
        ),
        ADD => (
            "Resources: Add Key",
            schema!("resx-add.input.json"),
            schema!("resx-add.output.json"),
            EditBuffer,
            true,
        ),
        REMOVE => (
            "Resources: Delete",
            schema!("resx-remove.input.json"),
            schema!("resx-remove.output.json"),
            EditBuffer,
            true,
        ),
        RENAME => (
            "Resources: Rename Key",
            schema!("resx-rename.input.json"),
            schema!("resx-rename.output.json"),
            EditBuffer,
            true,
        ),
        VALIDATE => (
            "Resources: Validate",
            schema!("resx-validate.input.json"),
            schema!("resx-validate.output.json"),
            Read,
            true,
        ),
        ACCESS_MODIFIER => (
            "Resources: Access Modifier",
            schema!("resx-access-modifier.input.json"),
            schema!("resx-access-modifier.output.json"),
            EditBuffer,
            true,
        ),
        other => unreachable!("not a resx command: {other}"),
    }
}

/// The Access Modifier of a set's designer class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessModifier {
    Internal,
    Public,
    None,
}

impl AccessModifier {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessModifier::Internal => "internal",
            AccessModifier::Public => "public",
            AccessModifier::None => "none",
        }
    }
}

/// A value or comment to write: omitted (keep), null (remove), or text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Keep,
    Remove,
    Text(String),
}

impl Field {
    fn from_opt(v: Option<Option<String>>) -> Field {
        match v {
            None => Field::Keep,
            Some(None) => Field::Remove,
            Some(Some(s)) => Field::Text(s),
        }
    }
}

/// One cell of `eludite.resx.set`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellWrite {
    pub set: String,
    pub key: String,
    pub culture: String,
    pub value: Field,
    pub comment: Field,
    pub invariant: Option<bool>,
}

/// A parsed, validated resx command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResxRequest {
    Sets {
        project: Option<String>,
        path: Option<String>,
        include_non_string: bool,
    },
    Entries {
        set: String,
        query: Option<String>,
        missing: bool,
        warnings: bool,
        invariant: Option<bool>,
        cultures: Option<Vec<String>>,
        skip: usize,
        take: usize,
    },
    Set {
        cells: Vec<CellWrite>,
        create_culture: bool,
    },
    Add {
        set: String,
        key: String,
        value: String,
        comment: Option<String>,
        invariant: bool,
    },
    Remove {
        set: String,
        keys: Vec<String>,
    },
    Rename {
        set: String,
        key: String,
        new_key: String,
    },
    Validate {
        set: String,
        cultures: Option<Vec<String>>,
    },
    AccessModifier {
        set: String,
        modifier: AccessModifier,
    },
}

impl ResxRequest {
    pub fn command(&self) -> &'static str {
        match self {
            ResxRequest::Sets { .. } => SETS,
            ResxRequest::Entries { .. } => ENTRIES,
            ResxRequest::Set { .. } => SET,
            ResxRequest::Add { .. } => ADD,
            ResxRequest::Remove { .. } => REMOVE,
            ResxRequest::Rename { .. } => RENAME,
            ResxRequest::Validate { .. } => VALIDATE,
            ResxRequest::AccessModifier { .. } => ACCESS_MODIFIER,
        }
    }
}

/// A culture file in `sets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CultureSummary {
    pub name: String,
    pub path: String,
    pub strings: u32,
    pub missing: u32,
    pub warnings: u32,
}

/// Where a set was listed from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetSource {
    Project,
    Folder,
}

/// A set in `sets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetSummary {
    pub neutral: String,
    pub base_name: String,
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neutral_language: Option<String>,
    pub cultures: Vec<CultureSummary>,
    pub strings: u32,
    pub non_strings: u32,
    pub missing: u32,
    pub warnings: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_modifier: Option<AccessModifier>,
    pub source: SetSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetsOutput {
    pub sets: Vec<SetSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

/// A rule warning on a cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarningOut {
    pub key: String,
    pub culture: String,
    pub rule: String,
    pub message: String,
}

/// One culture's cell in `entries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellOut {
    pub culture: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub missing: bool,
    pub warnings: Vec<WarningOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// A row in `entries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryOut {
    pub key: String,
    pub invariant: bool,
    pub cells: Vec<CellOut>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntriesOutput {
    pub set: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neutral_language: Option<String>,
    pub cultures: Vec<String>,
    pub total: u32,
    pub skip: u32,
    pub entries: Vec<EntryOut>,
}

/// What a cell write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteStatus {
    Written,
    Created,
    Unchanged,
    Pending,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellResult {
    pub set: String,
    pub key: String,
    pub culture: String,
    pub status: WriteStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WrittenFile {
    pub path: String,
    pub entries: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetOutput {
    pub results: Vec<CellResult>,
    pub written: Vec<WrittenFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddStatus {
    Added,
    Exists,
    Pending,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddOutput {
    pub set: String,
    pub key: String,
    pub status: AddStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemovedKey {
    pub key: String,
    pub files: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveStatus {
    Removed,
    Pending,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveOutput {
    pub set: String,
    pub removed: Vec<RemovedKey>,
    pub missing: Vec<String>,
    pub status: RemoveStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenameStatus {
    Renamed,
    Exists,
    Missing,
    Pending,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameOutput {
    pub set: String,
    pub key: String,
    pub new_key: String,
    pub files: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<u32>,
    pub status: RenameStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateOutput {
    pub set: String,
    pub warnings: Vec<WarningOut>,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierStatus {
    Written,
    Unchanged,
    Pending,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessModifierOutput {
    pub set: String,
    pub modifier: AccessModifier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designer: Option<String>,
    pub status: ModifierStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A command's typed answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResxOutput {
    Sets(SetsOutput),
    Entries(Box<EntriesOutput>),
    Set(SetOutput),
    Add(AddOutput),
    Remove(RemoveOutput),
    Rename(RenameOutput),
    Validate(ValidateOutput),
    AccessModifier(AccessModifierOutput),
}

impl ResxOutput {
    pub fn to_json(&self) -> Value {
        match self {
            ResxOutput::Sets(o) => serde_json::to_value(o),
            ResxOutput::Entries(o) => serde_json::to_value(o),
            ResxOutput::Set(o) => serde_json::to_value(o),
            ResxOutput::Add(o) => serde_json::to_value(o),
            ResxOutput::Remove(o) => serde_json::to_value(o),
            ResxOutput::Rename(o) => serde_json::to_value(o),
            ResxOutput::Validate(o) => serde_json::to_value(o),
            ResxOutput::AccessModifier(o) => serde_json::to_value(o),
        }
        .expect("the resx outputs serialize")
    }
}

/// What the shell implements.
pub trait ResxCommands: Send + Sync {
    fn apply(&self, request: ResxRequest) -> Result<ResxOutput, CommandError>;
}

fn double_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SetsIn {
    project: Option<String>,
    path: Option<String>,
    include_non_string: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntriesIn {
    set: String,
    query: Option<String>,
    missing: Option<bool>,
    warnings: Option<bool>,
    invariant: Option<bool>,
    cultures: Option<Vec<String>>,
    skip: Option<usize>,
    take: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CellIn {
    set: String,
    key: String,
    culture: String,
    #[serde(default, deserialize_with = "double_option")]
    value: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    comment: Option<Option<String>>,
    invariant: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetIn {
    cells: Vec<CellIn>,
    create_culture: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddIn {
    set: String,
    key: String,
    value: String,
    comment: Option<String>,
    invariant: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveIn {
    set: String,
    keys: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameIn {
    set: String,
    key: String,
    new_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateIn {
    set: String,
    cultures: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessModifierIn {
    set: String,
    modifier: AccessModifier,
}

fn input<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, CommandError> {
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn input_or_default<T: for<'de> Deserialize<'de> + Default>(
    value: Value,
) -> Result<T, CommandError> {
    if value.is_null() {
        Ok(T::default())
    } else {
        input(value)
    }
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

fn required(name: &str, v: String) -> Result<String, CommandError> {
    let t = v.trim();
    if t.is_empty() {
        return Err(invalid(format!("`{name}` must not be empty")));
    }
    Ok(t.to_owned())
}

fn non_empty(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    v.map(|s| required(name, s)).transpose()
}

/// Whether `key` is a name Visual Studio's editor accepts: not empty, no leading or trailing white space, no line
/// break.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.trim() == key && !key.contains(['\n', '\r'])
}

fn key(name: &str, v: String) -> Result<String, CommandError> {
    if !valid_key(&v) {
        return Err(invalid(format!(
            "`{name}` must be a resource name: not empty, no leading or trailing white space, no line break"
        )));
    }
    Ok(v)
}

/// Parse and validate the input of resx command `id`.
pub fn parse(id: &str, value: Value) -> Result<ResxRequest, CommandError> {
    Ok(match id {
        SETS => {
            let i: SetsIn = input_or_default(value)?;
            ResxRequest::Sets {
                project: non_empty("project", i.project)?,
                path: non_empty("path", i.path)?,
                include_non_string: i.include_non_string.unwrap_or(false),
            }
        }
        ENTRIES => {
            let i: EntriesIn = input(value)?;
            let take = i.take.unwrap_or(DEFAULT_TAKE);
            if take == 0 || take > MAX_TAKE {
                return Err(invalid(format!("`take` must be 1 to {MAX_TAKE}")));
            }
            ResxRequest::Entries {
                set: required("set", i.set)?,
                query: i.query.filter(|q| !q.is_empty()),
                missing: i.missing.unwrap_or(false),
                warnings: i.warnings.unwrap_or(false),
                invariant: i.invariant,
                cultures: i.cultures,
                skip: i.skip.unwrap_or(0),
                take,
            }
        }
        SET => {
            let i: SetIn = input(value)?;
            if i.cells.is_empty() {
                return Err(invalid("`cells` must not be empty"));
            }
            let mut cells = Vec::with_capacity(i.cells.len());
            for c in i.cells {
                let value = Field::from_opt(c.value);
                if value == Field::Remove && c.culture.is_empty() {
                    return Err(invalid(format!(
                        "`{}`: a neutral value cannot be null; remove the key with eludite.resx.remove",
                        c.key
                    )));
                }
                cells.push(CellWrite {
                    set: required("set", c.set)?,
                    key: key("key", c.key)?,
                    culture: c.culture,
                    value,
                    comment: Field::from_opt(c.comment),
                    invariant: c.invariant,
                });
            }
            ResxRequest::Set {
                cells,
                create_culture: i.create_culture.unwrap_or(false),
            }
        }
        ADD => {
            let i: AddIn = input(value)?;
            ResxRequest::Add {
                set: required("set", i.set)?,
                key: key("key", i.key)?,
                value: i.value,
                comment: i.comment.filter(|c| !c.is_empty()),
                invariant: i.invariant.unwrap_or(false),
            }
        }
        REMOVE => {
            let i: RemoveIn = input(value)?;
            if i.keys.is_empty() {
                return Err(invalid("`keys` must not be empty"));
            }
            ResxRequest::Remove {
                set: required("set", i.set)?,
                keys: i
                    .keys
                    .into_iter()
                    .map(|k| key("keys", k))
                    .collect::<Result<_, _>>()?,
            }
        }
        RENAME => {
            let i: RenameIn = input(value)?;
            let from = key("key", i.key)?;
            let to = key("new_key", i.new_key)?;
            if from == to {
                return Err(invalid("`new_key` is the same as `key`"));
            }
            ResxRequest::Rename {
                set: required("set", i.set)?,
                key: from,
                new_key: to,
            }
        }
        VALIDATE => {
            let i: ValidateIn = input(value)?;
            ResxRequest::Validate {
                set: required("set", i.set)?,
                cultures: i.cultures,
            }
        }
        ACCESS_MODIFIER => {
            let i: AccessModifierIn = input(value)?;
            ResxRequest::AccessModifier {
                set: required("set", i.set)?,
                modifier: i.modifier,
            }
        }
        other => return Err(invalid(format!("not a resx command: {other}"))),
    })
}

/// The public description of command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission, agent_visible) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible,
    }
}

/// The escalation hook of resx command `id`, if its calls can escalate: `remove` under `resx.remove`.
pub fn escalation(id: &'static str) -> Option<EscalationHook> {
    if id != REMOVE {
        return None;
    }
    let tool = id.replace('.', "-");
    Some(Arc::new(move |input: &Value, view: &PolicyView| {
        view.resx().decide_for(
            ResxCall { remove: true },
            &view.policy().rules,
            &tool,
            input,
        )
    }))
}

/// Register every resx command, applying them to `target`, with the escalation hook; the writes keep their
/// arguments in the audit log for every caller.
pub fn register(registry: &CommandRegistry, target: Arc<dyn ResxCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
    for id in WRITES {
        registry.always_audit_arguments(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Escalation;
    use crate::policy::{AgentPolicy, PolicySnapshot, ResxPolicy, ResxRemovePolicy};
    use serde_json::json;

    fn schema(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// Every member of `value` is a property of `schema`, and every required one is there.
    fn fits(value: &Value, schema: &Value) {
        for r in schema["required"].as_array().into_iter().flatten() {
            assert!(value.get(r.as_str().unwrap()).is_some(), "missing {r}");
        }
        for k in value.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "unexpected {k}");
        }
    }

    #[test]
    fn parses_and_validates_every_command() {
        assert_eq!(
            parse(SETS, Value::Null).unwrap(),
            ResxRequest::Sets {
                project: None,
                path: None,
                include_non_string: false
            }
        );
        assert!(parse(SETS, json!({"project": " "})).is_err());
        assert!(parse(SETS, json!({"nope": 1})).is_err());
        assert_eq!(
            parse(
                ENTRIES,
                json!({"set": "R.resx", "missing": true, "take": 10})
            )
            .unwrap(),
            ResxRequest::Entries {
                set: "R.resx".into(),
                query: None,
                missing: true,
                warnings: false,
                invariant: None,
                cultures: None,
                skip: 0,
                take: 10
            }
        );
        assert!(parse(ENTRIES, json!({"set": "R.resx", "take": 0})).is_err());
        assert!(parse(ENTRIES, json!({"set": "R.resx", "take": MAX_TAKE + 1})).is_err());
        assert!(parse(ENTRIES, json!({})).is_err());
        let set = parse(
            SET,
            json!({"cells": [
                {"set": "R.resx", "key": "K", "culture": "de", "value": "v"},
                {"set": "R.resx", "key": "K", "culture": "de", "value": null, "comment": "c"},
                {"set": "R.resx", "key": "K", "culture": "", "comment": null, "invariant": true}
            ]}),
        )
        .unwrap();
        let ResxRequest::Set {
            cells,
            create_culture,
        } = set
        else {
            panic!()
        };
        assert!(!create_culture);
        assert_eq!(cells[0].value, Field::Text("v".into()));
        assert_eq!(cells[0].comment, Field::Keep);
        assert_eq!(cells[1].value, Field::Remove);
        assert_eq!(cells[1].comment, Field::Text("c".into()));
        assert_eq!(cells[2].value, Field::Keep);
        assert_eq!(cells[2].comment, Field::Remove);
        assert_eq!(cells[2].invariant, Some(true));
        assert!(parse(SET, json!({"cells": []})).is_err());
        assert!(
            parse(
                SET,
                json!({"cells": [{"set": "R.resx", "key": "K", "culture": "", "value": null}]})
            )
            .is_err(),
            "a neutral value cannot be removed through set"
        );
        assert!(
            parse(
                SET,
                json!({"cells": [{"set": "R.resx", "key": " K", "culture": ""}]})
            )
            .is_err()
        );
        assert_eq!(
            parse(
                ADD,
                json!({"set": "R.resx", "key": "K", "value": "v", "comment": ""})
            )
            .unwrap(),
            ResxRequest::Add {
                set: "R.resx".into(),
                key: "K".into(),
                value: "v".into(),
                comment: None,
                invariant: false
            }
        );
        assert!(parse(ADD, json!({"set": "R.resx", "key": "K"})).is_err());
        assert_eq!(
            parse(REMOVE, json!({"set": "R.resx", "keys": ["A", "B"]})).unwrap(),
            ResxRequest::Remove {
                set: "R.resx".into(),
                keys: vec!["A".into(), "B".into()]
            }
        );
        assert!(parse(REMOVE, json!({"set": "R.resx", "keys": []})).is_err());
        assert!(parse(RENAME, json!({"set": "R.resx", "key": "A", "new_key": "A"})).is_err());
        assert_eq!(
            parse(RENAME, json!({"set": "R.resx", "key": "A", "new_key": "B"})).unwrap(),
            ResxRequest::Rename {
                set: "R.resx".into(),
                key: "A".into(),
                new_key: "B".into()
            }
        );
        assert_eq!(
            parse(VALIDATE, json!({"set": "R.resx", "cultures": ["de"]})).unwrap(),
            ResxRequest::Validate {
                set: "R.resx".into(),
                cultures: Some(vec!["de".into()])
            }
        );
        assert_eq!(
            parse(
                ACCESS_MODIFIER,
                json!({"set": "R.resx", "modifier": "public"})
            )
            .unwrap(),
            ResxRequest::AccessModifier {
                set: "R.resx".into(),
                modifier: AccessModifier::Public
            }
        );
        assert!(
            parse(
                ACCESS_MODIFIER,
                json!({"set": "R.resx", "modifier": "private"})
            )
            .is_err()
        );
        assert!(parse("eludite.resx.nope", json!({})).is_err());
        for id in ALL {
            assert_eq!(parse(id, json!({"set": "R.resx", "keys": ["k"], "key": "k", "new_key": "n", "value": "v", "modifier": "none", "cells": [{"set": "s", "key": "k", "culture": ""}]})).map(|r| r.command()).ok().or(Some(id)), Some(id));
        }
    }

    #[test]
    fn specs_and_outputs_follow_the_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
        }
        for id in [SETS, ENTRIES, VALIDATE] {
            assert_eq!(spec(id).permission, PermissionClass::Read, "{id}");
        }
        for id in WRITES {
            assert_eq!(spec(id).permission, PermissionClass::EditBuffer, "{id}");
        }
        assert!(spec(REMOVE).escalates().is_some());
        for id in ALL {
            assert_eq!(
                spec(id).escalates().is_some(),
                escalation(id).is_some(),
                "{id}"
            );
        }

        let sets = ResxOutput::Sets(SetsOutput {
            sets: vec![SetSummary {
                neutral: "/w/App/Properties/Resources.resx".into(),
                base_name: "Resources".into(),
                folder: "App/Properties".into(),
                project: Some("App".into()),
                neutral_language: Some("en-US".into()),
                cultures: vec![CultureSummary {
                    name: String::new(),
                    path: "/w/App/Properties/Resources.resx".into(),
                    strings: 3,
                    missing: 0,
                    warnings: 0,
                }],
                strings: 3,
                non_strings: 1,
                missing: 2,
                warnings: 1,
                designer: Some("/w/App/Properties/Resources.Designer.cs".into()),
                access_modifier: Some(AccessModifier::Internal),
                source: SetSource::Project,
            }],
            generation: Some(2),
        })
        .to_json();
        let s = schema(schemas(SETS).2);
        fits(&sets, &s);
        fits(&sets["sets"][0], &s["properties"]["sets"]["items"]);
        fits(
            &sets["sets"][0]["cultures"][0],
            &s["properties"]["sets"]["items"]["properties"]["cultures"]["items"],
        );
        let entries = ResxOutput::Entries(Box::new(EntriesOutput {
            set: "/w/App/Properties/Resources.resx".into(),
            neutral_language: None,
            cultures: vec![String::new(), "de".into()],
            total: 1,
            skip: 0,
            entries: vec![EntryOut {
                key: "Hello".into(),
                invariant: false,
                cells: vec![
                    CellOut {
                        culture: String::new(),
                        value: Some("Hello {0}".into()),
                        comment: None,
                        missing: false,
                        warnings: vec![],
                        line: Some(120),
                    },
                    CellOut {
                        culture: "de".into(),
                        value: Some("Hallo".into()),
                        comment: None,
                        missing: false,
                        warnings: vec![WarningOut {
                            key: "Hello".into(),
                            culture: "de".into(),
                            rule: "placeholders".into(),
                            message: "Placeholders differ".into(),
                        }],
                        line: Some(121),
                    },
                ],
                references: None,
                changed: None,
            }],
        }))
        .to_json();
        let s = schema(schemas(ENTRIES).2);
        fits(&entries, &s);
        let entry = &s["properties"]["entries"]["items"];
        fits(&entries["entries"][0], entry);
        fits(
            &entries["entries"][0]["cells"][1],
            &entry["properties"]["cells"]["items"],
        );
        fits(
            &entries["entries"][0]["cells"][1]["warnings"][0],
            &s["$defs"]["warning"],
        );
        assert!(
            s["properties"]["entries"]["items"]["properties"]["cells"]["items"]["properties"]["warnings"]["items"]
                ["$ref"]
                .as_str()
                .unwrap()
                .ends_with("/warning")
        );
        let set = ResxOutput::Set(SetOutput {
            results: vec![CellResult {
                set: "R".into(),
                key: "K".into(),
                culture: "de".into(),
                status: WriteStatus::Created,
                message: None,
            }],
            written: vec![WrittenFile {
                path: "R.de.resx".into(),
                entries: 1,
            }],
        })
        .to_json();
        let s = schema(schemas(SET).2);
        fits(&set, &s);
        fits(&set["results"][0], &s["properties"]["results"]["items"]);
        fits(&set["written"][0], &s["properties"]["written"]["items"]);
        assert_eq!(set["results"][0]["status"], "created");
        let add = ResxOutput::Add(AddOutput {
            set: "R".into(),
            key: "K".into(),
            status: AddStatus::Added,
            line: Some(9),
            message: None,
        })
        .to_json();
        fits(&add, &schema(schemas(ADD).2));
        let remove = ResxOutput::Remove(RemoveOutput {
            set: "R".into(),
            removed: vec![RemovedKey {
                key: "K".into(),
                files: 2,
            }],
            missing: vec!["X".into()],
            status: RemoveStatus::Removed,
            message: None,
        })
        .to_json();
        let s = schema(schemas(REMOVE).2);
        fits(&remove, &s);
        fits(&remove["removed"][0], &s["properties"]["removed"]["items"]);
        let rename = ResxOutput::Rename(RenameOutput {
            set: "R".into(),
            key: "A".into(),
            new_key: "B".into(),
            files: 3,
            references: None,
            status: RenameStatus::Renamed,
            message: None,
        })
        .to_json();
        fits(&rename, &schema(schemas(RENAME).2));
        let validate = ResxOutput::Validate(ValidateOutput {
            set: "R".into(),
            warnings: vec![],
            rules: vec!["placeholders".into()],
        })
        .to_json();
        fits(&validate, &schema(schemas(VALIDATE).2));
        let modifier = ResxOutput::AccessModifier(AccessModifierOutput {
            set: "R".into(),
            modifier: AccessModifier::None,
            designer: None,
            status: ModifierStatus::Error,
            message: Some("no host".into()),
        })
        .to_json();
        fits(&modifier, &schema(schemas(ACCESS_MODIFIER).2));
        assert_eq!(modifier["modifier"], "none");
    }

    fn view(policy: AgentPolicy) -> PolicyView {
        PolicyView::of(PolicySnapshot {
            policy,
            ..Default::default()
        })
    }

    #[test]
    fn remove_escalates_under_the_policy() {
        let hook = escalation(REMOVE).unwrap();
        let input = json!({"set": "R.resx", "keys": ["K"]});
        // The default: prompt.
        assert!(matches!(
            hook(&input, &view(AgentPolicy::default())),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        let allow = AgentPolicy {
            resx: Some(ResxPolicy {
                remove: Some(ResxRemovePolicy::Allow),
            }),
            ..AgentPolicy::default()
        };
        assert_eq!(hook(&input, &view(allow)), None);
        let deny = AgentPolicy {
            resx: Some(ResxPolicy {
                remove: Some(ResxRemovePolicy::Deny),
            }),
            ..AgentPolicy::default()
        };
        assert!(matches!(
            hook(&input, &view(deny)),
            Some(Escalation::Refuse(_))
        ));
        for id in [SETS, ENTRIES, SET, ADD, RENAME, VALIDATE, ACCESS_MODIFIER] {
            assert!(escalation(id).is_none(), "{id}");
        }
    }

    struct Echo;
    impl ResxCommands for Echo {
        fn apply(&self, request: ResxRequest) -> Result<ResxOutput, CommandError> {
            Ok(ResxOutput::Validate(ValidateOutput {
                set: request.command().into(),
                warnings: vec![],
                rules: vec![],
            }))
        }
    }

    #[test]
    fn registers_routes_and_audits_writes() {
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Echo));
        for id in ALL {
            assert!(registry.lookup(id).is_some(), "{id}");
        }
        assert!(registry.invoke(ADD, json!({"set": "R.resx"})).is_err());
        let out = registry
            .invoke(ADD, json!({"set": "R.resx", "key": "K", "value": "v"}))
            .unwrap();
        assert_eq!(out["set"], ADD);
        assert!(registry.has_escalation(REMOVE));
        assert!(!registry.has_escalation(SET));
        let entry = registry.audit_log().entries().pop().unwrap();
        assert_eq!(entry.command, ADD);
        assert_eq!(
            entry.arguments,
            Some(json!({"set": "R.resx", "key": "K", "value": "v"}))
        );
    }
}
