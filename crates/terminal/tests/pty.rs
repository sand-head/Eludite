//! A real PTY running `sh` (and `bash` for the integration), as brief 0041's proving test lists: output, resize,
//! the exit code, the scrollback cap, selection across wrapped lines, links on the screen, OSC 133 marks under bash
//! with the integration script and the heuristic without it, the environment's tool paths, Ctrl+C interrupting a
//! child, bracketed paste, the bell and find in the scrollback. Every wait is for a marker in the output, never a
//! sleep. Unix only: the Windows job runs the unit tests (no `sh` there).
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line, Point};
use eludite_terminal::links::{Target, find_links};
use eludite_terminal::pty::{self, Event, Matched, Options, Terminal, WaitFor};
use eludite_terminal::{ToolPaths, env, integration, profile::ShellKind, view};
use regex::Regex;

const T: Duration = Duration::from_secs(20);

struct Term {
    t: Terminal,
    events: Arc<Mutex<Vec<Event>>>,
    _home: tempfile::TempDir,
}

fn base_env(home: &std::path::Path) -> BTreeMap<String, String> {
    let mut e = env::terminal_env(None, None, &BTreeMap::new());
    e.insert("HOME".into(), home.to_string_lossy().into_owned());
    // A prompt the heuristic knows, whoever runs the tests.
    e.insert("PS1".into(), "$ ".into());
    e.insert("ENV".into(), String::new());
    e
}

fn spawn_with(
    program: &str,
    args: &[&str],
    cols: u16,
    rows: u16,
    scrollback: usize,
    extra: &[(&str, &str)],
) -> Term {
    let home = tempfile::tempdir().unwrap();
    let mut env = base_env(home.path());
    for (k, v) in extra {
        env.insert((*k).into(), (*v).into());
    }
    let events: Arc<Mutex<Vec<Event>>> = Arc::default();
    let sink = events.clone();
    let t = Terminal::spawn(
        Options {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env,
            cwd: Some(home.path().to_path_buf()),
            cols,
            rows,
            scrollback,
        },
        Arc::new(move |e| sink.lock().unwrap().push(e)),
    )
    .unwrap();
    Term {
        t,
        events,
        _home: home,
    }
}

fn sh() -> Term {
    spawn_with("/bin/sh", &[], 80, 24, 1000, &[])
}

impl Term {
    fn wait_for(&self, re: &str, mark: u64) -> pty::WaitResult {
        let r = self.t.wait(
            &WaitFor {
                mark,
                pattern: Some(Regex::new(re).unwrap()),
                ..Default::default()
            },
            T,
            &|| false,
        );
        assert!(
            matches!(r.matched, Matched::Pattern(_)),
            "waiting for {re}: {:?}, screen:\n{}",
            r.matched,
            self.t.screen().text()
        );
        r
    }

    fn prompt(&self, mark: u64) -> pty::WaitResult {
        let r = self.t.wait(
            &WaitFor {
                mark,
                prompt: true,
                ..Default::default()
            },
            T,
            &|| false,
        );
        assert_eq!(
            r.matched,
            Matched::Prompt,
            "screen:\n{}",
            self.t.screen().text()
        );
        r
    }

    /// Type `line` and Enter; the mark before it.
    fn run(&self, line: &str) -> u64 {
        let mark = self.t.mark();
        self.t.write(format!("{line}\r"));
        mark
    }

    fn exit(&self) -> Option<i32> {
        let r = self.t.wait(
            &WaitFor {
                exit: true,
                ..Default::default()
            },
            T,
            &|| false,
        );
        assert_eq!(r.matched, Matched::Exit);
        r.exit_code
    }
}

#[test]
fn echo_round_trips_and_the_screen_shows_it() {
    let s = sh();
    s.prompt(0);
    let mark = s.run("echo hello-$((40 + 2))");
    s.wait_for("hello-42", mark);
    let screen = s.t.screen();
    assert!(screen.text().contains("hello-42"), "{}", screen.text());
    assert_eq!((screen.cols, screen.rows), (80, 24));
    assert!(s.t.is_running());
    assert!(!s.t.integration(), "sh has no integration");
}

