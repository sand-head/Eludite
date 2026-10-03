//! DAP request parsing and the shapes the adapter answers with.
//!
//! Only the requests brief 0004 names are typed; anything else parses to [`Command::Unsupported`] and is answered with
//! a failed response. Paths are carried exactly as the client sent them (ADR-0007): nothing here touches the file
//! system.

use serde_json::{Value, json};

/// A DAP request as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub seq: i64,
    pub command: String,
    pub arguments: Value,
}

/// The requests the adapter understands, with their arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Initialize,
    Attach {
        process_id: u32,
    },
    SetBreakpoints {
        source_path: String,
        lines: Vec<u32>,
    },
    ConfigurationDone,
    Threads,
    StackTrace {
        thread_id: u32,
        start_frame: usize,
        levels: Option<usize>,
    },
    Scopes {
        frame_id: i64,
    },
    Variables {
        reference: i64,
    },
    Continue {
        thread_id: Option<u32>,
    },
    Disconnect {
        terminate_debuggee: bool,
    },
    /// A request the spike does not implement.
    Unsupported(String),
}

/// Parses one message body into a request. Responses and events from the client are an error.
pub fn parse_request(body: &[u8]) -> Result<Request, String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|e| format!("message is not JSON: {e}"))?;
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    if kind != "request" {
        return Err(format!("expected a request, got type {kind:?}"));
    }
    let seq = value
        .get("seq")
        .and_then(Value::as_i64)
        .ok_or("request has no integer seq")?;
    let command = value
        .get("command")
        .and_then(Value::as_str)
        .ok_or("request has no command")?
        .to_string();
    let arguments = value.get("arguments").cloned().unwrap_or(Value::Null);
    Ok(Request {
        seq,
        command,
        arguments,
    })
}

/// Types a request's arguments.
pub fn parse_command(request: &Request) -> Result<Command, String> {
    let args = &request.arguments;
    Ok(match request.command.as_str() {
        "initialize" => Command::Initialize,
        "attach" => Command::Attach {
            process_id: process_id(args)?,
        },
        "setBreakpoints" => {
            let source_path = args
                .pointer("/source/path")
                .and_then(Value::as_str)
                .ok_or("setBreakpoints needs source.path")?
                .to_string();
            let mut lines = Vec::new();
            if let Some(bps) = args.get("breakpoints").and_then(Value::as_array) {
                for bp in bps {
                    lines.push(line(bp.get("line"))?);
                }
            } else if let Some(ls) = args.get("lines").and_then(Value::as_array) {
                // The deprecated form.
                for l in ls {
                    lines.push(line(Some(l))?);
                }
            }
            Command::SetBreakpoints { source_path, lines }
        }
        "configurationDone" => Command::ConfigurationDone,
        "threads" => Command::Threads,
        "stackTrace" => Command::StackTrace {
            thread_id: u32_arg(args, "threadId")?,
            start_frame: args.get("startFrame").and_then(Value::as_u64).unwrap_or(0) as usize,
            levels: args
                .get("levels")
                .and_then(Value::as_u64)
                .filter(|&n| n > 0)
                .map(|n| n as usize),
        },
        "scopes" => Command::Scopes {
            frame_id: i64_arg(args, "frameId")?,
        },
        "variables" => Command::Variables {
            reference: i64_arg(args, "variablesReference")?,
        },
        "continue" => Command::Continue {
            thread_id: args
                .get("threadId")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
        },
        "disconnect" => Command::Disconnect {
            terminate_debuggee: args
                .get("terminateDebuggee")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        other => Command::Unsupported(other.to_string()),
    })
}

/// `attach` takes the process id as `processId` (number, or a string of digits as some clients send it).
fn process_id(args: &Value) -> Result<u32, String> {
    let v = args.get("processId").ok_or("attach needs processId")?;
    let n = match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    n.and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n != 0)
        .ok_or_else(|| format!("processId {v} is not a process id"))
}

fn line(v: Option<&Value>) -> Result<u32, String> {
    v.and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n > 0)
        .ok_or_else(|| "breakpoint needs a positive line".to_string())
}

fn u32_arg(args: &Value, name: &str) -> Result<u32, String> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("missing or bad {name}"))
}

fn i64_arg(args: &Value, name: &str) -> Result<i64, String> {
    args.get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("missing or bad {name}"))
}

/// The capabilities `initialize` answers with: only what the spike implements.
pub fn capabilities() -> Value {
    json!({
        "supportsConfigurationDoneRequest": true,
        "supportsTerminateRequest": false,
        "supportsEvaluateForHovers": false,
        "supportsStepBack": false,
        "supportsSetVariable": false,
        "supportsConditionalBreakpoints": false,
        "supportTerminateDebuggee": true,
        "exceptionBreakpointFilters": [],
    })
}

/// One breakpoint as `setBreakpoints` and the `breakpoint` event report it.
#[derive(Debug, Clone, PartialEq)]
pub struct BreakpointResult {
    pub id: i64,
    pub verified: bool,
    /// The requested line, or the line the breakpoint bound to.
    pub line: u32,
    pub source_path: String,
    pub message: Option<String>,
}

