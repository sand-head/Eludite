//! The DAP conformance tests (brief 0033): the shell against recorded sessions of the real debug adapters, on every
//! platform, with no adapter installed. Each scenario is a list of the person's and an agent's actions (F9 on a
//! marked line, F5, F10, F11, Shift+F11, Shift+F5, the Exception Settings, `eludite.debug.*` from an agent thread);
//! after each one settles, the test reads `eludite.debug.state` (without timing fields), the agent's answer (the stop
//! summary) and the row texts of the Locals, Call Stack, Threads and Breakpoints windows. Those snapshots are the
//! scenario's golden file, `corpus/dap/<adapter>/<scenario>.golden.json`, beside its recording `<scenario>.dap.json`.
//!
//! - **Replay** (always): the scenario runs against the replaying adapter (`eludite_dap::replay`) serving the
//!   checked-in recording through [`DebugSetup::connect`], and the snapshots must equal the golden file; a difference
//!   fails with the paths that differ. `REGOLDEN=1` rewrites the golden file from the replay instead.
//! - **Record** (with `RECORD_DAP=<dir>` when the adapter is on this machine): the scenario first runs against the
//!   real adapter with the recorder on the connection (`eludite_dap::record`), writing `<dir>/<adapter>/<scenario>`'s
//!   recording and golden file (the snapshots the shell took live), then replays that fresh recording in a second
//!   shell and requires the same snapshots. With `DAP_CORPUS_CHECK=1` the fresh recording must also reproduce the
//!   checked-in one, timing aside (`eludite_dap::record::compare`): the CI's Linux job re-records Mono and lldb-dap
//!   this way so an adapter upgrade is noticed, with `DAP_CORPUS_REQUIRE=mono,lldb` so a missing adapter fails the
//!   check instead of skipping it. `tools/dap-corpus/record.sh` (and `record.ps1`) runs these modes.
//!
//! vscode-js-debug (brief 0038) is a DAP server with one connection per debugging session: the browser session and
//! each child session `startDebugging` asks for. Its scenarios record each connection to a file of its own
//! (`<scenario>.dap.json` for the browser session, `<scenario>.child1.dap.json` for the first child, ...) and replay
//! the n-th connection the shell opens from the n-th file. The page is the corpus web project's (`app.ts` compiled
//! with its map, the "Add" button) served by the test, in the real embedded engine when recording and in a fake
//! engine when replaying; the run's own origin, DevTools port and target id are scrubbed as tokens. A click in the
//! page is not DAP: the recorder marks where it happened and the replay holds what followed until the test clicks.
//!
//! The programs: brief 0022's TestApp (`debuggers/mono/Eludite.Debugger.Mono.TestApp`) under `eludite-dbg-mono`, and
//! under netcoredbg built for net10.0; a small Cargo program written here under lldb-dap (attached to with address
//! space randomization off on Linux, as lldb-dap launches programs, so its addresses are the same every time). Recording needs Mono and
//! the built adapter (`dotnet build dotnet/Eludite.slnx`), lldb-dap and cargo, or netcoredbg and the .NET SDK.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_commands::debug as cmds;
use eludite_commands::{Caller, with_caller};
use eludite_dap::record::{self, RecordHandle, RecordOptions, Recording, Scrubber};
use eludite_dap::replay::{self, ReplayHandle, ReplayOptions};
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::super::tests::{Ws, assert_budget, setup_debug};
use super::native::NativeSetup;
use super::state::Mode;
use super::{DebugSetup, JsSetup};
use eludite_dap::transport::AdapterServer;

/// What `${PID}` becomes in a replay (a number no recorded value is likely to equal).
const REPLAY_PID: i64 = 3_999_971;
/// The Mono TestApp's debugger agent port in the attach scenario (fixed, so the recording is too).
const MONO_AGENT_PORT: u16 = 47_033;
/// The Cargo program's package (and binary) name.
const RS_PACKAGE: &str = "conformance";
/// The page the js-debug scenarios debug: the corpus web project's (brief 0038), with a button whose inline handler
/// throws for the exception scenario.
const JS_PAGE: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <title>Minimal API</title>\n</head>\n<body>\n  <h1>Minimal API</h1>\n  <p><label for=\"price\">Price</label><input id=\"price\" type=\"number\" value=\"5\"><button id=\"add\" type=\"button\">Add</button> Total: <span id=\"total\">0</span></p>\n  <p><button id=\"fail\" type=\"button\" onclick=\"JSON.parse('{')\">Fail</button></p>\n  <script src=\"/app.js\"></script>\n</body>\n</html>\n";
/// The most `startDebugging` target ids a js-debug recording scrubs (`${PENDING_TARGET}`, `${PENDING_TARGET_2}`, ...).
const PENDING_TARGETS: usize = 4;

fn pending_placeholder(n: usize) -> String {
    if n == 1 {
        "${PENDING_TARGET}".into()
    } else {
        format!("${{PENDING_TARGET_{n}}}")
    }
}

/// The target ids vscode-js-debug's `startDebugging` requests named, in order (each run's own: scrubbed as tokens).
fn pending_targets(r: &Recording) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in &r.messages {
        if m.dir == record::Dir::Adapter
            && m.message["type"] == "request"
            && m.message["command"] == "startDebugging"
            && let Some(id) = m.message["arguments"]["configuration"]["__pendingTargetId"].as_str()
            && !id.starts_with("${")
            && !out.iter().any(|o| o == id)
        {
            out.push(id.to_owned());
        }
    }
    out.truncate(PENDING_TARGETS);
    out
}

/// Where the replay's fake engine says its tab is (`browser_tests::PageEngine`).
const REPLAY_ORIGIN: &str = "127.0.0.1:5180";

/// The Cargo program the lldb-dap scenarios debug (MIT, this repository's).
const RS_MAIN: &str = r#"fn add(a: i32, b: i32) -> i32 {
    let sum = a + b; // MARK: add-sum
    sum // MARK: add-return
}

fn twice(i: i32) -> i32 {
    i * 2 // MARK: twice
}

fn tick(n: u64) -> u64 {
    n + 1 // MARK: tick
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = String::from("hello");
    let v: Vec<i32> = vec![1, 2, 3];
    let mut count = 7;
    let r = add(count, 3); // MARK: call-add
    count += r; // MARK: after-add
    let mut total = 0;
    for i in 0..10 {
        total += twice(i); // MARK: loop
    }
    println!("{text} {v:?} {count} {total}"); // MARK: print
    if args.iter().any(|a| a == "panic") {
        panic!("boom"); // MARK: panic
    }
    if args.iter().any(|a| a == "linger") {
        println!("lingering");
        // Spins on one line of main, so a pause stops here and nowhere else.
        loop {} // MARK: linger
    }
    if args.iter().any(|a| a == "wait") {
        println!("waiting");
        // Calls `tick` every few milliseconds, so an attached debugger's breakpoint there is hit at once.
        loop {
            std::thread::sleep(std::time::Duration::from_millis(5));
            std::hint::black_box(tick(std::hint::black_box(1))); // MARK: wait
        }
    }
}
"#;

/// Its manifest: full debug information, and no backtrace in a panic's message whatever the recording machine's
/// `RUST_BACKTRACE` says.
const RS_CARGO_TOML: &str = "[package]\nname = \"conformance\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[package.metadata.eludite.run]\nenv = { RUST_BACKTRACE = \"0\" }\n\n[profile.dev]\ndebug = 2\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Adapter {
    Mono,
    Lldb,
    Netcoredbg,
    JsDebug,
}

impl Adapter {
    fn name(self) -> &'static str {
        match self {
            Adapter::Mono => "mono",
            Adapter::Lldb => "lldb",
            Adapter::Netcoredbg => "netcoredbg",
            Adapter::JsDebug => "js-debug",
        }
    }
}

