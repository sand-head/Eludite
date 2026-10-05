//! What a build is: its identity file beside the executable, its channel, and how two builds compare.

use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The identity file a packaged Eludite carries beside its executable (`tools/package/RELEASE.md`).
pub const BUILD_FILE: &str = "build.json";

/// A release channel. A channel names a rule for which GitHub releases belong to it ([`Channel::accepts_tag`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Every green build of `main`: the release tagged `unstable-<build>`, a pre-release.
    Unstable,
}

impl Channel {
    /// Every channel, in the order the settings list them.
    pub const ALL: [Channel; 1] = [Channel::Unstable];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Unstable => "unstable",
        }
    }

    pub fn parse(s: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// The tag prefix of this channel's releases: `unstable-`.
    pub fn tag_prefix(self) -> String {
        format!("{}-", self.as_str())
    }

    /// The build id a release tag names when the tag belongs to this channel.
    pub fn build_of_tag(self, tag: &str) -> Option<BuildId> {
        let rest = tag.strip_prefix(&self.tag_prefix())?;
        BuildId::parse(rest)
    }

    pub fn accepts_tag(self, tag: &str) -> bool {
        self.build_of_tag(tag).is_some()
    }

    /// Whether the channel's releases are GitHub pre-releases.
    pub fn prerelease(self) -> bool {
        match self {
            Channel::Unstable => true,
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A build's id within its channel: dot-separated segments, each a number or a word, compared segment by segment
/// (numbers by value, else as text; a longer id with the same prefix is newer). CI uses `<YYYYMMDD>.<run number>`,
/// so `20261006.3` is newer than `20261005.142`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BuildId(String);

impl BuildId {
    /// A non-empty id of segments made of ASCII letters and digits.
    pub fn parse(s: &str) -> Option<BuildId> {
        if s.is_empty()
            || !s
                .split('.')
                .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return None;
        }
        Some(BuildId(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn segments(&self) -> impl Iterator<Item = Segment<'_>> {
        self.0.split('.').map(|seg| match seg.parse::<u64>() {
            Ok(n) if seg.len() <= 19 => Segment::Number(n),
            _ => Segment::Text(seg),
        })
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Segment<'a> {
    // A word sorts before any number, so `20261005.rc` is older than `20261005.1`.
    Text(&'a str),
    Number(u64),
}

impl PartialOrd for BuildId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BuildId {
    fn cmp(&self, other: &Self) -> Ordering {
        let mut a = self.segments();
        let mut b = other.segments();
        loop {
            match (a.next(), b.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(x), Some(y)) => match x.cmp(&y) {
                    Ordering::Equal => continue,
                    o => return o,
                },
            }
        }
    }
}

impl fmt::Display for BuildId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The operating system and architecture an archive is built for, spelled as the archive names spell them
/// (`eludite-<version>-<os>-<arch>`: `linux`, `windows` or `macos`; `uname -m`'s `x86_64`, `aarch64`, `arm64`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    /// This executable's.
    pub fn current() -> Platform {
        Platform {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        }
    }

    /// Whether an archive built for `other` runs here: the same OS, and the same architecture, with `arm64` and
    /// `aarch64` taken as one (macOS's `uname -m` says `arm64`, Rust says `aarch64`).
    pub fn matches(&self, other: &Platform) -> bool {
        fn arch(a: &str) -> &str {
            match a {
                "arm64" => "aarch64",
                "amd64" => "x86_64",
                other => other,
            }
        }
        self.os == other.os && arch(&self.arch) == arch(&other.arch)
    }

    /// The archive's extension on this platform: `zip` on Windows, `tar.gz` elsewhere.
    pub fn archive_extension(&self) -> &'static str {
        if self.os == "windows" {
            "zip"
        } else {
            "tar.gz"
        }
    }

    /// The shell executable's file name on this platform.
    pub fn executable(&self) -> &'static str {
        if self.os == "windows" {
            "eludite.exe"
        } else {
            "eludite"
        }
    }
}

/// `build.json`: what a packaged Eludite is, written by the packaging job into the archive's folder and read beside
/// the executable. A development build (cargo's target folder) has none, and the updater stays off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    /// The workspace version (`Cargo.toml`), as the archive name carries it.
    pub version: String,
    pub channel: Channel,
    pub build: BuildId,
    /// The commit the build came from (40 hex digits), when the packager knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub os: String,
    pub arch: String,
    /// When it was published, RFC 3339, when the packager knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

impl Build {
    pub fn platform(&self) -> Platform {
        Platform {
            os: self.os.clone(),
            arch: self.arch.clone(),
        }
    }

    /// The release tag this build came from (`unstable-20261005.142`).
    pub fn tag(&self) -> String {
        format!("{}{}", self.channel.tag_prefix(), self.build)
    }

    /// Parse the file's text.
    pub fn parse(text: &str) -> Result<Build, String> {
        serde_json::from_str(text).map_err(|e| format!("{BUILD_FILE}: {e}"))
    }

    /// Read `build.json` in `dir`: `Ok(None)` when there is none (a development build), `Err` when it is unreadable.
    pub fn read(dir: &Path) -> Result<Option<Build>, String> {
        let path = dir.join(BUILD_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => Build::parse(&text).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Write `build.json` into `dir`.
    pub fn write(&self, dir: &Path) -> std::io::Result<PathBuf> {
        let path = dir.join(BUILD_FILE);
        let text = serde_json::to_string_pretty(self).expect("a build serializes");
        std::fs::write(&path, text + "\n")?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> BuildId {
        BuildId::parse(s).unwrap()
    }

    #[test]
    fn build_ids_compare_segment_by_segment() {
        assert!(id("20261006.3") > id("20261005.142"));
        assert!(id("20261005.142") > id("20261005.9"));
        assert!(id("20261005.142") > id("20261005"));
        assert!(id("20261005.1") > id("20261005.rc"));
        assert_eq!(id("7").cmp(&id("7")), Ordering::Equal);
        assert!(id("99999999999999999999") < id("1") || id("99999999999999999999") > id("1"));
    }

    #[test]
    fn build_ids_are_segments_of_letters_and_digits() {
        assert!(BuildId::parse("").is_none());
        assert!(BuildId::parse("2026.").is_none());
        assert!(BuildId::parse("a b").is_none());
        assert!(BuildId::parse("../x").is_none());
        assert_eq!(id("20261005.142").as_str(), "20261005.142");
    }

    #[test]
    fn a_channel_owns_its_tags() {
        assert_eq!(
            Channel::Unstable.build_of_tag("unstable-20261005.142"),
            Some(id("20261005.142"))
        );
        assert!(!Channel::Unstable.accepts_tag("v0.1.0"));
        assert!(!Channel::Unstable.accepts_tag("unstable-"));
        assert!(!Channel::Unstable.accepts_tag("unstable-2026/1"));
        assert_eq!(Channel::parse("unstable"), Some(Channel::Unstable));
        assert_eq!(Channel::parse("stable"), None);
    }

    #[test]
    fn platforms_match_across_arch_spellings() {
        let here = Platform {
            os: "macos".into(),
            arch: "aarch64".into(),
        };
        assert!(here.matches(&Platform {
            os: "macos".into(),
            arch: "arm64".into()
        }));
        assert!(!here.matches(&Platform {
            os: "linux".into(),
            arch: "aarch64".into()
        }));
        assert_eq!(here.archive_extension(), "tar.gz");
        assert_eq!(
            Platform {
                os: "windows".into(),
                arch: "x86_64".into()
            }
            .executable(),
            "eludite.exe"
        );
    }

    #[test]
    fn build_json_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Build::read(dir.path()).unwrap(), None);
        let b = Build {
            version: "0.1.0".into(),
            channel: Channel::Unstable,
            build: id("20261005.142"),
            commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
            os: "linux".into(),
            arch: "x86_64".into(),
            published: None,
        };
        b.write(dir.path()).unwrap();
        assert_eq!(Build::read(dir.path()).unwrap(), Some(b.clone()));
        assert_eq!(b.tag(), "unstable-20261005.142");
        std::fs::write(dir.path().join(BUILD_FILE), "{").unwrap();
        assert!(Build::read(dir.path()).is_err());
    }
}
