//! The fake host's `eludite/nuget/*` service (brief 0048; feature `fake`): a scripted set of package sources and their
//! packages, the solution's projects and their packages, and the behaviors the shell's tests need: per-source failures,
//! a private source that asks the shell for credentials (`eludite/nuget/credentials`) during an interactive call and
//! refuses a non-interactive one, vulnerability and deprecation data, changes that edit the scripted projects, advance
//! the generation and restore (or fail to), and the `eludite/nuget/update` notifications NuGet's output produces.
//!
//! Script it with [`crate::fake::FakeHost::set_nuget`]: `{ "sources": [{ "name", "url", "enabled"?, "failing"?,
//! "private"? }], "packages": [{ "source", "id", "versions": [oldest..newest], "description"?, "iconUrl"?,
//! "vulnerable"?: { "version", "severity", "advisoryUrl" }, "deprecated"?: { "version", "message" } }], "projects":
//! [{ "path", "name", "packages": [{ "id", "version" }] }], "credentials"?: ["user", "password"] }`.

use std::time::Duration;

use serde_json::{Value, json};

/// The scripted NuGet state.
#[derive(Debug, Default)]
pub(crate) struct FakeNuGet {
    pub sources: Vec<Value>,
    pub packages: Vec<Value>,
    pub projects: Vec<Value>,
    pub credentials: Option<(String, String)>,
    pub authed: bool,
    /// The next restores fail with this diagnostic.
    pub restore_failure: Option<Value>,
    pub search_delay: Duration,
    /// Calls answered (every `eludite/nuget/*` request).
    pub calls: u64,
    /// `eludite/nuget/icon`'s answers by address (a `file:` address answers its own path).
    pub icons: std::collections::HashMap<String, String>,
}

/// A version's sort key: numeric parts, then release before prerelease (a missing prerelease sorts last).
pub(crate) fn version_key(v: &str) -> (Vec<u64>, bool, String) {
    let (core, pre) = v.split_once('-').unwrap_or((v, ""));
    let parts = core
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    (parts, pre.is_empty(), pre.to_owned())
}

pub(crate) fn is_prerelease(v: &str) -> bool {
    v.contains('-')
}

impl FakeNuGet {
    pub fn set(&mut self, script: &Value) {
        let list = |k: &str| script[k].as_array().cloned().unwrap_or_default();
        self.sources = list("sources");
        for s in &mut self.sources {
            if s.get("enabled").is_none() {
                s["enabled"] = json!(true);
            }
        }
        self.packages = list("packages");
        self.projects = list("projects");
        self.credentials = script["credentials"].as_array().map(|c| {
            (
                c[0].as_str().unwrap_or_default().to_owned(),
                c[1].as_str().unwrap_or_default().to_owned(),
            )
        });
        self.authed = false;
    }

    fn source(&self, name: &str) -> Option<&Value> {
        self.sources.iter().find(|s| s["name"] == name)
    }

    /// The enabled sources, or the one named.
    pub fn chosen_sources(&self, named: Option<&str>) -> Vec<Value> {
        self.sources
            .iter()
            .filter(|s| s["enabled"] == true && named.is_none_or(|n| s["name"] == n))
            .cloned()
            .collect()
    }