/// What a step does.
enum Act {
    /// F9 on a marked line (`=text` for the line containing `text`), through the UI's command; `fields` (with
    /// `"action": "set"`) make it conditional, a hit count or a tracepoint; `{"action": "delete"}` removes it.
    Line(&'static str, Value),
    /// A function breakpoint through the Breakpoints window's command.
    Function(&'static str, Value),
    /// A command from the UI thread (the Exception Settings window).
    Ui(&'static str, Value),
    /// Keys, as the person presses them.
    Keys(&'static str),
    /// A command from an agent on another thread; its answer is kept in the snapshot. Strings `@line:<mark>`,
    /// `@source`, `@pid`, `@port` and `@ref:<local>` in the arguments are this run's values.
    Agent(&'static str, Value),
    /// An agent's `start` or `attach`: it answers once the session runs, so whether its answer is `running` or
    /// already the first stop depends on how fast the adapter breaks. The answer is not kept; a `wait` step after it
    /// reads the stop.
    AgentStart(&'static str, Value),
    /// A click on the page's button with this name, through `eludite.browser.input` (not DAP: marked in the
    /// recording, and passed in the replay instead of clicking).
    Click(&'static str),
}

/// What a step waits for before its snapshot (always also for the shell to settle).
#[derive(Clone, Copy)]
enum Until {
    Settled,
    /// Break mode at this stop with the windows loaded.
    Break(u64),
    Mode(Mode),
    /// The debuggee printed this line (it is running).
    Output(&'static str),
    /// A child session (vscode-js-debug's page) runs.
    Child,
}

struct Step {
    label: &'static str,
    act: Act,
    until: Until,
}

fn step(label: &'static str, act: Act, until: Until) -> Step {
    Step { label, act, until }
}

/// The scenario's steps. The .NET scenarios run the same steps under `eludite-dbg-mono` and netcoredbg (the TestApp's
/// source is one file); hit counts and tracepoints are the adapter's on Mono and the shell's on netcoredbg.
fn steps(adapter: Adapter, scenario: &str) -> Vec<Step> {
    use Act::*;
    use Until::*;
    let set = |fields: Value| {
        let mut v = json!({"action": "set"});
        v.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        v
    };
    match (adapter, scenario) {
        (Adapter::Mono | Adapter::Netcoredbg, "launch-break-step") => vec![
            step("F9 at add-sum", Line("add-sum", json!({})), Settled),
            step("F5", Keys("f5"), Break(1)),
            step("F10", Keys("f10"), Break(2)),
            step("F11", Keys("f11"), Break(3)),
            step("Shift+F11", Keys("shift-f11"), Break(4)),
            step(
                "agent stack",
                Agent(cmds::STACK, json!({"count": 5})),
                Settled,
            ),
            step(
                "F5 to the end",
                Keys("f5"),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::Mono | Adapter::Netcoredbg, "tracepoints-and-exceptions") => vec![
            step(
                "hit count >=24 in the names loop",
                Line(
                    "=names[n] = \"name\" + n;",
                    set(json!({"hit_condition": ">=24"})),
                ),
                Settled,
            ),
            step(
                "tracepoint at loop-body",
                Line(
                    "loop-body",
                    set(json!({"log_message": "i={i} total={total}", "condition": "i % 25 == 0"})),
                ),
                Settled,
            ),
            step(
                "break when InvalidOperationException is thrown",
                Ui(
                    cmds::EXCEPTION_SETTINGS,
                    json!({"types": [{"type": "System.InvalidOperationException", "break_when_thrown": true}]}),
                ),
                Settled,
            ),
            step("F5 to the 24th hit", Keys("f5"), Break(1)),
            step("F5 to the 25th hit", Keys("f5"), Break(2)),
            step(
                "delete the hit count breakpoint",
                Line("=names[n] = \"name\" + n;", json!({"action": "delete"})),
                Settled,
            ),
            step("F5 to the throw", Keys("f5"), Break(3)),
            step(
                "agent exception_info",
                Agent(cmds::EXCEPTION_INFO, json!({})),
                Settled,
            ),
            step(
                "agent output of the tracepoints",
                Agent(cmds::OUTPUT, json!({"source": "debug"})),
                Settled,
            ),
            step(
                "F5 to the end",
                Keys("f5"),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::Mono | Adapter::Netcoredbg, "run-until-and-trace") => vec![
            step("F9 at add-sum", Line("add-sum", json!({})), Settled),
            step(
                "agent start",
                AgentStart(cmds::START, json!({"wait_ms": 20000})),
                Break(1),
            ),
            step(
                "agent wait until stopped",
                Agent(cmds::WAIT, json!({"until": "stopped", "wait_ms": 10000})),
                Break(1),
            ),
            step(
                "agent stack page",
                Agent(cmds::STACK, json!({"start": 1, "count": 1})),
                Settled,
            ),
            step(
                "agent run_until print-result",
                Agent(
                    cmds::RUN_UNTIL,
                    json!({"points": [{"path": "@source", "line": "@line:print-result"}], "wait_ms": 20000}),
                ),
                Break(2),
            ),
            step(
                "agent variables page",
                Agent(cmds::VARIABLES, json!({"start": 2, "count": 3})),
                Settled,
            ),
            step(
                "agent variables of names from 20",
                Agent(
                    cmds::VARIABLES,
                    json!({"reference": "@ref:names", "start": 20, "count": 10}),
                ),
                Settled,
            ),
            step(
                "agent variables of big",
                Agent(
                    cmds::VARIABLES,
                    json!({"reference": "@ref:big", "count": 20}),
                ),
                Settled,
            ),
            step(
                "agent variables of big from 990",
                Agent(
                    cmds::VARIABLES,
                    json!({"reference": "@ref:big", "start": 990, "count": 20}),
                ),
                Settled,
            ),
            step(
                "agent set_variable result",
                Agent(cmds::SET_VARIABLE, json!({"name": "result", "value": "11"})),
                Settled,
            ),
            step(
                "agent trace loop-body",
                Agent(
                    cmds::TRACE,
                    json!({"points": [{"path": "@source", "line": "@line:loop-body", "message": "i={i} total={total}", "condition": "i % 20 == 0"}],
                           "until": "terminated", "wait_ms": 20000}),
                ),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::Mono, "attach-detach") => vec![
            step("F9 at add-sum", Line("add-sum", json!({})), Settled),
            step(
                "agent attach",
                AgentStart(
                    cmds::ATTACH,
                    json!({"pid": "@pid", "adapter": "mono", "mono": {"address": "127.0.0.1", "port": "@port"}, "wait_ms": 10000}),
                ),
                Break(1),
            ),
            step(
                "agent wait until stopped",
                Agent(cmds::WAIT, json!({"until": "stopped", "wait_ms": 10000})),
                Break(1),
            ),
            step("F10", Keys("f10"), Break(2)),
            step(
                "agent stop detaches",
                Agent(cmds::STOP, json!({})),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::Lldb, "launch-break-step") => vec![
            step("F9 at call-add", Line("call-add", json!({})), Settled),
            step(
                "hit count %4 in the loop",
                Line("loop", set(json!({"hit_condition": "%4"}))),
                Settled,
            ),
            step("F5", Keys("f5"), Break(1)),
            step("F11", Keys("f11"), Break(2)),
            step("F10", Keys("f10"), Break(3)),
            step("Shift+F11", Keys("shift-f11"), Break(4)),
            step("F5 to the 4th hit", Keys("f5"), Break(5)),
            step("F5 to the 8th hit", Keys("f5"), Break(6)),
            step(
                "delete the loop breakpoint",
                Line("loop", json!({"action": "delete"})),
                Settled,
            ),
            step(
                "F5 to the end",
                Keys("f5"),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::Lldb, "pause-and-set-variable") => vec![
            step("F9 at after-add", Line("after-add", json!({})), Settled),
            step(
                "agent start lingering",
                AgentStart(cmds::START, json!({"args": ["linger"], "wait_ms": 20000})),
                Break(1),
            ),
            step(
                "agent wait until stopped",
                Agent(cmds::WAIT, json!({"until": "stopped", "wait_ms": 10000})),
                Break(1),
            ),
            step(
                "agent set_variable count",
                Agent(cmds::SET_VARIABLE, json!({"name": "count", "value": "40"})),
                Settled,
            ),
            step("F10", Keys("f10"), Break(2)),
            step(
                "F9 off at after-add",
                Line("after-add", json!({"action": "delete"})),
                Settled,
            ),
            step("F5", Keys("f5"), Output("lingering")),
            step(
                "agent pause",
                Agent(cmds::PAUSE, json!({"wait_ms": 10000})),
                Break(3),
            ),
            step(
                "Shift+F5",
                Keys("shift-f5"),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::Lldb, "attach-detach") => vec![
            step("F9 at tick", Line("tick", json!({})), Settled),
            step(
                "agent attach",
                AgentStart(
                    cmds::ATTACH,
                    json!({"pid": "@pid", "adapter": "lldb", "wait_ms": 10000}),
                ),
                Break(1),
            ),
            step(
                "agent wait until stopped",
                Agent(cmds::WAIT, json!({"until": "stopped", "wait_ms": 10000})),
                Break(1),
            ),
            step(
                "agent stack",
                Agent(cmds::STACK, json!({"count": 3})),
                Settled,
            ),
            step(
                "F9 off at tick",
                Line("tick", json!({"action": "delete"})),
                Settled,
            ),
            step(
                "agent stop detaches",
                Agent(cmds::STOP, json!({})),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::Lldb, "function-breakpoints-and-panic") => vec![
            step(
                "function breakpoint twice",
                Function("twice", json!({"action": "set"})),
                Settled,
            ),
            step(
                "tracepoint at print",
                Line(
                    "print",
                    set(json!({"log_message": "count={count} total={total}"})),
                ),
                Settled,
            ),
            step(
                "break on Rust panics",
                Ui(
                    cmds::EXCEPTION_SETTINGS,
                    json!({"break_on_rust_panic": true}),
                ),
                Settled,
            ),
            step(
                "agent start panicking",
                AgentStart(cmds::START, json!({"args": ["panic"], "wait_ms": 20000})),
                Break(1),
            ),
            step(
                "agent wait until stopped",
                Agent(cmds::WAIT, json!({"until": "stopped", "wait_ms": 10000})),
                Break(1),
            ),
            step(
                "delete the function breakpoint",
                Function("twice", json!({"action": "delete"})),
                Settled,
            ),
            step(
                "agent continue to the panic",
                Agent(cmds::CONTINUE, json!({"wait_ms": 10000})),
                Break(2),
            ),
            step(
                "agent stack",
                Agent(cmds::STACK, json!({"count": 8})),
                Settled,
            ),
            step(
                "agent stop",
                Agent(cmds::STOP, json!({})),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::JsDebug, "attach-break-step") => vec![
            step(
                "F9 in onAdd",
                Line("=const sum = total(cart);", json!({})),
                Settled,
            ),
            step(
                "agent attach to the tab",
                AgentStart(
                    cmds::ATTACH,
                    json!({"tab": "t1", "web_root": "@webroot", "wait_ms": 20000}),
                ),
                Child,
            ),
            step("click Add", Click("Add"), Break(1)),
            step("F11 into total", Keys("f11"), Break(2)),
            step("F10", Keys("f10"), Break(3)),
            step(
                "agent stack",
                Agent(cmds::STACK, json!({"count": 5})),
                Settled,
            ),
            step("Shift+F11", Keys("shift-f11"), Break(4)),
            step("F5", Keys("f5"), Mode(super::state::Mode::Running)),
            step(
                "agent stop",
                Agent(cmds::STOP, json!({"session": "@browser"})),
                Mode(super::state::Mode::Design),
            ),
        ],
        (Adapter::JsDebug, "logpoints-and-exceptions") => vec![
            step(
                "tracepoint in onAdd",
                Line(
                    "=cart.push(item);",
                    set(json!({"log_message": "added {item.name} at {price}"})),
                ),
                Settled,
            ),
            step(
                "break when thrown",
                Ui(cmds::EXCEPTION_SETTINGS, json!({"break_when_thrown": true})),
                Settled,
            ),
            step(
                "agent attach to the tab",
                AgentStart(
                    cmds::ATTACH,
                    json!({"tab": "t1", "web_root": "@webroot", "wait_ms": 20000}),
                ),
                Child,
            ),
            step("click Add", Click("Add"), Output("added item 1 at 5")),
            step("click Fail", Click("Fail"), Break(1)),
            step(
                "agent exception_info",
                Agent(cmds::EXCEPTION_INFO, json!({})),
                Settled,
            ),
            step(
                "agent stop",
                Agent(cmds::STOP, json!({"session": "@browser"})),
                Mode(super::state::Mode::Design),
            ),
        ],
        other => panic!("no scenario {other:?}"),
    }
}

/// The repository's root, canonical.
fn repo_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&root).unwrap_or(root)
}

fn corpus_dir() -> PathBuf {
    repo_root().join("corpus/dap")
}

fn testapp_dir() -> PathBuf {
    repo_root().join("debuggers/mono/Eludite.Debugger.Mono.TestApp")
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn env_on(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0")
}

/// A real adapter found on this machine, to record with.
enum Real {
    Mono {
        mono: eludite_dap::discovery::MonoInstall,
        adapter: PathBuf,
        version: String,
    },
    Lldb {
        adapter: eludite_dap::discovery::LldbAdapter,
        version: String,
    },
    Netcoredbg {
        found: eludite_dap::discovery::Found,
    },
    JsDebug {
        setup: JsSetup,
    },
}

/// The real adapter, or why not (printed as the record step's skip).
fn find_real(adapter: Adapter) -> Result<Real, String> {
    match adapter {
        Adapter::Mono => {
            if cfg!(windows) {
                return Err(".NET Framework under Mono is not debugged on Windows".into());
            }
            let mono = eludite_dap::discovery::MonoSearch::from_env().find_mono()?;
            let adapter = env_dir("ELUDITE_DBG_MONO").unwrap_or_else(|| {
                repo_root().join(
                    "debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe",
                )
            });
            let app = testapp_dir().join("bin/Debug/net472/Eludite.Debugger.Mono.TestApp.exe");
            if !adapter.is_file() || !app.is_file() {
                return Err("eludite-dbg-mono or the TestApp is not built (dotnet build dotnet/Eludite.slnx)".into());
            }
            let version = format!(
                "eludite-dbg-mono under mono {}",
                mono.version().unwrap_or_else(|| "(unknown version)".into())
            );
            Ok(Real::Mono {
                mono,
                adapter,
                version,
            })
        }
        Adapter::Lldb => {
            let adapter = eludite_dap::discovery::LldbSearch {
                configured: env_dir("ELUDITE_LLDB_DAP"),
                ..eludite_dap::discovery::LldbSearch::from_env()
            }
            .find(eludite_dap::launch::Platform::current())?;
            if std::process::Command::new("cargo")
                .arg("--version")
                .output()
                .is_err()
            {
                return Err("cargo is not on PATH".into());
            }
            let version = adapter.describe(adapter.version().as_deref());
            Ok(Real::Lldb { adapter, version })
        }
        Adapter::Netcoredbg => {
            let found = eludite_dap::discovery::AdapterSearch::from_env().find_netcoredbg()?;
            if std::process::Command::new("dotnet")
                .arg("--version")
                .output()
                .is_err()
            {
                return Err("the .NET SDK (dotnet) is not on PATH".into());
            }
            Ok(Real::Netcoredbg { found })
        }
        Adapter::JsDebug => {
            let mut setup = JsSetup::from_env();
            setup.search.configured = env_dir("ELUDITE_JS_DEBUG");
            setup.node.configured = env_dir("ELUDITE_NODE");
            setup.search.find()?;
            let (node, _) = setup.node.find(&eludite_dap::discovery::JS_DEBUG_NODE)?;
            eludite_dap::discovery::check_node_version(
                &node,
                eludite_dap::discovery::node_version_output(&node).as_deref(),
            )?;
            match eludite_browser::select_engine(
                eludite_browser::EngineChoice::Embedded,
                &eludite_browser::ChromiumSearch::defaults(),
            ) {
                (eludite_browser::EngineChoice::Embedded, _) => Ok(Real::JsDebug { setup }),
                (_, why) => Err(why.unwrap_or_else(|| "the embedded engine was not found".into())),
            }
        }
    }
}

/// The file of connection `n` (0: the first) of a scenario recorded at `first` (`<scenario>.dap.json`):
/// `<scenario>.child<n>.dap.json` for the later ones (vscode-js-debug's child sessions).
fn connection_path(first: &Path, n: usize) -> PathBuf {
    if n == 0 {
        return first.to_path_buf();
    }
    let name = first.file_name().unwrap().to_string_lossy();
    let stem = name.strip_suffix(".dap.json").unwrap_or(&name);
    first.with_file_name(format!("{stem}.child{n}.dap.json"))
}

/// The later connections' recordings beside `first`, in order, as far as they go.
fn more_recordings(first: &Path) -> Vec<Recording> {
    let mut out = Vec::new();
    for n in 1.. {
        match Recording::read(&connection_path(first, n)) {
            Ok(r) => out.push(r),
            Err(_) => break,
        }
    }
    out
}

/// The real vscode-js-debug server with the recorder on each connection it opens (brief 0038).
struct RecordingServer {
    inner: Arc<dyn AdapterServer>,
    /// The order across the server's connections.
    order: Arc<std::sync::atomic::AtomicU64>,
    first: PathBuf,
    version: String,
    roots: Arc<Mutex<Vec<(String, PathBuf)>>>,
    tokens: Arc<Mutex<Vec<(String, String)>>>,
    handles: Arc<Mutex<Vec<RecordHandle>>>,
    first_handle: Arc<Mutex<Option<RecordHandle>>>,
}

impl AdapterServer for RecordingServer {
    fn connect(&self) -> std::io::Result<eludite_dap::Connection> {
        let conn = self.inner.connect()?;
        let mut handles = self.handles.lock().unwrap();
        let (conn, handle) = record::record_ordered(
            conn,
            RecordOptions {
                path: connection_path(&self.first, handles.len()),
                adapter: Adapter::JsDebug.name().into(),
                version: self.version.clone(),
                roots: self.roots.lock().unwrap().clone(),
            },
            self.order.clone(),
        );
        for (placeholder, text) in self.tokens.lock().unwrap().iter() {
            handle.add_token(placeholder, text);
        }
        // The server's own loopback address (`tcp 127.0.0.1:PORT`).
        if let Some(address) = self.inner.describe().strip_prefix("tcp ") {
            handle.add_token("${JS_DEBUG}", address);
        }
        if handles.is_empty() {
            *self.first_handle.lock().unwrap() = Some(handle.clone());
        }
        handles.push(handle);
        Ok(conn)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// The replay of a vscode-js-debug scenario: connection n from recording n (brief 0038).
struct ReplayServer {
    recordings: Vec<Recording>,
    group: Arc<replay::ReplayGroup>,
    roots: Arc<Mutex<Vec<(String, PathBuf)>>>,
    tokens: Arc<Mutex<Vec<(String, String)>>>,
    handles: Arc<Mutex<Vec<ReplayHandle>>>,
    first_handle: Arc<Mutex<Option<ReplayHandle>>>,
}

impl AdapterServer for ReplayServer {
    fn connect(&self) -> std::io::Result<eludite_dap::Connection> {
        let mut handles = self.handles.lock().unwrap();
        let recording = self.recordings.get(handles.len()).ok_or_else(|| {
            std::io::Error::other(format!(
                "the recording has {} connections; the shell opened one more",
                self.recordings.len()
            ))
        })?;
        let (conn, handle) = replay::serve_in_group(
            recording,
            ReplayOptions {
                roots: self.roots.lock().unwrap().clone(),
                pid: REPLAY_PID,
                real_time: false,
                tokens: self.tokens.lock().unwrap().clone(),
            },
            self.group.clone(),
        );
        if handles.is_empty() {
            *self.first_handle.lock().unwrap() = Some(handle.clone());
        }
        handles.push(handle);
        Ok(conn)
    }

    fn describe(&self) -> String {
        "replay (tcp)".into()
    }
}

/// Serve `dir`'s files over HTTP on a loopback port (the js-debug scenarios' page, recording). The thread lives as
/// long as the test process.
fn serve_static(dir: PathBuf) -> u16 {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let dir = dir.clone();
            std::thread::spawn(move || {
                let mut reader = std::io::BufReader::new(&stream);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    return;
                }
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/");
                let path = path
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .trim_start_matches('/');
                let path = if path.is_empty() { "index.html" } else { path };
                let file = dir.join(path);
                let (status, body) = match std::fs::read(&file) {
                    Ok(b) if !path.contains("..") => ("200 OK", b),
                    _ => ("404 Not Found", b"not found".to_vec()),
                };
                let kind = match file.extension().and_then(|e| e.to_str()) {
                    Some("html") => "text/html; charset=utf-8",
                    Some("js") => "text/javascript",
                    Some("map") => "application/json",
                    _ => "text/plain",
                };
                let mut out = &stream;
                let _ = write!(
                    out,
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = out.write_all(&body);
            });
        }
    });
    port
}

/// Starts the real adapter (recording).
type RealConnect = Box<dyn Fn() -> std::io::Result<eludite_dap::Connection> + Send + Sync>;

/// How the scenario's session reaches its adapter.
enum Source {
    Record {
        real: Real,
        path: PathBuf,
    },
    /// The recording, and the later connections' (vscode-js-debug's child sessions).
    Replay {
        recording: Recording,
        more: Vec<Recording>,
    },
}

/// The shell of one scenario run.
struct Run {
    w: Ws,
    /// The program's source file (the breakpoints' marks).
    source: PathBuf,
    text: String,
    /// The process an attach targets (`@pid`).
    pid: i64,
    /// What the agent's last answer had, for `@ref:`.
    last_answer: Value,
    record: Arc<Mutex<Option<RecordHandle>>>,
    replay: Arc<Mutex<Option<ReplayHandle>>>,
    /// The TestApp waiting for an attach (recording).
    attached_app: Option<std::process::Child>,
    /// Placeholder roots this run substitutes and scrubs.
    roots: Vec<(String, PathBuf)>,
    recording: bool,
    /// Every connection's recorder or replay, the first one's also in `record` or `replay` (vscode-js-debug).
    records: Arc<Mutex<Vec<RecordHandle>>>,
    replays: Arc<Mutex<Vec<ReplayHandle>>>,
    /// The run's tokens (brief 0038: the page's origin, the DevTools endpoint, the target id).
    tokens: Vec<(String, String)>,
    /// The folder the js-debug scenarios' page is served from (`@webroot`).
    web: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(mut c) = self.attached_app.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// How long a step may take: a real adapter is slower than a replay.
fn step_timeout(recording: bool) -> Duration {
    if recording {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(10)
    }
}

impl Run {
    /// Run the UI until `done`, failing with what the replay waits for when it does not come.
    fn wait(&mut self, what: &str, mut done: impl FnMut(&mut Ws) -> bool) {
        let deadline = Instant::now() + step_timeout(self.recording);
        loop {
            self.w.vcx.run_until_parked();
            if done(&mut self.w) {
                return;
            }
            if Instant::now() >= deadline {
                if let Some(h) = self.record.lock().unwrap().clone() {
                    // What the real adapter said so far, to see why.
                    let _ = h.write();
                }
                for h in self.records.lock().unwrap().iter() {
                    let _ = h.write();
                }
                panic!("timed out waiting for {what}{}", self.replay_report());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn replay_report(&self) -> String {
        let mut all = self.replays.lock().unwrap().clone();
        if all.is_empty() {
            all.extend(self.replay.lock().unwrap().clone());
        }
        let mut out = String::new();
        for (n, h) in all.iter().enumerate() {
            let which = if n == 0 {
                "the replay".to_owned()
            } else {
                format!("the replay of connection {}", n + 1)
            };
            if let Some(w) = h.waiting_for() {
                out.push_str(&format!("\n{which}: {w}"));
            }
            for f in h.failures() {
                out.push_str(&format!("\n{which}: {f}"));
            }
        }
        out
    }

    /// Run a browser command on another thread, running the UI meanwhile.
    fn browser(&mut self, command: &'static str, args: Value) -> Value {
        let commands = self.w.commands.clone();
        let handle = std::thread::spawn(move || {
            commands
                .invoke(command, args)
                .unwrap_or_else(|e| json!({ "error": e.to_string() }))
        });
        let deadline = Instant::now() + step_timeout(self.recording);
        while !handle.is_finished() {
            assert!(Instant::now() < deadline, "{command} did not finish");
            self.w.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(1));
        }
        handle.join().unwrap()
    }

    /// The page's button `name` clicked (recording: marked first on every connection; replay: the mark passed).
    fn click(&mut self, name: &str) {
        let mark = format!("click {name}");
        if !self.recording {
            for h in self.replays.lock().unwrap().iter() {
                h.mark(&mark);
            }
            return;
        }
        let read = self.browser(eludite_commands::browser::READ_PAGE, json!({"tab": "t1"}));
        let r = read["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|n| n["role"] == "button" && n["name"] == name)
            .and_then(|n| n["ref"].as_str())
            .unwrap_or_else(|| panic!("no {name} button: {read}"))
            .to_owned();
        for h in self.records.lock().unwrap().iter() {
            h.mark(&mark);
        }
        let out = self.browser(
            eludite_commands::browser::INPUT,
            json!({"tab": "t1", "action": "click", "ref": r, "wait_ms": 0}),
        );
        assert!(out.get("error").is_none(), "click {name}: {out}");
    }

    /// The browser session's id (`@browser`).
    fn browser_session(&self) -> u32 {
        self.w.shell.read_with(&self.w.vcx, |s, _| {
            s.debugger()
                .sessions_info()
                .iter()
                .find(|r| r.parent.is_none() && r.runtime.as_deref() == Some("javascript"))
                .map(|r| r.id)
                .expect("a browser session")
        })
    }

    fn cmd(&mut self, command: &str, args: Value) -> Value {
        let r = self.w.shell.update_in(&mut self.w.vcx, |s, window, cx| {
            s.invoke(command, args.clone(), window, cx)
        });
        self.w.vcx.run_until_parked();
        r.unwrap_or_else(|e| panic!("{command} {args}: {e}{}", self.replay_report()))
    }

    /// The 1-based line of `mark` (`// MARK: mark`, or `=text` for the line containing `text`).
    fn line_of(&self, mark: &str) -> u32 {
        let found = match mark.strip_prefix('=') {
            Some(text) => self.text.lines().position(|l| l.contains(text)),
            None => self
                .text
                .lines()
                .position(|l| l.ends_with(&format!("// MARK: {mark}"))),
        };
        found.unwrap_or_else(|| panic!("no line {mark}")) as u32 + 1
    }

    /// `args` with this run's `@` values.
    fn expand(&self, v: &Value) -> Value {
        match v {
            Value::String(s) if s == "@source" => json!(self.source.to_string_lossy()),
            Value::String(s) if s == "@pid" => json!(self.pid),
            Value::String(s) if s == "@port" => json!(MONO_AGENT_PORT),
            Value::String(s) if s == "@webroot" => json!(self.web.to_string_lossy()),
            Value::String(s) if s == "@browser" => json!(self.browser_session()),
            Value::String(s) if s.starts_with("@line:") => json!(self.line_of(&s[6..])),
            Value::String(s) if s.starts_with("@ref:") => {
                let name = &s[5..];
                let rows = self.last_answer["locals"]["rows"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                rows.iter()
                    .find(|r| r["name"] == name)
                    .map(|r| r["reference"].clone())
                    .unwrap_or_else(|| panic!("no local {name} in {}", self.last_answer))
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.expand(x)).collect()),
            Value::Object(o) => {
                Value::Object(o.iter().map(|(k, x)| (k.clone(), self.expand(x))).collect())
            }
            other => other.clone(),
        }
    }

    /// Run `command` as an agent on another thread, running the UI meanwhile.
    fn agent(&mut self, command: &'static str, args: Value) -> Value {
        let commands = self.w.commands.clone();
        let caller = Caller::Agent {
            agent: "Conformance Agent".into(),
            call: eludite_commands::next_call_id(),
            tool_call: None,
        };
        let handle = std::thread::spawn(move || {
            with_caller(caller, || {
                commands
                    .invoke(command, args)
                    .unwrap_or_else(|e| json!({ "error": e.to_string() }))
            })
        });
        let deadline = Instant::now() + step_timeout(self.recording);
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                panic!(
                    "the agent's {command} did not finish{}",
                    self.replay_report()
                );
            }
            self.w.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(1));
        }
        handle.join().unwrap()
    }

    /// No request outstanding, the locals loaded, no launch or stop in progress: what a person would see settle.
    fn quiet(w: &Ws) -> bool {
        w.shell.read_with(&w.vcx, |s, _| {
            let d = s.debugger();
            d.pending.is_empty()
                && !d.model.locals_loading
                && d.trace_hits.is_empty()
                && d.pending_launch.is_none()
                && !matches!(
                    d.model.mode,
                    Mode::Launching | Mode::Stopping | Mode::Building
                )
        })
    }

    /// Wait for `until`, then for the shell to stay quiet a moment (longer against a real adapter, whose late events
    /// would otherwise land after the snapshot).
    fn settle(&mut self, until: Until, label: &str) {
        match until {
            Until::Settled => {}
            Until::Break(stop) => self.wait(&format!("{label}: break {stop}"), |w| {
                w.shell.read_with(&w.vcx, |s, _| {
                    let m = &s.debugger().model;
                    m.mode == Mode::Break && m.stop == stop && !m.locals_loading
                })
            }),
            Until::Mode(mode) => self.wait(&format!("{label}: mode {mode:?}"), |w| {
                w.shell
                    .read_with(&w.vcx, |s, _| s.debugger().model.mode == mode)
            }),
            Until::Output(line) => self.wait(&format!("{label}: output {line}"), |w| {
                w.shell.read_with(&w.vcx, |s, _| {
                    let m = &s.debugger().model;
                    m.mode == Mode::Running
                        && m.state().console.tail.iter().any(|l| l.contains(line))
                })
            }),
            Until::Child => self.wait(&format!("{label}: a child session"), |w| {
                w.shell.read_with(&w.vcx, |s, _| {
                    let d = s.debugger();
                    d.sessions_info()
                        .iter()
                        .any(|r| r.parent.is_some() && r.mode == "running")
                        && d.state()
                            .breakpoints
                            .iter()
                            .all(|b| b.verified || !b.enabled)
                })
            }),
        }
        let need = if self.recording { 15 } else { 2 };
        let mut calm = 0;
        self.wait(&format!("{label}: the shell to settle"), |w| {
            if Self::quiet(w) {
                calm += 1;
            } else {
                calm = 0;
            }
            if calm < need {
                std::thread::sleep(Duration::from_millis(10));
            }
            calm >= need
        });
    }

    /// The windows' rows as text.
    fn windows(&self) -> Value {
        self.w.shell.read_with(&self.w.vcx, |s, cx| {
            let d = s.debugger();
            let locals: Vec<String> = d
                .windows
                .locals
                .read(cx)
                .rows()
                .iter()
                .map(|r| {
                    format!(
                        "{}{}{} = {} : {}",
                        "  ".repeat(r.depth),
                        match r.expanded {
                            Some(true) => "- ",
                            Some(false) => "+ ",
                            None => "",
                        },
                        r.name,
                        r.value,
                        r.type_name
                    )
                })
                .collect();
            let stack: Vec<String> = d
                .windows
                .call_stack
                .read(cx)
                .rows()
                .iter()
                .map(|r| {
                    format!(
                        "{}{} | {}",
                        if r.selected { "> " } else { "  " },
                        r.name,
                        r.location
                    )
                })
                .collect();
            let threads: Vec<String> = d
                .windows
                .threads
                .read(cx)
                .rows()
                .iter()
                .map(|t| {
                    format!(
                        "{}{} {} | {}",
                        if t.current { "> " } else { "  " },
                        t.id,
                        t.name,
                        t.location
                    )
                })
                .collect();
            let breakpoints: Vec<String> = d
                .windows
                .breakpoints
                .read(cx)
                .rows()
                .iter()
                .map(breakpoint_text)
                .collect();
            json!({
                "locals": locals,
                "call_stack": stack,
                "threads": threads,
                "breakpoints": breakpoints,
            })
        })
    }

    fn act(&mut self, s: &Step) -> Option<Value> {
        match &s.act {
            Act::Line(mark, fields) => {
                let mut args =
                    json!({"path": self.source.to_string_lossy(), "line": self.line_of(mark)});
                args.as_object_mut()
                    .unwrap()
                    .extend(fields.as_object().unwrap().clone());
                self.cmd(cmds::TOGGLE_BREAKPOINT, args);
                None
            }
            Act::Function(name, fields) => {
                let mut args = json!({"function": name});
                args.as_object_mut()
                    .unwrap()
                    .extend(fields.as_object().unwrap().clone());
                self.cmd(cmds::TOGGLE_BREAKPOINT, args);
                None
            }
            Act::Ui(command, args) => {
                self.cmd(command, args.clone());
                None
            }
            Act::Keys(keys) => {
                self.w.vcx.simulate_keystrokes(keys);
                None
            }
            Act::Agent(command, args) => {
                let args = self.expand(args);
                let answer = self.agent(command, args);
                if answer.get("locals").is_some() {
                    self.last_answer = answer.clone();
                }
                Some(answer)
            }
            Act::AgentStart(command, args) => {
                let args = self.expand(args);
                let answer = self.agent(command, args);
                assert!(answer.get("error").is_none(), "{command}: {answer}");
                None
            }
            Act::Click(name) => {
                self.click(name);
                None
            }
        }
    }

    /// Run the steps; one snapshot per step.
    fn run(&mut self, steps: &[Step]) -> Vec<Value> {
        let mut out = Vec::new();
        for s in steps {
            let answer = self.act(s);
            self.settle(s.until, s.label);
            if let Some(h) = self.replay.lock().unwrap().clone() {
                h.check().unwrap_or_else(|e| panic!("{}: {e}", s.label));
            }
            for h in self.replays.lock().unwrap().iter() {
                h.check().unwrap_or_else(|e| panic!("{}: {e}", s.label));
            }
            let state = self.cmd(cmds::STATE, json!({}));
            out.push(json!({
                "step": s.label,
                "state": state,
                "answer": answer,
                "windows": self.windows(),
            }));
        }
        out
    }
}

fn file_name(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_owned())
}

fn breakpoint_text(r: &cmds::BreakpointRow) -> String {
    let at = match (&r.function, &r.path, r.line) {
        (Some(f), _, _) => format!("function {f}"),
        (_, Some(p), Some(l)) => format!("{}, line {l}", file_name(p)),
        _ => "?".into(),
    };
    let mut t = format!(
        "{at} [{}{}] hits {}",
        if r.enabled { "enabled" } else { "disabled" },
        if r.verified { ", bound" } else { "" },
        r.hits
    );
    for (k, v) in [
        ("when", &r.condition),
        ("hit count", &r.hit_condition),
        ("print", &r.log_message),
        ("message", &r.message),
    ] {
        if let Some(v) = v {
            t.push_str(&format!(" {k} {v}"));
        }
    }
    t
}

/// Copy `from`'s built TestApp files into the solution's output folder (or empty files to stand for them in a
/// replay, where nothing runs).
fn testapp_output(w: &Ws, out: &str, real: bool) {
    let dir = w.path(out);
    std::fs::create_dir_all(&dir).unwrap();
    let built = testapp_dir().join("bin/Debug/net472");
    for f in [
        "Eludite.Debugger.Mono.TestApp.exe",
        "Eludite.Debugger.Mono.TestApp.pdb",
        "Eludite.Debugger.Mono.TestApp.exe.config",
    ] {
        if real {
            std::fs::copy(built.join(f), dir.join(f)).unwrap();
        } else {
            std::fs::write(dir.join(f), "").unwrap();
        }
    }
}

/// Set a debugger path setting through the bus and wait until the searches have it.
fn set_mono_prefix(run: &mut Run, prefix: &Path) {
    run.w
        .commands
        .invoke(
            eludite_commands::settings::SET,
            json!({"key": "debugger.monoPrefix", "value": prefix.to_string_lossy()}),
        )
        .unwrap();
    let want = Some(prefix.to_path_buf());
    run.wait("debugger.monoPrefix", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.debugger().setup().mono.configured == want)
    });
}

fn set_build_before_run(run: &mut Run, on: bool) {
    run.w
        .commands
        .invoke(
            eludite_commands::settings::SET,
            json!({"key": "build.beforeRun", "value": on}),
        )
        .unwrap();
    run.wait("build before run", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.builds().build_before_run == on)
    });
}

fn run_checked(cmd: &mut std::process::Command, what: &str) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(
        out.status.success(),
        "{what}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The shell for `adapter`'s `scenario`, its program in place and its connector recording or replaying.
fn start(cx: &mut TestAppContext, adapter: Adapter, scenario: &str, source: Source) -> Run {
    let record: Arc<Mutex<Option<RecordHandle>>> = Arc::default();
    let replay_slot: Arc<Mutex<Option<ReplayHandle>>> = Arc::default();
    let roots_slot: Arc<Mutex<Vec<(String, PathBuf)>>> = Arc::default();
    let recording = matches!(source, Source::Record { .. });
    let real_mono = match &source {
        Source::Record {
            real: Real::Mono { mono, .. },
            ..
        } => Some(mono.clone()),
        _ => None,
    };
    let attach = scenario == "attach-detach";
    let pid_slot: Arc<Mutex<i64>> = Arc::new(Mutex::new(REPLAY_PID));
    let store = tempfile::tempdir().unwrap().keep();
    // The connection: the real adapter with the recorder, or the replay. One session per scenario.
    let (rec, rep, roots, pid) = (
        record.clone(),
        replay_slot.clone(),
        roots_slot.clone(),
        pid_slot.clone(),
    );
    let (real_connect, recording_data, record_path): (
        Option<RealConnect>,
        Option<Recording>,
        Option<PathBuf>,
    ) = match &source {
        Source::Record { real, path } => {
            let connect: RealConnect = match real {
                Real::Mono { mono, adapter, .. } => {
                    let (t, env) = (mono.adapter_transport(adapter), mono.env.clone());
                    Box::new(move || eludite_dap::transport::connect_with_env(&t, &env))
                }
                Real::Lldb { adapter, .. } => {
                    let t = adapter.transport();
                    Box::new(move || eludite_dap::transport::connect(&t))
                }
                Real::Netcoredbg { found } => {
                    let t = found.transport();
                    Box::new(move || eludite_dap::transport::connect(&t))
                }
                // vscode-js-debug is reached through `js` below, never `connect`.
                Real::JsDebug { .. } => {
                    Box::new(|| Err(std::io::Error::other("vscode-js-debug is started by `js`")))
                }
            };
            (Some(connect), None, Some(path.clone()))
        }
        Source::Replay { recording, .. } => (None, Some(recording.clone()), None),
    };
    let version = match &source {
        Source::Record { real, .. } => match real {
            Real::Mono { version, .. } | Real::Lldb { version, .. } => version.clone(),
            Real::Netcoredbg { found } => format!("netcoredbg ({})", found.path.display()),
            Real::JsDebug { .. } => String::new(),
        },
        Source::Replay { recording, .. } => recording.version.clone(),
    };
    // vscode-js-debug: a server whose every connection is recorded, or replayed from its own recording.
    let records: Arc<Mutex<Vec<RecordHandle>>> = Arc::default();
    let replays: Arc<Mutex<Vec<ReplayHandle>>> = Arc::default();
    let tokens_slot: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let js = if adapter == Adapter::JsDebug {
        let (roots, tokens) = (roots_slot.clone(), tokens_slot.clone());
        let starter: super::JsStarter = match &source {
            Source::Record {
                real: Real::JsDebug { setup },
                path,
            } => {
                let (setup, first, records, first_handle) =
                    (setup.clone(), path.clone(), records.clone(), record.clone());
                Arc::new(move || {
                    let (inner, description, version) = setup.start_server()?;
                    let server = RecordingServer {
                        inner,
                        order: Arc::default(),
                        first: first.clone(),
                        version: version.clone(),
                        roots: roots.clone(),
                        tokens: tokens.clone(),
                        handles: records.clone(),
                        first_handle: first_handle.clone(),
                    };
                    Ok((
                        Arc::new(server) as Arc<dyn AdapterServer>,
                        description,
                        version,
                    ))
                })
            }
            Source::Replay { recording, more } => {
                let mut recordings = vec![recording.clone()];
                recordings.extend(more.iter().cloned());
                let (replays, first_handle) = (replays.clone(), replay_slot.clone());
                Arc::new(move || {
                    let version = recordings[0].version.clone();
                    let server = ReplayServer {
                        group: replay::ReplayGroup::new(&recordings),
                        recordings: recordings.clone(),
                        roots: roots.clone(),
                        tokens: tokens.clone(),
                        handles: replays.clone(),
                        first_handle: first_handle.clone(),
                    };
                    Ok((
                        Arc::new(server) as Arc<dyn AdapterServer>,
                        format!("replay of {version}"),
                        version,
                    ))
                })
            }
            Source::Record { .. } => unreachable!("js-debug records with Real::JsDebug"),
        };
        JsSetup {
            start: Some(starter),
            ..JsSetup::default()
        }
    } else {
        JsSetup::default()
    };
    let adapter_name = adapter.name();
    let setup = DebugSetup {
        connect: Some(Arc::new(move || {
            let roots = roots.lock().unwrap().clone();
            if let Some(connect) = &real_connect {
                assert!(rec.lock().unwrap().is_none(), "a scenario has one session");
                let (conn, handle) = record::record(
                    connect()?,
                    RecordOptions {
                        path: record_path.clone().unwrap(),
                        adapter: adapter_name.into(),
                        version: version.clone(),
                        roots,
                    },
                );
                *rec.lock().unwrap() = Some(handle);
                Ok(conn)
            } else {
                assert!(rep.lock().unwrap().is_none(), "a scenario has one session");
                let (conn, handle) = replay::serve(
                    recording_data.as_ref().unwrap(),
                    ReplayOptions {
                        roots,
                        pid: *pid.lock().unwrap(),
                        real_time: false,
                        tokens: Vec::new(),
                    },
                );
                *rep.lock().unwrap() = Some(handle);
                Ok(conn)
            }
        })),
        search: eludite_dap::discovery::AdapterSearch::default(),
        mono: eludite_dap::discovery::MonoSearch::default(),
        mono_adapter: eludite_dap::discovery::MonoAdapterSearch::default(),
        // .NET Framework runs under Mono whatever the platform the replay runs on.
        platform: match adapter {
            Adapter::Mono => eludite_dap::launch::Platform::Linux,
            _ => eludite_dap::launch::Platform::current(),
        },
        store_dir: Some(store),
        dotnet: "dotnet".into(),
        js,
    };
    let w = setup_debug(cx, |_| {}, None, Some(setup));
    let tmp = w.dir.path().to_path_buf();
    let mut run = Run {
        w,
        source: PathBuf::new(),
        text: String::new(),
        pid: REPLAY_PID,
        last_answer: Value::Null,
        record,
        replay: replay_slot,
        attached_app: None,
        roots: Vec::new(),
        recording,
        records,
        replays,
        tokens: Vec::new(),
        web: PathBuf::new(),
    };
    set_build_before_run(&mut run, false);
    let mut roots = vec![
        ("${ROOT}".to_owned(), repo_root()),
        ("${TMP}".to_owned(), tmp.clone()),
    ];
    let write = |rel: &str, text: &str| {
        let p = tmp.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    match adapter {
        Adapter::Mono | Adapter::Netcoredbg => {
            let source = std::fs::canonicalize(testapp_dir().join("Program.cs")).unwrap();
            run.text = std::fs::read_to_string(&source).unwrap();
            run.source = source.clone();
            if adapter == Adapter::Mono {
                write(
                    "src/App/App.csproj",
                    "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net472</TargetFramework><AssemblyName>Eludite.Debugger.Mono.TestApp</AssemblyName></PropertyGroup></Project>",
                );
                testapp_output(&run.w, "src/App/bin/Debug/net472", recording);
                // Mono: the real one when recording, else a stand-in the launch configuration names.
                let mono_exe = match &real_mono {
                    Some(m) => m.mono.clone(),
                    None => {
                        let prefix = tmp.join("mono-prefix");
                        let exe = prefix.join("bin").join(if cfg!(windows) {
                            "mono.exe"
                        } else {
                            "mono"
                        });
                        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
                        std::fs::write(&exe, "").unwrap();
                        exe
                    }
                };
                let prefix = mono_exe.parent().unwrap().parent().unwrap().to_path_buf();
                set_mono_prefix(&mut run, &prefix);
                roots.push(("${MONO}".to_owned(), mono_exe));
            } else {
                // The TestApp's source built for net10.0, for netcoredbg.
                let source_attr = source.to_string_lossy().replace('&', "&amp;");
                write(
                    "src/App/App.csproj",
                    &format!(
                        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><AssemblyName>App</AssemblyName><EnableDefaultCompileItems>false</EnableDefaultCompileItems><Nullable>disable</Nullable><TreatWarningsAsErrors>false</TreatWarningsAsErrors></PropertyGroup><ItemGroup><Compile Include=\"{source_attr}\" /></ItemGroup></Project>"
                    ),
                );
                if recording {
                    run_checked(
                        std::process::Command::new("dotnet")
                            .args(["build", "-c", "Debug", "-nologo", "-v", "q"])
                            .current_dir(tmp.join("src/App")),
                        "dotnet build of the TestApp for net10.0",
                    );
                } else {
                    write("src/App/bin/Debug/net10.0/App.dll", "");
                }
            }
            if attach && recording {
                let mono = real_mono.as_ref().expect("recording under Mono");
                let app = run
                    .w
                    .path("src/App/bin/Debug/net472/Eludite.Debugger.Mono.TestApp.exe");
                let child = std::process::Command::new(&mono.mono)
                    .args([
                        "--debug",
                        &format!(
                            "--debugger-agent=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:{MONO_AGENT_PORT}"
                        ),
                    ])
                    .arg(&app)
                    .envs(mono.env.iter().map(|(k, v)| (k, v)))
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                run.pid = i64::from(child.id());
                run.attached_app = Some(child);
            } else if attach {
                // The replay attaches to a process that exists: this one.
                run.pid = i64::from(std::process::id());
            }
            run.w.open_solution();
        }
        Adapter::Lldb => {
            write("rs/Cargo.toml", RS_CARGO_TOML);
            write("rs/src/main.rs", RS_MAIN);
            let pin = std::fs::read_to_string(repo_root().join("rust-toolchain.toml")).unwrap();
            let channel = pin
                .lines()
                .find_map(|l| l.trim().strip_prefix("channel = "))
                .unwrap()
                .trim_matches('"')
                .to_owned();
            write(
                "rs/rust-toolchain.toml",
                &format!("[toolchain]\nchannel = \"{channel}\"\n"),
            );
            // A sysroot of the formatters only (no rust-src: standard library frames are external code), so the
            // launch's initCommands name the scenario's folder and not the toolchain's.
            let etc = tmp.join("sysroot/lib/rustlib/etc");
            std::fs::create_dir_all(&etc).unwrap();
            // The standard library's generic functions compiled into the program name their source under the
            // toolchain's own sysroot (its rust-src): `${SYSROOT}`.
            if recording {
                let real = eludite_dap::cargo::sysroot(
                    &eludite_dap::cargo::rustc_for(Path::new("cargo")),
                    &tmp.join("rs"),
                )
                .expect("rustc --print sysroot");
                roots.push(("${SYSROOT}".to_owned(), real.clone()));
                for f in ["lldb_lookup.py", "lldb_providers.py", "rust_types.py"] {
                    let from = eludite_dap::cargo::formatters_dir(&real).join(f);
                    if from.is_file() {
                        std::fs::copy(from, etc.join(f)).unwrap();
                    }
                }
                run_checked(
                    std::process::Command::new("cargo")
                        .args(["build", "--quiet"])
                        .current_dir(tmp.join("rs"))
                        .env("CARGO_PROFILE_DEV_DEBUG", "2")
                        .env_remove("CARGO_TARGET_DIR")
                        .stdin(std::process::Stdio::null()),
                    "cargo build of the conformance program",
                );
                if attach {
                    run.attached_app = Some(spawn_waiting(&tmp.join(format!(
                        "rs/target/debug/{RS_PACKAGE}{}",
                        std::env::consts::EXE_SUFFIX
                    ))));
                    run.pid = i64::from(run.attached_app.as_ref().unwrap().id());
                }
            } else {
                if attach {
                    // The replay attaches to a process that exists: this one.
                    run.pid = i64::from(std::process::id());
                }
                roots.push(("${SYSROOT}".to_owned(), tmp.join("rust-sysroot")));
                std::fs::write(etc.join("lldb_lookup.py"), "").unwrap();
                write(
                    &format!(
                        "rs/target/debug/{RS_PACKAGE}{}",
                        std::env::consts::EXE_SUFFIX
                    ),
                    "",
                );
            }
            let sysroot = tmp.join("sysroot");
            run.w.shell.update(&mut run.w.vcx, |s, _| {
                s.debug.native = NativeSetup {
                    lldb: eludite_dap::discovery::LldbSearch::default(),
                    formatters: true,
                    sysroot: Some(sysroot),
                };
            });
            run.source = tmp.join("rs/src/main.rs");
            run.text = RS_MAIN.to_owned();
            let rs = tmp.join("rs").to_string_lossy().into_owned();
            run.cmd(
                eludite_commands::workspace::WORKSPACE_OPEN_FOLDER,
                json!({ "path": rs }),
            );
            run.wait("the Cargo workspace", |w| {
                w.shell
                    .read_with(&w.vcx, |s, _| s.cargo_workspace().is_some())
            });
        }
        Adapter::JsDebug => {
            // The corpus web project's script, its map and its TypeScript source, under the page served.
            let web = tmp.join("web");
            let corpus = repo_root().join("corpus/web/minimal-api/wwwroot");
            std::fs::create_dir_all(&web).unwrap();
            for f in ["app.ts", "app.js", "app.js.map"] {
                std::fs::copy(corpus.join(f), web.join(f)).unwrap();
            }
            std::fs::write(web.join("index.html"), JS_PAGE).unwrap();
            run.source = web.join("app.ts");
            run.text = std::fs::read_to_string(&run.source).unwrap();
            run.web = web.clone();
            // The folder first: opening one closes the browser.
            run.cmd(
                eludite_commands::workspace::WORKSPACE_OPEN_FOLDER,
                json!({ "path": tmp.to_string_lossy() }),
            );
            let origin = if recording {
                format!("127.0.0.1:{}", serve_static(web))
            } else {
                super::super::browser_tests::install_page_engine(&run.w);
                REPLAY_ORIGIN.to_owned()
            };
            let opened = run.browser(
                eludite_commands::browser::TAB_OPEN,
                json!({"url": format!("http://{origin}/")}),
            );
            assert_eq!(opened["id"], "t1", "{opened}");
            let bus = run
                .w
                .shell
                .read_with(&run.w.vcx, |s, _| s.debug.browser_bus.clone())
                .expect("the browser");
            let target = bus
                .debug_target(super::super::browser::DebugTab::Id("t1".into()))
                .expect("the tab's debugging endpoint");
            run.tokens = vec![
                // vscode-js-debug names a page's scripts after the origin with a modifier letter colon.
                ("${ORIGIN_NAME}".to_owned(), origin.replace(':', "\u{a789}")),
                ("${ORIGIN}".to_owned(), origin),
                (
                    "${DEVTOOLS}".to_owned(),
                    format!("{}:{}", target.address, target.port),
                ),
                ("${DEVTOOLS_PORT}".to_owned(), target.port.to_string()),
            ];
            if let Some(t) = target.target_id {
                run.tokens.push(("${TARGET}".to_owned(), t));
            }
            if !recording {
                // The pending target ids `startDebugging` named when recording (`pending_targets`).
                for n in 1..=PENDING_TARGETS {
                    run.tokens
                        .push((pending_placeholder(n), format!("PENDING-TARGET-{n}")));
                }
            }
            *tokens_slot.lock().unwrap() = run.tokens.clone();
        }
    }
    *pid_slot.lock().unwrap() = run.pid;
    *roots_slot.lock().unwrap() = roots.clone();
    run.roots = roots;
    run
}

/// Start the Cargo program waiting (`wait`) for lldb-dap to attach to, once it has printed `waiting`. On Linux it
/// runs without address space randomization (`setarch -R`), as lldb-dap launches programs, so the addresses in the
/// recording are the same every time.
fn spawn_waiting(exe: &Path) -> std::process::Child {
    use std::io::BufRead;
    let mut cmd = if cfg!(target_os = "linux") {
        let mut c = std::process::Command::new("setarch");
        c.args([std::env::consts::ARCH, "-R"]).arg(exe);
        c
    } else {
        std::process::Command::new(exe)
    };
    let mut child = cmd
        .arg("wait")
        .env("RUST_BACKTRACE", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
    let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    while out.read_line(&mut line).unwrap() > 0 && !line.contains("waiting") {
        line.clear();
    }
    // Keep its stdout open (and drained) so it never blocks or dies on a write.
    std::thread::spawn(move || std::io::copy(&mut out, &mut std::io::sink()));
    child
}

/// Remove timing fields (`*_ms`) everywhere.
fn strip_timing(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.retain(|k, _| !k.ends_with("_ms"));
            o.values_mut().for_each(strip_timing);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_timing),
        _ => {}
    }
}

fn replace_text(v: &mut Value, from: &str, to: &str) {
    match v {
        Value::String(s) if s.contains(from) => *s = s.replace(from, to),
        Value::Object(o) => o.values_mut().for_each(|x| replace_text(x, from, to)),
        Value::Array(a) => a.iter_mut().for_each(|x| replace_text(x, from, to)),
        _ => {}
    }
}

/// A snapshot as the golden file keeps it: without timing fields; the adapter's description (which names the
/// adapter's binary and the Mono version this machine has: not DAP) as `${ADAPTER}`; an attached process's name,
/// command line and folder (from this machine's process table: not DAP) as `${PROCESS}`; paths and the process id
/// scrubbed by the recorder's rules.
fn normalize(mut snapshot: Value, scrubber: &Scrubber, process: Option<&str>) -> Value {
    strip_timing(&mut snapshot);
    // The goldens are recorded on Linux: the Rust program's name without Windows' `.exe`.
    if !std::env::consts::EXE_SUFFIX.is_empty() {
        let exe = format!("{RS_PACKAGE}{}", std::env::consts::EXE_SUFFIX);
        replace_text(&mut snapshot, &exe, RS_PACKAGE);
    }
    for at in ["/state/session", "/answer/session"] {
        let Some(session) = snapshot.pointer(at).cloned() else {
            continue;
        };
        if let Some(desc) = session["adapter"].as_str().filter(|d| !d.is_empty()) {
            replace_text(&mut snapshot, desc, "${ADAPTER}");
        }
        if session["attached"] == true {
            for k in ["project", "program", "args", "cwd"] {
                if let Some(v) = snapshot.pointer_mut(&format!("{at}/{k}")) {
                    *v = json!("${PROCESS}");
                }
            }
        }
    }
    if let Some(name) = process {
        for at in ["/state/sessions", "/answer/sessions"] {
            if let Some(rows) = snapshot.pointer_mut(at).and_then(Value::as_array_mut) {
                for r in rows {
                    for k in ["name", "project"] {
                        if r[k] == name {
                            r[k] = json!("${PROCESS}");
                        }
                    }
                }
            }
        }
        fn walk(v: &mut Value, name: &str) {
            match v {
                Value::String(s) => {
                    if s.starts_with("Debugging ") {
                        *s = "Debugging ${PROCESS}".into();
                    } else if s.contains(&format!(" {name} (")) {
                        *s = s.replace(&format!(" {name} ("), " ${PROCESS} (");
                    }
                }
                Value::Array(a) => a.iter_mut().for_each(|x| walk(x, name)),
                Value::Object(o) => o.values_mut().for_each(|x| walk(x, name)),
                _ => {}
            }
        }
        walk(&mut snapshot, name);
    }
    scrubber.scrub(&mut snapshot);
    snapshot
}

/// The name this machine's process table gives `pid`.
fn process_name(pid: i64) -> Option<String> {
    let all = eludite_dap::processes::list().ok()?;
    all.into_iter()
        .find(|p| i64::from(p.pid) == pid)
        .map(|p| p.name)
}

/// The golden file's text.
fn golden_text(adapter: Adapter, scenario: &str, steps: &[Value]) -> String {
    let v = json!({
        "adapter": adapter.name(),
        "scenario": scenario,
        "steps": steps,
    });
    serde_json::to_string_pretty(&v).unwrap() + "\n"
}

/// `Err` with the differences between the golden file's steps and these, step by step.
fn compare_golden(golden: &Value, steps: &[Value]) -> Result<(), String> {
    let expected = golden["steps"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for i in 0..expected.len().max(steps.len()) {
        match (expected.get(i), steps.get(i)) {
            (Some(e), Some(a)) if e != a => {
                out.push(format!(
                    "step {} ({}):",
                    i + 1,
                    e["step"].as_str().unwrap_or("?")
                ));
                for d in record::differences(e, a, 12) {
                    out.push(format!("  {d}"));
                }
            }
            (Some(e), None) => out.push(format!("step {} ({}): missing", i + 1, e["step"])),
            (None, Some(a)) => out.push(format!(
                "step {} ({}): not in the golden file",
                i + 1,
                a["step"]
            )),
            _ => {}
        }
    }
    if out.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the shell's answers differ from the golden file (REGOLDEN=1 rewrites it from the replay):\n{}",
            out.join("\n")
        ))
    }
}

fn paths(dir: &Path, adapter: Adapter, scenario: &str) -> (PathBuf, PathBuf) {
    let base = dir.join(adapter.name());
    (
        base.join(format!("{scenario}.dap.json")),
        base.join(format!("{scenario}.golden.json")),
    )
}

/// Replay `recording` through a fresh shell and return the normalized snapshots and the time it took.
fn replay_scenario(
    cx: &mut TestAppContext,
    adapter: Adapter,
    scenario: &str,
    recording: Recording,
    more: Vec<Recording>,
) -> (Vec<Value>, Duration) {
    let clock = Instant::now();
    let connections = 1 + more.len();
    let mut run = start(cx, adapter, scenario, Source::Replay { recording, more });
    let snapshots = run.run(&steps(adapter, scenario));
    let process = (scenario == "attach-detach")
        .then(|| process_name(run.pid))
        .flatten();
    let handle = run
        .replay
        .lock()
        .unwrap()
        .clone()
        .expect("the session connected");
    handle.check().unwrap_or_else(|e| panic!("{e}"));
    let replays = run.replays.lock().unwrap().clone();
    for h in &replays {
        h.check().unwrap_or_else(|e| panic!("{e}"));
    }
    if adapter == Adapter::JsDebug {
        assert_eq!(
            replays.len(),
            connections,
            "the shell opened {} of the recording's {connections} connections",
            replays.len()
        );
    }
    let mut scrubber = Scrubber::new(&run.roots);
    // The attached process is this test's own (or the recording's app): scrubbed in text whatever its id.
    scrubber.add_own_pid(run.pid);
    for (placeholder, text) in &run.tokens {
        scrubber.add_token(placeholder, text);
    }
    let out = snapshots
        .into_iter()
        .map(|s| normalize(s, &scrubber, process.as_deref()))
        .collect();
    let took = clock.elapsed();
    drop(run);
    (out, took)
}

/// Record `scenario` against the real adapter into `dir`: the recording (and the later connections', vscode-js-debug's
/// child sessions) and the golden file of the live snapshots.
fn record_scenario(
    cx: &mut TestAppContext,
    adapter: Adapter,
    scenario: &str,
    real: Real,
    dir: &Path,
) -> (Recording, Vec<Recording>, Vec<Value>) {
    let (rec_path, golden_path) = paths(dir, adapter, scenario);
    let clock = Instant::now();
    let mut run = start(
        cx,
        adapter,
        scenario,
        Source::Record {
            real,
            path: rec_path.clone(),
        },
    );
    let process = (scenario == "attach-detach")
        .then(|| process_name(run.pid))
        .flatten();
    let snapshots = run.run(&steps(adapter, scenario));
    let handle = run
        .record
        .lock()
        .unwrap()
        .clone()
        .expect("the session connected");
    // The session has ended; give the adapter a moment to close its side.
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.ended().is_none() && Instant::now() < deadline {
        run.w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(app) = run.attached_app.as_mut() {
        // Detached: the TestApp runs on to its end (the Cargo program waits until it is killed).
        let deadline = Instant::now() + Duration::from_secs(20);
        while adapter == Adapter::Mono
            && app.try_wait().ok().flatten().is_none()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        handle.add_pid(run.pid);
    }
    let mut recording = handle.write().unwrap();
    // The child sessions' connections, written once they ended too.
    let all = run.records.lock().unwrap().clone();
    for h in all.iter().skip(1) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while h.ended().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    // vscode-js-debug's pending target ids are the run's own: scrubbed everywhere.
    for (n, id) in pending_targets(&recording).iter().enumerate() {
        for h in &all {
            h.add_token(&pending_placeholder(n + 1), id);
        }
    }
    if !all.is_empty() {
        recording = handle.write().unwrap();
    }
    let more: Vec<Recording> = all.iter().skip(1).map(|h| h.write().unwrap()).collect();
    if adapter == Adapter::JsDebug {
        // The page's tab and the engine go with the run.
        let closed = run
            .w
            .shell
            .read_with(&run.w.vcx, |s, _| s.browser().shutdown());
        let _ = closed.recv_timeout(Duration::from_secs(10));
    }
    let scrubber = handle.scrubber();
    let mut scrubber_with_roots = scrubber.clone();
    if !scrubber_with_roots.pids().contains(&run.pid) && run.pid != REPLAY_PID {
        scrubber_with_roots.add_own_pid(run.pid);
    }
    let golden: Vec<Value> = snapshots
        .into_iter()
        .map(|s| normalize(s, &scrubber_with_roots, process.as_deref()))
        .collect();
    std::fs::write(&golden_path, golden_text(adapter, scenario, &golden)).unwrap();
    eprintln!(
        "recorded {} ({} messages, {} bytes; {} more connections) and {} in {:.1} s",
        rec_path.display(),
        recording.messages.len(),
        recording.to_text().len(),
        more.len(),
        golden_path.display(),
        clock.elapsed().as_secs_f64()
    );
    drop(run);
    (recording, more, golden)
}

/// One scenario: recorded first when `RECORD_DAP` is set and the adapter is here (then replayed from that fresh
/// recording), then replayed from the corpus and compared with its golden file.
fn conformance(cx: &mut TestAppContext, adapter: Adapter, scenario: &str) {
    if let Some(dir) = env_dir("RECORD_DAP") {
        match find_real(adapter) {
            Ok(real) => {
                let (recording, more, golden) = record_scenario(cx, adapter, scenario, real, &dir);
                for r in std::iter::once(&recording).chain(&more) {
                    assert!(
                        r.to_text().len() < record::MAX_RECORDING_BYTES,
                        "the recording is over 2 MB"
                    );
                }
                let (checked_in, _) = paths(&corpus_dir(), adapter, scenario);
                if env_on("DAP_CORPUS_CHECK") && checked_in != paths(&dir, adapter, scenario).0 {
                    if !checked_in.is_file() && adapter == Adapter::Netcoredbg {
                        eprintln!(
                            "{} is not checked in yet: nothing to compare (check the new recording in)",
                            checked_in.display()
                        );
                    } else {
                        let old = Recording::read(&checked_in)
                            .unwrap_or_else(|e| panic!("{}: {e}", checked_in.display()));
                        let old_more = more_recordings(&checked_in);
                        let compared =
                            record::compare(&old, &recording).and_then(|()| {
                                if old_more.len() != more.len() {
                                    return Err(format!(
                                        "{} connections checked in, {} re-recorded",
                                        old_more.len() + 1,
                                        more.len() + 1
                                    ));
                                }
                                old_more.iter().zip(&more).enumerate().try_for_each(
                                    |(n, (a, b))| {
                                        record::compare(a, b)
                                            .map_err(|e| format!("connection {}: {e}", n + 2))
                                    },
                                )
                            });
                        if let Err(e) = compared {
                            panic!(
                                "{adapter:?} {scenario}: the re-recording differs from the checked-in one (an adapter \
                                 or shell change; re-record with tools/dap-corpus/record.sh and review the diff):\n{e}"
                            );
                        }
                    }
                }
                let (replayed, _) = replay_scenario(cx, adapter, scenario, recording, more);
                let fresh = json!({"steps": golden});
                compare_golden(&fresh, &replayed).unwrap_or_else(|e| {
                    panic!("{adapter:?} {scenario}: the replay of the fresh recording is not what the shell saw live: {e}")
                });
            }
            Err(e) => {
                // `DAP_CORPUS_REQUIRE=mono,lldb`: the CI's re-record check, which must not pass by skipping.
                let required = std::env::var("DAP_CORPUS_REQUIRE").unwrap_or_default();
                assert!(
                    !required.split(',').any(|a| a.trim() == adapter.name()),
                    "{} is required here (DAP_CORPUS_REQUIRE) but not found: {e}",
                    adapter.name()
                );
                eprintln!("not recorded ({}): {e}", adapter.name());
            }
        }
    }
    let (rec_path, golden_path) = paths(&corpus_dir(), adapter, scenario);
    let Ok(recording) = Recording::read(&rec_path) else {
        assert_eq!(
            adapter,
            Adapter::Netcoredbg,
            "{} is missing: record it with tools/dap-corpus/record.sh",
            rec_path.display()
        );
        eprintln!(
            "skipped: {} is not recorded yet (it is recorded where netcoredbg can be downloaded: CI or tools/dap-corpus/record.sh)",
            rec_path.display()
        );
        return;
    };
    let more = more_recordings(&rec_path);
    let (replayed, took) = replay_scenario(cx, adapter, scenario, recording, more);
    eprintln!(
        "timing: replay of {} {scenario}: {:.0} ms",
        adapter.name(),
        took.as_secs_f64() * 1e3
    );
    if env_on("REGOLDEN") {
        std::fs::write(&golden_path, golden_text(adapter, scenario, &replayed)).unwrap();
        eprintln!("rewrote {}", golden_path.display());
        return;
    }
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("{}: {e}", golden_path.display())),
    )
    .unwrap();
    compare_golden(&golden, &replayed).unwrap_or_else(|e| panic!("{adapter:?} {scenario}: {e}"));
    assert_budget(
        &format!("{adapter:?} {scenario}: the replay"),
        took,
        Duration::from_secs(2),
    );
}

#[gpui::test]
fn mono_launch_break_step(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Mono, "launch-break-step");
}

#[gpui::test]
fn mono_attach_detach(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Mono, "attach-detach");
}

#[gpui::test]
fn mono_tracepoints_and_exceptions(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Mono, "tracepoints-and-exceptions");
}

#[gpui::test]
fn mono_run_until_and_trace(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Mono, "run-until-and-trace");
}

#[gpui::test]
fn lldb_launch_break_step(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Lldb, "launch-break-step");
}

#[gpui::test]
fn lldb_pause_and_set_variable(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Lldb, "pause-and-set-variable");
}

#[gpui::test]
fn lldb_attach_detach(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Lldb, "attach-detach");
}

#[gpui::test]
fn lldb_function_breakpoints_and_panic(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Lldb, "function-breakpoints-and-panic");
}

