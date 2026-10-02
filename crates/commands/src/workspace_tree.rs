//! `eludite.workspace.tree` (brief 0019): the open workspace's projects whatever their build system, as the
//! Workspace window shows them: .NET projects (`csproj`), Cargo packages with their targets (`cargo`) and plain
//! folders (`folder`). Read from a caller-supplied source, like `eludite.solution.tree`, so it runs on whichever
//! thread invokes it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const WORKSPACE_TREE: &str = "eludite.workspace.tree";

const INPUT: &str = include_str!("../../../protocol/schemas/workspace-tree.input.json");
const OUTPUT: &str = include_str!("../../../protocol/schemas/workspace-tree.output.json");

/// Files a project lists at most.
pub const MAX_FILES: usize = 5000;

/// One Cargo target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTarget {
    pub name: String,
    /// `bin`, `lib`, `proc-macro`, `example`, `test`, `bench` or `custom-build`.
    pub kind: String,
    pub path: String,
}

/// One project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProject {
    pub name: String,
    pub path: String,
    /// `csproj`, `cargo` or `folder`.
    pub kind: String,
    /// `sdk` or `legacy`, for kind csproj.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msbuild: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_frameworks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<WorkspaceTarget>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The startup project (brief 0020): `Some(true)` on that one project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup: Option<bool>,
}

/// `workspace-tree.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTreeOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// `none`, `loading`, `loaded` or `failed`.
    pub state: String,
    pub projects: Vec<WorkspaceProject>,
}

impl Default for WorkspaceTreeOutput {
    fn default() -> Self {
        Self {
            root: None,
            state: "none".into(),
            projects: Vec::new(),
        }
    }
}

/// Where the command reads the tree. Called on the invoking thread; it must be cheap and thread-safe.
pub type WorkspaceTreeSource = Arc<dyn Fn() -> WorkspaceTreeOutput + Send + Sync>;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    project: Option<String>,
}

pub fn spec() -> CommandSpec {
    CommandSpec {
        id: CommandId::new(WORKSPACE_TREE).expect("valid id"),
        title: "Workspace: List Projects, Packages and Folders".into(),
        input_schema: serde_json::from_str(INPUT).expect("valid schema"),
        output_schema: serde_json::from_str(OUTPUT).expect("valid schema"),
        permission: PermissionClass::Read,
        agent_visible: true,
    }
}

/// Register `eludite.workspace.tree`, reading from `source`.
pub fn register(
    registry: &mut CommandRegistry,
    source: WorkspaceTreeSource,
) -> Result<(), CommandError> {
    registry.register(spec(), move |input: Value| {
        let input: Input = if input.is_null() {
            Input::default()
        } else {
            serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))?
        };
        let mut out = source();
        if let Some(p) = &input.project {
            out.projects.retain(|x| &x.name == p);
        }
        for p in &mut out.projects {
            p.files.truncate(MAX_FILES);
        }
        serde_json::to_value(out).map_err(|e| CommandError::Failed(e.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lists_projects_of_every_kind_and_filters() {
        let mut r = CommandRegistry::new();
        register(
            &mut r,
            Arc::new(|| WorkspaceTreeOutput {
                root: Some("/w".into()),
                state: "loaded".into(),
                projects: vec![
                    WorkspaceProject {
                        name: "Eludite.Host".into(),
                        path: "/w/dotnet/src/Eludite.Host/Eludite.Host.csproj".into(),
                        kind: "csproj".into(),
                        msbuild: Some("sdk".into()),
                        target_frameworks: Some(vec!["net10.0".into()]),
                        version: None,
                        targets: None,
                        dependencies: None,
                        files: vec!["/w/dotnet/src/Eludite.Host/Program.cs".into()],
                        error: None,
                        startup: None,
                    },
                    WorkspaceProject {
                        name: "eludite-editor".into(),
                        path: "/w/crates/editor/Cargo.toml".into(),
                        kind: "cargo".into(),
                        msbuild: None,
                        target_frameworks: None,
                        version: Some("0.1.0".into()),
                        targets: Some(vec![WorkspaceTarget {
                            name: "eludite_editor".into(),
                            kind: "lib".into(),
                            path: "/w/crates/editor/src/lib.rs".into(),
                        }]),
                        dependencies: Some(vec!["rope".into()]),
                        files: vec!["/w/crates/editor/src/buffer.rs".into()],
                        error: None,
                        startup: None,
                    },
                ],
            }),
        )
        .unwrap();
        let all = r.invoke(WORKSPACE_TREE, json!({})).unwrap();
        assert_eq!(all["projects"].as_array().unwrap().len(), 2);
        assert_eq!(all["projects"][1]["targets"][0]["kind"], "lib");
        let one = r
            .invoke(WORKSPACE_TREE, json!({"project": "eludite-editor"}))
            .unwrap();
        assert_eq!(one["projects"][0]["kind"], "cargo");
        assert!(r.invoke(WORKSPACE_TREE, json!({"bogus": 1})).is_err());
        let schema: Value = serde_json::from_str(OUTPUT).unwrap();
        for k in all.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        let item = &schema["properties"]["projects"]["items"]["properties"];
        for p in all["projects"].as_array().unwrap() {
            for k in p.as_object().unwrap().keys() {
                assert!(item.get(k).is_some(), "{k}");
            }
            let kinds = item["kind"]["enum"].as_array().unwrap();
            assert!(kinds.contains(&p["kind"]));
        }
    }
}
