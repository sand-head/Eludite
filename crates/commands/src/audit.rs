//! The audit log: one entry per command invocation and per agent tool call, with who called, the arguments of agent
//! calls, the outcome, and the edits it produced joined in afterwards ([`AuditLog::attach_edit`]): an agent edit held
//! as a pending change is recorded when it is proposed and updated when it is accepted, rejected or refused.

use std::sync::Mutex;
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

use crate::{Caller, CommandId, PermissionClass};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Err(String),
}

/// What happened to an edit an entry produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditState {
    /// Held for review.
    Pending,
    /// Applied through the workspace-edit applier.
    Accepted,
    /// Discarded by the user (or the turn was cancelled).
    Rejected,
    /// The applier refused it (the document changed since).
    Failed,
}

/// One file's edit produced by an entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditRecord {
    /// The pending change's id in the Agents window.
    pub change: u64,
    pub path: String,
    /// Text edits.
    pub edits: u64,
    pub state: EditState,
    /// The applier's summary or the refusal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// One recorded invocation. Unknown commands are recorded with `permission: None`.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    /// Position in the log, from 1.
    pub seq: u64,
    /// The command id, or for an agent tool that is not a command, the tool's name.
    pub command: String,
    #[serde(skip)]
    pub timestamp: SystemTime,
    pub permission: Option<PermissionClass>,
    pub outcome: Outcome,
    pub caller: Caller,
    /// The arguments, for agent calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    /// The edits this call produced (agent edits held as pending changes, and what became of them).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<EditRecord>,
}

impl AuditEntry {
    pub fn is_ok(&self) -> bool {
        self.outcome == Outcome::Ok
    }
}

/// Append-only, in-memory record of every invocation. Persisting it is future work.
#[derive(Debug, Default)]
pub struct AuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

impl AuditLog {
    /// Record an invocation by the current caller ([`crate::current_caller`]); agent calls keep their arguments.
    /// Returns the entry's `seq`.
    pub fn record(
        &self,
        command: &str,
        permission: Option<PermissionClass>,
        outcome: Outcome,
    ) -> u64 {
        self.record_call(command, permission, outcome, crate::current_caller(), None)
    }

    /// Record an invocation by `caller` with its `arguments`. Returns the entry's `seq`.
    pub fn record_call(
        &self,
        command: &str,
        permission: Option<PermissionClass>,
        outcome: Outcome,
        caller: Caller,
        arguments: Option<Value>,
    ) -> u64 {
        let mut entries = self.lock();
        let seq = entries.len() as u64 + 1;
        entries.push(AuditEntry {
            seq,
            command: command.to_owned(),
            timestamp: SystemTime::now(),
            permission,
            outcome,
            caller,
            arguments,
            edits: Vec::new(),
        });
        seq
    }

    /// Join an edit to the newest entry of agent tool call `call`, or update the record of the same change there.
    /// Returns the entry's `seq`, or `None` when no entry has that call.
    pub fn attach_edit(&self, call: u64, record: EditRecord) -> Option<u64> {
        let mut entries = self.lock();
        let entry = entries
            .iter_mut()
            .rev()
            .find(|e| e.caller.call() == Some(call))?;
        match entry.edits.iter_mut().find(|r| r.change == record.change) {
            Some(r) => *r = record,
            None => entry.edits.push(record),
        }
        Some(entry.seq)
    }

    /// A copy of all entries, oldest first.
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.lock().clone()
    }

    /// The entry at `seq`.
    pub fn get(&self, seq: u64) -> Option<AuditEntry> {
        let i = usize::try_from(seq.checked_sub(1)?).ok()?;
        self.lock().get(i).cloned()
    }

    /// The entries of agent tool call `call`.
    pub fn for_call(&self, call: u64) -> Vec<AuditEntry> {
        self.lock()
            .iter()
            .filter(|e| e.caller.call() == Some(call))
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Entries for one command.
    pub fn for_command(&self, id: &CommandId) -> Vec<AuditEntry> {
        self.lock()
            .iter()
            .filter(|e| e.command == id.as_str())
            .cloned()
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<AuditEntry>> {
        // A poisoned log is still a valid log; keep recording.
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_calls_keep_arguments_and_join_edits() {
        let log = AuditLog::default();
        let user = log.record("test.read", Some(PermissionClass::Read), Outcome::Ok);
        let agent = Caller::Agent {
            agent: "Fake".into(),
            call: 7,
            tool_call: Some("toolu_1".into()),
        };
        let seq = log.record_call(
            "eludite.workspace.apply_edit",
            Some(PermissionClass::EditBuffer),
            Outcome::Ok,
            agent.clone(),
            Some(json!({"edit": {}})),
        );
        assert_eq!((user, seq), (1, 2));
        assert_eq!(log.get(1).unwrap().caller, Caller::User);
        assert_eq!(log.get(0).map(|e| e.seq), None);

        let mut record = EditRecord {
            change: 3,
            path: "/s/A.cs".into(),
            edits: 2,
            state: EditState::Pending,
            summary: None,
        };
        assert_eq!(log.attach_edit(7, record.clone()), Some(2));
        record.state = EditState::Accepted;
        record.summary = Some("2 edits in 1 file".into());
        assert_eq!(log.attach_edit(7, record.clone()), Some(2));
        assert_eq!(log.attach_edit(8, record.clone()), None);
        let e = log.get(2).unwrap();
        assert_eq!(e.edits, [record]);
        assert_eq!(e.arguments, Some(json!({"edit": {}})));
        assert_eq!(log.for_call(7).len(), 1);
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["caller"]["kind"], "agent");
        assert_eq!(v["edits"][0]["state"], "accepted");
        assert_eq!(v["outcome"]["kind"], "ok");
    }
}
