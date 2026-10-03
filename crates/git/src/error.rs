//! The crate's error: what kind of refusal or failure, and a message a person or an agent can act on.

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The folder is in no repository.
    NotARepository,
    /// No commit identity: the message names `git config user.name` and the settings.
    Identity,
    /// Nothing is staged (or changed, for Commit All).
    NothingToCommit,
    /// Files are conflicted; [`GitError::paths`] lists them.
    Conflicted,
    /// Local changes stand in the way; [`GitError::paths`] lists them.
    Dirty,
    /// The operation is refused as asked (an unmerged branch without `force`, a non-fast-forward push).
    Refused,
    Canceled,
    /// Authentication failed; the message says what was tried.
    Credentials,
    /// A path, branch, revision, stash or worktree that does not exist.
    NotFound,
    /// Anything else libgit2 reported.
    Git,
}

/// A failed git call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub kind: ErrorKind,
    pub message: String,
    /// The paths a conflict or a dirty tree names.
    pub paths: Vec<String>,
}

impl GitError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            paths: Vec::new(),
        }
    }

    pub fn with_paths(mut self, paths: Vec<String>) -> Self {
        self.paths = paths;
        self
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

impl From<git2::Error> for GitError {
    fn from(e: git2::Error) -> Self {
        let kind = match e.code() {
            git2::ErrorCode::NotFound => ErrorKind::NotFound,
            git2::ErrorCode::Auth => ErrorKind::Credentials,
            git2::ErrorCode::User => ErrorKind::Canceled,
            git2::ErrorCode::Conflict | git2::ErrorCode::MergeConflict => ErrorKind::Dirty,
            git2::ErrorCode::Unmerged => ErrorKind::Conflicted,
            git2::ErrorCode::NotFastForward => ErrorKind::Refused,
            _ => ErrorKind::Git,
        };
        Self::new(kind, e.message().to_owned())
    }
}

impl From<std::io::Error> for GitError {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorKind::Git, e.to_string())
    }
}
