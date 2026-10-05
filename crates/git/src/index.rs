//! The index operations of the Git Changes window: stage (which also marks a conflicted file resolved), unstage
//! and discard (Undo Changes).

use std::collections::BTreeSet;

use git2::build::CheckoutBuilder;

use crate::{ErrorKind, GitError, Repo, Result};

/// What [`Repo::stage`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Staged {
    pub staged: Vec<String>,
    /// The conflicted files it marked resolved.
    pub resolved: Vec<String>,
}

/// What [`Repo::discard`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discarded {
    pub restored: Vec<String>,
    /// Untracked files deleted.
    pub deleted: Vec<String>,
}

impl Repo {
    fn relative_all(&self, paths: &[String]) -> Result<Vec<String>> {
        paths.iter().map(|p| self.relative(p)).collect()
    }

    /// Stage `paths` (or every change when `None`): a file's working-tree content goes to the index, a deleted
    /// file's removal is staged, a conflicted file is marked resolved.
    pub fn stage(&self, paths: Option<&[String]>) -> Result<Staged> {
        let repo = self.repository()?;
        let mut index = repo.index()?;
        let conflicted: BTreeSet<String> = index
            .conflicts()?
            .filter_map(|c| c.ok())
            .filter_map(|c| c.our.or(c.their).or(c.ancestor))
            .map(|e| String::from_utf8_lossy(&e.path).into_owned())
            .collect();
        let before = self.status(false)?;
        let targets: Vec<String> = match paths {
            Some(p) => self.relative_all(p)?,
            None => before
                .unstaged
                .iter()
                .flat_map(|c| std::iter::once(c.path.clone()).chain(c.old_path.clone()))
                .chain(before.untracked.iter().cloned())
                .chain(before.conflicted.iter().cloned())
                .collect(),
        };
        let mut staged = Vec::new();
        for rel in &targets {
            let abs = self.absolute(rel);
            if abs.is_dir() {
                index.add_all([rel.as_str()], git2::IndexAddOption::DEFAULT, None)?;
                index.update_all([rel.as_str()], None)?;
            } else if abs.exists() {
                index.add_path(std::path::Path::new(rel))?;
            } else {
                index.remove_path(std::path::Path::new(rel))?;
            }
            staged.push(rel.clone());
        }
        index.write()?;
        let resolved = staged
            .iter()
            .filter(|p| conflicted.contains(*p))
            .cloned()
            .collect();
        Ok(Staged { staged, resolved })
    }

