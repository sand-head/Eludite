//! A terminal: a shell on a PTY (`alacritty_terminal::tty`: Unix PTYs, ConPTY on Windows), its output parsed into
//! `alacritty_terminal`'s grid on the terminal's own I/O thread, and the plain-text transcript and shell
//! integration marks agents read.
//!
//! - **Threads.** One I/O thread per terminal waits on the PTY and its child (with `polling`, as
//!   `alacritty_terminal`'s own event loop does), writes queued input, parses output in chunks of at most
//!   [`PARSE_CHUNK`] bytes under the grid's lock, and applies resizes. Nothing here blocks the caller: writes,
//!   resizes and closing are messages to that thread.
//! - **One update per frame.** After a chunk the thread sets the terminal dirty and raises [`Event::Changed`] only
//!   when it was clean; the view clears it when it takes the next snapshot, so a burst of output costs one
//!   notification until the view has drawn.
//! - **Agents.** [`Terminal::wait`] blocks the calling (agent) thread until a prompt, a pattern or the exit, or until
//!   the person interrupts ([`Terminal::interrupt_waits`]).

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event as TermEvent, EventListener, OnResize, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::tty::{self, EventedPty, EventedReadWrite};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use polling::{Event as PollEvent, Events, PollMode, Poller};
use regex::Regex;

use crate::scan::{MarkKind, Scanner, Segment};
use crate::text::Transcript;

/// The most bytes parsed under one hold of the grid's lock, so a view taking a snapshot waits at most that long.
pub const PARSE_CHUNK: usize = 16 * 1024;
/// The prompt heuristic's silence: a prompt-like last line after this long without output is a prompt.
pub const PROMPT_SILENCE: Duration = Duration::from_millis(300);
/// The poller key alacritty_terminal gives the child's exit notifications on every platform.
const CHILD_TOKEN: usize = 1;
/// The most marks kept.
const MAX_MARKS: usize = 4096;

/// How to start a terminal.
#[derive(Debug, Clone)]
pub struct Options {
    pub program: String,
    pub args: Vec<String>,
    /// Variables added to Eludite's environment.
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
    /// Lines of scrollback.
    pub scrollback: usize,
}

/// What a terminal tells its owner, from its I/O thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The grid changed (raised once until [`Terminal::take_dirty`]).
    Changed,
    Bell,
    Title(String),
    /// The shell ended, with its exit code when it had one.
    Exited(Option<i32>),
}

/// Where a terminal's events go.
pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

/// A shell integration mark at a position of the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    pub at: u64,
}

/// What a wait waits for.
#[derive(Debug, Clone, Default)]
pub struct WaitFor {
    /// Only what comes after this mark.
    pub mark: u64,
    pub prompt: bool,
    pub pattern: Option<Regex>,
    pub exit: bool,
}

/// What ended a wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matched {
    Prompt,
    Pattern(String),
    Exit,
    Timeout,
    /// The person typed into the terminal.
    Interrupted,
}

/// A wait's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitResult {
    pub matched: Matched,
    pub text: String,
    pub truncated: bool,
    /// The last command's (with integration and a prompt) or the shell's (exit) code.
    pub exit_code: Option<i32>,
    pub integration: bool,
    pub elapsed: Duration,
    pub mark: u64,
}

/// The screen as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenText {
    pub lines: Vec<String>,
    /// 0-based line and column of the cursor.
    pub cursor: (usize, usize),
    pub cols: usize,
    pub rows: usize,
}

