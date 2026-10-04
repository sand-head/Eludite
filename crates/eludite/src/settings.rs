//! The settings store (brief 0020, PLAN.md 4.12): two JSON files merged, user then solution, over the defaults of
//! the settings schema (`protocol/schemas/settings.json`, read by [`eludite_commands::settings::SettingsSchema`]),
//! with the documented environment variables overriding both while they are set, and a third file, the person's own
//! state for the open workspace, for the settings whose `x-eludite-scope` is `user-workspace` (brief 0047).
//!
//! | File | Where |
//! |---|---|
//! | User | `<config dir>/eludite/settings.json`: `$XDG_CONFIG_HOME/eludite/` (default `~/.config/eludite/`) on Linux, `%APPDATA%\eludite\` on Windows, `~/Library/Application Support/eludite/` on macOS; `ELUDITE_CONFIG_DIR` replaces the `eludite` folder |
//! | Solution | `.eludite/settings.json` in the open solution's folder (or the open folder) |
//! | User workspace | `<config dir>/eludite/workspaces/<folder name>-<16 hex digits>/settings.json` ([`workspace_state_dir`]): the person's state for the open workspace, beside the layouts, the search history and the git drafts that are also kept per workspace in Eludite's config directory; the hex digits are the layouts' FNV-1a hash of the folder's absolute path. Written with mode 0600 (its folder 0700) on Unix |
//!
//! - **Precedence** ([`eludite_commands::settings::SettingSpec::resolve`]). Environment variable, then the solution
//!   file, then the user file, then the schema's default; for a `user-workspace` setting (`browser.allowNoSandbox`),
//!   the person's workspace state takes the solution file's place and the solution file's value is ignored and
//!   reported (`ignored_keys`), so a committed file can never set it.
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
    SettingLayers, SettingRow, SettingScope, SettingSource, SettingsFileInfo, SettingsGetOutput,
    SettingsSchema, SettingsSetOutput,
};
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Map, Value};

/// The user settings file's name in Eludite's config directory.
pub const USER_FILE: &str = "settings.json";
/// The solution settings file, relative to the solution's (or folder's) directory.
pub const SOLUTION_FILE: &str = ".eludite/settings.json";
/// The folder of the per-workspace state in Eludite's config directory (brief 0047).
pub const WORKSPACES_DIR: &str = "workspaces";

/// The person's state folder for the workspace `root` under `state_root` (`<config dir>/eludite/workspaces`):
/// `<folder name>-<16 hex digits>`, the hex digits being the FNV-1a 64-bit hash of the absolute path that names the
/// workspace's layout file (`eludite_docking::persist::LayoutStore`), so two folders of one name do not collide.
pub fn workspace_state_dir(state_root: &Path, root: &Path) -> PathBuf {
    let abs = std::path::absolute(root).unwrap_or_else(|_| root.to_owned());
    // The layouts' file name ends with the same hash: `<stem>-<16 hex digits>.json`.
    let layout = eludite_docking::persist::LayoutStore::new(PathBuf::new()).solution_path(&abs);
    let stem = layout
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hash = stem.rsplit('-').next().unwrap_or_default();
    let name: String = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".into())
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_matches('.');
    let name = if name.is_empty() { "workspace" } else { name };
    state_root.join(format!("{name}-{hash}"))
}

