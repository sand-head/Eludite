//! Publish to a folder (brief 0074): Build > Publish Selection, the Workspace window's Publish... and
//! `eludite.build.publish`.
//!
//! A project's publish profiles are `Properties/PublishProfiles/*.pubxml` (MSBuild files Visual Studio writes).
//! [`PublishProfile`] reads the folder target's properties from one and [`PublishProfile::apply`] writes them back by
//! editing each element's text in place (a missing element is added to the first `PropertyGroup` with its siblings'
//! indentation), so comments, unknown properties and line endings stay byte for byte. A project without a profile
//! gets `FolderProfile.pubxml` in Visual Studio's template shape ([`default_profile_text`]). The publish itself is
//! the host's `publish` build target (`dotnet publish <project> -p:PublishProfile=<pubxml>`), started through the
//! build path, so it streams into the Output window's Build pane and fills the Error List like a build.

use std::path::{Path, PathBuf};

use eludite_commands::CommandError;
use eludite_commands::build::{BuildCommandOutput, BuildKind, BuildSystemChoice};
use gpui::{Context, Window};

use super::Shell;

/// The folder of a project's publish profiles.
pub fn profiles_dir(project: &Path) -> PathBuf {
    project
        .parent()
        .unwrap_or(Path::new("."))
        .join("Properties")
        .join("PublishProfiles")
}

/// A project's publish profiles, by file name.
pub fn profiles(project: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(profiles_dir(project))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pubxml"))
        })
        .collect();
    out.sort();
    out
}

/// What the Publish dialog edits of a folder profile (Visual Studio's Folder target settings).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishProfile {
    /// `PublishProtocol` (`FileSystem` for a folder).
    pub protocol: Option<String>,
    pub configuration: Option<String>,
    pub target_framework: Option<String>,
    /// `RuntimeIdentifier` (`linux-x64`); `None`: portable.
    pub runtime: Option<String>,
    /// `SelfContained`: the deployment mode (framework-dependent when false).
    pub self_contained: Option<bool>,
    pub single_file: Option<bool>,
    pub ready_to_run: Option<bool>,
    pub trimmed: Option<bool>,
    /// `PublishDir` (else `PublishUrl`), as written.
    pub publish_dir: Option<String>,
}

/// The byte range of the text of the first `<name>...</name>` (no attributes) in `xml`.
fn element_range(xml: &str, name: &str) -> Option<(usize, usize)> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some((start, end))
}

fn element(xml: &str, name: &str) -> Option<String> {
    element_range(xml, name)
        .map(|(s, e)| xml[s..e].trim().to_owned())
        .or_else(|| {
            // `<Name />`: present and empty.
            xml.contains(&format!("<{name} />")).then(String::new)
        })
}

fn as_bool(v: Option<String>) -> Option<bool> {
    v.map(|v| v.trim().eq_ignore_ascii_case("true"))
}

