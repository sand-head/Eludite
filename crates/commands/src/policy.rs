//! The per-solution permission policy for hosted agents (PLAN.md 5.3; `protocol/schemas/agents-policy.json`): a
//! committable file, `.eludite/agents-policy.json` beside the solution, deciding what an agent may do without asking.
//!
//! - Class **read** always runs; it is not configurable.
//! - **edit_buffer**: `review` (the default) holds the agent's edits as pending changes; `accept` applies them at once.
//! - **execute**: `prompt` (the default), `allow` or `deny`, unless a rule matches.
//! - **dangerous**: `prompt` (the default) or `deny`, unless a rule matches.
//! - **Rules** name a tool (as the agent names it; an Eludite MCP tool also by its bare name) and optionally the start
//!   of its shell command. Always Allow in a permission prompt adds one, and the file is rewritten with sorted keys
//!   so it diffs cleanly in review.
//! - **`browser`** (brief 0024): `origins` (where an agent may send the browser; [`BrowserPolicy::check_url`]),
//!   `network_bodies` and `evaluate`. The browser commands read it through their escalation hooks (ADR-0009), which
//!   see a [`PolicyView`]: this policy, the workspace folder and its launch urls, loaded on first use.
//! - **`debug`** (brief 0027, proposal 0001 section 5.5): `drive` (agents starting, attaching, restarting, resuming and
//!   changing a debugging session), `attach` (attaching to a process Eludite did not start) and `evaluate` (running
//!   debuggee code through expressions), applied by the debug commands' escalation hooks through
//!   [`DebugPolicy::decide`]: `prompt` makes a call dangerous (Always Allow writes `allow`, [`AlwaysAllow::Debug`]),
//!   `deny` refuses it for an agent with the policy named. Tool rules are checked first ([`DebugPolicy::decide_for`]).
//!   Whether a process is one Eludite started is the shell's knowledge, given to the hooks as
//!   [`PolicySnapshot::launched`].
//! - **`git`** (brief 0040): `commit` (an agent's commits without `amend`), `push` and `history` (amend, reset, rebase,
//!   a pull that rebases, aborting a merge), applied by the git commands' escalation hooks through
//!   [`GitPolicy::decide_for`]: `prompt` makes a call dangerous (Always Allow writes a tool rule), `deny` refuses it for
//!   an agent with the policy named; tool rules are checked first. A `force` is refused for agents whatever the policy
//!   and the rules say ([`GitCall::force`]).
//! - **`terminal`** (brief 0041): `run` (agents opening terminals and typing into them, resizing, clearing and
//!   closing them), applied by the terminal commands' escalation hooks through [`TerminalPolicy::decide`]: `prompt`
//!   (the default) makes the first such call of an agent session dangerous, and its "Allow for this session"
//!   ([`AlwaysAllow::Session`]) grants the rest of the session without writing anything ([`PolicySnapshot::session_grants`]);
//!   `allow` lets them run ([`AlwaysAllow::Granted`]); `deny` refuses them. Reading terminals is always allowed.
//! - **`forge`** (brief 0046): `read` (`allow`, or `deny` refusing agents' reads), `write` (comments, reviews,
//!   creating and editing pull requests and issues, reruns: `prompt`, the default, asks once per agent session with
//!   "Allow for this session" as `terminal.run` does; `allow`; `deny`), `merge` (merging and closing pull requests:
//!   `deny`, the default, or `prompt`, asking every time; a merge's `force` is refused) and `sign_in` (always `deny`:
//!   an agent never signs in or out), applied by the forge commands' escalation hooks through [`ForgePolicy`].
//! - **`nuget`** (brief 0048): `change` (an agent's install, uninstall, update and consolidate) and `sources` (adding,
//!   removing, enabling and disabling package sources), applied by the NuGet commands' escalation hooks through
//!   [`NuGetPolicy::decide_for`]: `prompt` (the default for both) makes a call dangerous (Always Allow writes a tool
//!   rule), `allow` (`change` only) leaves it at class execute, `deny` refuses it for an agent with the policy named;
//!   tool rules are checked first. Reads (search, installed, updates, the sources list) are always allowed.
//! - **Escalated calls** ([`crate::CallClass`]): a call whose class a hook raised is decided by [`AgentPolicy::decide_call`],
//!   where allow rules apply only when the hook says so ([`AlwaysAllow::Rule`]), and Always Allow remembers what the
//!   hook names ([`AgentPolicy::remember`]): a rule, an origin, or nothing.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CallClass, PermissionClass};

/// The policy file, relative to the solution's folder.
pub const POLICY_FILE: &str = ".eludite/agents-policy.json";

/// The prefix Claude-style agents give MCP tools of the `eludite` server.
const MCP_PREFIX: &str = "mcp__eludite__";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditPolicy {
    #[default]
    Review,
    Accept,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutePolicy {
    #[default]
    Prompt,
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DangerousPolicy {
    #[default]
    Prompt,
    Deny,
}

/// `browser.network_bodies`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkBodiesPolicy {
    #[default]
    Allow,
    Deny,
}

/// `browser.evaluate`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatePolicy {
    #[default]
    Allow,
    Prompt,
    Deny,
}

/// `debug.drive`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrivePolicy {
    #[default]
    Allow,
    Prompt,
    Deny,
}

/// `debug.attach`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachPolicy {
    #[default]
    Prompt,
    Deny,
}

/// A key of the `debug` object that Always Allow sets to `allow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DebugKnob {
    Drive,
    Evaluate,
}

impl DebugKnob {
    pub fn key(self) -> &'static str {
        match self {
            DebugKnob::Drive => "debug.drive",
            DebugKnob::Evaluate => "debug.evaluate",
        }
    }
}

/// `agents-policy.json`'s `debug` object (brief 0027).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<DrivePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<AttachPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate: Option<EvaluatePolicy>,
}

/// What a debug command's call does, for the `debug` policy: whether it drives the session, runs debuggee code
/// through an expression, and attaches (to a process Eludite started, or not).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DebugCall {
    pub drive: bool,
    pub evaluate: bool,
    /// `Some(foreign)` for an attach: `true` when the process is not one Eludite started.
    pub attach: Option<bool>,
}

impl DebugPolicy {
    /// What the policy makes of `call`: a refusal (`deny`, naming the key), a raise to dangerous (`prompt`, or an
    /// attach to a process Eludite did not start), or `None` (the command's declared class).
    pub fn decide(&self, call: DebugCall) -> Option<crate::Escalation> {
        use crate::Escalation;
        let drive = self.drive.unwrap_or_default();
        let evaluate = self.evaluate.unwrap_or_default();
        if call.attach.is_some() && self.attach.unwrap_or_default() == AttachPolicy::Deny {
            return Some(Escalation::Refuse(
                "the solution's policy sets debug.attach to deny".into(),
            ));
        }
        if call.drive && drive == DrivePolicy::Deny {
            return Some(Escalation::Refuse(
                "the solution's policy sets debug.drive to deny".into(),
            ));
        }
        if call.evaluate && evaluate == EvaluatePolicy::Deny {
            return Some(Escalation::Refuse(
                "the solution's policy sets debug.evaluate to deny".into(),
            ));
        }
        let mut reasons = Vec::new();
        let mut knobs = Vec::new();
        let foreign = call.attach == Some(true);
        if foreign {
            reasons.push("attach to a process Eludite did not start".to_owned());
        }
        if call.drive && drive == DrivePolicy::Prompt {
            reasons.push(
                "the solution's policy asks before an agent drives the debugger (debug.drive: prompt)".into(),
            );
            knobs.push(DebugKnob::Drive);
        }
        if call.evaluate && evaluate == EvaluatePolicy::Prompt {
            reasons.push(
                "the solution's policy asks before an agent runs debuggee code (debug.evaluate: prompt)".into(),
            );
            knobs.push(DebugKnob::Evaluate);
        }
        if reasons.is_empty() {
            return None;
        }
        Some(Escalation::Raise {
            class: PermissionClass::Dangerous,
            reason: reasons.join("; "),
            // An attach to a foreign process is allowed once: `attach` has no `allow` to write.
            always_allow: if foreign {
                AlwaysAllow::Never
            } else {
                AlwaysAllow::Debug(knobs)
            },
        })
    }

    /// [`DebugPolicy::decide`] for a call of `tool` with `input` under `rules`: a tool rule is checked first (proposal
    /// 0001 section 5.5), so when one matches, a refusal becomes a raise to dangerous and every raise lets the rules
    /// decide ([`AlwaysAllow::Rule`]).
    pub fn decide_for(
        &self,
        call: DebugCall,
        rules: &[PolicyRule],
        tool: &str,
        input: &Value,
    ) -> Option<crate::Escalation> {
        use crate::Escalation;
        let e = self.decide(call)?;
        if !rules.iter().any(|r| r.matches(tool, input)) {
            return Some(e);
        }
        Some(match e {
            Escalation::Refuse(why) => Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: why,
                always_allow: AlwaysAllow::Rule,
            },
            Escalation::Raise { class, reason, .. } => Escalation::Raise {
                class,
                reason,
                always_allow: AlwaysAllow::Rule,
            },
        })
    }
}

