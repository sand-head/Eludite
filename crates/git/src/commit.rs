//! Commit, Commit All and Amend, with the identity rule: `user.name` and `user.email` from the repository's config,
//! then the global config, then a fallback (the settings `git.userName` and `git.userEmail`), else a refusal naming
//! `git config user.name` and the settings. A commit during a merge gets the merged commits as further parents and
//! ends the merge; one during a cherry-pick keeps the picked commit's author and ends the pick. Commits are not
//! signed.

use git2::{Config, ConfigLevel, Repository, Signature};

use crate::{ErrorKind, GitError, GlobalConfig, Repo, Result, summary_of};

/// A name and an email.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
}

/// What to commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitOptions {
    pub message: String,
    /// Commit All: stage every change first.
    pub all: bool,
    pub amend: bool,
    /// The author, when not the committer.
    pub author: Option<Identity>,
    /// The identity when the configs have none (the settings).
    pub fallback: Option<Identity>,
}

/// The new commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub oid: git2::Oid,
    pub summary: String,
    pub branch: Option<String>,
    pub amended: bool,
    /// Files it changed against its first parent.
    pub files: usize,
}

/// The refusal when no identity is configured.
pub fn identity_refusal() -> GitError {
    GitError::new(
        ErrorKind::Identity,
        "Commits need a name and an email. Run `git config --global user.name \"Your Name\"` and `git config \
         --global user.email you@example.com` (or `git config user.name` in this repository), or set git.userName \
         and git.userEmail in Tools > Options > Source Control > Git Global Settings.",
    )
}

impl Repo {
    /// The global configuration this handle reads (see [`GlobalConfig`]).
    fn global(&self) -> Option<Config> {
        match self.global_config() {
            GlobalConfig::User => Config::open_default().ok(),
            GlobalConfig::Files(files) => {
                let mut c = Config::new().ok()?;
                for f in files {
                    c.add_file(f, ConfigLevel::Global, false).ok()?;
                }
                Some(c)
            }
        }
    }

    /// The committer identity: the repository's config, the global config, then `fallback`; else the refusal.
    pub fn identity(&self, fallback: Option<&Identity>) -> Result<Identity> {
        let repo = self.repository()?;
        let local = repo
            .config()
            .ok()
            .and_then(|c| c.open_level(ConfigLevel::Local).ok());
        let global = self.global();
        let get = |key: &str| -> Option<String> {
            [local.as_ref(), global.as_ref()]
                .into_iter()
                .flatten()
                .find_map(|c| c.get_string(key).ok())
                .filter(|v| !v.trim().is_empty())
        };
        let name = get("user.name").or_else(|| {
            fallback
                .map(|f| f.name.clone())
                .filter(|n| !n.trim().is_empty())
        });
        let email = get("user.email").or_else(|| {
            fallback
                .map(|f| f.email.clone())
                .filter(|e| !e.trim().is_empty())
        });
        match (name, email) {
            (Some(name), Some(email)) => Ok(Identity { name, email }),
            _ => Err(identity_refusal()),
        }
    }

    /// The committer signature, now.
    pub(crate) fn signature(&self, fallback: Option<&Identity>) -> Result<Signature<'static>> {
        let id = self.identity(fallback)?;
        Ok(Signature::now(&id.name, &id.email)?)
    }

    /// Commit the index (everything with `all`) on the current branch.
    pub fn commit(&self, opts: &CommitOptions) -> Result<Committed> {
        let message = opts.message.trim_end();
        if message.trim().is_empty() {
            return Err(GitError::new(ErrorKind::Refused, "Enter a commit message"));
        }
        let committer = self.signature(opts.fallback.as_ref())?;
        let repo = self.repository()?;
        let state = repo.state();
        if matches!(
            state,
            git2::RepositoryState::Rebase
                | git2::RepositoryState::RebaseInteractive
                | git2::RepositoryState::RebaseMerge
        ) {
            return Err(GitError::new(
                ErrorKind::Refused,
                "A rebase is in progress: stage the resolved files and continue it (eludite.git.rebase with \
                 `continue`), or abort it",
            ));
        }
        if opts.all {
            self.stage(None)?;
        }
        let mut index = repo.index()?;
        if index.has_conflicts() {
            let paths: Vec<String> = index
                .conflicts()?
                .filter_map(|c| c.ok())
                .filter_map(|c| c.our.or(c.their).or(c.ancestor))
                .map(|e| String::from_utf8_lossy(&e.path).into_owned())
                .collect();
            return Err(GitError::new(
                ErrorKind::Conflicted,
                format!(
                    "Resolve the conflicts first and stage the files: {}",
                    paths.join(", ")
                ),
            )
            .with_paths(paths));
        }
        let tree = repo.find_tree(index.write_tree()?)?;
        let head = match repo.head() {
            Ok(h) => Some(h.peel_to_commit()?),
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
            Err(e) => return Err(e.into()),
        };
        let message = format!("{message}\n");
        let branch = crate::status::head_info(&repo)?.branch;
        if opts.amend {
            let Some(head) = head else {
                return Err(GitError::new(
                    ErrorKind::Refused,
                    "There is no commit to amend",
                ));
            };
            let author = match &opts.author {
                Some(a) => Some(Signature::now(&a.name, &a.email)?),
                None => None,
            };
            let oid = head.amend(
                Some("HEAD"),
                author.as_ref(),
                Some(&committer),
                None,
                Some(&message),
                Some(&tree),
            )?;
            let files = changed_files(&repo, head.parent(0).ok().as_ref(), &tree)?;
            return Ok(Committed {
                oid,
                summary: summary_of(&message),
                branch,
                amended: true,
                files,
            });
        }
        let mut parents = Vec::new();
        if let Some(h) = &head {
            parents.push(h.clone());
        }
        let mut author = match &opts.author {
            Some(a) => Signature::now(&a.name, &a.email)?,
            None => committer.clone(),
        };
        let mut merging = false;
        match state {
            git2::RepositoryState::Merge => {
                merging = true;
                let text = std::fs::read_to_string(self.git_dir().join("MERGE_HEAD"))?;
                for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    let oid = git2::Oid::from_str(line)?;
                    parents.push(repo.find_commit(oid)?);
                }
            }
            git2::RepositoryState::CherryPick => {
                merging = true;
                if opts.author.is_none()
                    && let Ok(text) =
                        std::fs::read_to_string(self.git_dir().join("CHERRY_PICK_HEAD"))
                    && let Ok(oid) = git2::Oid::from_str(text.trim())
                    && let Ok(picked) = repo.find_commit(oid)
                {
                    let a = picked.author();
                    author = Signature::new(
                        a.name().unwrap_or_default(),
                        a.email().unwrap_or_default(),
                        &a.when(),
                    )?;
                }
            }
            _ => {}
        }
        let unchanged = match &head {
            Some(h) => h.tree_id() == tree.id(),
            None => tree.is_empty(),
        };
        if unchanged && !merging {
            return Err(GitError::new(
                ErrorKind::NothingToCommit,
                if opts.all {
                    "There are no changes to commit"
                } else {
                    "Nothing is staged: stage changes first, or Commit All"
                },
            ));
        }
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        let oid = repo.commit(
            Some("HEAD"),
            &author,
            &committer,
            &message,
            &tree,
            &parent_refs,
        )?;
        if merging {
            repo.cleanup_state()?;
        }
        let files = changed_files(&repo, head.as_ref(), &tree)?;
        Ok(Committed {
            oid,
            summary: summary_of(&message),
            branch,
            amended: false,
            files,
        })
    }
}

