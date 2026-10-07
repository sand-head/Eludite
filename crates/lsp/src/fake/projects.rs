//! The fake host's `eludite/project/*` and solution configuration service (brief 0049), for the shell's tests.
//!
//! A test gives each project its catalog answer ([`FakeHost::set_project_properties`]: the `eludite/project/properties`
//! result as the real host answers it in its default configuration), its launch profiles
//! ([`FakeHost::set_launch_profiles`]) and the solution's configurations ([`FakeHost::set_solution_configurations`]).
//! The fake then follows the real host's rules on that model: a value set for a configuration and platform is kept
//! for that pair and answered with `source: conditioned` there; an unconditioned value replaces the default one; a
//! value equal to the default (or null) removes it; an inherited property is written only with `override`;
//! `allConfigurations` drops the conditioned values; a write that changes anything reloads the solution (the
//! generation moves on, with `loading` and `loaded` statuses) and a stale `generation` is refused with -32801. Every
//! request is recorded like any other message.

use std::collections::BTreeMap;

use eludite_protocol::host::{self, methods};
use serde_json::{Value, json};

use super::FakeHost;

/// The fake's model of the projects (see the module docs).
#[derive(Default)]
pub(super) struct FakeProjects {
    /// The `eludite/project/properties` result per project path, as given.
    properties: BTreeMap<String, Value>,
    /// Values per project, `configuration|platform` (platform as MSBuild names it) and property.
    conditioned: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    /// `launchSettings.json` per project: its profiles in order.
    profiles: BTreeMap<String, Vec<Value>>,
    /// The `eludite/solution/configurations` result (without the generation and the active selection).
    configurations: Option<Value>,
    active: Option<Value>,
    /// Answer `eludite/project/properties` only after this long.
    delay: std::time::Duration,
}

fn project_platform(p: &str) -> String {
    if p.replace(' ', "").eq_ignore_ascii_case("AnyCPU") {
        "AnyCPU".into()
    } else {
        p.to_owned()
    }
}

impl FakeHost {
    /// The `eludite/project/properties` answer for `project` in its default configuration (`configuration`,
    /// `platform`, `properties`, ...; see `protocol/schemas/host/project-properties.json`).
    pub fn set_project_properties(&self, project: &str, result: Value) {
        self.lock()
            .projects
            .properties
            .insert(project.to_owned(), result);
    }

    /// Answer `eludite/project/properties` only after `delay` (the pages open before the values).
    pub fn set_project_properties_delay(&self, delay: std::time::Duration) {
        self.lock().projects.delay = delay;
    }

    /// `project`'s launch profiles, in the file's order (`launch-profiles.json` `$defs.profile` objects).
    pub fn set_launch_profiles(&self, project: &str, profiles: Value) {
        self.lock().projects.profiles.insert(
            project.to_owned(),
            profiles.as_array().cloned().unwrap_or_default(),
        );
    }

    /// The `eludite/solution/configurations` answer (`configurations`, `platforms`, `projects`, `format`).
    pub fn set_solution_configurations(&self, configurations: Value) {
        self.lock().projects.configurations = Some(configurations);
    }

    /// The value the fake holds for `project`'s `property` under `configuration|platform` (None: the unconditioned
    /// one).
    pub fn project_property(
        &self,
        project: &str,
        property: &str,
        condition: Option<(&str, &str)>,
    ) -> Option<String> {
        let s = self.lock();
        match condition {
            Some((c, p)) => s
                .projects
                .conditioned
                .get(project)?
                .get(&format!("{c}|{}", project_platform(p)))?
                .get(property)
                .cloned(),
            None => s.projects.properties.get(project)?["properties"]
                .as_array()?
                .iter()
                .find(|x| x["name"] == property)
                .and_then(|x| x["value"].as_str().map(str::to_owned)),
        }
    }