/// `git.commit`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitPolicy {
    #[default]
    Allow,
    Prompt,
    Deny,
}

/// `git.push` and `git.history`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardPolicy {
    #[default]
    Prompt,
    Deny,
}

/// `agents-policy.json`'s `git` object (brief 0040).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<GuardPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<GuardPolicy>,
}

/// What a git command's call does, for the `git` policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GitCall {
    /// A commit without `amend`.
    pub commit: bool,
    pub push: bool,
    /// Rewrites or moves history: amend, reset, rebase, a pull that rebases, aborting a merge.
    pub history: bool,
    /// Overwrites without asking (checkout, push, branch delete, worktree remove): refused for agents outright.
    pub force: bool,
}

/// The refusal of every `force` for agents.
pub const GIT_FORCE_REFUSED: &str =
    "agents may not use `force` with git commands (brief 0040): ask the user to do it in the IDE";

impl GitPolicy {
    /// What the policy makes of `call`: a refusal (`deny`, or any `force`), a raise to dangerous (`prompt`), or
    /// `None` (the command's class).
    pub fn decide(&self, call: GitCall) -> Option<crate::Escalation> {
        use crate::Escalation;
        if call.force {
            return Some(Escalation::Refuse(GIT_FORCE_REFUSED.into()));
        }
        let commit = self.commit.unwrap_or_default();
        let push = self.push.unwrap_or_default();
        let history = self.history.unwrap_or_default();
        if call.commit && commit == CommitPolicy::Deny {
            return Some(Escalation::Refuse(
                "the solution's policy sets git.commit to deny".into(),
            ));
        }
        if call.push && push == GuardPolicy::Deny {
            return Some(Escalation::Refuse(
                "the solution's policy sets git.push to deny".into(),
            ));
        }
        if call.history && history == GuardPolicy::Deny {
            return Some(Escalation::Refuse(
                "the solution's policy sets git.history to deny".into(),
            ));
        }
        let mut reasons = Vec::new();
        if call.commit && commit == CommitPolicy::Prompt {
            reasons.push("the solution's policy asks before an agent commits (git.commit: prompt)");
        }
        if call.push && push == GuardPolicy::Prompt {
            reasons.push("the solution's policy asks before an agent pushes (git.push: prompt)");
        }
        if call.history && history == GuardPolicy::Prompt {
            reasons.push(
                "the solution's policy asks before an agent rewrites history (git.history: prompt)",
            );
        }
        if reasons.is_empty() {
            return None;
        }
        Some(Escalation::raise(
            PermissionClass::Dangerous,
            reasons.join("; "),
        ))
    }

    /// [`GitPolicy::decide`] for a call of `tool` with `input` under `rules`: a matching tool rule turns a policy
    /// refusal into a raise to dangerous that the rule decides; a `force` stays refused.
    pub fn decide_for(
        &self,
        call: GitCall,
        rules: &[PolicyRule],
        tool: &str,
        input: &Value,
    ) -> Option<crate::Escalation> {
        use crate::Escalation;
        let e = self.decide(call)?;
        if call.force || !rules.iter().any(|r| r.matches(tool, input)) {
            return Some(e);
        }
        Some(match e {
            Escalation::Refuse(why) => Escalation::raise(PermissionClass::Dangerous, why),
            raise => raise,
        })
    }
}

/// `nuget.change`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NuGetChangePolicy {
    #[default]
    Prompt,
    Allow,
    Deny,
}

/// `agents-policy.json`'s `nuget` object (brief 0048).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NuGetPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<NuGetChangePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<GuardPolicy>,
}

/// What a NuGet command's call does, for the `nuget` policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NuGetCall {
    /// Install, uninstall, update or consolidate.
    pub change: bool,
    /// Add, remove, enable or disable a package source.
    pub sources: bool,
}

impl NuGetPolicy {
    /// What the policy makes of `call`: a refusal (`deny`), a raise to dangerous (`prompt`), or `None` (the command's
    /// class: execute).
    pub fn decide(&self, call: NuGetCall) -> Option<crate::Escalation> {
        use crate::Escalation;
        if call.change {
            return match self.change.unwrap_or_default() {
                NuGetChangePolicy::Deny => Some(Escalation::Refuse(
                    "the solution's policy sets nuget.change to deny".into(),
                )),
                NuGetChangePolicy::Prompt => Some(Escalation::raise(
                    PermissionClass::Dangerous,
                    "the solution's policy asks before an agent changes packages (nuget.change: prompt)",
                )),
                NuGetChangePolicy::Allow => None,
            };
        }
        if call.sources {
            return match self.sources.unwrap_or_default() {
                GuardPolicy::Deny => Some(Escalation::Refuse(
                    "the solution's policy sets nuget.sources to deny".into(),
                )),
                GuardPolicy::Prompt => Some(Escalation::raise(
                    PermissionClass::Dangerous,
                    "the solution's policy asks before an agent changes package sources (nuget.sources: prompt)",
                )),
            };
        }
        None
    }

    /// [`NuGetPolicy::decide`] for a call of `tool` with `input` under `rules`: a matching tool rule turns a policy
    /// refusal into a raise to dangerous that the rule decides.
    pub fn decide_for(
        &self,
        call: NuGetCall,
        rules: &[PolicyRule],
        tool: &str,
        input: &Value,
    ) -> Option<crate::Escalation> {
        use crate::Escalation;
        let e = self.decide(call)?;
        if !rules.iter().any(|r| r.matches(tool, input)) {
            return Some(e);
        }
        Some(match e {
            Escalation::Refuse(why) => Escalation::raise(PermissionClass::Dangerous, why),
            raise => raise,
        })
    }
}

/// `terminal.run`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPolicy {
    #[default]
    Prompt,
    Allow,
    Deny,
}

/// `agents-policy.json`'s `terminal` object (brief 0041).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunPolicy>,
}

/// The session grant "Allow for this session" gives for `terminal.run`.
pub const TERMINAL_RUN_GRANT: &str = "terminal.run";

impl TerminalPolicy {
    /// What the policy makes of an agent's call that runs something in a terminal: a refusal (`deny`), a raise to
    /// dangerous asked once per agent session (`prompt` until `granted`), or a call that runs without asking
    /// (`allow`, or `prompt` once granted). A `kill` is dangerous whatever the grant (it ends what runs).
    pub fn decide(&self, granted: bool, kill: bool) -> crate::Escalation {
        use crate::Escalation;
        let run = self.run.unwrap_or_default();
        if run == RunPolicy::Deny {
            return Escalation::Refuse("the solution's policy sets terminal.run to deny".into());
        }
        if kill {
            return Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "`kill` ends whatever runs in the terminal".into(),
                always_allow: AlwaysAllow::Never,
            };
        }
        match (run, granted) {
            (RunPolicy::Allow, _) => Escalation::Raise {
                class: PermissionClass::Execute,
                reason: "the solution's policy lets agents run commands in a terminal (terminal.run: allow)".into(),
                always_allow: AlwaysAllow::Granted,
            },
            (_, true) => Escalation::Raise {
                class: PermissionClass::Execute,
                reason: "allowed for this agent session (terminal.run: prompt)".into(),
                always_allow: AlwaysAllow::Granted,
            },
            _ => Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "the solution's policy asks before an agent runs commands in a terminal (terminal.run: \
                         prompt); Allow for this session holds until the agent's session ends"
                    .into(),
                always_allow: AlwaysAllow::Session(TERMINAL_RUN_GRANT.into()),
            },
        }
    }
}

/// `forge.read`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeReadPolicy {
    #[default]
    Allow,
    Deny,
}

/// `forge.merge`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeMergePolicy {
    #[default]
    Deny,
    Prompt,
}

/// `forge.sign_in`: only `deny` exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeSignInPolicy {
    #[default]
    Deny,
}

/// `agents-policy.json`'s `forge` object (brief 0046).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgePolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<ForgeReadPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<RunPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<ForgeMergePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<ForgeSignInPolicy>,
}

/// The session grant "Allow for this session" gives for `forge.write`.
pub const FORGE_WRITE_GRANT: &str = "forge.write";

/// Why an agent's sign-in is refused.
pub const FORGE_SIGN_IN_REFUSED: &str = "an agent never signs in to a forge or out of one (forge.sign_in: deny): ask the person to use Git > Sign in";