impl ScreenText {
    /// The lines joined, trailing spaces and empty lines at the end trimmed.
    pub fn text(&self) -> String {
        let mut lines: Vec<&str> = self.lines.iter().map(|l| l.trim_end()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

enum Msg {
    Input(Vec<u8>),
    Resize(u16, u16),
    /// Drop the screen and scrollback but the cursor's line.
    Clear,
    Close {
        kill: bool,
    },
}

/// The listener `alacritty_terminal` reports to while it parses (bells, titles, replies it writes to the PTY).
pub struct Listener {
    tx: Sender<Msg>,
    poller: Arc<Poller>,
    sink: EventSink,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Bell => (self.sink)(Event::Bell),
            TermEvent::Title(t) => (self.sink)(Event::Title(t)),
            TermEvent::ResetTitle => (self.sink)(Event::Title(String::new())),
            // Device status reports and the like: the application asked, the terminal answers on the PTY.
            TermEvent::PtyWrite(text) => {
                let _ = self.tx.send(Msg::Input(text.into_bytes()));
                let _ = self.poller.notify();
            }
            _ => {}
        }
    }
}

/// The emulator a terminal parses into.
pub type Emulator = Term<Listener>;

#[derive(Debug)]
struct Shared {
    text: Transcript,
    marks: VecDeque<Mark>,
    /// The shell sent an integration mark: its prompts and exit codes are known.
    integration: bool,
    /// `Some(code)` once the shell ended.
    exited: Option<Option<i32>>,
    last_output: Instant,
    cwd: Option<String>,
    title: String,
    size: (u16, u16),
    /// Grows with each interruption by the person.
    interrupts: u64,
    /// Waits in flight.
    waiting: usize,
}

struct Inner {
    term: FairMutex<Emulator>,
    shared: Mutex<Shared>,
    cond: Condvar,
    tx: Mutex<Sender<Msg>>,
    poller: Arc<Poller>,
    pid: u32,
    dirty: AtomicBool,
    /// Grows with each parsed chunk.
    generation: AtomicU64,
    sink: EventSink,
    #[cfg(unix)]
    fd: std::os::fd::RawFd,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The I/O thread finds the terminal gone and ends the shell.
        let _ = self.poller.notify();
    }
}

/// A terminal. Cheap to clone; every method may be called from any thread.
#[derive(Clone)]
pub struct Terminal {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("pid", &self.inner.pid)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn window_size(cols: u16, rows: u16) -> WindowSize {
    WindowSize {
        num_lines: rows,
        num_cols: cols,
        cell_width: 8,
        cell_height: 16,
    }
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(not(unix))]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

impl Terminal {
    /// Start `options.program` on a new PTY. Spawning takes a few milliseconds (fork and exec); call it off the UI
    /// thread.
    pub fn spawn(options: Options, sink: EventSink) -> io::Result<Terminal> {
        let cols = options.cols.max(2);
        let rows = options.rows.max(1);
        let pty_options = tty::Options {
            shell: Some(tty::Shell::new(
                options.program.clone(),
                options.args.clone(),
            )),
            working_directory: options.cwd.clone().filter(|d| d.is_dir()),
            drain_on_exit: true,
            env: options.env.clone().into_iter().collect(),
            #[cfg(windows)]
            escape_args: true,
        };
        let mut pty = tty::new(&pty_options, window_size(cols, rows), 0)?;
        #[cfg(unix)]
        let pid = pty.child().id();
        #[cfg(windows)]
        let pid = pty.child_watcher().pid().map_or(0, |p| p.get());
        #[cfg(unix)]
        let fd = {
            use std::os::fd::AsRawFd;
            pty.file().as_raw_fd()
        };
        let poller = Arc::new(Poller::new()?);
        let (tx, rx) = mpsc::channel();
        let listener = Listener {
            tx: tx.clone(),
            poller: poller.clone(),
            sink: sink.clone(),
        };
        let config = TermConfig {
            scrolling_history: options.scrollback,
            ..TermConfig::default()
        };
        let term = Term::new(
            config,
            &TermSize::new(cols as usize, rows as usize),
            listener,
        );
        let inner = Arc::new(Inner {
            term: FairMutex::new(term),
            shared: Mutex::new(Shared {
                text: Transcript::new(),
                marks: VecDeque::new(),
                integration: false,
                exited: None,
                last_output: Instant::now(),
                cwd: None,
                title: String::new(),
                size: (cols, rows),
                interrupts: 0,
                waiting: 0,
            }),
            cond: Condvar::new(),
            tx: Mutex::new(tx),
            poller: poller.clone(),
            pid,
            dirty: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            sink,
            #[cfg(unix)]
            fd,
        });
        // Register before the thread starts, so the child's early exit is not missed.
        #[allow(unsafe_code)]
        // SAFETY: the PTY's sources stay alive (owned by the I/O thread) until they are deregistered there.
        unsafe {
            pty.register(&poller, PollEvent::readable(0), PollMode::Level)?;
        }
        let io = IoLoop {
            inner: Arc::downgrade(&inner),
            pty,
            rx,
            poller,
            scanner: Scanner::new(),
            parser: Processor::new(),
            writes: VecDeque::new(),
        };
        std::thread::Builder::new()
            .name(format!("terminal {pid}"))
            .spawn(move || io.run())?;
        Ok(Terminal { inner })
    }

