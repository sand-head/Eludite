//! The settings store (brief 0020, PLAN.md 4.12): two JSON files merged, user then solution, over the defaults of
//! the settings schema (`protocol/schemas/settings.json`, read by [`eludite_commands::settings::SettingsSchema`]),
//! with the documented environment variables overriding both while they are set.
//!
//! | File | Where |
//! |---|---|
//! | User | `<config dir>/eludite/settings.json`: `$XDG_CONFIG_HOME/eludite/` (default `~/.config/eludite/`) on Linux, `%APPDATA%\eludite\` on Windows, `~/Library/Application Support/eludite/` on macOS; `ELUDITE_CONFIG_DIR` replaces the `eludite` folder |
//! | Solution | `.eludite/settings.json` in the open solution's folder (or the open folder) |
//!
//! - **Precedence.** Environment variable, then the solution file, then the user file, then the schema's default.
//!   A file that does not parse is ignored (reported by `eludite.settings.get` and on stderr); unknown keys and
//!   values of the wrong type are ignored and reported.
//! - **Live reload.** A `eludite-settings` thread stats both files every [`SettingsSetup::poll`] (100 ms) and, when
//!   one changed, reads it and signals the shell, which applies the effective values without a restart (the
//!   brief's budget is 200 ms from the change). Polling costs two `stat`s a tick and needs no new dependency. The
//!   files are first read by that thread too (at start, and when a solution opens), never by the UI thread.
//! - **Writes** (`eludite.settings.set`, the Options dialog) change the in-memory layer at once and hand the file
//!   to the same thread to write, so no caller waits on the disk, the UI thread included. A write keeps the file's
//!   other keys.
//! - **Thread-safe.** [`Settings`] is cheap to clone; `get` and `set` run on whichever thread invokes them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use eludite_commands::CommandError;
use eludite_commands::settings::{
    SettingRow, SettingScope, SettingSource, SettingSpec, SettingsFileInfo, SettingsGetOutput,
    SettingsSchema, SettingsSetOutput,
};
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Map, Value};

/// The user settings file's name in Eludite's config directory.
pub const USER_FILE: &str = "settings.json";
/// The solution settings file, relative to the solution's (or folder's) directory.
pub const SOLUTION_FILE: &str = ".eludite/settings.json";

/// Where the store reads from.
#[derive(Debug, Clone)]
pub struct SettingsSetup {
    /// The user file (`None`: no user layer, as when there is no config directory).
    pub user_path: Option<PathBuf>,
    /// The environment variables that override settings, by name (captured once, so tests control them).
    pub env: BTreeMap<String, String>,
    /// How often the files are checked for changes.
    pub poll: Duration,
}

impl SettingsSetup {
    /// The user file in the config directory and the override variables of the real environment.
    pub fn from_env(schema: &SettingsSchema) -> Self {
        let env = schema
            .settings
            .iter()
            .filter_map(|s| s.env.as_ref())
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .filter(|v| !v.is_empty())
                    .map(|v| (name.clone(), v))
            })
            .collect();
        Self {
            user_path: eludite_docking::eludite_config_dir().map(|d| d.join(USER_FILE)),
            env,
            poll: Duration::from_millis(100),
        }
    }

    /// No user file and no environment: only defaults and the solution file (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn isolated(user_path: Option<PathBuf>) -> Self {
        Self {
            user_path,
            env: BTreeMap::new(),
            poll: Duration::from_millis(20),
        }
    }
}

/// One file's keys.
#[derive(Debug, Clone, Default)]
struct Layer {
    path: PathBuf,
    values: Map<String, Value>,
    exists: bool,
    error: Option<String>,
    /// Modification time and length when last read.
    stamp: Option<(SystemTime, u64)>,
    /// Read at least once.
    loaded: bool,
}

