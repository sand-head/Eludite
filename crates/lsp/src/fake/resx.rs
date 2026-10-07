//! The fake host's `eludite/resx/*` service (proposal 0005), for the shell's tests.
//!
//! A test gives the sets the host would list ([`FakeHost::set_resx_sets`]: `eludite/resx/sets`'s `sets` array as
//! the real host answers it) and the fake answers `eludite/resx/sets` with them under the current generation, and
//! `eludite/resx/designer` by writing a stand-in designer file beside the neutral file (`<baseName>.Designer.cs`
//! holding one line per action, so a test can see the call land) or deleting it, following the real host's
//! statuses; `setModifier` updates the listed set's `generator` and `accessModifier` and reloads the solution (the
//! generation moves on). Every request is recorded like any other message.

use eludite_protocol::host::{self, methods};
use serde_json::{Value, json};

use super::FakeHost;

/// The fake's model of the resource sets.
#[derive(Default)]
pub(super) struct FakeResx {
    /// `eludite/resx/sets`'s `sets`, as given.
    sets: Vec<Value>,
    /// Answer `eludite/resx/sets` only after this long.
    delay: std::time::Duration,
}

impl FakeHost {
    /// The sets `eludite/resx/sets` answers (`protocol/schemas/host/resx-sets.json` `$defs.set` objects).
    pub fn set_resx_sets(&self, sets: Value) {
        self.lock().resx.sets = sets.as_array().cloned().unwrap_or_default();
    }

    /// Answer `eludite/resx/sets` only after `delay`.
    pub fn set_resx_sets_delay(&self, delay: std::time::Duration) {
        self.lock().resx.delay = delay;
    }

    /// The listed set whose neutral file is `path`, as the fake holds it now.
    pub fn resx_set(&self, path: &str) -> Option<Value> {
        self.lock()
            .resx
            .sets
            .iter()
            .find(|s| s["path"] == path)
            .cloned()
    }

    pub(super) fn answer_resx(
        &self,
        method: &str,
        params: &Value,
        reply: &dyn Fn(Value),
        error: &dyn Fn(i64, &str, Option<Value>),
        notify: &dyn Fn(&str, Value),
    ) {
        let current = self.generation();
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
        match method {
            methods::RESX_SETS => {
                let (delay, sets) = {
                    let s = self.lock();
                    (s.resx.delay, s.resx.sets.clone())
                };
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                let projects: Option<Vec<String>> = params["projects"].as_array().map(|a| {
                    a.iter()
                        .filter_map(|p| p.as_str().map(str::to_owned))
                        .collect()
                });
                let sets: Vec<Value> = sets
                    .into_iter()
                    .filter(|s| {
                        projects
                            .as_ref()
                            .is_none_or(|ps| ps.iter().any(|p| s["project"] == p.as_str()))
                    })
                    .collect();
                reply(json!({"generation": current, "sets": sets}));
            }
            methods::RESX_DESIGNER => {
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                let action = params["action"].as_str().unwrap_or_default().to_owned();
                let Some(mut set) = self.resx_set(&path) else {
                    return reply(json!({
                        "generation": current, "path": path, "status": "none", "modifier": "none"
                    }));
                };
                let designer = set["designer"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        std::path::Path::new(&path)
                            .with_file_name(format!(
                                "{}.Designer.cs",
                                set["baseName"].as_str().unwrap_or("Resources")
                            ))
                            .to_string_lossy()
                            .into_owned()
                    });
                let mut modifier = set["accessModifier"].as_str().unwrap_or("none").to_owned();
                let mut generation = current;
                let mut project_written = false;
                let status = match action.as_str() {
                    "delete" => {
                        if std::fs::remove_file(&designer).is_ok() {
                            "deleted"
                        } else {
                            "none"
                        }
                    }
                    "setModifier" => {
                        let Some(m) = params["modifier"].as_str() else {
                            return error(-32602, "modifier is required", None);
                        };
                        modifier = m.to_owned();
                        set["accessModifier"] = json!(m);
                        set["generator"] = match m {
                            "internal" => json!("ResXFileCodeGenerator"),
                            "public" => json!("PublicResXFileCodeGenerator"),
                            _ => Value::Null,
                        };
                        if m == "none" {
                            set["designer"] = Value::Null;
                            let _ = std::fs::remove_file(&designer);
                        } else {
                            set["designer"] = json!(designer);
                            let _ = std::fs::write(
                                &designer,
                                format!("// fake designer: {m} {}\r\n", set["namespace"]),
                            );
                        }
                        {
                            let mut s = self.lock();
                            if let Some(slot) = s.resx.sets.iter_mut().find(|s| s["path"] == path) {
                                *slot = set.clone();
                            }
                        }
                        project_written = true;
                        generation = self.reload(notify);
                        if m == "none" { "deleted" } else { "written" }
                    }
                    _ => {
                        if modifier == "none" {
                            let m = params["modifier"].as_str().unwrap_or("internal");
                            if set["generator"].is_null()
                                && !std::path::Path::new(&designer).exists()
                            {
                                modifier = m.to_owned();
                            }
                        }
                        if modifier == "none" {
                            "none"
                        } else {
                            let text =
                                format!("// fake designer: {modifier} {}\r\n", set["namespace"]);
                            if std::fs::read(&designer).ok().as_deref() == Some(text.as_bytes()) {
                                "unchanged"
                            } else {
                                let _ = std::fs::write(&designer, text);
                                "written"
                            }
                        }
                    }
                };
                let mut r = json!({
                    "generation": generation,
                    "path": path,
                    "status": status,
                    "modifier": modifier,
                    "className": set["baseName"],
                    "namespace": set["namespace"],
                });
                if status != "none" {
                    r["designer"] = json!(designer);
                }
                if project_written {
                    r["projectWritten"] = json!(true);
                }
                reply(r);
            }
            other => error(-32601, &format!("{other} not found"), None),
        }
    }
}
