//! Branches and the operations that move them: the branch list with upstreams and ahead/behind, Delete, Checkout
//! (libgit2's safe checkout: local changes the switch would overwrite refuse it, naming them), New Branch, Merge,
//! Rebase, Cherry-pick and Reset. Merge, rebase and cherry-pick stop at the first conflict with the conflicted paths
//! listed and their files written with conflict markers; staging a file resolves it.

use git2::build::CheckoutBuilder;
use git2::{AnnotatedCommit, BranchType, Oid, Repository};

use crate::commit::Identity;
use crate::{ErrorKind, GitError, Repo, Result, summary_of};

/// A branch of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// `main`, or `origin/main`.
    pub name: String,
    pub remote: bool,
    pub head: bool,
    pub oid: Oid,
    pub summary: String,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

/// A tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagInfo {
    pub name: String,
    pub oid: Oid,
}

/// Where a checkout left HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedOut {
    pub branch: Option<String>,
    pub oid: Oid,
    pub created: bool,
}

/// How a merge, pull, rebase or cherry-pick went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    UpToDate,
    FastForward(Oid),
    Merged(Oid),
    Rebased(Oid),
    Picked(Oid),
    Aborted(Oid),
    /// Stopped at a conflict: the conflicted paths, and for a rebase the step (1-based) and the steps.
    Conflicts {
        paths: Vec<String>,
        step: usize,
        steps: usize,
    },
}

/// The reset modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