impl Layer {
    /// A layer read later, by the settings thread's next poll.
    fn unread(path: PathBuf) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }

    fn stamp_of(path: &Path) -> Option<(SystemTime, u64)> {
        let m = std::fs::metadata(path).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    fn reload(&mut self) {
        self.loaded = true;
        self.stamp = Self::stamp_of(&self.path);
        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                self.exists = true;
                match serde_json::from_str::<Value>(&text) {
                    Ok(Value::Object(map)) => {
                        self.values = map;
                        self.error = None;
                    }
                    Ok(_) => {
                        self.values.clear();
                        self.error = Some("not a JSON object".into());
                    }
                    // A file being written may be cut short: keep the last good values until it parses again.
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.exists = false;
                self.values.clear();
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
        if let Some(e) = &self.error {
            eprintln!("eludite: settings file {}: {e}", self.path.display());
        }
    }

    fn info(&self) -> SettingsFileInfo {
        SettingsFileInfo {
            path: self.path.to_string_lossy().into_owned(),
            exists: self.exists,
            error: self.error.clone(),
        }
    }

    /// The file's text after setting (or removing) `key`.
    fn text_with(&self, key: &str, value: &Value) -> String {
        let mut map = self.values.clone();
        if value.is_null() {
            map.remove(key);
        } else {
            map.insert(key.to_owned(), value.clone());
        }
        // Sorted keys keep the file's diffs stable.
        let sorted: BTreeMap<_, _> = map.into_iter().collect();
        let mut text = serde_json::to_string_pretty(&sorted).expect("settings serialize");
        text.push('\n');
        text
    }
}

/// The merged settings.
pub struct SettingsStore {
    schema: Arc<SettingsSchema>,
    user: Option<Layer>,
    solution: Option<Layer>,
    env: BTreeMap<String, String>,
    /// Incremented whenever an effective value may have changed.
    version: u64,
}

impl SettingsStore {
    pub fn new(schema: Arc<SettingsSchema>, setup: &SettingsSetup) -> Self {
        Self {
            schema,
            user: setup.user_path.clone().map(Layer::unread),
            solution: None,
            env: setup.env.clone(),
            version: 0,
        }
    }

    /// The schema the store follows (the Options dialog is generated from it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn schema(&self) -> &Arc<SettingsSchema> {
        &self.schema
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The directory of the open solution or folder, whose `.eludite/settings.json` is the second layer.
    pub fn set_solution_dir(&mut self, dir: Option<&Path>) {
        let path = dir.map(|d| d.join(SOLUTION_FILE));
        if self.solution.as_ref().map(|l| &l.path) == path.as_ref() {
            return;
        }
        self.solution = path.map(Layer::unread);
        self.version += 1;
    }

    fn layer_value<'a>(&self, layer: Option<&'a Layer>, spec: &SettingSpec) -> Option<&'a Value> {
        let v = layer?.values.get(&spec.key)?;
        spec.validate(v).is_ok().then_some(v)
    }

    /// The effective value of `key` and where it came from. Panics on a key the schema does not have.
    pub fn effective(&self, key: &str) -> (Value, SettingSource) {
        let spec = self
            .schema
            .get(key)
            .unwrap_or_else(|| panic!("{key} is not a setting"));
        if let Some(v) = spec
            .env
            .as_ref()
            .and_then(|e| self.env.get(e))
            .and_then(|t| spec.parse_env(t))
        {
            return (v, SettingSource::Environment);
        }
        if let Some(v) = self.layer_value(self.solution.as_ref(), spec) {
            return (v.clone(), SettingSource::Solution);
        }
        if let Some(v) = self.layer_value(self.user.as_ref(), spec) {
            return (v.clone(), SettingSource::User);
        }
        (spec.default.clone(), SettingSource::Default)
    }

    pub fn bool(&self, key: &str) -> bool {
        self.effective(key).0.as_bool().unwrap_or(false)
    }

    pub fn string(&self, key: &str) -> String {
        self.effective(key)
            .0
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    /// A path setting, `None` when empty.
    pub fn path(&self, key: &str) -> Option<PathBuf> {
        Some(self.string(key))
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }

    /// Whether either file sets `key` (the agents registry falls back to `agents.json` when neither does).
    pub fn is_set_in_a_file(&self, key: &str) -> bool {
        [self.user.as_ref(), self.solution.as_ref()]
            .into_iter()
            .flatten()
            .any(|l| l.values.contains_key(key))
    }

    /// `eludite.settings.get`.
    pub fn get_output(&self, key: Option<&str>) -> SettingsGetOutput {
        let settings = self
            .schema
            .settings
            .iter()
            .filter(|s| key.is_none_or(|k| k == s.key))
            .map(|s| {
                let (value, source) = self.effective(&s.key);
                SettingRow {
                    key: s.key.clone(),
                    value,
                    default: s.default.clone(),
                    source,
                    env: s.env.clone(),
                    section: s.section.clone(),
                    label: s.label.clone(),
                    description: s.description.clone(),
                }
            })
            .collect();
        let mut unknown_keys: Vec<String> = [self.user.as_ref(), self.solution.as_ref()]
            .into_iter()
            .flatten()
            .flat_map(|l| l.values.keys())
            .filter(|k| self.schema.get(k).is_none())
            .cloned()
            .collect();
        unknown_keys.sort();
        unknown_keys.dedup();
        SettingsGetOutput {
            user_file: self
                .user
                .as_ref()
                .map(Layer::info)
                .unwrap_or(SettingsFileInfo {
                    path: "(no config directory)".into(),
                    exists: false,
                    error: Some("Eludite's config directory could not be found".into()),
                }),
            solution_file: self.solution.as_ref().map(Layer::info),
            settings,
            unknown_keys,
        }
    }

    /// `eludite.settings.set` on the in-memory layer: returns the output and the file to write with its text.
    pub fn set(
        &mut self,
        key: &str,
        value: Value,
        scope: SettingScope,
    ) -> Result<(SettingsSetOutput, PathBuf, String), CommandError> {
        let layer = match scope {
            SettingScope::User => self.user.as_mut().ok_or_else(|| {
                CommandError::Failed(
                    "there is no user settings file: Eludite's config directory is unknown".into(),
                )
            })?,
            SettingScope::Solution => self.solution.as_mut().ok_or_else(|| {
                CommandError::Failed(
                    "no solution or folder is open: there is no solution settings file".into(),
                )
            })?,
        };
        // Before the first poll read the file, read it now so the write keeps its other keys.
        if !layer.loaded {
            layer.reload();
        }
        let text = layer.text_with(key, &value);
        if value.is_null() {
            layer.values.remove(key);
        } else {
            layer.values.insert(key.to_owned(), value);
        }
        layer.exists = true;
        layer.error = None;
        let path = layer.path.clone();
        self.version += 1;
        let (value, source) = self.effective(key);
        Ok((
            SettingsSetOutput {
                key: key.to_owned(),
                scope,
                path: path.to_string_lossy().into_owned(),
                value,
                source,
            },
            path,
            text,
        ))
    }

    /// Re-read a file that changed on disk. Returns whether anything was re-read.
    pub fn reload_if_changed(&mut self) -> bool {
        let mut changed = false;
        for layer in [self.user.as_mut(), self.solution.as_mut()]
            .into_iter()
            .flatten()
        {
            if !layer.loaded || Layer::stamp_of(&layer.path) != layer.stamp {
                let before = layer.values.clone();
                layer.reload();
                changed |= layer.values != before;
            }
        }
        if changed {
            self.version += 1;
        }
        changed
    }

    /// After writing `path` ourselves: remember its stamp so the poller does not read it back.
    fn wrote(&mut self, path: &Path) {
        for layer in [self.user.as_mut(), self.solution.as_mut()]
            .into_iter()
            .flatten()
        {
            if layer.path == path {
                layer.stamp = Layer::stamp_of(path);
            }
        }
    }
}

