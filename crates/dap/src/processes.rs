//! The processes a debugger could attach to (brief 0027): this machine's process table with each process's command
//! line and runtime, read from `/proc` on Linux, `ps` on macOS and `tasklist` on Windows. Reading it blocks on the file
//! system or a child process, so callers run it on a worker thread, never the UI thread.
//!
//! - **Runtime** ([`runtime_of`]): `dotnet` running a `.dll` is [`Runtime::Dotnet`]; a program run by `mono` (or
//!   `mono-sgen`) is [`Runtime::Mono`]; anything else whose command line could be read is [`Runtime::Native`]. A
//!   process whose command line cannot be read is [`Runtime::Unknown`]. `tasklist` gives no command lines, so on Windows
//!   the runtime is judged by the image name alone (`dotnet.exe`, `mono.exe`; [`Runtime::Netfx`] is not detected yet).
//! - **Launched by Eludite** ([`launched_set`], [`is_launched`]): a process whose id is one of the roots (the programs
//!   the shell started) or whose parent chain reaches one.
//! - **Mono's debugger agent** ([`mono_agent`]): the `address` of `--debugger-agent=...,server=y,...` on a Mono
//!   program's command line, which `eludite-dbg-mono` attaches to (Mono has no late attach).

use std::collections::{HashMap, HashSet};

/// What runs a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Runtime {
    /// `dotnet <app>.dll`: .NET (Core), netcoredbg attaches by process id.
    Dotnet,
    /// A .NET Framework program on Windows (eludite-dbg-netfx, brief 0004).
    Netfx,
    /// Run by Mono: eludite-dbg-mono attaches to its debugger agent.
    Mono,
    /// Anything else: lldb-dap attaches by process id.
    Native,
    /// The command line could not be read.
    Unknown,
}

impl Runtime {
    /// `debug-processes.output.json`'s `runtime`.
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Dotnet => "dotnet",
            Runtime::Netfx => "netfx",
            Runtime::Mono => "mono",
            Runtime::Native => "native",
            Runtime::Unknown => "unknown",
        }
    }
}

/// One process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    /// The program's name: its command line's first argument without the folder, else the process table's name.
    pub name: String,
    /// The arguments, the first being the program; empty when they cannot be read.
    pub argv: Vec<String>,
    pub runtime: Runtime,
}

impl ProcessInfo {
    /// The command line, arguments separated by spaces (one with a space or nothing in double quotes).
    pub fn command_line(&self) -> String {
        join_command_line(&self.argv)
    }
}