/// The conflicted paths of the index.
pub(crate) fn conflicts(repo: &Repository) -> Result<Vec<String>> {
    let index = repo.index()?;
    if !index.has_conflicts() {
        return Ok(Vec::new());
    }
    let mut out: Vec<String> = index
        .conflicts()?
        .filter_map(|c| c.ok())
        .filter_map(|c| c.our.or(c.their).or(c.ancestor))
        .map(|e| String::from_utf8_lossy(&e.path).into_owned())
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

fn safe_checkout<'a>() -> CheckoutBuilder<'a> {
    let mut cb = CheckoutBuilder::new();
    cb.safe();
    cb
}

/// A checkout refused because local changes stand in the way.
fn dirty(paths: Vec<String>, what: &str) -> GitError {
    GitError::new(
        ErrorKind::Dirty,
        format!(
            "Your local changes to these files would be overwritten by {what}: {}. Commit or stash them first{}.",
            paths.join(", "),
            if what == "checkout" {
                ", or check out with force"
            } else {
                ""
            }
        ),
    )
    .with_paths(paths)
}

impl Repo {
    /// The local branches (and remote ones with `remote`) and the tags.
    pub fn branches(&self, remote: bool) -> Result<(Vec<BranchInfo>, Vec<TagInfo>)> {
        let repo = self.repository()?;
        let mut out = Vec::new();
        let kind = if remote {
            None
        } else {
            Some(BranchType::Local)
        };
        for b in repo.branches(kind)? {
            let (b, t) = b?;
            let Some(name) = b.name()?.map(str::to_owned) else {
                continue;
            };
            if t == BranchType::Remote && name.ends_with("/HEAD") {
                continue;
            }
            let Ok(commit) = b.get().peel_to_commit() else {
                continue;
            };
            let mut info = BranchInfo {
                name,
                remote: t == BranchType::Remote,
                head: b.is_head(),
                oid: commit.id(),
                summary: summary_of(commit.message().unwrap_or_default()),
                upstream: None,
                ahead: 0,
                behind: 0,
            };
            if t == BranchType::Local
                && let Ok(up) = b.upstream()
            {
                info.upstream = up.name().ok().flatten().map(str::to_owned);
                if let Some(u) = up.get().target() {
                    let (a, bh) = repo.graph_ahead_behind(commit.id(), u)?;
                    info.ahead = a;
                    info.behind = bh;
                }
            }
            out.push(info);
        }
        out.sort_by(|a, b| (a.remote, &a.name).cmp(&(b.remote, &b.name)));
        let mut tags = Vec::new();
        for name in repo.tag_names(None)?.iter().flatten() {
            if let Ok(obj) = repo.revparse_single(&format!("refs/tags/{name}"))
                && let Ok(c) = obj.peel_to_commit()
            {
                tags.push(TagInfo {
                    name: name.to_owned(),
                    oid: c.id(),
                });
            }
        }
        Ok((out, tags))
    }

    /// Delete local branch `name`; one not merged into HEAD only with `force`.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        let repo = self.repository()?;
        let mut b = repo.find_branch(name, BranchType::Local).map_err(|_| {
            GitError::new(ErrorKind::NotFound, format!("there is no branch `{name}`"))
        })?;
        if b.is_head() {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!("`{name}` is checked out: check out another branch first"),
            ));
        }
        let tip = b.get().peel_to_commit()?.id();
        if !force
            && let Ok(head) = repo.head().and_then(|h| h.peel_to_commit())
            && head.id() != tip
            && !repo.graph_descendant_of(head.id(), tip)?
        {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!(
                    "The branch `{name}` is not fully merged: delete it with force to lose its commits"
                ),
            ));
        }
        b.delete()?;
        Ok(())
    }

    /// Check out `name` (a branch, a remote branch, a tag or a commit), creating branch `name` at `start_point`
    /// first when `create`. Local changes the switch would overwrite refuse it unless `force`.
    pub fn checkout(
        &self,
        name: &str,
        create: bool,
        start_point: Option<&str>,
        force: bool,
    ) -> Result<CheckedOut> {
        let repo = self.repository()?;
        let mut created = false;
        let branch_ref: Option<String> = if create {
            let start = repo
                .revparse_single(start_point.unwrap_or("HEAD"))
                .map_err(|e| {
                    GitError::new(
                        ErrorKind::NotFound,
                        format!("unknown start point: {}", e.message()),
                    )
                })?
                .peel_to_commit()?;
            if repo.find_branch(name, BranchType::Local).is_ok() {
                return Err(GitError::new(
                    ErrorKind::Refused,
                    format!("A branch named `{name}` already exists"),
                ));
            }
            repo.branch(name, &start, false)?;
            created = true;
            Some(format!("refs/heads/{name}"))
        } else if repo.find_branch(name, BranchType::Local).is_ok() {
            Some(format!("refs/heads/{name}"))
        } else if let Ok(remote) = repo.find_branch(name, BranchType::Remote) {
            // A remote branch: check out a local tracking branch of its name.
            let local = name.split_once('/').map_or(name, |(_, b)| b).to_owned();
            if repo.find_branch(&local, BranchType::Local).is_err() {
                let commit = remote.get().peel_to_commit()?;
                let mut b = repo.branch(&local, &commit, false)?;
                b.set_upstream(Some(name))?;
                created = true;
            }
            Some(format!("refs/heads/{local}"))
        } else {
            None
        };
        let target = match &branch_ref {
            Some(r) => repo.revparse_single(r)?,
            None => repo.revparse_single(name).map_err(|e| {
                GitError::new(
                    ErrorKind::NotFound,
                    format!("there is no branch or revision `{name}`: {}", e.message()),
                )
            })?,
        };
        let commit = target.peel_to_commit()?;
        let mut blocked = Vec::new();
        let mut cb = CheckoutBuilder::new();
        if force {
            cb.force();
        } else {
            cb.safe();
            cb.notify_on(git2::CheckoutNotificationType::CONFLICT);
            cb.notify(|_, path, _, _, _| {
                if let Some(p) = path {
                    blocked.push(p.to_string_lossy().replace('\\', "/"));
                }
                true
            });
        }
        let result = repo.checkout_tree(commit.as_object(), Some(&mut cb));
        drop(cb);
        if let Err(e) = result {
            if created && let Ok(mut b) = repo.find_branch(name, BranchType::Local) {
                let _ = b.delete();
            }
            if !blocked.is_empty() || e.code() == git2::ErrorCode::Conflict {
                return Err(dirty(blocked, "checkout"));
            }
            return Err(e.into());
        }
        match &branch_ref {
            Some(r) => repo.set_head(r)?,
            None => repo.set_head_detached(commit.id())?,
        }
        Ok(CheckedOut {
            branch: branch_ref.map(|r| r.trim_start_matches("refs/heads/").to_owned()),
            oid: commit.id(),
            created,
        })
    }

    fn annotated<'r>(&self, repo: &'r Repository, rev: &str) -> Result<AnnotatedCommit<'r>> {
        if let Ok(r) = repo.resolve_reference_from_short_name(rev) {
            return Ok(repo.reference_to_annotated_commit(&r)?);
        }
        let obj = repo.revparse_single(rev).map_err(|e| {
            GitError::new(
                ErrorKind::NotFound,
                format!("there is no branch or revision `{rev}`: {}", e.message()),
            )
        })?;
        Ok(repo.find_annotated_commit(obj.peel_to_commit()?.id())?)
    }

    fn refuse_staged(&self, what: &str) -> Result<()> {
        let s = self.status(false)?;
        if !s.conflicted.is_empty() {
            return Err(GitError::new(
                ErrorKind::Conflicted,
                "Resolve the conflicts in progress first",
            )
            .with_paths(s.conflicted));
        }
        if !s.staged.is_empty() {
            return Err(dirty(s.staged.into_iter().map(|c| c.path).collect(), what));
        }
        Ok(())
    }

    /// Merge `rev` into the current branch: a fast-forward when possible (unless `no_ff`), else a merge commit; on a
    /// conflict, stop with the paths (the merge stays in progress).
    pub fn merge(
        &self,
        rev: &str,
        no_ff: bool,
        message: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<MergeOutcome> {
        let repo = self.repository()?;
        let their = self.annotated(&repo, rev)?;
        let (analysis, _) = repo.merge_analysis(&[&their])?;
        if analysis.is_up_to_date() {
            return Ok(MergeOutcome::UpToDate);
        }
        if (analysis.is_fast_forward() && !no_ff) || analysis.is_unborn() {
            let target = repo.find_commit(their.id())?;
            let mut blocked = Vec::new();
            let mut cb = safe_checkout();
            cb.notify_on(git2::CheckoutNotificationType::CONFLICT);
            cb.notify(|_, path, _, _, _| {
                if let Some(p) = path {
                    blocked.push(p.to_string_lossy().replace('\\', "/"));
                }
                true
            });
            let r = repo.checkout_tree(target.as_object(), Some(&mut cb));
            drop(cb);
            if r.is_err() {
                return Err(dirty(blocked, "the merge"));
            }
            match repo.head() {
                Ok(mut head) => {
                    head.set_target(target.id(), &format!("merge {rev}: Fast-forward"))?;
                }
                Err(_) => {
                    let head = repo.find_reference("HEAD")?;
                    let name = head
                        .symbolic_target()
                        .unwrap_or("refs/heads/main")
                        .to_owned();
                    repo.reference(&name, target.id(), true, "merge: Fast-forward")?;
                }
            }
            return Ok(MergeOutcome::FastForward(target.id()));
        }
        let sig = self.signature(identity)?;
        self.refuse_staged("the merge")?;
        let mut cb = safe_checkout();
        cb.allow_conflicts(true).conflict_style_merge(true);
        repo.merge(&[&their], None, Some(&mut cb))
            .map_err(|e| match e.code() {
                git2::ErrorCode::Conflict | git2::ErrorCode::MergeConflict => {
                    dirty(Vec::new(), "the merge")
                }
                _ => e.into(),
            })?;
        let paths = conflicts(&repo)?;
        if !paths.is_empty() {
            return Ok(MergeOutcome::Conflicts {
                paths,
                step: 0,
                steps: 0,
            });
        }
        let mut index = repo.index()?;
        let tree = repo.find_tree(index.write_tree()?)?;
        let head = repo.head()?.peel_to_commit()?;
        let theirs = repo.find_commit(their.id())?;
        let msg = message
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Merge branch '{rev}'"));
        let oid = repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &format!("{}\n", msg.trim_end()),
            &tree,
            &[&head, &theirs],
        )?;
        repo.cleanup_state()?;
        Ok(MergeOutcome::Merged(oid))
    }

    /// Abort the merge or cherry-pick in progress: back to HEAD, losing the resolution so far.
    pub fn abort_merge(&self) -> Result<MergeOutcome> {
        let repo = self.repository()?;
        if repo.state() == git2::RepositoryState::Clean {
            return Err(GitError::new(ErrorKind::Refused, "No merge is in progress"));
        }
        let head = repo.head()?.peel_to_commit()?;
        let mut cb = CheckoutBuilder::new();
        cb.force();
        repo.reset(head.as_object(), git2::ResetType::Hard, Some(&mut cb))?;
        repo.cleanup_state()?;
        Ok(MergeOutcome::Aborted(head.id()))
    }

    /// Rebase the current branch onto `onto`, stopping at the first conflict.
    pub fn rebase(&self, onto: &str, identity: Option<&Identity>) -> Result<MergeOutcome> {
        let sig = self.signature(identity)?;
        let repo = self.repository()?;
        if repo.state() != git2::RepositoryState::Clean {
            return Err(GitError::new(
                ErrorKind::Refused,
                "An operation is in progress: finish or abort it first",
            ));
        }
        let s = self.status(false)?;
        if !s.staged.is_empty() || !s.unstaged.is_empty() {
            let mut paths: Vec<String> = s
                .staged
                .iter()
                .chain(&s.unstaged)
                .map(|c| c.path.clone())
                .collect();
            paths.dedup();
            return Err(dirty(paths, "the rebase"));
        }
        let upstream = self.annotated(&repo, onto)?;
        let head = repo.head()?.peel_to_commit()?;
        if head.id() == upstream.id() || repo.graph_descendant_of(head.id(), upstream.id())? {
            return Ok(MergeOutcome::UpToDate);
        }
        if repo.graph_descendant_of(upstream.id(), head.id())? {
            // Nothing of ours to replay: a fast-forward.
            return match self.merge(onto, false, None, identity)? {
                MergeOutcome::FastForward(oid) => Ok(MergeOutcome::Rebased(oid)),
                other => Ok(other),
            };
        }
        let mut opts = git2::RebaseOptions::new();
        let mut cb = safe_checkout();
        cb.allow_conflicts(true).conflict_style_merge(true);
        opts.checkout_options(cb);
        let mut rebase = repo.rebase(None, Some(&upstream), None, Some(&mut opts))?;
        drive_rebase(&repo, &mut rebase, &sig)
    }

    /// Continue the rebase in progress after the conflicts were resolved and staged.
    pub fn rebase_continue(&self, identity: Option<&Identity>) -> Result<MergeOutcome> {
        let sig = self.signature(identity)?;
        let repo = self.repository()?;
        let mut rebase = repo
            .open_rebase(None)
            .map_err(|_| GitError::new(ErrorKind::Refused, "No rebase is in progress"))?;
        let paths = conflicts(&repo)?;
        if !paths.is_empty() {
            return Err(GitError::new(
                ErrorKind::Conflicted,
                format!("Resolve and stage these files first: {}", paths.join(", ")),
            )
            .with_paths(paths));
        }
        if rebase.operation_current().is_some() {
            commit_step(&mut rebase, &sig)?;
        }
        drive_rebase(&repo, &mut rebase, &sig)
    }

    /// Abort the rebase in progress: back to where it started.
    pub fn rebase_abort(&self) -> Result<MergeOutcome> {
        let repo = self.repository()?;
        let mut rebase = repo
            .open_rebase(None)
            .map_err(|_| GitError::new(ErrorKind::Refused, "No rebase is in progress"))?;
        rebase.abort()?;
        let head = repo.head()?.peel_to_commit()?.id();
        Ok(MergeOutcome::Aborted(head))
    }

    /// Cherry-pick `rev` onto the current branch, stopping at a conflict.
    pub fn cherry_pick(&self, rev: &str, identity: Option<&Identity>) -> Result<MergeOutcome> {
        let sig = self.signature(identity)?;
        let repo = self.repository()?;
        let commit = repo
            .revparse_single(rev)
            .map_err(|e| GitError::new(ErrorKind::NotFound, e.message().to_owned()))?
            .peel_to_commit()?;
        if commit.parent_count() > 1 {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!("{rev} is a merge commit: cherry-picking a merge is not supported"),
            ));
        }
        self.refuse_staged("the cherry-pick")?;
        let mut opts = git2::CherrypickOptions::new();
        let mut cb = safe_checkout();
        cb.allow_conflicts(true).conflict_style_merge(true);
        opts.checkout_builder(cb);
        repo.cherrypick(&commit, Some(&mut opts))
            .map_err(|e| match e.code() {
                git2::ErrorCode::Conflict | git2::ErrorCode::MergeConflict => {
                    dirty(Vec::new(), "the cherry-pick")
                }
                _ => e.into(),
            })?;
        let paths = conflicts(&repo)?;
        if !paths.is_empty() {
            return Ok(MergeOutcome::Conflicts {
                paths,
                step: 0,
                steps: 0,
            });
        }
        let mut index = repo.index()?;
        let tree = repo.find_tree(index.write_tree()?)?;
        let head = repo.head()?.peel_to_commit()?;
        let oid = repo.commit(
            Some("HEAD"),
            &commit.author(),
            &sig,
            commit.message().unwrap_or_default(),
            &tree,
            &[&head],
        )?;
        repo.cleanup_state()?;
        Ok(MergeOutcome::Picked(oid))
    }

    /// Reset the current branch to `rev`.
    pub fn reset(&self, rev: &str, mode: ResetMode) -> Result<Oid> {
        let repo = self.repository()?;
        let commit = repo
            .revparse_single(rev)
            .map_err(|e| GitError::new(ErrorKind::NotFound, e.message().to_owned()))?
            .peel_to_commit()?;
        let kind = match mode {
            ResetMode::Soft => git2::ResetType::Soft,
            ResetMode::Mixed => git2::ResetType::Mixed,
            ResetMode::Hard => git2::ResetType::Hard,
        };
        let mut cb = CheckoutBuilder::new();
        cb.force();
        repo.reset(
            commit.as_object(),
            kind,
            (mode == ResetMode::Hard).then_some(&mut cb),
        )?;
        Ok(commit.id())
    }
}

