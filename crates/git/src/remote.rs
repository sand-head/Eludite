//! Fetch, pull and push over libgit2's transports: local paths, `file://`, `git://`, `http://`, `https://` (git2's
//! `https` feature: OpenSSL on Linux, SecureTransport on macOS, WinHTTP on Windows) and ssh (`ssh` feature: the
//! bundled libssh2), with the credential callback of [`crate::credentials`], the proxy and certificate rule of
//! [`crate::transport`], and refusals that name the host. Each call reports its progress and stops at its
//! [`Cancel`].

use std::cell::RefCell;
use std::path::PathBuf;

use git2::{
    CertificateCheckStatus, Cred, CredentialHelper, FetchOptions, ProxyOptions, PushOptions,
    RemoteCallbacks,
};

use crate::branches::MergeOutcome;
use crate::commit::Identity;
use crate::credentials::{
    Attempt, CredentialState, KeyFile, SessionCredentials, Sources, UserPass, default_ssh_dir,
    remote_host, remote_scheme, ssh_key_candidates,
};
use crate::transport::{certificate_refusal, proxy_for};
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

/// The message for a credential failure on `url` after trying `tried` (kept for callers of brief 0040's API).
pub fn credential_failure(url: &str, tried: &str) -> String {
    crate::credentials::credential_failure(&remote_host(url).unwrap_or_default(), url, tried)
        .message
}

/// The callback's sources on this machine: the environment, the git config, the session's credentials.
struct LiveSources<'a> {
    config: Option<&'a git2::Config>,
    session: &'a SessionCredentials,
}

/// The configured helper's name: `credential.<url>.helper`, else `credential.helper`.
fn helper_name(config: &git2::Config) -> Option<String> {
    let mut url_specific = None;
    if let Ok(mut entries) = config.entries(Some(r"credential\..+\.helper")) {
        while let Some(Ok(e)) = entries.next() {
            if let Some(v) = e.value().filter(|v| !v.is_empty()) {
                url_specific = Some(v.to_owned());
            }
        }
    }
    config
        .get_string("credential.helper")
        .ok()
        .filter(|v| !v.is_empty())
        .or(url_specific)
}

impl Sources for LiveSources<'_> {
    fn agent(&self) -> bool {
        // Windows: libssh2 reaches Pageant or OpenSSH's agent pipe without SSH_AUTH_SOCK.
        cfg!(windows) || std::env::var_os("SSH_AUTH_SOCK").is_some_and(|s| !s.is_empty())
    }

    fn ssh_keys(&self) -> (Vec<KeyFile>, Vec<PathBuf>) {
        default_ssh_dir()
            .map(|d| ssh_key_candidates(&d))
            .unwrap_or_default()
    }

    fn helper(
        &self,
        url: &str,
        username: Option<&str>,
    ) -> (Option<String>, Option<(String, String)>) {
        let Some(config) = self.config else {
            return (None, None);
        };
        let name = helper_name(config);
        if name.is_none() {
            return (None, None);
        }
        let answer = CredentialHelper::new(url)
            .config(config)
            .username(username)
            .execute();
        (name, answer)
    }

    fn supplied(&self, host: &str) -> Option<UserPass> {
        self.session.get(host)
    }

    fn local_user(&self) -> String {
        ["USER", "USERNAME", "LOGNAME"]
            .iter()
            .find_map(|v| std::env::var(v).ok().filter(|u| !u.is_empty()))
            .unwrap_or_else(|| "git".into())
    }
}

/// What a transfer needs besides libgit2's remote: the configuration it read and the session's credentials.
struct Setup {
    config: Option<git2::Config>,
    session: SessionCredentials,
    ssl_verify: bool,
}

impl Setup {
    fn of(repo: &Repo) -> Setup {
        let config = repo.effective_config().ok();
        let ssl_verify = config
            .as_ref()
            .and_then(|c| c.get_bool("http.sslVerify").ok())
            .unwrap_or(true);
        Setup {
            config,
            session: repo.credentials().clone(),
            ssl_verify,
        }
    }

