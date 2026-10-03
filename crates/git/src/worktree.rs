//! Worktrees: the list with each one's branch and path, add (under `<repo>/.worktrees/<name>` by default, Eludite's
//! own convention from `docs/briefs/README.md`), and remove (only a clean one, unless `force`).

use std::path::{Path, PathBuf};

use git2::{WorktreeAddOptions, WorktreePruneOptions};

use crate::{ErrorKind, GitError, Repo, Result};

/// The folder new worktrees go in, under the main working tree.
pub const WORKTREES_DIR: &str = ".worktrees";

/// One worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// `main` for the main working tree; else the worktree's name.
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<git2::Oid>,
    pub main: bool,
    pub locked: bool,
}

impl Repo {
    /// The main working tree's root (this one's, unless this is a linked worktree).
    pub fn main_workdir(&self) -> PathBuf {
        let common = self.common_dir();
        if common.file_name().is_some_and(|n| n == ".git") {
            common
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.workdir().to_path_buf())
        } else {
            self.workdir().to_path_buf()
        }
    }

    /// Every worktree, the main one first.
    pub fn worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let repo = self.repository()?;
        let main_path = self.main_workdir();
        let main = Repo::open(&main_path)?;
        let info = |r: &Repo| -> (Option<String>, Option<git2::Oid>) {
            r.repository()
                .ok()
                .and_then(|x| crate::status::head_info(&x).ok())
                .map(|s| (s.branch, s.head))
                .unwrap_or((None, None))
        };
        let (branch, head) = info(&main);
        let mut out = vec![WorktreeInfo {
            name: "main".into(),
            path: main_path,
            branch,
            head,
            main: true,
            locked: false,
        }];
        for name in repo.worktrees()?.iter().flatten() {
            let wt = repo.find_worktree(name)?;
            let path = wt.path().to_path_buf();
            let (branch, head) = Repo::open(&path).map(|r| info(&r)).unwrap_or((None, None));
            out.push(WorktreeInfo {
                name: name.to_owned(),
                path,
                branch,
                head,
                main: false,
                locked: matches!(wt.is_locked(), Ok(git2::WorktreeLockStatus::Locked(_))),
            });
        }
        Ok(out)
    }

    /// Add worktree `name` at `path` (default `<main working tree>/.worktrees/<name>`), checking out `branch`, or a
    /// new branch `name` at HEAD.
    pub fn add_worktree(
        &self,
        name: &str,
        path: Option<&Path>,
        branch: Option<&str>,
    ) -> Result<WorktreeInfo> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!("`{name}` is not a worktree name (letters, digits, `.`, `_` and `-`)"),
            ));
        }
        let repo = self.repository()?;
        let path = path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.main_workdir().join(WORKTREES_DIR).join(name));
        if path.exists() {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!("{} already exists", path.display()),
            ));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let branch_name = branch.unwrap_or(name);
        let reference = match repo.find_branch(branch_name, git2::BranchType::Local) {
            Ok(b) => b.into_reference(),
            Err(_) if branch.is_none() => {
                let head = repo.head()?.peel_to_commit()?;
                repo.branch(name, &head, false)?.into_reference()
            }
            Err(_) => {
                return Err(GitError::new(
                    ErrorKind::NotFound,
                    format!("there is no branch `{branch_name}`"),
                ));
            }
        };
        let mut opts = WorktreeAddOptions::new();
        opts.reference(Some(&reference));
        repo.worktree(name, &path, Some(&opts))?;
        self.worktrees()?
            .into_iter()
            .find(|w| w.name == name)
            .ok_or_else(|| GitError::new(ErrorKind::Git, "the new worktree is not listed"))
    }

    /// Remove worktree `name` (its folder and its administrative files); one with changes only with `force`.
    pub fn remove_worktree(&self, name: &str, force: bool) -> Result<()> {
        let repo = self.repository()?;
        let wt = repo.find_worktree(name).map_err(|_| {
            GitError::new(
                ErrorKind::NotFound,
                format!("there is no worktree `{name}`"),
            )
        })?;
        if !force && let Ok(r) = Repo::open(wt.path()) {
            let s = r.status(false)?;
            let mut paths: Vec<String> = s
                .staged
                .iter()
                .chain(&s.unstaged)
                .map(|c| c.path.clone())
                .chain(s.untracked.iter().cloned())
                .chain(s.conflicted.iter().cloned())
                .collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                return Err(GitError::new(
                    ErrorKind::Dirty,
                    format!(
                        "The worktree `{name}` has changes ({}): commit or discard them, or remove it with force",
                        paths.join(", ")
                    ),
                )
                .with_paths(paths));
            }
        }
        let mut opts = WorktreePruneOptions::new();
        opts.valid(true).working_tree(true).locked(force);
        wt.prune(Some(&mut opts))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::ErrorKind;
    use crate::testutil::TestRepo;

    #[test]
    fn add_list_and_remove_worktrees() {
        let t = TestRepo::new();
        t.write(".gitignore", ".worktrees/\n");
        t.write("a.cs", "a\n");
        t.commit_all("init");
        let w = t.repo.add_worktree("fix", None, None).unwrap();
        assert_eq!(w.branch.as_deref(), Some("fix"));
        assert!(w.path.ends_with(".worktrees/fix"));
        assert!(w.path.join("a.cs").exists());
        let list = t.repo.worktrees().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].main && list[0].branch.as_deref() == Some("main"));
        assert_eq!(list[1].name, "fix");
        assert!(t.repo.add_worktree("fix", None, None).is_err(), "exists");
        assert!(t.repo.add_worktree("bad name", None, None).is_err());
        // A change in it refuses the removal.
        std::fs::write(w.path.join("a.cs"), "changed\n").unwrap();
        let e = t.repo.remove_worktree("fix", false).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Dirty);
        assert_eq!(e.paths, ["a.cs"]);
        t.repo.remove_worktree("fix", true).unwrap();
        assert!(!w.path.exists());
        assert_eq!(t.repo.worktrees().unwrap().len(), 1);
        // An existing branch at a given path; a clean removal.
        let at = t.dir.path().join("elsewhere");
        let w = t
            .repo
            .add_worktree("other", Some(&at), Some("fix"))
            .unwrap();
        assert_eq!(w.branch.as_deref(), Some("fix"));
        t.repo.remove_worktree("other", false).unwrap();
        assert_eq!(
            t.repo.remove_worktree("other", false).unwrap_err().kind,
            ErrorKind::NotFound
        );
    }
}
