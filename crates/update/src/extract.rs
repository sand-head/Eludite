//! Unpacking an archive into a folder: `.tar.gz` (Linux, macOS) through `tar` and `flate2`, `.zip` (Windows)
//! through [`crate::zip`]. The archive's single top-level folder (`eludite-<version>-<os>-<arch>/`) is stripped, so
//! the folder holds the layout itself. Entries that would escape the folder are refused: no absolute paths, no `..`,
//! no symbolic link out of the folder.

use std::fs::File;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};
use crate::http::Cancel;

/// Unpack `archive` into `into` (created, must be empty or absent). Returns the number of files written.
pub fn unpack(archive: &Path, into: &Path, cancel: &Cancel) -> Result<u64> {
    if into.exists() {
        std::fs::remove_dir_all(into)?;
    }
    std::fs::create_dir_all(into)?;
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        unpack_tar_gz(archive, into, cancel)
    } else if name.ends_with(".zip") {
        unpack_zip(archive, into, cancel)
    } else {
        Err(Error::archive(format!("{name}: not a .tar.gz or .zip")))
    }
}

/// The path inside `into` an entry named `name` lands at, with the top folder `top` stripped; `None` for the top
/// folder itself or an unsafe name.
fn relative(name: &Path, top: Option<&str>) -> Result<Option<PathBuf>> {
    let mut out = PathBuf::new();
    let mut parts = name.components().peekable();
    if let Some(top) = top
        && matches!(parts.peek(), Some(Component::Normal(c)) if *c == top)
    {
        parts.next();
    }
    for c in parts {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => {
                return Err(Error::archive(format!(
                    "{} would unpack outside the folder",
                    name.display()
                )));
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// The folder every entry of the archive sits under, if there is exactly one.
fn common_top<'a>(names: impl Iterator<Item = &'a Path>) -> Option<String> {
    let mut top: Option<String> = None;
    for name in names {
        let first = match name.components().next() {
            Some(Component::Normal(c)) => c.to_string_lossy().into_owned(),
            _ => return None,
        };
        match &top {
            None => top = Some(first),
            Some(t) if *t == first => {}
            Some(_) => return None,
        }
    }
    top
}

fn unpack_tar_gz(archive: &Path, into: &Path, cancel: &Cancel) -> Result<u64> {
    // Two passes: the first finds the top folder (and refuses bad names), the second unpacks.
    let open = || -> Result<tar::Archive<flate2::read::GzDecoder<BufReader<File>>>> {
        let file = File::open(archive)?;
        Ok(tar::Archive::new(flate2::read::GzDecoder::new(
            BufReader::new(file),
        )))
    };
    let mut names = Vec::new();
    for entry in open()?
        .entries()
        .map_err(|e| Error::archive(e.to_string()))?
    {
        let entry = entry.map_err(|e| Error::archive(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| Error::archive(e.to_string()))?
            .into_owned();
        names.push(path);
    }
    let top = common_top(names.iter().map(PathBuf::as_path));
    let mut written = 0u64;
    let mut ar = open()?;
    ar.set_preserve_permissions(true);
    ar.set_overwrite(true);
    for entry in ar.entries().map_err(|e| Error::archive(e.to_string()))? {
        cancel.check()?;
        let mut entry = entry.map_err(|e| Error::archive(e.to_string()))?;
        let name = entry
            .path()
            .map_err(|e| Error::archive(e.to_string()))?
            .into_owned();
        let Some(rel) = relative(&name, top.as_deref())? else {
            continue;
        };
        let dest = into.join(&rel);
        let kind = entry.header().entry_type();
        match kind {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&dest)?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                entry
                    .unpack(&dest)
                    .map_err(|e| Error::archive(format!("{}: {e}", rel.display())))?;
                written += 1;
            }
            tar::EntryType::Symlink | tar::EntryType::Link => {
                let target = entry
                    .link_name()
                    .map_err(|e| Error::archive(e.to_string()))?
                    .ok_or_else(|| {
                        Error::archive(format!("{}: a link without a target", rel.display()))
                    })?
                    .into_owned();
                check_link(&rel, &target)?;
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                entry
                    .unpack(&dest)
                    .map_err(|e| Error::archive(format!("{}: {e}", rel.display())))?;
                written += 1;
            }
            // Extended headers, global headers and the like carry no file.
            _ => {}
        }
    }
    Ok(written)
}

/// A link at `at` (relative to the folder) may point only inside the folder.
fn check_link(at: &Path, target: &Path) -> Result<()> {
    if target.is_absolute() {
        return Err(Error::archive(format!(
            "{} links outside the folder ({})",
            at.display(),
            target.display()
        )));
    }
    let mut depth: i64 = at.components().count() as i64 - 1;
    for c in target.components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => depth = -1,
        }
        if depth < 0 {
            return Err(Error::archive(format!(
                "{} links outside the folder ({})",
                at.display(),
                target.display()
            )));
        }
    }
    Ok(())
}

