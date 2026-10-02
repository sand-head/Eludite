//! The subset of the Debug Adapter Protocol Eludite speaks, typed (https://microsoft.github.io/debug-adapter-protocol/
//! specification). Decoding is tolerant: unknown members are ignored and missing optional ones default, because
//! adapters differ (netcoredbg omits several optional members; see `docs/briefs/0018-report.md`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `initialize` response body: what the adapter supports.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
    pub supports_conditional_breakpoints: bool,
    pub supports_hit_conditional_breakpoints: bool,
    pub supports_exception_info_request: bool,
    pub supports_terminate_request: bool,
    pub supports_evaluate_for_hovers: bool,
    pub supports_cancel_request: bool,
    pub support_terminate_debuggee: bool,
    /// Not a DAP capability (DAP has it as a client capability); decoded for adapters that say so anyway. The shell
    /// treats `eludite-dbg-mono` as paging (brief 0022 report, section 9).
    pub supports_variable_paging: bool,
    pub supports_delayed_stack_trace_loading: bool,
    pub supports_set_variable: bool,
    pub supports_function_breakpoints: bool,
    pub supports_log_points: bool,
    pub supports_exception_filter_options: bool,
    pub supports_goto_targets_request: bool,
    pub supports_data_breakpoints: bool,
    pub supports_step_back: bool,
    pub supports_restart_request: bool,
    pub supports_modules_request: bool,
    pub supports_read_memory_request: bool,
    pub supports_disassemble_request: bool,
    pub exception_breakpoint_filters: Vec<ExceptionBreakpointsFilter>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExceptionBreakpointsFilter {
    pub filter: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

/// `initialize` arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeArguments {
    #[serde(rename = "clientID")]
    pub client_id: String,
    pub client_name: String,
    #[serde(rename = "adapterID")]
    pub adapter_id: String,
    pub lines_start_at1: bool,
    pub columns_start_at1: bool,
    pub path_format: String,
    pub supports_variable_type: bool,
    pub supports_run_in_terminal_request: bool,
}

