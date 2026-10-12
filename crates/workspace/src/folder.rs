//! The plain-folder model (brief 0019, File > Open Folder): the folder's files, and what Eludite knows how to load
//! there (a .NET solution at the root or one folder down, else its project files, brief 0072; a Cargo workspace at the
//! root). Listing walks the disk, so the shell calls it off the UI thread.

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

/// The .NET solution of a folder: at `root`, else in a folder directly below it (this repository keeps
/// `dotnet/Eludite.slnx`); an `.slnx` before an `.sln`, then by path. Hidden and [`SKIPPED_FOLDERS`] are not searched.
pub fn find_solution(root: &Path) -> Option<PathBuf> {
    let is_solution = |p: &Path| {
        p.is_file()
            && p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("slnx") || e.eq_ignore_ascii_case("sln"))
    };
    let pick = |dir: &Path| -> Option<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_solution(p))
            .collect();
        found.sort_by_key(|p| {
            let slnx = p
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("slnx"));
            (!slnx, p.clone())
        });
        found.into_iter().next()
    };
    if let Some(found) = pick(root) {
        return Some(found);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            e.file_type().is_ok_and(|t| t.is_dir())
                && !name.starts_with('.')
                && !SKIPPED_FOLDERS.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    let mut candidates: Vec<PathBuf> = dirs.iter().filter_map(|d| pick(d)).collect();
    candidates.sort_by_key(|p| {
        let slnx = p
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("slnx"));
        (!slnx, p.clone())
    });
    candidates.into_iter().next()
}

/// The project file extensions a folder without a solution falls back to (brief 0072).
pub const PROJECT_EXTENSIONS: [&str; 3] = ["csproj", "vbproj", "fsproj"];

fn is_project_file(p: &Path) -> bool {
    p.is_file()
        && p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| PROJECT_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// The .NET projects of a folder with no solution (brief 0072, the `dotnet new console` layout): the `.csproj`,
/// `.vbproj` and `.fsproj` files at `root`, else those in the folders directly below it (hidden folders and
/// [`SKIPPED_FOLDERS`] are not searched), sorted by path.
pub fn find_projects(root: &Path) -> Vec<PathBuf> {
    let files_in = |dir: &Path| -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_project_file(p))
            .collect();
        found.sort();
        found
    };
    let at_root = files_in(root);
    if !at_root.is_empty() {
        return at_root;
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            e.file_type().is_ok_and(|t| t.is_dir())
                && !name.starts_with('.')
                && !SKIPPED_FOLDERS.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs.iter().flat_map(|d| files_in(d)).collect()
}

/// Where a folder with several projects and no solution keeps the solution Eludite writes for it (brief 0072).
pub fn generated_solution_path(root: &Path) -> PathBuf {
    root.join(".eludite").join("workspace.slnx")
}

/// The text of the generated `.slnx` listing `projects` (absolute paths under `root`), relative to
/// [`generated_solution_path`]'s folder with forward slashes, as `dotnet sln` writes them.
pub fn projects_slnx(root: &Path, projects: &[PathBuf]) -> String {
    let mut out = String::from("<Solution>\n");
    for p in projects {
        let rel = p.strip_prefix(root).unwrap_or(p);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let rel = rel
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;");
        out.push_str(&format!("  <Project Path=\"../{rel}\" />\n"));
    }
    out.push_str("</Solution>\n");
    out
}

/// What a folder's .NET part is (brief 0072): its solution, else its only project, else (several projects) the
/// generated solution with the text it must hold; `None` without either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DotnetEntry {
    Solution(PathBuf),
    Project(PathBuf),
    Generated { path: PathBuf, text: String },
}

impl DotnetEntry {
    /// The file the host opens.
    pub fn path(&self) -> &Path {
        match self {
            DotnetEntry::Solution(p) | DotnetEntry::Project(p) => p,
            DotnetEntry::Generated { path, .. } => path,
        }
    }
}

/// [`find_solution`], else [`find_projects`] (brief 0072).
pub fn find_dotnet_entry(root: &Path) -> Option<DotnetEntry> {
    if let Some(sln) = find_solution(root) {
        return Some(DotnetEntry::Solution(sln));
    }
    let projects = find_projects(root);
    match projects.len() {
        0 => None,
        1 => projects.into_iter().next().map(DotnetEntry::Project),
        _ => Some(DotnetEntry::Generated {
            path: generated_solution_path(root),
            text: projects_slnx(root, &projects),
        }),
    }
}

/// Write the generated solution when its text differs from what is on disk; blocking, small.
pub fn write_generated(path: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
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
        // One level down, as this repository's dotnet/Eludite.slnx; not deeper, not in build outputs.
        let mixed = tempfile::tempdir().unwrap();
        for rel in [
            "dotnet/Eludite.slnx",
            "dotnet/old/Old.sln",
            "target/x/Y.slnx",
            "a/b/Deep.slnx",
        ] {
            let p = mixed.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        assert_eq!(
            find_solution(mixed.path()),
            Some(mixed.path().join("dotnet/Eludite.slnx"))
        );
    }
    #[test]
    fn falls_back_to_project_files() {
        // Brief 0072: `dotnet new console` makes a folder with a project file and no solution.
        let dir = tempfile::tempdir().unwrap();
        let make = |rel: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        };
        assert_eq!(find_dotnet_entry(dir.path()), None);
        make("App.csproj");
        make("Program.cs");
        make("obj/Ghost.csproj");
        assert_eq!(
            find_dotnet_entry(dir.path()),
            Some(DotnetEntry::Project(dir.path().join("App.csproj")))
        );
        // Projects one level down when the root has none; not in build outputs or hidden folders, not deeper.
        let two = tempfile::tempdir().unwrap();
        for rel in [
            "src/Web/Web.csproj",
            "tests/Web.Tests/Web.Tests.fsproj",
            "Lib/Lib.vbproj",
            "bin/X/X.csproj",
            ".hidden/H.csproj",
            "a/b/Deep.csproj",
        ] {
            let p = two.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        assert_eq!(
            find_projects(two.path()),
            vec![two.path().join("Lib/Lib.vbproj")]
        );
        std::fs::create_dir_all(two.path().join("App")).unwrap();
        std::fs::write(two.path().join("App/App.csproj"), "").unwrap();
        let projects = find_projects(two.path());
        assert_eq!(
            projects,
            vec![
                two.path().join("App/App.csproj"),
                two.path().join("Lib/Lib.vbproj")
            ]
        );
        let Some(DotnetEntry::Generated { path, text }) = find_dotnet_entry(two.path()) else {
            panic!("expected a generated solution");
        };
        assert_eq!(path, two.path().join(".eludite/workspace.slnx"));
        assert_eq!(
            text,
            "<Solution>\n  <Project Path=\"../App/App.csproj\" />\n  <Project Path=\"../Lib/Lib.vbproj\" />\n</Solution>\n"
        );
        write_generated(&path, &text).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        // The generated solution is in a hidden folder: it is never found as the folder's own solution.
        assert_eq!(find_solution(two.path()), None);
        // A real solution wins over the projects.
        std::fs::write(two.path().join("All.sln"), "").unwrap();
        assert_eq!(
            find_dotnet_entry(two.path()),
            Some(DotnetEntry::Solution(two.path().join("All.sln")))
        );
    }
}