/// Where the store reads from.
#[derive(Debug, Clone)]
pub struct SettingsSetup {
    /// The user file (`None`: no user layer, as when there is no config directory).
    pub user_path: Option<PathBuf>,
    /// Where the per-workspace state folders are (`<config dir>/eludite/workspaces`; `None`: no user-workspace
    /// layer, so the per-person settings keep their defaults or the user file's values).
    pub state_dir: Option<PathBuf>,
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
        let config = eludite_docking::eludite_config_dir();
        Self {
            user_path: config.as_ref().map(|d| d.join(USER_FILE)),
            state_dir: config.map(|d| d.join(WORKSPACES_DIR)),
            env,
            poll: Duration::from_millis(100),
        }
    }

    /// No environment, and the user file (with the workspace state beside it, in `workspaces/`) only where given:
    /// otherwise only defaults and the solution file (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn isolated(user_path: Option<PathBuf>) -> Self {
        Self {
            state_dir: user_path
                .as_ref()
                .and_then(|p| p.parent())
                .map(|d| d.join(WORKSPACES_DIR)),
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
    /// The person's own file: written with mode 0600 on Unix (brief 0047).
    private: bool,
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
    /// The person's state for the open workspace (brief 0047), and where such states are.
    user_workspace: Option<Layer>,
    state_root: Option<PathBuf>,
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
            user_workspace: None,
            state_root: setup.state_dir.clone(),
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

    /// The directory of the open solution or folder, whose `.eludite/settings.json` is the second layer and whose
    /// state folder holds the person's own settings for it.
    pub fn set_solution_dir(&mut self, dir: Option<&Path>) {
        let path = dir.map(|d| d.join(SOLUTION_FILE));
        if self.solution.as_ref().map(|l| &l.path) == path.as_ref() {
            return;
        }
        self.solution = path.map(Layer::unread);
        self.user_workspace = dir.zip(self.state_root.as_deref()).map(|(d, root)| Layer {
            private: true,
            ..Layer::unread(workspace_state_dir(root, d).join(USER_FILE))
        });
        self.version += 1;
    }

    fn layers(&self) -> impl Iterator<Item = &Layer> {
        [
            self.user.as_ref(),
            self.solution.as_ref(),
            self.user_workspace.as_ref(),
        ]
        .into_iter()
        .flatten()
    }

    fn layers_mut(&mut self) -> impl Iterator<Item = &mut Layer> {
        [
            self.user.as_mut(),
            self.solution.as_mut(),
            self.user_workspace.as_mut(),
        ]
        .into_iter()
        .flatten()
    }

    /// The per-person keys the solution's file sets, ignored there (brief 0047).
    pub fn ignored_keys(&self) -> Vec<String> {
        self.schema
            .ignored_in_solution(self.solution.iter().flat_map(|l| l.values.keys()))
    }

    /// The effective value of `key` and where it came from. Panics on a key the schema does not have.
    pub fn effective(&self, key: &str) -> (Value, SettingSource) {
        let spec = self
            .schema
            .get(key)
            .unwrap_or_else(|| panic!("{key} is not a setting"));
        fn value<'a>(layer: Option<&'a Layer>, key: &str) -> Option<&'a Value> {
            layer.and_then(|l| l.values.get(key))
        }
        spec.resolve(SettingLayers {
            env: spec
                .env
                .as_ref()
                .and_then(|e| self.env.get(e))
                .map(String::as_str),
            user: value(self.user.as_ref(), key),
            user_workspace: value(self.user_workspace.as_ref(), key),
            solution: value(self.solution.as_ref(), key),
        })
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

    /// Whether the user or solution file sets `key` (the agents registry falls back to `agents.json` when neither
    /// does).
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
        let mut unknown_keys: Vec<String> = self
            .layers()
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
            user_workspace_file: self.user_workspace.as_ref().map(Layer::info),
            settings,
            ignored_keys: self.ignored_keys(),
            unknown_keys,
        }
    }

    /// `eludite.settings.set` on the in-memory layer: returns the output and the file to write with its text.
    pub fn set(
        &mut self,
        key: &str,
        value: Value,
        scope: SettingScope,
    ) -> Result<(SettingsSetOutput, FileWrite), CommandError> {
        let layer = match scope {
            SettingScope::User => self.user.as_mut().ok_or_else(|| {
                CommandError::Failed(
                    "there is no user settings file: Eludite's config directory is unknown".into(),
                )
            })?,
            SettingScope::Solution => self.solution.as_mut().ok_or_else(|| {
                CommandError::Failed(
                    "no workspace is open: there is no workspace settings file".into(),
                )
            })?,
            SettingScope::UserWorkspace => {
                let why = if self.solution.is_none() {
                    "no workspace is open: the person's settings for a workspace need one"
                } else {
                    "Eludite's config directory is unknown: there is no state for this workspace"
                };
                self.user_workspace
                    .as_mut()
                    .ok_or_else(|| CommandError::Failed(why.into()))?
            }
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
        let write = FileWrite {
            path: layer.path.clone(),
            text,
            private: layer.private,
        };
        self.version += 1;
        let (value, source) = self.effective(key);
        Ok((
            SettingsSetOutput {
                key: key.to_owned(),
                scope,
                path: write.path.to_string_lossy().into_owned(),
                value,
                source,
            },
            write,
        ))
    }

    /// Re-read a file that changed on disk. Returns whether anything was re-read.
    pub fn reload_if_changed(&mut self) -> bool {
        let mut changed = false;
        for layer in self.layers_mut() {
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
        for layer in self.layers_mut() {
            if layer.path == path {
                layer.stamp = Layer::stamp_of(path);
            }
        }
    }
}

/// A file write for the settings thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub path: PathBuf,
    pub text: String,
    /// The person's own file (brief 0047): mode 0600, its folder 0700, on Unix.
    pub private: bool,
}