fn unpack_zip(archive: &Path, into: &Path, cancel: &Cancel) -> Result<u64> {
    let mut file = BufReader::new(File::open(archive)?);
    let entries = crate::zip::entries(&mut file)?;
    let names: Vec<PathBuf> = entries.iter().map(|e| PathBuf::from(&e.name)).collect();
    let top = common_top(names.iter().map(PathBuf::as_path));
    let mut written = 0u64;
    for (entry, name) in entries.iter().zip(&names) {
        cancel.check()?;
        if entry.name.contains('\\') {
            return Err(Error::archive(format!(
                "{}: backslash in a zip name",
                entry.name
            )));
        }
        let Some(rel) = relative(name, top.as_deref())? else {
            continue;
        };
        let dest = into.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::io::BufWriter::new(File::create(&dest)?);
        crate::zip::read_entry(&mut file, entry, &mut out)?;
        drop(out);
        #[cfg(unix)]
        if let Some(mode) = entry.mode {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
        written += 1;
    }
    Ok(written)
}

/// Archives for the tests of this crate and the shell's.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::fs::File;
    use std::io::Write as _;
    use std::path::Path;

    /// A tar.gz of `files` (`(name, data, mode)`) under the folder `top`.
    pub fn tar_gz(path: &Path, top: &str, files: &[(&str, &[u8], u32)]) {
        let file = File::create(path).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_mode(0o755);
        dir.set_size(0);
        dir.set_cksum();
        tar.append_data(&mut dir, format!("{top}/"), std::io::empty())
            .unwrap();
        for (name, data, mode) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(*mode);
            h.set_cksum();
            tar.append_data(&mut h, format!("{top}/{name}"), *data)
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap().flush().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::tar_gz;

    #[test]
    fn a_tarball_unpacks_without_its_top_folder() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("eludite-0.1.0-linux-x86_64.tar.gz");
        tar_gz(
            &archive,
            "eludite-0.1.0-linux-x86_64",
            &[
                ("eludite", b"#!/bin/sh\n", 0o755),
                ("cef/libcef.so", b"so", 0o644),
                ("build.json", b"{}", 0o644),
            ],
        );
        let into = dir.path().join("layout");
        assert_eq!(unpack(&archive, &into, &Cancel::new()).unwrap(), 3);
        assert!(into.join("eludite").is_file());
        assert!(into.join("cef/libcef.so").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(into.join("eludite"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        // Unpacking again replaces the folder.
        std::fs::write(into.join("stale"), b"x").unwrap();
        unpack(&archive, &into, &Cancel::new()).unwrap();
        assert!(!into.join("stale").exists());
    }

    #[test]
    fn entries_escaping_the_folder_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad.tar.gz");
        let file = File::create(&archive).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        // `tar::Builder::append_data` refuses `..`; write the header's name field as an attacker would.
        let mut h = tar::Header::new_gnu();
        let name = b"top/../../escape";
        h.as_mut_bytes()[..name.len()].copy_from_slice(name);
        h.set_size(1);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append(&h, &b"x"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        let err = unpack(&archive, &dir.path().join("out"), &Cancel::new()).unwrap_err();
        assert!(err.to_string().contains("outside the folder"), "{err}");
        assert!(!dir.path().join("escape").exists());

        assert!(
            check_link(
                Path::new("cef/libvulkan.so.1"),
                Path::new("libvulkan.so.1.3")
            )
            .is_ok()
        );
        assert!(check_link(Path::new("cef/x"), Path::new("../eludite")).is_ok());
        assert!(check_link(Path::new("cef/x"), Path::new("../../etc/passwd")).is_err());
        assert!(check_link(Path::new("x"), Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn a_zip_unpacks_without_its_top_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = crate::zip::write::Writer::new();
        w.add("eludite-0.1.0-windows-x86_64/", b"", false, None);
        w.add(
            "eludite-0.1.0-windows-x86_64/eludite.exe",
            &[1u8; 5000],
            true,
            None,
        );
        w.add(
            "eludite-0.1.0-windows-x86_64/eludite-host/eludite-host.dll",
            b"dll",
            false,
            None,
        );
        let archive = dir.path().join("eludite-0.1.0-windows-x86_64.zip");
        std::fs::write(&archive, w.finish()).unwrap();
        let into = dir.path().join("layout");
        assert_eq!(unpack(&archive, &into, &Cancel::new()).unwrap(), 2);
        assert_eq!(
            std::fs::read(into.join("eludite.exe")).unwrap(),
            vec![1u8; 5000]
        );
        assert!(into.join("eludite-host/eludite-host.dll").is_file());

        let mut w = crate::zip::write::Writer::new();
        w.add("../escape.txt", b"x", false, None);
        std::fs::write(dir.path().join("bad.zip"), w.finish()).unwrap();
        assert!(
            unpack(
                &dir.path().join("bad.zip"),
                &dir.path().join("out"),
                &Cancel::new()
            )
            .is_err()
        );
    }

    #[test]
    fn archives_without_one_top_folder_unpack_as_they_are() {
        let names = [PathBuf::from("a/x"), PathBuf::from("b/y")];
        assert_eq!(common_top(names.iter().map(PathBuf::as_path)), None);
        let names = [PathBuf::from("a/x"), PathBuf::from("a/y")];
        assert_eq!(
            common_top(names.iter().map(PathBuf::as_path)).as_deref(),
            Some("a")
        );
        assert_eq!(
            relative(Path::new("a/x"), Some("a")).unwrap(),
            Some(PathBuf::from("x"))
        );
        assert_eq!(relative(Path::new("a/"), Some("a")).unwrap(), None);
        assert_eq!(
            relative(Path::new("b/x"), Some("a")).unwrap(),
            Some(PathBuf::from("b/x"))
        );
    }
}