    /// The proxy options for `url` (see [`proxy_for`]).
    fn proxy(&self, url: &str) -> ProxyOptions<'static> {
        let configured = self
            .config
            .as_ref()
            .and_then(|c| c.get_string("http.proxy").ok());
        let mut po = ProxyOptions::new();
        if let Some(p) = proxy_for(url, configured.as_deref(), &|n| std::env::var(n).ok()) {
            po.url(&p);
        }
        po
    }
}

fn to_cred(attempt: Attempt) -> std::result::Result<Cred, git2::Error> {
    match attempt {
        Attempt::Username(u) => Cred::username(&u),
        Attempt::Agent(u) => Cred::ssh_key_from_agent(&u),
        Attempt::KeyFile(u, k) => Cred::ssh_key(&u, k.public.as_deref(), &k.private, None),
        Attempt::Helper(up) | Attempt::Supplied(up) => {
            Cred::userpass_plaintext(&up.username, &up.password)
        }
        Attempt::Default => Cred::default(),
    }
}

fn callbacks<'a, 'p: 'a>(
    setup: &'a Setup,
    state: &'a RefCell<CredentialState>,
    cancel: &'a Cancel,
    progress: &'a RefCell<&'p mut (dyn FnMut(Progress) + 'p)>,
    updated: Option<&'a RefCell<Vec<String>>>,
) -> RemoteCallbacks<'a> {
    let mut cb = RemoteCallbacks::new();
    cb.credentials(move |url, username, allowed| {
        let sources = LiveSources {
            config: setup.config.as_ref(),
            session: &setup.session,
        };
        let next = state.borrow_mut().next(url, username, allowed, &sources);
        match next {
            Ok(attempt) => to_cred(attempt),
            Err(e) => Err(git2::Error::new(
                git2::ErrorCode::Auth,
                git2::ErrorClass::Net,
                e.message,
            )),
        }
    });
    let ssl_verify = setup.ssl_verify;
    cb.certificate_check(move |cert, _host| {
        // `http.sslVerify = false` accepts any TLS certificate, as git does; ssh host keys are libgit2's to check
        // against known_hosts either way.
        if !ssl_verify && cert.as_x509().is_some() {
            Ok(CertificateCheckStatus::CertificateOk)
        } else {
            Ok(CertificateCheckStatus::CertificatePassthrough)
        }
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

/// A failed transfer as the caller sees it: canceled, the credential callback's failure, a refused certificate
/// libgit2 refused the server's certificate: its certificate code, or an SSL-class error whose message says so.
/// OpenSSL (Linux) says "the SSL certificate is invalid"; SecureTransport (macOS) says "untrusted connection error".
fn is_certificate_refusal(e: &git2::Error) -> bool {
    if e.code() == git2::ErrorCode::Certificate {
        return true;
    }
    let m = e.message().to_ascii_lowercase();
    e.class() == git2::ErrorClass::Ssl && (m.contains("certificate") || m.contains("untrusted"))
}

/// naming the host, or libgit2's message.
fn transfer_error(
    e: git2::Error,
    cancel: &Cancel,
    url: &str,
    state: &mut CredentialState,
) -> GitError {
    if cancel.is_canceled() {
        return GitError::new(ErrorKind::Canceled, "canceled");
    }
    if let Some(f) = state.failure.take() {
        return f;
    }
    let host = remote_host(url).unwrap_or_else(|| url.to_owned());
    if is_certificate_refusal(&e) {
        return certificate_refusal(&host, remote_scheme(url) == "ssh", e.message());
    }
    if e.code() == git2::ErrorCode::Auth
        || (e.class() == git2::ErrorClass::Http && e.message().contains("auth"))
    {
        let mut g = crate::credentials::credential_failure(&host, url, &state.describe());
        if e.message().starts_with("Authentication failed")
            || e.message().starts_with("credentials_required")
        {
            g.message = e.message().to_owned();
        }
        return g;
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
        let setup = Setup::of(self);
        let state = RefCell::new(CredentialState::new());
        let updated = RefCell::new(Vec::new());
        let progress = RefCell::new(progress);
        let result = {
            let cb = callbacks(&setup, &state, cancel, &progress, Some(&updated));
            let mut fo = FetchOptions::new();
            fo.remote_callbacks(cb);
            fo.proxy_options(setup.proxy(&url));
            fo.download_tags(git2::AutotagOption::All);
            if prune {
                fo.prune(git2::FetchPrune::On);
            }
            r.fetch::<&str>(&[], Some(&mut fo), None)
        };
        result.map_err(|e| transfer_error(e, cancel, &url, &mut state.borrow_mut()))?;
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

    /// Fetch `refspec` (`+refs/pull/12/head:refs/remotes/origin/pr/12`) from `remote`, or from `url` when given (a
    /// fork's repository, brief 0046's pull request checkout), with the same credentials, proxy and certificate rules
    /// as [`Repo::fetch`]. Answers the commit the refspec's destination points at.
    pub fn fetch_refspec(
        &self,
        remote: Option<&str>,
        url: Option<&str>,
        refspec: &str,
        cancel: &Cancel,
    ) -> Result<git2::Oid> {
        cancel.check()?;
        let repo = self.repository()?;
        let mut r = match url {
            Some(u) => repo.remote_anonymous(u)?,
            None => {
                let name = self.default_remote(remote)?;
                repo.find_remote(&name).map_err(|_| {
                    GitError::new(ErrorKind::NotFound, format!("there is no remote `{name}`"))
                })?
            }
        };
        let remote_url = r.url().unwrap_or_default().to_owned();
        let setup = Setup::of(self);
        let state = RefCell::new(CredentialState::new());
        let mut ignore = |_: Progress| {};
        let progress: RefCell<&mut dyn FnMut(Progress)> = RefCell::new(&mut ignore);
        let result = {
            let cb = callbacks(&setup, &state, cancel, &progress, None);
            let mut fo = FetchOptions::new();
            fo.remote_callbacks(cb);
            fo.proxy_options(setup.proxy(&remote_url));
            fo.download_tags(git2::AutotagOption::None);
            r.fetch(&[refspec], Some(&mut fo), None)
        };
        result.map_err(|e| transfer_error(e, cancel, &remote_url, &mut state.borrow_mut()))?;
        let dest = refspec.rsplit_once(':').map(|(_, d)| d).ok_or_else(|| {
            GitError::new(ErrorKind::Git, format!("`{refspec}` names no destination"))
        })?;
        Ok(repo.refname_to_id(dest)?)
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
        let setup = Setup::of(self);
        let state = RefCell::new(CredentialState::new());
        let progress = RefCell::new(progress);
        let dst = format!("refs/heads/{remote_branch}");
        let rejected: RefCell<Option<String>> = RefCell::new(None);
        let non_ff = RefCell::new(false);
        let result = {
            let mut cb = callbacks(&setup, &state, cancel, &progress, None);
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
            po.proxy_options(setup.proxy(&url));
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
        result.map_err(|e| transfer_error(e, cancel, &url, &mut state.borrow_mut()))?;
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

    #[test]
    fn certificate_refusals_are_recognized_on_every_platform() {
        let cert = |code, class, m: &str| git2::Error::new(code, class, m);
        assert!(super::is_certificate_refusal(&cert(
            git2::ErrorCode::Certificate,
            git2::ErrorClass::Ssl,
            "the SSL certificate is invalid"
        )));
        // macOS's SecureTransport.
        assert!(super::is_certificate_refusal(&cert(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Ssl,
            "untrusted connection error"
        )));
        assert!(!super::is_certificate_refusal(&cert(
            git2::ErrorCode::GenericError,
            git2::ErrorClass::Net,
            "failed to connect"
        )));
    }
    use super::*;
    use crate::testutil::TestRepo;

    #[test]
    fn credential_failures_say_what_was_tried() {
        let m = credential_failure(
            "ssh://example.com/r.git",
            "the ssh agent, then the key files `~/.ssh/id_ed25519`",
        );
        assert!(
            m.starts_with("Authentication failed for ssh://example.com/r.git: tried the ssh agent"),
            "{m}"
        );
        assert!(
            m.contains("ssh-add") && m.contains("credential.helper"),
            "{m}"
        );
        assert!(CredentialState::new().describe().starts_with("nothing"));
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