    /// The fake's solution configurations, with the generation and the selection.
    fn configurations_result(&self) -> Value {
        let s = self.lock();
        let mut r = s.projects.configurations.clone().unwrap_or_else(|| {
            json!({"configurations": ["Debug", "Release"], "platforms": ["Any CPU"], "projects": []})
        });
        r["generation"] = json!(s.generation);
        if let Some(p) = &s.solution {
            r["path"] = json!(p);
        }
        let active = s.projects.active.clone().unwrap_or_else(|| {
            json!({"configuration": r["configurations"][0].as_str().unwrap_or("Debug"),
                   "platform": r["platforms"][0].as_str().unwrap_or("Any CPU")})
        });
        r["active"] = active;
        r
    }

    /// Reload the solution as the real host does after a write: the generation moves on, then `loading`, `loaded`.
    pub(super) fn reload(&self, notify: &dyn Fn(&str, Value)) -> host::Generation {
        let (generation, path, projects) = {
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
            json!({"generation": generation, "path": path, "state": "loading", "phase": "projectLoad"}),
        );
        notify(
            methods::SOLUTION_STATUS,
            json!({"generation": generation, "path": path, "state": "loaded", "elapsedMs": 1.0,
                   "counts": {"projects": projects, "legacyProjects": 0, "legacyEvaluationFailures": 0}}),
        );
        generation
    }

    fn evaluated(
        &self,
        project: &str,
        configuration: Option<&str>,
        platform: Option<&str>,
    ) -> Option<Value> {
        let s = self.lock();
        let mut r = s.projects.properties.get(project)?.clone();
        let configuration = configuration
            .map(str::to_owned)
            .unwrap_or_else(|| r["configuration"].as_str().unwrap_or("Debug").to_owned());
        let platform = project_platform(
            platform.unwrap_or_else(|| r["platform"].as_str().unwrap_or("AnyCPU")),
        );
        let key = format!("{configuration}|{platform}");
        let values = s
            .projects
            .conditioned
            .get(project)
            .and_then(|c| c.get(&key));
        let all: Vec<(String, String)> = s
            .projects
            .conditioned
            .get(project)
            .into_iter()
            .flatten()
            .flat_map(|(k, m)| m.keys().map(move |name| (k.clone(), name.clone())))
            .collect();
        for p in r["properties"].as_array_mut().into_iter().flatten() {
            // The test's default for the property is the fake's own note, not part of the answer.
            if let Some(o) = p.as_object_mut() {
                o.remove("x-default");
            }
            let name = p["name"].as_str().unwrap_or_default().to_owned();
            let conditions: Vec<Value> = all
                .iter()
                .filter(|(_, n)| *n == name)
                .map(|(k, _)| {
                    let (c, pl) = k.split_once('|').unwrap_or((k, ""));
                    json!(format!("'$(Configuration)|$(Platform)'=='{c}|{pl}'"))
                })
                .collect();
            if !conditions.is_empty() {
                p["conditions"] = Value::Array(conditions);
            }
            if let Some(v) = values.and_then(|m| m.get(&name)) {
                p["value"] = json!(v);
                p["raw"] = json!(v);
                p["source"] = json!("conditioned");
                p["conditioned"] = json!(true);
                p["inherited"] = json!(false);
                p["definedIn"] = json!({"file": project, "line": 10,
                    "condition": format!("'$(Configuration)|$(Platform)'=='{key}'")});
                if let Some(o) = p.as_object_mut() {
                    o.remove("inheritedFrom");
                }
            }
        }
        r["configuration"] = json!(configuration);
        r["platform"] = json!(platform);
        r["generation"] = json!(s.generation);
        r["project"] = json!(project);
        Some(r)
    }

