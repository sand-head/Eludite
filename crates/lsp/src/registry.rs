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
//!
//! Brief 0063 adds the .NET languages: `*.vb` goes to the host as the `roslyn` registration's second glob with the
//! `languageId` `vb`, and FsAutoComplete (`fsautocomplete`, the F# server) is a `process` server that is a .NET
//! global tool ([`CommandSpec::dotnet_tool`]): found beside `eludite`, at its override variable, in the pinned cache
//! `tools/fsautocomplete/fetch.sh` installs (`~/.cache/eludite/<tool>/<pin>/`, [`ServerRegistry::dotnet_tool_cache`]),
//! in the .NET global tools folder (`~/.dotnet/tools`, `%USERPROFILE%\.dotnet\tools`, or under `DOTNET_CLI_HOME`),
//! then on `PATH`. A .NET tool's apphost refuses to start when `dotnet` is not at the default location and
//! `DOTNET_ROOT` is unset, so a located .NET tool carries that variable ([`Located::envs`], [`dotnet_tool_envs`]) for
//! its probe and its spawn. Root markers with a `*` (`*.fsproj`) match the folder's file names by glob.

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
    /// The .NET global tool that provides it (`fsautocomplete`, brief 0063): searched beside `eludite`, then the
    /// override variable, then the pinned cache (`~/.cache/eludite/<tool>/<pin>/`, [`ServerRegistry::dotnet_tools`]),
    /// then the .NET global tools folder (`~/.dotnet/tools`), then `PATH`; spawned with `DOTNET_ROOT` when needed
    /// ([`dotnet_tool_envs`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dotnet_tool: Option<String>,
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

/// A .NET global tool a server is (brief 0063), by tool name under [`ServerRegistry::dotnet_tools`]: its pinned
/// version, installed by its fetch script into `~/.cache/eludite/<tool>/<pin>/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolSpec {
    /// `tools/<tool>/PIN`'s `version`: the folder under `~/.cache/eludite/<tool>/`.
    pub pin: String,
    /// The command that installs it (`tools/fsautocomplete/fetch.sh`), for the "not found" messages.
    pub fetch: String,
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
    /// `beside eludite`, `ELUDITE_RUST_ANALYZER`, `PATH`, `rustup`, `project node_modules`, `web servers cache`,
    /// `pinned cache`, `.NET global tools`.
    pub source: String,
    /// The Node.js that runs [`Located::path`], for a script.
    pub node: Option<PathBuf>,
    /// Environment variables the process needs (brief 0063): `DOTNET_ROOT` for a .NET tool when the variable is
    /// unset ([`dotnet_tool_envs`]); empty otherwise.
    pub envs: Vec<(String, String)>,
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

/// `--version` of a candidate executable (run by Node.js when the second argument is given, with the third argument's
/// environment variables set): its first line, or `None` when it does not run. [`probe_version`] in the real
/// environment.
pub type ProbeFn<'a> = dyn Fn(&Path, Option<&Path>, &[(String, String)]) -> Option<String> + 'a;

/// What [`ServerRegistration::locate_with`] may consult; the real process environment in
/// [`ServerRegistration::locate`].
pub struct Environment<'a> {
    /// The folder of the `eludite` executable.
    pub beside: Option<&'a Path>,
    pub var: &'a dyn Fn(&str) -> Option<OsString>,
    pub path_var: Option<OsString>,
    /// `--version` of a candidate ([`ProbeFn`]; the variables are a .NET tool's `DOTNET_ROOT`).
    pub probe: &'a ProbeFn<'a>,
    /// `rustup which <executable>`.
    pub rustup: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// Where the `node_modules` search starts: the server's root, a formatter's document folder.
    pub project: Option<&'a Path>,
    /// The registration's cache folder, when it exists: the web servers' for an npm package
    /// ([`ServerRegistry::web_servers_cache`]), the tool's pinned folder for a .NET tool
    /// ([`ServerRegistry::dotnet_tool_cache`]).
    pub cache: Option<&'a Path>,
    /// Node.js, for an npm package's script.
    pub node: &'a dyn Fn() -> Result<PathBuf, String>,
    /// The home folder (`HOME`, `USERPROFILE`), for the .NET global tools folder `.dotnet/tools` (under
    /// `DOTNET_CLI_HOME` instead when that is set).
    pub home: Option<&'a Path>,
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

