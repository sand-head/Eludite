//! Fetch, pull and push over libgit2's transports, with its credential callback: for ssh urls the ssh agent, for
//! http urls the configured `git-credential` helper (`Cred::credential_helper`), else libgit2's default; a failure
//! names what was tried. Each call reports its progress and stops at its [`Cancel`].
//!
//! Which transports exist is libgit2's build: this workspace builds libgit2 without its `https` and `ssh` features
//! (no new dependency), so local paths, `file://`, `git://` and plain `http://` remotes work; `https://` and ssh
//! remotes fail with libgit2's "unsupported URL protocol".

use std::cell::RefCell;

use git2::{Cred, CredentialType, FetchOptions, PushOptions, RemoteCallbacks, Repository};

use crate::branches::MergeOutcome;
use crate::commit::Identity;
use crate::{Cancel, ErrorKind, GitError, Repo, Result};

/// A transfer's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// `fetch` or `push`.
    pub phase: &'static str,
    pub current: usize,
    pub total: usize,
    pub bytes: usize,
}

/// What a fetch did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fetched {
    pub remote: String,
    /// Remote refs that moved (`origin/main`).
    pub updated: Vec<String>,
    pub received_objects: usize,
}

/// What a push did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub remote: String,
    pub branch: String,
    pub upstream: Option<String>,
    pub oid: git2::Oid,
}

/// What the credential callback tried, for the failure message.
#[derive(Debug, Default)]
struct Tried {
    agent: bool,
    helper: Option<String>,
    default: bool,
}

impl Tried {
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.agent {
            parts.push("the ssh agent (SSH_AUTH_SOCK)".to_owned());
        }
        if let Some(h) = &self.helper {
            parts.push(format!("the credential helper `{h}`"));
        }
        if self.default {
            parts.push("the system's default credentials".to_owned());
        }
        if parts.is_empty() {
            "nothing (the remote asked for no method Eludite supports)".into()
        } else {
            parts.join(", then ")
        }
    }
}

/// The message for a credential failure on `url` after trying `tried`.
pub fn credential_failure(url: &str, tried: &str) -> String {
    format!(
        "Authentication failed for {url}: tried {tried}. Set up a credential helper (`git config --global \
         credential.helper <helper>`, such as Git Credential Manager or `store`), or for ssh add your key to the ssh \
         agent (`ssh-add`)."
    )
}

fn callbacks<'a, 'p: 'a>(
    repo: &'a Repository,
    tried: &'a RefCell<Tried>,
    cancel: &'a Cancel,
    progress: &'a RefCell<&'p mut (dyn FnMut(Progress) + 'p)>,
    updated: Option<&'a RefCell<Vec<String>>>,
) -> RemoteCallbacks<'a> {
    let mut cb = RemoteCallbacks::new();
    cb.credentials(move |url, username, allowed| {
        let mut t = tried.borrow_mut();
        if allowed.contains(CredentialType::SSH_KEY) && !t.agent {
            t.agent = true;
            return Cred::ssh_key_from_agent(username.unwrap_or("git"));
        }
        if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) && t.helper.is_none() {
            let config = repo.config()?;
            t.helper = Some(
                config
                    .get_string("credential.helper")
                    .unwrap_or_else(|_| "(none configured)".into()),
            );
            if let Ok(c) = Cred::credential_helper(&config, url, username) {
                return Ok(c);
            }
        }
        if allowed.contains(CredentialType::DEFAULT) && !t.default {
            t.default = true;
            return Cred::default();
        }
        Err(git2::Error::new(
            git2::ErrorCode::Auth,
            git2::ErrorClass::Net,
            credential_failure(url, &t.describe()),
        ))
    });
    cb.transfer_progress(move |p| {
        (*progress.borrow_mut())(Progress {
            phase: "fetch",
            current: p.received_objects(),
            total: p.total_objects(),
            bytes: p.received_bytes(),
        });
        !cancel.is_canceled()
    });
    cb.push_transfer_progress(move |current, total, bytes| {
        (*progress.borrow_mut())(Progress {
            phase: "push",
            current,
            total,
            bytes,
        });
    });
    cb.sideband_progress(move |_| !cancel.is_canceled());
    if let Some(updated) = updated {
        cb.update_tips(move |name, _, _| {
            let short = name
                .strip_prefix("refs/remotes/")
                .or_else(|| name.strip_prefix("refs/tags/"))
                .unwrap_or(name);
            updated.borrow_mut().push(short.to_owned());
            true
        });
    }
    cb
}

