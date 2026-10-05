//! Language-server registration as data (brief 0019): which server handles which files, how to find and start it,
//! and what to send it. The built-in registrations are `servers.json` beside this file; the shell has no
//! per-language code path, so adding a language is adding an entry.
//!
//! Brief 0050 adds what the web servers need, still as data: several registrations may match one file (TypeScript and
//! ESLint for `*.ts`: [`ServerRegistry::all_for_path`]), a `languageId` per file glob, servers that are npm packages
//! run by Node.js and found in the project's `node_modules` before the variable, the web servers' cache
//! (`tools/web-servers/fetch.sh`) and `PATH` ([`CommandSpec::npm_package`]), an activation rule (ESLint only with a
//! configuration file: [`Activation`]), located modules (the project's TypeScript: [`ModuleSpec`]) and `${...}`
//! substitutions in the options ([`substitute`]), settings pushed with `workspace/didChangeConfiguration`, and the
//! formatters Format Document runs ([`FormatterSpec`], [`ServerRegistry::pick_formatter`]).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const BUILTIN: &str = include_str!("servers.json");

/// How a server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    /// Behind `eludite-host` (Roslyn): the host bridge.
    EluditeHost,
    /// A process the shell launches and speaks plain LSP to: the generic client.
    Process,
}

/// How a candidate executable proves it runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Probe {
    /// It answers `--version` (its first line is the version).
    #[default]
    Version,
    /// Its npm package's `package.json` names the version (the `vscode-langservers-extracted` servers have no
    /// `--version`: started without a transport they exit with an error).
    PackageJson,
}

/// How to find a server's (or a formatter's) executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandSpec {
    /// The executable's name without the platform suffix (`rust-analyzer`; `.exe` is added on Windows).
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// An environment variable naming the executable (`ELUDITE_RUST_ANALYZER`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_override: Option<String>,
    /// A rustup component that provides it (`rust-analyzer`), found with `rustup which`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rustup_component: Option<String>,
    /// The npm package whose `bin` entry is the executable (what `node_modules/.bin/<executable>` links to):
    /// searched in the project's `node_modules` (the nearest at or above the root), then the override variable, then
    /// the web servers' cache, then `PATH`; a script is run by Node.js.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_package: Option<String>,
    #[serde(default)]
    pub probe: Probe,
}

/// When a registration that is not always wanted runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Activation {
    /// A setting holding `auto`, `on` or `off` (`languageServers.eslint`).
    pub setting: String,
    /// With `auto`: one of these files at or above the server's root turns it on.
    pub root_files: Vec<String>,
}

/// A Node package a server runs on, located for it and named in its options as `${module:<name>}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleSpec {
    /// `typescript`.
    pub name: String,
    /// What the status bar calls it (`TypeScript`).
    pub label: String,
    /// The npm package (`typescript`).
    pub package: String,
    /// The path inside the package the options name (`lib`).
    pub path: String,
    /// A setting naming it instead (`languageServers.typescriptPath`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<String>,
}

/// One language server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerRegistration {
    /// Stable id (`rust-analyzer`, `roslyn`).
    pub id: String,
    /// The status bar's name for it (`rust-analyzer`, `C#`).
    pub name: String,
    /// The LSP `languageId` of its documents.
    pub language_id: String,
    /// The `languageId` of the documents a glob matches, when not [`ServerRegistration::language_id`]
    /// (`*.tsx`: `typescriptreact`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub language_ids: BTreeMap<String, String>,
    /// File-name patterns it handles (`*.rs`); `*` matches any run of characters.
    pub file_globs: Vec<String>,
    pub via: Via,
    /// Files whose folder is a workspace root for it (`Cargo.toml`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,
    /// LSP `initializationOptions` (with [`substitute`]'s `${...}`).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub initialization_options: Value,
    /// The answers to `workspace/configuration`, by section (with [`substitute`]'s `${...}`).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub settings: Value,
    /// Send the settings with `workspace/didChangeConfiguration` after `initialized` (servers that read them only
    /// from that notification: the HTML, CSS and JSON servers).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub push_settings: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<Activation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<ModuleSpec>,
}

/// When a formatter runs under `editor.formatter`'s `auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoRule {
    /// The project has the package in its `node_modules` (Prettier).
    ProjectPackage,
    /// One of the formatter's configuration files is at or above the file's folder (Biome).
    ConfigFile,
}

/// A formatter Format Document runs on a document's text (stdin to stdout), out of process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormatterSpec {
    /// `prettier`, `biome`: the values of the setting `editor.formatter`.
    pub id: String,
    pub name: String,
    pub file_globs: Vec<String>,
    /// How to find it; its `args` may hold `${file}`, the document's path.
    pub command: CommandSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_files: Vec<String>,
    pub auto: AutoRule,
    /// A JavaScript API the formatter's npm package exports, kept loaded in a Node.js worker after its first use so
    /// later runs are warm (Prettier's `format`, with `resolveConfig` for the project's configuration); without one
    /// (Biome, a native program) every run is a process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSpec>,
}

/// The JavaScript API of a [`FormatterSpec`]'s package: `format(text, {...config, filepath})`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerSpec {
    /// The function that formats (`format`): `(text, options) -> Promise<string>`.
    pub format: String,
    /// The function that finds the options for a file (`resolveConfig`): `(filepath) -> Promise<options | null>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_config: Option<String>,
}

/// The folder `tools/web-servers/fetch.sh` installs into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CacheSpec {
    /// Under `~/.cache/eludite/`: `web-servers`.
    pub folder: String,
    /// `tools/web-servers/PIN`'s `pin`: the folder under it.
    pub pin: String,
    /// A variable naming the installed folder instead (`ELUDITE_WEB_SERVERS`, what `fetch.sh` prints).
    pub env_override: String,
    /// The command that installs it, for the "not found" messages.
    pub fetch: String,
}