#[test]
fn fifty_thousand_lines_are_capped_at_the_scrollback() {
    let s = spawn_with("/bin/sh", &["-c", "seq 1 50000"], 80, 24, 1000, &[]);
    assert_eq!(s.exit(), Some(0));
    let all = s.t.scrollback(100_000);
    let lines: Vec<&str> = all.lines().collect();
    assert!(lines.len() <= 1000 + 24, "{} lines kept", lines.len());
    assert!(lines.len() >= 1000, "{} lines kept", lines.len());
    assert_eq!(*lines.last().unwrap(), "50000");
    assert_ne!(lines[0], "1", "the oldest lines were dropped");
    // The transcript (for agents) keeps more than the grid: the whole run here.
    let (text, cut, _) = s.t.since(0, usize::MAX);
    assert!(!cut);
    assert!(text.starts_with("1\n2\n") && text.trim_end().ends_with("50000"));
}

#[test]
fn resize_changes_the_size_programs_see() {
    let s = sh();
    s.prompt(0);
    let m = s.run("tput cols; stty size");
    let r = s.prompt(m);
    assert!(
        r.text.contains("\n80\n") && r.text.contains("24 80"),
        "{}",
        r.text
    );
    s.t.resize(100, 30);
    assert_eq!(s.t.size(), (100, 30));
    let m = s.run("tput cols; stty size");
    let r = s.prompt(m);
    assert!(
        r.text.contains("\n100\n") && r.text.contains("30 100"),
        "{}",
        r.text
    );
    let screen = s.t.screen();
    assert_eq!((screen.cols, screen.rows), (100, 30));
}

#[test]
fn the_exit_code_is_reported() {
    let s = spawn_with("/bin/sh", &["-c", "echo bye; exit 7"], 80, 24, 100, &[]);
    assert_eq!(s.exit(), Some(7));
    assert_eq!(s.t.exited(), Some(Some(7)));
    assert!(s.events.lock().unwrap().contains(&Event::Exited(Some(7))));
    assert!(s.t.since(0, usize::MAX).0.contains("bye"));
}

#[test]
fn selection_text_joins_wrapped_lines() {
    let s = spawn_with(
        "/bin/sh",
        &[
            "-c",
            "printf 'abcdefghijklmnopqrstuvwxyz\\nnext\\n'; sleep 30",
        ],
        10,
        6,
        100,
        &[],
    );
    s.wait_for("next", 0);
    // a..j on row 0, k..t on row 1 (wrapped), u..z on row 2, then `next`.
    let text = s.t.with_term_blocking(|t| {
        pty::text_between(
            t,
            Point::new(Line(0), Column(0)),
            Point::new(Line(3), Column(3)),
        )
    });
    assert_eq!(text, "abcdefghijklmnopqrstuvwxyz\nnext");
    let part = s.t.with_term_blocking(|t| {
        pty::text_between(
            t,
            Point::new(Line(0), Column(8)),
            Point::new(Line(1), Column(1)),
        )
    });
    assert_eq!(part, "ijkl");
    s.t.close(true);
}

#[test]
fn links_printed_by_the_shell_are_found_on_the_screen() {
    let s = sh();
    s.prompt(0);
    let m = s.run("echo 'src/lib.rs:3:1 and Program.cs(12,5) at https://example.com/x'");
    s.wait_for("(?m)^src/lib.rs", m);
    let screen = s.t.screen();
    let line = screen
        .lines
        .iter()
        .find(|l| l.starts_with("src/lib.rs"))
        .expect("the echoed line");
    let targets: Vec<Target> = find_links(line).into_iter().map(|l| l.target).collect();
    assert_eq!(
        targets,
        [
            Target::Path {
                path: "src/lib.rs".into(),
                line: Some(3),
                column: Some(1)
            },
            Target::Path {
                path: "Program.cs".into(),
                line: Some(12),
                column: Some(5)
            },
            Target::Url("https://example.com/x".into())
        ]
    );
}

/// bash with the integration script, in a home with no startup files.
fn bash() -> Option<Term> {
    bash_with(&[])
}

/// As [`bash`], with more variables.
fn bash_with(more: &[(&str, &str)]) -> Option<Term> {
    let bash = eludite_terminal::profile::which("bash", std::env::var("PATH").ok().as_deref())?;
    let dir = tempfile::tempdir().unwrap();
    integration::install(dir.path()).unwrap();
    let mut args = Vec::new();
    let mut extra = BTreeMap::new();
    assert!(integration::apply(
        ShellKind::Bash,
        dir.path(),
        &mut args,
        &mut extra,
        &|_| None
    ));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut extra: Vec<(&str, &str)> = extra
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    extra.extend_from_slice(more);
    let t = spawn_with(&bash.to_string_lossy(), &args, 80, 24, 1000, &extra);
    std::mem::forget(dir);
    Some(t)
}