fn commit_step(rebase: &mut git2::Rebase<'_>, sig: &git2::Signature<'_>) -> Result<()> {
    match rebase.commit(None, sig, None) {
        Ok(_) => Ok(()),
        // The step's change is already upstream: nothing to commit, skip it.
        Err(e) if e.code() == git2::ErrorCode::Applied => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn drive_rebase(
    repo: &Repository,
    rebase: &mut git2::Rebase<'_>,
    sig: &git2::Signature<'_>,
) -> Result<MergeOutcome> {
    let steps = rebase.len();
    while let Some(op) = rebase.next() {
        op?;
        let paths = conflicts(repo)?;
        if !paths.is_empty() {
            return Ok(MergeOutcome::Conflicts {
                paths,
                step: rebase.operation_current().map_or(0, |i| i + 1),
                steps,
            });
        }
        commit_step(rebase, sig)?;
    }
    rebase.finish(Some(sig))?;
    Ok(MergeOutcome::Rebased(repo.head()?.peel_to_commit()?.id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn base() -> TestRepo {
        let t = TestRepo::new();
        t.write("a.cs", "one\ntwo\nthree\n");
        t.commit_all("base");
        t
    }

    #[test]
    fn branches_checkout_new_branch_and_delete() {
        let t = base();
        let c = t.repo.checkout("feature", true, None, false).unwrap();
        assert_eq!((c.branch.as_deref(), c.created), (Some("feature"), true));
        t.write("b.cs", "b\n");
        t.commit_all("feature work");
        let (b, _) = t.repo.branches(true).unwrap();
        let names: Vec<(&str, bool)> = b.iter().map(|b| (b.name.as_str(), b.head)).collect();
        assert_eq!(names, [("feature", true), ("main", false)]);
        t.repo.checkout("main", false, None, false).unwrap();
        assert!(!t.path().join("b.cs").exists());
        let e = t.repo.delete_branch("feature", false).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Refused);
        assert!(e.message.contains("not fully merged"));
        t.repo.delete_branch("feature", true).unwrap();
        assert_eq!(t.repo.branches(false).unwrap().0.len(), 1);
        assert!(
            t.repo.checkout("main", true, None, false).is_err(),
            "exists"
        );
        // A revision checks out detached.
        let c = t.repo.checkout("HEAD", false, None, false).unwrap();
        assert_eq!(c.branch, None);
    }

    #[test]
    fn checkout_of_a_dirty_tree_is_refused_unless_safe_or_forced() {
        let t = base();
        t.repo.checkout("other", true, None, false).unwrap();
        t.write("a.cs", "one\nTWO\nthree\n");
        t.commit_all("other changes a");
        t.repo.checkout("main", false, None, false).unwrap();
        // A local change to a file the switch rewrites: refused, naming it.
        t.write("a.cs", "local\n");
        let e = t.repo.checkout("other", false, None, false).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Dirty);
        assert_eq!(e.paths, ["a.cs"]);
        assert!(
            e.message.contains("would be overwritten by checkout: a.cs"),
            "{e}"
        );
        assert_eq!(t.repo.current_branch().unwrap().as_deref(), Some("main"));
        // A change the switch does not touch comes along.
        t.repo.discard(None, false).unwrap();
        t.write("c.cs", "untracked\n");
        t.repo.checkout("other", false, None, false).unwrap();
        assert_eq!(t.read("c.cs"), "untracked\n");
        // Force overwrites.
        t.write("a.cs", "local\n");
        t.repo.checkout("main", false, None, true).unwrap();
        assert_eq!(t.read("a.cs"), "one\ntwo\nthree\n");
    }

    #[test]
    fn merge_fast_forward_merge_commit_and_conflict_resolution() {
        let t = base();
        t.repo.checkout("ff", true, None, false).unwrap();
        t.write("f.cs", "f\n");
        t.commit_all("ff");
        t.repo.checkout("main", false, None, false).unwrap();
        assert!(matches!(
            t.repo.merge("ff", false, None, None).unwrap(),
            MergeOutcome::FastForward(_)
        ));
        assert_eq!(
            t.repo.merge("ff", false, None, None).unwrap(),
            MergeOutcome::UpToDate
        );
        // Both sides change the same line: a conflict.
        t.repo.checkout("theirs", true, None, false).unwrap();
        t.write("a.cs", "one\nTHEIRS\nthree\n");
        t.commit_all("theirs");
        t.repo.checkout("main", false, None, false).unwrap();
        t.write("a.cs", "one\nOURS\nthree\n");
        t.commit_all("ours");
        let out = t.repo.merge("theirs", false, None, None).unwrap();
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
            text.contains("<<<<<<<") && text.contains("OURS") && text.contains("THEIRS"),
            "{text}"
        );
        let s = t.repo.status(false).unwrap();
        assert_eq!(s.conflicted, ["a.cs"]);
        assert_eq!(s.operation, Some(crate::Operation::Merge));
        // Resolve: edit, stage (which resolves), commit (which ends the merge with two parents).
        t.write("a.cs", "one\nBOTH\nthree\n");
        let staged = t.repo.stage(Some(&["a.cs".into()])).unwrap();
        assert_eq!(staged.resolved, ["a.cs"]);
        let c = t
            .repo
            .commit(&crate::commit::CommitOptions {
                message: "Merge theirs".into(),
                ..Default::default()
            })
            .unwrap();
        let repo = t.repo.repository().unwrap();
        assert_eq!(repo.find_commit(c.oid).unwrap().parent_count(), 2);
        assert_eq!(t.repo.status(false).unwrap().operation, None);
        // A clean merge commits at once.
        t.repo
            .checkout("side", true, Some("theirs"), false)
            .unwrap();
        t.write("s.cs", "s\n");
        t.commit_all("side");
        t.repo.checkout("main", false, None, false).unwrap();
        t.write("m.cs", "m\n");
        t.commit_all("main moves");
        assert!(matches!(
            t.repo.merge("side", false, None, None).unwrap(),
            MergeOutcome::Merged(_)
        ));
        assert!(t.path().join("s.cs").exists());
    }

    #[test]
    fn merge_abort_returns_to_head() {
        let t = base();
        t.repo.checkout("x", true, None, false).unwrap();
        t.write("a.cs", "X\n");
        t.commit_all("x");
        t.repo.checkout("main", false, None, false).unwrap();
        t.write("a.cs", "Y\n");
        t.commit_all("y");
        assert!(matches!(
            t.repo.merge("x", false, None, None).unwrap(),
            MergeOutcome::Conflicts { .. }
        ));
        assert!(matches!(
            t.repo.abort_merge().unwrap(),
            MergeOutcome::Aborted(_)
        ));
        assert_eq!(t.read("a.cs"), "Y\n");
        let s = t.repo.status(false).unwrap();
        assert!(s.conflicted.is_empty() && s.operation.is_none());
    }

    #[test]
    fn rebase_stops_at_a_conflict_continues_and_aborts() {
        let t = base();
        t.repo.checkout("topic", true, None, false).unwrap();
        t.write("t1.cs", "t1\n");
        t.commit_all("topic 1");
        t.write("a.cs", "one\nTOPIC\nthree\n");
        t.commit_all("topic 2");
        t.repo.checkout("main", false, None, false).unwrap();
        t.write("a.cs", "one\nMAIN\nthree\n");
        t.commit_all("main");
        t.repo.checkout("topic", false, None, false).unwrap();
        let out = t.repo.rebase("main", None).unwrap();
        assert_eq!(
            out,
            MergeOutcome::Conflicts {
                paths: vec!["a.cs".into()],
                step: 2,
                steps: 2
            }
        );
        assert_eq!(
            t.repo.status(false).unwrap().operation,
            Some(crate::Operation::Rebase)
        );
        assert!(t.read("a.cs").contains("<<<<<<<"));
        assert_eq!(
            t.repo.rebase_continue(None).unwrap_err().kind,
            ErrorKind::Conflicted
        );
        t.write("a.cs", "one\nMAIN TOPIC\nthree\n");
        t.repo.stage(Some(&["a.cs".into()])).unwrap();
        assert!(matches!(
            t.repo.rebase_continue(None).unwrap(),
            MergeOutcome::Rebased(_)
        ));
        let (log, _) = t
            .repo
            .log(&Default::default(), &crate::Cancel::new())
            .unwrap();
        let s: Vec<&str> = log.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(s, ["topic 2", "topic 1", "main", "base"]);
        assert_eq!(t.repo.current_branch().unwrap().as_deref(), Some("topic"));
        assert_eq!(t.repo.status(false).unwrap().operation, None);

        // Abort.
        t.repo.checkout("main", false, None, false).unwrap();
        t.write("a.cs", "one\nAGAIN\nthree\n");
        t.commit_all("main again");
        t.repo.checkout("topic", false, None, false).unwrap();
        let before = t.read("a.cs");
        assert!(matches!(
            t.repo.rebase("main", None).unwrap(),
            MergeOutcome::Conflicts { .. }
        ));
        assert!(matches!(
            t.repo.rebase_abort().unwrap(),
            MergeOutcome::Aborted(_)
        ));
        assert_eq!(t.read("a.cs"), before);
        assert_eq!(t.repo.current_branch().unwrap().as_deref(), Some("topic"));
        assert_eq!(
            t.repo.rebase("topic", None).unwrap(),
            MergeOutcome::UpToDate
        );
    }

    #[test]
    fn cherry_pick_and_reset() {
        let t = base();
        t.repo.checkout("src", true, None, false).unwrap();
        t.write("p.cs", "picked\n");
        let picked = t.commit_all("to pick");
        t.repo.checkout("main", false, None, false).unwrap();
        let out = t.repo.cherry_pick(&picked.to_string(), None).unwrap();
        let MergeOutcome::Picked(oid) = out else {
            panic!("{out:?}")
        };
        let repo = t.repo.repository().unwrap();
        assert_eq!(repo.find_commit(oid).unwrap().message(), Some("to pick\n"));
        assert_eq!(t.read("p.cs"), "picked\n");
        // A conflicting pick stops; staging and committing completes it.
        t.repo.checkout("src", false, None, false).unwrap();
        t.write("a.cs", "SRC\n");
        let c2 = t.commit_all("src a");
        t.repo.checkout("main", false, None, false).unwrap();
        t.write("a.cs", "MAIN\n");
        t.commit_all("main a");
        assert!(matches!(
            t.repo.cherry_pick(&c2.to_string(), None).unwrap(),
            MergeOutcome::Conflicts { .. }
        ));
        assert_eq!(
            t.repo.status(false).unwrap().operation,
            Some(crate::Operation::CherryPick)
        );
        t.write("a.cs", "RESOLVED\n");
        t.repo.stage(Some(&["a.cs".into()])).unwrap();
        let c = t
            .repo
            .commit(&crate::commit::CommitOptions {
                message: "src a".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(repo.find_commit(c.oid).unwrap().parent_count(), 1);
        assert_eq!(t.repo.status(false).unwrap().operation, None);

        // Reset: mixed keeps the changes unstaged, hard drops them.
        let before = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .parent_id(0)
            .unwrap();
        t.repo.reset("HEAD~1", ResetMode::Mixed).unwrap();
        assert_eq!(repo.head().unwrap().target(), Some(before));
        assert_eq!(t.repo.status(false).unwrap().unstaged[0].path, "a.cs");
        t.repo.reset("HEAD", ResetMode::Hard).unwrap();
        assert_eq!(t.read("a.cs"), "MAIN\n");
        t.repo.reset("HEAD~1", ResetMode::Soft).unwrap();
        assert_eq!(t.repo.status(false).unwrap().staged.len(), 1);
    }
}