/// Where a server's executable was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// The executable, or the script Node.js runs.
    pub path: PathBuf,
    /// The first line of its `--version` output, or its package's version.
    pub version: String,
    /// `beside eludite`, `ELUDITE_RUST_ANALYZER`, `PATH`, `rustup`, `project node_modules`, `web servers cache`.
    pub source: String,
    /// The Node.js that runs [`Located::path`], for a script.
    pub node: Option<PathBuf>,
}

/// A located [`ModuleSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedModule {
    pub path: PathBuf,
    /// Its package's version, when its `package.json` says.
    pub version: Option<String>,
    /// `settings`, `project`, `pinned`.
    pub source: &'static str,
}

/// What [`ServerRegistration::locate_with`] may consult; the real process environment in
/// [`ServerRegistration::locate`].
pub struct Environment<'a> {
    /// The folder of the `eludite` executable.
    pub beside: Option<&'a Path>,
    pub var: &'a dyn Fn(&str) -> Option<OsString>,
    pub path_var: Option<OsString>,
    /// `--version` of a candidate (run by Node.js when the second argument is given): its first line, or `None`
    /// when it does not run.
    pub probe: &'a dyn Fn(&Path, Option<&Path>) -> Option<String>,
    /// `rustup which <executable>`.
    pub rustup: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// Where the `node_modules` search starts: the server's root, a formatter's document folder.
    pub project: Option<&'a Path>,
    /// The web servers' cache folder, when it exists.
    pub cache: Option<&'a Path>,
    /// Node.js, for an npm package's script.
    pub node: &'a dyn Fn() -> Result<PathBuf, String>,
}

/// A file name with a node shebang or a JavaScript suffix: run by Node.js.
fn is_script(path: &Path) -> bool {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "js" | "mjs" | "cjs"))
    {
        return true;
    }
    let mut head = [0u8; 64];
    let n = std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    let head = String::from_utf8_lossy(&head[..n]);
    head.starts_with("#!") && head.lines().next().is_some_and(|l| l.contains("node"))
}

