//! Attach plans (brief 0027): which debug adapter attaches to a running process, and its `attach` arguments.
//!
//! | Adapter | `adapterID` | `attach` arguments | Notes |
//! |---|---|---|---|
//! | netcoredbg | `coreclr` | `processId` | A `dotnet <app>.dll` process |
//! | eludite-dbg-mono | `mono` | `address`, `port` (protocol/schemas/dap-mono.md) | The program's debugger agent, started with `--debugger-agent=...,server=y`; Mono has no late attach |
//! | eludite-dbg-netfx | `netfx` | | Windows only, not built yet (brief 0004): refused |
//! | lldb-dap | `lldb` | `pid` | A native process (protocol/schemas/dap-lldb.md) |
//! | vscode-js-debug | `pwa-chrome` | `address`, `port`, `targetId`, `urlFilter`, `webRoot`, ... ([`browser_attach`]) | A web page by its tab or url, never a pid (brief 0038, protocol/schemas/dap-js-debug.md) |
//!
//! The default adapter follows the process's runtime ([`crate::processes::Runtime`]); a tab or a url is vscode-js-debug's.

use std::path::Path;

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
    /// vscode-js-debug, for a web page (brief 0038).
    Javascript,
}

/// vscode-js-debug's `adapterID` (and the configurations' `type`) for a page in Chrome or Eludite's embedded Chromium.
pub const JS_ADAPTER_ID: &str = "pwa-chrome";

/// Why a browser is not attached to by process id.
pub const JS_BY_TAB: &str = "a browser is attached by tab or url, not by process: pass `tab` (eludite.browser.tabs' id) \
     or `url` (a page, or a Chrome DevTools websocket url) with adapter `javascript`";

impl AttachAdapter {
    /// `debug-attach.input.json`'s `adapter`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "coreclr" => AttachAdapter::Coreclr,
            "mono" => AttachAdapter::Mono,
            "netfx" => AttachAdapter::Netfx,
            "lldb" => AttachAdapter::Lldb,
            "javascript" => AttachAdapter::Javascript,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AttachAdapter::Coreclr => "coreclr",
            AttachAdapter::Mono => "mono",
            AttachAdapter::Netfx => "netfx",
            AttachAdapter::Lldb => "lldb",
            AttachAdapter::Javascript => "javascript",
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

    /// The adapter process it needs (none for `netfx`, which is refused, and for `javascript`, which the shell starts
    /// as a TCP server on Node.js: [`crate::transport::start_tcp_server`]).
    pub fn kind(self) -> Option<AdapterKind> {
        match self {
            AttachAdapter::Coreclr => Some(AdapterKind::Netcoredbg),
            AttachAdapter::Mono => Some(AdapterKind::Mono),
            AttachAdapter::Lldb => Some(AdapterKind::Lldb),
            AttachAdapter::Netfx | AttachAdapter::Javascript => None,
        }
    }

    /// `eludite.debug.state`'s `session.runtime` for a session through this adapter.
    pub fn runtime_name(self) -> &'static str {
        match self {
            AttachAdapter::Coreclr => "coreclr",
            AttachAdapter::Mono => "mono",
            AttachAdapter::Netfx => "netfx",
            AttachAdapter::Lldb => "native",
            AttachAdapter::Javascript => "javascript",
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
        AttachAdapter::Javascript => return Err(JS_BY_TAB.to_owned()),
    };
    Ok(AttachPlan {
        adapter,
        adapter_id,
        arguments,
    })
}

// ---- a web page (brief 0038) ----

/// The page vscode-js-debug attaches to: the browser's remote debugging endpoint and the page's target.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowserTarget {
    /// Where the browser's DevTools endpoint listens (`127.0.0.1` for the embedded engine).
    pub address: String,
    pub port: u16,
    /// The page's Chrome DevTools target id, when known.
    pub target_id: Option<String>,
    /// The page's url (js-debug's `urlFilter`).
    pub url: String,
    /// The page's title: the session's name.
    pub title: String,
}

/// A Chrome DevTools websocket url of a page, `ws://HOST:PORT/devtools/page/<id>`: (host, port, target id).
pub fn parse_devtools_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))?;
    let (authority, path) = rest.split_once('/')?;
    let (host, port) = authority.rsplit_once(':')?;
    let id = path.strip_prefix("devtools/page/")?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    (!host.is_empty() && !id.is_empty() && !id.contains('/'))
        .then(|| Some((host.to_owned(), port.parse().ok()?, id.to_owned())))
        .flatten()
}

/// vscode-js-debug's `attach` for `target` with the page's urls mapped under `web_root` (protocol/schemas/dap-js-debug.md):
/// the page by its target id (and url, for a js-debug that only filters), source maps on and waited for, never a
/// quick pick.
pub fn browser_attach(target: &BrowserTarget, web_root: &Path) -> AttachPlan {
    let mut arguments = json!({
        "type": JS_ADAPTER_ID,
        "request": "attach",
        "name": target.title,
        "address": target.address,
        "port": target.port,
        "urlFilter": target.url,
        "webRoot": web_root.to_string_lossy(),
        "sourceMaps": true,
        "pauseForSourceMap": true,
        "resolveSourceMapLocations": null,
        "targetSelection": "automatic",
        "skipFiles": [],
    });
    if let Some(id) = &target.target_id {
        arguments["targetId"] = json!(id);
    }
    AttachPlan {
        adapter: AttachAdapter::Javascript,
        adapter_id: JS_ADAPTER_ID,
        arguments,
    }
}

