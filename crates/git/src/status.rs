//! The status: the branch, its upstream with ahead and behind, the staged and unstaged changes with rename
//! detection, untracked, conflicted and ignored files, the operation in progress and the stash count; and the glyph
//! each file shows in the Workspace window and on its document tab ([`GlyphIndex`]).

use std::collections::HashMap;
use std::path::PathBuf;

use git2::{Repository, StatusOptions, StatusShow};

use crate::{Repo, Result};

/// What changed in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Typechange,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Added => "added",
            ChangeKind::Modified => "modified",
            ChangeKind::Deleted => "deleted",
            ChangeKind::Renamed => "renamed",
            ChangeKind::Typechange => "typechange",
        }
    }
}

/// One changed file of the Staged Changes or Changes group.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Change {
    /// Relative to the repository root, `/`-separated.
    pub path: String,
    pub kind: ChangeKind,
    /// A rename's old path.
    pub old_path: Option<String>,
}

/// An operation that stopped part way (a conflict).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    ApplyMailbox,
    Bisect,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry_pick",
            Operation::Revert => "revert",
            Operation::ApplyMailbox => "apply_mailbox",
            Operation::Bisect => "bisect",
        }
    }

    fn of(state: git2::RepositoryState) -> Option<Self> {
        use git2::RepositoryState as S;
        Some(match state {
            S::Clean => return None,
            S::Merge => Operation::Merge,
            S::Revert | S::RevertSequence => Operation::Revert,
            S::CherryPick | S::CherryPickSequence => Operation::CherryPick,
            S::Bisect => Operation::Bisect,
            S::Rebase | S::RebaseInteractive | S::RebaseMerge => Operation::Rebase,
            S::ApplyMailbox | S::ApplyMailboxOrRebase => Operation::ApplyMailbox,
        })
    }
}

/// A repository's status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub workdir: PathBuf,
    /// The checked-out branch (an unborn one too); `None` when HEAD is detached.
    pub branch: Option<String>,
    pub detached: bool,
    /// HEAD's commit; `None` on an unborn branch.
    pub head: Option<git2::Oid>,
    /// `origin/main`.
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub operation: Option<Operation>,
    pub staged: Vec<Change>,
    pub unstaged: Vec<Change>,
    pub untracked: Vec<String>,
    pub conflicted: Vec<String>,
    /// Ignored files and folders (a folder ends with `/`), when asked for.
    pub ignored: Vec<String>,
    pub stashes: usize,
    /// `http.sslVerify` is false in the repository's (or the global) config: TLS certificates of its remotes are not
    /// checked, and the Git Changes window says so (brief 0045).
    pub ssl_verify_off: bool,
}

impl Status {
    /// Files with any change (staged, unstaged, untracked or conflicted), each once.
    pub fn changed_files(&self) -> usize {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for c in self.staged.iter().chain(&self.unstaged) {
            seen.insert(&c.path);
        }
        for p in self.untracked.iter().chain(&self.conflicted) {
            seen.insert(p);
        }
        seen.len()
    }

    /// The glyph of each changed, untracked, conflicted and ignored file.
    pub fn glyphs(&self) -> GlyphIndex {
        GlyphIndex::new(self)
    }
}

/// Visual Studio's source-control glyphs in the Workspace window and on document tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileGlyph {
    /// Changed (staged or not): Visual Studio's red check.
    Modified,
    /// A new file in the index: the green plus.
    Added,
    /// A new file not in the index.
    Untracked,
    Deleted,
    Renamed,
    /// Ignored: the grey circle with a bar.
    Ignored,
    /// Conflicted: the red exclamation mark.
    Conflicted,
}

