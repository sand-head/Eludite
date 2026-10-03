//! Launch configuration for a .NET project, as Visual Studio derives it: the project's built program and the
//! `Properties/launchSettings.json` profile's arguments, environment and working directory; and which runtime and
//! debug adapter run it (brief 0022).
//!
//! The project file is read as text, not evaluated: `AssemblyName`, `TargetFramework(s)`, `TargetFrameworkVersion`,
//! `OutputType`, `OutputPath` and the SDK are taken from the project file itself. A property set only in an imported
//! file (`Directory.Build.props`) other than the target framework is not seen; the report lists that gap.
//!
//! - **Target frameworks** ([`classify_framework`]): `netcoreapp*`, `netstandard*` (not runnable) and `netN.M` with
//!   N >= 5 are CoreCLR; `net2*`, `net3*`, `net4*` (SDK-style) and a legacy project's `TargetFrameworkVersion` `v2.0` to
//!   `v4.8.1` are .NET Framework. A multi-targeted project runs its first target framework, or the one Visual Studio's
//!   Debug toolbar selected: `ActiveDebugFramework` in the project's `.user` file (`App.csproj.user`), when the project
//!   lists it (brief 0030, which debugs its `net10.0;net472` corpus under Mono where netcoredbg is missing).
//! - **The built program** ([`output_program`]): CoreCLR's `<AssemblyName>.dll` under `bin/<Configuration>/<tfm>/`
//!   (then `bin/<Configuration>/`); an SDK-style .NET Framework project's `<AssemblyName>.exe` there; a legacy
//!   project's `<AssemblyName>.exe` in its `OutputPath` (default `bin\Debug\`, backslashes normalized).
//! - **The adapter** ([`select_adapter`]): netcoredbg for CoreCLR; `eludite-dbg-mono` under the located Mono for .NET
//!   Framework on Linux and macOS; on Windows .NET Framework needs `eludite-dbg-netfx` (brief 0004), not built yet;
//!   lldb-dap for a Cargo package's native executable ([`FrameworkKind::Native`], brief 0029; its launch configuration
//!   is [`crate::cargo`]'s, made into a [`LaunchConfig`] by [`LaunchConfig::from_cargo`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

/// The configuration F5 runs (Visual Studio's default).
pub const CONFIGURATION: &str = "Debug";

/// What the project file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub path: PathBuf,
    /// The file stem (`Eludite.Host`).
    pub name: String,
    pub assembly_name: String,
    pub target_frameworks: Vec<String>,
    pub output_type: Option<String>,
    pub sdk: Option<String>,
    /// A legacy project's `TargetFrameworkVersion` (`v4.7.2`).
    pub target_framework_version: Option<String>,
    /// A legacy project's `OutputPath` for the Debug configuration, as written (`bin\Debug\`).
    pub output_path: Option<String>,
}

/// Which runtime a target framework runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameworkKind {
    /// .NET 5 and later, .NET Core (and .NET Standard, which is not runnable).
    CoreClr,
    /// .NET Framework 2.0 to 4.8.1.
    NetFramework,
    /// A native executable (a Cargo package's binary or test executable), run directly and debugged with lldb-dap
    /// (brief 0029). No target framework classifies as this.
    Native,
}

/// Classify a target framework moniker (`net472`, `net10.0`, `netcoreapp3.1`) or a legacy `TargetFrameworkVersion`
/// (`v4.7.2`). `None` for anything else (`uap10.0`, `xamarinios10`).
pub fn classify_framework(tfm: &str) -> Option<FrameworkKind> {
    let t = tfm.trim().to_ascii_lowercase();
    if let Some(v) = t.strip_prefix('v') {
        // A legacy TargetFrameworkVersion: v2.0 to v4.8.1.
        let major = v.split('.').next()?.parse::<u32>().ok()?;
        return (2..=4)
            .contains(&major)
            .then_some(FrameworkKind::NetFramework);
    }
    if t.starts_with("netcoreapp") || t.starts_with("netstandard") {
        return Some(FrameworkKind::CoreClr);
    }
    let rest = t.strip_prefix("net")?;
    // `net5.0`, `net10.0-windows`: a version with a dot is .NET 5 or later; `net472`, `net48`, `net35`, `net20` are
    // .NET Framework.
    let version = rest.split('-').next().unwrap_or_default();
    if version.contains('.') {
        let major = version.split('.').next()?.parse::<u32>().ok()?;
        return (major >= 5).then_some(FrameworkKind::CoreClr);
    }
    let first = version.chars().next()?;
    (version.chars().all(|c| c.is_ascii_digit()) && matches!(first, '2' | '3' | '4'))
        .then_some(FrameworkKind::NetFramework)
}

