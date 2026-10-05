//! The Terminal window (brief 0041; Visual Studio's View > Terminal, Ctrl+`): a tab per terminal with its profile's
//! name, Split (two terminals side by side in one tab), New Terminal with the profile dropdown, Kill and Clear, the
//! exit line with Restart, and "Agent <name> is typing" on a tab while an agent's `send` or `wait` runs.
//!
//! - **One set of terminals.** [`TerminalService`] owns them (`eludite-terminal`) and implements
//!   `eludite.terminal.*` on the invoking thread: an agent's `wait` blocks the agent's thread, never the UI's. The UI
//!   learns of terminals opened, changed, ringing, ending and closed through [`TerminalEvent`]s, applied in batches.
//! - **Every action is a command.** The window's buttons, the keys (Ctrl+`, Ctrl+Shift+`), the Workspace's Open in
//!   Terminal and Tools > Command Line dispatch `eludite.terminal.*`, which [`Shell::run_terminal`] runs off the UI
//!   thread (asking first before killing a running command).
//! - **The person wins.** A keystroke during an agent's `wait` ends it with `interrupted_by: "user"`; that agent's
//!   next `send` is refused until it `read`s.
//! - **Keys.** The terminal takes every key while focused except the shell's reserved chords ([`RESERVED_KEYS`]):
//!   the others' bindings are disabled in the terminal's key context ([`bind_keys`]).
//! - **Nothing at startup.** No shell starts until the window is shown from the menu or its key, or a terminal is
//!   opened; closing the workspace ends them (asking when a command runs).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use eludite_commands::terminal::{
    self as cmds, ClearOutput, CloseOutput, Cursor, ListOutput, OpenOutput, ReadMode, ReadOutput,
    ResizeOutput, SendOutput, TerminalCommands, TerminalOutput, TerminalRequest, TerminalRow,
    WaitOutput,
};
use eludite_commands::{Caller, CommandError, current_caller, workspace};
use eludite_docking::ids;
use eludite_terminal::links::Target;
use eludite_terminal::profile::{self, Profile};
use eludite_terminal::pty::{Event as PtyEvent, Matched, Options, Terminal, WaitFor};
use eludite_terminal::{TerminalView, TerminalViewEvent, ViewSettings};
use eludite_terminal::{ToolPaths, env as term_env, integration};
use eludite_ui::{RunCommand, Theme, slots};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement,
    IntoElement, ParentElement, PromptLevel, Render, SharedString, StatefulInteractiveElement,
    Styled, Task, Window, div, px,
};
use serde_json::{Value, json};

use super::Shell;

/// The keys that stay the shell's while a terminal has focus (the report's key routing table): Ctrl+` and
/// Ctrl+Shift+`, F5 and the debug keys, the build keys, Shift+Esc and the tool windows' chords (Ctrl+Alt, Ctrl+\\
/// and Ctrl+0 ones; a lone Ctrl+\\ or Ctrl+0 reaches the shell when no second key follows). Escape (back to the
/// editor) is the terminal view's own. Every other key of the shell's keymap goes to the terminal.
pub const RESERVED_KEYS: [&str; 27] = [
    "ctrl-`",
    "ctrl-shift-`",
    "ctrl-~",
    "f5",
    "ctrl-f5",
    "shift-f5",
    "ctrl-shift-f5",
    "f9",
    "f10",
    "f11",
    "shift-f11",
    "ctrl-f10",
    "ctrl-shift-f10",
    "ctrl-alt-pause",
    "ctrl-alt-p",
    "ctrl-alt-b",
    "ctrl-shift-b",
    "f6",
    "shift-f6",
    "shift-escape",
    "ctrl-alt-l",
    "ctrl-alt-o",
    "ctrl-alt-x",
    "ctrl-\\ ctrl-e",
    "ctrl-\\ ctrl-c",
    "ctrl-0 ctrl-g",
    "ctrl-0 ctrl-r",
];

/// Disable, in a terminal's key context, the shell's bindings that are not reserved, so those keys reach the shell
/// running in it. Call after the shell's keymap is bound.
pub fn bind_keys(cx: &mut App) {
    let keymap = eludite_ui::vs_keymap();
    let keys: Vec<&str> = keymap.iter().map(|k| k.keystrokes).collect();
    eludite_terminal::view::bind_keys(cx, &keys, &RESERVED_KEYS);
}

/// The size a terminal starts with before its view lays it out.
const START_SIZE: (u16, u16) = (120, 30);
/// How long "Agent <name> is typing" stays after the agent's last `send` or `wait`.
const AGENT_LINGER: Duration = Duration::from_millis(800);

/// What the service tells the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalEvent {
    Opened {
        id: String,
        name: String,
        split_with: Option<String>,
        replace: Option<String>,
        /// Opened by the person (focus it).
        focus: bool,
    },
    Changed(String),
    Bell(String),
    Exited(String, Option<i32>),
    Closed(String),
    /// An agent's `send` or `wait` started (`Some(name)`) or ended (`None`).
    Agent(String, Option<String>),
}

/// The `terminal.*` settings the service applies to new terminals.
#[derive(Debug, Clone, PartialEq)]
pub struct TermSettings {
    pub default_profile: String,
    pub profiles: Vec<Profile>,
    pub scrollback: usize,
    pub inherit_tool_paths: bool,
    pub shell_integration: bool,
    /// `build.cargoPath`, where Cargo is when set.
    pub cargo: Option<PathBuf>,
}

impl Default for TermSettings {
    fn default() -> Self {
        Self {
            default_profile: String::new(),
            profiles: Vec::new(),
            scrollback: 10_000,
            inherit_tool_paths: true,
            shell_integration: true,
            cargo: None,
        }
    }
}

/// Reads a variable of Eludite's environment.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// How the service finds profiles and tools (tests give fixed ones).
#[derive(Clone)]
pub struct TerminalSetup {
    /// Reads Eludite's environment.
    pub env: EnvLookup,
    /// The built-in profiles; `None` for the platform's.
    pub builtin: Option<Vec<Profile>>,
    /// Where the integration scripts are written.
    pub integration_dir: PathBuf,
}

impl TerminalSetup {
    pub fn from_env() -> Self {
        Self {
            env: Arc::new(|k| std::env::var(k).ok()),
            builtin: None,
            integration_dir: integration::default_dir(),
        }
    }
}

struct Entry {
    id: String,
    name: String,
    profile: String,
    cwd: PathBuf,
    terminal: Terminal,
    split_with: Option<String>,
    /// The agent whose `send` or `wait` is in flight.
    agent: Option<String>,
    /// Agents interrupted by the person, refused `send` until they `read`.
    locked: HashSet<String>,
    /// Each agent's last `send` mark (the default of its `wait` and `read since`).
    last_send: HashMap<String, u64>,
}

