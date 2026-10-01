//! The plain-folder model (brief 0019, File > Open Folder): the folder's files, and what at its root Eludite knows
//! how to load (a .NET solution, a Cargo workspace). Listing walks the disk, so the shell calls it off the UI thread.

use std::path::{Path, PathBuf};

/// Folders never listed: build outputs, dependency caches and VCS metadata. Hidden folders (`.git`, `.vs`,
/// `.worktrees`) are skipped as well.
pub const SKIPPED_FOLDERS: [&str; 4] = ["target", "bin", "obj", "node_modules"];

/// Files listed at most; the listing says when it stopped there.
pub const MAX_FILES: usize = 20_000;

/// A folder's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderListing {
    pub root: PathBuf,
    /// Absolute paths, sorted.
    pub files: Vec<PathBuf>,
    /// True when the walk stopped at `max_files`.
    pub truncated: bool,
}

/// List `root`'s files (recursively), skipping [`SKIPPED_FOLDERS`] and hidden folders, at most `max_files`.
pub fn list_folder(root: &Path, max_files: usize) -> FolderListing {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if !name.starts_with('.') && !SKIPPED_FOLDERS.contains(&name.as_ref()) {
                    stack.push(e.path());
                }
            } else if kind.is_file() {
                if files.len() >= max_files {
                    truncated = true;
                    break;
                }
                files.push(e.path());
            }
        }
        if truncated {
            break;
        }
    }
    files.sort();
    FolderListing {
        root: root.to_path_buf(),
        files,
        truncated,
    }
}

/// The .NET solution at `root` (not below it): an `.slnx` before an `.sln`, then by name.
pub fn find_solution(root: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e.eq_ignore_ascii_case("slnx") || e.eq_ignore_ascii_case("sln")
                })
        })
        .collect();
    found.sort_by_key(|p| {
        let slnx = p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("slnx"));
        (!slnx, p.clone())
    });
    found.into_iter().next()
}

/// `root/Cargo.toml`, when it exists.
pub fn find_cargo_manifest(root: &Path) -> Option<PathBuf> {
    let p = root.join("Cargo.toml");
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for rel in [
            "Cargo.toml",
            "Cargo.lock",
            "App.sln",
            "App.slnx",
            "crates/a/src/lib.rs",
            "crates/a/target/debug/junk",
            "target/debug/eludite",
            "src/App/bin/Debug/App.dll",
            "src/App/obj/x.cs",
            "src/App/Program.cs",
            ".git/HEAD",
            ".worktrees/x/Cargo.toml",
            "docs/PLAN.md",
        ] {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        dir
    }

    #[test]
    fn lists_files_without_build_outputs_or_hidden_folders() {
        let dir = tree();
        let l = list_folder(dir.path(), MAX_FILES);
        let rel: Vec<String> = l
            .files
            .iter()
            .map(|p| {
                p.strip_prefix(dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            rel,
            [
                "App.sln",
                "App.slnx",
                "Cargo.lock",
                "Cargo.toml",
                "crates/a/src/lib.rs",
                "docs/PLAN.md",
                "src/App/Program.cs"
            ]
        );
        assert!(!l.truncated);
        let capped = list_folder(dir.path(), 3);
        assert_eq!(capped.files.len(), 3);
        assert!(capped.truncated);
    }

    #[test]
    fn finds_what_the_root_holds() {
        let dir = tree();
        assert_eq!(find_solution(dir.path()), Some(dir.path().join("App.slnx")));
        std::fs::remove_file(dir.path().join("App.slnx")).unwrap();
        assert_eq!(find_solution(dir.path()), Some(dir.path().join("App.sln")));
        assert_eq!(
            find_cargo_manifest(dir.path()),
            Some(dir.path().join("Cargo.toml"))
        );
        assert_eq!(find_cargo_manifest(&dir.path().join("docs")), None);
        assert_eq!(find_solution(&dir.path().join("docs")), None);
    }
}
