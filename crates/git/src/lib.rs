//! Git over libgit2 (PLAN.md D2, 4.8; brief 0040): everything the Git Changes and Git Repository windows, the
//! status decorations and the `eludite.git.*` commands need, with no `git` process anywhere.
//!
//! Public API boundary:
//! - [`Repo`]: one repository's handle, found with [`Repo::discover`] from a workspace folder (or made with
//!   [`Repo::init`]). It holds paths only and opens libgit2's repository per call, so it is `Send + Sync` and cheap
//!   to clone; callers serialize writes themselves (the shell's git service does).
//! - Its operations, one module each: [`status`] (the status with rename detection, ahead and behind, the
//!   operation in progress), the index operations (stage, unstage, discard), [`commit`] (with the identity rule:
//!   the repository's config, the global config, then a fallback, else [`ErrorKind::Identity`]), [`diff`] (a path's
//!   old and new texts against the index, HEAD or a revision; the line diff is the caller's), [`log`] (the history
//!   with its graph lanes, bounded and cancelable), [`branches`] (branches, checkout with libgit2's safe checkout,
//!   merge, rebase, cherry-pick, reset), [`stash`], [`remote`] (fetch, pull, push over `file://`, `http(s)://` and
//!   ssh, with libgit2's credential callback), [`worktree`] and [`blame`].
//! - [`credentials`] (brief 0045): the order the credential callback tries the ssh agent, key files, the
//!   configured helper, the default credentials and the shell's prompt's answer ([`SessionCredentials`], in memory
//!   only, given to a handle with [`Repo::with_credentials`]); [`transport`]: the proxy order and the certificate
//!   rule (`http.sslVerify`), with refusals that name the host.
//! - [`watch`]: the [`StatusCache`] and the [`Watcher`] that keeps it current, bumping its generation when
//!   `.git/index`, `.git/HEAD`, `.git/refs/` or the working tree change (debounced 200 ms).
//! - [`Cancel`]: the token every long call checks.
//!
//! Every call blocks its thread on libgit2: the shell runs them off the UI thread.

pub mod blame;
pub mod branches;
pub mod commit;
pub mod credentials;
pub mod diff;
mod error;
pub mod index;
pub mod log;
pub mod remote;
pub mod stash;
pub mod status;
pub mod transport;
pub mod watch;
pub mod worktree;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use credentials::{SessionCredentials, UserPass};
pub use error::{ErrorKind, GitError};
pub use git2;
pub use git2::Oid;
pub use status::{Change, ChangeKind, FileGlyph, GlyphIndex, Operation, Status};
pub use watch::{StatusCache, WatchOptions, Watcher};

pub type Result<T, E = GitError> = std::result::Result<T, E>;

/// Where the global git configuration comes from: the user's (`~/.gitconfig`, `$XDG_CONFIG_HOME/git/config`, the
/// system file), or exactly these files (tests: none, so the machine's identity never leaks in).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GlobalConfig {
    #[default]
    User,
    Files(Vec<PathBuf>),
}

/// A cancellation token: set it from any thread; long calls check it and stop with [`ErrorKind::Canceled`].
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// `Err(Canceled)` once canceled.
    pub fn check(&self) -> Result<()> {
        if self.is_canceled() {
            Err(GitError::new(ErrorKind::Canceled, "canceled"))
        } else {
            Ok(())
        }
    }
}

/// One repository: its working tree and git directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    workdir: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    global: GlobalConfig,
    credentials: SessionCredentials,
}