    /// Every version any of `sources` lists for `id` (prerelease per `pre`), newest first, with the source of each.
    pub fn versions(&self, id: &str, pre: bool, sources: &[Value]) -> Vec<(String, String)> {
        let mut all: Vec<(String, String)> = self
            .packages
            .iter()
            .filter(|p| {
                p["id"]
                    .as_str()
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(id)
                    && sources.iter().any(|s| s["name"] == p["source"])
            })
            .flat_map(|p| {
                let source = p["source"].as_str().unwrap_or_default().to_owned();
                p["versions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(move |v| v.as_str().map(|v| (v.to_owned(), source.clone())))
            })
            .filter(|(v, _)| pre || !is_prerelease(v))
            .collect();
        all.sort_by_key(|(v, _)| std::cmp::Reverse(version_key(v)));
        all.dedup_by(|a, b| a.0 == b.0);
        all
    }

    fn package(&self, id: &str) -> Option<&Value> {
        self.packages.iter().find(|p| {
            p["id"]
                .as_str()
                .unwrap_or_default()
                .eq_ignore_ascii_case(id)
        })
    }

    /// The vulnerabilities and deprecation the sources know for `id` at `version`.
    pub fn metadata(&self, id: &str, version: &str) -> (Option<Value>, Option<Value>) {
        let Some(p) = self.package(id) else {
            return (None, None);
        };
        let vuln = p
            .get("vulnerable")
            .filter(|v| v["version"] == version)
            .map(|v| json!([{"severity": v["severity"], "advisoryUrl": v["advisoryUrl"]}]));
        let dep = p
            .get("deprecated")
            .filter(|d| d["version"] == version)
            .map(|d| json!({"reasons": ["legacy"], "message": d["message"]}));
        (vuln, dep)
    }

    /// `eludite/nuget/search`'s results and source rows. `asked` reports a private source that needs credentials.
    pub fn search(&self, params: &Value) -> (Vec<Value>, Vec<Value>, Option<String>) {
        let query = params["query"].as_str().unwrap_or_default().to_lowercase();
        let pre = params["prerelease"] == true;
        let sources = self.chosen_sources(params["source"].as_str());
        let mut results: Vec<Value> = Vec::new();
        let mut rows = Vec::new();
        let mut needs = None;
        for s in &sources {
            let name = s["name"].as_str().unwrap_or_default();
            if s["failing"] == true {
                rows.push(json!({"name": name, "url": s["url"], "count": 0, "elapsedMs": 1.0,
                                 "error": format!("Unable to load the service index for source {}.", s["url"].as_str().unwrap_or_default())}));
                continue;
            }
            if s["private"] == true && !self.authed {
                let host = host_of(s["url"].as_str().unwrap_or_default());
                rows.push(json!({"name": name, "url": s["url"], "count": 0, "elapsedMs": 1.0,
                                 "error": format!("credentials_required: {host} asked for credentials and none were given")}));
                needs = Some(host);
                continue;
            }
            let mut count = 0;
            for p in self.packages.iter().filter(|p| p["source"] == name) {
                let id = p["id"].as_str().unwrap_or_default();
                if !id.to_lowercase().contains(&query) {
                    continue;
                }
                let versions = self.versions(id, pre, std::slice::from_ref(s));
                let Some((latest, _)) = versions.first() else {
                    continue;
                };
                count += 1;
                if results.iter().any(|r| r["id"] == id) {
                    continue;
                }
                let mut r = json!({"id": id, "version": latest, "source": name,
                                   "versions": versions.iter().map(|(v, _)| v.clone()).collect::<Vec<_>>(),
                                   "description": p["description"].as_str().unwrap_or("A fake package."),
                                   "authors": "Fake", "downloads": 1000});
                if let Some(icon) = p.get("iconUrl") {
                    r["iconUrl"] = icon.clone();
                }
                let (vuln, dep) = self.metadata(id, latest);
                if let Some(v) = vuln {
                    r["vulnerabilities"] = v;
                }
                if let Some(d) = dep {
                    r["deprecation"] = d;
                }
                results.push(r);
            }
            rows.push(json!({"name": name, "url": s["url"], "count": count, "elapsedMs": 2.0}));
        }
        let take = params["take"].as_u64().unwrap_or(50) as usize;
        results.truncate(take);
        (results, rows, needs)
    }

    /// The projects' packages (`eludite/nuget/installed`).
    pub fn installed(&self, params: &Value) -> Vec<Value> {
        let named: Option<Vec<String>> = params["projects"].as_array().map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect()
        });
        let metadata = params["metadata"] == true;
        self.projects
            .iter()
            .filter(|p| {
                named
                    .as_ref()
                    .is_none_or(|n| n.iter().any(|x| p["path"] == x.as_str()))
            })
            .map(|p| {
                let packages: Vec<Value> = p["packages"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|pk| {
                        let id = pk["id"].as_str().unwrap_or_default();
                        let version = pk["version"].as_str().unwrap_or_default();
                        let source = self
                            .package(id)
                            .and_then(|x| self.source(x["source"].as_str().unwrap_or_default()))
                            .map(|s| s["url"].clone())
                            .unwrap_or(Value::Null);
                        let mut out = json!({"id": id, "version": version, "requested": version,
                                             "targetFrameworks": ["net10.0"], "transitive": false});
                        if !source.is_null() {
                            out["source"] = source;
                        }
                        let (vuln, dep) = self.metadata(id, version);
                        // Vulnerabilities come with the assets file (NuGet Audit); deprecation only from the sources.
                        if let Some(v) = vuln {
                            out["vulnerabilities"] = v;
                        }
                        if metadata && let Some(d) = dep {
                            out["deprecation"] = d;
                        }
                        out
                    })
                    .collect();
                json!({"path": p["path"], "name": p["name"], "format": if packages.is_empty() { "none" } else { "packageReference" },
                       "restored": true, "centralPackageManagement": false, "targetFrameworks": ["net10.0"],
                       "packages": packages})
            })
            .collect()
    }

    /// The `metadata` update's packages for `projects` (what has vulnerabilities or deprecation).
    pub fn metadata_packages(projects: &[Value]) -> Vec<Value> {
        let mut out: Vec<Value> = Vec::new();
        for p in projects {
            for pk in p["packages"].as_array().into_iter().flatten() {
                if (pk.get("vulnerabilities").is_some() || pk.get("deprecation").is_some())
                    && !out
                        .iter()
                        .any(|o| o["id"] == pk["id"] && o["version"] == pk["version"])
                {
                    let mut m = json!({"id": pk["id"], "version": pk["version"]});
                    if let Some(v) = pk.get("vulnerabilities") {
                        m["vulnerabilities"] = v.clone();
                    }
                    if let Some(d) = pk.get("deprecation") {
                        m["deprecation"] = d.clone();
                    }
                    out.push(m);
                }
            }
        }
        out
    }

    /// `eludite/nuget/updates`'s rows.
    pub fn updates(&self, params: &Value) -> (Vec<Value>, Vec<Value>) {
        let pre = params["prerelease"] == true;
        let sources = self.chosen_sources(params["source"].as_str());
        let mut updates = Vec::new();
        for p in self.installed(params) {
            for pk in p["packages"].as_array().into_iter().flatten() {
                let id = pk["id"].as_str().unwrap_or_default();
                let installed = pk["version"].as_str().unwrap_or_default();
                let versions = self.versions(id, pre, &sources);
                if let Some((latest, source)) = versions.first()
                    && version_key(latest) > version_key(installed)
                {
                    let mut row = json!({"project": p["path"], "id": id, "installed": installed, "latest": latest,
                                         "source": source, "requested": installed,
                                         "versions": versions.iter().map(|(v, _)| v.clone()).collect::<Vec<_>>()});
                    if let Some(v) = pk.get("vulnerabilities") {
                        row["vulnerabilities"] = v.clone();
                    }
                    updates.push(row);
                }
            }
        }
        let rows = sources
            .iter()
            .map(|s| json!({"name": s["name"], "url": s["url"], "count": updates.len()}))
            .collect();
        (updates, rows)
    }

    /// Applies `eludite/nuget/change` to the scripted projects: the packages as written, the projects changed, the
    /// output lines; or an error (code, message, data).
    #[allow(clippy::type_complexity)]
    pub fn change(
        &mut self,
        params: &Value,
    ) -> Result<(Vec<Value>, Vec<String>, Vec<String>), (i64, String, Value)> {
        let action = params["action"].as_str().unwrap_or_default().to_owned();
        let pre = params["prerelease"] == true;
        let sources = self.chosen_sources(params["source"].as_str());
        let named: Option<Vec<String>> = params["projects"].as_array().map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect()
        });
        if action == "install" && named.is_none() && self.projects.len() != 1 {
            return Err((
                -32602,
                "install names the projects (params.projects) when the solution has more than one"
                    .into(),
                Value::Null,
            ));
        }
        let mut written = Vec::new();
        let mut changed: Vec<String> = Vec::new();
        let mut lines = Vec::new();
        for pkg in params["packages"].as_array().cloned().unwrap_or_default() {
            let id = pkg["id"].as_str().unwrap_or_default().to_owned();
            let references = |p: &Value| {
                p["packages"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x["id"] == id.as_str()))
            };
            let targets: Vec<String> = match &named {
                Some(n) => n.clone(),
                None if action == "install" => vec![
                    self.projects[0]["path"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                ],
                None => self
                    .projects
                    .iter()
                    .filter(|p| references(p))
                    .filter_map(|p| p["path"].as_str().map(str::to_owned))
                    .collect(),
            };
            let version = match action.as_str() {
                "install" | "update" => {
                    let all = self.versions(&id, true, &sources);
                    let pick = match pkg["version"].as_str() {
                        Some(v) => all.iter().find(|(x, _)| x == v).map(|(x, _)| x.clone()),
                        None => all
                            .iter()
                            .find(|(x, _)| pre || !is_prerelease(x))
                            .map(|(x, _)| x.clone()),
                    };
                    match pick {
                        Some(v) => Some(v),
                        None => {
                            return Err((
                                -32014,
                                format!("Unable to find package {id} on the package sources"),
                                json!({"reason": "notFound", "package": id}),
                            ));
                        }
                    }
                }
                "consolidate" => pkg["version"].as_str().map(str::to_owned).or_else(|| {
                    self.projects
                        .iter()
                        .flat_map(|p| p["packages"].as_array().cloned().unwrap_or_default())
                        .filter(|x| x["id"] == id.as_str())
                        .filter_map(|x| x["version"].as_str().map(str::to_owned))
                        .max_by_key(|v| version_key(v))
                }),
                _ => None,
            };
            for project in self.projects.iter_mut() {
                let path = project["path"].as_str().unwrap_or_default().to_owned();
                if !targets.contains(&path) {
                    continue;
                }
                let name = project["name"].as_str().unwrap_or_default().to_owned();
                let list = project["packages"]
                    .as_array_mut()
                    .expect("a project lists its packages");
                let before = list.clone();
                match action.as_str() {
                    "uninstall" => {
                        list.retain(|x| x["id"] != id.as_str());
                        lines.push(format!("Uninstalling NuGet package {id} from {name}."));
                    }
                    _ => {
                        let v = version.clone().unwrap_or_default();
                        match list.iter_mut().find(|x| x["id"] == id.as_str()) {
                            Some(x) => x["version"] = json!(v),
                            None => list.push(json!({"id": id, "version": v})),
                        }
                        lines.push(format!("Installing NuGet package {id} {v} in {name}."));
                    }
                }
                if *list != before && !changed.contains(&path) {
                    changed.push(path);
                }
            }
            let mut w = json!({"id": id});
            if let Some(v) = version {
                w["version"] = json!(v);
            }
            written.push(w);
        }
        Ok((written, changed, lines))
    }

    /// A restore's outcome and its output lines.
    pub fn restore(&self) -> (Value, Vec<String>) {
        match &self.restore_failure {
            Some(d) => (
                json!({"result": "failed", "exitCode": 1, "elapsedMs": 40.0, "commandLine": "dotnet restore",
                       "lockedMode": false, "lockFiles": [], "diagnostics": [d]}),
                vec![
                    "dotnet restore".into(),
                    format!(
                        "{} : error {}: {}",
                        d["file"].as_str().unwrap_or_default(),
                        d["code"].as_str().unwrap_or_default(),
                        d["message"].as_str().unwrap_or_default()
                    ),
                    "Restore failed with 1 error.".into(),
                ],
            ),
            None => (
                json!({"result": "succeeded", "exitCode": 0, "elapsedMs": 30.0, "commandLine": "dotnet restore",
                       "lockedMode": false, "lockFiles": [], "diagnostics": []}),
                vec![
                    "dotnet restore".into(),
                    "  Restored the projects.".into(),
                    "Restore succeeded in 0.0 s.".into(),
                ],
            ),
        }
    }

    /// `eludite/nuget/sources`.
    pub fn sources_call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let action = params["action"].as_str().unwrap_or("list");
        let name = params["name"].as_str().unwrap_or_default().to_owned();
        let mut changed = true;
        match action {
            "list" => changed = false,
            "add" => self.sources.push(
                json!({"name": name, "url": params["url"], "enabled": true, "scope": "user"}),
            ),
            "remove" => {
                let before = self.sources.len();
                self.sources.retain(|s| s["name"] != name.as_str());
                if before == self.sources.len() {
                    return Err((-32602, format!("no package source is named {name}")));
                }
            }
            "enable" | "disable" => {
                match self.sources.iter_mut().find(|s| s["name"] == name.as_str()) {
                    Some(s) => s["enabled"] = json!(action == "enable"),
                    None => return Err((-32602, format!("no package source is named {name}"))),
                }
            }
            other => return Err((-32602, format!("unknown action {other}"))),
        }
        Ok(json!({
            "sources": self.sources.iter().map(|s| json!({
                "name": s["name"], "url": s["url"], "enabled": s["enabled"] == true,
                "local": !s["url"].as_str().unwrap_or_default().starts_with("http"),
                "scope": s.get("scope").and_then(Value::as_str).unwrap_or("solution"),
                "configFile": "/fake/NuGet.config"
            })).collect::<Vec<_>>(),
            "configFiles": ["/fake/NuGet.config"],
            "userConfig": "/fake/user/NuGet.Config",
            "changed": changed
        }))
    }

    /// Puts the scripted projects' packages into the tree's Dependencies nodes.
    pub fn decorate_tree(&self, tree: &mut Value) {
        if self.projects.is_empty() {
            return;
        }
        for project in tree.as_array_mut().into_iter().flatten() {
            let Some(p) = self.projects.iter().find(|p| p["path"] == project["path"]) else {
                continue;
            };
            let packages: Vec<Value> = p["packages"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|pk| {
                    let id = pk["id"].as_str().unwrap_or_default();
                    let version = pk["version"].as_str().unwrap_or_default();
                    let mut out = json!({"id": id, "version": version, "requested": version});
                    if let Some(deps) = self
                        .package(id)
                        .and_then(|x| x.get("dependencies"))
                        .and_then(Value::as_array)
                    {
                        out["transitive"] = json!(deps);
                    }
                    if let (Some(v), _) = self.metadata(id, version) {
                        out["vulnerabilities"] = v;
                    }
                    out
                })
                .collect();
            let deps = project
                .as_object_mut()
                .expect("a tree project is an object")
                .entry("dependencies")
                .or_insert_with(|| {
                    json!({"restored": true, "packages": [], "projects": [],
                           "frameworks": [{"name": "Microsoft.NETCore.App", "targetFramework": "net10.0"}]})
                });
            deps["packages"] = json!(packages);
        }
    }
}

/// The host (and port) of a url, as the credential prompt names it.
pub(crate) fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest).to_owned()
}

/// The `eludite/nuget/update` notifications for `lines`, from `seq` on.
pub(crate) fn output_updates(
    operation: u64,
    generation: u64,
    seq: &mut u64,
    lines: &[String],
) -> Vec<Value> {
    lines
        .iter()
        .map(|l| {
            let u = json!({"operation": operation, "generation": generation, "seq": *seq, "kind": "output",
                           "text": format!("{l}\n")});
            *seq += 1;
            u
        })
        .collect()
}