impl FileGlyph {
    /// The glyph's text.
    pub fn glyph(self) -> &'static str {
        match self {
            FileGlyph::Modified => "\u{2713}",
            FileGlyph::Added => "+",
            FileGlyph::Untracked => "?",
            FileGlyph::Deleted => "\u{2212}",
            FileGlyph::Renamed => "R",
            FileGlyph::Ignored => "\u{2298}",
            FileGlyph::Conflicted => "!",
        }
    }

    /// Its color (Visual Studio's dark theme): red for modified and conflicted, green for added, grey for ignored.
    pub fn color(self) -> u32 {
        match self {
            FileGlyph::Modified | FileGlyph::Renamed => 0xC5_86_86,
            FileGlyph::Added | FileGlyph::Untracked => 0x73_C9_91,
            FileGlyph::Deleted | FileGlyph::Conflicted => 0xF1_4C_4C,
            FileGlyph::Ignored => 0x8C_8C_8C,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FileGlyph::Modified => "modified",
            FileGlyph::Added => "added",
            FileGlyph::Untracked => "untracked",
            FileGlyph::Deleted => "deleted",
            FileGlyph::Renamed => "renamed",
            FileGlyph::Ignored => "ignored",
            FileGlyph::Conflicted => "conflicted",
        }
    }
}

/// Glyph lookup by repository-relative path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlyphIndex {
    files: HashMap<String, FileGlyph>,
    /// Ignored folders, `/`-terminated.
    ignored_dirs: Vec<String>,
}

impl GlyphIndex {
    fn new(s: &Status) -> Self {
        let mut files = HashMap::new();
        let glyph_of = |k: ChangeKind| match k {
            ChangeKind::Added => FileGlyph::Added,
            ChangeKind::Deleted => FileGlyph::Deleted,
            ChangeKind::Renamed => FileGlyph::Renamed,
            ChangeKind::Modified | ChangeKind::Typechange => FileGlyph::Modified,
        };
        let mut ignored_dirs = Vec::new();
        for p in &s.ignored {
            if p.ends_with('/') {
                ignored_dirs.push(p.clone());
            } else {
                files.insert(p.clone(), FileGlyph::Ignored);
            }
        }
        for p in &s.untracked {
            files.insert(p.clone(), FileGlyph::Untracked);
        }
        // Staged first, then unstaged over it: a staged add later modified stays Added (Visual Studio's plus).
        for c in &s.staged {
            files.insert(c.path.clone(), glyph_of(c.kind));
        }
        for c in &s.unstaged {
            let g = glyph_of(c.kind);
            files
                .entry(c.path.clone())
                .and_modify(|e| {
                    if *e != FileGlyph::Added || g == FileGlyph::Deleted {
                        *e = g
                    }
                })
                .or_insert(g);
        }
        for p in &s.conflicted {
            files.insert(p.clone(), FileGlyph::Conflicted);
        }
        ignored_dirs.sort();
        Self {
            files,
            ignored_dirs,
        }
    }

    /// The glyph of `rel` (relative, `/`-separated), or `None` for an unchanged tracked file.
    pub fn get(&self, rel: &str) -> Option<FileGlyph> {
        if let Some(g) = self.files.get(rel) {
            return Some(*g);
        }
        // An ignored folder's contents are ignored.
        let i = self.ignored_dirs.partition_point(|d| d.as_str() <= rel);
        (i > 0 && rel.starts_with(self.ignored_dirs[i - 1].as_str())).then_some(FileGlyph::Ignored)
    }