/// A file write for the settings thread.
struct Write {
    path: PathBuf,
    text: String,
}

/// The shared store, its writer and poller thread, and the channel that tells the shell to apply changes.
#[derive(Clone)]
pub struct Settings {
    store: Arc<Mutex<SettingsStore>>,
    writes: mpsc::Sender<Write>,
    changed: UnboundedSender<Instant>,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings").finish_non_exhaustive()
    }
}

impl Settings {
    /// Load the user file and start the `eludite-settings` thread. `changed` receives the time a change was seen,
    /// whenever the effective settings may have changed (a file edited on disk, or a set).
    pub fn start(
        schema: Arc<SettingsSchema>,
        setup: SettingsSetup,
        changed: UnboundedSender<Instant>,
    ) -> Self {
        let store = Arc::new(Mutex::new(SettingsStore::new(schema, &setup)));
        let (writes, rx) = mpsc::channel::<Write>();
        let thread_store = store.clone();
        let thread_changed = changed.clone();
        let poll = setup.poll;
        std::thread::Builder::new()
            .name("eludite-settings".into())
            .spawn(move || {
                // The first pass reads the files at once.
                let mut first = true;
                loop {
                    let next = if std::mem::take(&mut first) {
                        Err(RecvTimeoutError::Timeout)
                    } else {
                        rx.recv_timeout(poll)
                    };
                    match next {
                        Ok(w) => match eludite_docking::persist::write_atomic(&w.path, &w.text) {
                            Ok(()) => lock(&thread_store).wrote(&w.path),
                            Err(e) => eprintln!("eludite: cannot write {}: {e}", w.path.display()),
                        },
                        Err(RecvTimeoutError::Timeout) => {
                            let seen = Instant::now();
                            if lock(&thread_store).reload_if_changed()
                                && thread_changed.unbounded_send(seen).is_err()
                            {
                                return;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("spawn the settings thread");
        Self {
            store,
            writes,
            changed,
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, SettingsStore> {
        lock(&self.store)
    }

    /// `eludite.settings.set`: change the layer, write the file in the background, signal the shell.
    pub fn set(
        &self,
        key: &str,
        value: Value,
        scope: SettingScope,
    ) -> Result<SettingsSetOutput, CommandError> {
        let (out, path, text) = self.lock().set(key, value, scope)?;
        let _ = self.writes.send(Write { path, text });
        let _ = self.changed.unbounded_send(Instant::now());
        Ok(out)
    }

    /// The open solution's or folder's directory changed. Its file is read by the next poll.
    pub fn set_solution_dir(&self, dir: Option<&Path>) {
        let changed = {
            let mut s = self.lock();
            let before = s.version();
            s.set_solution_dir(dir);
            s.version() != before
        };
        if changed {
            let _ = self.changed.unbounded_send(Instant::now());
        }
    }
}

fn lock(m: &Mutex<SettingsStore>) -> MutexGuard<'_, SettingsStore> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store(dir: &Path, env: &[(&str, &str)]) -> SettingsStore {
        let mut setup = SettingsSetup::isolated(Some(dir.join("user/settings.json")));
        setup.env = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let mut s = SettingsStore::new(Arc::new(SettingsSchema::builtin()), &setup);
        s.reload_if_changed();
        s
    }

    #[test]
    fn layers_merge_user_then_solution_then_environment_over_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, text: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write(
            "user/settings.json",
            r#"{"build.onSave": true, "build.beforeRun": false, "agents.default": "Gemini", "nope": 1,
                "build.cargoPath": 7}"#,
        );
        write(
            "sln/.eludite/settings.json",
            r#"{"build.beforeRun": true, "debugger.netcoredbgPath": "/opt/ncdbg"}"#,
        );
        let mut s = store(dir.path(), &[]);
        assert_eq!(
            s.effective("build.onSave"),
            (json!(true), SettingSource::User)
        );
        assert_eq!(
            s.effective("build.showOutputOnStart"),
            (json!(true), SettingSource::Default)
        );
        assert_eq!(
            s.effective("build.beforeRun"),
            (json!(false), SettingSource::User)
        );
        // A value of the wrong type is ignored.
        assert_eq!(
            s.effective("build.cargoPath"),
            (json!(""), SettingSource::Default)
        );
        s.set_solution_dir(Some(&dir.path().join("sln")));
        assert!(s.reload_if_changed(), "the poll reads the solution file");
        assert_eq!(
            s.effective("build.beforeRun"),
            (json!(true), SettingSource::Solution)
        );
        assert_eq!(
            s.path("debugger.netcoredbgPath"),
            Some(PathBuf::from("/opt/ncdbg"))
        );
        let out = s.get_output(None);
        assert_eq!(out.unknown_keys, ["nope"]);
        assert!(out.user_file.exists && out.solution_file.as_ref().unwrap().exists);
        assert_eq!(out.settings.len(), s.schema().settings.len());

        // The environment wins over both files.
        let mut e = store(
            dir.path(),
            &[
                ("ELUDITE_BUILD_ON_SAVE", "0"),
                ("ELUDITE_NETCOREDBG", "/env/ncdbg"),
            ],
        );
        e.set_solution_dir(Some(&dir.path().join("sln")));
        e.reload_if_changed();
        assert_eq!(
            e.effective("build.onSave"),
            (json!(false), SettingSource::Environment)
        );
        assert_eq!(
            e.effective("debugger.netcoredbgPath"),
            (json!("/env/ncdbg"), SettingSource::Environment)
        );
    }

    #[test]
    fn set_keeps_other_keys_null_removes_and_bad_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user/settings.json");
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, r#"{"agents.default": "Gemini"}"#).unwrap();
        let mut s = store(dir.path(), &[]);
        let (out, path, text) = s
            .set("build.onSave", json!(true), SettingScope::User)
            .unwrap();
        assert_eq!(
            (out.value.clone(), out.source),
            (json!(true), SettingSource::User)
        );
        assert_eq!(path, user);
        let written: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            written,
            json!({"agents.default": "Gemini", "build.onSave": true})
        );
        let (_, _, text) = s
            .set("agents.default", Value::Null, SettingScope::User)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"build.onSave": true})
        );
        assert!(
            s.set("build.onSave", json!(true), SettingScope::Solution)
                .is_err(),
            "no solution open"
        );

        std::fs::write(&user, "{ not json").unwrap();
        let mut bad = store(dir.path(), &[]);
        assert!(bad.get_output(None).user_file.error.is_some());
        assert_eq!(bad.effective("build.onSave").1, SettingSource::Default);
        // Fixed on disk: picked up by the poll.
        std::fs::write(&user, r#"{"build.onSave": true}"#).unwrap();
        assert!(bad.reload_if_changed());
        assert_eq!(
            bad.effective("build.onSave"),
            (json!(true), SettingSource::User)
        );
        assert!(!bad.reload_if_changed(), "nothing changed since");
    }
}