impl BreakpointResult {
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "verified": self.verified,
            "line": self.line,
            "source": { "path": self.source_path },
        });
        if let Some(m) = &self.message {
            v["message"] = json!(m);
        }
        v
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThreadInfo {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StackFrameInfo {
    pub id: i64,
    pub name: String,
    /// The document path the PDB records, as recorded (no mapping).
    pub source_path: Option<String>,
    pub line: u32,
    pub column: u32,
}

impl StackFrameInfo {
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "name": self.name,
            "line": self.line,
            "column": self.column,
        });
        if let Some(p) = &self.source_path {
            let name = p.rsplit(['\\', '/']).next().unwrap_or(p);
            v["source"] = json!({ "name": name, "path": p });
        } else {
            v["presentationHint"] = json!("subtle");
        }
        v
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScopeInfo {
    pub name: String,
    pub reference: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariableInfo {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    pub reference: i64,
}

impl VariableInfo {
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "name": self.name,
            "value": self.value,
            "variablesReference": self.reference,
        });
        if let Some(t) = &self.type_name {
            v["type"] = json!(t);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(body: &str) -> Command {
        parse_command(&parse_request(body.as_bytes()).unwrap()).unwrap()
    }

    #[test]
    fn parses_the_brief_requests() {
        assert_eq!(
            req(
                r#"{"seq":1,"type":"request","command":"initialize","arguments":{"adapterID":"netfx"}}"#
            ),
            Command::Initialize
        );
        assert_eq!(
            req(r#"{"seq":2,"type":"request","command":"attach","arguments":{"processId":4242}}"#),
            Command::Attach { process_id: 4242 }
        );
        assert_eq!(
            req(r#"{"seq":2,"type":"request","command":"attach","arguments":{"processId":"17"}}"#),
            Command::Attach { process_id: 17 }
        );
        assert_eq!(
            req(
                r#"{"seq":3,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":"C:\\src\\Program.cs"},"breakpoints":[{"line":12},{"line":20}]}}"#
            ),
            Command::SetBreakpoints {
                source_path: r"C:\src\Program.cs".into(),
                lines: vec![12, 20]
            }
        );
        assert_eq!(
            req(r#"{"seq":4,"type":"request","command":"configurationDone"}"#),
            Command::ConfigurationDone
        );
        assert_eq!(
            req(r#"{"seq":5,"type":"request","command":"threads"}"#),
            Command::Threads
        );
        assert_eq!(
            req(
                r#"{"seq":6,"type":"request","command":"stackTrace","arguments":{"threadId":99,"startFrame":1,"levels":20}}"#
            ),
            Command::StackTrace {
                thread_id: 99,
                start_frame: 1,
                levels: Some(20)
            }
        );
        assert_eq!(
            req(r#"{"seq":7,"type":"request","command":"scopes","arguments":{"frameId":3}}"#),
            Command::Scopes { frame_id: 3 }
        );
        assert_eq!(
            req(
                r#"{"seq":8,"type":"request","command":"variables","arguments":{"variablesReference":1000}}"#
            ),
            Command::Variables { reference: 1000 }
        );
        assert_eq!(
            req(r#"{"seq":9,"type":"request","command":"continue","arguments":{"threadId":99}}"#),
            Command::Continue {
                thread_id: Some(99)
            }
        );
        assert_eq!(
            req(r#"{"seq":10,"type":"request","command":"disconnect","arguments":{}}"#),
            Command::Disconnect {
                terminate_debuggee: false
            }
        );
        assert_eq!(
            req(r#"{"seq":11,"type":"request","command":"stepIn","arguments":{"threadId":1}}"#),
            Command::Unsupported("stepIn".into())
        );
    }

    #[test]
    fn keeps_a_remote_unix_path_as_given() {
        let c = req(
            r#"{"seq":3,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":"/home/me/src/Program.cs"},"lines":[7]}}"#,
        );
        assert_eq!(
            c,
            Command::SetBreakpoints {
                source_path: "/home/me/src/Program.cs".into(),
                lines: vec![7]
            }
        );
    }

    #[test]
    fn rejects_bad_requests() {
        assert!(parse_request(b"not json").is_err());
        assert!(parse_request(br#"{"seq":1,"type":"event","event":"x"}"#).is_err());
        assert!(parse_request(br#"{"type":"request","command":"threads"}"#).is_err());
        let bad = |s: &str| parse_command(&parse_request(s.as_bytes()).unwrap()).unwrap_err();
        bad(r#"{"seq":1,"type":"request","command":"attach","arguments":{}}"#);
        bad(r#"{"seq":1,"type":"request","command":"attach","arguments":{"processId":0}}"#);
        bad(r#"{"seq":1,"type":"request","command":"attach","arguments":{"processId":"abc"}}"#);
        bad(
            r#"{"seq":1,"type":"request","command":"setBreakpoints","arguments":{"breakpoints":[]}}"#,
        );
        bad(
            r#"{"seq":1,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":"a.cs"},"breakpoints":[{"line":0}]}}"#,
        );
        bad(r#"{"seq":1,"type":"request","command":"stackTrace","arguments":{}}"#);
    }

    #[test]
    fn frame_without_source_is_subtle() {
        let f = StackFrameInfo {
            id: 1,
            name: "[External Code]".into(),
            source_path: None,
            line: 0,
            column: 0,
        };
        assert_eq!(f.to_json()["presentationHint"], "subtle");
        let f = StackFrameInfo {
            source_path: Some(r"C:\a\Program.cs".into()),
            ..f
        };
        assert_eq!(f.to_json()["source"]["name"], "Program.cs");
    }
}