impl ProjectInfo {
    /// A legacy (non-SDK) project: no `Sdk` attribute on `<Project>`.
    pub fn is_legacy(&self) -> bool {
        self.sdk.is_none()
    }

    /// The runtime the project's program runs on: its first target framework's, or a legacy project's
    /// `TargetFrameworkVersion`. A project without either is taken as CoreCLR.
    pub fn framework_kind(&self) -> FrameworkKind {
        let tfm = if self.is_legacy() {
            self.target_framework_version
                .as_deref()
                .or(self.target_frameworks.first().map(String::as_str))
        } else {
            self.target_frameworks.first().map(String::as_str)
        };
        tfm.and_then(classify_framework)
            .unwrap_or(FrameworkKind::CoreClr)
    }
}

impl ProjectInfo {
    /// An application (Visual Studio's startup project candidates): `OutputType` Exe or WinExe, or a web project.
    /// Test projects are not, even when their test framework makes them executables.
    pub fn is_executable(&self) -> bool {
        let exe = self
            .output_type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("exe") || t.eq_ignore_ascii_case("winexe"));
        let web = self
            .sdk
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("Microsoft.NET.Sdk.Web"));
        exe || web
    }
}

/// The text of the first `<tag>...</tag>` in `xml`, trimmed (no attributes on the tag).
fn element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    let v = xml[start..end].trim();
    (!v.is_empty() && !v.contains("$(")).then(|| v.to_owned())
}

/// The `OutputPath` of configuration `configuration` in a legacy project: the one in a `<PropertyGroup>` whose
/// condition names the configuration, else the first unconditional one.
fn configuration_output_path(xml: &str, configuration: &str) -> Option<String> {
    let mut fallback = None;
    let mut rest = xml;
    while let Some(start) = rest.find("<PropertyGroup") {
        let after = &rest[start..];
        let Some(head_end) = after.find('>') else {
            break;
        };
        let head = &after[..head_end];
        let Some(end) = after.find("</PropertyGroup>") else {
            break;
        };
        let body = &after[head_end..end];
        if let Some(path) = element(body, "OutputPath") {
            if !head.contains("Condition") {
                fallback.get_or_insert(path);
            } else if head.contains(&format!("'{configuration}|"))
                || head.contains(&format!("'{configuration}'"))
            {
                return Some(path);
            }
        }
        rest = &after[end..];
    }
    fallback
}

/// The value of attribute `name` on the root `<Project ...>` element.
fn project_attribute(xml: &str, name: &str) -> Option<String> {
    let start = xml.find("<Project")?;
    let end = xml[start..].find('>')? + start;
    let tag = &xml[start..end];
    let key = format!("{name}=\"");
    let s = tag.find(&key)? + key.len();
    let e = tag[s..].find('"')? + s;
    Some(tag[s..e].to_owned())
}