/// A failed transfer as the caller sees it: canceled, a credential failure, or libgit2's message.
fn transfer_error(e: git2::Error, cancel: &Cancel, url: &str, tried: &Tried) -> GitError {
    if cancel.is_canceled() {
        return GitError::new(ErrorKind::Canceled, "canceled");
    }
    if e.code() == git2::ErrorCode::Auth
        || (e.class() == git2::ErrorClass::Http && e.message().contains("auth"))
    {
        let msg = if e.message().starts_with("Authentication failed") {
            e.message().to_owned()
        } else {
            credential_failure(url, &tried.describe())
        };
        return GitError::new(ErrorKind::Credentials, msg);
    }
    e.into()
}

impl Repo {
    /// The remote a fetch, pull or push of the current branch uses: `remote`, else the branch's upstream remote,
    /// else `origin`.
    pub fn default_remote(&self, remote: Option<&str>) -> Result<String> {
        if let Some(r) = remote {
            return Ok(r.to_owned());
        }
        let repo = self.repository()?;
        if let Some(b) = crate::status::head_info(&repo)?.branch
            && let Ok(name) = repo.config()?.get_string(&format!("branch.{b}.remote"))
        {
            return Ok(name);
        }
        Ok("origin".into())
    }

    /// Fetch `remote` (see [`Repo::default_remote`]).
    pub fn fetch(
        &self,
        remote: Option<&str>,
        prune: bool,
        cancel: &Cancel,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Fetched> {
        cancel.check()?;
        let name = self.default_remote(remote)?;
        let repo = self.repository()?;
        let mut r = repo.find_remote(&name).map_err(|_| {
            GitError::new(ErrorKind::NotFound, format!("there is no remote `{name}`"))
        })?;
        let url = r.url().unwrap_or_default().to_owned();
        let tried = RefCell::new(Tried::default());
        let updated = RefCell::new(Vec::new());
        let progress = RefCell::new(progress);
        let result = {
            let cb = callbacks(&repo, &tried, cancel, &progress, Some(&updated));
            let mut fo = FetchOptions::new();
            fo.remote_callbacks(cb);
            fo.download_tags(git2::AutotagOption::All);
            if prune {
                fo.prune(git2::FetchPrune::On);
            }
            r.fetch::<&str>(&[], Some(&mut fo), None)
        };
        result.map_err(|e| transfer_error(e, cancel, &url, &tried.borrow()))?;
        let received = r.stats().received_objects();
        let mut updated = updated.into_inner();
        updated.sort();
        updated.dedup();
        Ok(Fetched {
            remote: name,
            updated,
            received_objects: received,
        })
    }

    /// Pull: fetch, then fast-forward or merge the upstream (or rebase onto it with `rebase`).
    pub fn pull(
        &self,
        remote: Option<&str>,
        rebase: bool,
        identity: Option<&Identity>,
        cancel: &Cancel,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<MergeOutcome> {
        let fetched = self.fetch(remote, false, cancel, progress)?;
        let repo = self.repository()?;
        let head = crate::status::head_info(&repo)?;
        let Some(branch) = head.branch else {
            return Err(GitError::new(
                ErrorKind::Refused,
                "HEAD is detached: check out a branch to pull into",
            ));
        };
        let upstream = match head.upstream {
            Some(u) => u,
            None => {
                let candidate = format!("{}/{branch}", fetched.remote);
                if repo
                    .find_branch(&candidate, git2::BranchType::Remote)
                    .is_err()
                {
                    return Err(GitError::new(
                        ErrorKind::NotFound,
                        format!(
                            "`{branch}` has no upstream and the remote has no `{branch}`: push it first"
                        ),
                    ));
                }
                candidate
            }
        };
        if rebase {
            self.rebase(&upstream, identity)
        } else {
            self.merge(&upstream, false, None, identity)
        }
    }

    /// Push `branch` (default the current one) to `remote`; refused as a non-fast-forward unless `force`. The branch
    /// gets the pushed ref as its upstream with `set_upstream`, or when it has none.
    pub fn push(
        &self,
        remote: Option<&str>,
        branch: Option<&str>,
        set_upstream: bool,
        force: bool,
        cancel: &Cancel,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Pushed> {
        cancel.check()?;
        let repo = self.repository()?;
        let branch = match branch {
            Some(b) => b.to_owned(),
            None => crate::status::head_info(&repo)?.branch.ok_or_else(|| {
                GitError::new(
                    ErrorKind::Refused,
                    "HEAD is detached: check out a branch to push",
                )
            })?,
        };
        let local = repo
            .find_branch(&branch, git2::BranchType::Local)
            .map_err(|_| {
                GitError::new(
                    ErrorKind::NotFound,
                    format!("there is no branch `{branch}`"),
                )
            })?;
        let oid = local.get().peel_to_commit()?.id();
        let (remote_name, remote_branch) = match local.upstream() {
            Ok(up) if !set_upstream || remote.is_none() => {
                let n = up.name()?.unwrap_or_default().to_owned();
                match n.split_once('/') {
                    Some((r, b)) if remote.is_none_or(|x| x == r) => (r.to_owned(), b.to_owned()),
                    _ => (self.default_remote(remote)?, branch.clone()),
                }
            }
            _ => (self.default_remote(remote)?, branch.clone()),
        };
        let had_upstream = local.upstream().is_ok();
        let mut r = repo.find_remote(&remote_name).map_err(|_| {
            GitError::new(
                ErrorKind::NotFound,
                format!("there is no remote `{remote_name}`"),
            )
        })?;
        let url = r.pushurl().or(r.url()).unwrap_or_default().to_owned();
        let tried = RefCell::new(Tried::default());
        let progress = RefCell::new(progress);
        let dst = format!("refs/heads/{remote_branch}");
        let rejected: RefCell<Option<String>> = RefCell::new(None);
        let non_ff = RefCell::new(false);
        let result = {
            let mut cb = callbacks(&repo, &tried, cancel, &progress, None);
            cb.push_update_reference(|name, status| {
                if let Some(s) = status {
                    *rejected.borrow_mut() = Some(format!("{name}: {s}"));
                }
                Ok(())
            });
            // The remote's tip, before anything is sent: refuse a non-fast-forward (libgit2's local transport does
            // not, and a server's refusal comes after the upload).
            if !force {
                cb.push_negotiation(|updates| {
                    for u in updates {
                        let theirs = u.src();
                        if theirs.is_zero() || theirs == u.dst() {
                            continue;
                        }
                        let ours = repo.find_commit(theirs).is_ok()
                            && repo.graph_descendant_of(u.dst(), theirs).unwrap_or(false);
                        if !ours {
                            *non_ff.borrow_mut() = true;
                            return Err(git2::Error::from_str("non-fast-forward"));
                        }
                    }
                    Ok(())
                });
            }
            let mut po = PushOptions::new();
            po.remote_callbacks(cb);
            let spec = format!("{}refs/heads/{branch}:{dst}", if force { "+" } else { "" });
            r.push(&[spec.as_str()], Some(&mut po))
        };
        if non_ff.into_inner() {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!(
                    "The push to {remote_name}/{remote_branch} was rejected as a non-fast-forward: the remote has \
                     commits you do not have. Pull first, then push."
                ),
            ));
        }
        result.map_err(|e| transfer_error(e, cancel, &url, &tried.borrow()))?;
        if let Some(why) = rejected.into_inner() {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!("The remote rejected the push: {why}"),
            ));
        }
        let upstream_name = format!("{remote_name}/{remote_branch}");
        let mut upstream = local
            .upstream()
            .ok()
            .and_then(|u| u.name().ok().flatten().map(str::to_owned));
        if set_upstream || !had_upstream {
            // The push updated the remote-tracking ref; make it the upstream.
            if repo
                .find_branch(&upstream_name, git2::BranchType::Remote)
                .is_err()
            {
                repo.reference(&format!("refs/remotes/{upstream_name}"), oid, true, "push")?;
            }
            let mut local = repo.find_branch(&branch, git2::BranchType::Local)?;
            local.set_upstream(Some(&upstream_name))?;
            upstream = Some(upstream_name);
        }
        Ok(Pushed {
            remote: remote_name,
            branch,
            upstream,
            oid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn credential_failures_say_what_was_tried() {
        let t = Tried {
            agent: true,
            helper: Some("store".into()),
            default: false,
        };
        let m = credential_failure("https://example.com/r.git", &t.describe());
        assert!(
            m.contains("the ssh agent (SSH_AUTH_SOCK), then the credential helper `store`"),
            "{m}"
        );
        assert!(m.contains("credential.helper"), "{m}");
        assert!(Tried::default().describe().starts_with("nothing"));
    }

    #[test]
    fn an_unknown_remote_and_a_canceled_fetch() {
        let t = TestRepo::new();
        t.write("a", "a");
        t.commit_all("a");
        let e = t
            .repo
            .fetch(None, false, &Cancel::new(), &mut |_| {})
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::NotFound);
        let c = Cancel::new();
        c.cancel();
        assert_eq!(
            t.repo.fetch(None, false, &c, &mut |_| {}).unwrap_err().kind,
            ErrorKind::Canceled
        );
    }
}