/// An npm package's `package.json` in a `node_modules` folder.
fn package_json(node_modules: &Path, package: &str) -> Option<Value> {
    let text = std::fs::read_to_string(node_modules.join(package).join("package.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The script `node_modules/<package>`'s `bin` names `executable` (a string `bin` is the package's own name), and
/// the package's version.
pub fn npm_bin(node_modules: &Path, package: &str, executable: &str) -> Option<(PathBuf, String)> {
    let manifest = package_json(node_modules, package)?;
    let rel = match &manifest["bin"] {
        Value::String(s) if package.rsplit('/').next() == Some(executable) => s.clone(),
        Value::Object(bins) => bins.get(executable)?.as_str()?.to_owned(),
        _ => return None,
    };
    let script = node_modules.join(package).join(rel);
    let version = manifest["version"].as_str().unwrap_or_default().to_owned();
    script.is_file().then_some((script, version))
}

/// The `node_modules` folders at and above `dir`, nearest first (Node's resolution order).
pub fn node_modules_above(dir: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    dir.ancestors()
        .map(|d| d.join("node_modules"))
        .filter(|d| d.is_dir())
}

/// Whether one of `names` is a file at or above `dir`.
pub fn file_above(dir: &Path, names: &[String]) -> Option<PathBuf> {
    dir.ancestors()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

impl ServerRegistration {
    /// Whether this server handles the file at `path` (by file name).
    pub fn matches(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        self.file_globs.iter().any(|g| glob_match(g, name))
    }

    /// The `languageId` of a document at `path`: the first of [`ServerRegistration::language_ids`]' globs that
    /// matches it, else [`ServerRegistration::language_id`].
    pub fn language_id_for(&self, path: &Path) -> &str {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        self.language_ids
            .iter()
            .find(|(g, _)| glob_match(g, name))
            .map_or(self.language_id.as_str(), |(_, id)| id.as_str())
    }

    /// The nearest folder at or above `file`'s that holds one of the root markers.
    pub fn find_root(&self, file: &Path) -> Option<PathBuf> {
        let start = if file.is_dir() { file } else { file.parent()? };
        start
            .ancestors()
            .find(|dir| self.root_markers.iter().any(|m| dir.join(m).is_file()))
            .map(Path::to_path_buf)
    }

    /// Whether the server runs for a workspace at `root`, given its activation setting's value (`auto` when unset):
    /// always without an [`Activation`]; with one, `on`, or `auto` and one of its root files at or above `root`.
    pub fn activated(&self, root: &Path, setting: Option<&str>) -> bool {
        let Some(a) = &self.activation else {
            return true;
        };
        match setting.unwrap_or("auto") {
            "on" => true,
            "off" => false,
            _ => file_above(root, &a.root_files).is_some(),
        }
    }

    /// Find the executable in the real environment (see [`ServerRegistration::locate_with`]), for a workspace at
    /// `project`, with the web servers' `cache` and the Node.js `node` finds. Spawns processes: call it off the UI
    /// thread.
    pub fn locate(
        &self,
        project: Option<&Path>,
        cache: Option<&Path>,
        node: &dyn Fn() -> Result<PathBuf, String>,
    ) -> Result<Located, String> {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        self.locate_with(&Environment {
            beside: beside.as_deref(),
            var: &|k| std::env::var_os(k).filter(|v| !v.is_empty()),
            path_var: std::env::var_os("PATH"),
            probe: &probe_version,
            rustup: &rustup_which,
            project,
            cache,
            node,
        })
    }

    /// [`ServerRegistration::locate`] against `env`. For an npm package: the project's `node_modules` (nearest at
    /// or above `env.project`), then the override variable, then the web servers' cache, then `PATH`. Otherwise:
    /// beside the `eludite` executable, then the override variable, then `PATH`, then the rustup component. Each
    /// candidate must prove it runs ([`Probe`]).
    pub fn locate_with(&self, env: &Environment<'_>) -> Result<Located, String> {
        let Some(cmd) = &self.command else {
            return Err(format!("{} is not launched by the shell", self.id));
        };
        locate_command(cmd, env, None)
    }

    /// Where module `m` is: the setting's path (`configured`), else the project's `node_modules/<package>/<path>`
    /// (nearest at or above `project`), else the cache's.
    pub fn locate_module(
        m: &ModuleSpec,
        configured: Option<&Path>,
        project: Option<&Path>,
        cache: Option<&Path>,
    ) -> Option<LocatedModule> {
        let version_of = |dir: &Path| {
            dir.ancestors()
                .map(|d| d.join("package.json"))
                .find(|p| p.is_file())
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["version"].as_str().map(str::to_owned))
        };
        if let Some(c) = configured.filter(|c| c.exists()) {
            let dir = if c.is_file() { c.parent()? } else { c };
            return Some(LocatedModule {
                path: c.to_path_buf(),
                version: version_of(dir),
                source: "settings",
            });
        }
        let in_modules = |nm: &Path| {
            let p = nm.join(&m.package).join(&m.path);
            p.exists().then(|| (version_of(&nm.join(&m.package)), p))
        };
        if let Some(found) = project
            .into_iter()
            .flat_map(node_modules_above)
            .find_map(|nm| in_modules(&nm))
        {
            return Some(LocatedModule {
                path: found.1,
                version: found.0,
                source: "project",
            });
        }
        let (version, path) = in_modules(&cache?.join("node_modules"))?;
        Some(LocatedModule {
            path,
            version,
            source: "pinned",
        })
    }
}

/// Where to look for an npm package's executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NpmPlaces {
    /// The project's `node_modules`, the variable, the cache, `PATH`.
    All,
    /// The project's `node_modules` only (`editor.formatter`'s `auto` for Prettier).
    Project,
}

fn locate_command(
    cmd: &CommandSpec,
    env: &Environment<'_>,
    places: Option<NpmPlaces>,
) -> Result<Located, String> {
    let exe = format!("{}{}", cmd.executable, std::env::consts::EXE_SUFFIX);
    // A candidate that runs: probed directly, or a script under Node.js.
    let try_path = |p: &Path,
                    source: &str,
                    package_version: Option<&str>|
     -> Result<Option<Located>, String> {
        if !p.is_file() {
            return Ok(None);
        }
        let node = if is_script(p) {
            Some((env.node)()?)
        } else {
            None
        };
        let version = match (cmd.probe, package_version) {
            (Probe::PackageJson, Some(v)) => Some(v.to_owned()),
            _ => (env.probe)(p, node.as_deref()),
        };
        Ok(version.map(|version| Located {
            path: p.to_path_buf(),
            version,
            source: source.to_owned(),
            node,
        }))
    };
    if let Some(package) = &cmd.npm_package {
        let places = places.unwrap_or(NpmPlaces::All);
        for nm in env.project.into_iter().flat_map(node_modules_above) {
            if let Some((script, version)) = npm_bin(&nm, package, &cmd.executable)
                && let Some(found) = try_path(&script, "project node_modules", Some(&version))?
            {
                return Ok(found);
            }
        }
        if places == NpmPlaces::Project {
            return Err(format!(
                "{} is not in the project's node_modules",
                cmd.executable
            ));
        }
        if let Some(var) = &cmd.env_override
            && let Some(value) = (env.var)(var)
        {
            let p = PathBuf::from(value);
            return try_path(&p, var, None)?
                .ok_or_else(|| format!("{var}={} does not run", p.display()));
        }
        if let Some(cache) = env.cache
            && let Some((script, version)) =
                npm_bin(&cache.join("node_modules"), package, &cmd.executable)
            && let Some(found) = try_path(&script, "web servers cache", Some(&version))?
        {
            return Ok(found);
        }
        if let Some(paths) = &env.path_var {
            for dir in std::env::split_paths(paths) {
                if let Some(found) = try_path(&dir.join(&exe), "PATH", None)? {
                    return Ok(found);
                }
            }
        }
        return Err(format!(
            "{} not found in the project's node_modules, {}the web servers' cache or on PATH",
            cmd.executable,
            cmd.env_override
                .as_deref()
                .map(|v| format!("in {v}, "))
                .unwrap_or_default(),
        ));
    }
    if let Some(dir) = env.beside
        && let Some(found) = try_path(&dir.join(&exe), "beside eludite", None)?
    {
        return Ok(found);
    }
    if let Some(var) = &cmd.env_override
        && let Some(value) = (env.var)(var)
    {
        let p = PathBuf::from(value);
        return try_path(&p, var, None)?
            .ok_or_else(|| format!("{var}={} does not run", p.display()));
    }
    if let Some(paths) = &env.path_var {
        for dir in std::env::split_paths(paths) {
            if let Some(found) = try_path(&dir.join(&exe), "PATH", None)? {
                return Ok(found);
            }
        }
    }
    if let Some(component) = &cmd.rustup_component
        && let Some(p) = (env.rustup)(&cmd.executable)
        && let Some(found) = try_path(&p, &format!("rustup component {component}"), None)?
    {
        return Ok(found);
    }
    Err(format!(
        "{} not found beside eludite, {}on PATH{}",
        cmd.executable,
        cmd.env_override
            .as_deref()
            .map(|v| format!("in {v}, "))
            .unwrap_or_default(),
        cmd.rustup_component
            .as_deref()
            .map(|c| format!(" or as the rustup component (`rustup component add {c}`)"))
            .unwrap_or_default()
    ))
}

/// What Format Document runs for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatterPick<'a> {
    /// A formatter, found.
    Formatter(&'a FormatterSpec, Located),
    /// The language server's `textDocument/formatting`.
    Server,
}

/// Replace `${name}` in every string of `value` with `vars(name)`. A string that is only an unknown variable
/// becomes `null`; an unknown variable inside a longer string becomes empty.
pub fn substitute(value: &Value, vars: &dyn Fn(&str) -> Option<String>) -> Value {
    match value {
        Value::String(s) => {
            if let Some(name) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}'))
                && !name.contains("${")
            {
                return vars(name).map_or(Value::Null, Value::String);
            }
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(i) = rest.find("${") {
                out.push_str(&rest[..i]);
                match rest[i..].find('}') {
                    Some(j) => {
                        out.push_str(&vars(&rest[i + 2..i + j]).unwrap_or_default());
                        rest = &rest[i + j + 1..];
                    }
                    None => {
                        out.push_str(&rest[i..]);
                        rest = "";
                    }
                }
            }
            out.push_str(rest);
            Value::String(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| substitute(v, vars)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), substitute(v, vars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The servers Eludite knows, and the formatters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerRegistry {
    pub servers: Vec<ServerRegistration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formatters: Vec<FormatterSpec>,
    /// The web servers' cache (`tools/web-servers/fetch.sh`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_servers: Option<CacheSpec>,
}

impl ServerRegistry {
    /// The built-in registrations (`servers.json`).
    pub fn builtin() -> Self {
        Self::from_json(BUILTIN).expect("servers.json is a valid registry")
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// The first server for the file at `path` (its primary server), if any.
    pub fn for_path(&self, path: &Path) -> Option<&ServerRegistration> {
        self.servers.iter().find(|s| s.matches(path))
    }

    /// Every server for the file at `path`, in registration order (the first is its primary server).
    pub fn all_for_path(&self, path: &Path) -> Vec<&ServerRegistration> {
        self.servers.iter().filter(|s| s.matches(path)).collect()
    }

    pub fn get(&self, id: &str) -> Option<&ServerRegistration> {
        self.servers.iter().find(|s| s.id == id)
    }

    /// The web servers' cache folder: the override variable's value, else `<home>/.cache/eludite/<folder>/<pin>`;
    /// `None` when it does not exist.
    pub fn web_servers_cache(
        &self,
        var: &dyn Fn(&str) -> Option<OsString>,
        home: Option<&Path>,
    ) -> Option<PathBuf> {
        let spec = self.web_servers.as_ref()?;
        let dir = match var(&spec.env_override) {
            Some(v) => PathBuf::from(v),
            None => home?
                .join(".cache")
                .join("eludite")
                .join(&spec.folder)
                .join(&spec.pin),
        };
        dir.is_dir().then_some(dir)
    }

    /// The command that installs the web servers (`tools/web-servers/fetch.sh`), for messages.
    pub fn fetch_command(&self) -> Option<&str> {
        self.web_servers.as_ref().map(|c| c.fetch.as_str())
    }

    /// What formats `file` under `editor.formatter`'s `choice` (`auto`, `prettier`, `biome`, `server`):
    /// `server` the language server; a formatter's id that formatter (located anywhere), else the server when it
    /// does not handle the file; `auto` the first formatter whose [`AutoRule`] holds (Prettier in the project's
    /// `node_modules`; Biome with its configuration file at or above the file's folder) and is found, else the
    /// server. An error when the chosen formatter is not found. `env.project` is the file's folder. Spawns
    /// processes: off the UI thread.
    pub fn pick_formatter(
        &self,
        file: &Path,
        choice: &str,
        env: &Environment<'_>,
    ) -> Result<FormatterPick<'_>, String> {
        let handles = |f: &FormatterSpec| {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            f.file_globs.iter().any(|g| glob_match(g, name))
        };
        match choice {
            "server" => Ok(FormatterPick::Server),
            "auto" => {
                for f in self.formatters.iter().filter(|f| handles(f)) {
                    let located = match f.auto {
                        AutoRule::ProjectPackage => {
                            locate_command(&f.command, env, Some(NpmPlaces::Project))
                        }
                        AutoRule::ConfigFile => {
                            if env
                                .project
                                .and_then(|d| file_above(d, &f.config_files))
                                .is_none()
                            {
                                continue;
                            }
                            locate_command(&f.command, env, Some(NpmPlaces::All))
                        }
                    };
                    if let Ok(found) = located {
                        return Ok(FormatterPick::Formatter(f, found));
                    }
                }
                Ok(FormatterPick::Server)
            }
            id => {
                let Some(f) = self.formatters.iter().find(|f| f.id == id) else {
                    return Err(format!(
                        "editor.formatter is {id}, which is not a formatter"
                    ));
                };
                if !handles(f) {
                    return Ok(FormatterPick::Server);
                }
                let found =
                    locate_command(&f.command, env, Some(NpmPlaces::All)).map_err(
                        |e| match self.fetch_command() {
                            Some(fetch) => format!("{e} (run {fetch})"),
                            None => e,
                        },
                    )?;
                Ok(FormatterPick::Formatter(f, found))
            }
        }
    }
}

/// `*`-only glob over a file name, ASCII case-insensitive (`*.cs` matches `Program.CS`).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some((b'*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some((c, rest)) => n
                .split_first()
                .is_some_and(|(d, n)| c.eq_ignore_ascii_case(d) && go(rest, n)),
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

/// `--version` of `path` (under `node` for a script): its first line, or `None` when it does not run.
pub fn probe_version(path: &Path, node: Option<&Path>) -> Option<String> {
    let mut command = match node {
        Some(node) => {
            let mut c = Command::new(node);
            c.arg(path);
            c
        }
        None => Command::new(path),
    };
    let out = command
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().next().unwrap_or_default().trim().to_owned())
}

fn rustup_which(executable: &str) -> Option<PathBuf> {
    let out = Command::new("rustup")
        .args(["which", executable])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| {
        PathBuf::from(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_node() -> Result<PathBuf, String> {
        Err("Node.js was not found".into())
    }

    #[test]
    fn builtin_registry_routes_by_file_name() {
        let r = ServerRegistry::builtin();
        let ra = r
            .for_path(Path::new("/w/crates/editor/src/buffer.rs"))
            .unwrap();
        assert_eq!(ra.id, "rust-analyzer");
        assert_eq!(ra.language_id, "rust");
        assert_eq!(ra.via, Via::Process);
        assert_eq!(ra.root_markers, ["Cargo.toml"]);
        assert_eq!(
            ra.command.as_ref().unwrap().env_override.as_deref(),
            Some("ELUDITE_RUST_ANALYZER")
        );
        assert_eq!(
            ra.settings["rust-analyzer"]["cargo"]["targetDir"],
            Value::Bool(true)
        );
        let cs = r.for_path(Path::new("/s/App/Program.cs")).unwrap();
        assert_eq!((cs.id.as_str(), cs.via), ("roslyn", Via::EluditeHost));
        assert!(r.for_path(Path::new("/s/README.md")).is_none());
        assert!(r.for_path(Path::new("/s/Cargo.toml")).is_none());
    }

    /// Brief 0050: a TypeScript or JavaScript file has two servers, TypeScript first; each web file kind has its
    /// server and its documents' `languageId`.
    #[test]
    fn several_servers_resolve_for_one_file_in_registration_order() {
        let r = ServerRegistry::builtin();
        let ids = |f: &str| -> Vec<&str> {
            r.all_for_path(Path::new(f))
                .iter()
                .map(|s| s.id.as_str())
                .collect()
        };
        for f in [
            "/v/src/main.ts",
            "/v/App.tsx",
            "/v/a.mts",
            "/v/a.cts",
            "/v/a.js",
            "/v/a.jsx",
            "/v/a.mjs",
            "/v/a.cjs",
        ] {
            assert_eq!(ids(f), ["typescript", "eslint"], "{f}");
        }
        assert_eq!(
            r.for_path(Path::new("/v/src/main.ts")).unwrap().id,
            "typescript"
        );
        assert_eq!(ids("/v/index.html"), ["html"]);
        assert_eq!(ids("/w/Views/Home/Index.cshtml"), ["html"]);
        assert_eq!(ids("/w/Components/Pages/Counter.razor"), ["html"]);
        assert_eq!(ids("/v/site.scss"), ["css"]);
        assert_eq!(ids("/v/package.json"), ["json"]);
        assert_eq!(ids("/v/Program.cs"), ["roslyn"]);
        let lang = |id: &str, f: &str| r.get(id).unwrap().language_id_for(Path::new(f)).to_owned();
        assert_eq!(lang("typescript", "/v/main.ts"), "typescript");
        assert_eq!(lang("typescript", "/v/App.tsx"), "typescriptreact");
        assert_eq!(lang("typescript", "/v/a.js"), "javascript");
        assert_eq!(lang("eslint", "/v/a.jsx"), "javascriptreact");
        assert_eq!(lang("html", "/w/Index.cshtml"), "html");
        assert_eq!(lang("html", "/w/Counter.razor"), "html");
        assert_eq!(lang("css", "/v/site.scss"), "scss");
        assert_eq!(lang("css", "/v/theme.less"), "less");
        assert_eq!(lang("css", "/v/site.css"), "css");
        assert_eq!(lang("json", "/v/package.json"), "json");
        assert_eq!(lang("json", "/v/tsconfig.json"), "jsonc");
        assert_eq!(lang("json", "/v/tsconfig.app.json"), "jsonc");
        assert_eq!(lang("json", "/v/.vscode/a.jsonc"), "jsonc");
        // Every web server is an npm package run by Node.js, with its variable.
        for id in ["typescript", "eslint", "html", "css", "json"] {
            let cmd = r.get(id).unwrap().command.as_ref().unwrap();
            assert!(cmd.npm_package.is_some(), "{id}");
            assert!(
                cmd.env_override.as_deref().unwrap().starts_with("ELUDITE_"),
                "{id}"
            );
            assert_eq!(cmd.args, ["--stdio"], "{id}");
        }
        let json = r.get("json").unwrap();
        assert_eq!(
            json.initialization_options["handledSchemaProtocols"],
            json!(["file"])
        );
        assert!(json.push_settings);
        let matched: Vec<&str> = json.settings["json"]["schemas"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["fileMatch"].as_array().unwrap())
            .map(|m| m.as_str().unwrap())
            .collect();
        for f in [
            "package.json",
            "tsconfig.json",
            "launchSettings.json",
            "appsettings.json",
            "appsettings.*.json",
            "global.json",
        ] {
            assert!(matched.contains(&f), "{f}");
        }
    }

    #[test]
    fn registrations_are_data() {
        let r = ServerRegistry::from_json(
            r#"{"servers": [{"id": "ts", "name": "TypeScript", "languageId": "typescript",
                "fileGlobs": ["*.ts", "*.tsx"], "via": "process",
                "command": {"executable": "typescript-language-server", "args": ["--stdio"]}}]}"#,
        )
        .unwrap();
        assert_eq!(r.for_path(Path::new("/a/b.tsx")).unwrap().id, "ts");
        assert!(ServerRegistry::from_json(r#"{"servers": [{"id": "x"}]}"#).is_err());
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.cs", "Program.CS"));
        assert!(!glob_match("*.rs", "main.rsx"));
        assert!(glob_match("Cargo.toml", "Cargo.toml"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("a*b", "ac"));
        assert!(glob_match("tsconfig.*.json", "tsconfig.app.json"));
    }

    #[test]
    fn roots_come_from_markers() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("crate/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(dir.path().join("crate/Cargo.toml"), "").unwrap();
        std::fs::write(src.join("lib.rs"), "").unwrap();
        let ra = ServerRegistry::builtin();
        let ra = ra.get("rust-analyzer").unwrap();
        assert_eq!(
            ra.find_root(&src.join("lib.rs")).as_deref(),
            Some(dir.path().join("crate").as_path())
        );
    }

    /// ESLint runs with `auto` only when a flat or legacy configuration is at or above the root; `on` and `off` win.
    #[test]
    fn eslint_activates_on_a_configuration_file() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("repo/app");
        std::fs::create_dir_all(&app).unwrap();
        let r = ServerRegistry::builtin();
        let eslint = r.get("eslint").unwrap();
        assert!(!eslint.activated(&app, None));
        assert!(!eslint.activated(&app, Some("auto")));
        assert!(eslint.activated(&app, Some("on")));
        std::fs::write(
            dir.path().join("repo/eslint.config.js"),
            "export default [];",
        )
        .unwrap();
        assert!(eslint.activated(&app, None));
        assert!(!eslint.activated(&app, Some("off")));
        std::fs::remove_file(dir.path().join("repo/eslint.config.js")).unwrap();
        std::fs::write(app.join(".eslintrc.json"), "{}").unwrap();
        assert!(eslint.activated(&app, Some("auto")));
        // Servers without an activation rule always run.
        assert!(r.get("typescript").unwrap().activated(&app, Some("off")));
    }

    fn touch(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// An npm package with a node script `bin`, as npm installs it.
    fn install(node_modules: &Path, package: &str, bin: &str, script: &str, version: &str) {
        touch(
            &node_modules.join(package).join("package.json"),
            &json!({"name": package, "version": version, "bin": {bin: script}}).to_string(),
        );
        touch(
            &node_modules.join(package).join(script),
            "#!/usr/bin/env node\nconsole.log('x')\n",
        );
    }

    #[test]
    fn locate_prefers_beside_then_env_then_path_then_rustup_and_skips_dead_proxies() {
        let dir = tempfile::tempdir().unwrap();
        let exe = format!("rust-analyzer{}", std::env::consts::EXE_SUFFIX);
        let mk = |sub: &str| {
            let d = dir.path().join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(&exe), "").unwrap();
            d
        };
        let (beside, env_dir, path_dir, proxy_dir, rustup_dir) = (
            mk("beside"),
            mk("env"),
            mk("path"),
            mk("proxy"),
            mk("rustup"),
        );
        let ra = ServerRegistry::builtin();
        let ra = ra.get("rust-analyzer").unwrap();
        // The rustup proxy on PATH answers nothing (no component installed).
        let probe = |p: &Path, _: Option<&Path>| {
            (!p.starts_with(dir.path().join("proxy"))).then(|| "rust-analyzer 1.0".to_owned())
        };
        let rustup_path = rustup_dir.join(&exe);
        let rustup = move |_: &str| Some(rustup_path.clone());
        let none = |_: &str| None;
        let env_var = env_dir.join(&exe).into_os_string();
        let with_env = move |k: &str| (k == "ELUDITE_RUST_ANALYZER").then(|| env_var.clone());
        let path_var = std::env::join_paths([&proxy_dir, &path_dir]).ok();
        let env = |beside: Option<&Path>,
                   var: &dyn Fn(&str) -> Option<OsString>,
                   path_var: Option<OsString>| {
            ra.locate_with(&Environment {
                beside,
                var,
                path_var,
                probe: &probe,
                rustup: &rustup,
                project: None,
                cache: None,
                node: &no_node,
            })
        };
        let found = env(Some(&beside), &with_env, path_var.clone()).unwrap();
        assert_eq!(
            (found.path.parent().unwrap(), found.source.as_str()),
            (beside.as_path(), "beside eludite")
        );
        assert_eq!(found.node, None);
        let found = env(None, &with_env, path_var.clone()).unwrap();
        assert_eq!(found.source, "ELUDITE_RUST_ANALYZER");
        assert_eq!(found.version, "rust-analyzer 1.0");
        let found = env(None, &none, path_var.clone()).unwrap();
        assert_eq!(
            (found.path.parent().unwrap(), found.source.as_str()),
            (path_dir.as_path(), "PATH")
        );
        let only_proxy = std::env::join_paths([&proxy_dir]).ok();
        let found = env(None, &none, only_proxy).unwrap();
        assert_eq!(found.source, "rustup component rust-analyzer");
        let missing = |_: &str| Some(dir.path().join("nope").into_os_string());
        assert!(
            env(None, &missing, path_var)
                .unwrap_err()
                .contains("does not run")
        );
    }

    /// Brief 0050: an npm server is found in the project's `node_modules` (the nearest at or above the root) first,
    /// then the variable, then the web servers' cache, then `PATH`; a script runs under the Node.js found; a server
    /// without `--version` proves itself by its package's version.
    #[test]
    fn npm_servers_are_found_in_the_project_then_the_variable_then_the_cache_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo/web");
        std::fs::create_dir_all(&root).unwrap();
        let cache = dir.path().join("cache");
        let on_path = dir.path().join("bin");
        let node = dir.path().join("node/bin/node");
        touch(&node, "");
        let exe = format!("typescript-language-server{}", std::env::consts::EXE_SUFFIX);
        touch(&on_path.join(&exe), "");
        let r = ServerRegistry::builtin();
        let ts = r.get("typescript").unwrap();
        let html = r.get("html").unwrap();
        let probed = std::cell::RefCell::new(Vec::new());
        let probe = |p: &Path, n: Option<&Path>| {
            probed
                .borrow_mut()
                .push((p.to_path_buf(), n.map(Path::to_path_buf)));
            Some("5.3.0".to_owned())
        };
        let node_found = || Ok(node.clone());
        let var_value: std::cell::RefCell<Option<OsString>> = Default::default();
        let var = |k: &str| {
            (k == "ELUDITE_TYPESCRIPT_LANGUAGE_SERVER")
                .then(|| var_value.borrow().clone())
                .flatten()
        };
        let path_var = std::env::join_paths([&on_path]).ok();
        let locate = |reg: &ServerRegistration, cache: Option<&Path>| {
            reg.locate_with(&Environment {
                beside: None,
                var: &var,
                path_var: path_var.clone(),
                probe: &probe,
                rustup: &|_| None,
                project: Some(&root),
                cache,
                node: &node_found,
            })
        };
        // Only PATH.
        let found = locate(ts, None).unwrap();
        assert_eq!(
            (found.source.as_str(), found.node.as_deref()),
            ("PATH", None)
        );
        // The cache, before PATH.
        install(
            &cache.join("node_modules"),
            "typescript-language-server",
            "typescript-language-server",
            "lib/cli.mjs",
            "5.3.0",
        );
        let found = locate(ts, Some(&cache)).unwrap();
        assert_eq!(found.source, "web servers cache");
        assert!(
            found
                .path
                .ends_with("typescript-language-server/lib/cli.mjs")
        );
        assert_eq!(found.node.as_deref(), Some(node.as_path()));
        assert_eq!(
            probed.borrow().last().unwrap(),
            &(found.path.clone(), Some(node.clone())),
            "a script answers --version under Node.js"
        );
        // The variable, before the cache.
        let script = dir.path().join("own/tsls.js");
        touch(&script, "");
        *var_value.borrow_mut() = Some(script.clone().into_os_string());
        let found = locate(ts, Some(&cache)).unwrap();
        assert_eq!(
            (found.source.as_str(), found.path.as_path()),
            ("ELUDITE_TYPESCRIPT_LANGUAGE_SERVER", script.as_path())
        );
        // The project's node_modules (one folder up), before everything.
        install(
            &dir.path().join("repo/node_modules"),
            "typescript-language-server",
            "typescript-language-server",
            "lib/cli.mjs",
            "4.3.3",
        );
        let found = locate(ts, Some(&cache)).unwrap();
        assert_eq!(found.source, "project node_modules");
        assert!(found.path.starts_with(dir.path().join("repo/node_modules")));
        // vscode-langservers-extracted's servers have no --version: their package's version.
        install(
            &cache.join("node_modules"),
            "vscode-langservers-extracted",
            "vscode-html-language-server",
            "bin/vscode-html-language-server",
            "4.10.0",
        );
        let before = probed.borrow().len();
        let found = locate(html, Some(&cache)).unwrap();
        assert_eq!(
            (found.version.as_str(), found.source.as_str()),
            ("4.10.0", "web servers cache")
        );
        assert_eq!(probed.borrow().len(), before, "not run");
        // No Node.js: the script cannot run, and the message says why.
        let e = html
            .locate_with(&Environment {
                beside: None,
                var: &|_| None,
                path_var: None,
                probe: &probe,
                rustup: &|_| None,
                project: Some(&root),
                cache: Some(&cache),
                node: &no_node,
            })
            .unwrap_err();
        assert!(e.contains("Node.js was not found"), "{e}");
        // Nothing anywhere: the message names the places.
        let e = html
            .locate_with(&Environment {
                beside: None,
                var: &|_| None,
                path_var: None,
                probe: &probe,
                rustup: &|_| None,
                project: Some(&root),
                cache: None,
                node: &node_found,
            })
            .unwrap_err();
        assert!(
            e.contains("vscode-html-language-server not found")
                && e.contains("ELUDITE_HTML_LANGUAGE_SERVER"),
            "{e}"
        );
    }

    /// The project's TypeScript wins over the pinned one; the setting over both; the options name it.
    #[test]
    fn modules_are_the_setting_then_the_project_then_the_pinned_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        let cache = dir.path().join("cache");
        let ts = &ServerRegistry::builtin().get("typescript").unwrap().clone();
        let m = &ts.modules[0];
        touch(
            &cache.join("node_modules/typescript/package.json"),
            r#"{"version": "6.0.3"}"#,
        );
        touch(&cache.join("node_modules/typescript/lib/tsserver.js"), "");
        std::fs::create_dir_all(&root).unwrap();
        let found = ServerRegistration::locate_module(m, None, Some(&root), Some(&cache)).unwrap();
        assert_eq!(
            (found.source, found.version.as_deref()),
            ("pinned", Some("6.0.3"))
        );
        assert_eq!(found.path, cache.join("node_modules/typescript/lib"));
        touch(
            &root.join("node_modules/typescript/package.json"),
            r#"{"version": "5.9.3"}"#,
        );
        touch(&root.join("node_modules/typescript/lib/tsserver.js"), "");
        let found = ServerRegistration::locate_module(m, None, Some(&root), Some(&cache)).unwrap();
        assert_eq!(
            (found.source, found.version.as_deref()),
            ("project", Some("5.9.3"))
        );
        let mine = dir.path().join("ts/lib/tsserver.js");
        touch(
            &dir.path().join("ts/package.json"),
            r#"{"version": "5.0.0"}"#,
        );
        touch(&mine, "");
        let found =
            ServerRegistration::locate_module(m, Some(&mine), Some(&root), Some(&cache)).unwrap();
        assert_eq!(
            (found.source, found.version.as_deref()),
            ("settings", Some("5.0.0"))
        );
        assert!(ServerRegistration::locate_module(m, None, None, None).is_none());
        let options = substitute(&ts.initialization_options, &|v| {
            (v == "module:typescript").then(|| "/app/node_modules/typescript/lib".to_owned())
        });
        assert_eq!(
            options["tsserver"]["path"],
            "/app/node_modules/typescript/lib"
        );
        assert_eq!(
            substitute(&ts.initialization_options, &|_| None)["tsserver"]["path"],
            Value::Null
        );
    }

    #[test]
    fn substitutions_fill_strings_everywhere() {
        let v = json!({"a": "${rootUri}", "b": ["x ${rootName} y", 3], "c": {"d": "${cacheUri}/schemas/package.json"}, "e": "${nope}", "f": "${broken"});
        let out = substitute(&v, &|k| match k {
            "rootUri" => Some("file:///w".into()),
            "rootName" => Some("w".into()),
            "cacheUri" => Some("file:///c".into()),
            _ => None,
        });
        assert_eq!(
            out,
            json!({"a": "file:///w", "b": ["x w y", 3], "c": {"d": "file:///c/schemas/package.json"}, "e": null, "f": "${broken"})
        );
    }

    /// `auto`: the project's Prettier, else Biome when its configuration is there, else the server; a named formatter
    /// is found anywhere (the cache too), and one not found is an error that names the fetch command.
    #[test]
    fn the_format_chain_is_project_prettier_then_biome_then_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("app/src");
        std::fs::create_dir_all(&src).unwrap();
        let cache = dir.path().join("cache");
        let node = dir.path().join("node");
        touch(&node, "");
        let r = ServerRegistry::builtin();
        let file = src.join("main.ts");
        let node_found = || Ok(node.clone());
        let pick =
            |file: &Path, choice: &str, cache: Option<&Path>| -> Result<(String, String), String> {
                let env = Environment {
                    beside: None,
                    var: &|_| None,
                    path_var: None,
                    probe: &|_, _| Some("3.9.9".to_owned()),
                    rustup: &|_| None,
                    project: file.parent(),
                    cache,
                    node: &node_found,
                };
                Ok(match r.pick_formatter(file, choice, &env)? {
                    FormatterPick::Server => ("server".into(), String::new()),
                    FormatterPick::Formatter(f, l) => (f.id.clone(), l.source),
                })
            };
        assert_eq!(pick(&file, "auto", None).unwrap().0, "server");
        assert_eq!(pick(&file, "server", None).unwrap().0, "server");
        let e = pick(&file, "prettier", None).unwrap_err();
        assert!(
            e.contains("prettier not found") && e.contains("tools/web-servers/fetch.sh"),
            "{e}"
        );
        install(
            &cache.join("node_modules"),
            "prettier",
            "prettier",
            "bin/prettier.cjs",
            "3.9.9",
        );
        install(
            &cache.join("node_modules"),
            "@biomejs/biome",
            "biome",
            "bin/biome",
            "2.5.15",
        );
        // The pinned Prettier is not the project's: auto stays with the server.
        assert_eq!(pick(&file, "auto", Some(&cache)).unwrap().0, "server");
        assert_eq!(
            pick(&file, "prettier", Some(&cache)).unwrap(),
            ("prettier".into(), "web servers cache".into())
        );
        // Biome with its configuration file at the root.
        touch(&dir.path().join("app/biome.json"), "{}");
        assert_eq!(
            pick(&file, "auto", Some(&cache)).unwrap(),
            ("biome".into(), "web servers cache".into())
        );
        // The project's Prettier first.
        install(
            &dir.path().join("app/node_modules"),
            "prettier",
            "prettier",
            "bin/prettier.cjs",
            "3.0.0",
        );
        assert_eq!(
            pick(&file, "auto", Some(&cache)).unwrap(),
            ("prettier".into(), "project node_modules".into())
        );
        // Biome does not format HTML: the server does.
        assert_eq!(
            pick(&src.join("index.html"), "biome", Some(&cache))
                .unwrap()
                .0,
            "server"
        );
        // A Rust file has no formatter here.
        assert_eq!(
            pick(&src.join("lib.rs"), "auto", Some(&cache)).unwrap().0,
            "server"
        );
        assert!(pick(&file, "clang", None).is_err());
    }

    #[test]
    fn the_web_servers_cache_is_the_variable_else_the_pinned_folder() {
        let dir = tempfile::tempdir().unwrap();
        let r = ServerRegistry::builtin();
        let pinned = dir
            .path()
            .join(".cache/eludite/web-servers")
            .join(&r.web_servers.as_ref().unwrap().pin);
        assert_eq!(r.web_servers_cache(&|_| None, Some(dir.path())), None);
        std::fs::create_dir_all(&pinned).unwrap();
        assert_eq!(
            r.web_servers_cache(&|_| None, Some(dir.path())),
            Some(pinned)
        );
        let other = dir.path().join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        let o = other.clone().into_os_string();
        assert_eq!(
            r.web_servers_cache(
                &|k| (k == "ELUDITE_WEB_SERVERS").then(|| o.clone()),
                Some(dir.path())
            ),
            Some(other)
        );
        assert_eq!(r.fetch_command(), Some("tools/web-servers/fetch.sh"));
    }

    /// `tools/web-servers/PIN` lists every package of the checked-in lockfile with its version and SPDX license, and
    /// its pin is the one servers.json searches.
    #[test]
    fn the_pin_matches_the_lockfile_and_servers_json() {
        let pin = include_str!("../../../tools/web-servers/PIN");
        let lock: Value =
            serde_json::from_str(include_str!("../../../tools/web-servers/package-lock.json"))
                .unwrap();
        let listed: BTreeMap<String, (String, String)> = pin
            .lines()
            .filter_map(|l| l.strip_prefix("package "))
            .map(|l| {
                let mut parts = l.splitn(3, ' ');
                let name = parts.next().unwrap().to_owned();
                let version = parts.next().unwrap().to_owned();
                (name, (version, parts.next().unwrap().to_owned()))
            })
            .collect();
        let locked: BTreeMap<String, (String, String)> = lock["packages"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| !k.is_empty())
            .map(|(k, v)| {
                (
                    k.strip_prefix("node_modules/").unwrap().to_owned(),
                    (
                        v["version"].as_str().unwrap().to_owned(),
                        v["license"].as_str().unwrap().to_owned(),
                    ),
                )
            })
            .collect();
        assert_eq!(listed, locked);
        for (name, (_, spdx)) in &listed {
            assert!(
                [
                    "MIT",
                    "Apache-2.0",
                    "MIT OR Apache-2.0",
                    "ISC",
                    "BSD-2-Clause"
                ]
                .contains(&spdx.as_str()),
                "{name}: {spdx}"
            );
        }
        let pinned = pin.lines().find_map(|l| l.strip_prefix("pin ")).unwrap();
        assert_eq!(ServerRegistry::builtin().web_servers.unwrap().pin, pinned);
    }
}
