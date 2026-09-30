//! Git integration over libgit2 (PLAN.md D2, 4.8).
//!
//! Status and branch queries for the status bar, Solution Explorer decorations
//! and agents (git status is agent-visible state, PLAN.md 5.4).

use std::path::Path;

pub use git2::{Error, Status as StatusFlags};
use git2::{Repository, StatusOptions};

/// Status of every changed or untracked path (ignored files excluded),
/// relative to the repository root, sorted by path.
pub fn repo_status(path: &Path) -> Result<Vec<(String, StatusFlags)>, Error> {
    let repo = Repository::discover(path)?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts))?;
    let mut out: Vec<_> = statuses
        .iter()
        .filter_map(|e| e.path().map(|p| (p.to_owned(), e.status())))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The checked-out branch's short name, or `None` when HEAD is detached.
/// For a fresh repository with no commits, returns the unborn branch name.
pub fn current_branch(path: &Path) -> Result<Option<String>, Error> {
    let repo = Repository::discover(path)?;
    match repo.head() {
        Ok(head) if head.is_branch() => Ok(head.shorthand().map(str::to_owned)),
        Ok(_) => Ok(None),
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            let head = repo.find_reference("HEAD")?;
            Ok(head
                .symbolic_target()
                .and_then(|t| t.strip_prefix("refs/heads/"))
                .map(str::to_owned))
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;
    use std::fs;

    fn init_repo(dir: &Path) -> Repository {
        let repo = Repository::init(dir).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        repo
    }

    #[test]
    fn status_and_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path());
        assert_eq!(current_branch(tmp.path()).unwrap().as_deref(), Some("main"));

        fs::write(tmp.path().join("committed.cs"), "class A {}\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("committed.cs")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        assert!(repo_status(tmp.path()).unwrap().is_empty());

        fs::write(tmp.path().join("committed.cs"), "class B {}\n").unwrap();
        fs::write(tmp.path().join("new.cs"), "class C {}\n").unwrap();
        let status = repo_status(tmp.path()).unwrap();
        assert_eq!(
            status,
            vec![
                ("committed.cs".to_owned(), StatusFlags::WT_MODIFIED),
                ("new.cs".to_owned(), StatusFlags::WT_NEW),
            ]
        );

        repo.branch(
            "feature",
            &repo.head().unwrap().peel_to_commit().unwrap(),
            false,
        )
        .unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        assert_eq!(
            current_branch(tmp.path()).unwrap().as_deref(),
            Some("feature")
        );
    }

    #[test]
    fn not_a_repo_errors() {
        let tmp = tempfile::tempdir().unwrap();
        // Guard against a parent directory being a repo: use a path libgit2 cannot discover upward from.
        let nested = tmp.path().join("x");
        fs::create_dir(&nested).unwrap();
        if Repository::discover(&nested).is_err() {
            assert!(repo_status(&nested).is_err());
        }
    }
}
