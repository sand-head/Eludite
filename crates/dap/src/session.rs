//! The start of a debug session, in DAP's order: `initialize`, then `launch` (or `attach`) sent without waiting, then
//! after the adapter's `initialized` event the breakpoints, the function breakpoints (the user's and the Rust panics
//! row's `rust_panic`, in one list, when there are any and the adapter has them), the exception filters with their
//! options (exception types, brief 0026, where the adapter supports them) and `configurationDone`, then the `launch`
//! answer. It waits on the adapter at every step, so it runs on a worker thread.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::{DapClient, DapError};
use crate::types::{
    Breakpoint, Capabilities, ExceptionFilterOptions, FunctionBreakpoint, InitializeArguments,
    SetBreakpointsArguments, SetBreakpointsResponse, SetExceptionBreakpointsArguments,
    SetFunctionBreakpointsArguments, Source, SourceBreakpoint,
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

    /// A `startDebugging` reverse request's `request` (brief 0038): `launch` or `attach`.
    pub fn from_request(request: &str) -> Option<Self> {
        match request {
            "launch" => Some(StartKind::Launch),
            "attach" => Some(StartKind::Attach),
            _ => None,
        }
    }
}

/// Which files' breakpoints a session's adapter gets (brief 0038): each adapter claims the source kinds it can bind,
/// and a breakpoint goes to every session whose adapter claims its file's kind; a kind no adapter claims goes to every
/// session (the behavior before brief 0038), so `breakpoints[].sessions` names only the adapters that can bind a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterFamily {
    /// netcoredbg, eludite-dbg-mono, eludite-dbg-netfx (`coreclr`, `mono`, `netfx`).
    Dotnet,
    /// vscode-js-debug's sessions of a page target (a browser session's children).
    Javascript,
    /// lldb-dap (`native`).
    Native,
    /// vscode-js-debug's browser session, the parent of the page targets' sessions: it gets no breakpoints and no
    /// exception filters; its children do (protocol/schemas/dap-js-debug.md).
    Browser,
    /// An adapter of no known family (a test's): it gets every breakpoint.
    Other,
}

/// The source kinds (file extensions, lowercase) .NET adapters claim.
pub const DOTNET_KINDS: &[&str] = &["cs", "vb", "fs", "fsx", "cshtml", "razor"];
/// The source kinds vscode-js-debug claims.
pub const JAVASCRIPT_KINDS: &[&str] = &[
    "js", "mjs", "cjs", "ts", "mts", "cts", "tsx", "jsx", "vue", "svelte",
];
/// The source kinds lldb-dap claims.
pub const NATIVE_KINDS: &[&str] = &[
    "rs", "c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx", "m", "mm", "swift",
];

/// A path's extension, lowercase.
fn kind_of(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

impl AdapterFamily {
    /// The family of a session whose `session.runtime` is `runtime` (`coreclr`, `mono`, `netfx`, `native`,
    /// `javascript`); a `javascript` session without a parent is a [`AdapterFamily::Browser`] one.
    pub fn of_runtime(runtime: Option<&str>, child: bool) -> Self {
        match runtime {
            Some("coreclr" | "mono" | "netfx") => AdapterFamily::Dotnet,
            Some("native") => AdapterFamily::Native,
            Some("javascript") if child => AdapterFamily::Javascript,
            Some("javascript") => AdapterFamily::Browser,
            _ => AdapterFamily::Other,
        }
    }

    /// The kinds it claims (none for `Browser` and `Other`).
    pub fn kinds(self) -> &'static [&'static str] {
        match self {
            AdapterFamily::Dotnet => DOTNET_KINDS,
            AdapterFamily::Javascript => JAVASCRIPT_KINDS,
            AdapterFamily::Native => NATIVE_KINDS,
            AdapterFamily::Browser | AdapterFamily::Other => &[],
        }
    }

    /// Whether it claims `path`'s kind.
    pub fn claims(self, path: &str) -> bool {
        kind_of(path).is_some_and(|k| self.kinds().contains(&k.as_str()))
    }

    /// Whether `path`'s breakpoints go to a session of this family: it claims the kind, or no family does (a
    /// `Browser` session takes none, an `Other` one every one).
    pub fn takes(self, path: &str) -> bool {
        match self {
            AdapterFamily::Browser => false,
            AdapterFamily::Other => true,
            f => f.claims(path) || !claimed(path),
        }
    }

    /// Whether a session of this family gets exception filters (not a `Browser` one).
    pub fn takes_exceptions(self) -> bool {
        self != AdapterFamily::Browser
    }
}