impl ForgePolicy {
    /// An agent's read: refused under `read: deny`, else its declared class.
    pub fn decide_read(&self) -> Option<crate::Escalation> {
        (self.read.unwrap_or_default() == ForgeReadPolicy::Deny).then(|| {
            crate::Escalation::Refuse("the solution's policy sets forge.read to deny".into())
        })
    }

    /// An agent's write (a comment, a review, a new or edited pull request or issue, a rerun): `deny` refuses,
    /// `allow` runs it at class execute, `prompt` (the default) asks once per agent session.
    pub fn decide_write(&self, granted: bool) -> crate::Escalation {
        use crate::Escalation;
        match (self.write.unwrap_or_default(), granted) {
            (RunPolicy::Deny, _) => Escalation::Refuse("the solution's policy sets forge.write to deny".into()),
            (RunPolicy::Allow, _) => Escalation::Raise {
                class: PermissionClass::Execute,
                reason: "the solution's policy lets agents write to the forge (forge.write: allow)".into(),
                always_allow: AlwaysAllow::Granted,
            },
            (RunPolicy::Prompt, true) => Escalation::Raise {
                class: PermissionClass::Execute,
                reason: "allowed for this agent session (forge.write: prompt)".into(),
                always_allow: AlwaysAllow::Granted,
            },
            (RunPolicy::Prompt, false) => Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "the solution's policy asks before an agent writes to the forge (forge.write: prompt); Allow \
                         for this session holds until the agent's session ends"
                    .into(),
                always_allow: AlwaysAllow::Session(FORGE_WRITE_GRANT.into()),
            },
        }
    }

    /// An agent's merge or close: `deny` (the default) refuses, `prompt` asks every time; `force` is refused.
    pub fn decide_merge(&self, force: bool) -> crate::Escalation {
        use crate::Escalation;
        if force {
            return Escalation::Refuse(
                "`force` merges a pull request whose checks fail: refused for agents".into(),
            );
        }
        match self.merge.unwrap_or_default() {
            ForgeMergePolicy::Deny => Escalation::Refuse(
                "the solution's policy sets forge.merge to deny (the default): merging and closing pull requests is \
                 the person's"
                    .into(),
            ),
            ForgeMergePolicy::Prompt => Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "the solution's policy asks before an agent merges or closes a pull request (forge.merge: \
                         prompt)"
                    .into(),
                always_allow: AlwaysAllow::Never,
            },
        }
    }
}

/// Whether a process is one Eludite started (Start Debugging or Start Without Debugging in this session, or a child
/// process of one): by id, or by name. The shell gives it to the escalation hooks ([`PolicySnapshot::launched`]);
/// without one no process is.
#[derive(Clone, Default)]
pub struct LaunchedProcesses(Option<LaunchedCheck>);

/// Is a process (by id, or without one by name) one Eludite started?
pub type LaunchedCheck = Arc<dyn Fn(Option<u32>, Option<&str>) -> bool + Send + Sync>;

impl LaunchedProcesses {
    pub fn new(f: impl Fn(Option<u32>, Option<&str>) -> bool + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(f)))
    }

    /// Whether process `pid` (or, without a pid, a process named `name`) is one Eludite started.
    pub fn contains(&self, pid: Option<u32>, name: Option<&str>) -> bool {
        self.0.as_ref().is_some_and(|f| f(pid, name))
    }
}

impl std::fmt::Debug for LaunchedProcesses {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "LaunchedProcesses(..)"
        } else {
            "LaunchedProcesses(none)"
        })
    }
}

impl PartialEq for LaunchedProcesses {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for LaunchedProcesses {}

/// The `origins` entry for file urls under the workspace folder.
pub const WORKSPACE_ORIGIN: &str = "$workspace";
/// The `origins` entry for the workspace's launch urls.
pub const LAUNCH_URLS_ORIGIN: &str = "$launch_urls";
/// `origins` when the policy has none.
pub const DEFAULT_ORIGINS: [&str; 5] = [
    "localhost",
    "127.0.0.1",
    "[::1]",
    WORKSPACE_ORIGIN,
    LAUNCH_URLS_ORIGIN,
];

/// `agents-policy.json`'s `browser` object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origins: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_bodies: Option<NetworkBodiesPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate: Option<EvaluatePolicy>,
}

/// A url outside the allowed origins: what to name, and the origin Always Allow would add (none for urls without
/// one, such as `data:`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffOrigin {
    pub shown: String,
    pub origin: Option<String>,
}

impl BrowserPolicy {
    /// The entries in force: the file's, or [`DEFAULT_ORIGINS`].
    pub fn origins(&self) -> Vec<String> {
        match &self.origins {
            Some(o) => o.clone(),
            None => DEFAULT_ORIGINS.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// Whether an agent may send the browser to `url` without the call becoming dangerous. `about:` urls are always
    /// allowed; a url that does not parse is off the origins.
    pub fn check_url(
        &self,
        url: &str,
        workspace: Option<&Path>,
        launch_urls: &[String],
    ) -> Result<(), OffOrigin> {
        let parsed = ParsedUrl::parse(url);
        let off = |p: &Option<ParsedUrl>| OffOrigin {
            shown: p
                .as_ref()
                .and_then(ParsedUrl::origin)
                .unwrap_or_else(|| url.chars().take(200).collect()),
            origin: p.as_ref().and_then(ParsedUrl::origin),
        };
        let Some(u) = &parsed else {
            return Err(off(&parsed));
        };
        if u.scheme == "about" {
            return Ok(());
        }
        for entry in self.origins() {
            let hit = match entry.as_str() {
                WORKSPACE_ORIGIN => workspace.is_some_and(|w| u.is_file_under(w)),
                LAUNCH_URLS_ORIGIN => launch_urls
                    .iter()
                    .filter_map(|l| ParsedUrl::parse(l))
                    .any(|l| l.scheme == u.scheme && l.host == u.host && l.port() == u.port()),
                e => u.matches_entry(e),
            };
            if hit {
                return Ok(());
            }
        }
        Err(off(&parsed))
    }
}

/// The parts of a url the origin rule looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    /// Lowercase.
    pub scheme: String,
    /// Lowercase; IPv6 in brackets. Empty for urls without an authority.
    pub host: String,
    pub explicit_port: Option<u16>,
    /// For `file:` urls: the decoded, normalized path.
    pub path: Option<PathBuf>,
}

impl ParsedUrl {
    /// Parse an absolute url; `None` when it has no scheme or a malformed authority.
    pub fn parse(url: &str) -> Option<ParsedUrl> {
        let url = url.trim();
        let (scheme, rest) = url.split_once(':')?;
        if scheme.is_empty()
            || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return None;
        }
        let scheme = scheme.to_ascii_lowercase();
        let Some(after) = rest.strip_prefix("//") else {
            return Some(ParsedUrl {
                scheme,
                host: String::new(),
                explicit_port: None,
                path: None,
            });
        };
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let (authority, tail) = after.split_at(end);
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (inner, after) = v6.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(p) if !p.is_empty() => Some(p.parse().ok()?),
                Some(_) | None if after.is_empty() || after == ":" => None,
                _ => return None,
            };
            (format!("[{}]", inner.to_ascii_lowercase()), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) if !p.is_empty() => (h.to_ascii_lowercase(), Some(p.parse().ok()?)),
                Some((h, _)) => (h.to_ascii_lowercase(), None),
                None => (authority.to_ascii_lowercase(), None),
            }
        };
        let path = (scheme == "file").then(|| {
            let p = tail.split(['?', '#']).next().unwrap_or_default();
            normalize(&percent_decode(p))
        });
        Some(ParsedUrl {
            scheme,
            host,
            explicit_port: port,
            path,
        })
    }

    /// The port in effect: the explicit one, or the scheme's default.
    pub fn port(&self) -> Option<u16> {
        self.explicit_port.or(match self.scheme.as_str() {
            "http" | "ws" => Some(80),
            "https" | "wss" => Some(443),
            "ftp" => Some(21),
            _ => None,
        })
    }

    /// `scheme://host[:port]` (the port when it is not the scheme's default), or for a `file:` url the url of its
    /// path; `None` for urls without an origin (`data:`, `javascript:`, `about:`).
    pub fn origin(&self) -> Option<String> {
        if let Some(path) = &self.path {
            let p = path.to_string_lossy().replace('\\', "/");
            let p = if p.starts_with('/') {
                p
            } else {
                format!("/{p}")
            };
            return Some(format!("file://{p}"));
        }
        if self.host.is_empty() {
            return None;
        }
        let default = ParsedUrl {
            explicit_port: None,
            ..self.clone()
        }
        .port();
        Some(match self.explicit_port.filter(|p| Some(*p) != default) {
            Some(p) => format!("{}://{}:{p}", self.scheme, self.host),
            None => format!("{}://{}", self.scheme, self.host),
        })
    }

    fn is_file_under(&self, dir: &Path) -> bool {
        match &self.path {
            Some(p) if self.scheme == "file" => p.starts_with(normalize(&dir.to_string_lossy())),
            _ => false,
        }
    }

    fn host_matches(&self, pattern: &str) -> bool {
        let pattern = pattern.to_ascii_lowercase();
        match pattern.strip_prefix("*.") {
            Some(domain) => self.host == domain || self.host.ends_with(&format!(".{domain}")),
            None => self.host == pattern,
        }
    }

    /// One `origins` entry: a host or host and port (http and https), an origin, or a `file:` path.
    fn matches_entry(&self, entry: &str) -> bool {
        let entry = entry.trim();
        if entry.contains("://") {
            let Some(e) = ParsedUrl::parse(entry) else {
                return false;
            };
            if e.scheme == "file" {
                return match (&e.path, &self.path) {
                    (Some(dir), Some(p)) if self.scheme == "file" => p.starts_with(dir),
                    _ => false,
                };
            }
            return e.scheme == self.scheme
                && self.host_matches(&e.host)
                && e.explicit_port.is_none_or(|p| self.port() == Some(p));
        }
        if !matches!(self.scheme.as_str(), "http" | "https") {
            return false;
        }
        // `host`, `host:port`, `[v6]`, `[v6]:port`, `*.domain`.
        let (host, port) = if entry.starts_with('[') {
            match entry.split_once("]:") {
                Some((h, p)) => (format!("{h}]"), p.parse::<u16>().ok()),
                None => (entry.to_owned(), None),
            }
        } else {
            match entry.rsplit_once(':') {
                Some((h, p)) => match p.parse::<u16>() {
                    Ok(p) => (h.to_owned(), Some(p)),
                    Err(_) => return false,
                },
                None => (entry.to_owned(), None),
            }
        };
        self.host_matches(&host) && port.is_none_or(|p| self.port() == Some(p))
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(h, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A path with `.` and `..` resolved lexically (Windows drive paths from `file:///C:/...` keep their drive).
fn normalize(p: &str) -> PathBuf {
    let p = p.replace('\\', "/");
    // `/C:/x` from a file url is `C:/x`.
    let p = match p.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => p[1..].to_owned(),
        _ => p,
    };
    let mut out = PathBuf::new();
    for c in Path::new(&p).components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// The `applicationUrl`s of the `Properties/launchSettings.json` profiles under `workspace` (the folder itself and
/// three levels of subfolders, skipping hidden folders and build output), each split at `;`.
pub fn launch_urls(workspace: &Path) -> Vec<String> {
    const SKIP: [&str; 6] = ["bin", "obj", "node_modules", "target", "packages", "dist"];
    let mut out = Vec::new();
    let mut dirs = vec![(workspace.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let file = dir.join("Properties").join("launchSettings.json");
        if let Ok(text) = std::fs::read_to_string(&file)
            && let Ok(v) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
            && let Some(profiles) = v.get("profiles").and_then(Value::as_object)
        {
            for p in profiles.values() {
                for url in p
                    .get("applicationUrl")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .split(';')
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                {
                    if !out.iter().any(|o| o == url) {
                        out.push(url.to_owned());
                    }
                }
            }
        }
        if depth == 3 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push((e.path(), depth + 1));
            }
        }
    }
    out
}

/// What the escalation hooks see of the policy (ADR-0009): read on first use, so a hook that needs nothing costs
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicySnapshot {
    pub policy: AgentPolicy,
    /// The workspace (solution) folder.
    pub workspace: Option<PathBuf>,
    /// [`launch_urls`] of the workspace.
    pub launch_urls: Vec<String>,
    /// Which processes Eludite started (the debug commands' `attach` hook; brief 0027).
    pub launched: LaunchedProcesses,
    /// What "Allow for this session" granted the running agent session ([`AlwaysAllow::Session`]; brief 0041).
    pub session_grants: Vec<String>,
}

/// Makes the [`PolicySnapshot`] of the moment (the shell's: the open solution's policy file).
pub type PolicySource = Arc<dyn Fn() -> PolicySnapshot + Send + Sync>;

/// What an escalation hook may read: the policy, the workspace folder and its launch urls.
#[derive(Default)]
pub struct PolicyView {
    source: Option<PolicySource>,
    snapshot: OnceLock<PolicySnapshot>,
}

impl std::fmt::Debug for PolicyView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyView")
            .field("snapshot", &self.snapshot.get())
            .finish_non_exhaustive()
    }
}