/// `argv` as one line: arguments separated by spaces, one containing a space (or empty) in double quotes.
pub fn join_command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() || a.contains(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` cut at `max` characters, ending with `…` when cut.
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
    s.push('\u{2026}');
    s
}

/// The file name of `path` (either separator), without a Windows `.exe`.
fn base_name(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
}

/// What runs a process with command line `argv` (and process-table name `name`).
pub fn runtime_of(name: &str, argv: &[String]) -> Runtime {
    let Some(first) = argv.first() else {
        return match base_name(name).to_ascii_lowercase().as_str() {
            "" => Runtime::Unknown,
            "dotnet" => Runtime::Dotnet,
            n if n.starts_with("mono") => Runtime::Mono,
            _ => Runtime::Unknown,
        };
    };
    let program = base_name(first).to_ascii_lowercase();
    let dll = argv
        .iter()
        .skip(1)
        .any(|a| a.to_ascii_lowercase().ends_with(".dll"));
    if program == "dotnet" && dll {
        return Runtime::Dotnet;
    }
    // A script or wrapper that runs `dotnet <app>.dll` (a shell running `dotnet`, `dotnet exec`) has both in its line.
    if dll
        && argv
            .iter()
            .any(|a| base_name(a).eq_ignore_ascii_case("dotnet"))
    {
        return Runtime::Dotnet;
    }
    if program == "mono" || program.starts_with("mono-sgen") {
        return Runtime::Mono;
    }
    Runtime::Native
}

/// Where a Mono program's debugger agent listens: `--debugger-agent=...,address=HOST:PORT,...` with `server=y`.
/// `HOST` defaults to `127.0.0.1` when the address is only a port.
pub fn mono_agent(argv: &[String]) -> Option<(String, u16)> {
    let options = argv
        .iter()
        .find_map(|a| a.strip_prefix("--debugger-agent="))?;
    let mut server = false;
    let mut address = None;
    for part in options.split(',') {
        match part.split_once('=') {
            Some(("server", v)) => server = v.eq_ignore_ascii_case("y"),
            Some(("address", v)) => address = Some(v.to_owned()),
            _ => {}
        }
    }
    if !server {
        return None;
    }
    let address = address?;
    let (host, port) = match address.rsplit_once(':') {
        Some((h, p)) => (if h.is_empty() { "127.0.0.1" } else { h }, p),
        None => ("127.0.0.1", address.as_str()),
    };
    Some((host.to_owned(), port.parse().ok()?))
}

/// Every process visible to this user. Blocks (reads `/proc`, or runs `ps` or `tasklist`).
pub fn list() -> Result<Vec<ProcessInfo>, String> {
    #[cfg(target_os = "linux")]
    {
        list_proc(std::path::Path::new("/proc"))
    }
    #[cfg(target_os = "macos")]
    {
        list_ps()
    }
    #[cfg(windows)]
    {
        list_tasklist()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Err("listing processes is not supported on this platform".into())
    }
}

/// The processes under a `/proc` file system.
#[cfg(target_os = "linux")]
pub fn list_proc(proc_dir: &std::path::Path) -> Result<Vec<ProcessInfo>, String> {
    let entries =
        std::fs::read_dir(proc_dir).map_err(|e| format!("{}: {e}", proc_dir.display()))?;
    let mut out = Vec::new();
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(p) = read_proc(proc_dir, pid) {
            out.push(p);
        }
    }
    Ok(out)
}

/// One process from `/proc/<pid>`: `None` when it is gone or is a kernel thread (no command line and no parent but
/// `kthreadd`).
#[cfg(target_os = "linux")]
fn read_proc(proc_dir: &std::path::Path, pid: u32) -> Option<ProcessInfo> {
    let dir = proc_dir.join(pid.to_string());
    let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
    let (comm, parent) = parse_stat(&stat)?;
    let argv: Vec<String> = match std::fs::read(dir.join("cmdline")) {
        Ok(bytes) => bytes
            .split(|b| *b == 0)
            .filter(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    // Kernel threads have an empty command line and are children of kthreadd (2) or are it.
    if argv.is_empty() && (pid == 2 || parent == Some(2)) {
        return None;
    }
    let name = argv
        .first()
        .map(|a| base_name(a).to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| comm.clone());
    let runtime = if argv.is_empty() {
        Runtime::Unknown
    } else {
        runtime_of(&comm, &argv)
    };
    Some(ProcessInfo {
        pid,
        parent,
        name,
        argv,
        runtime,
    })
}

/// `/proc/<pid>/stat`'s `comm` and parent id (`ppid`, the field after the state that follows the closing `)`).
pub fn parse_stat(stat: &str) -> Option<(String, Option<u32>)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?.to_owned();
    let mut rest = stat.get(close + 1..)?.split_whitespace();
    let _state = rest.next()?;
    let ppid: u32 = rest.next()?.parse().ok()?;
    Some((comm, (ppid != 0).then_some(ppid)))
}

/// The processes `ps -axww -o pid=,ppid=,args=` lists (macOS).
#[cfg(target_os = "macos")]
fn list_ps() -> Result<Vec<ProcessInfo>, String> {
    let out = std::process::Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,args="])
        .output()
        .map_err(|e| format!("ps: {e}"))?;
    if !out.status.success() {
        return Err(format!("ps exited with {}", out.status));
    }
    Ok(parse_ps(&String::from_utf8_lossy(&out.stdout)))
}

/// `ps -o pid=,ppid=,args=` output: each line `PID PPID ARGS...`. Arguments are split at spaces (ps does not quote
/// them).
pub fn parse_ps(text: &str) -> Vec<ProcessInfo> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pid: u32 = it.next()?.parse().ok()?;
            let ppid: u32 = it.next()?.parse().ok()?;
            let argv: Vec<String> = it.map(str::to_owned).collect();
            let name = argv
                .first()
                .map(|a| base_name(a).to_owned())
                .unwrap_or_default();
            let runtime = if argv.is_empty() {
                Runtime::Unknown
            } else {
                runtime_of(&name, &argv)
            };
            Some(ProcessInfo {
                pid,
                parent: (ppid != 0).then_some(ppid),
                name,
                argv,
                runtime,
            })
        })
        .collect()
}

/// The processes `tasklist /FO CSV /NH` lists (Windows): image names and ids, no parents or command lines.
#[cfg(windows)]
fn list_tasklist() -> Result<Vec<ProcessInfo>, String> {
    let out = std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
        .map_err(|e| format!("tasklist: {e}"))?;
    if !out.status.success() {
        return Err(format!("tasklist exited with {}", out.status));
    }
    Ok(parse_tasklist(&String::from_utf8_lossy(&out.stdout)))
}

/// `tasklist /FO CSV /NH` output: `"Image Name","PID",...` per line.
pub fn parse_tasklist(text: &str) -> Vec<ProcessInfo> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line
                .split("\",\"")
                .map(|f| f.trim_matches(|c| c == '"' || c == '\r'))
                .collect();
            let image = fields.first()?.to_string();
            let pid: u32 = fields.get(1)?.parse().ok()?;
            let runtime = runtime_of(&image, &[]);
            Some(ProcessInfo {
                pid,
                parent: None,
                name: base_name(&image).to_owned(),
                argv: Vec::new(),
                runtime,
            })
        })
        .collect()
}

/// The ids of `roots` and of every process whose parent chain in `all` reaches one of them.
pub fn launched_set(roots: &[u32], all: &[ProcessInfo]) -> HashSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in all {
        if let Some(parent) = p.parent {
            children.entry(parent).or_default().push(p.pid);
        }
    }
    let mut out: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = roots.to_vec();
    while let Some(pid) = stack.pop() {
        if out.insert(pid)
            && let Some(c) = children.get(&pid)
        {
            stack.extend(c);
        }
    }
    out
}

/// The parent of process `pid`, read from the process table (`/proc/<pid>/stat` on Linux, `ps` on macOS).
pub fn parent_of(pid: u32) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat(&stat)?.1
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .ok()
            .filter(|p| *p != 0)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Whether `pid` is one of `roots` or a descendant of one: its parent chain is walked (at most 64 steps). Cheap: one
/// small read per ancestor.
pub fn is_launched(pid: u32, roots: &[u32]) -> bool {
    let mut at = pid;
    for _ in 0..64 {
        if roots.contains(&at) {
            return true;
        }
        match parent_of(at) {
            Some(p) if p != at && p > 1 => at = p,
            _ => return false,
        }
    }
    false
}

/// The working directory of process `pid`, when the process table says (Linux: `/proc/<pid>/cwd`).
pub fn cwd_of(pid: u32) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// Whether process `pid` exists.
pub fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // A zombie has exited: only its entry is left.
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
            s.rfind(')')
                .and_then(|i| s.get(i + 1..))
                .and_then(|r| r.split_whitespace().next())
                != Some("Z")
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        list().is_ok_and(|all| all.iter().any(|p| p.pid == pid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn runtimes_from_command_lines() {
        assert_eq!(
            runtime_of(
                "dotnet",
                &argv(&["/usr/bin/dotnet", "bin/Debug/net10.0/App.dll"])
            ),
            Runtime::Dotnet
        );
        assert_eq!(
            runtime_of("dotnet", &argv(&["dotnet", "build"])),
            Runtime::Native,
            "dotnet without a .dll is the SDK, not an app"
        );
        assert_eq!(
            runtime_of("sh", &argv(&["/bin/sh", "/tmp/x/dotnet", "/s/App.dll"])),
            Runtime::Dotnet
        );
        assert_eq!(
            runtime_of("mono", &argv(&["mono", "--debug", "TestApp.exe"])),
            Runtime::Mono
        );
        assert_eq!(
            runtime_of("mono-sgen", &argv(&["/usr/bin/mono-sgen", "a.exe"])),
            Runtime::Mono
        );
        assert_eq!(runtime_of("sleep", &argv(&["sleep", "5"])), Runtime::Native);
        assert_eq!(runtime_of("", &[]), Runtime::Unknown);
        assert_eq!(runtime_of("dotnet.exe", &[]), Runtime::Dotnet);
        assert_eq!(runtime_of("svchost.exe", &[]), Runtime::Unknown);
        assert_eq!(Runtime::Dotnet.as_str(), "dotnet");
    }

    #[test]
    fn command_lines_join_and_cut() {
        assert_eq!(
            join_command_line(&argv(&["a", "b c", ""])),
            "a \"b c\" \"\""
        );
        assert_eq!(cut("abcdef", 4), "abc\u{2026}");
        assert_eq!(cut("abc", 4), "abc");
        assert_eq!(cut(&"x".repeat(600), 500).chars().count(), 500);
    }

    #[test]
    fn mono_agents_are_read_from_the_command_line() {
        let line = argv(&[
            "mono",
            "--debug",
            "--debugger-agent=transport=dt_socket,server=y,address=127.0.0.1:55555,suspend=n",
            "TestApp.exe",
        ]);
        assert_eq!(mono_agent(&line), Some(("127.0.0.1".into(), 55555)));
        let client = argv(&[
            "mono",
            "--debugger-agent=transport=dt_socket,address=127.0.0.1:4000",
            "a.exe",
        ]);
        assert_eq!(
            mono_agent(&client),
            None,
            "a client agent cannot be attached"
        );
        assert_eq!(
            mono_agent(&argv(&[
                "mono",
                "--debugger-agent=server=y,address=4711",
                "a.exe"
            ])),
            Some(("127.0.0.1".into(), 4711))
        );
        assert_eq!(mono_agent(&argv(&["mono", "a.exe"])), None);
    }

    #[test]
    fn stat_ps_and_tasklist_parse() {
        assert_eq!(
            parse_stat("42 (my (odd) name) S 7 42 42 0 -1"),
            Some(("my (odd) name".into(), Some(7)))
        );
        assert_eq!(parse_stat("1 (init) S 0 1"), Some(("init".into(), None)));
        let ps = parse_ps("  10     1 /usr/bin/dotnet /a/App.dll\n  11    10 sleep 5\n bad\n");
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].name, "dotnet");
        assert_eq!(ps[0].runtime, Runtime::Dotnet);
        assert_eq!(ps[1].parent, Some(10));
        let tl = parse_tasklist(
            "\"System Idle Process\",\"0\",\"Services\",\"0\",\"8 K\"\r\n\"dotnet.exe\",\"4242\",\"Console\",\"1\",\"40,000 K\"\r\n",
        );
        assert_eq!(tl.len(), 2);
        assert_eq!(tl[1].pid, 4242);
        assert_eq!(tl[1].name, "dotnet");
        assert_eq!(tl[1].runtime, Runtime::Dotnet);
    }

    #[test]
    fn launched_sets_follow_parents() {
        let p = |pid, parent| ProcessInfo {
            pid,
            parent,
            name: String::new(),
            argv: Vec::new(),
            runtime: Runtime::Native,
        };
        let all = vec![
            p(1, None),
            p(10, Some(1)),
            p(11, Some(10)),
            p(12, Some(11)),
            p(20, Some(1)),
        ];
        let set = launched_set(&[10], &all);
        assert!(set.contains(&10) && set.contains(&11) && set.contains(&12));
        assert!(!set.contains(&20) && !set.contains(&1));
    }
}