fn xml_escape(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_unescape(v: &str) -> String {
    v.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `xml` with element `name`'s text set to `value` (added to the first `PropertyGroup` when missing), or the element
/// removed with its line (`None`). Everything else is kept.
pub fn set_element(xml: &str, name: &str, value: Option<&str>) -> String {
    let nl = if xml.contains("\r\n") { "\r\n" } else { "\n" };
    match (element_range(xml, name), value) {
        (Some((s, e)), Some(v)) => format!("{}{}{}", &xml[..s], xml_escape(v), &xml[e..]),
        (Some((s, e)), None) => {
            let open = s - format!("<{name}>").len();
            let close = e + format!("</{name}>").len();
            // The whole line when the element is alone on it.
            let line_start = xml[..open].rfind('\n').map_or(0, |i| i + 1);
            let line_end = xml[close..].find('\n').map_or(xml.len(), |i| close + i + 1);
            if xml[line_start..open].trim().is_empty() && xml[close..line_end].trim().is_empty() {
                format!("{}{}", &xml[..line_start], &xml[line_end..])
            } else {
                format!("{}{}", &xml[..open], &xml[close..])
            }
        }
        (None, None) => xml.to_owned(),
        (None, Some(v)) => {
            let Some(group_end) = xml.find("</PropertyGroup>") else {
                // No property group: one before `</Project>`.
                let at = xml.rfind("</Project>").unwrap_or(xml.len());
                return format!(
                    "{}  <PropertyGroup>{nl}    <{name}>{}</{name}>{nl}  </PropertyGroup>{nl}{}",
                    &xml[..at],
                    xml_escape(v),
                    &xml[at..]
                );
            };
            // The indentation of the group's last element, else the group's own plus two spaces.
            let line_start = xml[..group_end].rfind('\n').map_or(0, |i| i + 1);
            let group_indent = &xml[line_start..group_end];
            let prev = xml[..line_start.saturating_sub(1)]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            let prev_line = &xml[prev..line_start];
            let indent: String = if prev_line.trim_start().starts_with('<')
                && !prev_line.contains("<PropertyGroup")
            {
                prev_line
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect()
            } else {
                format!("{group_indent}  ")
            };
            format!(
                "{}{indent}<{name}>{}</{name}>{nl}{}",
                &xml[..line_start],
                xml_escape(v),
                &xml[line_start..]
            )
        }
    }
}

impl PublishProfile {
    /// The folder target's properties of `xml`.
    pub fn parse(xml: &str) -> Self {
        let get = |n: &str| element(xml, n).map(|v| xml_unescape(&v));
        Self {
            protocol: get("PublishProtocol"),
            configuration: get("Configuration"),
            target_framework: get("TargetFramework"),
            runtime: get("RuntimeIdentifier").filter(|r| !r.is_empty()),
            self_contained: as_bool(get("SelfContained")),
            single_file: as_bool(get("PublishSingleFile")),
            ready_to_run: as_bool(get("PublishReadyToRun")),
            trimmed: as_bool(get("PublishTrimmed")),
            publish_dir: get("PublishDir").or_else(|| get("PublishUrl")),
        }
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        std::fs::read_to_string(path)
            .map(|t| Self::parse(&t))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// A folder profile: `PublishProtocol` `FileSystem` (or none, as `dotnet publish` treats it).
    pub fn is_folder(&self) -> bool {
        self.protocol
            .as_deref()
            .is_none_or(|p| p.eq_ignore_ascii_case("FileSystem"))
    }

    /// The configuration it publishes (`Release` when it names none, Visual Studio's default).
    pub fn configuration(&self) -> String {
        self.configuration
            .clone()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| "Release".into())
    }

    /// Its publish folder, absolute (relative paths are the project folder's; backslashes as written work on every
    /// platform `dotnet publish` runs on, and are shown with the platform's separator).
    pub fn publish_dir_in(&self, project: &Path, tfm: Option<&str>) -> PathBuf {
        let dir = project.parent().unwrap_or(Path::new("."));
        let written = self.publish_dir.clone().unwrap_or_else(|| {
            default_publish_dir(
                &self.configuration(),
                self.target_framework
                    .as_deref()
                    .or(tfm)
                    .unwrap_or("net10.0"),
            )
        });
        let written = written.replace('\\', "/");
        let p = Path::new(written.trim_end_matches('/'));
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            dir.join(p)
        }
    }

    /// `xml` with this profile's properties written: changed elements edited in place, missing ones added, the
    /// optional ones (`RuntimeIdentifier`, `PublishSingleFile`, `PublishReadyToRun`, `PublishTrimmed`) removed when
    /// `None`. `PublishUrl` is rewritten when the file uses it instead of `PublishDir`.
    pub fn apply(&self, xml: &str) -> String {
        let b = |v: Option<bool>| v.map(|b| if b { "true" } else { "false" }.to_owned());
        let dir_name =
            if element(xml, "PublishDir").is_none() && element(xml, "PublishUrl").is_some() {
                "PublishUrl"
            } else {
                "PublishDir"
            };
        let values: [(&str, Option<String>); 9] = [
            ("Configuration", self.configuration.clone()),
            (dir_name, self.publish_dir.clone()),
            ("PublishProtocol", self.protocol.clone()),
            ("TargetFramework", self.target_framework.clone()),
            ("RuntimeIdentifier", self.runtime.clone()),
            ("SelfContained", b(self.self_contained)),
            ("PublishSingleFile", b(self.single_file)),
            ("PublishReadyToRun", b(self.ready_to_run)),
            ("PublishTrimmed", b(self.trimmed)),
        ];
        let mut out = xml.to_owned();
        for (name, value) in values {
            let current = element(&out, name).map(|v| xml_unescape(&v));
            if current.as_deref() == value.as_deref() {
                continue;
            }
            out = set_element(&out, name, value.as_deref());
        }
        out
    }
}

/// Visual Studio's default publish folder: `bin\Release\<tfm>\publish\` (written with `/` off Windows).
pub fn default_publish_dir(configuration: &str, tfm: &str) -> String {
    let sep = if cfg!(windows) { "\\" } else { "/" };
    format!("bin{sep}{configuration}{sep}{tfm}{sep}publish{sep}")
}

/// A new folder profile in Visual Studio's template shape: Release, `tfm`, framework-dependent.
pub fn default_profile_text(tfm: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!--\nhttps://go.microsoft.com/fwlink/?LinkID=208121.\n-->\n\
         <Project>\n  <PropertyGroup>\n    <Configuration>Release</Configuration>\n    <PublishDir>{}</PublishDir>\n    \
         <PublishProtocol>FileSystem</PublishProtocol>\n    <_TargetId>Folder</_TargetId>\n    \
         <TargetFramework>{tfm}</TargetFramework>\n    <SelfContained>false</SelfContained>\n  </PropertyGroup>\n</Project>\n",
        default_publish_dir("Release", tfm)
    )
}