/// Read `path` (a `.csproj`, `.vbproj` or `.fsproj`), with the target framework from the nearest
/// `Directory.Build.props` when the project file has none.
pub fn read_project(path: &Path) -> Result<ProjectInfo, String> {
    let xml = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut target_frameworks: Vec<String> = element(&xml, "TargetFrameworks")
        .or_else(|| element(&xml, "TargetFramework"))
        .map(|v| {
            v.split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    // Visual Studio's choice of the framework to debug (the Debug toolbar's framework list writes it to the `.user`
    // file beside the project): that framework first.
    let user = PathBuf::from(format!("{}.user", path.display()));
    if let Some(active) = std::fs::read_to_string(user)
        .ok()
        .and_then(|xml| element(&xml, "ActiveDebugFramework"))
        && let Some(ix) = target_frameworks
            .iter()
            .position(|t| t.eq_ignore_ascii_case(&active))
    {
        let tfm = target_frameworks.remove(ix);
        target_frameworks.insert(0, tfm);
    }
    if target_frameworks.is_empty() {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if let Ok(props) = std::fs::read_to_string(d.join("Directory.Build.props")) {
                if let Some(tf) = element(&props, "TargetFramework") {
                    target_frameworks.push(tf);
                }
                break;
            }
            dir = d.parent();
        }
    }
    Ok(ProjectInfo {
        path: path.to_path_buf(),
        assembly_name: element(&xml, "AssemblyName").unwrap_or_else(|| name.clone()),
        name,
        target_frameworks,
        output_type: element(&xml, "OutputType"),
        sdk: project_attribute(&xml, "Sdk"),
        target_framework_version: element(&xml, "TargetFrameworkVersion"),
        output_path: configuration_output_path(&xml, CONFIGURATION),
    })
}

/// The built program of `project` in `configuration` (the first candidate that exists):
///
/// - CoreCLR: `<AssemblyName>.dll` under `bin/<configuration>/<tfm>/` (the first target framework first), then
///   `bin/<configuration>/`;
/// - an SDK-style .NET Framework project: `<AssemblyName>.exe` in the same places;
/// - a legacy project: `<AssemblyName>.exe` in its `OutputPath` (default `bin\<configuration>\`).
///
/// The error names the first candidate and says to build.
pub fn output_program(project: &ProjectInfo, configuration: &str) -> Result<PathBuf, String> {
    let dir = project.path.parent().unwrap_or(Path::new("."));
    let kind = project.framework_kind();
    let extension = match kind {
        FrameworkKind::CoreClr => "dll",
        FrameworkKind::NetFramework => "exe",
        FrameworkKind::Native => {
            return Err(format!(
                "{} is not a .NET project: a Cargo package's executable comes from its Cargo build",
                project.name
            ));
        }
    };
    let file = format!("{}.{extension}", project.assembly_name);
    let candidates: Vec<PathBuf> = if project.is_legacy() && kind == FrameworkKind::NetFramework {
        let out = project
            .output_path
            .clone()
            .unwrap_or_else(|| format!("bin\\{configuration}\\"))
            .replace('\\', "/");
        let out = out.trim_end_matches('/');
        vec![dir.join(out).join(&file)]
    } else {
        let bin = dir.join("bin").join(configuration);
        let mut c: Vec<PathBuf> = project
            .target_frameworks
            .iter()
            .map(|tf| bin.join(tf).join(&file))
            .collect();
        c.push(bin.join(&file));
        c
    };
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "{} is not built: {} does not exist. Build the project first (dotnet build {}).",
                project.name,
                candidates[0].display(),
                project.path.display()
            )
        })
}

/// The OS the shell runs on, as adapter selection sees it (a parameter, so tests choose it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
}

/// The debug adapter for a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    /// netcoredbg (`--interpreter=vscode`), `adapterID` `coreclr`.
    Netcoredbg,
    /// `mono eludite-dbg-mono.exe`, `adapterID` `mono`.
    Mono,
    /// lldb-dap (or CodeLLDB), `adapterID` `lldb` (brief 0029).
    Lldb,
}

/// Why F5 on a .NET Framework project fails on Windows.
pub const NETFX_ON_WINDOWS: &str = "Debugging .NET Framework on Windows needs eludite-dbg-netfx (brief 0004), which is not \
                                    built yet; on Linux and macOS Eludite debugs it under Mono";

/// The adapter that debugs a `kind` program on `platform`.
pub fn select_adapter(kind: FrameworkKind, platform: Platform) -> Result<AdapterKind, String> {
    match (kind, platform) {
        (FrameworkKind::CoreClr, _) => Ok(AdapterKind::Netcoredbg),
        (FrameworkKind::NetFramework, Platform::Windows) => Err(NETFX_ON_WINDOWS.to_owned()),
        (FrameworkKind::NetFramework, _) => Ok(AdapterKind::Mono),
        (FrameworkKind::Native, _) => Ok(AdapterKind::Lldb),
    }
}

