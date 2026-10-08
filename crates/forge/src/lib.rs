//! `eludite-forge` (brief 0046): pull requests, reviews, issues and checks on GitHub, GitLab, Azure DevOps, Forgejo
//! and Gitea (Codeberg), and Tangled, behind one [`Forge`] trait.
//!
//! The public boundary:
//!
//! - [`detect`]: which forge a remote url lives on ([`detect::detect`], the `forge.hosts` entries, the version
//!   probe), never run at startup.
//! - [`Forge`] and [`forge::capabilities`]: one implementation per family ([`github`], [`gitlab`], [`azure`],
//!   [`forgejo`] serving Gitea too, [`tangled`]) and the table the windows hide what a forge lacks by. The model they
//!   answer in is [`model`], serialized as `protocol/schemas/forge-*.output.json` describes.
//! - [`http`]: the [`http::Transport`] trait, the real [`http::UreqTransport`] (`ureq` with `rustls`), and
//!   [`client::Client`] on top (conditional requests against [`cache::Cache`], rate limits, errors as
//!   [`error::ErrorKind`]s).
//! - [`credentials`]: tokens in the operating system's credential store (`keyring`), the consented 0600 file, and
//!   the tests' [`credentials::MemoryStore`]; [`auth`]: personal access tokens, the device flows, the `gh` and `glab`
//!   CLIs' tokens, Tangled's app password sessions.
//! - [`hub::Hub`]: what the shell and the commands hold: detection with the settings, signed-in forges, the cache's
//!   stale-while-refreshing reads with a generation per repository, the local pending reviews. [`ops`] turns the
//!   `eludite.forge.*` commands' input into hub calls and their output JSON, with a [`ops::GitSide`] for what needs
//!   the repository (the current branch, a checkout, a new branch).
//! - [`replay`]: recorded fixtures, the in-process replay transport, the loopback fixture server, and the recording
//!   transport `tools/forge-corpus/record.sh` drives.
//!
//! Nothing here runs a process except [`auth::cli_token`] (`gh auth token`, `glab auth token`), at sign-in when the
//! person chooses it; nothing here keeps a token anywhere but the credential store.

pub mod auth;
pub mod azure;
pub mod cache;
pub mod client;
pub mod common;
pub mod credentials;
pub mod detect;
pub mod error;
pub mod forge;
pub mod forgejo;
pub mod github;
pub mod gitlab;
pub mod http;
pub mod hub;
pub mod model;
pub mod ops;
mod process;
pub mod replay;
pub mod tangled;
pub mod util;

pub use error::{ErrorKind, ForgeError, Result};
pub use forge::{Forge, PendingMode};
pub use model::*;
