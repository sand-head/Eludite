//! The shell's [`WorkspaceTarget`]: how `eludite.solution.*`, `eludite.file.*` and `eludite.editor.*` reach the
//! session and the editors from whichever thread invokes the bus.
//!
//! - `eludite.solution.open` and `close` only hand work to the [`ServerSession`] worker, so they run on any thread.
//! - The file and editor commands touch GPUI entities, which live on the UI thread. From another thread (an agent)
//!   the request is posted to the UI as a [`UiJob`] and the caller waits for the shell's answer. On the UI thread
//!   the shell applies the request itself first, then invokes the bus with the result staged in [`stage`], so the
//!   menus, keys, clicks and agents run the same command and the audit log records every call.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, ThreadId};
use std::time::Duration;

use eludite_commands::CommandError;
use eludite_commands::workspace::{
    SolutionCloseOutput, SolutionOpenOutput, WorkspaceOutput, WorkspaceRequest, WorkspaceTarget,
};
use futures::channel::mpsc::UnboundedSender;

use super::session::ServerSession;

/// How long an agent's file or editor command waits for the UI thread.
const UI_TIMEOUT: Duration = Duration::from_secs(30);

type Outcome = Result<WorkspaceOutput, CommandError>;

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// A file or editor command from another thread, for the UI thread to apply.
pub struct UiJob {
    pub request: WorkspaceRequest,
    pub reply: mpsc::SyncSender<Outcome>,
    /// Who invoked the command (an agent's edits are held for review, brief 0016).
    pub caller: eludite_commands::Caller,
}

pub struct ShellTarget {
    pub session: ServerSession,
    pub ui_thread: ThreadId,
    pub jobs: UnboundedSender<UiJob>,
}

const SOLUTION_EXTENSIONS: [&str; 4] = ["sln", "slnx", "csproj", "vbproj"];

/// Absolute path of an existing `.sln`, `.slnx`, `.csproj` or `.vbproj`.
pub fn solution_path(path: &str) -> Result<PathBuf, CommandError> {
    let p = std::path::absolute(Path::new(path))
        .map_err(|e| CommandError::InvalidInput(format!("{path}: {e}")))?;
    let ext_ok = p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        SOLUTION_EXTENSIONS
            .iter()
            .any(|x| x.eq_ignore_ascii_case(e))
    });
    if !ext_ok {
        return Err(CommandError::InvalidInput(format!(
            "{}: expected a .sln, .slnx, .csproj or .vbproj file",
            p.display()
        )));
    }
    if !p.is_file() {
        return Err(CommandError::InvalidInput(format!(
            "{} does not exist",
            p.display()
        )));
    }
    Ok(p)
}

impl WorkspaceTarget for ShellTarget {
    fn apply(&self, request: WorkspaceRequest) -> Outcome {
        match request {
            WorkspaceRequest::SolutionOpen { path } => {
                let p = solution_path(&path)?;
                self.session.open(p.clone());
                Ok(WorkspaceOutput::SolutionOpen(SolutionOpenOutput {
                    path: p.to_string_lossy().into_owned(),
                    state: "loading".into(),
                }))
            }
            WorkspaceRequest::SolutionClose => {
                let was = self.session.close();
                Ok(WorkspaceOutput::SolutionClose(SolutionCloseOutput {
                    closed: was.is_some(),
                    path: was.map(|p| p.to_string_lossy().into_owned()),
                }))
            }
            other if thread::current().id() == self.ui_thread => {
                STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                    Err(CommandError::Failed(format!(
                        "{} runs on the UI thread through the shell",
                        other.command()
                    )))
                })
            }
            other => {
                let (reply, rx) = mpsc::sync_channel(1);
                self.jobs
                    .unbounded_send(UiJob {
                        request: other,
                        reply,
                        caller: eludite_commands::current_caller(),
                    })
                    .map_err(|_| CommandError::Failed("the window is closed".into()))?;
                rx.recv_timeout(UI_TIMEOUT)
                    .map_err(|_| CommandError::Failed("the UI did not answer".into()))?
            }
        }
    }
}
