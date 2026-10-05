//! The updater's errors, each telling the person what to do.

use std::fmt;

/// What went wrong.
#[derive(Debug)]
pub enum Error {
    /// No HTTP answer came from `host`.
    Network {
        host: String,
        message: String,
    },
    /// An HTTP answer that is not what was asked: a 404 release list, a 403 rate limit.
    Http {
        url: String,
        status: u16,
        message: String,
    },
    /// The answer did not parse.
    Malformed(String),
    /// The archive's digest does not match `SHA256SUMS`.
    Verification {
        name: String,
        expected: String,
        actual: String,
    },
    /// The archive could not be unpacked, or holds something it must not.
    Archive(String),
    /// The install folder, the stage folder, a file.
    Io(std::io::Error),
    /// The swap failed and was undone.
    Apply(String),
    Canceled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn malformed(message: impl Into<String>) -> Self {
        Error::Malformed(message.into())
    }

    pub fn archive(message: impl Into<String>) -> Self {
        Error::Archive(message.into())
    }

    /// The error's kind, for the status output: `network`, `http`, `malformed`, `verification`, `archive`, `io`,
    /// `apply`, `canceled`.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Network { .. } => "network",
            Error::Http { .. } => "http",
            Error::Malformed(_) => "malformed",
            Error::Verification { .. } => "verification",
            Error::Archive(_) => "archive",
            Error::Io(_) => "io",
            Error::Apply(_) => "apply",
            Error::Canceled => "canceled",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Network { host, message } => {
                write!(f, "{host} could not be reached ({message})")
            }
            Error::Http {
                url,
                status,
                message,
            } => write!(f, "{url} answered {status}: {message}"),
            Error::Malformed(m) => write!(f, "the answer is not what was expected: {m}"),
            Error::Verification {
                name,
                expected,
                actual,
            } => write!(
                f,
                "{name} does not match SHA256SUMS (expected {expected}, got {actual}); it was deleted"
            ),
            Error::Archive(m) => write!(f, "the archive could not be unpacked: {m}"),
            Error::Io(e) => write!(f, "{e}"),
            Error::Apply(m) => write!(f, "the update was not applied: {m}"),
            Error::Canceled => f.write_str("canceled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