impl PolicyView {
    /// A view of a fixed snapshot.
    pub fn of(snapshot: PolicySnapshot) -> Self {
        let view = Self::default();
        let _ = view.snapshot.set(snapshot);
        view
    }

    /// A view that asks `source` on first use.
    pub fn from_source(source: Option<PolicySource>) -> Self {
        Self {
            source,
            snapshot: OnceLock::new(),
        }
    }

    fn get(&self) -> &PolicySnapshot {
        self.snapshot
            .get_or_init(|| self.source.as_ref().map(|s| s()).unwrap_or_default())
    }

    pub fn policy(&self) -> &AgentPolicy {
        &self.get().policy
    }

    pub fn workspace(&self) -> Option<&Path> {
        self.get().workspace.as_deref()
    }

    pub fn launch_urls(&self) -> &[String] {
        &self.get().launch_urls
    }

    /// The `browser` object (its defaults when absent).
    pub fn browser(&self) -> BrowserPolicy {
        self.policy().browser.clone().unwrap_or_default()
    }

    /// The `debug` object (its defaults when absent).
    pub fn debug(&self) -> DebugPolicy {
        self.policy().debug.clone().unwrap_or_default()
    }

    /// The `git` object (its defaults when absent).
    pub fn git(&self) -> GitPolicy {
        self.policy().git.clone().unwrap_or_default()
    }

    /// The `terminal` object (its defaults when absent).
    pub fn terminal(&self) -> TerminalPolicy {
        self.policy().terminal.clone().unwrap_or_default()
    }

    /// The `forge` object (its defaults when absent).
    pub fn forge(&self) -> ForgePolicy {
        self.policy().forge.clone().unwrap_or_default()
    }

    /// The `nuget` object (its defaults when absent).
    pub fn nuget(&self) -> NuGetPolicy {
        self.policy().nuget.clone().unwrap_or_default()
    }

    /// Whether "Allow for this session" granted `key` to the running agent session.
    pub fn session_granted(&self, key: &str) -> bool {
        self.get().session_grants.iter().any(|g| g == key)
    }

    /// Which processes Eludite started.
    pub fn launched(&self) -> &LaunchedProcesses {
        &self.get().launched
    }

    /// [`BrowserPolicy::check_url`] with this view's workspace and launch urls.
    pub fn check_url(&self, url: &str) -> Result<(), OffOrigin> {
        self.browser()
            .check_url(url, self.workspace(), self.launch_urls())
    }

    /// Whether `path` is in the workspace folder (lexically, after resolving `.` and `..`); false without one.
    pub fn in_workspace(&self, path: &str) -> bool {
        self.workspace().is_some_and(|w| {
            let p = normalize(path);
            p.is_absolute() && p.starts_with(normalize(&w.to_string_lossy()))
        })
    }
}

/// What Always Allow remembers for a call (an escalation hook says; ADR-0009).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum AlwaysAllow {
    /// A rule for the tool (the calls of every command without an escalation). Rules apply to the call.
    #[default]
    Rule,
    /// Add an origin to `browser.origins`. Allow rules do not apply to the call.
    Origin(String),
    /// Nothing: Always Allow allows this call once. Allow rules do not apply to the call.
    Never,
    /// Set these keys of the `debug` object to `allow` (brief 0027). Allow rules do not apply to the call.
    Debug(Vec<DebugKnob>),
    /// "Allow for this session" (brief 0041): grant this key to the running agent session; nothing is written.
    /// Allow rules do not apply to the call.
    Session(String),
    /// The policy (or a session grant) allows the call already: it runs without asking unless a deny rule matches
    /// (brief 0041). Its hook may give it at its declared class.
    Granted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    Allow,
    Deny,
}

/// One rule of the policy file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_prefix: Option<String>,
    pub decision: RuleDecision,
}

impl PolicyRule {
    /// Whether the rule is for `tool` (as the agent names it; an Eludite MCP tool also by its bare name) and, with a
    /// `command_prefix`, for `input`'s shell command.
    pub fn matches(&self, tool: &str, input: &Value) -> bool {
        let named = self.tool == tool
            || tool
                .strip_prefix(MCP_PREFIX)
                .is_some_and(|bare| bare == self.tool)
            || self
                .tool
                .strip_prefix(MCP_PREFIX)
                .is_some_and(|bare| bare == tool);
        named
            && self.command_prefix.as_deref().is_none_or(|prefix| {
                shell_command(input).is_some_and(|c| c.trim_start().starts_with(prefix))
            })
    }
}