/// What runs a `kind` program on `platform`, as `eludite.debug.state`'s `session.runtime` names it.
pub fn runtime_name(kind: FrameworkKind, platform: Platform) -> &'static str {
    match (kind, platform) {
        (FrameworkKind::CoreClr, _) => "coreclr",
        (FrameworkKind::NetFramework, Platform::Windows) => "netfx",
        (FrameworkKind::NetFramework, _) => "mono",
        (FrameworkKind::Native, _) => "native",
    }
}

/// One profile of `launchSettings.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchProfile {
    #[serde(skip)]
    pub name: String,
    pub command_name: String,
    pub command_line_args: Option<String>,
    pub environment_variables: BTreeMap<String, String>,
    pub working_directory: Option<String>,
    pub application_url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LaunchSettings {
    profiles: serde_json::Map<String, Value>,
}

/// `<project dir>/Properties/launchSettings.json`'s profiles, in file order. A missing file has none.
pub fn read_launch_settings(project_dir: &Path) -> Result<Vec<LaunchProfile>, String> {
    let path = project_dir.join("Properties").join("launchSettings.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let settings: LaunchSettings =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(settings
        .profiles
        .into_iter()
        .filter_map(|(name, v)| {
            let mut p: LaunchProfile = serde_json::from_value(v).ok()?;
            p.name = name;
            Some(p)
        })
        .collect())
}

/// Split a command line into arguments: whitespace separates, double quotes group, `\"` is a quote.
pub fn split_command_line(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_arg = false;
    let mut quoted = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
                in_arg = true;
            }
            '"' => {
                quoted = !quoted;
                in_arg = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_arg {
                    args.push(std::mem::take(&mut cur));
                    in_arg = false;
                }
            }
            c => {
                cur.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(cur);
    }
    args
}

/// Everything needed to run a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    pub project: PathBuf,
    /// The built program: the DLL (CoreCLR) or the .exe (.NET Framework).
    pub program: PathBuf,
    pub kind: FrameworkKind,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Added to the inherited environment.
    pub env: BTreeMap<String, String>,
    pub profile: Option<String>,
}

/// The launch configuration of `project` with launch profile `profile` (default: the first profile whose
/// `commandName` is `Project`; none is fine).
pub fn launch_config(project: &Path, profile: Option<&str>) -> Result<LaunchConfig, String> {
    let info = read_project(project)?;
    let program = output_program(&info, CONFIGURATION)?;
    let dir = project.parent().unwrap_or(Path::new(".")).to_path_buf();
    let profiles = read_launch_settings(&dir)?;
    let chosen = match profile {
        Some(name) => Some(
            profiles
                .iter()
                .find(|p| p.name == name)
                .ok_or_else(|| {
                    format!(
                        "no launch profile `{name}` in {}",
                        dir.join("Properties").join("launchSettings.json").display()
                    )
                })?
                .clone(),
        ),
        None => profiles.into_iter().find(|p| p.command_name == "Project"),
    };
    let mut env = BTreeMap::new();
    let mut args = Vec::new();
    // Visual Studio runs an SDK project in its project folder unless the profile says otherwise.
    let mut cwd = dir.clone();
    if let Some(p) = &chosen {
        env.extend(p.environment_variables.clone());
        if let Some(url) = &p.application_url
            && !env.contains_key("ASPNETCORE_URLS")
        {
            env.insert("ASPNETCORE_URLS".into(), url.clone());
        }
        if let Some(a) = &p.command_line_args {
            args = split_command_line(a);
        }
        if let Some(w) = &p.working_directory {
            let w = w.replace("$(ProjectDir)", &format!("{}/", dir.display()));
            cwd = dir.join(w);
        }
    }
    Ok(LaunchConfig {
        project: project.to_path_buf(),
        program,
        kind: info.framework_kind(),
        args,
        cwd,
        env,
        profile: chosen.map(|p| p.name),
    })
}

