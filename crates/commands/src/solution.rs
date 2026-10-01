//! `eludite.solution.tree` (brief 0016): the open solution's projects and files, as the Workspace window shows them, for
//! agents (PLAN.md 5.4, the solution graph). Read from a caller-supplied source, like `diagnostics.list`, so it runs on
//! whichever thread invokes it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const SOLUTION_TREE: &str = "eludite.solution.tree";

const INPUT: &str = include_str!("../../../protocol/schemas/solution-tree.input.json");
const OUTPUT: &str = include_str!("../../../protocol/schemas/solution-tree.output.json");

/// Files a project lists at most.
pub const MAX_FILES: usize = 5000;

/// One project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeProject {
    pub name: String,
    pub path: String,
    /// `sdk` or `legacy`.
    pub kind: String,
    pub target_frameworks: Vec<String>,
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `solution-tree.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolutionTreeOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `none`, `loading`, `loaded` or `failed`.
    pub state: String,
    pub projects: Vec<TreeProject>,
}

impl Default for SolutionTreeOutput {
    fn default() -> Self {
        Self {
            path: None,
            state: "none".into(),
            projects: Vec::new(),
        }
    }
}

/// Where the command reads the tree. Called on the invoking thread; it must be cheap and thread-safe.
pub type TreeSource = Arc<dyn Fn() -> SolutionTreeOutput + Send + Sync>;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    project: Option<String>,
}

pub fn spec() -> CommandSpec {
    CommandSpec {
        id: CommandId::new(SOLUTION_TREE).expect("valid id"),
        title: "Workspace: List Projects and Files".into(),
        input_schema: serde_json::from_str(INPUT).expect("valid schema"),
        output_schema: serde_json::from_str(OUTPUT).expect("valid schema"),
        permission: PermissionClass::Read,
        agent_visible: true,
    }
}

/// Register `eludite.solution.tree`, reading from `source`.
pub fn register(registry: &mut CommandRegistry, source: TreeSource) -> Result<(), CommandError> {
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
    fn lists_and_filters_projects() {
        let mut r = CommandRegistry::new();
        register(
            &mut r,
            Arc::new(|| SolutionTreeOutput {
                path: Some("/s/App.slnx".into()),
                state: "loaded".into(),
                projects: vec![
                    TreeProject {
                        name: "App".into(),
                        path: "/s/App/App.csproj".into(),
                        kind: "sdk".into(),
                        target_frameworks: vec!["net10.0".into()],
                        files: vec!["/s/App/Program.cs".into()],
                        error: None,
                    },
                    TreeProject {
                        name: "Old".into(),
                        path: "/s/Old/Old.csproj".into(),
                        kind: "legacy".into(),
                        target_frameworks: vec!["net48".into()],
                        files: vec![],
                        error: Some("MSBuild not found".into()),
                    },
                ],
            }),
        )
        .unwrap();
        let all = r.invoke(SOLUTION_TREE, json!({})).unwrap();
        assert_eq!(all["projects"].as_array().unwrap().len(), 2);
        assert_eq!(all["state"], "loaded");
        let one = r.invoke(SOLUTION_TREE, json!({"project": "Old"})).unwrap();
        assert_eq!(one["projects"][0]["error"], "MSBuild not found");
        assert!(r.invoke(SOLUTION_TREE, json!({"bogus": 1})).is_err());
        assert!(r.lookup(SOLUTION_TREE).unwrap().agent_visible);
        let schema: Value = serde_json::from_str(OUTPUT).unwrap();
        for k in all.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
    }
}
