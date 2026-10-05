//! The stage: `.eludite-update/` inside the install folder, where an archive is downloaded and unpacked so the swap
//! is a rename on one filesystem. `staged.json` there names the update that is ready to apply.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::build::{Build, Platform};
use crate::error::{Error, Result};
use crate::http::Cancel;
use crate::release::Candidate;

/// The stage folder's name inside the install folder.
pub const STAGE_DIR: &str = ".eludite-update";
/// The previous build's files after a swap, inside the install folder, until the next successful start removes it.
pub const PREVIOUS_DIR: &str = ".eludite-previous";
/// The file naming the staged update.
pub const STAGED_FILE: &str = "staged.json";
/// Where the applier copy and its plan live.
pub const APPLY_DIR: &str = "apply";

/// An install folder's stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub install_dir: PathBuf,
}

/// An update downloaded, verified, unpacked and checked: ready to swap in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub tag: String,
    pub build: Build,
    /// The unpacked layout (the archive's folder without its top directory).
    pub layout: PathBuf,
    /// The archive it came from, kept until the swap.
    pub archive: PathBuf,
    /// When it was staged, RFC 3339.
    pub at: String,
}

impl Stage {
    pub fn new(install_dir: &Path) -> Stage {
        Stage {
            install_dir: install_dir.to_owned(),
        }
    }

    pub fn root(&self) -> PathBuf {
        self.install_dir.join(STAGE_DIR)
    }

    pub fn previous_dir(&self) -> PathBuf {
        self.install_dir.join(PREVIOUS_DIR)
    }

    pub fn staged_file(&self) -> PathBuf {
        self.root().join(STAGED_FILE)
    }

    /// Whether the install folder can be written: the stage folder is created and a probe file written in it.
    /// `Err` names the folder and the error, for the status output ("installed in a folder you cannot write").
    pub fn ensure_writable(&self) -> std::result::Result<(), String> {
        let root = self.root();
        let probe = root.join(".probe");
        std::fs::create_dir_all(&root)
            .and_then(|()| std::fs::write(&probe, b""))
            .and_then(|()| std::fs::remove_file(&probe))
            .map_err(|e| format!("{} cannot be written ({e})", self.install_dir.display()))
    }

    /// The folder of one release's files.
    pub fn dir_of(&self, tag: &str) -> PathBuf {
        self.root().join(tag)
    }

    /// Where the archive of `candidate` is downloaded to.
    pub fn archive_path(&self, candidate: &Candidate) -> PathBuf {
        self.dir_of(&candidate.tag)
            .join(&candidate.archive.asset.name)
    }

    /// Unpack the downloaded `archive` of `candidate` and check the layout: the executable and `build.json`, which
    /// must name the candidate's channel and build and this `platform`. Records it as the staged update.
    pub fn stage(
        &self,
        candidate: &Candidate,
        archive: &Path,
        platform: &Platform,
        cancel: &Cancel,
    ) -> Result<Staged> {
        let layout = self.dir_of(&candidate.tag).join("layout");
        crate::extract::unpack(archive, &layout, cancel)?;
        let exe = layout.join(platform.executable());
        if !exe.is_file() {
            return Err(Error::archive(format!(
                "{} holds no {}",
                candidate.archive.asset.name,
                platform.executable()
            )));
        }
        let build = Build::read(&layout)
            .map_err(Error::archive)?
            .ok_or_else(|| {
                Error::archive(format!(
                    "{} carries no {} (not a release archive)",
                    candidate.archive.asset.name,
                    crate::build::BUILD_FILE
                ))
            })?;
        if (build.channel, build.build.clone()) != candidate.expected_build() {
            return Err(Error::archive(format!(
                "{} says it is {} {}, not {}",
                crate::build::BUILD_FILE,
                build.channel,
                build.build,
                candidate.tag
            )));
        }
        if !platform.matches(&build.platform()) {
            return Err(Error::archive(format!(
                "{} is built for {}-{}, not {}-{}",
                crate::build::BUILD_FILE,
                build.os,
                build.arch,
                platform.os,
                platform.arch
            )));
        }
        let staged = Staged {
            tag: candidate.tag.clone(),
            build,
            layout,
            archive: archive.to_owned(),
            at: crate::time::now_rfc3339(),
        };
        let text = serde_json::to_string_pretty(&staged).expect("staged serializes");
        std::fs::write(self.staged_file(), text)?;
        // Other releases' leftovers go.
        self.clean(Some(&candidate.tag));
        Ok(staged)
    }

    /// The staged update, if its files are still there.
    pub fn staged(&self) -> Option<Staged> {
        let text = std::fs::read_to_string(self.staged_file()).ok()?;
        let staged: Staged = serde_json::from_str(&text).ok()?;
        staged.layout.is_dir().then_some(staged)
    }

    /// Forget the staged update and delete its files.
    pub fn discard(&self) {
        if let Some(s) = self.staged() {
            let _ = std::fs::remove_dir_all(self.dir_of(&s.tag));
        }
        let _ = std::fs::remove_file(self.staged_file());
    }