impl Repo {
    /// The repository `path` is in (as `git` finds it, upward), or `None` when it is in none. Bare repositories have
    /// no working tree and count as none.
    pub fn discover(path: &Path) -> Result<Option<Repo>> {
        match git2::Repository::discover(path) {
            Ok(r) => Ok(Self::from_repository(&r)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The repository whose working tree is exactly `path`.
    pub fn open(path: &Path) -> Result<Repo> {
        let r = git2::Repository::open(path)?;
        Self::from_repository(&r).ok_or_else(|| {
            GitError::new(
                ErrorKind::NotARepository,
                format!("{} is a bare repository", path.display()),
            )
        })
    }

    /// `git init` in `path` with the initial branch `main` (Visual Studio's Create Git Repository). Refused when
    /// `path` is already inside a repository.
    pub fn init(path: &Path) -> Result<Repo> {
        if let Some(existing) = Self::discover(path)? {
            return Err(GitError::new(
                ErrorKind::Refused,
                format!(
                    "{} is already in the repository at {}",
                    path.display(),
                    existing.workdir().display()
                ),
            ));
        }
        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main").mkpath(true);
        let r = git2::Repository::init_opts(path, &opts)?;
        Self::from_repository(&r)
            .ok_or_else(|| GitError::new(ErrorKind::Git, "the new repository has no working tree"))
    }

    fn from_repository(r: &git2::Repository) -> Option<Repo> {
        let workdir = r.workdir()?;
        Some(Repo {
            workdir: strip_slash(workdir),
            git_dir: strip_slash(r.path()),
            common_dir: strip_slash(r.commondir()),
            global: GlobalConfig::User,
            credentials: SessionCredentials::default(),
        })
    }

    /// This handle reading the global configuration from `global` instead (tests).
    pub fn with_global_config(mut self, global: GlobalConfig) -> Self {
        self.global = global;
        self
    }

    /// The working tree's root.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The git directory (`.git`, or `.git/worktrees/<name>` for a linked worktree).
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Where refs and objects live (the main repository's `.git` for a linked worktree).
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    pub fn global_config(&self) -> &GlobalConfig {
        &self.global
    }

    /// This handle offering the credentials kept in `credentials` (the shell's prompt's answers) to remotes that
    /// ask for a user name and password, after the helper and the default credentials.
    pub fn with_credentials(mut self, credentials: SessionCredentials) -> Self {
        self.credentials = credentials;
        self
    }

    pub fn credentials(&self) -> &SessionCredentials {
        &self.credentials
    }

    /// The configuration a transfer reads (`credential.helper`, `http.proxy`, `http.sslVerify`): the repository's,
    /// then the global configuration this handle reads (see [`GlobalConfig`]).
    pub fn effective_config(&self) -> Result<git2::Config> {
        match &self.global {
            GlobalConfig::User => Ok(self.repository()?.config()?),
            GlobalConfig::Files(files) => {
                let mut c = git2::Config::new()?;
                for f in files {
                    c.add_file(f, git2::ConfigLevel::Global, false)?;
                }
                let local = self.common_dir.join("config");
                if local.is_file() {
                    c.add_file(&local, git2::ConfigLevel::Local, false)?;
                }
                Ok(c)
            }
        }
    }

    /// Whether TLS certificates are verified: `http.sslVerify`, true unless set false (as in git).
    pub fn ssl_verify(&self) -> bool {
        self.effective_config()
            .ok()
            .and_then(|c| c.get_bool("http.sslVerify").ok())
            .unwrap_or(true)
    }

    /// libgit2's repository, opened for one call.
    pub fn repository(&self) -> Result<git2::Repository> {
        Ok(git2::Repository::open(&self.workdir)?)
    }

    /// `path` (relative to the working tree, or absolute inside it) as the repository names it: relative,
    /// `/`-separated.
    pub fn relative(&self, path: &str) -> Result<String> {
        let p = Path::new(path);
        let rel = if p.is_absolute() {
            let normalized = normalize(p);
            let root = normalize(&self.workdir);
            normalized
                .strip_prefix(&root)
                .map(Path::to_path_buf)
                .map_err(|_| {
                    GitError::new(
                        ErrorKind::NotFound,
                        format!(
                            "{path} is not in the repository at {}",
                            self.workdir.display()
                        ),
                    )
                })?
        } else {
            normalize(p)
        };
        let s = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if s.is_empty() || s.starts_with("..") {
            return Err(GitError::new(
                ErrorKind::NotFound,
                format!("{path} is not a file in the repository"),
            ));
        }
        Ok(s)
    }

    /// The absolute path of repository path `rel`.
    pub fn absolute(&self, rel: &str) -> PathBuf {
        self.workdir.join(rel)
    }
}

fn strip_slash(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let t = s.trim_end_matches(['/', '\\']);
    if t.is_empty() {
        p.to_path_buf()
    } else {
        PathBuf::from(t)
    }
}

/// `p` with `.` and `..` resolved lexically.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            std::path::Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// A commit id's first seven hexadecimal digits.
pub fn short(oid: Oid) -> String {
    oid.to_string()[..7].to_owned()
}

/// A commit message's first line.
pub fn summary_of(message: &str) -> String {
    message.lines().next().unwrap_or_default().trim().to_owned()
}

#[cfg(test)]
pub(crate) mod testutil;
