//! Console programs in the integrated terminal (brief 0074), as Visual Studio runs them in a console window.
//!
//! Start Without Debugging on a console project (`OutputType` `Exe`, not a web project) runs the program in a new
//! Terminal window tab on a real PTY (brief 0041), so its input, colors, `Console.ReadKey` and prompts without a
//! newline work. [`wrap`] makes the command the tab runs: the program, then Visual Studio's closing lines
//! (`<program> (process <pid>) exited with code <n>.` and `Press any key to close this window . . .`) and one key's
//! wait. The launch thread watches the tab's transcript for the exit line ([`exit_code`], [`wait_for_exit`]) and ends
//! the session with its code. The setting `debug.console` chooses: `integratedTerminal` (the default) or `output`
//! (the Output window's Debug pane, as before). Web projects and `WinExe` programs keep the Output window.
//!
//! The same tab serves DAP's `runInTerminal` reverse request ([`run_in_terminal_answer`]): an adapter that asks
//! (lldb-dap, vscode-js-debug) gets its command started in a tab and the process id back. netcoredbg 3.2 never asks:
//! its vscode protocol has no `console` launch argument and keeps the debuggee's stdin to itself.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use eludite_dap::launch::Platform;
use eludite_terminal::Terminal;
use serde_json::{Value, json};

use super::terminal::TerminalService;

/// `debug.console`'s values.
pub const INTEGRATED_TERMINAL: &str = "integratedTerminal";
pub const OUTPUT: &str = "output";

/// The words of Visual Studio's console after the program exits.
pub const EXITED_WITH_CODE: &str = "exited with code ";
pub const PRESS_ANY_KEY: &str = "Press any key to close this window . . .";

/// How often the launch thread looks at the tab for the exit line.
const POLL: Duration = Duration::from_millis(20);

/// The command a tab runs for `program` with `args` on `platform`: the program, then the exit lines and one key.
/// Unix: `sh -c <script> sh <program> <args>...` (the arguments reach the program untouched); Windows: `cmd.exe`
/// with delayed expansion for the exit code and `pause`.
pub fn wrap(program: &str, args: &[String], platform: Platform) -> (String, Vec<String>) {
    match platform {
        Platform::Windows => {
            let quote = |a: &str| {
                if a.is_empty() || a.contains([' ', '\t', '&', '(', ')', '^', '|', '<', '>']) {
                    format!("\"{}\"", a.replace('"', "\"\""))
                } else {
                    a.to_owned()
                }
            };
            let mut line = quote(program);
            for a in args {
                line.push(' ');
                line.push_str(&quote(a));
            }
            let script = format!(
                "{line} & echo. & echo {} {EXITED_WITH_CODE}!errorlevel!. & echo {} & pause >nul",
                program, PRESS_ANY_KEY
            );
            (
                "cmd.exe".into(),
                vec!["/d".into(), "/v:on".into(), "/c".into(), script],
            )
        }
        Platform::Linux | Platform::MacOs => {
            let script = format!(
                "\"$@\"; c=$?; printf '\\n%s (process %s) {EXITED_WITH_CODE}%s.\\n{PRESS_ANY_KEY}' \"$1\" \"$$\" \"$c\"; \
                 stty raw -echo 2>/dev/null; dd bs=1 count=1 >/dev/null 2>&1; stty sane 2>/dev/null; exit $c"
            );
            let mut a = vec!["-c".into(), script, "sh".into(), program.to_owned()];
            a.extend(args.iter().cloned());
            ("sh".into(), a)
        }
    }
}

/// The exit code in a tab's text: the last `... exited with code <n>.` line.
pub fn exit_code(text: &str) -> Option<i32> {
    text.lines().rev().find_map(|l| {
        let rest = &l[l.find(EXITED_WITH_CODE)? + EXITED_WITH_CODE.len()..];
        rest.trim_end().trim_end_matches('.').trim().parse().ok()
    })
}

/// A program running in a tab.
#[derive(Clone)]
pub struct TerminalRun {
    pub terminal: String,
    pub pid: u32,
    pub handle: Terminal,
}

/// Start `program` with `args` in a new tab named `name`, wrapped with the exit lines ([`wrap`]).
pub fn start(
    service: &TerminalService,
    name: &str,
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
    platform: Platform,
) -> Result<TerminalRun, String> {
    let (cmd, wrapped) = wrap(program, args, platform);
    service
        .open_program(name, &cmd, &wrapped, cwd, env, true)
        .map(|(terminal, pid, handle)| TerminalRun {
            terminal,
            pid,
            handle,
        })
        .map_err(|e| e.to_string())
}