/// The profile a publish uses: `profile` as a name (`FolderProfile`) or a `.pubxml` path, else the project's first
/// profile, else a new `FolderProfile.pubxml` (written; `tfm` is its framework). Returns its path and whether it was
/// created.
pub fn resolve_profile(
    project: &Path,
    profile: Option<&str>,
    tfm: &str,
) -> Result<(PathBuf, bool), String> {
    let existing = profiles(project);
    if let Some(p) = profile {
        let as_path = Path::new(p);
        let candidate = if as_path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pubxml"))
        {
            if as_path.is_absolute() {
                as_path.to_path_buf()
            } else {
                project.parent().unwrap_or(Path::new(".")).join(as_path)
            }
        } else {
            profiles_dir(project).join(format!("{p}.pubxml"))
        };
        return if candidate.is_file() {
            Ok((candidate, false))
        } else {
            Err(format!(
                "no publish profile `{p}`: the project's profiles are {}",
                if existing.is_empty() {
                    "none".to_owned()
                } else {
                    existing
                        .iter()
                        .filter_map(|e| e.file_stem())
                        .map(|s| s.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ))
        };
    }
    if let Some(first) = existing.into_iter().next() {
        return Ok((first, false));
    }
    let path = profiles_dir(project).join("FolderProfile.pubxml");
    std::fs::create_dir_all(profiles_dir(project))
        .and_then(|_| std::fs::write(&path, default_profile_text(tfm)))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, true))
}

impl Shell {
    /// `eludite.build.publish` on the UI thread: the project (named, else the active document's, else the startup
    /// project), its profile (made when it has none), then the host's `publish` build target.
    pub(super) fn publish_start(
        &mut self,
        project: Option<String>,
        profile: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(BuildCommandOutput, u64), CommandError> {
        if self.solution.is_none() {
            return Err(CommandError::Failed(
                "open a .NET solution first: Publish publishes one of its projects".into(),
            ));
        }
        let path = match project {
            Some(p) => self.resolve_project(Some(p))?,
            None => self
                .resolve_project(None)
                .ok()
                .or_else(|| self.debug.model.startup_project.clone().map(PathBuf::from))
                .ok_or_else(|| {
                    CommandError::Failed(
                        "no project given, no document is active and there is no startup project"
                            .into(),
                    )
                })?,
        };
        let info = eludite_dap::launch::read_project(&path).map_err(CommandError::Failed)?;
        if info.is_legacy() {
            return Err(CommandError::Failed(format!(
                "{} is a legacy (non-SDK) project: Publish needs an SDK-style project",
                info.name
            )));
        }
        let tfm = info
            .target_frameworks
            .first()
            .cloned()
            .unwrap_or_else(|| "net10.0".into());
        let (pubxml, _) =
            resolve_profile(&path, profile.as_deref(), &tfm).map_err(CommandError::Failed)?;
        let p = PublishProfile::read(&pubxml).map_err(CommandError::Failed)?;
        if !p.is_folder() {
            return Err(CommandError::Failed(format!(
                "{} publishes with {}: Eludite publishes to a folder (FileSystem) only",
                pubxml.display(),
                p.protocol.unwrap_or_default()
            )));
        }
        let publish_dir = p.publish_dir_in(&path, Some(&tfm));
        self.builds.publish_next = Some((pubxml.to_string_lossy().into_owned(), p.configuration()));
        let started = self.start_build(
            BuildKind::Publish,
            Some(Some(path.to_string_lossy().into_owned())),
            None,
            None,
            Some(BuildSystemChoice::Msbuild),
            window,
            cx,
        );
        // A refused start leaves nothing for the next build.
        self.builds.publish_next = None;
        let (mut out, ticket) = started?;
        if let BuildCommandOutput::Result(r) = &mut out {
            r.configuration = p.configuration();
            r.publish_dir = Some(publish_dir.to_string_lossy().into_owned());
        }
        Ok((out, ticket))
    }
}