fn changed_files(
    repo: &Repository,
    parent: Option<&git2::Commit<'_>>,
    tree: &git2::Tree<'_>,
) -> Result<usize> {
    let old = match parent {
        Some(p) => Some(p.tree()?),
        None => None,
    };
    Ok(repo
        .diff_tree_to_tree(old.as_ref(), Some(tree), None)?
        .deltas()
        .len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn commit_all_amend_and_nothing_to_commit() {
        let t = TestRepo::new();
        t.write("a.cs", "a\n");
        let c = t
            .repo
            .commit(&CommitOptions {
                message: "Add a\n\nmore".into(),
                all: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(c.summary, "Add a");
        assert_eq!(c.branch.as_deref(), Some("main"));
        assert_eq!(c.files, 1);
        let e = t
            .repo
            .commit(&CommitOptions {
                message: "again".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::NothingToCommit);
        t.write("b.cs", "b\n");
        t.repo.stage(Some(&["b.cs".into()])).unwrap();
        let amended = t
            .repo
            .commit(&CommitOptions {
                message: "Add a and b".into(),
                amend: true,
                ..Default::default()
            })
            .unwrap();
        assert!(amended.amended);
        assert_eq!(amended.files, 2);
        let repo = t.repo.repository().unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.id(), amended.oid);
        assert_eq!(head.parent_count(), 0, "amend replaced the first commit");
        assert_eq!(head.message(), Some("Add a and b\n"));
        assert_eq!(head.committer().name(), Some("Test"));
    }

    #[test]
    fn the_identity_rule_and_its_refusal() {
        let t = TestRepo::new();
        t.write("a.cs", "a\n");
        // The repository's config wins over the fallback.
        let fallback = Identity {
            name: "Setting".into(),
            email: "setting@example.com".into(),
        };
        assert_eq!(t.repo.identity(Some(&fallback)).unwrap().name, "Test");
        let repo = t.repo.repository().unwrap();
        let mut cfg = repo
            .config()
            .unwrap()
            .open_level(ConfigLevel::Local)
            .unwrap();
        cfg.remove("user.name").unwrap();
        cfg.remove("user.email").unwrap();
        // No repository config and no global config: the refusal names git config and the settings.
        let e = t
            .repo
            .commit(&CommitOptions {
                message: "x".into(),
                all: true,
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Identity);
        assert!(e.message.contains("git config --global user.name"), "{e}");
        assert!(e.message.contains("git.userName"), "{e}");
        // A global file is next.
        let global = t.path().join("global.gitconfig");
        std::fs::write(
            &global,
            "[user]\n\tname = Global\n\temail = global@example.com\n",
        )
        .unwrap();
        let g = t
            .repo
            .clone()
            .with_global_config(GlobalConfig::Files(vec![global]));
        assert_eq!(g.identity(Some(&fallback)).unwrap().name, "Global");
        // Then the settings.
        let c = t
            .repo
            .commit(&CommitOptions {
                message: "From the settings".into(),
                all: true,
                fallback: Some(fallback),
                ..Default::default()
            })
            .unwrap();
        let commit = repo.find_commit(c.oid).unwrap();
        assert_eq!(commit.author().email(), Some("setting@example.com"));
    }
}