/// Whether some family claims `path`'s kind.
pub fn claimed(path: &str) -> bool {
    [
        AdapterFamily::Dotnet,
        AdapterFamily::Javascript,
        AdapterFamily::Native,
    ]
    .iter()
    .any(|f| f.claims(path))
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
    /// Exception filters to enable; those the adapter does not offer are left out.
    pub exception_filters: Vec<String>,
    /// Filters with a condition (exception types), sent as `filterOptions` when the adapter supports them; a filter
    /// named here is not also named in `exception_filters`.
    pub exception_options: Vec<ExceptionFilterOptions>,
    /// Function breakpoints in one list (`setFunctionBreakpoints` replaces them all): the user's (brief 0026) and the
    /// Rust panics row's `rust_panic` (brief 0029); sent when there are any and the adapter supports them.
    pub function_breakpoints: Vec<FunctionBreakpoint>,
}

/// What the handshake learned.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    pub capabilities: Capabilities,
    /// The adapter's answer for each path of [`StartPlan::breakpoints`], in order.
    pub breakpoints: Vec<(String, Vec<Breakpoint>)>,
    pub exception_filters: Vec<String>,
    /// The adapter's answer for [`StartPlan::function_breakpoints`] (empty when they were not sent).
    pub function_breakpoints: Vec<Breakpoint>,
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
            set_function_breakpoints_arguments(&plan.function_breakpoints),
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
    let options: Vec<ExceptionFilterOptions> = if capabilities.supports_exception_filter_options {
        plan.exception_options
            .iter()
            .filter(|o| offered.contains(&o.filter_id.as_str()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    if !offered.is_empty() {
        client.request_wait(
            "setExceptionBreakpoints",
            set_exception_breakpoints_arguments(&filters, &options),
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
        exception_filters: filters,
        function_breakpoints,
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

/// The arguments of `setFunctionBreakpoints`.
pub fn set_function_breakpoints_arguments(breakpoints: &[FunctionBreakpoint]) -> Value {
    serde_json::to_value(SetFunctionBreakpointsArguments {
        breakpoints: breakpoints.to_vec(),
    })
    .expect("function breakpoints serialize")
}

/// The arguments of `setExceptionBreakpoints`: `filters` without conditions, `filterOptions` with them (left out
/// when empty, for adapters without `supportsExceptionFilterOptions`).
pub fn set_exception_breakpoints_arguments(
    filters: &[String],
    options: &[ExceptionFilterOptions],
) -> Value {
    serde_json::to_value(SetExceptionBreakpointsArguments {
        filters: filters.to_vec(),
        filter_options: options.to_vec(),
    })
    .expect("exception filters serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brief 0038's file-kind table: each family takes its own kinds, a kind nobody claims goes to every family, a
    /// browser session (js-debug's parent) takes nothing.
    #[test]
    fn breakpoints_go_to_the_adapters_that_claim_the_file() {
        use AdapterFamily::*;
        let fam = |r: &str, child: bool| AdapterFamily::of_runtime(Some(r), child);
        assert_eq!(fam("coreclr", false), Dotnet);
        assert_eq!(fam("mono", false), Dotnet);
        assert_eq!(fam("netfx", false), Dotnet);
        assert_eq!(fam("native", false), Native);
        assert_eq!(fam("javascript", false), Browser);
        assert_eq!(fam("javascript", true), Javascript);
        assert_eq!(AdapterFamily::of_runtime(None, false), Other);
        let cases: [(&str, [bool; 5]); 10] = [
            // path: Dotnet, Javascript, Native, Browser, Other
            ("/w/App/Program.cs", [true, false, false, false, true]),
            ("/w/App/Module.FS", [true, false, false, false, true]),
            (
                "/w/App/Pages/Index.cshtml",
                [true, false, false, false, true],
            ),
            ("/w/wwwroot/app.ts", [false, true, false, false, true]),
            ("/w/src/App.vue", [false, true, false, false, true]),
            ("/w/src/main.mjs", [false, true, false, false, true]),
            ("/w/src/main.rs", [false, false, true, false, true]),
            ("/w/native/lib.cpp", [false, false, true, false, true]),
            ("/w/notes.txt", [true, true, true, false, true]),
            ("/w/Makefile", [true, true, true, false, true]),
        ];
        for (path, want) in cases {
            let got = [Dotnet, Javascript, Native, Browser, Other].map(|f| f.takes(path));
            assert_eq!(got, want, "{path}");
        }
        assert!(claimed("C:\\w\\App\\Program.cs"));
        assert!(!claimed("/w/index.html"));
        assert!(!Browser.takes_exceptions() && Javascript.takes_exceptions());
        assert_eq!(StartKind::from_request("attach"), Some(StartKind::Attach));
        assert_eq!(StartKind::from_request("launch"), Some(StartKind::Launch));
        assert_eq!(StartKind::from_request("restart"), None);
    }
}