    pub fn pid(&self) -> u32 {
        self.inner.pid
    }

    fn send(&self, msg: Msg) {
        let _ = lock(&self.inner.tx).send(msg);
        let _ = self.inner.poller.notify();
    }

    /// Write bytes to the PTY (what the person or an agent typed).
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        if !bytes.is_empty() {
            self.send(Msg::Input(bytes));
        }
    }

    /// Paste `text`: newlines become carriage returns, and the text is bracketed (`ESC [200~` ... `ESC [201~`) when
    /// the application asked for bracketed paste.
    pub fn paste(&self, text: &str) {
        let bracketed = self.with_term_blocking(|t| t.mode().contains(TermMode::BRACKETED_PASTE));
        self.write(paste_bytes(text, bracketed));
    }

    /// Resize the grid and the PTY (applied on the I/O thread).
    pub fn resize(&self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        {
            let mut s = lock(&self.inner.shared);
            if s.size == (cols, rows) {
                return;
            }
            s.size = (cols, rows);
        }
        self.send(Msg::Resize(cols, rows));
    }

    /// The grid's size: (columns, rows).
    pub fn size(&self) -> (u16, u16) {
        lock(&self.inner.shared).size
    }

    /// End the shell: SIGHUP (and a kill after a second), or with `kill` every process of the terminal at once.
    pub fn close(&self, kill: bool) {
        self.send(Msg::Close { kill });
    }

    /// Erase the screen (but the cursor's line) and the scrollback.
    pub fn clear(&self) {
        self.send(Msg::Clear);
    }

    /// `Some(code)` once the shell ended.
    pub fn exited(&self) -> Option<Option<i32>> {
        lock(&self.inner.shared).exited
    }

    pub fn is_running(&self) -> bool {
        self.exited().is_none()
    }

