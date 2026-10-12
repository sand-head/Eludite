//! The fake host's `eludite/project/editItems` and `eludite/solution/reload` (brief 0073), for the shell's tests.
//!
//! `eludite/solution/reload` advances the generation and says the solution loaded again, as the real host does after
//! reopening it. `eludite/project/editItems` applies each edit to the fake's tree (`set_tree`'s projects): `include`
//! adds a file (Compile for code, else None), `exclude` and `remove` drop the files at or under the path, `rename`
//! rewrites them, `moveUp` and `moveDown` swap a file with the previous or next Compile file of its project
//! (F# compile order), `addFolder` adds a Folder item. When anything changed the generation advances like a reload.
//! Every request is recorded like any other message ([`FakeHost::received_params`]).

use eludite_protocol::host::methods;
use serde_json::{Value, json};

use super::FakeHost;

impl FakeHost {
    pub(super) fn answer_items(
        &self,
        method: &str,
        params: &Value,
        reply: &dyn Fn(Value),
        notify: &dyn Fn(&str, Value),
    ) {
        let changed = match method {
            methods::SOLUTION_RELOAD => self.lock().solution.is_some(),
            _ => {
                let project = params["project"].as_str().unwrap_or_default().to_owned();
                let mut results = Vec::new();
                let mut changed = false;
                {
                    let mut s = self.lock();
                    if let Some(projects) = s.tree.as_array_mut()
                        && let Some(p) = projects.iter_mut().find(|p| p["path"] == project)
                    {
                        for edit in params["edits"].as_array().into_iter().flatten() {
                            let applied = apply(p, edit);
                            changed |= applied;
                            results.push(json!({"op": edit["op"], "path": edit["path"], "applied": applied}));
                        }
                    }
                }
                let generation = self.advance(changed, notify);
                return reply(json!({"changed": changed, "generation": generation, "edits": results}));
            }
        };
        let generation = self.advance(changed, notify);
        reply(json!({ "generation": generation }));
    }

    /// One more generation and a loaded solution, when `changed`; the generation.
    fn advance(&self, changed: bool, notify: &dyn Fn(&str, Value)) -> u64 {
        if !changed {
            return self.generation();
        }
        let (generation, path) = {
            let mut s = self.lock();
            s.generation += 1;
            (s.generation, s.solution.clone())
        };
        notify(
            methods::SOLUTION_STATUS,
            json!({"generation": generation, "path": path, "state": "loaded", "elapsedMs": 1.0,
                   "counts": {"projects": 1, "legacyProjects": 0, "legacyEvaluationFailures": 0}}),
        );
        generation
    }
}

fn under(file: &str, path: &str) -> bool {
    file == path
        || file
            .strip_prefix(path)
            .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('\\'))
}

/// Apply one edit to the fake project `p`; whether it changed it.
fn apply(p: &mut Value, edit: &Value) -> bool {
    let path = edit["path"].as_str().unwrap_or_default().to_owned();
    let Some(files) = p["files"].as_array_mut() else {
        return false;
    };
    match edit["op"].as_str().unwrap_or_default() {
        "include" => {
            if files.iter().any(|f| f["path"] == path.as_str()) {
                return false;
            }
            let code = [".cs", ".vb", ".fs"].iter().any(|e| path.ends_with(e));
            files.push(json!({"path": path, "itemType": if code { "compile" } else { "none" }}));
            true
        }
        "exclude" | "remove" => {
            let before = files.len();
            files.retain(|f| !under(f["path"].as_str().unwrap_or_default(), &path));
            files.len() != before
        }
        "rename" => {
            let to = edit["to"].as_str().unwrap_or_default();
            let mut changed = false;
            for f in files.iter_mut() {
                let old = f["path"].as_str().unwrap_or_default().to_owned();
                if under(&old, &path) {
                    f["path"] = json!(format!("{to}{}", &old[path.len()..]));
                    changed = true;
                }
            }
            changed
        }
        op @ ("moveUp" | "moveDown") => {
            let compile: Vec<usize> = (0..files.len())
                .filter(|&i| files[i]["itemType"] == "compile")
                .collect();
            let Some(at) = compile.iter().position(|&i| files[i]["path"] == path.as_str()) else {
                return false;
            };
            let other = if op == "moveUp" {
                at.checked_sub(1)
            } else {
                Some(at + 1).filter(|&j| j < compile.len())
            };
            match other {
                Some(j) => {
                    files.swap(compile[at], compile[j]);
                    true
                }
                None => false,
            }
        }
        "addFolder" => {
            files.push(json!({"path": path, "itemType": "folder"}));
            true
        }
        _ => false,
    }
}