/// The shell command a tool call carries (`command`, as Claude's Bash and ACP terminals name it).
pub fn shell_command(input: &Value) -> Option<&str> {
    input.get("command").and_then(Value::as_str)
}

/// `agents-policy.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPolicy {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_buffer: Option<EditPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execute: Option<ExecutePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangerous: Option<DangerousPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<PolicyRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<ForgePolicy>,
    pub nuget: Option<NuGetPolicy>,
}

impl Default for AgentPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            edit_buffer: None,
            execute: None,
            dangerous: None,
            rules: Vec::new(),
            browser: None,
            debug: None,
            git: None,
            terminal: None,
            forge: None,
            nuget: None,
        }
    }
}

/// What the policy says about one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Run it, with the reason shown in the transcript.
    Allow(String),
    Deny(String),
    /// Ask the user.
    Ask,
    /// An edit: hold it as a pending change for review.
    Review,
}

impl AgentPolicy {
    /// The policy file of the solution in `solution_dir`.
    pub fn path_for(solution_dir: &Path) -> PathBuf {
        solution_dir.join(POLICY_FILE)
    }

    /// Read `path`; a missing file is the defaults, a malformed or newer one an error naming the file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let p: Self =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if p.version != 1 {
            return Err(format!(
                "{}: version {} is not supported (1 is)",
                path.display(),
                p.version
            ));
        }
        Ok(p)
    }

    /// Write `path` (creating `.eludite/`) atomically: a temporary file beside it, then a rename.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Keys sorted, so the file diffs cleanly (whether or not serde_json preserves insertion order in this build).
        let value = sorted(serde_json::to_value(self).map_err(std::io::Error::other)?);
        let mut text = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
        text.push('\n');
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Decide a call of `tool` (as the agent names it) of class `class` with `input`.
    pub fn decide(&self, class: PermissionClass, tool: &str, input: &Value) -> Verdict {
        self.decide_with(class, tool, input, true)
    }

    /// Decide a call whose class an escalation hook may have raised (ADR-0009): its effective class, with allow
    /// rules applying only when the hook says Always Allow writes rules ([`AlwaysAllow::Rule`]); deny rules always
    /// apply. A refused call is denied with the refusal.
    pub fn decide_call(&self, call: &CallClass, tool: &str, input: &Value) -> Verdict {
        if let Some(why) = &call.refused {
            return Verdict::Deny(why.clone());
        }
        if call.always_allow == AlwaysAllow::Granted {
            if let Some(r) = self
                .rules
                .iter()
                .find(|r| r.decision == RuleDecision::Deny && r.matches(tool, input))
            {
                return Verdict::Deny(format!("the solution's policy rule for {}", r.tool));
            }
            return Verdict::Allow(
                call.reason
                    .clone()
                    .unwrap_or_else(|| "the solution's policy allows it".into()),
            );
        }
        self.decide_with(
            call.class,
            tool,
            input,
            call.always_allow == AlwaysAllow::Rule,
        )
    }

    fn decide_with(
        &self,
        class: PermissionClass,
        tool: &str,
        input: &Value,
        allow_rules: bool,
    ) -> Verdict {
        let rule = || {
            self.rules
                .iter()
                .filter(|r| allow_rules || r.decision == RuleDecision::Deny)
                .find(|r| r.matches(tool, input))
        };
        match class {
            PermissionClass::Read => Verdict::Allow(format!("{tool} is class read")),
            PermissionClass::EditBuffer => match self.edit_buffer.unwrap_or_default() {
                EditPolicy::Review => Verdict::Review,
                EditPolicy::Accept => Verdict::Allow("the policy accepts edits".into()),
            },
            PermissionClass::Execute | PermissionClass::Dangerous => {
                if let Some(r) = rule() {
                    let why = format!(
                        "the solution's policy rule for {}{}",
                        r.tool,
                        r.command_prefix
                            .as_deref()
                            .map(|p| format!(" `{p}`"))
                            .unwrap_or_default()
                    );
                    return match r.decision {
                        RuleDecision::Allow => Verdict::Allow(why),
                        RuleDecision::Deny => Verdict::Deny(why),
                    };
                }
                let class_name = class.as_str();
                let deny = || Verdict::Deny(format!("the solution's policy denies {class_name}"));
                if class == PermissionClass::Execute {
                    match self.execute.unwrap_or_default() {
                        ExecutePolicy::Prompt => Verdict::Ask,
                        ExecutePolicy::Allow => {
                            Verdict::Allow("the solution's policy allows execute".into())
                        }
                        ExecutePolicy::Deny => deny(),
                    }
                } else {
                    match self.dangerous.unwrap_or_default() {
                        DangerousPolicy::Prompt => Verdict::Ask,
                        DangerousPolicy::Deny => deny(),
                    }
                }
            }
        }
    }

    /// Always Allow: add a rule for this call (its exact shell command when it has one) unless one exists. Returns
    /// the rule.
    pub fn allow_always(&mut self, tool: &str, input: &Value) -> PolicyRule {
        let rule = PolicyRule {
            tool: tool.strip_prefix(MCP_PREFIX).unwrap_or(tool).to_owned(),
            command_prefix: shell_command(input).map(|c| c.trim().to_owned()),
            decision: RuleDecision::Allow,
        };
        if !self.rules.contains(&rule) {
            self.rules.push(rule.clone());
        }
        rule
    }

    /// Always Allow for a call: what its [`AlwaysAllow`] says (a rule, an origin, or nothing). Returns what was
    /// added, for the transcript, or `None` when nothing persists.
    pub fn remember(&mut self, call: &CallClass, tool: &str, input: &Value) -> Option<String> {
        match &call.always_allow {
            AlwaysAllow::Rule => {
                let r = self.allow_always(tool, input);
                Some(format!("a rule for {}", r.tool))
            }
            AlwaysAllow::Origin(origin) => {
                self.allow_origin(origin);
                Some(format!("the origin {origin}"))
            }
            AlwaysAllow::Debug(knobs) => {
                let debug = self.debug.get_or_insert_with(DebugPolicy::default);
                for k in knobs {
                    match k {
                        DebugKnob::Drive => debug.drive = Some(DrivePolicy::Allow),
                        DebugKnob::Evaluate => debug.evaluate = Some(EvaluatePolicy::Allow),
                    }
                }
                Some(
                    knobs
                        .iter()
                        .map(|k| format!("{}: allow", k.key()))
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            }
            // A session grant is the shell's to keep; nothing is written.
            AlwaysAllow::Never | AlwaysAllow::Session(_) | AlwaysAllow::Granted => None,
        }
    }

    /// Add `origin` to `browser.origins` (the defaults first when the list is absent), keeping it sorted.
    pub fn allow_origin(&mut self, origin: &str) {
        let browser = self.browser.get_or_insert_with(BrowserPolicy::default);
        let mut origins = browser.origins();
        if !origins.iter().any(|o| o == origin) {
            origins.push(origin.to_owned());
        }
        origins.sort();
        origins.dedup();
        browser.origins = Some(origins);
    }
}