    /// Delete every release folder but `keep`'s, and the applier copy.
    pub fn clean(&self, keep: Option<&str>) {
        let Ok(entries) = std::fs::read_dir(self.root()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == STAGED_FILE || Some(name.as_ref()) == keep {
                continue;
            }
            if entry.path().is_dir() {
                let _ = std::fs::remove_dir_all(entry.path());
            } else {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// After a start: the previous build's files are no longer needed, and no update is staged (the swap used it).
    /// Returns what was removed.
    pub fn clean_after_start(&self) -> Vec<PathBuf> {
        let mut removed = Vec::new();
        let previous = self.previous_dir();
        if previous.is_dir() && std::fs::remove_dir_all(&previous).is_ok() {
            removed.push(previous);
        }
        let apply = self.root().join(APPLY_DIR);
        if apply.is_dir() && std::fs::remove_dir_all(&apply).is_ok() {
            removed.push(apply);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{BuildId, Channel};
    use crate::release::{ArchiveAsset, Asset};

    pub(crate) fn candidate(tag: &str, build: &str, name: &str) -> Candidate {
        Candidate {
            channel: Channel::Unstable,
            build: BuildId::parse(build).unwrap(),
            tag: tag.into(),
            version: "0.1.0".into(),
            published_at: None,
            html_url: None,
            archive: ArchiveAsset {
                asset: Asset {
                    name: name.into(),
                    size: 0,
                    url: format!("http://127.0.0.1:1/{name}"),
                },
                version: "0.1.0".into(),
                platform: Platform {
                    os: "linux".into(),
                    arch: "x86_64".into(),
                },
            },
            sums: Asset {
                name: "SHA256SUMS".into(),
                size: 0,
                url: "http://127.0.0.1:1/SHA256SUMS".into(),
            },
        }
    }

    pub(crate) fn linux() -> Platform {
        Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        }
    }

    fn build_json(build: &str) -> Vec<u8> {
        serde_json::to_vec(&Build {
            version: "0.1.0".into(),
            channel: Channel::Unstable,
            build: BuildId::parse(build).unwrap(),
            commit: None,
            os: "linux".into(),
            arch: "x86_64".into(),
            published: None,
        })
        .unwrap()
    }

    #[test]
    fn staging_checks_the_layout_against_the_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let stage = Stage::new(dir.path());
        stage.ensure_writable().unwrap();
        let c = candidate(
            "unstable-20261005.2",
            "20261005.2",
            "eludite-0.1.0-linux-x86_64.tar.gz",
        );
        let archive = stage.archive_path(&c);
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        let good = build_json("20261005.2");
        crate::extract::test_support::tar_gz(
            &archive,
            "eludite-0.1.0-linux-x86_64",
            &[("eludite", b"bin", 0o755), ("build.json", &good, 0o644)],
        );
        let staged = stage.stage(&c, &archive, &linux(), &Cancel::new()).unwrap();
        assert_eq!(staged.tag, "unstable-20261005.2");
        assert!(staged.layout.join("eludite").is_file());
        assert_eq!(stage.staged().unwrap(), staged);

        // The wrong build inside is refused and nothing stays staged.
        let wrong = build_json("20261005.1");
        crate::extract::test_support::tar_gz(
            &archive,
            "eludite-0.1.0-linux-x86_64",
            &[("eludite", b"bin", 0o755), ("build.json", &wrong, 0o644)],
        );
        let err = stage
            .stage(&c, &archive, &linux(), &Cancel::new())
            .unwrap_err();
        assert!(err.to_string().contains("not unstable-20261005.2"), "{err}");
        // No executable.
        crate::extract::test_support::tar_gz(&archive, "x", &[("build.json", &good, 0o644)]);
        let err = stage
            .stage(&c, &archive, &linux(), &Cancel::new())
            .unwrap_err();
        assert!(err.to_string().contains("holds no eludite"), "{err}");
        // No build.json.
        crate::extract::test_support::tar_gz(&archive, "x", &[("eludite", b"bin", 0o755)]);
        let err = stage
            .stage(&c, &archive, &linux(), &Cancel::new())
            .unwrap_err();
        assert!(err.to_string().contains("carries no build.json"), "{err}");

        stage.discard();
        assert_eq!(stage.staged(), None);
        assert!(!stage.dir_of(&c.tag).exists());
    }

    #[test]
    fn cleaning_keeps_one_release_and_removes_the_previous_build_after_a_start() {
        let dir = tempfile::tempdir().unwrap();
        let stage = Stage::new(dir.path());
        std::fs::create_dir_all(stage.dir_of("unstable-1")).unwrap();
        std::fs::create_dir_all(stage.dir_of("unstable-2")).unwrap();
        std::fs::create_dir_all(stage.root().join(APPLY_DIR)).unwrap();
        std::fs::create_dir_all(stage.previous_dir()).unwrap();
        stage.clean(Some("unstable-2"));
        assert!(!stage.dir_of("unstable-1").exists());
        assert!(stage.dir_of("unstable-2").exists());
        assert!(!stage.root().join(APPLY_DIR).exists());
        let removed = stage.clean_after_start();
        assert_eq!(removed, vec![stage.previous_dir()]);
    }
}