#[gpui::test]
fn netcoredbg_launch_break_step(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Netcoredbg, "launch-break-step");
}

#[gpui::test]
fn netcoredbg_tracepoints_and_exceptions(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Netcoredbg, "tracepoints-and-exceptions");
}

#[gpui::test]
fn netcoredbg_run_until_and_trace(cx: &mut TestAppContext) {
    conformance(cx, Adapter::Netcoredbg, "run-until-and-trace");
}

#[gpui::test]
fn js_debug_attach_break_step(cx: &mut TestAppContext) {
    conformance(cx, Adapter::JsDebug, "attach-break-step");
}

#[gpui::test]
fn js_debug_logpoints_and_exceptions(cx: &mut TestAppContext) {
    conformance(cx, Adapter::JsDebug, "logpoints-and-exceptions");
}

/// A golden file edited by hand (here in memory: Mono's F11 lands in `Twice` with `x = 6`) fails the replay with the
/// step, the path and both values.
#[gpui::test]
fn a_golden_edit_fails_with_a_readable_diff(cx: &mut TestAppContext) {
    let (rec_path, golden_path) = paths(&corpus_dir(), Adapter::Mono, "launch-break-step");
    let recording = Recording::read(&rec_path).unwrap();
    let mut golden: Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).unwrap()).unwrap();
    let (replayed, _) = replay_scenario(
        cx,
        Adapter::Mono,
        "launch-break-step",
        recording,
        Vec::new(),
    );
    compare_golden(&golden, &replayed).unwrap();
    let step = golden["steps"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["step"] == "F11")
        .unwrap();
    let row = &mut golden["steps"][step]["windows"]["locals"][0];
    assert_eq!(*row, "x = 5 : int");
    *row = json!("x = 6 : int");
    let e = compare_golden(&golden, &replayed).unwrap_err();
    assert!(
        e.contains(&format!("step {} (F11):", step + 1))
            && e.contains(".windows.locals[0]: expected \"x = 6 : int\", got \"x = 5 : int\""),
        "{e}"
    );
    assert_eq!(e.lines().count(), 3, "one step, one difference: {e}");
}
