//! The Agents window's transcript model: a flat list of rows, so the virtualized GPUI `list` re-measures only the rows
//! that changed (brief 0005's finding: agent text is one row per line, and while it streams only the last line
//! changes). Tool calls fold every `tool_call_update`, their permission request, the Eludite MCP call that served them
//! (with its audit entry) and the pending changes they produced into one row.

use std::collections::HashMap;

use eludite_acp::protocol::{
    PermissionOption, PlanEntry, RequestPermissionRequest, SessionUpdate, ToolCall, ToolCallStatus,
};
use eludite_commands::PermissionClass;
use eludite_ui::transcript::ToolStatus;
use serde_json::{Value, json};

/// What became of a tool call's permission request.
#[derive(Debug, Clone, PartialEq)]
pub enum Permission {
    /// Waiting for the user (request `key`).
    Asked {
        key: u64,
        class: PermissionClass,
    },
    Allowed {
        /// By policy, without asking.
        auto: bool,
        reason: String,
    },
    Denied {
        auto: bool,
        reason: String,
    },
}

/// The Eludite command an MCP tool call ran.
#[derive(Debug, Clone, PartialEq)]
pub struct McpLink {
    pub command: String,
    pub class: PermissionClass,
    pub ok: bool,
    pub ms: f64,
    /// The audit entry.
    pub audit: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRow {
    pub call: ToolCall,
    pub permission: Option<Permission>,
    pub options: Vec<PermissionOption>,
    pub mcp: Option<McpLink>,
    /// Pending changes it proposed: (id, path, state label).
    pub changes: Vec<(u64, String, String)>,
}

impl ToolRow {
    pub fn name(&self) -> String {
        self.call
            .agent_tool_name()
            .map(str::to_owned)
            .or_else(|| self.call.title.clone())
            .unwrap_or_else(|| self.call.tool_call_id.clone())
    }

    /// The status the transcript shows (brief 0016 Contract).
    pub fn status(&self) -> ToolStatus {
        match &self.permission {
            Some(Permission::Asked { .. }) => return ToolStatus::AwaitingPermission,
            Some(Permission::Denied { .. }) => return ToolStatus::Denied,
            _ => {}
        }
        if self.changes.iter().any(|(_, _, s)| s == "pending") {
            return ToolStatus::AwaitingReview;
        }
        match self.call.status {
            Some(ToolCallStatus::Failed) => ToolStatus::Failed,
            Some(ToolCallStatus::Completed) => ToolStatus::Completed,
            Some(ToolCallStatus::InProgress) => ToolStatus::Running,
            None | Some(ToolCallStatus::Pending) => match &self.permission {
                Some(Permission::Allowed { auto: true, .. }) => ToolStatus::AllowedWithoutPrompt,
                Some(Permission::Allowed { .. }) => ToolStatus::Running,
                _ => ToolStatus::Pending,
            },
        }
    }

    /// The note under the card: how permission was decided, the command and audit entry, the changes.
    pub fn note(&self) -> Option<String> {
        let mut parts = Vec::new();
        match &self.permission {
            Some(Permission::Allowed { auto: true, reason }) => {
                parts.push(format!("Allowed without prompt: {reason}"))
            }
            Some(Permission::Allowed {
                auto: false,
                reason,
            }) => parts.push(format!("Allowed by you{}", suffix(reason))),
            Some(Permission::Denied { auto, reason }) => parts.push(format!(
                "Denied{}{}",
                if *auto { " by policy" } else { " by you" },
                suffix(reason)
            )),
            Some(Permission::Asked { class, .. }) => {
                parts.push(format!("Awaiting your answer (class {})", class.as_str()))
            }
            None => {}
        }
        if let Some(m) = &self.mcp {
            parts.push(format!(
                "{} ({}) audit #{} {:.1} ms",
                m.command,
                m.class.as_str(),
                m.audit,
                m.ms
            ));
        }
        for (id, path, state) in &self.changes {
            let name = std::path::Path::new(path)
                .file_name()
                .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
            parts.push(format!("Change #{id} {name}: {state}"));
        }
        (!parts.is_empty()).then(|| parts.join(" \u{2022} "))
    }
}

fn suffix(reason: &str) -> String {
    if reason.is_empty() {
        String::new()
    } else {
        format!(": {reason}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    User(String),
    /// One line of agent text.
    Agent(String),
    Thought {
        text: String,
        expanded: bool,
    },
    Tool(Box<ToolRow>),
    Plan(Vec<PlanEntry>),
    Notice(String),
    Error(String),
}

/// Rows `start..old_end` were replaced by rows `start..new_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub rows: Vec<Row>,
    tools: HashMap<String, usize>,
    /// The last row is an agent line still receiving text.
    agent_open: bool,
    thought_open: bool,
    dirty_from: Option<usize>,
    synced_len: usize,
}

impl Transcript {
    fn mark(&mut self, ix: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(ix, |d| d.min(ix)));
    }

    fn push(&mut self, row: Row) {
        self.agent_open = false;
        self.thought_open = false;
        self.mark(self.rows.len());
        self.rows.push(row);
    }

    /// What changed since the last call, for `ListState::splice`.
    pub fn take_splice(&mut self) -> Option<Splice> {
        let start = self.dirty_from.take()?;
        let s = Splice {
            start,
            old_end: self.synced_len.max(start),
            new_end: self.rows.len(),
        };
        self.synced_len = self.rows.len();
        Some(s)
    }

    pub fn user(&mut self, text: &str) {
        self.push(Row::User(text.to_owned()));
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.push(Row::Notice(text.into()));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Row::Error(text.into()));
    }