#[derive(Default)]
struct State {
    next: u64,
    terminals: Vec<Entry>,
    active: Option<String>,
    settings: TermSettings,
    workspace: Option<PathBuf>,
    tools: Option<(Option<PathBuf>, ToolPaths)>,
    integration_installed: bool,
}

/// The terminals and `eludite.terminal.*` (any thread).
pub struct TerminalService {
    state: Mutex<State>,
    events: UnboundedSender<TerminalEvent>,
    setup: TerminalSetup,
}

fn agent_of(caller: &Caller) -> Option<String> {
    match caller {
        Caller::Agent { agent, .. } => Some(agent.clone()),
        _ => None,
    }
}

fn failed(message: impl Into<String>) -> CommandError {
    CommandError::Failed(message.into())
}

impl TerminalService {
    pub fn new(setup: TerminalSetup, events: UnboundedSender<TerminalEvent>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            events,
            setup,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send_event(&self, e: TerminalEvent) {
        let _ = self.events.unbounded_send(e);
    }

    pub fn set_settings(&self, settings: TermSettings) {
        let mut s = self.state();
        if s.settings.cargo != settings.cargo {
            s.tools = None;
        }
        s.settings = settings;
    }

    pub fn set_workspace(&self, folder: Option<&Path>) {
        self.state().workspace = folder.map(Path::to_path_buf);
    }

    /// The profiles: the built-in ones and the setting's.
    pub fn profiles(&self) -> Vec<Profile> {
        let added = self.state().settings.profiles.clone();
        let builtin = self
            .setup
            .builtin
            .clone()
            .unwrap_or_else(|| profile::builtin_profiles(&*self.setup.env));
        profile::merge_profiles(builtin, &added)
    }

    /// The profiles' names, the default first.
    pub fn profile_names(&self) -> Vec<String> {
        let profiles = self.profiles();
        let default = self.state().settings.default_profile.clone();
        let mut names: Vec<String> = profiles.iter().map(|p| p.name.clone()).collect();
        if let Some(d) = profile::pick(&profiles, None, &default).map(|p| p.name.clone())
            && let Some(i) = names.iter().position(|n| *n == d)
        {
            let d = names.remove(i);
            names.insert(0, d);
        }
        names
    }

    /// The terminal `id`, or the active one.
    fn terminal(&self, id: Option<&str>) -> Result<(String, Terminal), CommandError> {
        let s = self.state();
        let id = match id {
            Some(i) => i.to_owned(),
            None => s
                .active
                .clone()
                .ok_or_else(|| failed("no terminal is open: call eludite.terminal.open first"))?,
        };
        s.terminals
            .iter()
            .find(|e| e.id == id)
            .map(|e| (e.id.clone(), e.terminal.clone()))
            .ok_or_else(|| failed(format!("no terminal `{id}`: see eludite.terminal.list")))
    }

    fn with_entry<R>(&self, id: &str, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        self.state()
            .terminals
            .iter_mut()
            .find(|e| e.id == id)
            .map(f)
    }

    pub fn ids(&self) -> Vec<String> {
        self.state()
            .terminals
            .iter()
            .map(|e| e.id.clone())
            .collect()
    }

    pub fn handle(&self, id: &str) -> Option<Terminal> {
        self.state()
            .terminals
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.terminal.clone())
    }

    pub fn active(&self) -> Option<String> {
        self.state().active.clone()
    }

    pub fn set_active(&self, id: &str) {
        let mut s = self.state();
        if s.terminals.iter().any(|e| e.id == id) {
            s.active = Some(id.to_owned());
        }
    }

    /// The folder links in `id` resolve against: the shell's current folder (OSC 7), else where it started.
    pub fn cwd_of(&self, id: &str) -> Option<PathBuf> {
        let s = self.state();
        let e = s.terminals.iter().find(|e| e.id == id)?;
        Some(
            e.terminal
                .cwd()
                .map(PathBuf::from)
                .unwrap_or_else(|| e.cwd.clone()),
        )
    }

    /// The terminals whose shell runs a command: (name, the command's name).
    pub fn busy(&self) -> Vec<(String, String)> {
        self.state()
            .terminals
            .iter()
            .filter(|e| e.terminal.busy())
            .map(|e| {
                (
                    e.name.clone(),
                    e.terminal
                        .foreground()
                        .unwrap_or_else(|| "a command".into()),
                )
            })
            .collect()
    }

    /// The person typed into `id`: an agent's wait there ends.
    pub fn person_typed(&self, id: &str) {
        if let Some(t) = self.handle(id)
            && t.waiting() > 0
        {
            t.interrupt_waits();
        }
    }

    /// End every terminal (the workspace closes).
    pub fn close_all(&self) {
        let entries: Vec<(String, Terminal)> = {
            let mut s = self.state();
            s.active = None;
            s.terminals.drain(..).map(|e| (e.id, e.terminal)).collect()
        };
        for (id, t) in entries {
            t.close(true);
            self.send_event(TerminalEvent::Closed(id));
        }
    }

    fn tools(&self) -> ToolPaths {
        let mut s = self.state();
        let cargo = s.settings.cargo.clone();
        if let Some((c, t)) = &s.tools
            && *c == cargo
        {
            return t.clone();
        }
        let t = ToolPaths::locate(&*self.setup.env, cargo.as_deref());
        s.tools = Some((cargo, t.clone()));
        t
    }

