//! The fake host's solution structure, item properties, references and unloaded projects (brief 0081), for the
//! shell's tests.
//!
//! A test gives the solution folders ([`FakeHost::set_solution_folders`]: `eludite/solution/tree`'s `folders`) and the
//! file properties of a file ([`FakeHost::set_item_properties`]); the fake keeps a small model of them and of the
//! tree's projects and applies `eludite/solution/edit`, `eludite/project/setItemProperties`,
//! `eludite/project/editReferences` and `eludite/project/setUnloaded` to it the way the real host changes the files:
//! each write moves the generation on and sends `eludite/solution/status` `loaded` (the shell then asks for the tree
//! again, which shows the change). A reference a test marks with [`FakeHost::set_reference_cycle`] is refused as a
//! cycle. Every request is recorded like any other message; a stale `generation` answers -32801.

use std::collections::{BTreeSet, HashMap};

use eludite_protocol::host::{self, methods};
use serde_json::{Value, json};

use super::FakeHost;

/// The fake's model.
#[derive(Default)]
pub(super) struct FakeStructure {
    /// `eludite/solution/tree`'s `folders`, as given and as edited.
    folders: Vec<Value>,
    /// Unloaded project files.
    unloaded: BTreeSet<String>,
    /// File properties by file path (`project-item-properties.json` result members).
    items: HashMap<String, Value>,
    /// (project, reference) pairs refused as a cycle.
    cycles: BTreeSet<(String, String)>,
    /// `setDependencies` calls: project to its list.
    pub(super) dependencies: HashMap<String, Vec<String>>,
}

