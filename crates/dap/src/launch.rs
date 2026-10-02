//! Launch configuration for a .NET project, as Visual Studio derives it: the project's built program (its output
//! DLL, run with `dotnet`) and the `Properties/launchSettings.json` profile's arguments, environment and working
//! directory.
//!
//! The project file is read as text, not evaluated: `AssemblyName`, `TargetFramework(s)`, `OutputType` and the SDK
//! are taken from the project file itself, and the output is looked for under `bin/<Configuration>/<tfm>/` (then
//! `bin/<Configuration>/`). A property set only in an imported file (`Directory.Build.props`) other than the target
//! framework is not seen; the report lists that gap.

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
    })
}

/// The built program of `project` in `configuration`: the first existing `<AssemblyName>.dll` under
/// `bin/<configuration>/<tfm>/` (the first target framework first), then `bin/<configuration>/`.
pub fn output_dll(project: &ProjectInfo, configuration: &str) -> Result<PathBuf, String> {
    let dir = project.path.parent().unwrap_or(Path::new("."));
    let bin = dir.join("bin").join(configuration);
    let file = format!("{}.dll", project.assembly_name);
    let mut candidates: Vec<PathBuf> = project
        .target_frameworks
        .iter()
        .map(|tf| bin.join(tf).join(&file))
        .collect();
    candidates.push(bin.join(&file));
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
    /// The built DLL.
    pub program: PathBuf,
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
    let program = output_dll(&info, CONFIGURATION)?;
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

    /// The command line of Start Without Debugging: `dotnet <dll> <args>`.
    pub fn without_debugging(&self) -> (String, Vec<String>) {
        let mut args = vec![self.program.to_string_lossy().into_owned()];
        args.extend(self.args.iter().cloned());
        ("dotnet".to_owned(), args)
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
        assert_eq!(
            startup_project(&[lib.clone(), host.clone(), web]),
            Some(host.clone())
        );
        assert_eq!(startup_project(&[lib]), None);

        // Not built yet: the error says to build.
        let err = output_dll(&info, CONFIGURATION).unwrap_err();
        assert!(err.contains("dotnet build"), "{err}");
        let dll = t.path().join("Host/bin/Debug/net10.0/eludite-host.dll");
        write(&dll, "");
        assert_eq!(output_dll(&info, CONFIGURATION).unwrap(), dll);

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
        assert_eq!(v["program"], dll.to_string_lossy().as_ref());
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
    fn command_lines_split_like_visual_studio() {
        assert_eq!(split_command_line(""), Vec::<String>::new());
        assert_eq!(split_command_line("  a  b "), ["a", "b"]);
        assert_eq!(split_command_line(r#"a "b c" d"#), ["a", "b c", "d"]);
        assert_eq!(split_command_line(r#"x\"y "#), ["x\"y"]);
        assert_eq!(split_command_line(r#""""#), [""]);
    }
}
