//! Stashes: list, push (with a message, untracked files too when asked), apply, pop and drop. Applying a stash that
//! conflicts behaves as a merge: the files get markers, the conflicted paths are listed, and `pop` keeps the stash.

use git2::{StashApplyOptions, StashFlags};

use crate::branches::{MergeOutcome, conflicts};
use crate::commit::Identity;
use crate::{ErrorKind, GitError, Repo, Result};

/// One stash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashInfo {
    pub index: usize,
    pub message: String,
    pub oid: git2::Oid,
}

impl Repo {
    /// The stashes, newest (index 0) first.
    pub fn stashes(&self) -> Result<Vec<StashInfo>> {
        let mut repo = self.repository()?;
        let mut out = Vec::new();
        repo.stash_foreach(|index, message, oid| {
            out.push(StashInfo {
                index,
                message: message.to_owned(),
                oid: *oid,
            });
            true
        })?;
        Ok(out)
    }

    /// Stash the local changes (untracked files too with `include_untracked`).
    pub fn stash_push(
        &self,
        message: Option<&str>,
        include_untracked: bool,
        identity: Option<&Identity>,
    ) -> Result<git2::Oid> {
        let sig = self.signature(identity)?;
        let mut repo = self.repository()?;
        let flags = if include_untracked {
            StashFlags::INCLUDE_UNTRACKED
        } else {
            StashFlags::DEFAULT
        };
        repo.stash_save2(&sig, message, Some(flags)).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                GitError::new(
                    ErrorKind::NothingToCommit,
                    "There are no local changes to stash",
                )
            } else {
                e.into()
            }
        })
    }

    /// Apply stash `index` (and drop it when `pop` and it applied cleanly).
    pub fn stash_apply(&self, index: usize, pop: bool) -> Result<MergeOutcome> {
        let mut repo = self.repository()?;
        let count = self.stashes()?.len();
        if index >= count {
            return Err(GitError::new(
                ErrorKind::NotFound,
                format!("there is no stash {index} ({count} stashes)"),
            ));
        }
        let mut opts = StashApplyOptions::new();
        let mut cb = git2::build::CheckoutBuilder::new();
        cb.safe().allow_conflicts(true).conflict_style_merge(true);
        opts.checkout_options(cb);
        let r = repo.stash_apply(index, Some(&mut opts));
        let paths = conflicts(&repo)?;
        if !paths.is_empty() {
            return Ok(MergeOutcome::Conflicts {
                paths,
                step: 0,
                steps: 0,
            });
        }
        match r {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.code(),
                    git2::ErrorCode::Conflict | git2::ErrorCode::MergeConflict
                ) =>
            {
                return Err(GitError::new(
                    ErrorKind::Dirty,
                    "Your local changes would be overwritten by the stash: commit or stash them first",
                ));
            }
            Err(e) => return Err(e.into()),
        }
        if pop {
            repo.stash_drop(index)?;
        }
        let head = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .unwrap_or_else(git2::Oid::zero);
        Ok(MergeOutcome::Merged(head))
    }

    /// Drop stash `index`.
    pub fn stash_drop(&self, index: usize) -> Result<()> {
        let mut repo = self.repository()?;
        repo.stash_drop(index).map_err(|e| {
            if e.code() == git2::ErrorCode::NotFound {
                GitError::new(ErrorKind::NotFound, format!("there is no stash {index}"))
            } else {
                e.into()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::ErrorKind;
    use crate::branches::MergeOutcome;
    use crate::testutil::TestRepo;

    #[test]
    fn push_list_apply_pop_and_drop() {
        let t = TestRepo::new();
        t.write("a.cs", "a\n");
        t.commit_all("init");
        assert_eq!(
            t.repo.stash_push(None, false, None).unwrap_err().kind,
            ErrorKind::NothingToCommit
        );
        t.write("a.cs", "changed\n");
        t.write("new.cs", "new\n");
        t.repo
            .stash_push(Some("work in progress"), true, None)
            .unwrap();
        assert_eq!(t.read("a.cs"), "a\n");
        assert!(!t.path().join("new.cs").exists(), "untracked files go too");
        let list = t.repo.stashes().unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            list[0].message.contains("work in progress"),
            "{}",
            list[0].message
        );
        assert_eq!(t.repo.status(false).unwrap().stashes, 1);
        assert!(matches!(
            t.repo.stash_apply(0, false).unwrap(),
            MergeOutcome::Merged(_)
        ));
        assert_eq!(t.read("a.cs"), "changed\n");
        assert_eq!(t.repo.stashes().unwrap().len(), 1, "apply keeps it");
        t.repo.discard(None, false).unwrap();
        assert!(matches!(
            t.repo.stash_apply(0, true).unwrap(),
            MergeOutcome::Merged(_)
        ));
        assert!(t.repo.stashes().unwrap().is_empty(), "pop drops it");
        assert_eq!(t.read("new.cs"), "new\n");
        t.repo.stash_push(Some("again"), true, None).unwrap();
        t.repo.stash_drop(0).unwrap();
        assert!(t.repo.stashes().unwrap().is_empty());
        assert_eq!(t.repo.stash_drop(0).unwrap_err().kind, ErrorKind::NotFound);
        assert_eq!(
            t.repo.stash_apply(3, false).unwrap_err().kind,
            ErrorKind::NotFound
        );
    }

    #[test]
    fn a_conflicting_pop_behaves_as_a_merge_and_keeps_the_stash() {
        let t = TestRepo::new();
        t.write("a.cs", "one\ntwo\nthree\n");
        t.commit_all("init");
        t.write("a.cs", "one\nSTASHED\nthree\n");
        t.repo.stash_push(Some("mine"), false, None).unwrap();
        t.write("a.cs", "one\nCOMMITTED\nthree\n");
        t.commit_all("moved on");
        let out = t.repo.stash_apply(0, true).unwrap();
        assert_eq!(
            out,
            MergeOutcome::Conflicts {
                paths: vec!["a.cs".into()],
                step: 0,
                steps: 0
            }
        );
        let text = t.read("a.cs");
        assert!(
            text.contains("<<<<<<<") && text.contains("STASHED") && text.contains("COMMITTED"),
            "{text}"
        );
        assert_eq!(
            t.repo.stashes().unwrap().len(),
            1,
            "a conflicting pop keeps the stash"
        );
        assert_eq!(t.repo.status(false).unwrap().conflicted, ["a.cs"]);
        t.write("a.cs", "one\nBOTH\nthree\n");
        assert_eq!(
            t.repo.stage(Some(&["a.cs".into()])).unwrap().resolved,
            ["a.cs"]
        );
        assert!(t.repo.status(false).unwrap().conflicted.is_empty());
    }
}