    pub(super) fn answer_projects(
        &self,
        method: &str,
        params: &Value,
        reply: &dyn Fn(Value),
        error: &dyn Fn(i64, &str, Option<Value>),
        notify: &dyn Fn(&str, Value),
    ) {
        let current = self.generation();
        let stale = |requested: Option<u64>| match requested {
            Some(g) if g == current => false,
            Some(g) => {
                error(
                    host::error_codes::CONTENT_MODIFIED,
                    "stale",
                    Some(json!({"requestedGeneration": g, "currentGeneration": current})),
                );
                true
            }
            None => {
                error(-32602, "generation is required", None);
                true
            }
        };
        let project = params["project"].as_str().unwrap_or_default().to_owned();
        match method {
            methods::PROJECT_PROPERTIES => {
                let delay = self.lock().projects.delay;
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                match self.evaluated(
                    &project,
                    params["configuration"].as_str(),
                    params["platform"].as_str(),
                ) {
                    Some(mut r) => {
                        if let Some(f) = params["framework"].as_str() {
                            r["framework"] = json!(f);
                        }
                        reply(r)
                    }
                    None => error(
                        -32602,
                        &format!("{project} is not a project of the solution"),
                        None,
                    ),
                }
            }
            methods::PROJECT_SET_PROPERTY => {
                if stale(params["generation"].as_u64()) {
                    return;
                }
                if !self.lock().projects.properties.contains_key(&project) {
                    return error(
                        -32602,
                        &format!("{project} is not a project of the solution"),
                        None,
                    );
                }
                let mut results = Vec::new();
                let mut written = false;
                for edit in params["edits"].as_array().into_iter().flatten() {
                    let name = edit["name"].as_str().unwrap_or_default().to_owned();
                    let value = edit["value"].as_str().map(str::to_owned);
                    let mut s = self.lock();
                    let Some(base) = s.projects.properties.get_mut(&project) else {
                        break;
                    };
                    let Some(prop) = base["properties"]
                        .as_array_mut()
                        .and_then(|a| a.iter_mut().find(|p| p["name"] == name.as_str()))
                    else {
                        drop(s);
                        return error(
                            -32602,
                            &format!("{name} is not a property of the catalog"),
                            None,
                        );
                    };
                    let default = prop["x-default"].as_str().unwrap_or_default().to_owned();
                    let mut result = json!({"name": name, "status": "unchanged"});
                    if edit["allConfigurations"] == true {
                        let removed: Vec<String> = s
                            .projects
                            .conditioned
                            .get_mut(&project)
                            .into_iter()
                            .flat_map(|c| c.iter_mut())
                            .filter_map(|(k, m)| {
                                m.remove(&name)
                                    .map(|_| format!("'$(Configuration)|$(Platform)'=='{k}'"))
                            })
                            .collect();
                        if !removed.is_empty() {
                            result["removedConditions"] = json!(removed);
                            result["status"] = json!("written");
                            written = true;
                        }
                    }
                    let prop = s
                        .projects
                        .properties
                        .get_mut(&project)
                        .and_then(|b| b["properties"].as_array_mut())
                        .and_then(|a| a.iter_mut().find(|p| p["name"] == name.as_str()))
                        .expect("found above");
                    if let Some(c) = edit["configuration"].as_str() {
                        let platform =
                            project_platform(edit["platform"].as_str().unwrap_or("AnyCPU"));
                        let key = format!("{c}|{platform}");
                        result["condition"] =
                            json!(format!("'$(Configuration)|$(Platform)'=='{key}'"));
                        let map = s
                            .projects
                            .conditioned
                            .entry(project.clone())
                            .or_default()
                            .entry(key)
                            .or_default();
                        match value {
                            Some(v) if v != default => {
                                if map.get(&name) != Some(&v) {
                                    map.insert(name.clone(), v);
                                    result["status"] = json!("written");
                                    result["line"] = json!(10);
                                    written = true;
                                }
                            }
                            _ => {
                                if map.remove(&name).is_some() {
                                    result["status"] = json!("removed");
                                    written = true;
                                }
                            }
                        }
                    } else if prop["source"] == "inherited" && edit["override"] != true {
                        result["status"] = json!("inherited");
                        result["inheritedFrom"] = prop["inheritedFrom"].clone();
                    } else {
                        let new = value.clone().unwrap_or_else(|| default.clone());
                        if prop["value"].as_str() != Some(new.as_str())
                            || prop["source"] == "inherited"
                        {
                            let removed = value.is_none() || new == default;
                            prop["value"] = json!(new);
                            prop["raw"] = if removed { Value::Null } else { json!(new) };
                            prop["source"] = json!(if removed { "default" } else { "project" });
                            prop["inherited"] = json!(false);
                            if let Some(o) = prop.as_object_mut() {
                                o.remove("inheritedFrom");
                                o.remove("definedIn");
                            }
                            if !removed {
                                prop["definedIn"] = json!({"file": project, "line": 5});
                            }
                            if result["status"] == "unchanged" {
                                result["status"] =
                                    json!(if removed { "removed" } else { "written" });
                            }
                            if !removed {
                                result["line"] = json!(5);
                            }
                            written = true;
                        }
                    }
                    results.push(result);
                }
                let generation = if written {
                    self.reload(notify)
                } else {
                    current
                };
                reply(
                    json!({"generation": generation, "project": project, "written": written, "results": results}),
                );
            }
            methods::PROJECT_LAUNCH_PROFILES => {
                let profiles = self.lock().projects.profiles.get(&project).cloned();
                reply(self.profiles_result(&project, profiles));
            }
            methods::PROJECT_SET_LAUNCH_PROFILE => {
                if stale(params["generation"].as_u64()) {
                    return;
                }
                let name = params["profile"].as_str().unwrap_or_default().to_owned();
                let mut s = self.lock();
                let list = s.projects.profiles.entry(project.clone()).or_default();
                let at = list.iter().position(|p| p["name"] == name.as_str());
                let refuse = |msg: &str| error(-32602, msg, None);
                match (params["action"].as_str().unwrap_or_default(), at) {
                    ("create", None) => {
                        let mut p = json!({"name": name, "commandName": "Project", "environmentVariables": []});
                        apply_values(&mut p, &params["values"]);
                        list.push(p);
                    }
                    ("set", Some(i)) => apply_values(&mut list[i], &params["values"]),
                    ("rename", Some(i)) => {
                        let new = params["newName"].as_str().unwrap_or_default();
                        if list.iter().any(|p| p["name"] == new) {
                            drop(s);
                            return refuse("a profile with that name exists");
                        }
                        list[i]["name"] = json!(new);
                    }
                    ("delete", Some(i)) => {
                        list.remove(i);
                    }
                    _ => {
                        drop(s);
                        return refuse("no such profile, or it exists");
                    }
                }
                let profiles = Some(list.clone());
                drop(s);
                reply(self.profiles_result(&project, profiles));
            }
            methods::SOLUTION_CONFIGURATIONS => reply(self.configurations_result()),
            methods::SOLUTION_SET_CONFIGURATION => {
                if stale(params["generation"].as_u64()) {
                    return;
                }
                if !params["select"].is_null() {
                    self.lock().projects.active = Some(params["select"].clone());
                }
                let mut written = false;
                for m in params["mappings"].as_array().into_iter().flatten() {
                    let mut s = self.lock();
                    let Some(c) = s.projects.configurations.as_mut() else {
                        break;
                    };
                    for p in c["projects"].as_array_mut().into_iter().flatten() {
                        if p["path"] != m["project"] {
                            continue;
                        }
                        for row in p["mappings"].as_array_mut().into_iter().flatten() {
                            if row["solutionConfiguration"] == m["solutionConfiguration"]
                                && row["solutionPlatform"] == m["solutionPlatform"]
                            {
                                for k in ["configuration", "platform", "build"] {
                                    if !m[k].is_null() && row[k] != m[k] {
                                        row[k] = m[k].clone();
                                        written = true;
                                    }
                                }
                            }
                        }
                    }
                }
                let generation = if written {
                    self.reload(notify)
                } else {
                    current
                };
                let r = self.configurations_result();
                reply(
                    json!({"generation": generation, "path": r["path"], "written": written, "active": r["active"]}),
                );
            }
            other => error(-32601, &format!("{other} not found"), None),
        }
    }

