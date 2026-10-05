//! Releases on GitHub: the release list parsed, a channel's newest release chosen, the archive and `SHA256SUMS`
//! assets picked for a platform, and `SHA256SUMS` parsed (`tools/package/RELEASE.md`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::build::{Build, BuildId, Channel, Platform};

/// The checksum file every release carries, `sha256sum`'s format: `<64 hex digits>  <asset name>` per line.
pub const SUMS_ASSET: &str = "SHA256SUMS";

/// One downloadable file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    /// `browser_download_url`.
    pub url: String,
}

/// One GitHub release, the fields the updater reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub name: String,
    pub prerelease: bool,
    pub draft: bool,
    pub published_at: Option<String>,
    /// The release page.
    pub html_url: Option<String>,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Parse `GET /repos/{owner}/{repo}/releases`'s answer (an array of releases).
    pub fn parse_list(json: &Value) -> Result<Vec<Release>, String> {
        let Some(items) = json.as_array() else {
            return Err("the release list is not a JSON array".into());
        };
        items.iter().map(Release::parse).collect()
    }

    /// Parse one release object.
    pub fn parse(v: &Value) -> Result<Release, String> {
        let tag = v["tag_name"]
            .as_str()
            .ok_or("a release has no tag_name")?
            .to_owned();
        let assets = v["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|a| {
                        Some(Asset {
                            name: a["name"].as_str()?.to_owned(),
                            size: a["size"].as_u64().unwrap_or(0),
                            url: a["browser_download_url"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Release {
            name: v["name"].as_str().unwrap_or(&tag).to_owned(),
            tag,
            prerelease: v["prerelease"].as_bool().unwrap_or(false),
            draft: v["draft"].as_bool().unwrap_or(false),
            published_at: v["published_at"].as_str().map(str::to_owned),
            html_url: v["html_url"].as_str().map(str::to_owned),
            assets,
        })
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The archive of a release that runs on `platform`: `eludite-<version>-<os>-<arch>.<tar.gz|zip>`, its version
/// taken from the name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveAsset {
    pub asset: Asset,
    pub version: String,
    pub platform: Platform,
}

/// Parse an archive asset's name: `eludite-<version>-<os>-<arch>.tar.gz` or `.zip`.
pub fn parse_archive_name(name: &str) -> Option<(String, Platform, &'static str)> {
    let (stem, ext) = if let Some(s) = name.strip_suffix(".tar.gz") {
        (s, "tar.gz")
    } else {
        (name.strip_suffix(".zip")?, "zip")
    };
    let rest = stem.strip_prefix("eludite-")?;
    // The version may hold dashes (`0.2.0-rc.1`); the os and arch never do.
    let (rest, arch) = rest.rsplit_once('-')?;
    let (version, os) = rest.rsplit_once('-')?;
    if version.is_empty() || os.is_empty() || arch.is_empty() {
        return None;
    }
    Some((
        version.to_owned(),
        Platform {
            os: os.to_owned(),
            arch: arch.to_owned(),
        },
        ext,
    ))
}

/// A newer build of the chosen channel, ready to download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub channel: Channel,
    pub build: BuildId,
    pub tag: String,
    pub version: String,
    pub published_at: Option<String>,
    pub html_url: Option<String>,
    pub archive: ArchiveAsset,
    pub sums: Asset,
}

impl Candidate {
    /// What the candidate's `build.json` must say once unpacked.
    pub fn expected_build(&self) -> (Channel, BuildId) {
        (self.channel, self.build.clone())
    }
}

/// Why a channel offers nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Nothing {
    /// The channel has no release at all.
    NoRelease,
    /// The installed build is the channel's newest.
    UpToDate { tag: String },
    /// The newest release has no archive for this platform (or no `SHA256SUMS`).
    NoArchive { tag: String, platform: Platform },
}

/// The newest release of `channel` in `releases` (drafts skipped, the tag rule of the channel), and whether it is
/// newer than `installed` on `platform`.
pub fn choose(
    releases: &[Release],
    channel: Channel,
    installed: Option<&Build>,
    platform: &Platform,
) -> Result<Candidate, Nothing> {
    let newest = releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| channel.build_of_tag(&r.tag).map(|b| (b, r)))
        .max_by(|(a, _), (b, _)| a.cmp(b));
    let Some((build, release)) = newest else {
        return Err(Nothing::NoRelease);
    };
    if let Some(installed) = installed
        && installed.channel == channel
        && installed.build >= build
    {
        return Err(Nothing::UpToDate {
            tag: release.tag.clone(),
        });
    }
    let archive = release
        .assets
        .iter()
        .filter_map(|a| {
            let (version, p, _) = parse_archive_name(&a.name)?;
            platform.matches(&p).then(|| ArchiveAsset {
                asset: a.clone(),
                version,
                platform: p,
            })
        })
        .next();
    let sums = release.asset(SUMS_ASSET);
    match (archive, sums) {
        (Some(archive), Some(sums)) => Ok(Candidate {
            channel,
            build,
            tag: release.tag.clone(),
            version: archive.version.clone(),
            published_at: release.published_at.clone(),
            html_url: release.html_url.clone(),
            archive,
            sums: sums.clone(),
        }),
        _ => Err(Nothing::NoArchive {
            tag: release.tag.clone(),
            platform: platform.clone(),
        }),
    }
}