impl LaunchConfig {
    /// netcoredbg's `launch` arguments: it runs `dotnet` on the DLL itself.
    pub fn netcoredbg_arguments(&self) -> Value {
        json!({
            "name": format!("{} (Eludite)", self.project.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default()),
            "type": "coreclr",
            "request": "launch",
            "program": self.program.to_string_lossy(),
            "args": self.args,
            "cwd": self.cwd.to_string_lossy(),
            "env": self.env,
            "stopAtEntry": false,
            "justMyCode": true,
        })
    }

    /// eludite-dbg-mono's `launch` arguments (protocol/schemas/dap-mono.md): the .exe, run by `mono` (the located one).
    pub fn mono_arguments(&self, mono: &Path) -> Value {
        json!({
            "name": format!("{} (Eludite)", self.project.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default()),
            "type": "mono",
            "request": "launch",
            "program": self.program.to_string_lossy(),
            "args": self.args,
            "cwd": self.cwd.to_string_lossy(),
            "env": self.env,
            "runtimeExecutable": mono.to_string_lossy(),
            "runtimeArgs": [],
            "stopAtEntry": false,
            "justMyCode": true,
        })
    }

    /// A Cargo package's launch configuration as the shell's session sees it: the package's `Cargo.toml` as the
    /// project, the executable as the program, [`FrameworkKind::Native`], no launch profile.
    pub fn from_cargo(c: &crate::cargo::CargoLaunch) -> Self {
        Self {
            project: c.manifest.clone(),
            program: c.program.clone(),
            kind: FrameworkKind::Native,
            args: c.args.clone(),
            cwd: c.cwd.clone(),
            env: c.env.clone(),
            profile: None,
        }
    }

    /// The command line of Start Without Debugging for a CoreCLR program: `dotnet <dll> <args>`.
    pub fn without_debugging(&self) -> (String, Vec<String>) {
        let mut args = vec![self.program.to_string_lossy().into_owned()];
        args.extend(self.args.iter().cloned());
        ("dotnet".to_owned(), args)
    }

    /// The command line of Start Without Debugging on `platform`: `<dotnet> <dll> <args>` for CoreCLR; for .NET
    /// Framework `<mono> <exe> <args>` off Windows and `<exe> <args>` on Windows; a native executable runs as
    /// `<exe> <args>` everywhere. `mono` is needed off Windows only.
    pub fn run_command(
        &self,
        platform: Platform,
        dotnet: &str,
        mono: Option<&Path>,
    ) -> Result<(String, Vec<String>), String> {
        let program = self.program.to_string_lossy().into_owned();
        let with = |first: Option<String>| {
            let mut a: Vec<String> = first.into_iter().collect();
            a.extend(self.args.iter().cloned());
            a
        };
        Ok(match (self.kind, platform) {
            (FrameworkKind::CoreClr, _) => (dotnet.to_owned(), with(Some(program))),
            (FrameworkKind::NetFramework, Platform::Windows) | (FrameworkKind::Native, _) => {
                (program, with(None))
            }
            (FrameworkKind::NetFramework, _) => {
                let mono = mono.ok_or("running a .NET Framework program off Windows needs Mono")?;
                (mono.to_string_lossy().into_owned(), with(Some(program)))
            }
        })
    }
}