    fn profiles_result(&self, project: &str, profiles: Option<Vec<Value>>) -> Value {
        let file = std::path::Path::new(project)
            .parent()
            .map(|d| d.join("Properties").join("launchSettings.json"))
            .unwrap_or_default();
        json!({"generation": self.generation(), "project": project, "file": file,
               "exists": profiles.is_some(), "profiles": profiles.unwrap_or_default()})
    }
}

/// Apply `setLaunchProfile`'s `values` (camelCase; null removes; the environment table is replaced in order).
fn apply_values(profile: &mut Value, values: &Value) {
    for (k, v) in values.as_object().into_iter().flatten() {
        if v.is_null() {
            if let Some(o) = profile.as_object_mut() {
                o.remove(k);
            }
            if k == "environmentVariables" {
                profile[k] = json!([]);
            }
        } else {
            profile[k] = v.clone();
        }
    }
}

impl FakeHost {
    /// A sample `eludite/project/properties` answer for `project` (an SDK project in `Debug|AnyCPU` targeting
    /// `frameworks`), with a few properties of each page and type: an inherited `LangVersion` and `Authors`
    /// (from `<dir>/Directory.Build.props`), per-configuration `DefineConstants`, `Optimize`, `TreatWarningsAsErrors`
    /// and `OutputPath`, and the Win32 icon read-only. Each property's `x-default` is the default the fake compares an
    /// edit with (dropped from the answers).
    pub fn sample_project_properties(project: &str, frameworks: &[&str]) -> Value {
        let dir = std::path::Path::new(project)
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = std::path::Path::new(project)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let props = format!("{dir}/Directory.Build.props");
        let prop = |name: &str,
                    page: &str,
                    section: &str,
                    label: &str,
                    kind: &str,
                    per: bool,
                    value: &str| {
            json!({"name": name, "page": page, "section": section, "label": label, "type": kind,
                   "perConfiguration": per, "value": value, "source": "default", "conditioned": false,
                   "inherited": false, "x-default": value})
        };
        let mut assembly = prop(
            "AssemblyName",
            "application",
            "General",
            "Assembly name",
            "string",
            false,
            &name,
        );
        assembly["source"] = json!("default");
        let mut output = prop(
            "OutputType",
            "application",
            "General",
            "Output type",
            "enum",
            false,
            "Exe",
        );
        output["values"] = json!([{"value": "Exe", "label": "Console Application"},
                                  {"value": "WinExe", "label": "Windows Application"},
                                  {"value": "Library", "label": "Class Library"}]);
        output["source"] = json!("project");
        output["raw"] = json!("Exe");
        output["definedIn"] = json!({"file": project, "line": 3});
        output["x-default"] = json!("Library");
        let mut nullable = prop(
            "Nullable",
            "application",
            "General",
            "Nullable",
            "enum",
            false,
            "",
        );
        nullable["values"] = json!([{"value": "disable", "label": "Disable"}, {"value": "enable", "label": "Enable"},
                                    {"value": "warnings", "label": "Warnings"},
                                    {"value": "annotations", "label": "Annotations"}]);
        let mut implicit = prop(
            "ImplicitUsings",
            "application",
            "General",
            "Implicit global usings",
            "bool",
            false,
            "",
        );
        implicit["trueValue"] = json!("enable");
        implicit["falseValue"] = json!("disable");
        let mut lang = prop(
            "LangVersion",
            "application",
            "General",
            "Language version",
            "enum",
            false,
            "12.0",
        );
        lang["values"] = json!([{"value": "default", "label": "Default"}, {"value": "latest", "label": "Latest"},
                                {"value": "13.0", "label": "C# 13.0"}, {"value": "12.0", "label": "C# 12.0"}]);
        lang["source"] = json!("inherited");
        lang["inherited"] = json!(true);
        lang["inheritedFrom"] = json!(props);
        lang["raw"] = json!("12.0");
        lang["definedIn"] = json!({"file": props, "line": 4});
        let mut icon = prop(
            "ApplicationIcon",
            "application",
            "Win32 resources",
            "Icon",
            "path",
            false,
            "",
        );
        icon["readOnly"] = json!(true);
        icon["readOnlyReason"] = json!("Win32 resources are edited on Windows.");
        let mut define = prop(
            "DefineConstants",
            "build",
            "General",
            "Conditional compilation symbols",
            "list",
            true,
            "TRACE;DEBUG",
        );
        define["x-default"] = json!("TRACE;DEBUG");
        let mut optimize = prop(
            "Optimize",
            "build",
            "General",
            "Optimize code",
            "bool",
            true,
            "false",
        );
        optimize["trueValue"] = json!("true");
        optimize["falseValue"] = json!("false");
        let mut warnings = prop(
            "TreatWarningsAsErrors",
            "build",
            "Errors and warnings",
            "Treat warnings as errors",
            "bool",
            true,
            "false",
        );
        warnings["trueValue"] = json!("true");
        warnings["falseValue"] = json!("false");
        let output_path = prop(
            "OutputPath",
            "build",
            "Output",
            "Output path",
            "path",
            true,
            "bin/Debug/",
        );
        let mut pre = prop(
            "PreBuildEvent",
            "build",
            "Events",
            "Pre-build event",
            "multiline",
            false,
            "",
        );
        pre["target"] = json!("PreBuild");
        let version = prop(
            "Version", "package", "General", "Version", "string", false, "1.0.0",
        );
        let mut authors = prop(
            "Authors",
            "package",
            "General",
            "Authors",
            "string",
            false,
            "Corpus Authors",
        );
        authors["source"] = json!("inherited");
        authors["inherited"] = json!(true);
        authors["inheritedFrom"] = json!(props);
        let mut style = prop(
            "EnforceCodeStyleInBuild",
            "codeAnalysis",
            "General",
            "Enforce code style on build",
            "bool",
            false,
            "false",
        );
        style["trueValue"] = json!("true");
        style["falseValue"] = json!("false");
        let frameworks: Vec<Value> = frameworks.iter().map(|f| json!(f)).collect();
        let mut tf = prop(
            "TargetFramework",
            "application",
            "General",
            "Target framework",
            "string",
            false,
            frameworks
                .first()
                .and_then(Value::as_str)
                .unwrap_or("net10.0"),
        );
        tf["source"] = json!("project");
        json!({
            "generation": 0, "project": project, "kind": "sdk", "configuration": "Debug", "platform": "AnyCPU",
            "configurations": ["Debug", "Release"], "platforms": ["AnyCPU"], "frameworks": frameworks,
            "pages": [
                {"id": "application", "title": "Application", "state": "ready"},
                {"id": "build", "title": "Build", "state": "ready"},
                {"id": "package", "title": "Package", "state": "ready"},
                {"id": "debug", "title": "Debug", "state": "launchProfiles"},
                {"id": "codeAnalysis", "title": "Code Analysis", "state": "ready"},
                {"id": "resources", "title": "Resources", "state": "notYet", "note": "Not yet."},
                {"id": "settings", "title": "Settings", "state": "notYet", "note": "Not yet."},
                {"id": "signing", "title": "Signing", "state": "notYet", "note": "Not yet."}
            ],
            "properties": [assembly, tf, output, nullable, implicit, lang, icon, define, optimize, warnings,
                           output_path, pre, version, authors, style]
        })
    }
}