    /// Unstage `paths` (or every staged change when `None`): their index entries go back to HEAD's, or are removed
    /// on an unborn branch. A staged rename unstages both of its paths.
    pub fn unstage(&self, paths: Option<&[String]>) -> Result<Vec<String>> {
        let repo = self.repository()?;
        let before = self.status(false)?;
        let mut targets: Vec<String> = match paths {
            Some(p) => self.relative_all(p)?,
            None => before.staged.iter().map(|c| c.path.clone()).collect(),
        };
        for c in &before.staged {
            if targets.contains(&c.path)
                && let Some(old) = &c.old_path
                && !targets.contains(old)
            {
                targets.push(old.clone());
            }
        }
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        match repo.head() {
            Ok(head) => {
                let commit = head.peel_to_commit()?;
                repo.reset_default(Some(commit.as_object()), targets.iter())?;
            }
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
                let mut index = repo.index()?;
                for t in &targets {
                    index.remove_path(std::path::Path::new(t))?;
                }
                index.write()?;
            }
            Err(e) => return Err(e.into()),
        }
        Ok(targets)
    }

    /// Undo Changes on `paths` (or every change when `None`): the working-tree files go back to the index's content
    /// and untracked files are deleted; with `staged`, the index entries go back to HEAD's first.
    pub fn discard(&self, paths: Option<&[String]>, staged: bool) -> Result<Discarded> {
        let repo = self.repository()?;
        let rel = match paths {
            Some(p) => Some(self.relative_all(p)?),
            None => None,
        };
        let status = self.status(false)?;
        if !status.conflicted.is_empty()
            && rel
                .as_ref()
                .is_none_or(|r| r.iter().any(|p| status.conflicted.contains(p)))
        {
            return Err(GitError::new(
                ErrorKind::Conflicted,
                "Conflicted files cannot be undone: resolve and stage them, or abort the operation",
            )
            .with_paths(status.conflicted.clone()));
        }
        if staged {
            self.unstage(rel.as_deref())?;
        }
        let status = self.status(false)?;
        let wanted = |p: &str| rel.as_ref().is_none_or(|r| r.iter().any(|x| x == p));
        let mut out = Discarded::default();
        for p in &status.untracked {
            if wanted(p) {
                let abs = self.absolute(p);
                if abs.is_dir() {
                    std::fs::remove_dir_all(&abs)?;
                } else if abs.exists() {
                    std::fs::remove_file(&abs)?;
                }
                out.deleted.push(p.clone());
            }
        }
        let restore: Vec<String> = status
            .unstaged
            .iter()
            .flat_map(|c| std::iter::once(c.path.clone()).chain(c.old_path.clone()))
            .filter(|p| wanted(p))
            .collect();
        if !restore.is_empty() {
            let mut cb = CheckoutBuilder::new();
            cb.force().update_index(false);
            for p in &restore {
                cb.path(p);
            }
            repo.checkout_index(None, Some(&mut cb))?;
            // A renamed file's new path is not in the index: it was untracked, deleted above when asked.
            for c in &status.unstaged {
                if c.kind == crate::ChangeKind::Renamed && wanted(&c.path) {
                    let abs = self.absolute(&c.path);
                    if abs.exists()
                        && repo
                            .index()?
                            .get_path(std::path::Path::new(&c.path), 0)
                            .is_none()
                    {
                        std::fs::remove_file(abs)?;
                    }
                }
            }
            out.restored = restore;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::TestRepo;
    use crate::{ChangeKind, ErrorKind};

    #[test]
    fn stage_unstage_and_discard() {
        let t = TestRepo::new();
        t.write("a.cs", "one\n");
        t.write("gone.cs", "bye\n");
        t.commit_all("init");
        t.write("a.cs", "two\n");
        std::fs::remove_file(t.path().join("gone.cs")).unwrap();
        t.write("dir/new.cs", "new\n");

        let s = t
            .repo
            .stage(Some(&["a.cs".into(), "gone.cs".into()]))
            .unwrap();
        assert_eq!(s.staged, ["a.cs", "gone.cs"]);
        let st = t.repo.status(false).unwrap();
        assert_eq!(
            st.staged
                .iter()
                .map(|c| (c.path.as_str(), c.kind))
                .collect::<Vec<_>>(),
            [
                ("a.cs", ChangeKind::Modified),
                ("gone.cs", ChangeKind::Deleted)
            ]
        );
        assert_eq!(st.untracked, ["dir/new.cs"]);

        assert_eq!(t.repo.unstage(Some(&["a.cs".into()])).unwrap(), ["a.cs"]);
        let st = t.repo.status(false).unwrap();
        assert_eq!(st.staged.len(), 1);
        assert_eq!(st.unstaged[0].path, "a.cs");

        // Stage All and Unstage All.
        t.repo.stage(None).unwrap();
        let st = t.repo.status(false).unwrap();
        assert_eq!(st.staged.len(), 3);
        assert!(st.unstaged.is_empty() && st.untracked.is_empty());
        t.repo.unstage(None).unwrap();
        let st = t.repo.status(false).unwrap();
        assert!(st.staged.is_empty());
        assert_eq!(st.untracked, ["dir/new.cs"]);

        // Undo Changes: a.cs back to the index, the untracked file deleted.
        let d = t
            .repo
            .discard(Some(&["a.cs".into(), "dir/new.cs".into()]), false)
            .unwrap();
        assert_eq!(d.restored, ["a.cs"]);
        assert_eq!(d.deleted, ["dir/new.cs"]);
        assert_eq!(t.read("a.cs"), "one\n");
        assert!(!t.path().join("dir/new.cs").exists());

        // A staged change discarded with `staged` goes back to HEAD.
        t.write("a.cs", "three\n");
        t.repo.stage(Some(&["a.cs".into()])).unwrap();
        t.repo.discard(None, true).unwrap();
        assert_eq!(t.read("a.cs"), "one\n");
        assert_eq!(t.read("gone.cs"), "bye\n");
        let st = t.repo.status(false).unwrap();
        assert!(st.staged.is_empty() && st.unstaged.is_empty() && st.untracked.is_empty());
    }

    #[test]
    fn unstage_on_an_unborn_branch_and_absolute_paths() {
        let t = TestRepo::new();
        t.write("a.cs", "a\n");
        let abs = t.path().join("a.cs").to_string_lossy().into_owned();
        t.repo.stage(Some(std::slice::from_ref(&abs))).unwrap();
        assert_eq!(
            t.repo.status(false).unwrap().staged[0].kind,
            ChangeKind::Added
        );
        t.repo.unstage(Some(&[abs])).unwrap();
        assert_eq!(t.repo.status(false).unwrap().untracked, ["a.cs"]);
        let e = t
            .repo
            .stage(Some(&[TestRepo::elsewhere("here.cs")]))
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound);
    }
}