/// Parse `SHA256SUMS`: lower-case hex digests by file name. A line that is not `<hex>  <name>` (or `<hex> *<name>`)
/// is an error, so a corrupt file never verifies anything.
pub fn parse_sums(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut sums = BTreeMap::new();
    for (ix, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let (hex, name) = line
            .split_once(' ')
            .ok_or_else(|| format!("{SUMS_ASSET} line {}: no name", ix + 1))?;
        let name = name.trim_start_matches([' ', '*']);
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) || name.is_empty() {
            return Err(format!(
                "{SUMS_ASSET} line {}: not a SHA-256 digest and a name",
                ix + 1
            ));
        }
        sums.insert(name.to_owned(), hex.to_ascii_lowercase());
    }
    Ok(sums)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, prerelease: bool, draft: bool, assets: &[&str]) -> Value {
        json!({
            "tag_name": tag, "name": tag, "prerelease": prerelease, "draft": draft,
            "published_at": "2026-10-05T12:00:00Z", "html_url": format!("https://example.test/{tag}"),
            "assets": assets.iter().map(|a| json!({
                "name": a, "size": 10, "browser_download_url": format!("https://example.test/{tag}/{a}")
            })).collect::<Vec<_>>()
        })
    }

    fn linux() -> Platform {
        Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        }
    }

    fn installed(build: &str) -> Build {
        Build {
            version: "0.1.0".into(),
            channel: Channel::Unstable,
            build: BuildId::parse(build).unwrap(),
            commit: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            published: None,
        }
    }

    #[test]
    fn archive_names_parse() {
        let (v, p, ext) = parse_archive_name("eludite-0.1.0-linux-x86_64.tar.gz").unwrap();
        assert_eq!(
            (v.as_str(), p.os.as_str(), p.arch.as_str(), ext),
            ("0.1.0", "linux", "x86_64", "tar.gz")
        );
        let (v, p, ext) = parse_archive_name("eludite-0.2.0-rc.1-windows-x86_64.zip").unwrap();
        assert_eq!(
            (v.as_str(), p.os.as_str(), p.arch.as_str(), ext),
            ("0.2.0-rc.1", "windows", "x86_64", "zip")
        );
        assert!(parse_archive_name("SHA256SUMS").is_none());
        assert!(parse_archive_name("eludite-0.1.0.tar.gz").is_none());
        assert!(parse_archive_name("other-0.1.0-linux-x86_64.tar.gz").is_none());
    }

    #[test]
    fn the_newest_release_of_the_channel_is_chosen() {
        let list = json!([
            release(
                "unstable-20261005.9",
                true,
                false,
                &["eludite-0.1.0-linux-x86_64.tar.gz", "SHA256SUMS"]
            ),
            release(
                "v0.1.0",
                false,
                false,
                &["eludite-0.1.0-linux-x86_64.tar.gz", "SHA256SUMS"]
            ),
            release(
                "unstable-20261006.1",
                true,
                true,
                &["eludite-0.1.0-linux-x86_64.tar.gz", "SHA256SUMS"]
            ),
            release(
                "unstable-20261005.142",
                true,
                false,
                &[
                    "eludite-0.1.0-linux-x86_64.tar.gz",
                    "eludite-0.1.0-windows-x86_64.zip",
                    "eludite-0.1.0-macos-arm64.tar.gz",
                    "SHA256SUMS"
                ]
            ),
        ]);
        let releases = Release::parse_list(&list).unwrap();
        // The draft is skipped, the stable tag is not the channel's, the greatest build wins over list order.
        let c = choose(
            &releases,
            Channel::Unstable,
            Some(&installed("20261005.9")),
            &linux(),
        )
        .unwrap();
        assert_eq!(c.tag, "unstable-20261005.142");
        assert_eq!(c.version, "0.1.0");
        assert_eq!(c.archive.asset.name, "eludite-0.1.0-linux-x86_64.tar.gz");
        assert_eq!(c.sums.name, "SHA256SUMS");
        // macOS on Apple silicon finds the arm64 archive.
        let mac = Platform {
            os: "macos".into(),
            arch: "aarch64".into(),
        };
        let c = choose(&releases, Channel::Unstable, None, &mac).unwrap();
        assert_eq!(c.archive.asset.name, "eludite-0.1.0-macos-arm64.tar.gz");
        // Installed at the newest, or newer: up to date.
        assert_eq!(
            choose(
                &releases,
                Channel::Unstable,
                Some(&installed("20261005.142")),
                &linux()
            ),
            Err(Nothing::UpToDate {
                tag: "unstable-20261005.142".into()
            })
        );
        assert_eq!(
            choose(
                &releases,
                Channel::Unstable,
                Some(&installed("20261007.1")),
                &linux()
            ),
            Err(Nothing::UpToDate {
                tag: "unstable-20261005.142".into()
            })
        );
        // No archive for the platform.
        let bsd = Platform {
            os: "freebsd".into(),
            arch: "x86_64".into(),
        };
        assert!(matches!(
            choose(&releases, Channel::Unstable, None, &bsd),
            Err(Nothing::NoArchive { .. })
        ));
        // No release of the channel at all.
        let only_stable =
            Release::parse_list(&json!([release("v0.1.0", false, false, &[])])).unwrap();
        assert_eq!(
            choose(&only_stable, Channel::Unstable, None, &linux()),
            Err(Nothing::NoRelease)
        );
    }

    #[test]
    fn a_release_without_sums_offers_nothing() {
        let list = json!([release(
            "unstable-20261005.1",
            true,
            false,
            &["eludite-0.1.0-linux-x86_64.tar.gz"]
        )]);
        let releases = Release::parse_list(&list).unwrap();
        assert!(matches!(
            choose(&releases, Channel::Unstable, None, &linux()),
            Err(Nothing::NoArchive { .. })
        ));
    }

    #[test]
    fn sums_parse_and_refuse_garbage() {
        let hex = "a".repeat(64);
        let text = format!(
            "{hex}  eludite-0.1.0-linux-x86_64.tar.gz\n{hex} *eludite-0.1.0-windows-x86_64.zip\r\n\n"
        );
        let sums = parse_sums(&text).unwrap();
        assert_eq!(sums.len(), 2);
        assert_eq!(sums["eludite-0.1.0-windows-x86_64.zip"], hex);
        assert!(parse_sums("not a sums file").is_err());
        assert!(parse_sums("abc  name").is_err());
        assert!(parse_sums(&format!("{hex}  ")).is_err());
    }
}
