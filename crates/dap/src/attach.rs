//! Attach plans (brief 0027): which debug adapter attaches to a running process, and its `attach` arguments.
//!
//! | Adapter | `adapterID` | `attach` arguments | Notes |
//! |---|---|---|---|
//! | netcoredbg | `coreclr` | `processId` | A `dotnet <app>.dll` process |
//! | eludite-dbg-mono | `mono` | `address`, `port` (protocol/schemas/dap-mono.md) | The program's debugger agent, started with `--debugger-agent=...,server=y`; Mono has no late attach |
//! | eludite-dbg-netfx | `netfx` | | Windows only, not built yet (brief 0004): refused |
//! | lldb-dap | `lldb` | `pid` | A native process (protocol/schemas/dap-lldb.md) |
//!
//! The default adapter follows the process's runtime ([`crate::processes::Runtime`]).

use serde_json::{Value, json};

use crate::launch::{AdapterKind, NETFX_ON_WINDOWS, Platform};
use crate::processes::Runtime;

/// The adapter `eludite.debug.attach` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachAdapter {
    Coreclr,
    Mono,
    Netfx,
    Lldb,
}

impl AttachAdapter {
    /// `debug-attach.input.json`'s `adapter`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "coreclr" => AttachAdapter::Coreclr,
            "mono" => AttachAdapter::Mono,
            "netfx" => AttachAdapter::Netfx,
            "lldb" => AttachAdapter::Lldb,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AttachAdapter::Coreclr => "coreclr",
            AttachAdapter::Mono => "mono",
            AttachAdapter::Netfx => "netfx",
            AttachAdapter::Lldb => "lldb",
        }
    }

    /// The adapter for a process of `runtime`.
    pub fn for_runtime(runtime: Runtime) -> Result<Self, String> {
        Ok(match runtime {
            Runtime::Dotnet => AttachAdapter::Coreclr,
            Runtime::Mono => AttachAdapter::Mono,
            Runtime::Netfx => AttachAdapter::Netfx,
            Runtime::Native => AttachAdapter::Lldb,
            Runtime::Unknown => {
                return Err(
                    "the process's runtime is unknown (its command line cannot be read): pass `adapter` (coreclr, \
                     mono or lldb)"
                        .into(),
                );
            }
        })
    }

    /// The adapter process it needs (none for `netfx`, which is refused).
    pub fn kind(self) -> Option<AdapterKind> {
        match self {
            AttachAdapter::Coreclr => Some(AdapterKind::Netcoredbg),
            AttachAdapter::Mono => Some(AdapterKind::Mono),
            AttachAdapter::Lldb => Some(AdapterKind::Lldb),
            AttachAdapter::Netfx => None,
        }
    }

    /// `eludite.debug.state`'s `session.runtime` for a session through this adapter.
    pub fn runtime_name(self) -> &'static str {
        match self {
            AttachAdapter::Coreclr => "coreclr",
            AttachAdapter::Mono => "mono",
            AttachAdapter::Netfx => "netfx",
            AttachAdapter::Lldb => "native",
        }
    }
}

/// What to send: the `initialize` `adapterID` and the `attach` arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct AttachPlan {
    pub adapter: AttachAdapter,
    pub adapter_id: &'static str,
    pub arguments: Value,
}

/// Why a Mono program without a debugger agent cannot be attached.
pub fn no_mono_agent(pid: u32) -> String {
    format!(
        "Mono has no late attach: process {pid} was not started with a debugger agent that listens. Start the program \
         with `mono --debug --debugger-agent=transport=dt_socket,server=y,address=127.0.0.1:PORT,suspend=n \
         <program.exe>` and attach again (or pass `mono` with the agent's `address` and `port`)"
    )
}

/// The plan for attaching `adapter` to process `pid` on `platform`; `mono` is where a Mono program's debugger agent
/// listens (from its command line or the caller).
pub fn attach_plan(
    adapter: AttachAdapter,
    pid: u32,
    mono: Option<(String, u16)>,
    platform: Platform,
) -> Result<AttachPlan, String> {
    let (adapter_id, arguments) = match adapter {
        AttachAdapter::Coreclr => (
            "coreclr",
            json!({
                "name": format!("Attach to process {pid} (Eludite)"),
                "type": "coreclr",
                "request": "attach",
                "processId": pid,
                "justMyCode": true,
            }),
        ),
        AttachAdapter::Mono => {
            let (address, port) = mono.ok_or_else(|| no_mono_agent(pid))?;
            ("mono", json!({ "address": address, "port": port }))
        }
        AttachAdapter::Netfx => {
            return Err(if platform == Platform::Windows {
                NETFX_ON_WINDOWS.to_owned()
            } else {
                "eludite-dbg-netfx debugs .NET Framework on Windows only (brief 0004); on Linux and macOS a .NET \
                 Framework program runs under Mono: attach with adapter `mono` to its debugger agent"
                    .to_owned()
            });
        }
        AttachAdapter::Lldb => ("lldb", json!({ "pid": pid })),
    };
    Ok(AttachPlan {
        adapter,
        adapter_id,
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_per_adapter() {
        let p = attach_plan(AttachAdapter::Coreclr, 42, None, Platform::Linux).unwrap();
        assert_eq!(p.adapter_id, "coreclr");
        assert_eq!(p.arguments["processId"], 42);
        assert_eq!(p.arguments["request"], "attach");
        let p = attach_plan(
            AttachAdapter::Mono,
            7,
            Some(("127.0.0.1".into(), 55555)),
            Platform::Linux,
        )
        .unwrap();
        assert_eq!(p.adapter_id, "mono");
        assert_eq!(p.arguments, json!({"address": "127.0.0.1", "port": 55555}));
        let e = attach_plan(AttachAdapter::Mono, 7, None, Platform::Linux).unwrap_err();
        assert!(
            e.contains("no late attach") && e.contains("server=y"),
            "{e}"
        );
        let p = attach_plan(AttachAdapter::Lldb, 9, None, Platform::MacOs).unwrap();
        assert_eq!((p.adapter_id, &p.arguments), ("lldb", &json!({"pid": 9})));
        assert_eq!(
            attach_plan(AttachAdapter::Netfx, 1, None, Platform::Windows).unwrap_err(),
            NETFX_ON_WINDOWS
        );
        assert!(
            attach_plan(AttachAdapter::Netfx, 1, None, Platform::Linux)
                .unwrap_err()
                .contains("Windows only")
        );
    }

    #[test]
    fn adapters_follow_the_runtime() {
        assert_eq!(
            AttachAdapter::for_runtime(Runtime::Dotnet),
            Ok(AttachAdapter::Coreclr)
        );
        assert_eq!(
            AttachAdapter::for_runtime(Runtime::Mono),
            Ok(AttachAdapter::Mono)
        );
        assert_eq!(
            AttachAdapter::for_runtime(Runtime::Native),
            Ok(AttachAdapter::Lldb)
        );
        assert!(AttachAdapter::for_runtime(Runtime::Unknown).is_err());
        assert_eq!(AttachAdapter::parse("lldb"), Some(AttachAdapter::Lldb));
        assert_eq!(AttachAdapter::parse("gdb"), None);
        assert_eq!(AttachAdapter::Lldb.runtime_name(), "native");
        assert_eq!(AttachAdapter::Netfx.kind(), None);
        for a in [
            AttachAdapter::Coreclr,
            AttachAdapter::Mono,
            AttachAdapter::Netfx,
            AttachAdapter::Lldb,
        ] {
            assert_eq!(AttachAdapter::parse(a.as_str()), Some(a));
        }
    }
}