    fn open(&self, request: TerminalRequest, caller: &Caller) -> Result<OpenOutput, CommandError> {
        let TerminalRequest::Open {
            profile: profile_name,
            cwd,
            env: extra,
            name,
            split_with,
            replace,
        } = request
        else {
            unreachable!("open")
        };
        let (settings, workspace, replaced) = {
            let s = self.state();
            let replaced = replace.as_ref().map(|r| {
                s.terminals.iter().find(|e| &e.id == r).map(|e| {
                    (
                        e.profile.clone(),
                        e.cwd.clone(),
                        e.name.clone(),
                        e.terminal.is_running(),
                    )
                })
            });
            (s.settings.clone(), s.workspace.clone(), replaced)
        };
        let replaced = match replaced {
            Some(None) => {
                return Err(failed(format!(
                    "no terminal `{}`: see eludite.terminal.list",
                    replace.unwrap_or_default()
                )));
            }
            Some(Some((_, _, _, true))) => {
                return Err(failed(
                    "its shell still runs: Restart replaces a terminal whose shell has ended",
                ));
            }
            Some(Some((p, c, n, false))) => Some((p, c, n)),
            None => None,
        };
        if let Some(s) = &split_with
            && self.handle(s).is_none()
        {
            return Err(failed(format!("no terminal `{s}` to split")));
        }
        let profiles = self.profiles();
        let wanted = profile_name
            .clone()
            .or_else(|| replaced.as_ref().map(|r| r.0.clone()));
        let profile = profile::pick(&profiles, wanted.as_deref(), &settings.default_profile)
            .cloned()
            .ok_or_else(|| {
                failed(format!(
                    "no profile `{}`: the profiles are {}",
                    wanted.unwrap_or_default(),
                    profiles
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        let home = (self.setup.env)("HOME")
            .or_else(|| (self.setup.env)("USERPROFILE"))
            .map(PathBuf::from);
        let resolve = |p: &str| {
            let p = Path::new(p);
            if p.is_absolute() {
                Some(p.to_path_buf())
            } else {
                workspace.as_ref().map(|w| w.join(p))
            }
        };
        let mut dir = cwd
            .as_deref()
            .and_then(resolve)
            .or_else(|| replaced.as_ref().map(|r| r.1.clone()))
            .or_else(|| profile.cwd.as_deref().and_then(resolve))
            .or_else(|| workspace.clone())
            .or(home)
            .unwrap_or_else(|| PathBuf::from("."));
        // Open in Terminal on a project file: its folder.
        if dir.is_file()
            && let Some(parent) = dir.parent()
        {
            dir = parent.to_path_buf();
        }
        if !dir.is_dir() {
            return Err(failed(format!("no folder {}", dir.display())));
        }
        let tools = settings.inherit_tool_paths.then(|| self.tools());
        let mut env_extra: BTreeMap<String, String> = profile.env.clone();
        env_extra.extend(extra);
        let base_path = (self.setup.env)("PATH");
        let mut env = term_env::terminal_env(base_path.as_deref(), tools.as_ref(), &env_extra);
        let mut args = profile.args.clone();
        let mut integrated = false;
        if settings.shell_integration {
            let installed = self.state().integration_installed;
            let ok = installed || integration::install(&self.setup.integration_dir).is_ok();
            if ok {
                self.state().integration_installed = true;
                integrated = integration::apply(
                    profile.kind(),
                    &self.setup.integration_dir,
                    &mut args,
                    &mut env,
                    &*self.setup.env,
                );
            }
        }
        let size = split_with
            .as_deref()
            .and_then(|s| self.handle(s))
            .map(|t| t.size())
            .or_else(|| {
                let s = self.state();
                s.active
                    .as_ref()
                    .and_then(|a| s.terminals.iter().find(|e| &e.id == a))
                    .map(|e| e.terminal.size())
            })
            .unwrap_or(START_SIZE);
        let id = {
            let mut s = self.state();
            s.next += 1;
            format!("term{}", s.next)
        };
        let events = self.events.clone();
        let sink_id = id.clone();
        let terminal = Terminal::spawn(
            Options {
                program: profile.command.clone(),
                args,
                env,
                cwd: Some(dir.clone()),
                cols: size.0,
                rows: size.1,
                scrollback: settings.scrollback,
            },
            Arc::new(move |e| {
                let event = match e {
                    PtyEvent::Changed => TerminalEvent::Changed(sink_id.clone()),
                    PtyEvent::Bell => TerminalEvent::Bell(sink_id.clone()),
                    PtyEvent::Exited(code) => TerminalEvent::Exited(sink_id.clone(), code),
                    PtyEvent::Title(_) => return,
                };
                let _ = events.unbounded_send(event);
            }),
        )
        .map_err(|e| failed(format!("{}: {e}", profile.command)))?;
        let name = {
            let s = self.state();
            match name.or_else(|| replaced.as_ref().map(|r| r.2.clone())) {
                Some(n) => n,
                None => {
                    let taken = |n: &str| s.terminals.iter().any(|e| e.name == n);
                    let mut n = profile.name.clone();
                    let mut k = 2;
                    while taken(&n) {
                        n = format!("{} ({k})", profile.name);
                        k += 1;
                    }
                    n
                }
            }
        };
        let pid = terminal.pid();
        let entry = Entry {
            id: id.clone(),
            name: name.clone(),
            profile: profile.name.clone(),
            cwd: dir.clone(),
            terminal,
            split_with: split_with.clone(),
            agent: None,
            locked: HashSet::new(),
            last_send: HashMap::new(),
        };
        {
            let mut s = self.state();
            match replace
                .as_ref()
                .and_then(|r| s.terminals.iter().position(|e| &e.id == r))
            {
                Some(i) => {
                    let old = std::mem::replace(&mut s.terminals[i], entry);
                    s.terminals[i].split_with = old.split_with;
                    for e in s.terminals.iter_mut() {
                        if e.split_with.as_deref() == Some(old.id.as_str()) {
                            e.split_with = Some(id.clone());
                        }
                    }
                }
                None => s.terminals.push(entry),
            }
            s.active = Some(id.clone());
        }
        self.send_event(TerminalEvent::Opened {
            id: id.clone(),
            name: name.clone(),
            split_with,
            replace,
            focus: matches!(caller, Caller::User),
        });
        Ok(OpenOutput {
            terminal: id,
            pid,
            name,
            profile: profile.name,
            cwd: dir.to_string_lossy().into_owned(),
            integration: integrated,
            mark: 0,
        })
    }

    fn list(&self) -> ListOutput {
        let profiles = self.profile_names();
        let s = self.state();
        ListOutput {
            terminals: s
                .terminals
                .iter()
                .map(|e| {
                    let (cols, rows) = e.terminal.size();
                    let exited = e.terminal.exited();
                    TerminalRow {
                        terminal: e.id.clone(),
                        name: e.name.clone(),
                        profile: e.profile.clone(),
                        pid: e.terminal.pid(),
                        cwd: e
                            .terminal
                            .cwd()
                            .unwrap_or_else(|| e.cwd.to_string_lossy().into_owned()),
                        running: exited.is_none(),
                        exit_code: exited.flatten(),
                        busy: e.terminal.busy(),
                        integration: e.terminal.integration(),
                        cols,
                        rows,
                        split_with: e.split_with.clone(),
                        agent: e.agent.clone(),
                        mark: e.terminal.mark(),
                    }
                })
                .collect(),
            active: s.active.clone(),
            profiles,
        }
    }

    fn set_agent(&self, id: &str, agent: Option<String>) {
        self.with_entry(id, |e| e.agent = agent.clone());
        self.send_event(TerminalEvent::Agent(id.to_owned(), agent));
    }

    fn send(
        &self,
        terminal: Option<String>,
        text: String,
        newline: bool,
        caller: &Caller,
    ) -> Result<SendOutput, CommandError> {
        let (id, t) = self.terminal(terminal.as_deref())?;
        if !t.is_running() {
            return Err(failed(format!(
                "the shell in {id} has ended: open a new terminal (or Restart it with eludite.terminal.open's `replace`)"
            )));
        }
        let agent = agent_of(caller);
        if let Some(a) = &agent
            && self
                .with_entry(&id, |e| e.locked.contains(a))
                .unwrap_or(false)
        {
            return Err(failed(format!(
                "the person typed into {id} while you waited: call eludite.terminal.read to see what they did before sending more"
            )));
        }
        let mut bytes = text.into_bytes();
        if newline {
            bytes.push(b'\r');
        }
        let n = bytes.len();
        let mark = t.mark();
        if let Some(a) = &agent {
            self.set_agent(&id, Some(a.clone()));
            self.with_entry(&id, |e| e.last_send.insert(a.clone(), mark));
        }
        t.write(bytes);
        if agent.is_some() {
            self.set_agent(&id, None);
        }
        Ok(SendOutput {
            terminal: id,
            mark,
            bytes: n,
        })
    }

    fn read(
        &self,
        terminal: Option<String>,
        mode: ReadMode,
        mark: Option<u64>,
        max_lines: usize,
        caller: &Caller,
    ) -> Result<ReadOutput, CommandError> {
        let (id, t) = self.terminal(terminal.as_deref())?;
        let agent = agent_of(caller);
        let last = agent.as_ref().and_then(|a| {
            self.with_entry(&id, |e| e.last_send.get(a).copied())
                .flatten()
        });
        if let Some(a) = &agent {
            // The agent looked: it may send again.
            self.with_entry(&id, |e| e.locked.remove(a));
        }
        let screen = t.screen();
        let (text, truncated, end) = match mode {
            ReadMode::Screen => (screen.text(), false, t.mark()),
            ReadMode::Since => t.since(mark.or(last).unwrap_or(0), cmds::READ_TEXT_MAX),
            ReadMode::Scrollback => (t.scrollback(max_lines), false, t.mark()),
        };
        let (cols, rows) = t.size();
        let exited = t.exited();
        Ok(ReadOutput {
            terminal: id,
            text,
            cursor: Cursor {
                line: screen.cursor.0 + 1,
                column: screen.cursor.1 + 1,
            },
            cols,
            rows,
            running: exited.is_none(),
            exit_code: exited.flatten(),
            busy: t.busy(),
            integration: t.integration(),
            truncated,
            mark: end,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn wait(
        &self,
        terminal: Option<String>,
        mark: Option<u64>,
        prompt: bool,
        pattern: Option<String>,
        exit: bool,
        timeout_ms: u64,
        caller: &Caller,
    ) -> Result<WaitOutput, CommandError> {
        let (id, t) = self.terminal(terminal.as_deref())?;
        let agent = agent_of(caller);
        let last = agent.as_ref().and_then(|a| {
            self.with_entry(&id, |e| e.last_send.get(a).copied())
                .flatten()
        });
        let mark = mark.or(last).unwrap_or_else(|| t.mark());
        let pattern = pattern
            .map(|p| eludite_terminal::regex::Regex::new(&p))
            .transpose()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
        if let Some(a) = &agent {
            self.set_agent(&id, Some(a.clone()));
        }
        let r = t.wait(
            &WaitFor {
                mark,
                prompt,
                pattern,
                exit,
            },
            Duration::from_millis(timeout_ms),
            // The terminal closed (no longer listed): stop waiting.
            &|| self.handle(&id).is_none(),
        );
        if let Some(a) = &agent {
            if r.matched == Matched::Interrupted {
                self.with_entry(&id, |e| e.locked.insert(a.clone()));
            }
            self.set_agent(&id, None);
        }
        let (matched, matched_text) = match &r.matched {
            Matched::Prompt => ("prompt", None),
            Matched::Pattern(m) => ("pattern", Some(m.clone())),
            Matched::Exit => ("exit", None),
            Matched::Timeout => ("timeout", None),
            Matched::Interrupted => ("interrupted", None),
        };
        Ok(WaitOutput {
            terminal: id,
            matched: matched.into(),
            interrupted_by: (r.matched == Matched::Interrupted).then(|| "user".into()),
            text: r.text,
            matched_text,
            exit_code: r.exit_code,
            integration: r.integration,
            truncated: r.truncated,
            elapsed_ms: r.elapsed.as_millis() as u64,
            mark: r.mark,
        })
    }

    fn close(&self, terminal: Option<String>, kill: bool) -> Result<CloseOutput, CommandError> {
        let (id, t) = self.terminal(terminal.as_deref())?;
        let busy = t.busy();
        if busy && !kill {
            return Err(failed(format!(
                "{} runs in {id}: close it with `kill: true` to end it",
                t.foreground().unwrap_or_else(|| "a command".into())
            )));
        }
        {
            let mut s = self.state();
            s.terminals.retain(|e| e.id != id);
            for e in s.terminals.iter_mut() {
                if e.split_with.as_deref() == Some(id.as_str()) {
                    e.split_with = None;
                }
            }
            if s.active.as_deref() == Some(id.as_str()) {
                s.active = s.terminals.last().map(|e| e.id.clone());
            }
        }
        t.close(kill);
        self.send_event(TerminalEvent::Closed(id.clone()));
        Ok(CloseOutput {
            terminal: id,
            killed: busy,
        })
    }
}

impl TerminalCommands for TerminalService {
    fn apply(&self, request: TerminalRequest) -> Result<TerminalOutput, CommandError> {
        let caller = current_caller();
        Ok(match request {
            r @ TerminalRequest::Open { .. } => TerminalOutput::Open(self.open(r, &caller)?),
            TerminalRequest::List => TerminalOutput::List(self.list()),
            TerminalRequest::Send {
                terminal,
                text,
                newline,
            } => TerminalOutput::Send(self.send(terminal, text, newline, &caller)?),
            TerminalRequest::Read {
                terminal,
                mode,
                mark,
                max_lines,
            } => TerminalOutput::Read(self.read(terminal, mode, mark, max_lines, &caller)?),
            TerminalRequest::Wait {
                terminal,
                mark,
                prompt,
                pattern,
                exit,
                timeout_ms,
            } => TerminalOutput::Wait(
                self.wait(terminal, mark, prompt, pattern, exit, timeout_ms, &caller)?,
            ),
            TerminalRequest::Resize {
                terminal,
                cols,
                rows,
            } => {
                let (id, t) = self.terminal(terminal.as_deref())?;
                t.resize(cols, rows);
                TerminalOutput::Resize(ResizeOutput {
                    terminal: id,
                    cols,
                    rows,
                })
            }
            TerminalRequest::Close { terminal, kill } => {
                TerminalOutput::Close(self.close(terminal, kill)?)
            }
            TerminalRequest::Clear { terminal } => {
                let (id, t) = self.terminal(terminal.as_deref())?;
                t.clear();
                TerminalOutput::Clear(ClearOutput {
                    terminal: id,
                    mark: t.mark(),
                })
            }
        })
    }
}

/// Register `eludite.terminal.*` on `commands` over a new service; the events go to the UI.
pub fn register(
    commands: &eludite_commands::CommandRegistry,
    setup: TerminalSetup,
) -> (Arc<TerminalService>, UnboundedReceiver<TerminalEvent>) {
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let service = Arc::new(TerminalService::new(setup, tx));
    cmds::register(commands, service.clone());
    (service, rx)
}

// ----- The window -----

/// Element ids (and debug selectors) of the window.
pub const NEW: &str = "terminal-new";
pub const PROFILES: &str = "terminal-profiles";
pub const SPLIT: &str = "terminal-split";
pub const KILL: &str = "terminal-kill";
pub const CLEAR: &str = "terminal-clear";

pub fn tab_selector(id: &str) -> String {
    format!("terminal-tab-{id}")
}

pub fn tab_close_selector(id: &str) -> String {
    format!("terminal-tab-close-{id}")
}

pub fn agent_selector(id: &str) -> String {
    format!("terminal-agent-{id}")
}

pub fn pane_selector(id: &str) -> String {
    format!("terminal-pane-{id}")
}

pub fn profile_selector(name: &str) -> String {
    format!(
        "terminal-profile-{}",
        name.replace(|c: char| !c.is_ascii_alphanumeric(), "-")
            .to_ascii_lowercase()
    )
}

/// What the window asks of the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalWindowEvent {
    /// A tab was selected: its terminal is the active one.
    Selected(String),
}

/// The Terminal tool window: tabs of one or two terminals.
pub struct TerminalWindow {
    theme: Theme,
    /// Each tab's terminals (two for a split).
    tabs: Vec<Vec<String>>,
    active: usize,
    views: HashMap<String, Entity<TerminalView>>,
    names: HashMap<String, String>,
    agents: HashMap<String, String>,
    profiles: Vec<String>,
    dropdown: bool,
    /// The pane of the active tab last focused.
    focused: Option<String>,
}

impl EventEmitter<TerminalWindowEvent> for TerminalWindow {}

impl TerminalWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            tabs: Vec::new(),
            active: 0,
            views: HashMap::new(),
            names: HashMap::new(),
            agents: HashMap::new(),
            profiles: Vec::new(),
            dropdown: false,
            focused: None,
        }
    }

    pub fn set_profiles(&mut self, profiles: Vec<String>, cx: &mut Context<Self>) {
        self.profiles = profiles;
        cx.notify();
    }

    #[cfg_attr(not(all(test, unix)), allow(dead_code))]
    pub fn profiles(&self) -> &[String] {
        &self.profiles
    }

    /// Add a terminal's view: in its own tab, beside `split_with`, or in `replace`'s place.
    pub fn add(
        &mut self,
        id: &str,
        name: &str,
        view: Entity<TerminalView>,
        split_with: Option<&str>,
        replace: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.views.insert(id.to_owned(), view);
        self.names.insert(id.to_owned(), name.to_owned());
        let placed = replace.and_then(|r| {
            self.tabs.iter_mut().enumerate().find_map(|(i, t)| {
                let p = t.iter().position(|x| x == r)?;
                t[p] = id.to_owned();
                Some(i)
            })
        });
        let placed = placed.or_else(|| {
            split_with.and_then(|s| {
                self.tabs.iter_mut().enumerate().find_map(|(i, t)| {
                    t.iter().any(|x| x == s).then(|| {
                        t.push(id.to_owned());
                        i
                    })
                })
            })
        });
        let tab = match placed {
            Some(i) => i,
            None => {
                self.tabs.push(vec![id.to_owned()]);
                self.tabs.len() - 1
            }
        };
        if let Some(r) = replace {
            self.views.remove(r);
            self.names.remove(r);
            self.agents.remove(r);
        }
        self.active = tab;
        self.focused = Some(id.to_owned());
        cx.notify();
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.views.remove(id);
        self.names.remove(id);
        self.agents.remove(id);
        for t in self.tabs.iter_mut() {
            t.retain(|x| x != id);
        }
        self.tabs.retain(|t| !t.is_empty());
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        if self.focused.as_deref() == Some(id) {
            self.focused = self.tabs.get(self.active).and_then(|t| t.first().cloned());
        }
        cx.notify();
    }

    pub fn set_agent(&mut self, id: &str, agent: Option<String>, cx: &mut Context<Self>) {
        match agent {
            Some(a) => {
                self.agents.insert(id.to_owned(), a);
            }
            None => {
                self.agents.remove(id);
            }
        }
        cx.notify();
    }

    /// The agent typing in `id`, as the tab shows it.
    #[cfg_attr(not(all(test, unix)), allow(dead_code))]
    pub fn agent(&self, id: &str) -> Option<&str> {
        self.agents.get(id).map(String::as_str)
    }

    pub fn view(&self, id: &str) -> Option<&Entity<TerminalView>> {
        self.views.get(id)
    }

    pub fn views(&self) -> impl Iterator<Item = (&String, &Entity<TerminalView>)> {
        self.views.iter()
    }

    /// The tabs' terminal ids, in order.
    #[cfg_attr(not(all(test, unix)), allow(dead_code))]
    pub fn tabs(&self) -> &[Vec<String>] {
        &self.tabs
    }

    #[cfg_attr(not(all(test, unix)), allow(dead_code))]
    pub fn name(&self, id: &str) -> Option<&str> {
        self.names.get(id).map(String::as_str)
    }

    /// The active tab's focused (or first) terminal.
    pub fn active_terminal(&self) -> Option<String> {
        let tab = self.tabs.get(self.active)?;
        self.focused
            .clone()
            .filter(|f| tab.contains(f))
            .or_else(|| tab.first().cloned())
    }

    pub fn set_focused(&mut self, id: &str) {
        if let Some(i) = self.tabs.iter().position(|t| t.iter().any(|x| x == id)) {
            self.active = i;
            self.focused = Some(id.to_owned());
        }
    }

    fn select(&mut self, tab: usize, window: &mut Window, cx: &mut Context<Self>) {
        if tab >= self.tabs.len() {
            return;
        }
        self.active = tab;
        if let Some(id) = self.active_terminal() {
            if let Some(v) = self.views.get(&id) {
                gpui::Focusable::focus_handle(v, cx).focus(window, cx);
            }
            cx.emit(TerminalWindowEvent::Selected(id));
        }
        cx.notify();
    }

    fn run(command: &'static str, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command, args)), cx);
    }
}

fn toolbar_button(id: &'static str, label: &'static str, t: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px_2()
        .text_size(t.typography.ui)
        .text_color(t.text)
        .cursor_pointer()
        .hover(|s| s.bg(t.menu_hover))
        .child(label)
}

impl Render for TerminalWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let active_terminal = self.active_terminal();
        let mut strip = div()
            .id("terminal-tabs")
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .h(t.typography.tab_height)
            .overflow_x_scroll();
        for (i, tab) in self.tabs.iter().enumerate() {
            let active = i == self.active;
            let title: String = tab
                .iter()
                .map(|id| self.names.get(id).cloned().unwrap_or_else(|| id.clone()))
                .collect::<Vec<_>>()
                .join(" | ");
            let first = tab[0].clone();
            let sel = tab_selector(&first);
            let close_sel = tab_close_selector(&first);
            let agent = tab
                .iter()
                .find_map(|id| self.agents.get(id).map(|a| (id.clone(), a.clone())));
            let mut el = eludite_ui::tab(
                &t,
                title,
                eludite_ui::elements::TabStyle {
                    active,
                    ..Default::default()
                },
            )
            .id(SharedString::from(sel.clone()))
            .debug_selector(move || sel)
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |w, _, window, cx| w.select(i, window, cx)));
            if let Some((id, a)) = agent {
                let asel = agent_selector(&id);
                el = el.child(
                    div()
                        .id(SharedString::from(asel.clone()))
                        .debug_selector(move || asel)
                        .italic()
                        .text_size(t.typography.small)
                        .child(format!("Agent {a} is typing")),
                );
            }
            let ids: Vec<String> = tab.clone();
            el = el.child(
                div()
                    .id(SharedString::from(close_sel.clone()))
                    .debug_selector(move || close_sel)
                    .px_1()
                    .hover(|s| s.bg(t.menu_hover))
                    .child("\u{2715}")
                    .on_click(cx.listener(move |_, _, window, cx| {
                        cx.stop_propagation();
                        for id in &ids {
                            Self::run(cmds::CLOSE, json!({ "terminal": id }), window, cx);
                        }
                    })),
            );
            strip = strip.child(el);
        }
        let target = active_terminal.clone();
        let target2 = active_terminal.clone();
        let target3 = active_terminal.clone();
        let toolbar = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1()
            .child(toolbar_button(NEW, "+ New Terminal", &t).on_click(
                cx.listener(|_, _, window, cx| Self::run(cmds::OPEN, json!({}), window, cx)),
            ))
            .child(
                toolbar_button(PROFILES, "\u{25BE}", &t).on_click(cx.listener(|w, _, _, cx| {
                    w.dropdown = !w.dropdown;
                    cx.notify();
                })),
            )
            .child(toolbar_button(SPLIT, "Split", &t).on_click(cx.listener(
                move |_, _, window, cx| match &target {
                    Some(id) => Self::run(cmds::OPEN, json!({ "split_with": id }), window, cx),
                    None => Self::run(cmds::OPEN, json!({}), window, cx),
                },
            )))
            .child(toolbar_button(KILL, "Kill", &t).on_click(cx.listener(
                move |_, _, window, cx| {
                    if let Some(id) = &target2 {
                        Self::run(cmds::CLOSE, json!({ "terminal": id }), window, cx);
                    }
                },
            )))
            .child(toolbar_button(CLEAR, "Clear", &t).on_click(cx.listener(
                move |_, _, window, cx| {
                    if let Some(id) = &target3 {
                        Self::run(cmds::CLEAR, json!({ "terminal": id }), window, cx);
                    }
                },
            )));
        let dropdown = self.dropdown.then(|| {
            let mut menu = eludite_ui::popup::popup_panel(&t)
                .id("terminal-profile-menu")
                .debug_selector(|| "terminal-profile-menu".into())
                .absolute()
                .top(t.typography.tab_height)
                .right_0()
                .py_1()
                .min_w(px(200.))
                .text_size(t.typography.ui)
                .on_mouse_down_out(cx.listener(|w, _, _, cx| {
                    w.dropdown = false;
                    cx.notify();
                }));
            for name in &self.profiles {
                let sel = profile_selector(name);
                let n = name.clone();
                menu = menu.child(
                    eludite_ui::menu_row(sel, name.clone(), 0, false, false, false, &t).on_click(
                        cx.listener(move |w, _, window, cx| {
                            w.dropdown = false;
                            Self::run(cmds::OPEN, json!({ "profile": n }), window, cx);
                            cx.notify();
                        }),
                    ),
                );
            }
            menu
        });
        let header = div()
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .w_full()
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .child(strip)
            .child(toolbar)
            .children(dropdown);
        let body: AnyElement = match self.tabs.get(self.active) {
            Some(tab) => {
                let mut row = div().flex().flex_row().flex_1().min_h_0().size_full();
                for (i, id) in tab.iter().enumerate() {
                    if let Some(v) = self.views.get(id) {
                        let sel = pane_selector(id);
                        let pane = div()
                            .id(SharedString::from(sel.clone()))
                            .debug_selector(move || sel)
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .when(i > 0, |d| d.border_l_1().border_color(t.border))
                            .child(v.clone());
                        row = row.child(pane);
                    }
                }
                row.into_any_element()
            }
            None => div()
                .id("terminal-empty")
                .debug_selector(|| "terminal-empty".into())
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(t.text_muted)
                .text_size(t.typography.ui)
                .child("No terminal is open.")
                .child(
                    toolbar_button("terminal-empty-new", "New Terminal (Ctrl+Shift+`)", &t)
                        .text_color(t.accent)
                        .on_click(cx.listener(|_, _, window, cx| {
                            Self::run(cmds::OPEN, json!({}), window, cx)
                        })),
                )
                .into_any_element(),
        };
        div()
            .id("terminal-window")
            .flex()
            .flex_col()
            .size_full()
            .bg(t.panel)
            .child(header)
            .child(body)
    }
}