impl InitializeArguments {
    /// Eludite's arguments for adapter `adapter_id` (`coreclr` for netcoredbg): 1-based lines and columns, native paths.
    pub fn eludite(adapter_id: &str) -> Self {
        Self {
            client_id: "eludite".into(),
            client_name: "Eludite".into(),
            adapter_id: adapter_id.into(),
            lines_start_at1: true,
            columns_start_at1: true,
            path_format: "path".into(),
            supports_variable_type: true,
            supports_run_in_terminal_request: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Source {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One breakpoint asked for in `setBreakpoints`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SourceBreakpoint {
    pub line: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
}

/// One breakpoint asked for in `setFunctionBreakpoints`: a function by name (`rust_panic`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FunctionBreakpoint {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsArguments {
    pub source: Source,
    pub breakpoints: Vec<SourceBreakpoint>,
}

/// A breakpoint as the adapter reports it (in `setBreakpoints` responses and `breakpoint` events).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Breakpoint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetBreakpointsResponse {
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadsResponse {
    pub threads: Vec<Thread>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StackFrame {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    /// 1-based; 0 for a frame without source (netcoredbg's `[Native Frames]`).
    pub line: i64,
    pub column: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<i64>,
    /// `normal`, `label` or `subtle` (`eludite-dbg-mono` marks frames without source `subtle`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<String>,
}

impl StackFrame {
    /// The frame's source path, when it has source and a line.
    pub fn path(&self) -> Option<&str> {
        (self.line > 0)
            .then(|| self.source.as_ref()?.path.as_deref())
            .flatten()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StackTraceResponse {
    pub stack_frames: Vec<StackFrame>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_frames: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Scope {
    pub name: String,
    /// `arguments`, `locals`, `registers` or another kind the adapter names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<String>,
    pub variables_reference: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
    pub expensive: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScopesResponse {
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub variables_reference: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub named_variables: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_variables: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation_hint: Option<VariablePresentationHint>,
}

/// How a client should show a variable: its kind (`property`, `method`, `class`, `data`, ...), attributes
/// (`readOnly`, `rawString`, ...) and visibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VariablePresentationHint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VariablesResponse {
    pub variables: Vec<Variable>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvaluateResponse {
    pub result: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub variables_reference: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExceptionInfoResponse {
    pub exception_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub break_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<ExceptionDetails>,
}

/// `exceptionInfo`'s details: the exception and its inner exceptions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExceptionDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_type_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inner_exception: Vec<ExceptionDetails>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoppedEvent {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    pub all_threads_stopped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub hit_breakpoint_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContinuedEvent {
    pub thread_id: i64,
    pub all_threads_continued: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub output: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExitedEvent {
    pub exit_code: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadEvent {
    pub reason: String,
    pub thread_id: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BreakpointEvent {
    pub reason: String,
    pub breakpoint: Breakpoint,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProcessEvent {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_process_id: Option<i64>,
}

/// An adapter event, decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Initialized,
    Stopped(StoppedEvent),
    Continued(ContinuedEvent),
    Exited(ExitedEvent),
    Terminated,
    Thread(ThreadEvent),
    Output(OutputEvent),
    Breakpoint(BreakpointEvent),
    Process(ProcessEvent),
    Capabilities(Capabilities),
    /// Any other event (`module`, `loadedSource`, ...), or one whose body did not decode.
    Other {
        event: String,
        body: Value,
    },
}

impl Event {
    /// Decode event `event` with `body` (absent bodies are `null`).
    pub fn decode(event: &str, body: Value) -> Event {
        fn typed<T: for<'de> Deserialize<'de>>(body: &Value) -> Option<T> {
            serde_json::from_value(if body.is_null() {
                Value::Object(Default::default())
            } else {
                body.clone()
            })
            .ok()
        }
        let decoded = match event {
            "initialized" => Some(Event::Initialized),
            "terminated" => Some(Event::Terminated),
            "stopped" => typed(&body).map(Event::Stopped),
            "continued" => typed(&body).map(Event::Continued),
            "exited" => typed(&body).map(Event::Exited),
            "thread" => typed(&body).map(Event::Thread),
            "output" => typed(&body).map(Event::Output),
            "breakpoint" => typed(&body).map(Event::Breakpoint),
            "process" => typed(&body).map(Event::Process),
            "capabilities" => body
                .get("capabilities")
                .and_then(|c| serde_json::from_value(c.clone()).ok())
                .map(Event::Capabilities),
            _ => None,
        };
        decoded.unwrap_or_else(|| Event::Other {
            event: event.to_owned(),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_netcoredbg_shapes() {
        // Bodies as netcoredbg 3.2.0-1092 sends them (brief 0018 probe against eludite-host).
        let caps: Capabilities = serde_json::from_value(json!({
            "exceptionBreakpointFilters": [{"filter": "user-unhandled", "label": "user-unhandled"},
                                            {"filter": "all", "label": "all"}],
            "supportTerminateDebuggee": true, "supportsCancelRequest": true,
            "supportsConditionalBreakpoints": true, "supportsConfigurationDoneRequest": true,
            "supportsExceptionFilterOptions": true, "supportsExceptionInfoRequest": true,
            "supportsExceptionOptions": false, "supportsFunctionBreakpoints": true,
            "supportsSetExpression": true, "supportsSetVariable": true, "supportsTerminateRequest": true
        }))
        .unwrap();
        assert!(caps.supports_conditional_breakpoints);
        assert!(!caps.supports_hit_conditional_breakpoints);
        assert_eq!(caps.exception_breakpoint_filters[1].filter, "all");
        // What brief 0025's `capabilities` reads (brief 0018 report, section 5): netcoredbg has function breakpoints,
        // setVariable and exception filter options, and none of the others.
        assert!(caps.supports_function_breakpoints);
        assert!(caps.supports_set_variable);
        assert!(caps.supports_exception_filter_options);
        assert!(caps.supports_exception_info_request);
        assert!(caps.supports_terminate_request);
        for (name, v) in [
            ("variable paging", caps.supports_variable_paging),
            (
                "delayed stack loading",
                caps.supports_delayed_stack_trace_loading,
            ),
            ("log points", caps.supports_log_points),
            ("goto targets", caps.supports_goto_targets_request),
            ("data breakpoints", caps.supports_data_breakpoints),
            ("step back", caps.supports_step_back),
            ("restart", caps.supports_restart_request),
            ("modules", caps.supports_modules_request),
            ("read memory", caps.supports_read_memory_request),
            ("disassemble", caps.supports_disassemble_request),
        ] {
            assert!(!v, "{name}");
        }
        // eludite-dbg-mono's answer (protocol/schemas/dap-mono.md): delayed stack loading, log points, set variable.
        let mono: Capabilities = serde_json::from_value(json!({
            "supportsConfigurationDoneRequest": true, "supportsConditionalBreakpoints": true,
            "supportsHitConditionalBreakpoints": true, "supportsFunctionBreakpoints": true, "supportsLogPoints": true,
            "supportsEvaluateForHovers": true, "supportsSetVariable": true, "supportsExceptionInfoRequest": true,
            "supportsExceptionFilterOptions": true, "supportsTerminateRequest": true,
            "supportTerminateDebuggee": true, "supportsDelayedStackTraceLoading": true
        }))
        .unwrap();
        assert!(mono.supports_delayed_stack_trace_loading && mono.supports_log_points);
        assert!(!mono.supports_variable_paging && !mono.supports_goto_targets_request);
        // lldb-dap 18.1.3's answer (brief 0029, protocol/schemas/dap-lldb.md): no goto targets, no terminate request.
        let lldb: Capabilities = serde_json::from_value(json!({
            "completionTriggerCharacters": [".", " ", "\t"],
            "exceptionBreakpointFilters": [{"default": false, "filter": "cpp_catch", "label": "C++ Catch"},
                                            {"default": false, "filter": "cpp_throw", "label": "C++ Throw"}],
            "supportTerminateDebuggee": true, "supportsCompletionsRequest": true,
            "supportsConditionalBreakpoints": true, "supportsConfigurationDoneRequest": true,
            "supportsDelayedStackTraceLoading": true, "supportsDisassembleRequest": true,
            "supportsEvaluateForHovers": true, "supportsExceptionInfoRequest": true, "supportsExceptionOptions": true,
            "supportsFunctionBreakpoints": true, "supportsGotoTargetsRequest": false,
            "supportsHitConditionalBreakpoints": true, "supportsLoadedSourcesRequest": false, "supportsLogPoints": true,
            "supportsModulesRequest": true, "supportsProgressReporting": true, "supportsRestartFrame": false,
            "supportsRestartRequest": true, "supportsRunInTerminalRequest": true, "supportsSetVariable": true,
            "supportsStepBack": false, "supportsStepInTargetsRequest": false, "supportsValueFormattingOptions": true
        }))
        .unwrap();
        assert!(lldb.supports_hit_conditional_breakpoints && lldb.supports_log_points);
        assert!(lldb.supports_restart_request && lldb.supports_modules_request);
        assert!(!lldb.supports_goto_targets_request && !lldb.supports_terminate_request);
        assert_eq!(lldb.exception_breakpoint_filters[1].filter, "cpp_throw");
        let info: ExceptionInfoResponse = serde_json::from_value(json!({
            "exceptionId": "System.InvalidOperationException", "description": "boom", "breakMode": "always",
            "details": {"message": "boom", "typeName": "InvalidOperationException",
                        "fullTypeName": "System.InvalidOperationException", "stackTrace": "   at Program.Fail()",
                        "innerException": [{"message": "inner", "typeName": "FormatException"}]}
        }))
        .unwrap();
        let details = info.details.unwrap();
        assert_eq!(details.stack_trace.as_deref(), Some("   at Program.Fail()"));
        assert_eq!(details.inner_exception[0].message.as_deref(), Some("inner"));

        let stopped = Event::decode(
            "stopped",
            json!({"allThreadsStopped": true, "reason": "breakpoint", "threadId": 1238390}),
        );
        assert_eq!(
            stopped,
            Event::Stopped(StoppedEvent {
                reason: "breakpoint".into(),
                thread_id: Some(1238390),
                all_threads_stopped: true,
                ..Default::default()
            })
        );
        let frames: StackTraceResponse = serde_json::from_value(json!({"stackFrames": [
            {"column": 9, "endColumn": 107, "endLine": 69, "id": 0, "line": 69, "moduleId": "x",
             "name": "Eludite.Host.Rpc.HostRpcTarget.Ping()",
             "source": {"name": "HostRpcTarget.cs", "path": "/s/HostRpcTarget.cs"}},
            {"column": 0, "endColumn": 0, "endLine": 0, "id": 1, "line": 0, "moduleId": "", "name": "[Native Frames]"}
        ]}))
        .unwrap();
        assert_eq!(frames.stack_frames[0].path(), Some("/s/HostRpcTarget.cs"));
        assert_eq!(frames.stack_frames[1].path(), None);
        let vars: VariablesResponse = serde_json::from_value(json!({"variables": [
            {"evaluateName": "this", "name": "this", "namedVariables": 10,
             "type": "Eludite.Host.Rpc.HostRpcTarget", "value": "{Eludite.Host.Rpc.HostRpcTarget}", "variablesReference": 2},
            {"evaluateName": "timestamp", "name": "timestamp", "type": "string", "value": "null", "variablesReference": 0}
        ]}))
        .unwrap();
        assert_eq!(
            vars.variables[0].type_name.as_deref(),
            Some("Eludite.Host.Rpc.HostRpcTarget")
        );
        assert_eq!(vars.variables[1].variables_reference, 0);
        let bp = Event::decode(
            "breakpoint",
            json!({"breakpoint": {"endLine": 69, "id": 1, "line": 69,
                   "source": {"name": "HostRpcTarget.cs", "path": "/s/HostRpcTarget.cs"}, "verified": true},
                   "reason": "changed"}),
        );
        assert!(
            matches!(bp, Event::Breakpoint(b) if b.breakpoint.verified && b.breakpoint.id == Some(1))
        );
        assert_eq!(
            Event::decode("initialized", Value::Null),
            Event::Initialized
        );
        assert!(matches!(
            Event::decode("module", json!({"reason": "new"})),
            Event::Other { ref event, .. } if event == "module"
        ));
        let out = Event::decode("output", json!({"category": "stdout", "output": "hello\n"}));
        assert!(matches!(out, Event::Output(o) if o.output == "hello\n"));
        let init = serde_json::to_value(InitializeArguments::eludite("coreclr")).unwrap();
        assert_eq!(init["adapterID"], "coreclr");
        assert_eq!(init["clientID"], "eludite");
        assert_eq!(init["linesStartAt1"], true);
    }
}