/// `v` with every object's keys inserted in sorted order.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCHEMA: &str = include_str!("../../../protocol/schemas/agents-policy.json");

    #[test]
    fn defaults_follow_plan_5_3() {
        let p = AgentPolicy::default();
        let none = json!({});
        assert!(matches!(
            p.decide(PermissionClass::Read, "x", &none),
            Verdict::Allow(_)
        ));
        assert_eq!(
            p.decide(PermissionClass::EditBuffer, "Write", &none),
            Verdict::Review
        );
        assert_eq!(
            p.decide(PermissionClass::Execute, "Bash", &none),
            Verdict::Ask
        );
        assert_eq!(
            p.decide(PermissionClass::Dangerous, "WebFetch", &none),
            Verdict::Ask
        );
    }

    #[test]
    fn rules_classes_and_always_allow_persist() {
        let dir = std::env::temp_dir().join(format!("eludite-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = AgentPolicy::path_for(&dir);
        assert_eq!(AgentPolicy::load(&path).unwrap(), AgentPolicy::default());

        let mut p = AgentPolicy::default();
        let build = json!({"command": "dotnet build Eludite.slnx"});
        let rule = p.allow_always("Bash", &build);
        assert_eq!(
            rule.command_prefix.as_deref(),
            Some("dotnet build Eludite.slnx")
        );
        p.allow_always("Bash", &build);
        assert_eq!(p.rules.len(), 1, "no duplicate rules");
        p.allow_always(
            "mcp__eludite__eludite-solution-open",
            &json!({"path": "/a.sln"}),
        );
        assert_eq!(p.rules[1].tool, "eludite-solution-open");
        p.save(&path).unwrap();

        let loaded = AgentPolicy::load(&path).unwrap();
        assert_eq!(loaded, p);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"));
        assert!(
            text.find("\"rules\"").unwrap() < text.find("\"version\"").unwrap(),
            "sorted keys"
        );
        // The file follows its schema (the subset this crate can check by hand).
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        for k in v.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }

        assert!(matches!(
            loaded.decide(PermissionClass::Execute, "Bash", &build),
            Verdict::Allow(r) if r.contains("dotnet build")
        ));
        assert_eq!(
            loaded.decide(
                PermissionClass::Execute,
                "Bash",
                &json!({"command": "rm -rf obj"})
            ),
            Verdict::Ask
        );
        assert!(matches!(
            loaded.decide(
                PermissionClass::Execute,
                "mcp__eludite__eludite-solution-open",
                &json!({})
            ),
            Verdict::Allow(_)
        ));

        std::fs::write(
            &path,
            r#"{"version": 1, "edit_buffer": "accept", "execute": "deny", "dangerous": "deny",
                "rules": [{"tool": "Bash", "command_prefix": "git push", "decision": "allow"}]}"#,
        )
        .unwrap();
        let strict = AgentPolicy::load(&path).unwrap();
        assert!(matches!(
            strict.decide(PermissionClass::EditBuffer, "Write", &json!({})),
            Verdict::Allow(_)
        ));
        assert!(matches!(
            strict.decide(PermissionClass::Execute, "Bash", &json!({"command": "ls"})),
            Verdict::Deny(_)
        ));
        assert!(matches!(
            strict.decide(
                PermissionClass::Dangerous,
                "Bash",
                &json!({"command": "git push origin"})
            ),
            Verdict::Allow(_)
        ));
        std::fs::write(&path, r#"{"version": 2}"#).unwrap();
        assert!(AgentPolicy::load(&path).unwrap_err().contains("version 2"));
        std::fs::write(&path, r#"{"version": 1, "bogus": true}"#).unwrap();
        assert!(AgentPolicy::load(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_forge_object_loads_follows_its_schema_and_decides() {
        let p: AgentPolicy = serde_json::from_str(
            r#"{"version": 1, "forge": {"read": "allow", "write": "allow", "merge": "prompt", "sign_in": "deny"}}"#,
        )
        .unwrap();
        let f = p.forge.clone().unwrap();
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let props = &schema["properties"]["forge"]["properties"];
        for (k, v) in serde_json::to_value(&f).unwrap().as_object().unwrap() {
            assert!(props[k]["enum"].as_array().unwrap().contains(v), "{k}");
        }
        assert_eq!(
            serde_json::to_value(&f).unwrap().as_object().unwrap().len(),
            props.as_object().unwrap().len(),
            "every key of the schema has a field"
        );
        assert!(
            serde_json::from_str::<AgentPolicy>(r#"{"version": 1, "forge": {"sign_in": "allow"}}"#)
                .is_err()
        );
        // Defaults: reads run, the first write of a session asks, merges are refused.
        let d = ForgePolicy::default();
        assert_eq!(d.decide_read(), None);
        match d.decide_write(false) {
            crate::Escalation::Raise {
                class,
                always_allow,
                ..
            } => {
                assert_eq!(class, PermissionClass::Dangerous);
                assert_eq!(always_allow, AlwaysAllow::Session(FORGE_WRITE_GRANT.into()));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            d.decide_write(true),
            crate::Escalation::Raise {
                class: PermissionClass::Execute,
                always_allow: AlwaysAllow::Granted,
                ..
            }
        ));
        assert!(matches!(
            d.decide_merge(false),
            crate::Escalation::Refuse(_)
        ));
        assert!(matches!(
            f.decide_merge(false),
            crate::Escalation::Raise {
                class: PermissionClass::Dangerous,
                always_allow: AlwaysAllow::Never,
                ..
            }
        ));
        assert!(
            matches!(f.decide_merge(true), crate::Escalation::Refuse(_)),
            "force, whatever the policy"
        );
        let deny = ForgePolicy {
            read: Some(ForgeReadPolicy::Deny),
            write: Some(RunPolicy::Deny),
            ..Default::default()
        };
        assert!(matches!(
            deny.decide_read(),
            Some(crate::Escalation::Refuse(_))
        ));
        assert!(
            matches!(deny.decide_write(true), crate::Escalation::Refuse(_)),
            "deny wins over a grant"
        );
    }

    #[test]
    fn the_git_object_loads_and_follows_its_schema() {
        let dir = std::env::temp_dir().join(format!("eludite-policy-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = AgentPolicy::path_for(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version": 1, "git": {"commit": "prompt", "push": "deny", "history": "prompt"}}"#,
        )
        .unwrap();
        let p = AgentPolicy::load(&path).unwrap();
        let git = p.git.clone().unwrap();
        assert_eq!(
            (git.commit, git.push, git.history),
            (
                Some(CommitPolicy::Prompt),
                Some(GuardPolicy::Deny),
                Some(GuardPolicy::Prompt)
            )
        );
        p.save(&path).unwrap();
        assert_eq!(AgentPolicy::load(&path).unwrap(), p);
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let props = &schema["properties"]["git"]["properties"];
        for (k, v) in serde_json::to_value(&git).unwrap().as_object().unwrap() {
            assert!(props[k]["enum"].as_array().unwrap().contains(v), "{k}");
        }
        // Defaults: commit allow, push and history prompt.
        let d = GitPolicy::default();
        assert_eq!(
            d.decide(GitCall {
                commit: true,
                ..Default::default()
            }),
            None
        );
        assert!(matches!(
            d.decide(GitCall {
                push: true,
                ..Default::default()
            }),
            Some(crate::Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        assert!(matches!(
            d.decide(GitCall { force: true, ..Default::default() }),
            Some(crate::Escalation::Refuse(r)) if r == GIT_FORCE_REFUSED
        ));
        std::fs::write(&path, r#"{"version": 1, "git": {"push": "allow"}}"#).unwrap();
        assert!(AgentPolicy::load(&path).is_err(), "push has no allow");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_nuget_object_loads_and_follows_its_schema() {
        let dir = std::env::temp_dir().join(format!("eludite-policy-nuget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = AgentPolicy::path_for(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version": 1, "nuget": {"change": "allow", "sources": "deny"}}"#,
        )
        .unwrap();
        let p = AgentPolicy::load(&path).unwrap();
        let nuget = p.nuget.clone().unwrap();
        assert_eq!(
            (nuget.change, nuget.sources),
            (Some(NuGetChangePolicy::Allow), Some(GuardPolicy::Deny))
        );
        p.save(&path).unwrap();
        assert_eq!(AgentPolicy::load(&path).unwrap(), p);
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let props = &schema["properties"]["nuget"]["properties"];
        for (k, v) in serde_json::to_value(&nuget).unwrap().as_object().unwrap() {
            assert!(props[k]["enum"].as_array().unwrap().contains(v), "{k}");
        }
        let change = NuGetCall {
            change: true,
            ..Default::default()
        };
        let sources = NuGetCall {
            sources: true,
            ..Default::default()
        };
        // Defaults: both prompt (dangerous: the Agents window asks).
        let d = NuGetPolicy::default();
        for call in [change, sources] {
            assert!(matches!(
                d.decide(call),
                Some(crate::Escalation::Raise {
                    class: PermissionClass::Dangerous,
                    ..
                })
            ));
        }
        assert_eq!(d.decide(NuGetCall::default()), None);
        // allow leaves a change at class execute; deny refuses, unless a tool rule decides.
        assert_eq!(nuget.decide(change), None);
        assert!(
            matches!(nuget.decide(sources), Some(crate::Escalation::Refuse(r)) if r.contains("nuget.sources"))
        );
        let rule = PolicyRule {
            tool: "eludite-nuget-sources".into(),
            command_prefix: None,
            decision: RuleDecision::Allow,
        };
        assert!(matches!(
            nuget.decide_for(sources, &[rule], "eludite-nuget-sources", &json!({})),
            Some(crate::Escalation::Raise { .. })
        ));
        std::fs::write(&path, r#"{"version": 1, "nuget": {"sources": "allow"}}"#).unwrap();
        assert!(AgentPolicy::load(&path).is_err(), "sources has no allow");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn origins(list: &[&str]) -> BrowserPolicy {
        BrowserPolicy {
            origins: Some(list.iter().map(|s| (*s).to_owned()).collect()),
            ..Default::default()
        }
    }

    #[test]
    fn urls_parse_to_their_origins() {
        let u = ParsedUrl::parse("HTTP://User@LocalHost:5000/a?b#c").unwrap();
        assert_eq!(
            (u.scheme.as_str(), u.host.as_str(), u.port()),
            ("http", "localhost", Some(5000))
        );
        assert_eq!(u.origin().as_deref(), Some("http://localhost:5000"));
        let u = ParsedUrl::parse("https://example.com/x").unwrap();
        assert_eq!(u.port(), Some(443));
        assert_eq!(u.origin().as_deref(), Some("https://example.com"));
        assert_eq!(
            ParsedUrl::parse("https://example.com:443/")
                .unwrap()
                .origin()
                .as_deref(),
            Some("https://example.com")
        );
        let u = ParsedUrl::parse("http://[::1]:8080/").unwrap();
        assert_eq!((u.host.as_str(), u.port()), ("[::1]", Some(8080)));
        let f = ParsedUrl::parse("file:///w/site/../site/a%20b.html?x").unwrap();
        assert_eq!(f.path.as_deref(), Some(Path::new("/w/site/a b.html")));
        assert_eq!(f.origin().as_deref(), Some("file:///w/site/a b.html"));
        assert_eq!(
            ParsedUrl::parse("data:text/html,hi").unwrap().origin(),
            None
        );
        assert!(ParsedUrl::parse("no scheme").is_none());
        assert!(ParsedUrl::parse("http://host:port/").is_none());
    }

    #[test]
    fn browser_origins_default_and_match_by_scheme_host_and_port() {
        let ws = Path::new("/w");
        let launch = vec![
            "https://localhost:7001".to_owned(),
            "http://app.test:5080".to_owned(),
        ];
        let d = BrowserPolicy::default();
        let ok = |p: &BrowserPolicy, url: &str| p.check_url(url, Some(ws), &launch).is_ok();
        // Defaults: loopback on any port and http or https, files under the workspace, the launch urls.
        for url in [
            "http://localhost:5000/",
            "https://localhost/",
            "http://127.0.0.1:41234/form.html",
            "http://[::1]:8080/",
            "file:///w/site/index.html",
            "http://app.test:5080/home",
            "about:blank",
        ] {
            assert!(ok(&d, url), "{url}");
        }
        for url in [
            "https://example.com/",
            "http://app.test:5081/",
            "https://app.test:5080/",
            "file:///etc/passwd",
            "file:///w/../etc/passwd",
            "ftp://localhost/",
            "data:text/html,hi",
            "javascript:alert(1)",
        ] {
            assert!(!ok(&d, url), "{url}");
        }
        let off = d
            .check_url("https://example.com/a", Some(ws), &launch)
            .unwrap_err();
        assert_eq!(off.origin.as_deref(), Some("https://example.com"));
        assert_eq!(off.shown, "https://example.com");
        let off = d
            .check_url("data:text/html,hi", Some(ws), &launch)
            .unwrap_err();
        assert_eq!(off.origin, None);
        // No workspace: no file is under it.
        assert!(d.check_url("file:///w/a.html", None, &[]).is_err());

        // An explicit list replaces the defaults.
        let p = origins(&[
            "127.0.0.1:8080",
            "https://example.com",
            "http://api.test:9000",
            "*.corp.test",
            "file:///srv/www",
        ]);
        assert!(ok(&p, "http://127.0.0.1:8080/"));
        assert!(ok(&p, "https://127.0.0.1:8080/"));
        assert!(
            !ok(&p, "http://127.0.0.1:8081/"),
            "the port is part of the entry"
        );
        assert!(
            !ok(&p, "http://localhost:5000/"),
            "the defaults are replaced"
        );
        assert!(
            ok(&p, "https://example.com:8443/x"),
            "an origin without a port: any port"
        );
        assert!(
            !ok(&p, "http://example.com/"),
            "the scheme is part of the origin"
        );
        assert!(ok(&p, "http://api.test:9000/v1"));
        assert!(!ok(&p, "http://api.test/v1"));
        assert!(ok(&p, "https://corp.test/") && ok(&p, "http://a.b.corp.test:81/"));
        assert!(!ok(&p, "https://notcorp.test/"));
        assert!(ok(&p, "file:///srv/www/a/index.html"));
        assert!(!ok(&p, "file:///srv/www2/index.html"));
        assert!(!ok(&p, "file:///w/a.html"), "$workspace is not in the list");
    }

    #[test]
    fn always_allow_adds_the_origin_to_a_sorted_list_with_the_defaults() {
        let mut p = AgentPolicy::default();
        p.allow_origin("https://example.com");
        let list = p.browser.as_ref().unwrap().origins.clone().unwrap();
        assert_eq!(
            list,
            [
                "$launch_urls",
                "$workspace",
                "127.0.0.1",
                "[::1]",
                "https://example.com",
                "localhost"
            ]
        );
        p.allow_origin("https://example.com");
        p.allow_origin("http://a.test:81");
        let list = p.browser.as_ref().unwrap().origins.clone().unwrap();
        assert_eq!(list.len(), 7);
        assert!(list.windows(2).all(|w| w[0] <= w[1]), "{list:?}");
        assert!(
            p.browser
                .as_ref()
                .unwrap()
                .check_url("https://example.com/x", None, &[])
                .is_ok()
        );
        // remember: what the call's AlwaysAllow names.
        let mut q = AgentPolicy::default();
        let call = CallClass {
            class: PermissionClass::Dangerous,
            reason: Some("navigate off the allowed origins: https://b.test".into()),
            always_allow: AlwaysAllow::Origin("https://b.test".into()),
            refused: None,
        };
        assert_eq!(
            q.remember(&call, "mcp__eludite__eludite-browser-navigate", &json!({})),
            Some("the origin https://b.test".into())
        );
        assert!(q.rules.is_empty());
        let once = CallClass {
            always_allow: AlwaysAllow::Never,
            ..call.clone()
        };
        let before = q.clone();
        assert_eq!(
            q.remember(&once, "eludite-browser-upload", &json!({})),
            None
        );
        assert_eq!(q, before);
        let plain = CallClass::declared(PermissionClass::Execute);
        assert_eq!(
            q.remember(&plain, "mcp__eludite__eludite-browser-input", &json!({})),
            Some("a rule for eludite-browser-input".into())
        );
    }

    #[test]
    fn escalated_calls_are_not_allowed_by_rules_for_ordinary_calls() {
        let mut p = AgentPolicy {
            execute: Some(ExecutePolicy::Allow),
            ..Default::default()
        };
        p.allow_always("mcp__eludite__eludite-browser-navigate", &json!({}));
        let tool = "mcp__eludite__eludite-browser-navigate";
        let plain = CallClass::declared(PermissionClass::Execute);
        assert!(matches!(
            p.decide_call(&plain, tool, &json!({})),
            Verdict::Allow(_)
        ));
        let far = CallClass {
            class: PermissionClass::Dangerous,
            reason: Some("navigate off the allowed origins: https://example.com".into()),
            always_allow: AlwaysAllow::Origin("https://example.com".into()),
            refused: None,
        };
        assert_eq!(p.decide_call(&far, tool, &json!({})), Verdict::Ask);
        // A rule-remembering escalation (evaluate: prompt) is allowed by its rule.
        let rule = CallClass {
            always_allow: AlwaysAllow::Rule,
            ..far.clone()
        };
        assert!(matches!(
            p.decide_call(&rule, tool, &json!({})),
            Verdict::Allow(_)
        ));
        // Deny rules always apply; refusals are denials.
        p.rules.push(PolicyRule {
            tool: "eludite-browser-navigate".into(),
            command_prefix: None,
            decision: RuleDecision::Deny,
        });
        p.rules.remove(0);
        assert!(matches!(
            p.decide_call(&far, tool, &json!({})),
            Verdict::Deny(_)
        ));
        let refused = CallClass {
            refused: Some("the solution's policy sets browser.evaluate to deny".into()),
            ..CallClass::declared(PermissionClass::Execute)
        };
        assert_eq!(
            AgentPolicy::default().decide_call(&refused, "x", &json!({})),
            Verdict::Deny("the solution's policy sets browser.evaluate to deny".into())
        );
        // dangerous: deny denies escalated calls.
        let strict = AgentPolicy {
            dangerous: Some(DangerousPolicy::Deny),
            ..Default::default()
        };
        assert!(matches!(
            strict.decide_call(&far, tool, &json!({})),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn the_browser_object_parses_follows_its_schema_and_round_trips() {
        let text = r#"{"version": 1, "browser": {"origins": ["localhost", "https://example.com"],
            "network_bodies": "deny", "evaluate": "prompt"}}"#;
        let p: AgentPolicy = serde_json::from_str(text).unwrap();
        let b = p.browser.clone().unwrap();
        assert_eq!(b.network_bodies, Some(NetworkBodiesPolicy::Deny));
        assert_eq!(b.evaluate, Some(EvaluatePolicy::Prompt));
        assert_eq!(b.origins().len(), 2);
        assert!(
            serde_json::from_str::<AgentPolicy>(r#"{"version": 1, "browser": {"bogus": 1}}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<AgentPolicy>(
                r#"{"version": 1, "browser": {"evaluate": "maybe"}}"#
            )
            .is_err()
        );
        // Every key the Rust type writes is in the schema, with the enum values the schema lists.
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let bs = &schema["properties"]["browser"];
        assert_eq!(bs["additionalProperties"], false);
        let v = serde_json::to_value(&p).unwrap();
        for (k, val) in v["browser"].as_object().unwrap() {
            let sub = &bs["properties"][k];
            assert!(!sub.is_null(), "{k}");
            if let Some(e) = sub["enum"].as_array() {
                assert!(e.contains(val), "{k}: {val}");
            }
        }
        for (k, e) in [
            ("network_bodies", ["allow", "deny"].as_slice()),
            ("evaluate", &["allow", "prompt", "deny"]),
        ] {
            assert_eq!(bs["properties"][k]["enum"], json!(e), "{k}");
        }
        // Defaults when the object is absent.
        let d = AgentPolicy::default();
        assert!(d.browser.is_none());
        let view = PolicyView::of(PolicySnapshot::default());
        assert_eq!(view.browser(), BrowserPolicy::default());
        assert_eq!(view.browser().origins(), DEFAULT_ORIGINS);
        // Saved sorted, with the browser object.
        let dir = tempdir("browser");
        let path = AgentPolicy::path_for(&dir);
        let mut p = p;
        p.allow_origin("http://b.test");
        p.save(&path).unwrap();
        assert_eq!(AgentPolicy::load(&path).unwrap(), p);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.find("\"evaluate\"").unwrap() < saved.find("\"network_bodies\"").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_debug_object_parses_defaults_decides_and_remembers() {
        use crate::Escalation;
        let p: AgentPolicy = serde_json::from_str(
            r#"{"version": 1, "debug": {"drive": "prompt", "attach": "deny", "evaluate": "deny"}}"#,
        )
        .unwrap();
        let d = p.debug.clone().unwrap();
        assert_eq!(d.drive, Some(DrivePolicy::Prompt));
        assert_eq!(d.attach, Some(AttachPolicy::Deny));
        assert_eq!(d.evaluate, Some(EvaluatePolicy::Deny));
        for bad in [
            r#"{"version": 1, "debug": {"bogus": 1}}"#,
            r#"{"version": 1, "debug": {"attach": "allow"}}"#,
            r#"{"version": 1, "debug": {"drive": "maybe"}}"#,
        ] {
            assert!(serde_json::from_str::<AgentPolicy>(bad).is_err(), "{bad}");
        }
        // Every key and value the Rust type writes is in the schema.
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let ds = &schema["properties"]["debug"];
        assert_eq!(ds["additionalProperties"], false);
        for (k, e) in [
            ("drive", ["allow", "prompt", "deny"].as_slice()),
            ("attach", &["prompt", "deny"]),
            ("evaluate", &["allow", "prompt", "deny"]),
        ] {
            assert_eq!(ds["properties"][k]["enum"], json!(e), "{k}");
        }
        let v = serde_json::to_value(&p).unwrap();
        for (k, val) in v["debug"].as_object().unwrap() {
            assert!(
                ds["properties"][k]["enum"]
                    .as_array()
                    .unwrap()
                    .contains(val)
            );
        }
        // Defaults: drive allow, attach prompt, evaluate allow.
        let none = DebugPolicy::default();
        let drive = DebugCall {
            drive: true,
            ..DebugCall::default()
        };
        let eval = DebugCall {
            evaluate: true,
            ..DebugCall::default()
        };
        let attach = |foreign| DebugCall {
            drive: true,
            evaluate: false,
            attach: Some(foreign),
        };
        assert_eq!(none.decide(drive), None);
        assert_eq!(none.decide(eval), None);
        assert_eq!(none.decide(attach(false)), None);
        assert!(matches!(
            none.decide(attach(true)),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                always_allow: AlwaysAllow::Never,
                ..
            })
        ));
        assert_eq!(none.decide(DebugCall::default()), None, "reads");
        // The file's: attach denied outright; evaluate denied; drive asks.
        assert!(
            matches!(d.decide(attach(false)), Some(Escalation::Refuse(m)) if m.contains("debug.attach"))
        );
        assert!(
            matches!(d.decide(eval), Some(Escalation::Refuse(m)) if m.contains("debug.evaluate"))
        );
        assert!(matches!(
            d.decide(drive),
            Some(Escalation::Raise { always_allow: AlwaysAllow::Debug(k), .. }) if k == vec![DebugKnob::Drive]
        ));
        // A foreign attach under drive: prompt names both and is allowed once.
        let prompt = DebugPolicy {
            drive: Some(DrivePolicy::Prompt),
            ..DebugPolicy::default()
        };
        match prompt.decide(attach(true)) {
            Some(Escalation::Raise {
                reason,
                always_allow,
                ..
            }) => {
                assert!(reason.contains("did not start") && reason.contains("debug.drive: prompt"));
                assert_eq!(always_allow, AlwaysAllow::Never);
            }
            other => panic!("{other:?}"),
        }
        // The view's defaults when the object is absent.
        assert_eq!(
            PolicyView::of(PolicySnapshot::default()).debug(),
            DebugPolicy::default()
        );
        assert!(
            !PolicyView::of(PolicySnapshot::default())
                .launched()
                .contains(Some(1), None)
        );
        // Always Allow writes `allow` for the keys that asked, and the file keeps sorted keys.
        let mut q = AgentPolicy::default();
        let call = CallClass {
            class: PermissionClass::Dangerous,
            reason: Some("x".into()),
            always_allow: AlwaysAllow::Debug(vec![DebugKnob::Drive, DebugKnob::Evaluate]),
            refused: None,
        };
        assert_eq!(
            q.remember(
                &call,
                "mcp__eludite__eludite-debug-set_variable",
                &json!({})
            ),
            Some("debug.drive: allow, debug.evaluate: allow".into())
        );
        let qd = q.debug.clone().unwrap();
        assert_eq!(qd.drive, Some(DrivePolicy::Allow));
        assert_eq!(qd.evaluate, Some(EvaluatePolicy::Allow));
        assert!(q.rules.is_empty());
        // An escalated debug call is not allowed by allow rules written for ordinary calls... unless the hook found a
        // rule for the tool, which then decides (`decide_for`).
        let tool = "mcp__eludite__eludite-debug-continue";
        let mut r = AgentPolicy::default();
        r.allow_always(tool, &json!({}));
        assert_eq!(r.decide_call(&call, tool, &json!({})), Verdict::Ask);
        let ruled = prompt
            .decide_for(drive, &r.rules, tool, &json!({}))
            .unwrap();
        let Escalation::Raise {
            class,
            reason,
            always_allow,
        } = ruled
        else {
            panic!("a raise")
        };
        let c = CallClass {
            class,
            reason: Some(reason),
            always_allow,
            refused: None,
        };
        assert!(matches!(
            r.decide_call(&c, tool, &json!({})),
            Verdict::Allow(_)
        ));
        let dir = tempdir("debug");
        let path = AgentPolicy::path_for(&dir);
        q.save(&path).unwrap();
        assert_eq!(AgentPolicy::load(&path).unwrap(), q);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.find("\"drive\"").unwrap() < saved.find("\"evaluate\"").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eludite-policy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn launch_urls_come_from_launch_settings_under_the_workspace() {
        let dir = tempdir("launch");
        let props = dir.join("src/Web/Properties");
        std::fs::create_dir_all(&props).unwrap();
        std::fs::write(
            props.join("launchSettings.json"),
            "\u{feff}{\"profiles\": {\"http\": {\"applicationUrl\": \"http://localhost:5080\"}, \"https\": {\"applicationUrl\": \"https://web.test:7001;http://web.test:5000\"}, \"IIS\": {\"commandName\": \"IISExpress\"}}}",
        )
        .unwrap();
        let hidden = dir.join("bin/Properties");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(
            hidden.join("launchSettings.json"),
            r#"{"profiles": {"x": {"applicationUrl": "http://skipped.test"}}}"#,
        )
        .unwrap();
        let mut urls = launch_urls(&dir);
        urls.sort();
        assert_eq!(
            urls,
            [
                "http://localhost:5080",
                "http://web.test:5000",
                "https://web.test:7001"
            ]
        );
        let view = PolicyView::of(PolicySnapshot {
            policy: AgentPolicy::default(),
            workspace: Some(dir.clone()),
            launch_urls: urls,
            ..Default::default()
        });
        assert!(view.check_url("https://web.test:7001/orders").is_ok());
        assert!(view.check_url("https://web.test:7002/").is_err());
        assert!(view.in_workspace(&dir.join("a/b.txt").to_string_lossy()));
        assert!(!view.in_workspace(&dir.join("../x.txt").to_string_lossy()));
        assert!(!view.in_workspace("relative.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