/// The startup project among `projects` (in solution order): the first executable one.
pub fn startup_project(projects: &[PathBuf]) -> Option<PathBuf> {
    projects
        .iter()
        .find(|p| read_project(p).is_ok_and(|i| i.is_executable()))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn reads_projects_and_finds_the_startup_project() {
        let t = tempfile::tempdir().unwrap();
        write(
            &t.path().join("Directory.Build.props"),
            "<Project><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>",
        );
        let lib = t.path().join("Lib/Lib.csproj");
        write(&lib, "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>");
        let host = t.path().join("Host/Eludite.Host.csproj");
        write(
            &host,
            "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n    <AssemblyName>eludite-host</AssemblyName>\n  </PropertyGroup>\n</Project>",
        );
        let web = t.path().join("Web/Web.csproj");
        write(
            &web,
            "<Project Sdk=\"Microsoft.NET.Sdk.Web\"><PropertyGroup><TargetFrameworks>net8.0;net10.0</TargetFrameworks></PropertyGroup></Project>",
        );
        let info = read_project(&host).unwrap();
        assert_eq!(info.assembly_name, "eludite-host");
        assert_eq!(info.target_frameworks, ["net10.0"]);
        assert!(info.is_executable());
        assert!(!read_project(&lib).unwrap().is_executable());
        let w = read_project(&web).unwrap();
        assert!(w.is_executable());
        assert_eq!(w.target_frameworks, ["net8.0", "net10.0"]);
        // Visual Studio's ActiveDebugFramework (the .user file) puts its framework first; one the project does not
        // list is ignored.
        write(
            &t.path().join("Web/Web.csproj.user"),
            "<Project><PropertyGroup><ActiveDebugFramework>net10.0</ActiveDebugFramework></PropertyGroup></Project>",
        );
        assert_eq!(
            read_project(&t.path().join("Web/Web.csproj"))
                .unwrap()
                .target_frameworks,
            ["net10.0", "net8.0"]
        );
        write(
            &t.path().join("Web/Web.csproj.user"),
            "<Project><PropertyGroup><ActiveDebugFramework>net472</ActiveDebugFramework></PropertyGroup></Project>",
        );
        assert_eq!(
            read_project(&t.path().join("Web/Web.csproj"))
                .unwrap()
                .target_frameworks,
            ["net8.0", "net10.0"]
        );
        assert_eq!(
            startup_project(&[lib.clone(), host.clone(), web]),
            Some(host.clone())
        );
        assert_eq!(startup_project(&[lib]), None);

        // Not built yet: the error says to build.
        let err = output_program(&info, CONFIGURATION).unwrap_err();
        assert!(err.contains("dotnet build"), "{err}");
        let dll = t.path().join("Host/bin/Debug/net10.0/eludite-host.dll");
        write(&dll, "");
        assert_eq!(output_program(&info, CONFIGURATION).unwrap(), dll);

        // No launchSettings.json: no arguments, the project folder, no environment.
        let c = launch_config(&host, None).unwrap();
        assert_eq!(c.program, dll);
        assert!(c.args.is_empty());
        assert_eq!(c.cwd, t.path().join("Host"));
        assert_eq!(c.profile, None);

        write(
            &t.path().join("Host/Properties/launchSettings.json"),
            r#"{"profiles": {
                 "Docker": {"commandName": "Docker"},
                 "Eludite.Host": {"commandName": "Project", "commandLineArgs": "--stdio --name \"two words\"",
                                  "environmentVariables": {"ELUDITE_LEGACY": "0"},
                                  "applicationUrl": "http://localhost:5000"},
                 "Other": {"commandName": "Project", "workingDirectory": "data"}
               }}"#,
        );
        let c = launch_config(&host, None).unwrap();
        assert_eq!(c.profile.as_deref(), Some("Eludite.Host"));
        assert_eq!(c.args, ["--stdio", "--name", "two words"]);
        assert_eq!(c.env.get("ELUDITE_LEGACY").map(String::as_str), Some("0"));
        assert_eq!(
            c.env.get("ASPNETCORE_URLS").map(String::as_str),
            Some("http://localhost:5000")
        );
        let v = c.netcoredbg_arguments();
        // Compare as paths: the config uses native separators, the fixture was joined with `/`.
        assert_eq!(
            std::path::PathBuf::from(v["program"].as_str().unwrap()),
            dll.components().collect::<std::path::PathBuf>()
        );
        assert_eq!(v["args"][2], "two words");
        assert_eq!(v["env"]["ELUDITE_LEGACY"], "0");
        assert_eq!(v["type"], "coreclr");
        let (cmd, args) = c.without_debugging();
        assert_eq!(cmd, "dotnet");
        assert_eq!(args[0], dll.to_string_lossy());
        assert_eq!(args[1], "--stdio");
        let other = launch_config(&host, Some("Other")).unwrap();
        assert_eq!(other.cwd, t.path().join("Host").join("data"));
        assert!(
            launch_config(&host, Some("Nope"))
                .unwrap_err()
                .contains("Nope")
        );
    }

    #[test]
    fn target_frameworks_classify_as_coreclr_or_net_framework() {
        use FrameworkKind::*;
        for (tfm, kind) in [
            ("net10.0", Some(CoreClr)),
            ("net5.0", Some(CoreClr)),
            ("net8.0-windows", Some(CoreClr)),
            ("netcoreapp3.1", Some(CoreClr)),
            ("netstandard2.0", Some(CoreClr)),
            ("net472", Some(NetFramework)),
            ("net48", Some(NetFramework)),
            ("net481", Some(NetFramework)),
            ("net35", Some(NetFramework)),
            ("net20", Some(NetFramework)),
            ("v4.7.2", Some(NetFramework)),
            ("v4.8.1", Some(NetFramework)),
            ("v2.0", Some(NetFramework)),
            ("v5.0", None),
            ("net1.1", None),
            ("uap10.0", None),
            ("", None),
        ] {
            assert_eq!(classify_framework(tfm), kind, "{tfm}");
        }
    }

    #[test]
    fn net_framework_programs_are_the_exe_of_legacy_and_sdk_style_projects() {
        let t = tempfile::tempdir().unwrap();
        // An SDK-style net472 project builds to bin/Debug/net472/<AssemblyName>.exe.
        let sdk = t.path().join("Sdk/Tool.csproj");
        write(
            &sdk,
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net472</TargetFramework><AssemblyName>tool</AssemblyName></PropertyGroup></Project>",
        );
        let info = read_project(&sdk).unwrap();
        assert_eq!(info.framework_kind(), FrameworkKind::NetFramework);
        assert!(!info.is_legacy());
        let err = output_program(&info, CONFIGURATION).unwrap_err();
        assert!(
            err.contains(&format!("net472{}tool.exe", std::path::MAIN_SEPARATOR)),
            "{err}"
        );
        let exe = t.path().join("Sdk/bin/Debug/net472/tool.exe");
        write(&exe, "");
        // A DLL beside it is not the program.
        write(&t.path().join("Sdk/bin/Debug/net472/tool.dll"), "");
        assert_eq!(output_program(&info, CONFIGURATION).unwrap(), exe);

        // A legacy project builds to its Debug OutputPath (backslashes normalized) as <AssemblyName>.exe.
        let legacy = t.path().join("Legacy/Legacy.csproj");
        write(
            &legacy,
            r#"<?xml version="1.0" encoding="utf-8"?>
<Project ToolsVersion="15.0" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <AssemblyName>Legacy.App</AssemblyName>
    <TargetFrameworkVersion>v4.7.2</TargetFrameworkVersion>
  </PropertyGroup>
  <PropertyGroup Condition=" '$(Configuration)|$(Platform)' == 'Release|AnyCPU' ">
    <OutputPath>bin\Release\</OutputPath>
  </PropertyGroup>
  <PropertyGroup Condition=" '$(Configuration)|$(Platform)' == 'Debug|AnyCPU' ">
    <OutputPath>..\out\debug\</OutputPath>
  </PropertyGroup>
</Project>"#,
        );
        let info = read_project(&legacy).unwrap();
        assert!(info.is_legacy());
        assert!(info.is_executable());
        assert_eq!(info.target_framework_version.as_deref(), Some("v4.7.2"));
        assert_eq!(info.framework_kind(), FrameworkKind::NetFramework);
        let err = output_program(&info, CONFIGURATION).unwrap_err();
        assert!(
            err.contains("Legacy.App.exe") && err.contains("out"),
            "{err}"
        );
        let exe = t.path().join("Legacy/../out/debug/Legacy.App.exe");
        write(&exe, "");
        assert_eq!(output_program(&info, CONFIGURATION).unwrap(), exe);
        // Without an OutputPath: bin\Debug\.
        let plain = t.path().join("Plain/Plain.csproj");
        write(
            &plain,
            "<Project><PropertyGroup><OutputType>Exe</OutputType><TargetFrameworkVersion>v4.8</TargetFrameworkVersion></PropertyGroup></Project>",
        );
        let exe = t.path().join("Plain/bin/Debug/Plain.exe");
        write(&exe, "");
        assert_eq!(
            output_program(&read_project(&plain).unwrap(), CONFIGURATION).unwrap(),
            exe
        );

        // The launch configuration carries the kind; Mono's launch arguments and the run commands follow it.
        let c = launch_config(&sdk, None).unwrap();
        assert_eq!(c.kind, FrameworkKind::NetFramework);
        let mono = Path::new("/opt/mono/bin/mono");
        let v = c.mono_arguments(mono);
        assert_eq!(v["type"], "mono");
        assert!(v["program"].as_str().unwrap().ends_with("tool.exe"));
        assert_eq!(v["runtimeExecutable"], "/opt/mono/bin/mono");
        assert_eq!(v["stopAtEntry"], false);
        assert_eq!(v["justMyCode"], true);
        let (cmd, args) = c
            .run_command(Platform::Linux, "dotnet", Some(mono))
            .unwrap();
        assert_eq!(cmd, "/opt/mono/bin/mono");
        assert!(args[0].ends_with("tool.exe"));
        let (cmd, args) = c.run_command(Platform::Windows, "dotnet", None).unwrap();
        assert!(cmd.ends_with("tool.exe") && args.is_empty());
        assert!(c.run_command(Platform::MacOs, "dotnet", None).is_err());
    }

    #[test]
    fn the_adapter_follows_the_framework_and_the_platform() {
        use FrameworkKind::*;
        use Platform::*;
        for p in [Linux, MacOs, Windows] {
            assert_eq!(select_adapter(CoreClr, p), Ok(AdapterKind::Netcoredbg));
            assert_eq!(runtime_name(CoreClr, p), "coreclr");
        }
        assert_eq!(select_adapter(NetFramework, Linux), Ok(AdapterKind::Mono));
        assert_eq!(select_adapter(NetFramework, MacOs), Ok(AdapterKind::Mono));
        let err = select_adapter(NetFramework, Windows).unwrap_err();
        assert_eq!(
            err,
            "Debugging .NET Framework on Windows needs eludite-dbg-netfx (brief 0004), which is not built yet; on Linux \
             and macOS Eludite debugs it under Mono"
        );
        assert_eq!(runtime_name(NetFramework, Linux), "mono");
        assert_eq!(runtime_name(NetFramework, Windows), "netfx");
        // A Cargo package's executable: lldb-dap and `native` everywhere (brief 0029).
        for p in [Linux, MacOs, Windows] {
            assert_eq!(select_adapter(Native, p), Ok(AdapterKind::Lldb));
            assert_eq!(runtime_name(Native, p), "native");
        }
        let cargo = crate::cargo::CargoLaunch {
            manifest: "/w/app/Cargo.toml".into(),
            package: "app".into(),
            target: "app".into(),
            test: false,
            program: "/w/target/debug/app".into(),
            args: vec!["--fast".into()],
            env: BTreeMap::new(),
            cwd: "/w".into(),
        };
        let c = LaunchConfig::from_cargo(&cargo);
        assert_eq!((c.kind, c.profile.clone()), (Native, None));
        assert_eq!(c.project, Path::new("/w/app/Cargo.toml"));
        for p in [Linux, MacOs, Windows] {
            assert_eq!(
                c.run_command(p, "dotnet", None).unwrap(),
                ("/w/target/debug/app".to_owned(), vec!["--fast".to_owned()])
            );
        }
    }

    #[test]
    fn command_lines_split_like_visual_studio() {
        assert_eq!(split_command_line(""), Vec::<String>::new());
        assert_eq!(split_command_line("  a  b "), ["a", "b"]);
        assert_eq!(split_command_line(r#"a "b c" d"#), ["a", "b c", "d"]);
        assert_eq!(split_command_line(r#"x\"y "#), ["x\"y"]);
        assert_eq!(split_command_line(r#""""#), [""]);
    }
}