#[test]
fn osc_133_marks_give_each_commands_exit_code_under_bash() {
    let Some(b) = bash() else {
        eprintln!("skipped: no bash on PATH");
        return;
    };
    b.prompt(0);
    assert!(b.t.integration(), "bash sent the marks");
    let m = b.run("echo out-$((6 * 7))");
    let r = b.prompt(m);
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(
        r.text.trim(),
        "out-42",
        "the output only, without prompt or command"
    );
    let m = b.run("(exit 3)");
    assert_eq!(b.prompt(m).exit_code, Some(3));
    let m = b.run("false; echo two; ls /nonexistent-eludite");
    let r = b.prompt(m);
    assert_eq!(r.exit_code, Some(2), "ls's code");
    assert!(r.text.contains("two"));
    // The folder through OSC 7.
    let m = b.run("cd /tmp");
    b.prompt(m);
    assert_eq!(b.t.cwd().as_deref(), Some("/tmp"));
    // read since: the commands' output, and the marks behind it.
    let (since, _, _) = b.t.since(m, usize::MAX);
    assert!(since.contains("cd /tmp"));
    assert!(b.t.marks_since(m).len() >= 3);
}

#[test]
fn without_integration_the_prompt_heuristic_waits_for_silence() {
    let s = sh();
    s.prompt(0);
    let m = s.run("printf 'one\\n'; sleep 0.4; printf 'two\\n'");
    let started = Instant::now();
    let r = s.prompt(m);
    assert!(!r.integration);
    assert!(
        r.text.contains("one") && r.text.contains("two"),
        "{}",
        r.text
    );
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "not before the command ended"
    );
    assert_eq!(r.exit_code, None, "unknown without integration");
}

#[test]
fn the_environment_has_the_tool_paths_once() {
    let tools = ToolPaths {
        dirs: vec![
            PathBuf::from("/opt/eludite-test-tools"),
            PathBuf::from("/usr/bin"),
        ],
        vars: BTreeMap::from([(
            "DOTNET_ROOT".to_owned(),
            "/opt/eludite-test-tools".to_owned(),
        )]),
    };
    let base = "/usr/bin:/bin:/usr/bin";
    let e = env::terminal_env(Some(base), Some(&tools), &BTreeMap::new());
    let path = e["PATH"].clone();
    let s = spawn_with(
        "/bin/sh",
        &[
            "-c",
            "echo \"P=$PATH\"; echo \"D=$DOTNET_ROOT\"; echo \"T=$TERM_PROGRAM\"",
        ],
        200,
        10,
        100,
        &[("PATH", &path), ("DOTNET_ROOT", "/opt/eludite-test-tools")],
    );
    s.exit();
    let text = s.t.since(0, usize::MAX).0;
    let p = text
        .lines()
        .find_map(|l| l.strip_prefix("P="))
        .unwrap()
        .to_owned();
    assert_eq!(p, "/opt/eludite-test-tools:/usr/bin:/bin");
    assert!(text.contains("D=/opt/eludite-test-tools"));
    assert!(text.contains("T=Eludite"));
}

#[test]
fn ctrl_c_interrupts_the_foreground_child() {
    let s = sh();
    s.prompt(0);
    s.run("sleep 30");
    let deadline = Instant::now() + T;
    while !s.t.busy() {
        assert!(Instant::now() < deadline, "sleep never ran");
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    let m = s.t.mark();
    s.t.write(vec![3]);
    s.prompt(m);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!s.t.busy());
    let m = s.run("echo status=$?");
    s.wait_for("status=130", m);
}

#[test]
fn bracketed_paste_reaches_an_application_that_asked() {
    let s = spawn_with(
        "/bin/sh",
        &[
            "-c",
            "stty raw -echo; printf '\\033[?2004hready\\r\\n'; head -c 14 | od -An -c; printf 'done\\r\\n'; sleep 30",
        ],
        80,
        10,
        100,
        &[],
    );
    s.wait_for("ready", 0);
    s.t.paste("xy");
    s.wait_for("done", 0);
    let text = s.t.since(0, usize::MAX).0;
    assert!(text.contains("2   0   0   ~   x   y"), "{text}");
    s.t.close(true);
}