use gpui::prelude::FluentBuilder as _;

// ----- The shell's half -----

/// The terminal half of the shell.
pub struct TerminalUi {
    pub service: Arc<TerminalService>,
    pub window: Entity<TerminalWindow>,
    settings: ViewSettings,
    linger: HashMap<String, Task<()>>,
    /// The person said yes to closing the workspace with commands running.
    close_confirmed: bool,
}

impl TerminalUi {
    pub fn new(service: Arc<TerminalService>, theme: Theme, cx: &mut Context<Shell>) -> Self {
        Self {
            service,
            window: cx.new(|_| TerminalWindow::new(theme)),
            settings: ViewSettings::default(),
            linger: HashMap::new(),
            close_confirmed: false,
        }
    }
}

impl Shell {
    /// Wire the Terminal window (called once, at the end of [`Shell::new`]).
    pub(super) fn terminal_install(
        &mut self,
        mut events: UnboundedReceiver<TerminalEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use futures::StreamExt as _;
        cx.observe(&self.terminal.window, |_, _, cx| cx.notify())
            .detach();
        cx.subscribe(
            &self.terminal.window,
            |shell, _, e: &TerminalWindowEvent, _| match e {
                TerminalWindowEvent::Selected(id) => shell.terminal.service.set_active(id),
            },
        )
        .detach();
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        for e in batch {
                            shell.on_terminal_event(e, window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self._tasks.push(task);
        self.terminal_refresh_profiles(cx);
    }

    /// The profiles' names for the dropdown, found off the UI thread.
    fn terminal_refresh_profiles(&mut self, cx: &mut Context<Self>) {
        let service = self.terminal.service.clone();
        let work = cx.background_spawn(async move { service.profile_names() });
        let window = self.terminal.window.clone();
        cx.spawn(async move |_, cx| {
            let names = work.await;
            window.update(cx, |w, cx| w.set_profiles(names, cx));
        })
        .detach();
    }

    fn on_terminal_event(
        &mut self,
        event: TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalEvent::Opened {
                id,
                name,
                split_with,
                replace,
                focus,
            } => {
                let Some(terminal) = self.terminal.service.handle(&id) else {
                    return;
                };
                let theme = self.theme;
                let settings = self.terminal.settings.clone();
                let view = cx.new(|cx| TerminalView::new(terminal, theme, settings, cx));
                let vid = id.clone();
                cx.subscribe_in(
                    &view,
                    window,
                    move |shell, _, e: &TerminalViewEvent, window, cx| {
                        shell.on_terminal_view_event(&vid, e, window, cx)
                    },
                )
                .detach();
                self.terminal.window.update(cx, |w, cx| {
                    w.add(
                        &id,
                        &name,
                        view.clone(),
                        split_with.as_deref(),
                        replace.as_deref(),
                        cx,
                    )
                });
                // The window shows what opened (an agent's terminal too: the person sees what it types); the
                // person's terminal takes the focus.
                let _ = self.invoke(
                    "eludite.view.show",
                    json!({ "id": ids::TERMINAL }),
                    window,
                    cx,
                );
                if focus {
                    gpui::Focusable::focus_handle(&view, cx).focus(window, cx);
                }
                self.dock.update(cx, |_, cx| cx.notify());
            }
            TerminalEvent::Changed(id) => {
                if let Some(v) = self.terminal.window.read(cx).view(&id).cloned() {
                    v.update(cx, |v, cx| v.changed(cx));
                }
            }
            TerminalEvent::Bell(id) => {
                if let Some(v) = self.terminal.window.read(cx).view(&id).cloned() {
                    v.update(cx, |v, cx| v.bell(cx));
                }
            }
            TerminalEvent::Exited(id, _) => {
                if let Some(v) = self.terminal.window.read(cx).view(&id).cloned() {
                    v.update(cx, |v, cx| v.changed(cx));
                }
            }
            TerminalEvent::Closed(id) => {
                self.terminal.linger.remove(&id);
                self.terminal.window.update(cx, |w, cx| w.remove(&id, cx));
                if let Some(next) = self.terminal.window.read(cx).active_terminal() {
                    self.terminal.service.set_active(&next);
                }
            }
            TerminalEvent::Agent(id, Some(agent)) => {
                self.terminal.linger.remove(&id);
                self.terminal
                    .window
                    .update(cx, |w, cx| w.set_agent(&id, Some(agent), cx));
            }
            TerminalEvent::Agent(id, None) => {
                let window_entity = self.terminal.window.clone();
                let tid = id.clone();
                let task = cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(AGENT_LINGER).await;
                    window_entity.update(cx, |w, cx| w.set_agent(&tid, None, cx));
                });
                self.terminal.linger.insert(id, task);
            }
        }
        cx.notify();
    }

    fn on_terminal_view_event(
        &mut self,
        id: &str,
        event: &TerminalViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalViewEvent::Typed => {
                self.terminal.service.person_typed(id);
                self.terminal.service.set_active(id);
                self.terminal.window.update(cx, |w, _| w.set_focused(id));
            }
            TerminalViewEvent::FocusEditor => {
                if let Some(doc) = self.controller.active_document()
                    && let Some(view) = self.views.borrow().get(&doc).cloned()
                {
                    gpui::Focusable::focus_handle(&view, cx).focus(window, cx);
                } else {
                    gpui::Focusable::focus_handle(self, cx).focus(window, cx);
                }
            }
            TerminalViewEvent::Restart => {
                self.run(cmds::OPEN, json!({ "replace": id }), window, cx);
            }
            TerminalViewEvent::OpenLink(target) => {
                self.terminal_open_link(id, target.clone(), window, cx)
            }
        }
    }

    /// Ctrl+click on a link: a file opens in the editor at its position, a folder shows in the Workspace, a url
    /// in the system's browser. The path is checked off the UI thread.
    fn terminal_open_link(
        &mut self,
        id: &str,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            Target::Url(url) => self.run(
                eludite_commands::browser::OPEN_EXTERNAL,
                json!({ "url": url }),
                window,
                cx,
            ),
            Target::Path { path, line, column } => {
                let base = self.terminal.service.cwd_of(id);
                let home = std::env::var("HOME").ok().map(PathBuf::from);
                let candidate = resolve_link(&path, base.as_deref(), home.as_deref());
                let check = cx.background_spawn(async move {
                    let kind = std::fs::metadata(&candidate).ok().map(|m| m.is_dir());
                    (candidate, kind)
                });
                cx.spawn_in(window, async move |this, cx| {
                    let (path, kind) = check.await;
                    let _ = this.update_in(cx, |shell, window, cx| match kind {
                        Some(false) => {
                            let mut args = json!({ "path": path.to_string_lossy() });
                            if let Some(l) = line {
                                args["line"] = json!(l);
                                args["column"] = json!(column.unwrap_or(1));
                            }
                            shell.run(workspace::FILE_OPEN, args, window, cx);
                        }
                        Some(true) => {
                            let _ = shell.invoke(
                                "eludite.view.show",
                                json!({ "id": ids::WORKSPACE }),
                                window,
                                cx,
                            );
                            shell
                                .explorer
                                .update(cx, |e, cx| e.reveal_folder(&path, cx));
                            shell
                                .status
                                .set(slots::STATE, format!("{}", path.display()));
                            cx.notify();
                        }
                        None => {
                            shell
                                .status
                                .set(slots::STATE, format!("No such file: {}", path.display()));
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// `eludite.terminal.*` and the Terminal window's View item from the UI: run off the UI thread, asking first
    /// before killing a running command or closing the workspace over one. Returns false for other commands.
    pub(super) fn run_terminal(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        // View > Terminal (Ctrl+`): the window, with a terminal in it.
        if command == "eludite.view.show"
            && args.get("id").and_then(Value::as_str) == Some(ids::TERMINAL)
        {
            let _ = self.invoke(command, args.clone(), window, cx);
            match self.terminal.window.read(cx).active_terminal() {
                Some(id) => {
                    if let Some(v) = self.terminal.window.read(cx).view(&id).cloned() {
                        gpui::Focusable::focus_handle(&v, cx).focus(window, cx);
                    }
                }
                None if self.terminal.service.ids().is_empty() => {
                    self.terminal_spawn(cmds::OPEN.into(), json!({}), window, cx);
                }
                None => {}
            }
            cx.notify();
            return true;
        }
        if command == workspace::WORKSPACE_CLOSE && !self.terminal.close_confirmed {
            let busy = self.terminal.service.busy();
            if let Some((name, what)) = busy.first() {
                let answer = window.prompt(
                    PromptLevel::Warning,
                    &format!("{what} is running in the terminal '{name}'. Close the workspace and end it?"),
                    Some("Closing the workspace ends its terminals."),
                    &["Yes", "No"],
                    cx,
                );
                let args = std::mem::take(args);
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(0) = answer.await {
                        let _ = this.update_in(cx, |shell, window, cx| {
                            shell.terminal.close_confirmed = true;
                            shell.run(workspace::WORKSPACE_CLOSE, args, window, cx);
                            shell.terminal.close_confirmed = false;
                        });
                    }
                })
                .detach();
                return true;
            }
            return false;
        }
        if !cmds::ALL.contains(&command) {
            return false;
        }
        let args = std::mem::take(args);
        // Kill Terminal over a running command asks first (Visual Studio's question).
        if command == cmds::CLOSE && args.get("kill").and_then(Value::as_bool) != Some(true) {
            let id = args
                .get("terminal")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| self.terminal.service.active());
            if let Some(t) = id.as_deref().and_then(|i| self.terminal.service.handle(i))
                && t.busy()
            {
                let what = t.foreground().unwrap_or_else(|| "A command".into());
                let answer = window.prompt(
                    PromptLevel::Warning,
                    &format!("{what} is running in this terminal. Kill it?"),
                    None,
                    &["Yes", "No"],
                    cx,
                );
                let mut args = args;
                args["kill"] = json!(true);
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(0) = answer.await {
                        let _ = this.update_in(cx, |shell, window, cx| {
                            shell.terminal_spawn(cmds::CLOSE.into(), args, window, cx)
                        });
                    }
                })
                .detach();
                return true;
            }
        }
        self.terminal_spawn(command.to_owned(), args, window, cx);
        true
    }

    /// Invoke `command` on the bus off the UI thread; a failure goes to the status bar.
    fn terminal_spawn(
        &mut self,
        command: String,
        args: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = self.commands.clone();
        let work = cx.background_spawn(async move {
            let r = commands.invoke(&command, args);
            (command, r)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (command, r) = work.await;
            let _ = this.update(cx, |shell, cx| {
                let text = match r {
                    Ok(_) => "Ready".to_owned(),
                    Err(e) => {
                        eprintln!("eludite: {command}: {e}");
                        match e {
                            CommandError::Failed(m) | CommandError::InvalidInput(m) => m,
                            other => other.to_string(),
                        }
                    }
                };
                shell.status.set(slots::STATE, text);
                cx.notify();
            });
        })
        .detach();
    }

    /// `terminal.*` from the settings store.
    pub(super) fn terminal_apply_settings(&mut self, cx: &mut Context<Self>) {
        let (term, view) = {
            let s = self.settings.lock();
            let profiles =
                serde_json::from_value::<Vec<Profile>>(s.effective("terminal.profiles").0)
                    .unwrap_or_default();
            let term = TermSettings {
                default_profile: s.string("terminal.defaultProfile"),
                profiles,
                scrollback: s
                    .effective("terminal.scrollback")
                    .0
                    .as_u64()
                    .unwrap_or(10_000) as usize,
                inherit_tool_paths: s.bool("terminal.inheritToolPaths"),
                shell_integration: s.bool("terminal.shellIntegration"),
                cargo: s.path("build.cargoPath"),
            };
            let view = ViewSettings {
                font_size: px(s.effective("terminal.fontSize").0.as_u64().unwrap_or(13) as f32),
                copy_on_select: s.bool("terminal.copyOnSelect"),
                visual_bell: s.string("terminal.bell") != "none",
                ..ViewSettings::default()
            };
            (term, view)
        };
        let profiles_changed = {
            let s = self.terminal.service.state();
            s.settings.profiles != term.profiles
                || s.settings.default_profile != term.default_profile
        };
        self.terminal.service.set_settings(term);
        if profiles_changed {
            self.terminal_refresh_profiles(cx);
        }
        if self.terminal.settings != view {
            self.terminal.settings = view.clone();
            let views: Vec<_> = self
                .terminal
                .window
                .read(cx)
                .views()
                .map(|(_, v)| v.clone())
                .collect();
            for v in views {
                let s = view.clone();
                v.update(cx, |v, cx| v.set_settings(s, cx));
            }
        }
    }

    /// The workspace changed: new terminals start in its folder.
    pub(super) fn terminal_workspace_changed(&mut self, folder: Option<&Path>) {
        self.terminal.service.set_workspace(folder);
    }

    /// The workspace closes: its terminals end.
    pub(super) fn terminal_close_all(&mut self) {
        self.terminal.service.close_all();
    }

    #[cfg(all(test, unix))]
    pub fn terminal_ui(&self) -> &TerminalUi {
        &self.terminal
    }
}

impl TerminalUi {
    /// The settings the views draw with.
    #[cfg(all(test, unix))]
    pub fn view_settings(&self) -> Option<&ViewSettings> {
        Some(&self.settings)
    }
}

/// A link's path: absolute as it is, `~/` under home, else relative to the terminal's folder.
pub fn resolve_link(path: &str, base: Option<&Path>, home: Option<&Path>) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    if let (Some(rest), Some(h)) = (path.strip_prefix("~/"), home) {
        return h.join(rest);
    }
    match base {
        Some(b) => b.join(p),
        None => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_resolve_against_the_terminals_folder() {
        let base = Path::new("/w/src");
        assert_eq!(
            resolve_link("lib.rs", Some(base), None),
            PathBuf::from("/w/src/lib.rs")
        );
        assert_eq!(
            resolve_link("/abs/x.cs", Some(base), None),
            PathBuf::from("/abs/x.cs")
        );
        assert_eq!(
            resolve_link("~/notes.md", Some(base), Some(Path::new("/home/me"))),
            PathBuf::from("/home/me/notes.md")
        );
    }

    #[test]
    fn the_reserved_keys_are_in_the_shells_keymap() {
        let keymap = eludite_ui::vs_keymap();
        for k in RESERVED_KEYS {
            assert!(
                keymap.iter().any(|b| b.keystrokes == k),
                "{k} is not a shell key"
            );
        }
        // The keys a shell needs go to the terminal.
        for k in [
            "ctrl-r ctrl-r",
            "ctrl-s",
            "ctrl-z",
            "ctrl-f",
            "ctrl-space",
            "f12",
        ] {
            assert!(!RESERVED_KEYS.contains(&k), "{k}");
        }
    }
}
