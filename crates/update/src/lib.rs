//! `eludite-update`: Eludite updates itself from GitHub releases, by channel (brief 0055, ADR-0011).
//!
//! **What a build is.** A packaged Eludite carries `build.json` beside its executable ([`build::Build`]: the version,
//! the channel, the build id, the commit, the platform). A development build has none and the updater stays off,
//! saying so. **A channel** ([`build::Channel`]) is a rule for which releases of the repository belong to it: `unstable`
//! is every release tagged `unstable-<build>` (CI publishes one per green build of `main`). Build ids
//! ([`build::BuildId`]) compare segment by segment, so `20261006.3` is newer than `20261005.142`.
//!
//! **A release** ([`release`]) is read from GitHub's release list with a conditional request (an `ETag`, so an
//! unchanged list costs nothing against the rate limit). The channel's newest release is chosen; its archive for
//! this platform, `eludite-<version>-<os>-<arch>.tar.gz` (`.zip` on Windows), and its `SHA256SUMS` are the download.
//! The archive is streamed to the stage with its SHA-256 computed as it arrives and kept only when the digest matches.
//!
//! **The stage** ([`stage`]) is `.eludite-update/` inside the install folder, so the swap is a rename on one
//! filesystem. The archive is unpacked there ([`extract`]: `tar` and `flate2`, or the small [`zip`] reader) with its
//! top folder stripped, checked (the executable, `build.json` naming the expected build and platform) and recorded in
//! `staged.json`. An install folder that cannot be written (a root-owned `/opt`) is reported, not fought.
//!
//! **The swap** ([`apply`]) runs after the shell quits, in a copy of the new executable started with a plan: the
//! install folder's entries move to `.eludite-previous/`, the layout's move in, the new Eludite starts with the old
//! arguments and `--updated-from <build>`. A failure undoes the renames and starts the old build. The next
//! successful start removes `.eludite-previous/`.
//!
//! **The updater** ([`updater::Updater`]) is one worker thread with a [`updater::Status`] snapshot and a listener; it
//! does nothing until asked. The shell asks on Help > Check for Updates, on `eludite.update.*`, and from a timer
//! when the mode says so ([`updater::Mode`]: `ask` until the person answers once, `notify`, `download`, `off`). No
//! network call happens at startup.
//!
//! **Public API boundary.** The shell uses [`updater::Updater`] with [`updater::Setup`] and [`updater::Config`],
//! [`apply::run`] for `--apply-update`, [`apply::launch`] and [`apply::plan`] to restart into a staged build, and
//! [`stage::Stage::clean_after_start`]. The rest is public for the tests and the tools; [`test_support`] (feature
//! `test-support`) is the loopback release server the shell's tests run against too.

pub mod apply;
pub mod build;
pub mod download;
pub mod error;
pub mod extract;
pub mod http;
pub mod release;
pub mod stage;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod time;
pub mod updater;
pub mod zip;

pub use build::{Build, BuildId, Channel, Platform};
pub use error::{Error, Result};
pub use updater::{Config, Mode, Setup, State, Status, Summary, Updater};

/// The repository releases are read from.
pub const REPOSITORY: &str = "sand-head/Eludite";