/// The exception filters vscode-js-debug gets for the Exception Settings window's boxes: Thrown is `all` (caught
/// exceptions too), User-Unhandled is `uncaught` (brief 0038).
pub fn js_exception_filters(
    break_when_thrown: bool,
    break_when_user_unhandled: bool,
) -> Vec<String> {
    let mut f = Vec::new();
    if break_when_thrown {
        f.push("all".to_owned());
    }
    if break_when_user_unhandled {
        f.push("uncaught".to_owned());
    }
    f
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
        assert_eq!(
            AttachAdapter::parse("javascript"),
            Some(AttachAdapter::Javascript)
        );
        assert_eq!(AttachAdapter::Javascript.runtime_name(), "javascript");
        assert_eq!(AttachAdapter::Javascript.kind(), None);
        assert_eq!(AttachAdapter::parse("lldb"), Some(AttachAdapter::Lldb));
        assert_eq!(AttachAdapter::parse("gdb"), None);
        assert_eq!(AttachAdapter::Lldb.runtime_name(), "native");
        assert_eq!(AttachAdapter::Netfx.kind(), None);
        for a in [
            AttachAdapter::Coreclr,
            AttachAdapter::Mono,
            AttachAdapter::Netfx,
            AttachAdapter::Lldb,
            AttachAdapter::Javascript,
        ] {
            assert_eq!(AttachAdapter::parse(a.as_str()), Some(a));
        }
    }

    /// Brief 0038: the `pwa-chrome` attach from a tab (its target id and url) and from a DevTools websocket url; a
    /// browser is never attached by pid.
    #[test]
    fn browser_attach_arguments_from_a_tab_and_a_url() {
        let tab = BrowserTarget {
            address: "127.0.0.1".into(),
            port: 41234,
            target_id: Some("C1A1645D5649ECB8E7FE646FE0AED20C".into()),
            url: "http://localhost:5180/".into(),
            title: "Minimal API".into(),
        };
        let p = browser_attach(&tab, Path::new("/w/MinimalApi/wwwroot"));
        assert_eq!(
            (p.adapter, p.adapter_id),
            (AttachAdapter::Javascript, "pwa-chrome")
        );
        assert_eq!(
            p.arguments,
            json!({
                "type": "pwa-chrome", "request": "attach", "name": "Minimal API",
                "address": "127.0.0.1", "port": 41234, "targetId": "C1A1645D5649ECB8E7FE646FE0AED20C",
                "urlFilter": "http://localhost:5180/", "webRoot": "/w/MinimalApi/wwwroot",
                "sourceMaps": true, "pauseForSourceMap": true, "resolveSourceMapLocations": null,
                "targetSelection": "automatic", "skipFiles": []
            })
        );
        // From a DevTools websocket url: its host, port and page id.
        let (host, port, id) = parse_devtools_url(
            "ws://127.0.0.1:9222/devtools/page/D8F68AEE573F7CFAE3F4D6313C4A1582",
        )
        .unwrap();
        assert_eq!(
            (host.as_str(), port, id.as_str()),
            ("127.0.0.1", 9222, "D8F68AEE573F7CFAE3F4D6313C4A1582")
        );
        let url = BrowserTarget {
            address: host,
            port,
            target_id: Some(id),
            url: "http://localhost:5173/".into(),
            title: "Vite App".into(),
        };
        let p = browser_attach(&url, Path::new("/w/vite-counter"));
        assert_eq!(p.arguments["port"], 9222);
        assert_eq!(p.arguments["targetId"], "D8F68AEE573F7CFAE3F4D6313C4A1582");
        // Without a target id: the url filter only.
        let p = browser_attach(
            &BrowserTarget {
                target_id: None,
                ..tab
            },
            Path::new("/w"),
        );
        assert!(p.arguments.get("targetId").is_none());
        assert_eq!(
            parse_devtools_url("ws://[::1]:9222/devtools/page/AB"),
            Some(("::1".into(), 9222, "AB".into()))
        );
        assert_eq!(
            parse_devtools_url("ws://127.0.0.1:9222/devtools/browser/AB"),
            None
        );
        assert_eq!(
            parse_devtools_url("http://127.0.0.1:9222/devtools/page/AB"),
            None
        );
        assert_eq!(parse_devtools_url("ws://127.0.0.1/devtools/page/AB"), None);
        let e = attach_plan(AttachAdapter::Javascript, 42, None, Platform::Linux).unwrap_err();
        assert!(e.contains("by tab or url"), "{e}");
    }

    /// Brief 0038: the Exception Settings window's boxes as js-debug's filters.
    #[test]
    fn exception_settings_map_to_js_debugs_filters() {
        assert_eq!(js_exception_filters(false, false), Vec::<String>::new());
        assert_eq!(js_exception_filters(true, false), vec!["all"]);
        assert_eq!(js_exception_filters(false, true), vec!["uncaught"]);
        assert_eq!(js_exception_filters(true, true), vec!["all", "uncaught"]);
    }
}