    pub fn len(&self) -> usize {
        self.files.len() + self.ignored_dirs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn kind_of_index(s: git2::Status) -> Option<ChangeKind> {
    Some(if s.contains(git2::Status::INDEX_NEW) {
        ChangeKind::Added
    } else if s.contains(git2::Status::INDEX_RENAMED) {
        ChangeKind::Renamed
    } else if s.contains(git2::Status::INDEX_DELETED) {
        ChangeKind::Deleted
    } else if s.contains(git2::Status::INDEX_TYPECHANGE) {
        ChangeKind::Typechange
    } else if s.contains(git2::Status::INDEX_MODIFIED) {
        ChangeKind::Modified
    } else {
        return None;
    })
}

fn kind_of_workdir(s: git2::Status) -> Option<ChangeKind> {
    Some(if s.contains(git2::Status::WT_RENAMED) {
        ChangeKind::Renamed
    } else if s.contains(git2::Status::WT_DELETED) {
        ChangeKind::Deleted
    } else if s.contains(git2::Status::WT_TYPECHANGE) {
        ChangeKind::Typechange
    } else if s.contains(git2::Status::WT_MODIFIED) {
        ChangeKind::Modified
    } else {
        return None;
    })
}

fn delta_paths(d: &git2::DiffDelta<'_>) -> (Option<String>, Option<String>) {
    let p = |f: git2::DiffFile<'_>| f.path().map(|p| p.to_string_lossy().replace('\\', "/"));
    (p(d.old_file()), p(d.new_file()))
}

impl Repo {
    /// The status, with ignored files and folders when `include_ignored`.
    pub fn status(&self, include_ignored: bool) -> Result<Status> {
        let mut repo = self.repository()?;
        let mut out = head_info(&repo)?;
        out.workdir = self.workdir().to_path_buf();
        out.operation = Operation::of(repo.state());
        {
            let mut opts = StatusOptions::new();
            opts.show(StatusShow::IndexAndWorkdir)
                .include_untracked(true)
                .recurse_untracked_dirs(true)
                .include_ignored(include_ignored)
                .recurse_ignored_dirs(false)
                .exclude_submodules(true)
                .renames_head_to_index(true)
                .renames_index_to_workdir(true);
            let statuses = repo.statuses(Some(&mut opts))?;
            for e in statuses.iter() {
                let s = e.status();
                let path = e.path().map(|p| p.replace('\\', "/")).unwrap_or_default();
                if s.contains(git2::Status::CONFLICTED) {
                    out.conflicted.push(path);
                    continue;
                }
                if s.contains(git2::Status::IGNORED) {
                    out.ignored.push(path);
                    continue;
                }
                if let Some(kind) = kind_of_index(s) {
                    let (old, new) = e
                        .head_to_index()
                        .map(|d| delta_paths(&d))
                        .unwrap_or((None, None));
                    let path = new.clone().unwrap_or_else(|| path.clone());
                    out.staged.push(Change {
                        old_path: (kind == ChangeKind::Renamed).then_some(old).flatten(),
                        path,
                        kind,
                    });
                }
                if s.contains(git2::Status::WT_NEW) {
                    out.untracked.push(path.clone());
                } else if let Some(kind) = kind_of_workdir(s) {
                    let (old, new) = e
                        .index_to_workdir()
                        .map(|d| delta_paths(&d))
                        .unwrap_or((None, None));
                    out.unstaged.push(Change {
                        old_path: (kind == ChangeKind::Renamed).then_some(old).flatten(),
                        path: new.unwrap_or(path),
                        kind,
                    });
                }
            }
        }
        out.staged.sort_by(|a, b| a.path.cmp(&b.path));
        out.unstaged.sort_by(|a, b| a.path.cmp(&b.path));
        out.untracked.sort();
        out.conflicted.sort();
        out.ignored.sort();
        let mut stashes = 0;
        repo.stash_foreach(|_, _, _| {
            stashes += 1;
            true
        })
        .ok();
        out.stashes = stashes;
        out.ssl_verify_off = !self.ssl_verify();
        Ok(out)
    }

    /// The checked-out branch's short name (an unborn one too), or `None` when HEAD is detached.
    pub fn current_branch(&self) -> Result<Option<String>> {
        Ok(head_info(&self.repository()?)?.branch)
    }
}

/// The branch, HEAD, upstream, ahead and behind.
pub(crate) fn head_info(repo: &Repository) -> Result<Status> {
    let mut out = Status::default();
    match repo.head() {
        Ok(head) => {
            out.head = head.target();
            if head.is_branch() {
                let name = head.shorthand().map(str::to_owned);
                if let Some(name) = &name
                    && let Ok(branch) = repo.find_branch(name, git2::BranchType::Local)
                    && let Ok(up) = branch.upstream()
                {
                    out.upstream = up.name().ok().flatten().map(str::to_owned);
                    if let (Some(local), Some(remote)) = (head.target(), up.get().target()) {
                        let (a, b) = repo.graph_ahead_behind(local, remote)?;
                        out.ahead = a;
                        out.behind = b;
                    }
                }
                out.branch = name;
            } else {
                out.detached = true;
            }
        }
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            let head = repo.find_reference("HEAD")?;
            out.branch = head
                .symbolic_target()
                .and_then(|t| t.strip_prefix("refs/heads/"))
                .map(str::to_owned);
        }
        Err(e) => return Err(e.into()),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn groups_renames_and_glyphs() {
        let t = TestRepo::new();
        let s = t.repo.status(false).unwrap();
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.head, None);
        t.write("a.cs", "class A\n{\n    int x;\n    int y;\n}\n");
        t.write("b.cs", "class B { }\n");
        t.write(".gitignore", "bin/\n*.log\n");
        t.commit_all("init");
        t.write("a.cs", "class A\n{\n    int x;\n    int z;\n}\n");
        std::fs::rename(t.path().join("b.cs"), t.path().join("c.cs")).unwrap();
        t.repo.stage(Some(&["b.cs".into(), "c.cs".into()])).unwrap();
        t.write("new.cs", "class N { }\n");
        t.write("bin/out.dll", "x");
        t.write("x.log", "x");
        let s = t.repo.status(true).unwrap();
        assert!(s.head.is_some());
        assert_eq!(
            s.staged,
            vec![Change {
                path: "c.cs".into(),
                kind: ChangeKind::Renamed,
                old_path: Some("b.cs".into())
            }]
        );
        assert_eq!(
            s.unstaged,
            vec![Change {
                path: "a.cs".into(),
                kind: ChangeKind::Modified,
                old_path: None
            }]
        );
        assert_eq!(s.untracked, ["new.cs"]);
        assert_eq!(s.ignored, ["bin/", "x.log"]);
        assert_eq!(s.changed_files(), 3);
        let g = s.glyphs();
        assert_eq!(g.get("a.cs"), Some(FileGlyph::Modified));
        assert_eq!(g.get("c.cs"), Some(FileGlyph::Renamed));
        assert_eq!(g.get("new.cs"), Some(FileGlyph::Untracked));
        assert_eq!(g.get("bin/out.dll"), Some(FileGlyph::Ignored));
        assert_eq!(g.get("x.log"), Some(FileGlyph::Ignored));
        assert_eq!(g.get(".gitignore"), None);
        assert_eq!(g.get("binary.cs"), None);
        t.repo.stage(Some(&["new.cs".into()])).unwrap();
        t.write("new.cs", "class N { int a; }\n");
        assert_eq!(
            t.repo.status(false).unwrap().glyphs().get("new.cs"),
            Some(FileGlyph::Added),
            "a staged add stays added when modified again"
        );
    }

    #[test]
    fn detached_head_and_relative_paths() {
        let t = TestRepo::new();
        t.write("a.txt", "a\n");
        let first = t.commit_all("one");
        t.write("a.txt", "b\n");
        t.commit_all("two");
        let r = t.repo.repository().unwrap();
        r.set_head_detached(first).unwrap();
        let s = t.repo.status(false).unwrap();
        assert!(s.detached);
        assert_eq!(s.branch, None);
        assert_eq!(s.head, Some(first));
        assert_eq!(t.repo.relative("src/../a.txt").unwrap(), "a.txt");
        let abs = t.path().join("dir").join("f.cs");
        assert_eq!(t.repo.relative(&abs.to_string_lossy()).unwrap(), "dir/f.cs");
        assert!(t.repo.relative("/elsewhere/f.cs").is_err());
        assert!(t.repo.relative("../out.cs").is_err());
    }

    #[test]
    fn discover_init_and_no_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("w").join("src");
        std::fs::create_dir_all(&nested).unwrap();
        if Repo::discover(&nested).unwrap().is_some() {
            return; // The temporary folder is inside a repository on this machine.
        }
        let r = Repo::init(&tmp.path().join("w")).unwrap();
        assert_eq!(
            r.workdir().canonicalize().unwrap(),
            tmp.path().join("w").canonicalize().unwrap()
        );
        let found = Repo::discover(&nested).unwrap().unwrap();
        assert_eq!(found.workdir(), r.workdir());
        assert_eq!(found.current_branch().unwrap().as_deref(), Some("main"));
        let again = Repo::init(&nested).unwrap_err();
        assert_eq!(again.kind, crate::ErrorKind::Refused);
    }
}