    /// Whether a command runs: the terminal's foreground process group is not the shell's (Unix; false elsewhere).
    pub fn busy(&self) -> bool {
        if !self.is_running() {
            return false;
        }
        #[cfg(unix)]
        {
            #[allow(unsafe_code)]
            // SAFETY: tcgetpgrp only reads the terminal's foreground group; a closed descriptor answers -1.
            let pgrp = unsafe { libc::tcgetpgrp(self.inner.fd) };
            pgrp > 0 && pgrp as u32 != self.inner.pid
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// The name of the command running in the foreground (Linux: `/proc/<group>/comm`), when one runs.
    pub fn foreground(&self) -> Option<String> {
        if !self.busy() {
            return None;
        }
        #[cfg(unix)]
        {
            #[allow(unsafe_code)]
            // SAFETY: as in `busy`.
            let pgrp = unsafe { libc::tcgetpgrp(self.inner.fd) };
            std::fs::read_to_string(format!("/proc/{pgrp}/comm"))
                .ok()
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Whether the shell sent an integration mark.
    pub fn integration(&self) -> bool {
        lock(&self.inner.shared).integration
    }

    /// The shell's current folder, when its integration reported one (OSC 7).
    pub fn cwd(&self) -> Option<String> {
        lock(&self.inner.shared).cwd.clone()
    }

    pub fn title(&self) -> String {
        lock(&self.inner.shared).title.clone()
    }

    /// The transcript's end: the mark of what comes next.
    pub fn mark(&self) -> u64 {
        lock(&self.inner.shared).text.end()
    }

    /// The plain text printed after `mark`, at most its last `max` bytes: (text, whether its start was cut, the end
    /// mark).
    pub fn since(&self, mark: u64, max: usize) -> (String, bool, u64) {
        let s = lock(&self.inner.shared);
        let (text, cut) = s.text.since(mark, max);
        (text, cut, s.text.end())
    }

    /// The integration marks after `mark`.
    pub fn marks_since(&self, mark: u64) -> Vec<Mark> {
        lock(&self.inner.shared)
            .marks
            .iter()
            .filter(|m| m.at >= mark)
            .copied()
            .collect()
    }

    /// Whether the grid changed since the last call (clears it).
    pub fn take_dirty(&self) -> bool {
        self.inner.dirty.swap(false, Ordering::AcqRel)
    }

    /// Mark the grid changed (a view that could not take its snapshot this frame tries again on the next).
    pub fn mark_dirty(&self) {
        self.inner.dirty.store(true, Ordering::Release);
    }

    /// Grows with each chunk of output parsed.
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }

    /// Run `f` on the emulator if its lock is free now (the view, each frame).
    pub fn try_with_term<R>(&self, f: impl FnOnce(&Emulator) -> R) -> Option<R> {
        self.inner.term.try_lock_unfair().map(|t| f(&t))
    }

    /// Run `f` on the emulator, waiting at most one parse chunk ([`PARSE_CHUNK`]) for its lock.
    pub fn with_term_blocking<R>(&self, f: impl FnOnce(&Emulator) -> R) -> R {
        let t = self.inner.term.lock();
        f(&t)
    }

    /// The screen as text, with the cursor.
    pub fn screen(&self) -> ScreenText {
        self.with_term_blocking(|t| {
            let grid = t.grid();
            let (rows, cols) = (grid.screen_lines(), grid.columns());
            let lines = (0..rows as i32).map(|l| row_text(t, Line(l))).collect();
            let c = grid.cursor.point;
            ScreenText {
                lines,
                cursor: (c.line.0.max(0) as usize, c.column.0),
                cols,
                rows,
            }
        })
    }

    /// The last `max_lines` lines of the scrollback and the screen, trailing spaces trimmed.
    pub fn scrollback(&self, max_lines: usize) -> String {
        self.with_term_blocking(|t| {
            let grid = t.grid();
            let top = -(grid.history_size() as i32);
            let mut bottom = grid.screen_lines() as i32 - 1;
            // Empty lines below the cursor are not content.
            while bottom > grid.cursor.point.line.0 && row_text(t, Line(bottom)).trim().is_empty() {
                bottom -= 1;
            }
            let first = (bottom + 1 - max_lines as i32).max(top);
            (first..=bottom)
                .map(|l| row_text(t, Line(l)).trim_end().to_owned())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// The lines of the scrollback and screen, oldest first, and how many are history: for finding text.
    pub fn all_lines(&self) -> (Vec<String>, usize) {
        self.with_term_blocking(|t| {
            let grid = t.grid();
            let history = grid.history_size();
            let lines = (-(history as i32)..grid.screen_lines() as i32)
                .map(|l| row_text(t, Line(l)))
                .collect();
            (lines, history)
        })
    }

    /// End every wait in flight with [`Matched::Interrupted`] (the person typed).
    pub fn interrupt_waits(&self) {
        let mut s = lock(&self.inner.shared);
        s.interrupts += 1;
        drop(s);
        self.inner.cond.notify_all();
    }

    /// Waits in flight.
    pub fn waiting(&self) -> usize {
        lock(&self.inner.shared).waiting
    }

    /// Block until `w` is satisfied, `timeout` passes, the person interrupts, or `cancel` says so (checked every
    /// 50 ms). For an agent's thread, never the UI's.
    pub fn wait(&self, w: &WaitFor, timeout: Duration, cancel: &dyn Fn() -> bool) -> WaitResult {
        let started = Instant::now();
        let deadline = started + timeout;
        let mut s = lock(&self.inner.shared);
        let interrupts = s.interrupts;
        s.waiting += 1;
        let result = loop {
            let integration = s.integration;
            let end = s.text.end();
            let finish = |matched: Matched, s: &Shared, exit_code: Option<i32>| {
                let (text, truncated) = match (&matched, integration) {
                    (Matched::Prompt, true) => command_output(s, w.mark),
                    _ => s.text.since(w.mark, WAIT_TEXT_MAX),
                };
                WaitResult {
                    matched,
                    text,
                    truncated,
                    exit_code,
                    integration,
                    elapsed: started.elapsed(),
                    mark: end,
                }
            };
            if s.interrupts != interrupts {
                break finish(Matched::Interrupted, &s, None);
            }
            if let Some(re) = &w.pattern {
                let (text, _) = s.text.since(w.mark, usize::MAX);
                if let Some(m) = re.find(&text) {
                    let found = m.as_str().to_owned();
                    break finish(Matched::Pattern(found), &s, None);
                }
            }
            if w.prompt {
                if integration {
                    let after: Vec<&Mark> = s.marks.iter().filter(|m| m.at >= w.mark).collect();
                    // A prompt counts once drawn: its B came (the person can type). A shell that sends no B: an A
                    // after the mark.
                    let uses_b = s.marks.iter().any(|m| m.kind == MarkKind::CommandStart);
                    let prompt = after.iter().enumerate().rev().find_map(|(i, m)| {
                        let drawn = if uses_b {
                            after[i..].iter().any(|n| n.kind == MarkKind::CommandStart)
                        } else {
                            m.at > w.mark
                        };
                        (m.kind == MarkKind::PromptStart && drawn).then_some(i)
                    });
                    if let Some(p) = prompt {
                        let code = after[..p].iter().rev().find_map(|m| match m.kind {
                            MarkKind::CommandEnd(c) => c,
                            _ => None,
                        });
                        break finish(Matched::Prompt, &s, code);
                    }
                } else if end > w.mark
                    && s.last_output.elapsed() >= PROMPT_SILENCE
                    && self.cursor_line_is_prompt()
                {
                    break finish(Matched::Prompt, &s, None);
                }
            }
            if let Some(code) = s.exited {
                // The shell ended: the exit, or what else was waited for never comes.
                break finish(Matched::Exit, &s, code);
            }
            let now = Instant::now();
            if now >= deadline || cancel() {
                break finish(Matched::Timeout, &s, None);
            }
            let mut step = (deadline - now).min(Duration::from_millis(50));
            if w.prompt && !integration && end > w.mark {
                let silent = s.last_output.elapsed();
                if silent < PROMPT_SILENCE {
                    step = step.min(PROMPT_SILENCE - silent + Duration::from_millis(5));
                }
            }
            s = self
                .inner
                .cond
                .wait_timeout(s, step)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        s.waiting -= 1;
        result
    }

    /// The prompt heuristic: the cursor's line, up to the cursor, ends in `$ `, `> `, `# ` or `% `.
    fn cursor_line_is_prompt(&self) -> bool {
        self.with_term_blocking(|t| {
            let c = t.grid().cursor.point;
            let line = row_text(t, c.line);
            let upto: String = line.chars().take(c.column.0).collect();
            let upto = upto.trim_end_matches('\u{0}');
            ["$ ", "> ", "# ", "% "].iter().any(|p| upto.ends_with(p))
                || (c.column.0 == 0 && ["$", ">", "#"].iter().any(|p| line.trim_end().ends_with(p)))
        })
    }
}

/// The most text a wait answers.
pub const WAIT_TEXT_MAX: usize = 64 * 1024;

/// The output of the commands after `mark`: the text between each `C` and its `D` (falls back to everything after
/// the mark, up to the last prompt, when the shell marked no command).
fn command_output(s: &Shared, mark: u64) -> (String, bool) {
    let mut out = String::new();
    let mut start: Option<u64> = None;
    let mut command: Option<u64> = None;
    let mut any = false;
    let mut cut = false;
    for m in s.marks.iter().filter(|m| m.at >= mark) {
        match m.kind {
            MarkKind::CommandStart => command = Some(m.at),
            MarkKind::OutputStart => start = Some(m.at),
            MarkKind::CommandEnd(_) => {
                // No output mark (a shell without PS0): the output follows the command's own line.
                let from = start
                    .take()
                    .or_else(|| command.take().and_then(|b| s.text.next_line_mark(b)));
                if let Some(a) = from {
                    let (t, c) = s.text.range(a, m.at, WAIT_TEXT_MAX);
                    out.push_str(&t);
                    cut |= c;
                    any = true;
                }
            }
            _ => {}
        }
    }
    if !any {
        let end = s
            .marks
            .iter()
            .rev()
            .find(|m| m.at >= mark && m.kind == MarkKind::PromptStart)
            .map_or(s.text.end(), |m| m.at);
        return s.text.range(mark, end, WAIT_TEXT_MAX);
    }
    if out.len() > WAIT_TEXT_MAX {
        let mut a = out.len() - WAIT_TEXT_MAX;
        while !out.is_char_boundary(a) {
            a += 1;
        }
        out.drain(..a);
        cut = true;
    }
    (out, cut)
}

/// A grid line as text (wide characters once, their spacers skipped).
pub fn row_text(t: &Emulator, line: Line) -> String {
    use alacritty_terminal::term::cell::Flags;
    let grid = t.grid();
    let row = &grid[line];
    let mut s = String::with_capacity(grid.columns());
    for col in 0..grid.columns() {
        let cell = &row[Column(col)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        s.push(cell.c);
    }
    s
}

/// The text between two grid points (inclusive), as a selection copies it: wrapped lines joined, other lines
/// ending with a newline, trailing spaces trimmed.
pub fn text_between(t: &Emulator, start: Point, end: Point) -> String {
    use alacritty_terminal::index::Side;
    use alacritty_terminal::selection::{Selection, SelectionType};
    let (a, b) = if (start.line, start.column) <= (end.line, end.column) {
        (start, end)
    } else {
        (end, start)
    };
    let mut sel = Selection::new(SelectionType::Simple, a, Side::Left);
    sel.update(b, Side::Right);
    let Some(range) = sel.to_range(t) else {
        return String::new();
    };
    let text = t.bounds_to_string(range.start, range.end);
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a paste writes: `\r` for line ends, bracketed when asked (ESC removed from the text, so it cannot end the
/// bracket early).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let clean: String = text.chars().filter(|&c| c != '\u{1b}').collect();
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

/// The I/O thread's state.
struct IoLoop {
    inner: std::sync::Weak<Inner>,
    pty: tty::Pty,
    rx: Receiver<Msg>,
    poller: Arc<Poller>,
    scanner: Scanner,
    parser: Processor<StdSyncHandler>,
    writes: VecDeque<(Vec<u8>, usize)>,
}

impl IoLoop {
    fn run(mut self) {
        let mut events = Events::with_capacity(NonZeroUsize::new(64).expect("non-zero"));
        let mut interest = PollEvent::readable(0);
        let mut buf = vec![0u8; 64 * 1024];
        let mut exit: Option<Option<i32>> = None;
        let mut kill = false;
        'run: loop {
            let timeout = self
                .parser
                .sync_timeout()
                .sync_timeout()
                .map(|t| t.saturating_duration_since(Instant::now()));
            events.clear();
            if let Err(e) = self.poller.wait(&mut events, timeout)
                && e.kind() != ErrorKind::Interrupted
            {
                break;
            }
            let Some(inner) = self.inner.upgrade() else {
                kill = true;
                break;
            };
            // A synchronized update (mode 2026) that timed out is drawn as it is.
            if events.is_empty()
                && timeout.is_some()
                && self
                    .parser
                    .sync_timeout()
                    .sync_timeout()
                    .is_some_and(|t| t <= Instant::now())
            {
                self.parser.stop_sync(&mut *inner.term.lock());
                self.changed(&inner);
            }
            loop {
                match self.rx.try_recv() {
                    Ok(Msg::Input(bytes)) => self.writes.push_back((bytes, 0)),
                    Ok(Msg::Resize(cols, rows)) => {
                        self.pty.on_resize(window_size(cols, rows));
                        inner
                            .term
                            .lock()
                            .resize(TermSize::new(cols as usize, rows as usize));
                        self.changed(&inner);
                    }
                    Ok(Msg::Clear) => {
                        self.clear(&inner);
                    }
                    Ok(Msg::Close { kill: k }) => {
                        kill = k;
                        break 'run;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'run,
                }
            }
            for event in events.iter() {
                if event.key == CHILD_TOKEN {
                    if let Some(tty::ChildEvent::Exited(status)) = self.pty.next_child_event() {
                        // What the shell printed last.
                        let _ = self.read(&inner, &mut buf);
                        exit = Some(status.map(exit_code));
                        break 'run;
                    }
                    continue;
                }
                if event.readable
                    && let Err(e) = self.read(&inner, &mut buf)
                {
                    // Linux answers EIO once the shell's side closed: its exit event follows.
                    #[cfg(target_os = "linux")]
                    if e.raw_os_error() == Some(libc::EIO) {
                        continue;
                    }
                    let _ = e;
                    break 'run;
                }
                if event.writable && self.write().is_err() {
                    break 'run;
                }
            }
            if !self.writes.is_empty() && self.write().is_err() {
                break;
            }
            let want_write = !self.writes.is_empty();
            if want_write != interest.writable {
                interest.writable = want_write;
                if self
                    .pty
                    .reregister(&self.poller, interest, PollMode::Level)
                    .is_err()
                {
                    break;
                }
            }
        }
        let _ = self.pty.deregister(&self.poller);
        if exit.is_none() {
            exit = Some(self.end_child(kill));
        }
        let code = exit.flatten();
        if let Some(inner) = self.inner.upgrade() {
            // The event first: whoever sees the exit can count on the event having been sent.
            (inner.sink)(Event::Exited(code));
            let mut s = lock(&inner.shared);
            s.exited = Some(code);
            drop(s);
            inner.cond.notify_all();
            self.changed(&inner);
        }
        // Dropping the PTY hangs up and reaps the child, here rather than on the caller's thread.
        drop(self.pty);
    }

    /// End a shell that has not exited: SIGHUP (as closing a terminal does), SIGKILL after a second or at once
    /// with `kill` (the foreground job too). Returns its exit code when known.
    #[cfg(unix)]
    fn end_child(&mut self, kill: bool) -> Option<i32> {
        let pid = self.pty.child().id() as libc::pid_t;
        #[allow(unsafe_code)]
        // SAFETY: plain signals to the shell's session; a stale id is the shell's own (not reaped until the PTY drops).
        unsafe {
            if kill {
                let fg = {
                    use std::os::fd::AsRawFd;
                    libc::tcgetpgrp(self.pty.file().as_raw_fd())
                };
                if fg > 0 {
                    libc::killpg(fg, libc::SIGKILL);
                }
                libc::killpg(pid, libc::SIGKILL);
            } else {
                libc::kill(pid, libc::SIGHUP);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(tty::ChildEvent::Exited(status)) = self.pty.next_child_event() {
                return status.map(exit_code);
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        #[allow(unsafe_code)]
        // SAFETY: as above.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
        None
    }

    #[cfg(not(unix))]
    fn end_child(&mut self, _kill: bool) -> Option<i32> {
        // Closing the pseudoconsole ends its processes.
        None
    }

    fn changed(&self, inner: &Inner) {
        inner.generation.fetch_add(1, Ordering::AcqRel);
        if !inner.dirty.swap(true, Ordering::AcqRel) {
            (inner.sink)(Event::Changed);
        }
    }

    /// Read what the PTY has, parse it and record its text and marks.
    fn read(&mut self, inner: &Inner, buf: &mut [u8]) -> io::Result<()> {
        let mut any = false;
        loop {
            let n = match self.pty.reader().read(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                    break;
                }
                Err(e) => {
                    if any {
                        self.changed(inner);
                    }
                    return Err(e);
                }
            };
            any = true;
            let segments = self.scanner.feed(&buf[..n]);
            for seg in &segments {
                if let Segment::Bytes(b) = seg {
                    for chunk in b.chunks(PARSE_CHUNK) {
                        let mut term = inner.term.lock_unfair();
                        self.parser.advance(&mut *term, chunk);
                    }
                }
            }
            let mut s = lock(&inner.shared);
            for seg in segments {
                match seg {
                    Segment::Bytes(b) => s.text.append(&b),
                    Segment::Mark(kind) => {
                        s.integration = true;
                        let at = s.text.end();
                        s.marks.push_back(Mark { kind, at });
                        if s.marks.len() > MAX_MARKS {
                            s.marks.pop_front();
                        }
                    }
                    Segment::Cwd(path) => s.cwd = Some(path),
                }
            }
            s.last_output = Instant::now();
            drop(s);
            inner.cond.notify_all();
            self.changed(inner);
        }
        Ok(())
    }

    fn write(&mut self) -> io::Result<()> {
        while let Some((bytes, done)) = self.writes.front_mut() {
            match self.pty.writer().write(&bytes[*done..]) {
                Ok(0) => break,
                Ok(n) => {
                    *done += n;
                    if *done >= bytes.len() {
                        self.writes.pop_front();
                    }
                }
                Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Clear: scroll the cursor's line to the top, then drop the scrollback (the prompt stays).
    fn clear(&mut self, inner: &Inner) {
        let line = {
            let t = inner.term.lock();
            t.grid().cursor.point.line.0.max(0)
        };
        let seq = if line > 0 {
            format!("\x1b[{line}S\x1b[{line}A\x1b[3J")
        } else {
            "\x1b[3J".to_owned()
        };
        {
            let mut term = inner.term.lock_unfair();
            self.parser.advance(&mut *term, seq.as_bytes());
        }
        self.changed(inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pastes_end_lines_with_returns_and_bracket_when_asked() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste_bytes("x\x1by", true), b"\x1b[200~xy\x1b[201~");
    }
}