/// Whether `dir` holds root marker `marker`: a file of that name, or with a `*` a file whose name matches the glob.
fn has_marker(dir: &Path, marker: &str) -> bool {
    if !marker.contains('*') {
        return dir.join(marker).is_file();
    }
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| glob_match(marker, name))
                && entry.path().is_file()
        })
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

    /// The nearest folder at or above `file`'s that holds one of the root markers: a marker with a `*` (`*.fsproj`)
    /// matches the folder's file names by glob, one without is a file name (`Cargo.toml`).
    pub fn find_root(&self, file: &Path) -> Option<PathBuf> {
        let start = if file.is_dir() { file } else { file.parent()? };
        start
            .ancestors()
            .find(|dir| self.root_markers.iter().any(|m| has_marker(dir, m)))
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
    /// `project`, with the registration's `cache` folder (the web servers' for an npm package, the pinned one for a
    /// .NET tool) and the Node.js `node` finds. Spawns processes: call it off the UI thread.
    pub fn locate(
        &self,
        project: Option<&Path>,
        cache: Option<&Path>,
        node: &dyn Fn() -> Result<PathBuf, String>,
    ) -> Result<Located, String> {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let home = crate::node::home_dir();
        self.locate_with(&Environment {
            beside: beside.as_deref(),
            var: &|k| std::env::var_os(k).filter(|v| !v.is_empty()),
            path_var: std::env::var_os("PATH"),
            probe: &probe_version,
            rustup: &rustup_which,
            project,
            cache,
            node,
            home: home.as_deref(),
        })
    }

    /// [`ServerRegistration::locate`] against `env`. For an npm package: the project's `node_modules` (nearest at
    /// or above `env.project`), then the override variable, then the web servers' cache, then `PATH`. For a .NET
    /// tool: beside the `eludite` executable, then the override variable, then the pinned cache (`env.cache`), then
    /// the .NET global tools folder, then `PATH`, each probed with the tool's environment ([`dotnet_tool_envs`]).
    /// Otherwise: beside the `eludite` executable, then the override variable, then `PATH`, then the rustup
    /// component. Each candidate must prove it runs ([`Probe`]).
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
    // A .NET tool runs with `DOTNET_ROOT` when the variable is unset (brief 0063); anything else with nothing added.
    let envs = if cmd.dotnet_tool.is_some() {
        dotnet_tool_envs(env.var, env.path_var.as_ref())
    } else {
        Vec::new()
    };
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
            _ => (env.probe)(p, node.as_deref(), &envs),
        };
        Ok(version.map(|version| Located {
            path: p.to_path_buf(),
            version,
            source: source.to_owned(),
            node,
            envs: envs.clone(),
        }))
    };
    let on_path = |exe: &str| -> Result<Option<Located>, String> {
        if let Some(paths) = &env.path_var {
            for dir in std::env::split_paths(paths) {
                if let Some(found) = try_path(&dir.join(exe), "PATH", None)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    };
    let from_var = |var: &str, value: OsString| -> Result<Located, String> {
        let p = PathBuf::from(value);
        try_path(&p, var, None)?.ok_or_else(|| format!("{var}={} does not run", p.display()))
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
            return from_var(var, value);
        }
        if let Some(cache) = env.cache
            && let Some((script, version)) =
                npm_bin(&cache.join("node_modules"), package, &cmd.executable)
            && let Some(found) = try_path(&script, "web servers cache", Some(&version))?
        {
            return Ok(found);
        }
        if let Some(found) = on_path(&exe)? {
            return Ok(found);
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
        return from_var(var, value);
    }
    if cmd.dotnet_tool.is_some() {
        // The pinned cache (the registration's fetch script), the .NET global tools folder, then PATH.
        if let Some(cache) = env.cache
            && let Some(found) = try_path(&cache.join(&exe), "pinned cache", None)?
        {
            return Ok(found);
        }
        if let Some(tools) = dotnet_global_tools(env.var, env.home)
            && let Some(found) = try_path(&tools.join(&exe), ".NET global tools", None)?
        {
            return Ok(found);
        }
        if let Some(found) = on_path(&exe)? {
            return Ok(found);
        }
        return Err(format!(
            "{} not found beside eludite, {}in the pinned cache, in the .NET global tools folder or on PATH",
            cmd.executable,
            cmd.env_override
                .as_deref()
                .map(|v| format!("in {v}, "))
                .unwrap_or_default(),
        ));
    }
    if let Some(found) = on_path(&exe)? {
        return Ok(found);
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

/// The .NET global tools folder: `.dotnet/tools` under `DOTNET_CLI_HOME` when set, else under `home` (`HOME`,
/// `%USERPROFILE%`).
fn dotnet_global_tools(
    var: &dyn Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let base = match var("DOTNET_CLI_HOME") {
        Some(h) => PathBuf::from(h),
        None => home?.to_path_buf(),
    };
    Some(base.join(".dotnet").join("tools"))
}

/// The environment a .NET global tool's apphost needs (brief 0063): nothing when `DOTNET_ROOT` is set, else
/// `DOTNET_ROOT` as the folder of the `dotnet` executable on `PATH` with symlinks resolved (the SDK root, which holds
/// `shared/`). The apphost refuses to start when `dotnet` is neither at the default location nor registered nor named
/// by the variable, as with a user-local SDK (`~/.dotnet`). Nothing when `dotnet` is not on `PATH` either.
pub fn dotnet_tool_envs(
    var: &dyn Fn(&str) -> Option<OsString>,
    path_var: Option<&OsString>,
) -> Vec<(String, String)> {
    if var("DOTNET_ROOT").is_some() {
        return Vec::new();
    }
    let dotnet = format!("dotnet{}", std::env::consts::EXE_SUFFIX);
    let root = path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(&dotnet))
        .filter(|p| p.is_file())
        .find_map(|p| {
            let real = std::fs::canonicalize(&p).unwrap_or(p);
            real.parent()
                .map(|d| without_verbatim_prefix(d.to_path_buf()))
        });
    match root {
        Some(root) => vec![(
            "DOTNET_ROOT".to_owned(),
            root.to_string_lossy().into_owned(),
        )],
        None => Vec::new(),
    }
}

/// `canonicalize` on Windows yields `\\?\C:\...`, which the apphost does not read: the plain form.
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) if cfg!(windows) => PathBuf::from(plain),
        _ => path,
    }
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
    /// The .NET global tools servers are (brief 0063), by tool name (`fsautocomplete`): the pin of
    /// `tools/<tool>/PIN` and the fetch script.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dotnet_tools: BTreeMap<String, ToolSpec>,
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

    /// The pinned cache folder of .NET tool `tool`: `<home>/.cache/eludite/<tool>/<pin>`, what `tools/<tool>/fetch.sh`
    /// installs into; `None` for a tool the registry does not pin, or when the folder does not exist.
    pub fn dotnet_tool_cache(&self, tool: &str, home: Option<&Path>) -> Option<PathBuf> {
        let spec = self.dotnet_tools.get(tool)?;
        let dir = home?
            .join(".cache")
            .join("eludite")
            .join(tool)
            .join(&spec.pin);
        dir.is_dir().then_some(dir)
    }

    /// The command that installs server `reg` when it is not found, for its status message: the web servers' for an
    /// npm package, the tool's for a .NET tool, none otherwise (rust-analyzer is a rustup component).
    pub fn fetch_command_for(&self, reg: &ServerRegistration) -> Option<&str> {
        let cmd = reg.command.as_ref()?;
        if cmd.npm_package.is_some() {
            return self.fetch_command();
        }
        let tool = cmd.dotnet_tool.as_deref()?;
        self.dotnet_tools.get(tool).map(|t| t.fetch.as_str())
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

/// `--version` of `path` (under `node` for a script, with `envs` set): its first line, or `None` when it does not
/// run.
pub fn probe_version(
    path: &Path,
    node: Option<&Path>,
    envs: &[(String, String)],
) -> Option<String> {
    let mut command = match node {
        Some(node) => {
            let mut c = Command::new(node);
            c.arg(path);
            c
        }
        None => Command::new(path),
    };
    let out = command
        .envs(envs.iter().map(|(k, v)| (k, v)))
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
        let probe = |p: &Path, _: Option<&Path>, _: &[(String, String)]| {
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
                home: None,
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
        let probe = |p: &Path, n: Option<&Path>, _: &[(String, String)]| {
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
                home: None,
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
                home: None,
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
                home: None,
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
                    probe: &|_, _, _| Some("3.9.9".to_owned()),
                    rustup: &|_| None,
                    project: file.parent(),
                    cache,
                    node: &node_found,
                    home: None,
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

    /// Brief 0063: `.vb` is the host's (Roslyn), as `vb`; the F# files are FsAutoComplete's, as `fsharp`, a .NET tool
    /// with its variable and its workspace options.
    #[test]
    fn vb_goes_to_the_host_and_fsharp_to_fsautocomplete() {
        let r = ServerRegistry::builtin();
        let vb = r.for_path(Path::new("/s/App/Module1.vb")).unwrap();
        assert_eq!((vb.id.as_str(), vb.via), ("roslyn", Via::EluditeHost));
        assert_eq!(vb.name, "C# and Visual Basic");
        assert_eq!(vb.language_id_for(Path::new("/s/App/Module1.vb")), "vb");
        assert_eq!(vb.language_id_for(Path::new("/s/App/Program.cs")), "csharp");
        assert_eq!(r.all_for_path(Path::new("/s/App/Module1.VB")).len(), 1);
        for f in [
            "/f/Library.fs",
            "/f/Library.fsi",
            "/f/script.fsx",
            "/f/old.fsscript",
        ] {
            let regs = r.all_for_path(Path::new(f));
            assert_eq!(regs.len(), 1, "{f}");
            let fsac = regs[0];
            assert_eq!(
                (fsac.id.as_str(), fsac.via),
                ("fsautocomplete", Via::Process)
            );
            assert_eq!(fsac.name, "FsAutoComplete");
            assert_eq!(fsac.language_id_for(Path::new(f)), "fsharp", "{f}");
        }
        let fsac = r.get("fsautocomplete").unwrap();
        assert_eq!(fsac.root_markers, ["*.fsproj", "*.sln", "*.slnx"]);
        let cmd = fsac.command.as_ref().unwrap();
        assert_eq!(cmd.executable, "fsautocomplete");
        assert!(cmd.args.is_empty());
        assert_eq!(cmd.env_override.as_deref(), Some("ELUDITE_FSAUTOCOMPLETE"));
        assert_eq!(cmd.dotnet_tool.as_deref(), Some("fsautocomplete"));
        assert_eq!(cmd.probe, Probe::Version);
        assert_eq!(
            fsac.initialization_options,
            json!({"AutomaticWorkspaceInit": true})
        );
        assert_eq!(
            r.fetch_command_for(fsac),
            Some("tools/fsautocomplete/fetch.sh")
        );
        assert_eq!(
            r.fetch_command_for(r.get("typescript").unwrap()),
            Some("tools/web-servers/fetch.sh")
        );
        assert_eq!(r.fetch_command_for(r.get("rust-analyzer").unwrap()), None);
        assert_eq!(r.fetch_command_for(vb), None);
        assert!(r.for_path(Path::new("/f/Lib.fsproj")).is_none());
    }

    /// Brief 0063: a root marker with a `*` matches the folder's file names (`*.fsproj`), the nearest folder wins, and
    /// a marker without one is still a file name (`Cargo.toml`: a folder named so does not count).
    #[test]
    fn glob_root_markers_find_the_folder_holding_a_project_file() {
        let dir = tempfile::tempdir().unwrap();
        let types = dir.path().join("repo/src/Lib/Types");
        std::fs::create_dir_all(&types).unwrap();
        std::fs::write(types.join("Shapes.fs"), "").unwrap();
        let r = ServerRegistry::builtin();
        let fsac = r.get("fsautocomplete").unwrap();
        assert_eq!(fsac.find_root(&types.join("Shapes.fs")), None);
        std::fs::write(dir.path().join("repo/App.sln"), "").unwrap();
        assert_eq!(
            fsac.find_root(&types.join("Shapes.fs")).as_deref(),
            Some(dir.path().join("repo").as_path())
        );
        std::fs::write(dir.path().join("repo/src/Lib/Lib.fsproj"), "").unwrap();
        assert_eq!(
            fsac.find_root(&types.join("Shapes.fs")).as_deref(),
            Some(dir.path().join("repo/src/Lib").as_path())
        );
        // A folder whose name matches is not a marker.
        std::fs::create_dir_all(types.join("Nope.fsproj")).unwrap();
        assert_eq!(
            fsac.find_root(&types.join("Shapes.fs")).as_deref(),
            Some(dir.path().join("repo/src/Lib").as_path())
        );
        let ra = r.get("rust-analyzer").unwrap();
        std::fs::create_dir_all(types.join("Cargo.toml")).unwrap();
        assert_eq!(ra.find_root(&types.join("Shapes.fs")), None);
    }

    /// Brief 0063: a .NET tool is found beside eludite, then at its variable, then in the pinned cache, then in the
    /// .NET global tools folder (under `DOTNET_CLI_HOME` when set), then on PATH; every candidate is probed with the
    /// tool's environment; nothing anywhere names the places.
    #[test]
    fn dotnet_tools_are_found_beside_then_variable_then_cache_then_global_tools_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = format!("fsautocomplete{}", std::env::consts::EXE_SUFFIX);
        let mk = |sub: &str| {
            let d = dir.path().join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(&exe), "").unwrap();
            d
        };
        let (beside, env_dir, cache, path_dir) = (mk("beside"), mk("env"), mk("cache"), mk("bin"));
        let home = dir.path().join("home");
        let tools = home.join(".dotnet/tools");
        let cli_home = dir.path().join("cli-home");
        let cli_tools = cli_home.join(".dotnet/tools");
        let r = ServerRegistry::builtin();
        let fsac = r.get("fsautocomplete").unwrap();
        let probed = std::cell::RefCell::new(Vec::new());
        let probe = |p: &Path, n: Option<&Path>, envs: &[(String, String)]| {
            assert_eq!(n, None);
            probed.borrow_mut().push((p.to_path_buf(), envs.to_vec()));
            Some("0.84.0+abc".to_owned())
        };
        let vars: std::cell::RefCell<BTreeMap<String, OsString>> = Default::default();
        let var = |k: &str| vars.borrow().get(k).cloned();
        let path_var = std::env::join_paths([&path_dir]).ok();
        let locate = |beside: Option<&Path>, cache: Option<&Path>, home: Option<&Path>| {
            fsac.locate_with(&Environment {
                beside,
                var: &var,
                path_var: path_var.clone(),
                probe: &probe,
                rustup: &|_| panic!("not a rustup component"),
                project: Some(dir.path()),
                cache,
                node: &no_node,
                home,
            })
        };
        // Only PATH (no `dotnet` there: nothing to set).
        let found = locate(None, None, Some(&home)).unwrap();
        assert_eq!(
            (
                found.source.as_str(),
                found.path.as_path(),
                found.node.as_deref()
            ),
            ("PATH", path_dir.join(&exe).as_path(), None)
        );
        assert_eq!(found.version, "0.84.0+abc");
        assert!(found.envs.is_empty(), "{:?}", found.envs);
        // The global tools folder, before PATH.
        mk("home/.dotnet/tools");
        let found = locate(None, None, Some(&home)).unwrap();
        assert_eq!(
            (found.source.as_str(), found.path.as_path()),
            (".NET global tools", tools.join(&exe).as_path())
        );
        // DOTNET_CLI_HOME moves it.
        vars.borrow_mut()
            .insert("DOTNET_CLI_HOME".into(), cli_home.clone().into_os_string());
        assert_eq!(locate(None, None, Some(&home)).unwrap().source, "PATH");
        mk("cli-home/.dotnet/tools");
        assert_eq!(
            locate(None, None, Some(&home)).unwrap().path,
            cli_tools.join(&exe)
        );
        vars.borrow_mut().remove("DOTNET_CLI_HOME");
        // The pinned cache, before the global tools.
        let found = locate(None, Some(&cache), Some(&home)).unwrap();
        assert_eq!(
            (found.source.as_str(), found.path.as_path()),
            ("pinned cache", cache.join(&exe).as_path())
        );
        // The variable, before the cache.
        vars.borrow_mut().insert(
            "ELUDITE_FSAUTOCOMPLETE".into(),
            env_dir.join(&exe).into_os_string(),
        );
        let found = locate(None, Some(&cache), Some(&home)).unwrap();
        assert_eq!(found.source, "ELUDITE_FSAUTOCOMPLETE");
        // Beside eludite, before everything.
        let found = locate(Some(&beside), Some(&cache), Some(&home)).unwrap();
        assert_eq!(
            (found.source.as_str(), found.path.as_path()),
            ("beside eludite", beside.join(&exe).as_path())
        );
        // A variable naming something that does not run is an error, not a fallthrough.
        vars.borrow_mut().insert(
            "ELUDITE_FSAUTOCOMPLETE".into(),
            dir.path().join("nope").into_os_string(),
        );
        let e = locate(None, Some(&cache), Some(&home)).unwrap_err();
        assert!(
            e.contains("ELUDITE_FSAUTOCOMPLETE") && e.contains("does not run"),
            "{e}"
        );
        vars.borrow_mut().remove("ELUDITE_FSAUTOCOMPLETE");
        // Nothing anywhere: the message names the places.
        let e = fsac
            .locate_with(&Environment {
                beside: None,
                var: &var,
                path_var: None,
                probe: &probe,
                rustup: &|_| None,
                project: None,
                cache: None,
                node: &no_node,
                home: None,
            })
            .unwrap_err();
        assert!(
            e.contains("fsautocomplete not found")
                && e.contains("ELUDITE_FSAUTOCOMPLETE")
                && e.contains("pinned cache")
                && e.contains(".NET global tools")
                && e.contains("PATH"),
            "{e}"
        );
        // Every candidate was probed directly (no Node.js) and without a variable: `dotnet` was not on PATH.
        assert!(!probed.borrow().is_empty());
        assert!(probed.borrow().iter().all(|(_, envs)| envs.is_empty()));
    }

    /// Brief 0063: with `dotnet` on PATH and `DOTNET_ROOT` unset, a .NET tool is probed and located with `DOTNET_ROOT`
    /// set to the resolved `dotnet`'s folder (symlinks followed); set already, nothing is added; a server that is not
    /// a .NET tool never gets it.
    #[test]
    fn dotnet_root_is_added_for_a_dotnet_tool_when_unset_and_not_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let exe = format!("fsautocomplete{}", std::env::consts::EXE_SUFFIX);
        let dotnet = format!("dotnet{}", std::env::consts::EXE_SUFFIX);
        let sdk = dir.path().join("sdk");
        std::fs::create_dir_all(sdk.join("shared")).unwrap();
        std::fs::write(sdk.join(&dotnet), "").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(&exe), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(sdk.join(&dotnet), bin.join(&dotnet)).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(sdk.join(&dotnet), bin.join(&dotnet)).unwrap();
        // Through the symlink on Unix; a copy on Windows, so the folder is the copy's.
        #[cfg(unix)]
        let expected_root = without_verbatim_prefix(std::fs::canonicalize(&sdk).unwrap());
        #[cfg(not(unix))]
        let expected_root = without_verbatim_prefix(std::fs::canonicalize(&bin).unwrap());
        let path_var = std::env::join_paths([&bin]).ok();
        let none = |_: &str| None;
        let envs = dotnet_tool_envs(&none, path_var.as_ref());
        assert_eq!(
            envs,
            vec![(
                "DOTNET_ROOT".to_owned(),
                expected_root.to_string_lossy().into_owned()
            )]
        );
        let set = |k: &str| (k == "DOTNET_ROOT").then(|| OsString::from("/opt/dotnet"));
        assert!(dotnet_tool_envs(&set, path_var.as_ref()).is_empty());
        assert!(dotnet_tool_envs(&none, None).is_empty());
        let r = ServerRegistry::builtin();
        let probed = std::cell::RefCell::new(Vec::new());
        let probe = |p: &Path, _: Option<&Path>, envs: &[(String, String)]| {
            probed.borrow_mut().push((p.to_path_buf(), envs.to_vec()));
            Some("x".to_owned())
        };
        let locate = |id: &str, var: &dyn Fn(&str) -> Option<OsString>| {
            r.get(id).unwrap().locate_with(&Environment {
                beside: Some(&bin),
                var,
                path_var: path_var.clone(),
                probe: &probe,
                rustup: &|_| None,
                project: None,
                cache: None,
                node: &no_node,
                home: None,
            })
        };
        let found = locate("fsautocomplete", &none).unwrap();
        assert_eq!(found.envs, envs);
        assert_eq!(probed.borrow().last().unwrap().1, envs, "probed with it");
        assert!(locate("fsautocomplete", &set).unwrap().envs.is_empty());
        // rust-analyzer beside eludite: not a .NET tool.
        std::fs::write(
            bin.join(format!("rust-analyzer{}", std::env::consts::EXE_SUFFIX)),
            "",
        )
        .unwrap();
        let found = locate("rust-analyzer", &none).unwrap();
        assert_eq!(found.source, "beside eludite");
        assert!(found.envs.is_empty());
        assert!(probed.borrow().last().unwrap().1.is_empty());
    }

    /// Brief 0063: `tools/fsautocomplete/PIN`'s version is the pin servers.json searches under `~/.cache/eludite/`, the
    /// fetch script is the one the messages name, and the cache folder is found by that pin.
    #[test]
    fn the_fsautocomplete_pin_matches_servers_json() {
        let pin = include_str!("../../../tools/fsautocomplete/PIN");
        let version = pin
            .lines()
            .find_map(|l| l.strip_prefix("version "))
            .unwrap()
            .trim();
        assert!(
            version.starts_with(|c: char| c.is_ascii_digit()),
            "{version}"
        );
        let r = ServerRegistry::builtin();
        let spec = r.dotnet_tools.get("fsautocomplete").unwrap();
        assert_eq!(spec.pin, version);
        assert_eq!(spec.fetch, "tools/fsautocomplete/fetch.sh");
        assert!(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tools/fsautocomplete/fetch.sh"
            ))
            .is_file()
        );
        assert!(
            Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../tools/fsautocomplete/fetch.ps1"
            ))
            .is_file()
        );
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            r.dotnet_tool_cache("fsautocomplete", Some(dir.path())),
            None
        );
        assert_eq!(r.dotnet_tool_cache("fsautocomplete", None), None);
        let pinned = dir
            .path()
            .join(".cache/eludite/fsautocomplete")
            .join(version);
        std::fs::create_dir_all(&pinned).unwrap();
        assert_eq!(
            r.dotnet_tool_cache("fsautocomplete", Some(dir.path())),
            Some(pinned)
        );
        assert_eq!(r.dotnet_tool_cache("other-tool", Some(dir.path())), None);
    }
}
