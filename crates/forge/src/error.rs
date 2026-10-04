//! The one error type every forge call answers with (`forge-*.output.json`'s `refresh_error`).

use serde::{Deserialize, Serialize};

/// Why a forge call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    RateLimited,
    Network,
    SignInRequired,
    Unauthorized,
    Forbidden,
    NotFound,
    Unsupported,
    Conflict,
    Invalid,
    Canceled,
    Other,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::Network => "network",
            ErrorKind::SignInRequired => "sign_in_required",
            ErrorKind::Unauthorized => "unauthorized",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Invalid => "invalid",
            ErrorKind::Canceled => "canceled",
            ErrorKind::Other => "other",
        }
    }
}

/// A failed forge call. Its message never carries a token: requests' headers are never formatted into it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeError {
    pub kind: ErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<String>,
}

impl ForgeError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            host: None,
            reset_at: None,
        }
    }

    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// `sign_in_required: <host>`: no token is stored for the host, or the forge refused it.
    pub fn sign_in_required(host: &str, why: &str) -> Self {
        Self::new(
            ErrorKind::SignInRequired,
            format!(
                "sign_in_required: {host}: {why} Sign in with Git > Sign in to the forge (agents cannot sign in: ask the \
                 person)."
            ),
        )
        .with_host(host)
    }

    pub fn unsupported(what: &str, forge: &str) -> Self {
        Self::new(
            ErrorKind::Unsupported,
            format!("{forge} does not support {what}"),
        )
    }

    pub fn canceled() -> Self {
        Self::new(ErrorKind::Canceled, "canceled")
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Invalid, message)
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, message)
    }
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ForgeError {}

pub type Result<T> = std::result::Result<T, ForgeError>;