/// Wait until the program in `run` exits: its code from the exit line, or `None` when the tab ended without one
/// (stopped, or the shell could not start). Polls on the calling thread (the launch thread).
pub fn wait_for_exit(run: &TerminalRun) -> Option<i32> {
    let mut mark = 0;
    let mut text = String::new();
    loop {
        let (more, _, end) = run.handle.since(mark, 64 * 1024);
        if !more.is_empty() {
            text.push_str(&more);
            mark = end;
            if let Some(c) = exit_code(&text) {
                return Some(c);
            }
            // Keep only the tail: the exit line is the last thing the tab prints.
            if text.len() > 16 * 1024 {
                text.drain(..text.len() - 4 * 1024);
            }
        }
        if run.handle.exited().is_some() {
            let (rest, _, _) = run.handle.since(mark, 64 * 1024);
            text.push_str(&rest);
            return exit_code(&text);
        }
        std::thread::sleep(POLL);
    }
}

/// Answer DAP's `runInTerminal` (`args`, `cwd`, `env`, `title`): start `args` in a new tab, unwrapped (the adapter
/// owns the process), and answer `{processId}`.
pub fn run_in_terminal_answer(
    service: &Arc<TerminalService>,
    arguments: &Value,
) -> Result<Value, String> {
    let args: Vec<String> = arguments["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let Some((program, rest)) = args.split_first() else {
        return Err("runInTerminal: no command".into());
    };
    let cwd = arguments["cwd"]
        .as_str()
        .filter(|c| !c.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let env: BTreeMap<String, String> = arguments["env"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let title = arguments["title"]
        .as_str()
        .filter(|t| !t.is_empty())
        .unwrap_or("Debug Console");
    let (_, pid, _) = service
        .open_program(title, program, rest, &cwd, &env, true)
        .map_err(|e| e.to_string())?;
    Ok(json!({ "processId": pid }))
}

/// Asks eludite-host what `dotnet run` would run (`eludite/project/runTarget`) from the launch thread: the project, its
/// configuration and framework; `None` when the host does not answer (no solution, an older host, a failure or
/// [`RUN_TARGET_WAIT`]), and the launch then reads the project file.
pub type RunTargetAsk = Arc<
    dyn Fn(&Path, &str, Option<&str>) -> Option<eludite_dap::launch::EvaluatedTarget> + Send + Sync,
>;

/// The longest the launch thread waits for the evaluation (a cold evaluation loads the SDK's imports once).
pub const RUN_TARGET_WAIT: Duration = Duration::from_secs(10);

/// [`RunTargetAsk`] over the host session.
pub fn host_run_target(session: super::session::ServerSession) -> RunTargetAsk {
    Arc::new(move |project, configuration, framework| {
        let params = eludite_lsp::host::RunTargetParams {
            project: project.to_string_lossy().into_owned(),
            configuration: Some(configuration.to_owned()).filter(|c| !c.is_empty()),
            platform: None,
            framework: framework.map(str::to_owned),
        };
        let (_, rx) = session.request::<eludite_lsp::host::ProjectRunTarget>(params);
        let (tx, got) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("run-target".into())
            .spawn(move || {
                let _ = tx.send(futures::executor::block_on(rx));
            })
            .ok()?;
        let started = std::time::Instant::now();
        let reply = got.recv_timeout(RUN_TARGET_WAIT).ok()?.ok()?;
        super::documents::trace(format_args!(
            "debug launch: runTarget answered in {:.2} ms",
            started.elapsed().as_secs_f64() * 1e3
        ));
        reply
            .result
            .ok()
            .map(|r| eludite_dap::launch::EvaluatedTarget::from_host(&r))
    })
}

/// How long a test waits at most for a tab's text (a hang bound, not a budget).
#[cfg(test)]
pub fn wait_text(run: &Terminal, needle: &str, bound: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < bound {
        if run.since(0, 1 << 20).0.contains(needle) {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn the_exit_line_gives_the_code() {
        assert_eq!(
            exit_code("Hello\n/p/App (process 12) exited with code 3.\nPress any key"),
            Some(3)
        );
        assert_eq!(exit_code("exited with code -1."), Some(-1));
        assert_eq!(exit_code("still running"), None);
    }

    #[test]
    fn the_wrapper_keeps_arguments_whole() {
        let (cmd, args) = wrap("/p/App", &["a b".into(), "$HOME".into()], Platform::Linux);
        assert_eq!(cmd, "sh");
        assert_eq!(args[0], "-c");
        assert_eq!(&args[2..], ["sh", "/p/App", "a b", "$HOME"]);
        assert!(args[1].contains(PRESS_ANY_KEY));
        let (cmd, args) = wrap("C:\\p\\App.exe", &["a b".into()], Platform::Windows);
        assert_eq!(cmd, "cmd.exe");
        assert!(args[3].starts_with("C:\\p\\App.exe \"a b\" & "));
        assert!(args[3].contains("!errorlevel!"));
    }
}