#[test]
fn the_bell_is_an_event() {
    let s = spawn_with(
        "/bin/sh",
        &["-c", "printf 'ring\\a'; sleep 30"],
        80,
        10,
        100,
        &[],
    );
    s.wait_for("ring", 0);
    let deadline = Instant::now() + T;
    while !s.events.lock().unwrap().contains(&Event::Bell) {
        assert!(Instant::now() < deadline, "no bell");
        std::thread::sleep(Duration::from_millis(5));
    }
    s.t.close(true);
}

#[test]
fn find_searches_the_scrollback() {
    let s = spawn_with(
        "/bin/sh",
        &[
            "-c",
            "echo first NEEDLE; seq 1 100; echo second needle; echo end",
        ],
        80,
        10,
        1000,
        &[],
    );
    s.exit();
    let found = view::find_in(&s.t, "needle");
    assert_eq!(found.len(), 2);
    let (lines, history) = s.t.all_lines();
    assert!(history > 0, "the first one is in the scrollback");
    assert!(found[0].0 < history);
    assert_eq!(&lines[found[0].0][found[0].1.clone()], "NEEDLE");
}

#[test]
fn the_persons_input_interrupts_a_wait_and_kill_ends_a_busy_terminal() {
    let s = sh();
    s.prompt(0);
    let mark = s.run("sleep 30");
    let t = s.t.clone();
    let waiter = std::thread::spawn(move || {
        t.wait(
            &WaitFor {
                mark,
                prompt: true,
                ..Default::default()
            },
            T,
            &|| false,
        )
    });
    let deadline = Instant::now() + T;
    while s.t.waiting() == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    s.t.interrupt_waits();
    let r = waiter.join().unwrap();
    assert_eq!(r.matched, Matched::Interrupted);
    // The wait may end before bash hands the terminal to `sleep`.
    let deadline = Instant::now() + T;
    while !s.t.busy() {
        assert!(Instant::now() < deadline, "sleep never took the foreground");
        std::thread::sleep(Duration::from_millis(2));
    }
    s.t.close(true);
    assert!(s.exit().is_some_and(|c| c != 0));
}

#[test]
fn clear_keeps_the_prompt_line_and_marks() {
    let s = sh();
    s.prompt(0);
    let m = s.run("seq 1 40");
    s.wait_for("(?m)^40", m);
    s.prompt(m);
    let before = s.t.mark();
    s.t.clear();
    let deadline = Instant::now() + T;
    while s.t.scrollback(1000).contains("\n20") {
        assert!(Instant::now() < deadline, "not cleared");
        std::thread::sleep(Duration::from_millis(5));
    }
    let screen = s.t.screen();
    assert_eq!(screen.cursor.0, 0, "the prompt line is at the top");
    assert!(screen.lines[0].starts_with("$ "));
    let m = s.run("echo after");
    assert!(m >= before);
    s.wait_for("after", m);
}

/// A line redrawn after a mark was taken (a line editor wrapping a long command at the margin writes a lone `\r` and
/// the rest): the transcript's text shrinks, its marks never do, so the wait still sees the prompt after the mark
/// (the bug the Xvfb run found with a long prompt).
#[test]
fn a_line_redrawn_after_a_mark_keeps_the_marks_growing() {
    let s = spawn_with(
        "/bin/sh",
        &[
            "-c",
            "stty -echo; printf 'aaaaaaaaaaaaaaaaaaaa'; read x; \
             printf '\\rb\\n\\033]133;C\\007out\\n\\033]133;D;0\\007\\033]133;A\\007$ \\033]133;B\\007'; sleep 30",
        ],
        80,
        10,
        100,
        &[],
    );
    s.wait_for("aaaa", 0);
    let m = s.t.mark();
    s.t.write("\r");
    let r = s.prompt(m);
    assert!(r.integration);
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.text, "out\n");
    let marks: Vec<u64> = s.t.marks_since(0).iter().map(|k| k.at).collect();
    assert!(marks.iter().all(|&at| at >= m), "{marks:?} after {m}");
    s.t.close(true);
}