fn name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl FakeHost {
    /// The solution folders `eludite/solution/tree` answers (`solution-tree.json` `$defs.solutionFolder` objects).
    pub fn set_solution_folders(&self, folders: Value) {
        self.lock().structure.folders = folders.as_array().cloned().unwrap_or_default();
    }

    /// The solution folders as the fake holds them now.
    pub fn solution_folders(&self) -> Value {
        Value::Array(self.lock().structure.folders.clone())
    }

    /// The file properties `eludite/project/itemProperties` answers for `path` (result members; the defaults fill
    /// in the rest).
    pub fn set_item_properties(&self, path: &str, properties: Value) {
        self.lock()
            .structure
            .items
            .insert(path.to_owned(), properties);
    }

    /// The file properties the fake holds for `path` now.
    pub fn item_properties(&self, path: &str) -> Option<Value> {
        self.lock().structure.items.get(path).cloned()
    }

    /// Refuse adding `reference` to `project` as a cycle.
    pub fn set_reference_cycle(&self, project: &str, reference: &str) {
        self.lock()
            .structure
            .cycles
            .insert((project.to_owned(), reference.to_owned()));
    }

    /// The unloaded project files.
    pub fn unloaded_projects(&self) -> Vec<String> {
        self.lock().structure.unloaded.iter().cloned().collect()
    }

    /// The tree result with the folders, the projects' folders kept and the unloaded projects marked.
    pub(super) fn structure_tree(&self, mut result: Value) -> Value {
        let s = self.lock();
        if result["path"].is_null() {
            return result;
        }
        if !s.structure.folders.is_empty() {
            result["folders"] = Value::Array(s.structure.folders.clone());
        }
        if !s.structure.unloaded.is_empty() {
            result["unloaded"] = json!(s.structure.unloaded.iter().collect::<Vec<_>>());
            for p in result["projects"].as_array_mut().into_iter().flatten() {
                if p["path"]
                    .as_str()
                    .is_some_and(|x| s.structure.unloaded.contains(x))
                {
                    p["unloaded"] = json!(true);
                    p["files"] = json!([]);
                    if let Some(o) = p.as_object_mut() {
                        o.remove("dependencies");
                    }
                }
            }
        }
        result
    }

    /// Move the generation on and say the solution loaded, as the real host's reload does; returns the generation.
    fn structure_reload(&self, notify: &dyn Fn(&str, Value)) -> u64 {
        let (next, solution, count) = {
            let mut s = self.lock();
            s.generation += 1;
            (
                s.generation,
                s.solution.clone().unwrap_or_default(),
                s.tree.as_array().map_or(0, Vec::len),
            )
        };
        notify(
            methods::SOLUTION_STATUS,
            json!({"generation": next, "path": solution, "state": "loaded", "elapsedMs": 1.0,
                   "counts": {"projects": count, "legacyProjects": 0, "legacyEvaluationFailures": 0}}),
        );
        next
    }

    pub(super) fn answer_structure(
        &self,
        method: &str,
        params: &Value,
        reply: &dyn Fn(Value),
        error: &dyn Fn(i64, &str, Option<Value>),
        notify: &dyn Fn(&str, Value),
    ) {
        let current = self.generation();
        if method != methods::PROJECT_SET_UNLOADED {
            match params["generation"].as_u64() {
                Some(g) if g == current => {}
                Some(g) => {
                    return error(
                        host::error_codes::CONTENT_MODIFIED,
                        "stale",
                        Some(json!({"requestedGeneration": g, "currentGeneration": current})),
                    );
                }
                None => return error(-32602, "generation is required", None),
            }
        }
        let Some(solution) = self.lock().solution.clone() else {
            return error(-32602, "no solution is open", None);
        };
        match method {
            methods::SOLUTION_EDIT => self.solution_edit(&solution, params, reply, error, notify),
            methods::PROJECT_ITEM_PROPERTIES => {
                let path = params["path"].as_str().unwrap_or_default();
                let mut r = json!({"generation": current, "project": params["project"], "path": path,
                    "listed": true, "itemType": "None",
                    "itemTypes": ["None", "Compile", "Content", "EmbeddedResource", "AdditionalFiles", "Page"],
                    "copyToOutputDirectory": "never"});
                if let Some(Value::Object(given)) = self.lock().structure.items.get(path) {
                    for (k, v) in given {
                        r[k] = v.clone();
                    }
                }
                reply(r);
            }
            methods::PROJECT_SET_ITEM_PROPERTIES => {
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                {
                    let mut s = self.lock();
                    let entry = s.structure.items.entry(path).or_insert_with(|| json!({}));
                    for k in [
                        "itemType",
                        "copyToOutputDirectory",
                        "customTool",
                        "customToolNamespace",
                    ] {
                        if !params[k].is_null() {
                            entry[k] = params[k].clone();
                        }
                    }
                }
                let next = self.structure_reload(notify);
                reply(json!({"generation": next, "project": params["project"], "written": true}));
            }
            methods::PROJECT_EDIT_REFERENCES => {
                let project = params["project"].as_str().unwrap_or_default().to_owned();
                let mut refused = Vec::new();
                let mut written = false;
                {
                    let mut s = self.lock();
                    let cycles = s.structure.cycles.clone();
                    let Some(p) = s
                        .tree
                        .as_array_mut()
                        .and_then(|a| a.iter_mut().find(|p| p["path"] == project.as_str()))
                    else {
                        drop(s);
                        return error(
                            -32602,
                            &format!("{project} is not a project of the solution"),
                            None,
                        );
                    };
                    if p["dependencies"].is_null() {
                        p["dependencies"] = json!({"restored": true, "packages": [], "projects": [], "frameworks": []});
                    }
                    let deps = &mut p["dependencies"];
                    for edit in params["add"].as_array().into_iter().flatten() {
                        let r = edit["reference"].as_str().unwrap_or_default().to_owned();
                        if cycles.contains(&(project.clone(), r.clone())) {
                            refused.push(json!({"reference": r, "reason": format!("cycle: {} -> {} -> {}", name_of(&project), name_of(&r), name_of(&project))}));
                            continue;
                        }
                        let list = if edit["kind"] == "project" {
                            &mut deps["projects"]
                        } else {
                            if deps["assemblies"].is_null() {
                                deps["assemblies"] = json!([]);
                            }
                            &mut deps["assemblies"]
                        };
                        let row = if edit["kind"] == "project" {
                            json!({"name": name_of(&r), "path": r})
                        } else {
                            json!({"name": name_of(&r), "include": name_of(&r), "path": r})
                        };
                        if let Some(a) = list.as_array_mut()
                            && !a
                                .iter()
                                .any(|x| x["path"] == row["path"] || x["include"] == r.as_str())
                        {
                            a.push(row);
                            written = true;
                        }
                    }
                    for edit in params["remove"].as_array().into_iter().flatten() {
                        let r = edit["reference"].as_str().unwrap_or_default();
                        let mut found = false;
                        for key in ["projects", "assemblies"] {
                            if let Some(a) = deps[key].as_array_mut() {
                                let before = a.len();
                                a.retain(|x| x["path"] != r && x["include"] != r && x["name"] != r);
                                found |= a.len() != before;
                            }
                        }
                        if found {
                            written = true;
                        } else {
                            refused.push(json!({"reference": r, "reason": "notReferenced"}));
                        }
                    }
                }
                let generation = if written {
                    self.structure_reload(notify)
                } else {
                    current
                };
                reply(
                    json!({"generation": generation, "project": project, "written": written, "refused": refused}),
                );
            }
            methods::PROJECT_SET_UNLOADED => {
                let unloaded = params["unloaded"] == true;
                let unloaded_now: Vec<String> = {
                    let mut s = self.lock();
                    for p in params["projects"].as_array().into_iter().flatten() {
                        let p = p.as_str().unwrap_or_default().to_owned();
                        if unloaded {
                            s.structure.unloaded.insert(p);
                        } else {
                            s.structure.unloaded.remove(&p);
                        }
                    }
                    s.structure.unloaded.iter().cloned().collect()
                };
                let next = self.structure_reload(notify);
                reply(json!({"generation": next, "unloaded": unloaded_now}));
            }
            _ => error(-32601, "not a brief 0081 method", None),
        }
    }

    fn solution_edit(
        &self,
        solution: &str,
        params: &Value,
        reply: &dyn Fn(Value),
        error: &dyn Fn(i64, &str, Option<Value>),
        notify: &dyn Fn(&str, Value),
    ) {
        let str_of = |k: &str| params[k].as_str().map(str::to_owned);
        let action = params["action"].as_str().unwrap_or_default();
        let mut folder_out = None;
        let mut apply = || -> Result<bool, String> {
            let mut guard = self.lock();
            let s = &mut *guard;
            let st = &mut s.structure;
            let folder_ix =
                |folders: &[Value], f: &str| folders.iter().position(|x| x["path"] == f);
            match action {
                "addProject" => {
                    let project = str_of("project").unwrap_or_default();
                    let folder = str_of("folder");
                    let tree = s.tree.as_array_mut();
                    match tree {
                        Some(t) if t.iter().any(|p| p["path"] == project.as_str()) => Ok(false),
                        Some(t) => {
                            let mut p = json!({"name": name_of(&project), "path": project, "kind": "sdk",
                                               "targetFrameworks": ["net10.0"], "files": []});
                            if let Some(f) = folder {
                                p["solutionFolder"] = json!(f);
                            }
                            t.push(p);
                            Ok(true)
                        }
                        None => Err("no tree".into()),
                    }
                }
                "removeProject" => {
                    let project = str_of("project").unwrap_or_default();
                    let t = s.tree.as_array_mut().ok_or_else(|| "no tree".to_owned())?;
                    let before = t.len();
                    t.retain(|p| p["path"] != project.as_str());
                    Ok(t.len() != before)
                }
                "addFolder" => {
                    let name = str_of("name").unwrap_or_default();
                    let parent = str_of("folder").unwrap_or_else(|| "/".into());
                    let path = format!("{parent}{name}/");
                    if folder_ix(&st.folders, &path).is_some() {
                        Err(format!("{path} exists"))
                    } else {
                        st.folders
                            .push(json!({"path": path, "name": name, "files": []}));
                        folder_out = Some(path);
                        Ok(true)
                    }
                }
                "renameFolder" => {
                    let from = str_of("folder").unwrap_or_default();
                    let name = str_of("name").unwrap_or_default();
                    let parent = from
                        .trim_end_matches('/')
                        .rsplit_once('/')
                        .map(|(p, _)| format!("{p}/"))
                        .unwrap_or_else(|| "/".into());
                    let to = format!("{parent}{name}/");
                    for f in st.folders.iter_mut() {
                        if let Some(rest) = f["path"].as_str().and_then(|p| p.strip_prefix(&from)) {
                            let new = format!("{to}{rest}");
                            if rest.is_empty() {
                                f["name"] = json!(name);
                            }
                            f["path"] = json!(new);
                        }
                    }
                    for p in s.tree.as_array_mut().into_iter().flatten() {
                        if let Some(rest) = p["solutionFolder"]
                            .as_str()
                            .and_then(|x| x.strip_prefix(&from))
                        {
                            p["solutionFolder"] = json!(format!("{to}{rest}"));
                        }
                    }
                    folder_out = Some(to);
                    Ok(true)
                }
                "removeFolder" => {
                    let from = str_of("folder").unwrap_or_default();
                    st.folders
                        .retain(|f| !f["path"].as_str().is_some_and(|p| p.starts_with(&from)));
                    if let Some(t) = s.tree.as_array_mut() {
                        t.retain(|p| {
                            !p["solutionFolder"]
                                .as_str()
                                .is_some_and(|x| x.starts_with(&from))
                        });
                    }
                    Ok(true)
                }
                "addFile" | "removeFile" => {
                    let f = str_of("folder").unwrap_or_default();
                    let file = str_of("file").unwrap_or_default();
                    match folder_ix(&st.folders, &f) {
                        None => Err(format!("{f} is not a solution folder")),
                        Some(i) => {
                            let files = st.folders[i]["files"]
                                .as_array_mut()
                                .ok_or_else(|| "files".to_owned())?;
                            let had = files.iter().any(|x| x == file.as_str());
                            if action == "addFile" && !had {
                                files.push(json!(file));
                                Ok(true)
                            } else if action == "removeFile" && had {
                                files.retain(|x| x != file.as_str());
                                Ok(true)
                            } else {
                                Ok(false)
                            }
                        }
                    }
                }
                "setDependencies" => {
                    let project = str_of("project").unwrap_or_default();
                    let list = params["dependsOn"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                    st.dependencies.insert(project, list);
                    Ok(true)
                }
                other => Err(format!("unknown action {other}")),
            }
        };
        let outcome = apply();
        match outcome {
            Err(e) => error(-32602, &e, None),
            Ok(written) => {
                let generation = if written {
                    self.structure_reload(notify)
                } else {
                    self.generation()
                };
                let mut r = json!({"generation": generation, "path": solution, "written": written});
                if let Some(f) = folder_out {
                    r["folder"] = json!(f);
                }
                if !written {
                    r["message"] = json!("nothing to change");
                }
                reply(r);
            }
        }
    }
}