    fn agent_text(&mut self, text: &str) {
        let mut lines = text.split('\n');
        let first = lines.next().unwrap_or_default();
        if self.agent_open
            && let Some(Row::Agent(last)) = self.rows.last_mut()
        {
            last.push_str(first);
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::Agent(first.to_owned()));
        }
        for line in lines {
            self.push(Row::Agent(line.to_owned()));
        }
        self.agent_open = true;
    }

    fn thought_text(&mut self, text: &str) {
        if self.thought_open
            && let Some(Row::Thought { text: last, .. }) = self.rows.last_mut()
        {
            last.push_str(text);
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::Thought {
                text: text.to_owned(),
                expanded: false,
            });
            self.thought_open = true;
        }
    }

    /// Expand or collapse the thinking block at `ix`.
    pub fn toggle_thought(&mut self, ix: usize) {
        if let Some(Row::Thought { expanded, .. }) = self.rows.get_mut(ix) {
            *expanded = !*expanded;
            self.mark(ix);
        }
    }

    fn tool_index(&mut self, call: &ToolCall) -> usize {
        if let Some(&ix) = self.tools.get(&call.tool_call_id) {
            return ix;
        }
        let ix = self.rows.len();
        self.tools.insert(call.tool_call_id.clone(), ix);
        self.push(Row::Tool(Box::new(ToolRow {
            call: call.clone(),
            permission: None,
            options: Vec::new(),
            mcp: None,
            changes: Vec::new(),
        })));
        ix
    }

    fn tool_mut(&mut self, ix: usize) -> Option<&mut ToolRow> {
        self.mark(ix);
        match self.rows.get_mut(ix) {
            Some(Row::Tool(t)) => Some(t),
            _ => None,
        }
    }

    pub fn apply(&mut self, update: &SessionUpdate) {
        match update {
            SessionUpdate::AgentMessageChunk(c) => {
                if let Some(t) = c.as_text() {
                    self.agent_text(t);
                }
            }
            SessionUpdate::AgentThoughtChunk(c) => {
                if let Some(t) = c.as_text() {
                    self.thought_text(t);
                }
            }
            SessionUpdate::UserMessageChunk(_) => {}
            SessionUpdate::ToolCall(t) | SessionUpdate::ToolCallUpdate(t) => {
                let known = self.tools.contains_key(&t.tool_call_id);
                if !known && matches!(update, SessionUpdate::ToolCallUpdate(_)) {
                    return;
                }
                let ix = self.tool_index(t);
                if known && let Some(row) = self.tool_mut(ix) {
                    row.call.apply(t.clone());
                }
            }
            other => {
                if let Some(entries) = other.plan_entries() {
                    // A plan replaces the previous one when it is the last row.
                    if let Some(Row::Plan(last)) = self.rows.last_mut() {
                        *last = entries;
                        self.mark(self.rows.len() - 1);
                    } else {
                        self.push(Row::Plan(entries));
                    }
                }
            }
        }
    }

    /// A permission request: attach it to its tool call's row (made if the call was not announced).
    pub fn permission(&mut self, req: &RequestPermissionRequest, state: Permission) {
        let ix = self.tool_index(&req.tool_call);
        let options = req.options.clone();
        if let Some(row) = self.tool_mut(ix) {
            if row.call.raw_input.is_none() {
                row.call.raw_input = req.tool_call.raw_input.clone();
            }
            row.permission = Some(state);
            row.options = options;
        }
    }

    /// The answer to request `key`.
    pub fn answer(&mut self, key: u64, state: Permission) -> bool {
        let ix = self.rows.iter().position(|r| {
            matches!(r, Row::Tool(t) if matches!(t.permission, Some(Permission::Asked { key: k, .. }) if k == key))
        });
        match ix.and_then(|ix| self.tool_mut(ix)) {
            Some(row) => {
                row.permission = Some(state);
                true
            }
            None => false,
        }
    }

    /// The tool call row an Eludite MCP call served: the one named in the call's `_meta`, else the newest call of
    /// `mcp__eludite__<tool>` not linked yet.
    pub fn link_mcp(
        &mut self,
        tool_call: Option<&str>,
        tool: &str,
        link: McpLink,
    ) -> Option<String> {
        let full = format!("mcp__{}__{tool}", super::endpoint::MCP_SERVER_NAME);
        let ix = match tool_call.and_then(|id| self.tools.get(id).copied()) {
            Some(ix) => Some(ix),
            None => self.rows.iter().rposition(|r| {
                matches!(r, Row::Tool(t) if t.mcp.is_none() && t.call.agent_tool_name() == Some(full.as_str()))
            }),
        }?;
        let row = self.tool_mut(ix)?;
        row.mcp = Some(link);
        Some(row.call.tool_call_id.clone())
    }

    /// Record a pending change (or its new state) on its tool call's row.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn change(&mut self, tool_call: &str, id: u64, path: &str, state: &str) {
        let Some(&ix) = self.tools.get(tool_call) else {
            return;
        };
        if let Some(row) = self.tool_mut(ix) {
            match row.changes.iter_mut().find(|c| c.0 == id) {
                Some(c) => c.2 = state.to_owned(),
                None => row.changes.push((id, path.to_owned(), state.to_owned())),
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tool(&self, tool_call_id: &str) -> Option<&ToolRow> {
        match self.rows.get(*self.tools.get(tool_call_id)?) {
            Some(Row::Tool(t)) => Some(t),
            _ => None,
        }
    }

    pub fn tools(&self) -> impl Iterator<Item = &ToolRow> {
        self.rows.iter().filter_map(|r| match r {
            Row::Tool(t) => Some(&**t),
            _ => None,
        })
    }

    /// The keys of permission requests waiting for an answer, oldest first.
    pub fn asked(&self) -> Vec<u64> {
        self.tools()
            .filter_map(|t| match t.permission {
                Some(Permission::Asked { key, .. }) => Some(key),
                _ => None,
            })
            .collect()
    }

    /// All agent text, lines joined with `\n`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn agent_message(&self) -> String {
        let mut out = String::new();
        let mut prev_agent = false;
        for r in &self.rows {
            if let Row::Agent(t) = r {
                if prev_agent {
                    out.push('\n');
                }
                out.push_str(t);
                prev_agent = true;
            } else {
                if prev_agent {
                    out.push('\n');
                }
                prev_agent = false;
            }
        }
        out
    }

    /// The transcript as JSON (the manual run's record).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_json(&self) -> Value {
        let mut out = Vec::new();
        let mut agent = String::new();
        let flush = |agent: &mut String, out: &mut Vec<Value>| {
            if !agent.is_empty() {
                out.push(json!({"agent": std::mem::take(agent)}));
            }
        };
        for r in &self.rows {
            if let Row::Agent(t) = r {
                if !agent.is_empty() {
                    agent.push('\n');
                }
                agent.push_str(t);
                continue;
            }
            flush(&mut agent, &mut out);
            out.push(match r {
                Row::User(t) => json!({"user": t}),
                Row::Thought { text, .. } => json!({"thought": text}),
                Row::Tool(t) => json!({"tool_call": {
                    "id": t.call.tool_call_id, "tool": t.name(), "kind": t.call.kind,
                    "status": t.status().label(), "arguments": t.call.raw_input,
                    "result": t.call.content_text(), "note": t.note()
                }}),
                Row::Plan(entries) => json!({"plan": entries}),
                Row::Notice(t) => json!({"notice": t}),
                Row::Error(t) => json!({"error": t}),
                Row::Agent(_) => unreachable!(),
            });
        }
        flush(&mut agent, &mut out);
        Value::Array(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_acp::protocol::ContentBlock;

    fn call(id: &str, tool: &str) -> ToolCall {
        ToolCall {
            tool_call_id: id.into(),
            title: Some(tool.into()),
            status: Some(ToolCallStatus::Pending),
            meta: Some(json!({"claudeCode": {"toolName": tool}})),
            ..Default::default()
        }
    }

    #[test]
    fn agent_lines_split_and_only_the_tail_is_dirty() {
        let mut t = Transcript::default();
        t.user("hi");
        t.take_splice();
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "Hello ",
        )));
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "world\nsecond",
        )));
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 1,
                old_end: 1,
                new_end: 3
            })
        );
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            " line",
        )));
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 2,
                old_end: 3,
                new_end: 3
            })
        );
        assert_eq!(t.agent_message(), "Hello world\nsecond line");
    }

    #[test]
    fn thinking_is_one_collapsed_block_and_toggles() {
        let mut t = Transcript::default();
        for piece in ["Let me ", "think."] {
            t.apply(&SessionUpdate::AgentThoughtChunk(ContentBlock::text(piece)));
        }
        assert_eq!(
            t.rows,
            [Row::Thought {
                text: "Let me think.".into(),
                expanded: false
            }]
        );
        t.toggle_thought(0);
        assert!(matches!(t.rows[0], Row::Thought { expanded: true, .. }));
    }

    #[test]
    fn tool_rows_fold_updates_permissions_mcp_calls_and_changes() {
        let mut t = Transcript::default();
        let read = call("a", "mcp__eludite__diagnostics-list");
        t.apply(&SessionUpdate::ToolCall(read.clone()));
        let req = RequestPermissionRequest {
            session_id: "s".into(),
            tool_call: read.clone(),
            options: Vec::new(),
        };
        t.permission(
            &req,
            Permission::Allowed {
                auto: true,
                reason: "diagnostics.list is class read".into(),
            },
        );
        assert_eq!(
            t.tool("a").unwrap().status(),
            ToolStatus::AllowedWithoutPrompt
        );
        let linked = t.link_mcp(
            None,
            "diagnostics-list",
            McpLink {
                command: "diagnostics.list".into(),
                class: PermissionClass::Read,
                ok: true,
                ms: 0.1,
                audit: 3,
            },
        );
        assert_eq!(linked.as_deref(), Some("a"));
        t.apply(&SessionUpdate::ToolCallUpdate(ToolCall {
            tool_call_id: "a".into(),
            status: Some(ToolCallStatus::Completed),
            ..Default::default()
        }));
        let row = t.tool("a").unwrap();
        assert_eq!(row.status(), ToolStatus::Completed);
        let note = row.note().unwrap();
        assert!(
            note.contains("Allowed without prompt: diagnostics.list is class read"),
            "{note}"
        );
        assert!(note.contains("audit #3"), "{note}");

        // An unannounced call asking permission gets a row; the answer finds it by key.
        let shell = call("b", "Bash");
        t.permission(
            &RequestPermissionRequest {
                session_id: "s".into(),
                tool_call: shell,
                options: Vec::new(),
            },
            Permission::Asked {
                key: 9,
                class: PermissionClass::Execute,
            },
        );
        assert_eq!(t.asked(), [9]);
        assert_eq!(
            t.tool("b").unwrap().status(),
            ToolStatus::AwaitingPermission
        );
        assert!(t.answer(
            9,
            Permission::Denied {
                auto: false,
                reason: String::new()
            }
        ));
        assert_eq!(t.tool("b").unwrap().status(), ToolStatus::Denied);
        assert!(t.asked().is_empty());

        // An edit tool awaiting review, then accepted.
        t.apply(&SessionUpdate::ToolCall(call(
            "c",
            "mcp__eludite__eludite-workspace-apply_edit",
        )));
        t.change("c", 1, "/s/A.cs", "pending");
        assert_eq!(t.tool("c").unwrap().status(), ToolStatus::AwaitingReview);
        t.change("c", 1, "/s/A.cs", "accepted");
        assert!(
            t.tool("c")
                .unwrap()
                .note()
                .unwrap()
                .contains("Change #1 A.cs: accepted")
        );
        // Updates for unknown calls are ignored; plans replace the previous plan at the tail.
        t.apply(&SessionUpdate::ToolCallUpdate(call("zzz", "x")));
        assert!(t.tool("zzz").is_none());
        let plan = |s: &str| SessionUpdate::Other {
            kind: "plan".into(),
            raw: json!({"sessionUpdate": "plan", "entries": [{"content": "x", "priority": "high", "status": s}]}),
        };
        t.apply(&plan("pending"));
        t.apply(&plan("completed"));
        assert!(matches!(t.rows.last(), Some(Row::Plan(e)) if e[0].status == "completed"));
        assert_eq!(
            t.rows.iter().filter(|r| matches!(r, Row::Plan(_))).count(),
            1
        );
        let json = t.to_json();
        assert_eq!(json[0]["tool_call"]["status"], "completed");
    }
}