impl FileWrite {
    /// Write the file through a temporary file and a rename, so a reader never sees half of it; a private file's
    /// temporary file is created with mode 0600, so the file never has another mode.
    pub fn write(&self) -> std::io::Result<()> {
        if !self.private {
            return eludite_docking::persist::write_atomic(&self.path, &self.text);
        }
        if let Some(dir) = self.path.parent() {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        {
            use std::io::Write as _;
            let mut file = options.open(&tmp)?;
            // A temporary file left by an earlier run keeps its mode: set it again.
            #[cfg(unix)]
            std::fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            file.write_all(self.text.as_bytes())?;
        }
        std::fs::rename(&tmp, &self.path)
    }
}

/// The shared store, its writer and poller thread, and the channel that tells the shell to apply changes.
#[derive(Clone)]
pub struct Settings {
    store: Arc<Mutex<SettingsStore>>,
    writes: mpsc::Sender<FileWrite>,
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
        let (writes, rx) = mpsc::channel::<FileWrite>();
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
                        Ok(w) => match w.write() {
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
        let (out, write) = self.lock().set(key, value, scope)?;
        let _ = self.writes.send(write);
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
        let (out, FileWrite { path, text, .. }) = s
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
        let (_, write) = s
            .set("agents.default", Value::Null, SettingScope::User)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&write.text).unwrap(),
            json!({"build.onSave": true})
        );
        assert!(
            s.set("build.onSave", json!(true), SettingScope::Solution)
                .is_err(),
            "no workspace open"
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

    /// Brief 0047: the person's state for a workspace is a folder named after it in `workspaces/` beside the user
    /// file; `browser.allowNoSandbox` is read from there (then the user file), never from the workspace's file,
    /// whose value is reported ignored; a set writes the state file, mode 0600 on Unix, and nothing else.
    #[test]
    fn the_persons_workspace_state_holds_the_opt_in_and_the_workspace_file_cannot() {
        let dir = tempfile::tempdir().unwrap();
        let sln = dir.path().join("src/My App");
        std::fs::create_dir_all(sln.join(".eludite")).unwrap();
        std::fs::write(
            sln.join(SOLUTION_FILE),
            r#"{"browser.allowNoSandbox": true, "build.onSave": true}"#,
        )
        .unwrap();
        let mut s = store(dir.path(), &[]);
        s.set_solution_dir(Some(&sln));
        s.reload_if_changed();
        assert_eq!(
            s.effective("browser.allowNoSandbox"),
            (json!(false), SettingSource::Default),
            "a committed file never opts the machine out"
        );
        assert_eq!(
            s.effective("build.onSave"),
            (json!(true), SettingSource::Solution)
        );
        let out = s.get_output(None);
        assert_eq!(out.ignored_keys, ["browser.allowNoSandbox"]);
        assert!(out.unknown_keys.is_empty());
        let state = out.user_workspace_file.clone().unwrap();
        let expected_dir = workspace_state_dir(&dir.path().join("user/workspaces"), &sln);
        assert_eq!(PathBuf::from(&state.path), expected_dir.join(USER_FILE));
        assert!(!state.exists);
        let name = expected_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("My_App-") && name.len() == "My_App-".len() + 16,
            "{name}"
        );
        assert_ne!(
            workspace_state_dir(Path::new("/s"), Path::new("/a/App")),
            workspace_state_dir(Path::new("/s"), Path::new("/b/App")),
            "two folders of one name"
        );

        // The set: the state file, private, with the opt-in only; the workspace's file untouched.
        let (out, write) = s
            .set(
                "browser.allowNoSandbox",
                json!(true),
                SettingScope::UserWorkspace,
            )
            .unwrap();
        assert_eq!(out.source, SettingSource::UserWorkspace);
        assert_eq!(write.path, expected_dir.join(USER_FILE));
        assert!(write.private);
        write.write().unwrap();
        s.wrote(&write.path);
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&write.path).unwrap()).unwrap(),
            json!({"browser.allowNoSandbox": true})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&write.path), 0o600);
            assert_eq!(mode(&expected_dir), 0o700);
        }
        assert_eq!(
            s.effective("browser.allowNoSandbox"),
            (json!(true), SettingSource::UserWorkspace)
        );
        // Read back by a new store (the next session), with the user file under it.
        std::fs::write(
            dir.path().join("user/settings.json"),
            r#"{"browser.allowNoSandbox": false}"#,
        )
        .unwrap();
        let mut next = store(dir.path(), &[]);
        assert_eq!(
            next.effective("browser.allowNoSandbox"),
            (json!(false), SettingSource::User)
        );
        next.set_solution_dir(Some(&sln));
        next.reload_if_changed();
        assert_eq!(
            next.effective("browser.allowNoSandbox"),
            (json!(true), SettingSource::UserWorkspace)
        );
        // Another workspace has its own state.
        next.set_solution_dir(Some(&dir.path().join("other")));
        next.reload_if_changed();
        assert_eq!(
            next.effective("browser.allowNoSandbox"),
            (json!(false), SettingSource::User)
        );
        // With no workspace there is no state to write.
        next.set_solution_dir(None);
        assert!(
            next.set(
                "browser.allowNoSandbox",
                json!(true),
                SettingScope::UserWorkspace
            )
            .is_err()
        );
    }
}
