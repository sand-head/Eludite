//! The start of a debug session, in DAP's order: `initialize`, then `launch` (or `attach`) sent without waiting, then
//! after the adapter's `initialized` event the breakpoints, the function breakpoints (when there are any and the
//! adapter has them), the exception filters and `configurationDone`, then the `launch` answer. It waits on the adapter at every step, so it runs on a worker thread.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::client::{DapClient, DapError};
use crate::types::{
    Breakpoint, Capabilities, FunctionBreakpoint, InitializeArguments, SetBreakpointsArguments,
    SetBreakpointsResponse, Source, SourceBreakpoint,
};

/// `launch` or `attach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartKind {
    Launch,
    Attach,
}

impl StartKind {
    pub fn command(self) -> &'static str {
        match self {
            StartKind::Launch => "launch",
            StartKind::Attach => "attach",
        }
    }
}

/// What to start and with which breakpoints.
#[derive(Debug, Clone, PartialEq)]
pub struct StartPlan {
    /// `initialize`'s `adapterID` (`coreclr` for netcoredbg).
    pub adapter_id: String,
    pub kind: StartKind,
    /// The adapter-specific `launch` or `attach` arguments.
    pub arguments: Value,
    /// Breakpoints per source path.
    pub breakpoints: Vec<(String, Vec<SourceBreakpoint>)>,
    /// Function breakpoints (the Rust panics row's `rust_panic`, brief 0029), sent with `setFunctionBreakpoints` when
    /// there are any and the adapter supports them.
    pub function_breakpoints: Vec<FunctionBreakpoint>,
    /// Exception filters to enable; those the adapter does not offer are left out.
    pub exception_filters: Vec<String>,
}

/// What the handshake learned.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub capabilities: Capabilities,
    /// The adapter's answer for each path of [`StartPlan::breakpoints`], in order.
    pub breakpoints: Vec<(String, Vec<Breakpoint>)>,
    /// The adapter's answer for [`StartPlan::function_breakpoints`] (empty when none were sent).
    pub function_breakpoints: Vec<Breakpoint>,
    pub exception_filters: Vec<String>,
}

/// Run the handshake for `plan` on `client`, each step within `timeout`. Blocks: call it on a worker thread.
pub fn start(client: &DapClient, plan: &StartPlan, timeout: Duration) -> Result<Started, DapError> {
    let deadline = Instant::now() + timeout;
    let left = || deadline.saturating_duration_since(Instant::now());
    let init = serde_json::to_value(InitializeArguments::eludite(&plan.adapter_id))
        .map_err(|e| DapError::Io(e.to_string()))?;
    let caps_body = client.request_wait("initialize", init, left())?;
    let mut capabilities: Capabilities = serde_json::from_value(caps_body).unwrap_or_default();
    // A `capabilities` event that came before the answer (netcoredbg sends one) counts too.
    let early = client.capabilities();
    if capabilities.exception_breakpoint_filters.is_empty() {
        capabilities.exception_breakpoint_filters = early.exception_breakpoint_filters;
    }
    client.set_capabilities(capabilities.clone());
    let launched = client.request_channel(plan.kind.command(), plan.arguments.clone())?;
    if !client.wait_initialized(left()) {
        // The launch may have failed first: report its message rather than a timeout.
        if let Ok(Err(e)) = launched.try_recv() {
            return Err(e);
        }
        return Err(if client.is_closed() {
            DapError::Closed
        } else {
            DapError::Timeout("initialized".into())
        });
    }
    let mut breakpoints = Vec::new();
    for (path, bps) in &plan.breakpoints {
        let answer = set_breakpoints(client, path, bps, left())?;
        breakpoints.push((path.clone(), answer));
    }
    let mut function_breakpoints = Vec::new();
    if !plan.function_breakpoints.is_empty() && capabilities.supports_function_breakpoints {
        // A function breakpoint the adapter refuses does not stop the launch: the answer is simply empty.
        match client.request_wait(
            "setFunctionBreakpoints",
            json!({ "breakpoints": plan.function_breakpoints }),
            left(),
        ) {
            Ok(body) => {
                function_breakpoints = serde_json::from_value::<SetBreakpointsResponse>(body)
                    .unwrap_or_default()
                    .breakpoints;
            }
            Err(DapError::Failed { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    let offered: Vec<&str> = capabilities
        .exception_breakpoint_filters
        .iter()
        .map(|f| f.filter.as_str())
        .collect();
    let filters: Vec<String> = plan
        .exception_filters
        .iter()
        .filter(|f| offered.contains(&f.as_str()))
        .cloned()
        .collect();
    if !offered.is_empty() {
        client.request_wait(
            "setExceptionBreakpoints",
            json!({ "filters": filters }),
            left(),
        )?;
    }
    if capabilities.supports_configuration_done_request {
        client.request_wait("configurationDone", Value::Null, left())?;
    }
    launched.recv_timeout(left()).unwrap_or_else(|e| match e {
        std::sync::mpsc::RecvTimeoutError::Timeout => {
            Err(DapError::Timeout(plan.kind.command().into()))
        }
        std::sync::mpsc::RecvTimeoutError::Disconnected => Err(DapError::Closed),
    })?;
    Ok(Started {
        capabilities,
        breakpoints,
        function_breakpoints,
        exception_filters: filters,
    })
}

/// `setBreakpoints` for one file, waiting for the answer.
pub fn set_breakpoints(
    client: &DapClient,
    path: &str,
    breakpoints: &[SourceBreakpoint],
    timeout: Duration,
) -> Result<Vec<Breakpoint>, DapError> {
    let body = client.request_wait(
        "setBreakpoints",
        set_breakpoints_arguments(path, breakpoints),
        timeout,
    )?;
    Ok(serde_json::from_value::<SetBreakpointsResponse>(body)
        .unwrap_or_default()
        .breakpoints)
}

/// The arguments of `setBreakpoints` for `path`.
pub fn set_breakpoints_arguments(path: &str, breakpoints: &[SourceBreakpoint]) -> Value {
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    serde_json::to_value(SetBreakpointsArguments {
        source: Source {
            name,
            path: Some(path.to_owned()),
        },
        breakpoints: breakpoints.to_vec(),
    })
    .expect("breakpoints serialize")
}
