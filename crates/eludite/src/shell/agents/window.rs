//! The Agents tool window (View > Agents, Ctrl+\, Ctrl+C): the agent picker and its state, login instructions, the
//! virtualized transcript, the permission prompt, the pending changes, and the prompt box (Enter sends, Shift+Enter
//! starts a new line, Escape cancels the turn). It renders and emits [`AgentsWindowEvent`]s; the shell turns them
//! into `eludite.agents.*` commands, and all agent and MCP I/O happens on other threads. A tool call's images show
//! as thumbnails under its card (brief 0024); clicking one asks the shell to open the full image. An agent's debug
//! command reads as one line above its card (brief 0027): the action and the result as the person would see them, the
//! stop's location a link that opens the file at the line, and the summary the agent received folded until expanded.
//! The agent's messages are Markdown (brief 0043); a click on a link in them emits [`AgentsWindowEvent::OpenLink`].
//!
//! Brief 0057: the prompt box is an `eludite_editor::TextInput` (caret, selection, clipboard, undo, wrapping, 2 to 8
//! rows then scrolling) that keeps its text and caret across turns and while the window is hidden. Submitting pushes
//! the prompt onto an in-memory history (the last [`HISTORY_LIMIT`]); Up on the first row of an empty or unedited box
//! recalls the previous prompt, Down the next, then the empty box. Typing `/` at the start opens the slash menu of the
//! agent's commands ([`filter_commands`]), above the box: Up and Down select, Tab and Enter complete (Enter sends
//! when the typed name is already complete), Escape and a click outside close it.
//!
//! Brief 0058: a footer under the box holds the pickers ([`pickers`]: the model, the effort, any other select option
//! the agent offers, then the mode) and the Send/Stop button. Each picker is a flat `Name ▾` button opening a list of
//! the choices, the current one checked, each with its description; Up, Down, Enter and Escape work while it is open
//! (the prompt box keeps the focus and the footer takes those keys first). A pick emits
//! [`AgentsWindowEvent::Configure`]; the button shows the picked name muted until the agent answers. While a turn
//! runs the pickers are muted, open nothing and say "Wait for the turn to end".
//!
//! Brief 0059: the header is one row (the agent picker, the state as a colored dot and word whose tooltip is the
//! agent's version and MCP endpoint, the Start/Restart icon button); a bordered block under it only for the login
//! instructions and an error. Tool calls are one line each (the kind's glyph, the adapter's title, the status badge,
//! a chevron), collapsed until clicked; a running turn shows a status line with its elapsed time and a spinner, and
//! the usage strip above the prompt box shows the session's context and cost ([`USAGE_STRIP`]). The spinner and the
//! elapsed time are redrawn by a timer that runs only while a turn runs ([`AgentsWindow::ticking`]): an idle window
//! requests no frames. Every color comes from the theme.
//!
//! Brief 0060: the agent picker's list ends with "Add server…" ([`ADD_SERVER_ITEM`]), which asks the shell to open
//! the Add server dialog ([`super::providers::ProviderDialog`]); the window draws the dialog while it is open.
//!
//! Brief 0061: the window shows one session of several. What it shows of a session (the transcript and its list
//! state, the permission prompt, the header, the pickers and their pending picks, the running turn's start, the prompt
//! box's text, whether the box is disabled) is a [`SessionView`]: the shell swaps the shown session's view for
//! another's ([`AgentsWindow::show_view`]), and briefly for a session off screen while it applies that session's events
//! ([`AgentsWindow::swap_view`]). The header gains the history button ([`HISTORY_BUTTON`], a clock, "Sessions") and
//! the New session button ([`NEW_BUTTON`], `+`): the history list ([`HISTORY_MENU`], rows [`session_item`]) shows each
//! session's title, agent and when it last changed, a dot while it runs and `?` while it waits for an answer, the shown
//! one checked; Up, Down, Enter and Escape work while it is open, and a row emits [`AgentsWindowEvent::Switch`]. With a
//! session shown, picking another agent in the agent picker emits [`AgentsWindowEvent::NewSession`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eludite_acp::LoginMethod;
use eludite_acp::protocol::{AvailableCommand, SessionConfigOption, SessionModeState};
use eludite_editor::{EditorStyle, TextInput, TextInputEvent, input_actions};
use eludite_ui::Theme;
use eludite_ui::popup::{COMPLETION_ROWS, CompletionKind, completion_row, popup_panel};
use eludite_ui::transcript::{
    SPINNER_STEP, ToolCard, ToolStatus, UsageStrip, agent_block, clip_lines, elapsed_text, notice,
    plan_card, spinner_frame, status_line, thought_block, tool_call_card, usage_line, usage_strip,
    user_prompt,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, FollowMode, FontWeight, HighlightStyle, ImageSource,
    InteractiveElement, IntoElement, ListAlignment, ListState, ParentElement, Pixels, Render, Rgba,
    SharedString, Stateful, StatefulInteractiveElement, Styled, StyledText, Subscription, Task,
    Window, anchored, canvas, deferred, div, img, list, px, relative,
};
use serde_json::Value;

use super::transcript::{Row, Transcript};

/// What the user asked for in the window.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentsWindowEvent {
    Start {
        agent: Option<String>,
        restart: bool,
    },
    Prompt(String),
    Cancel,
    /// Answer permission request `request`.
    Answer {
        request: u64,
        decision: Decision,
    },
    /// Accept or reject pending change `change` (`None`: all of them).
    Review {
        change: Option<u64>,
        accept: bool,
    },
    /// Open the review view of a pending change.
    OpenChange(u64),
    /// Open image `index` of tool call `tool_call` in full.
    OpenImage {
        tool_call: String,
        index: usize,
    },
    /// Open `path` at `line` (a debug row's stop location; brief 0027).
    OpenLocation {
        path: String,
        line: u32,
    },
    /// Follow a link in the agent's message: its target as written.
    OpenLink(String),
    /// A picker's choice (brief 0058): `option` is `mode` or a config option's id.
    Configure {
        option: String,
        value: String,
    },
    /// "Add server…" at the end of the agent picker (brief 0060).
    AddServer,
    /// Show session `id` (brief 0061): a row of the history list.
    Switch(String),
    /// A new session (the `+` button, or another agent picked while a session is shown).
    NewSession {
        agent: Option<String>,
    },
    /// The history list opened: the shell refreshes its rows.
    HistoryOpened,
}

/// A permission answer (`agents-permission.input.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    AlwaysAllow,
    Deny,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::AlwaysAllow => "always_allow",
            Decision::Deny => "deny",
        }
    }
}

/// The agent's state as the header shows it (`agents-state.output.json`'s `state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateKind {
    #[default]
    Stopped,
    Starting,
    Ready,
    NeedsLogin,
    Running,
    Error,
}

impl StateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StateKind::Stopped => "stopped",
            StateKind::Starting => "starting",
            StateKind::Ready => "ready",
            StateKind::NeedsLogin => "needs_login",
            StateKind::Running => "running",
            StateKind::Error => "error",
        }
    }

    fn label(self) -> &'static str {
        match self {
            StateKind::Stopped => "Stopped",
            StateKind::Starting => "Starting\u{2026}",
            StateKind::Ready => "Ready",
            StateKind::NeedsLogin => "Needs login",
            StateKind::Running => "Running",
            StateKind::Error => "Error",
        }
    }

    fn color(self, t: &Theme) -> Rgba {
        match self {
            StateKind::Ready => t.success,
            StateKind::Running | StateKind::Starting => ToolStatus::Running.color(t),
            StateKind::NeedsLogin => t.warning,
            StateKind::Error => ToolStatus::Failed.color(t),
            StateKind::Stopped => t.text_muted,
        }
    }
}

/// What the header shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeaderState {
    pub agents: Vec<String>,
    pub selected: usize,
    pub state: StateKind,
    /// The agent's own name and version and the MCP endpoint (the state's tooltip), the login label or the error (a
    /// block under the header).
    pub detail: String,
    pub login: Vec<LoginMethod>,
    /// The session shown (brief 0061): its id and title; `None` before the first.
    pub session: Option<(String, String)>,
}

/// One row of the history list (brief 0061).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRow {
    pub id: String,
    pub title: String,
    pub agent: String,
    /// When it last changed, in words (`2 min ago`, `yesterday`, the date).
    pub when: String,
    pub running: bool,
    pub waiting: bool,
    pub current: bool,
}

/// What the window shows of one session (brief 0061), swapped in when the session is shown.
pub struct SessionView {
    pub transcript: Transcript,
    list: ListState,
    pub prompt: Option<Prompt>,
    pub header: HeaderState,
    modes: Option<SessionModeState>,
    config_options: Vec<SessionConfigOption>,
    picker_list: Vec<Picker>,
    pending: HashMap<String, String>,
    open_picker: Option<(String, usize)>,
    turn_started: Option<Instant>,
    /// The prompt box's text while the session is not shown.
    pub draft: String,
    /// Why the prompt box is disabled (a stored session its agent cannot resume).
    pub disabled: Option<String>,
}

impl Default for SessionView {
    fn default() -> Self {
        Self::new(Transcript::default())
    }
}

impl SessionView {
    /// A session's view of `transcript`, its list at the end.
    pub fn new(transcript: Transcript) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            transcript,
            list,
            prompt: None,
            header: HeaderState::default(),
            modes: None,
            config_options: Vec::new(),
            picker_list: Vec::new(),
            pending: HashMap::new(),
            open_picker: None,
            turn_started: None,
            draft: String::new(),
            disabled: None,
        }
    }
}

/// The history button in the header (brief 0061), its list, and the New session button.
pub const HISTORY_BUTTON: &str = "agents-history";
pub const HISTORY_MENU: &str = "agents-history-menu";
pub const NEW_BUTTON: &str = "agents-new";
pub const HISTORY_TIP: &str = "Sessions";
pub const NEW_TIP: &str = "New session";
/// The history list's last line when there are more sessions than it shows.
pub const OLDER_ON_DISK: &str = "Older sessions are on disk";

/// A row of the history list.
pub fn session_item(id: &str) -> String {
    format!("agents-session-{id}")
}

/// A pending change as the window lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeItem {
    pub id: u64,
    pub path: String,
    pub summary: String,
    pub pending: bool,
}

/// The permission prompt (the oldest request waiting).
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub request: u64,
    /// The tool's own name (`Bash`).
    pub tool: String,
    /// What the call does as the adapter titled it (`ls`), bold in the sentence (brief 0059).
    pub title: String,
    pub class: String,
    pub detail: String,
    /// Whether Always Allow can persist (a solution is open, so there is a policy file, and the call's escalation
    /// lets it remember something).
    pub can_persist: bool,
    /// Always Allow is "Allow for this session": a grant until the agent session ends, nothing written (brief 0041).
    pub session: bool,
    /// Why the call's class was raised above its command's (ADR-0009).
    pub reason: Option<String>,
}

/// Where the window's and the review views' buttons were last painted, by element id (for the real-input driver,
/// `--bounds-out`).
pub type Painted = Rc<RefCell<HashMap<String, Bounds<Pixels>>>>;

/// `el`, recording its painted bounds in `painted` under `id`.
pub fn tracked(painted: &Painted, id: impl Into<String>, el: Stateful<Div>) -> Stateful<Div> {
    let painted = painted.clone();
    let id = id.into();
    el.relative().child(
        canvas(
            move |b, _, _| {
                painted.borrow_mut().insert(id.clone(), b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// Frame probes for the streaming benchmark.
#[derive(Debug, Default)]
pub struct Probes {
    pub recording: bool,
    pub frame_work_ms: Vec<f64>,
}

pub const PROMPT_BOX: &str = "agents-prompt";
/// The slash-command menu (brief 0057), its rows by command name, and the line describing the selected one.
pub const SLASH_MENU: &str = "agents-slash-menu";
pub const SLASH_DETAIL: &str = "agents-slash-detail";
/// The slash menu's widest width: it is as wide as the prompt box up to this, never wider than the box.
pub const SLASH_MENU_WIDTH: f32 = 480.;
/// The prompt box's placeholder.
pub const PLACEHOLDER: &str = "Ask the agent\u{2026} (Enter to send, Shift+Enter for a new line, / for commands, Esc to stop)";
/// The prompts the history keeps (in memory, for the session).
pub const HISTORY_LIMIT: usize = 50;

pub fn slash_item(name: &str) -> String {
    format!("agents-slash-item-{name}")
}
pub const SEND_BUTTON: &str = "agents-send";
pub const START_BUTTON: &str = "agents-start";
pub const AGENT_PICKER: &str = "agents-picker";

/// The agent picker's last row, "Add server…" (brief 0060).
pub const ADD_SERVER_ITEM: &str = "agents-picker-add-server";

pub fn agent_item(ix: usize) -> String {
    format!("agents-agent-{ix}")
}

/// The mode picker's button (brief 0058); its rows are [`mode_item`].
pub const MODE_PICKER: &str = "agents-mode";
/// The open picker's list.
pub const PICKER_MENU: &str = "agents-option-menu";
/// The open picker's narrowest width, when the footer is at least that wide.
pub const PICKER_MIN_WIDTH: f32 = 220.;
/// The footer under the prompt box: the pickers and Send.
pub const FOOTER: &str = "agents-footer";
/// The pickers' tooltip while a turn runs.
pub const WAIT_FOR_TURN: &str = "Wait for the turn to end";
/// `eludite.agents.configure`'s `option` for the mode.
pub const MODE_KEY: &str = eludite_commands::agents::MODE_OPTION;

pub fn mode_item(id: &str) -> String {
    format!("agents-mode-{id}")
}

/// A config option's picker button.
pub fn option_picker(id: &str) -> String {
    format!("agents-option-{id}")
}

/// A row of a config option's picker.
pub fn option_item(id: &str, value: &str) -> String {
    format!("agents-option-{id}-{value}")
}

/// One choice of a [`Picker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

/// One picker of the footer: the session's mode, or one select config option (brief 0058).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    /// [`MODE_KEY`] or the option's id: `eludite.agents.configure`'s `option`.
    pub key: String,
    pub is_mode: bool,
    /// The option's name (`Model`), for messages.
    pub title: String,
    /// The option's category (`model`, `thought_level`, ...).
    pub category: Option<String>,
    pub current: String,
    pub choices: Vec<Choice>,
}

impl Picker {
    pub fn button_id(&self) -> String {
        if self.is_mode {
            MODE_PICKER.into()
        } else {
            option_picker(&self.key)
        }
    }

    pub fn row_id(&self, value: &str) -> String {
        if self.is_mode {
            mode_item(value)
        } else {
            option_item(&self.key, value)
        }
    }

    /// The name of `value` (the value itself when it is not listed).
    pub fn name_of(&self, value: &str) -> String {
        self.choices
            .iter()
            .find(|c| c.value == value)
            .map_or_else(|| value.to_owned(), |c| c.name.clone())
    }

    pub fn has(&self, value: &str) -> bool {
        self.choices.iter().any(|c| c.value == value)
    }
}

/// The footer's pickers, left to right: the model options, the effort (thought level) options, the agent's other
/// select options, then the mode. Options of another kind are not shown, nor a config option of category `mode` when
/// the agent also offers modes (the Node Claude Code adapter offers both).
pub fn pickers(modes: Option<&SessionModeState>, options: &[SessionConfigOption]) -> Vec<Picker> {
    let rank = |category: Option<&str>| match category {
        Some("model") => 0,
        Some("thought_level") => 1,
        Some("mode") => 3,
        _ => 2,
    };
    let mut out: Vec<Picker> = options
        .iter()
        .filter(|o| !(modes.is_some() && o.category.as_deref() == Some("mode")))
        .filter_map(|o| {
            let s = o.as_select()?;
            Some(Picker {
                key: o.id.clone(),
                is_mode: false,
                title: o.name.clone(),
                category: o.category.clone(),
                current: s.current_value.clone(),
                choices: s
                    .options
                    .iter()
                    .map(|c| Choice {
                        value: c.value.clone(),
                        name: c.name.clone(),
                        description: c.description.clone(),
                    })
                    .collect(),
            })
        })
        .collect();
    out.sort_by_key(|p| rank(p.category.as_deref()));
    if let Some(m) = modes {
        out.push(Picker {
            key: MODE_KEY.into(),
            is_mode: true,
            title: "Mode".into(),
            category: Some("mode".into()),
            current: m.current_mode_id.clone(),
            choices: m
                .available_modes
                .iter()
                .map(|m| Choice {
                    value: m.id.clone(),
                    name: m.name.clone(),
                    description: m.description.clone(),
                })
                .collect(),
        });
    }
    out
}

/// A plain tooltip: a picker's while a turn runs, the state's, Restart's, a tool card's, the usage strip's.
struct TextTip {
    text: SharedString,
    theme: Theme,
}

/// A tooltip builder saying `text`.
fn tip(text: impl Into<SharedString>, theme: Theme) -> impl Fn(&mut Window, &mut App) -> AnyView {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| TextTip {
            text: text.clone(),
            theme,
        })
        .into()
    }
}

impl Render for TextTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .px_2()
            .py_1()
            .bg(t.panel)
            .border_1()
            .border_color(t.border)
            .text_size(t.typography.ui)
            .text_color(t.text)
            .child(self.text.clone())
    }
}

/// The usage strip above the prompt box (brief 0059).
pub const USAGE_STRIP: &str = "agents-usage";
/// The status line of a running turn (brief 0059).
pub const STATUS_LINE: &str = "agents-status";
/// The state in the header, whose tooltip is the agent's version and MCP endpoint.
pub const STATE_LABEL: &str = "agents-state";
/// The Start/Restart button's tooltips.
pub const RESTART_TIP: &str = "Restart the agent";
pub const START_TIP: &str = "Start the agent";
/// The most lines of a tool call's arguments an expanded card shows.
pub const ARGUMENT_LINES: usize = 40;

/// The permission prompt's sentence (brief 0059), `Claude Code wants to run ls (execute).`, and where the call's
/// title is in it (drawn bold).
pub fn permission_sentence(
    agent: &str,
    title: &str,
    class: &str,
    reason: Option<&str>,
) -> (String, std::ops::Range<usize>) {
    let head = format!("{agent} wants to run ");
    let bold = head.len()..head.len() + title.len();
    let tail = match reason {
        Some(r) => format!(" ({class}: {r})."),
        None => format!(" ({class})."),
    };
    (format!("{head}{title}{tail}"), bold)
}

/// What the transcript says when a turn ends with `stop` (brief 0059): nothing for `end_turn`, else in words.
pub fn stop_notice(stop: &str) -> Option<String> {
    Some(match stop {
        "end_turn" => return None,
        "cancelled" => "Stopped".into(),
        "max_tokens" => "The model reached its output limit".into(),
        "max_turn_requests" => "The agent reached its request limit".into(),
        "refusal" => "The model declined to continue".into(),
        other => other.to_owned(),
    })
}

pub fn tool_card(ix: usize) -> String {
    format!("agents-tool-{ix}")
}

pub fn thought(ix: usize) -> String {
    format!("agents-thought-{ix}")
}

pub fn decision_button(d: Decision) -> String {
    format!("agents-permission-{}", d.as_str())
}

pub fn review_button(change: Option<u64>, accept: bool) -> String {
    let what = if accept { "accept" } else { "reject" };
    match change {
        Some(c) => format!("agents-change-{c}-{what}"),
        None => format!("agents-changes-{what}-all"),
    }
}

pub fn change_link(change: u64) -> String {
    format!("agents-change-{change}")
}

/// The Markdown of agent text row `ix`.
pub fn agent_text(ix: usize) -> String {
    format!("agents-text-{ix}")
}

/// The debug line of the tool call in row `ix` (brief 0027), its stop location and its Show/Hide toggle.
pub fn debug_row(ix: usize) -> String {
    format!("agents-debug-{ix}")
}

pub fn debug_location(ix: usize) -> String {
    format!("agents-debug-location-{ix}")
}

pub fn debug_expand(ix: usize) -> String {
    format!("agents-debug-expand-{ix}")
}

/// Thumbnail `n` of the tool call in row `ix`.
pub fn thumb(ix: usize, n: usize) -> String {
    format!("agents-thumb-{ix}-{n}")
}

/// The query of the slash menu in `text` with the caret at `caret`: the first word after a leading `/`, while the
/// caret is in it (no whitespace between the `/` and the caret).
pub fn slash_query(text: &str, caret: usize) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let end = 1 + rest.find(char::is_whitespace).unwrap_or(rest.len());
    (1..=end).contains(&caret).then(|| &text[1..end])
}

/// A command the slash menu lists, and where the typed word matched its name.
pub type Match<'a> = (&'a AvailableCommand, std::ops::Range<usize>);

/// The commands whose name contains `query` (ASCII case-insensitive), with where it matched: names that start with
/// it first, then the rest, each group alphabetical.
pub fn filter_commands<'a>(commands: &'a [AvailableCommand], query: &str) -> Vec<Match<'a>> {
    let q = query.to_ascii_lowercase();
    let mut out: Vec<_> = commands
        .iter()
        .filter_map(|c| {
            let at = c.name.to_ascii_lowercase().find(&q)?;
            Some((c, at..at + q.len()))
        })
        .collect();
    out.sort_by(|(a, ra), (b, rb)| (ra.start != 0, &a.name).cmp(&(rb.start != 0, &b.name)));
    out
}

pub struct AgentsWindow {
    theme: Theme,
    pub transcript: Transcript,
    list: ListState,
    /// The prompt box (brief 0057).
    pub input: Entity<TextInput>,
    /// Prompts sent this session, oldest first, and which one the box shows (Up and Down).
    history: Vec<String>,
    history_pos: Option<usize>,
    /// The slash menu's selected row, and the text it was closed for (Escape, a click outside) until the text changes.
    menu_selected: usize,
    menu_closed_for: Option<String>,
    _input_events: Subscription,
    pub header: HeaderState,
    pub changes: Vec<ChangeItem>,
    pub prompt: Option<Prompt>,
    picker_open: bool,
    /// The agent's modes and config options (brief 0058), as the shell last heard them.
    pub modes: Option<SessionModeState>,
    pub config_options: Vec<SessionConfigOption>,
    /// [`pickers`] of the two, computed when they change (not on every frame).
    picker_list: Vec<Picker>,
    /// Picks the agent has not answered yet: picker key to value (shown muted).
    pub pending: HashMap<String, String>,
    /// The open picker's key and its highlighted row.
    open_picker: Option<(String, usize)>,
    /// When the running turn started (brief 0059), and the timer redrawing its status line and spinner.
    turn_started: Option<Instant>,
    ticker: Option<Task<()>>,
    mono: SharedString,
    pub probes: Rc<RefCell<Probes>>,
    pub painted: Painted,
    /// The Add server dialog while it is open (brief 0060); the shell opens and closes it.
    pub provider_dialog: Option<Entity<super::providers::ProviderDialog>>,
    /// The history list's rows (brief 0061), newest first, at most [`super::sessions::HISTORY_ROWS`]; whether there
    /// are more; whether it is open and its highlighted row.
    pub history_rows: Vec<HistoryRow>,
    pub history_more: bool,
    history_open: bool,
    history_selected: usize,
    /// Why the prompt box is disabled (the shown session's).
    pub disabled: Option<String>,
}

impl EventEmitter<AgentsWindowEvent> for AgentsWindow {}

impl AgentsWindow {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        let input = cx.new(|cx| {
            let mut input = TextInput::new(true, cx);
            input.set_placeholder(PLACEHOLDER, cx);
            // The prompt is prose: the UI's family.
            input.set_style(
                EditorStyle {
                    font_family: theme.typography.ui_font.into(),
                    ..EditorStyle::for_theme(&theme)
                },
                cx,
            );
            input
        });
        let _input_events = cx.subscribe(&input, Self::on_input_event);
        Self {
            theme,
            transcript: Transcript::default(),
            list,
            input,
            history: Vec::new(),
            history_pos: None,
            menu_selected: 0,
            menu_closed_for: None,
            _input_events,
            header: HeaderState::default(),
            changes: Vec::new(),
            prompt: None,
            picker_open: false,
            modes: None,
            config_options: Vec::new(),
            picker_list: Vec::new(),
            pending: HashMap::new(),
            open_picker: None,
            turn_started: None,
            ticker: None,
            mono: eludite_editor::default_font_family(),
            probes: Rc::default(),
            painted: Rc::default(),
            provider_dialog: None,
            history_rows: Vec::new(),
            history_more: false,
            history_open: false,
            history_selected: 0,
            disabled: None,
        }
    }

    /// Swap what the window shows of a session for `view` (brief 0061): the transcript and its list, the permission
    /// prompt, the header, the pickers, the running turn's start and whether the box is disabled. The prompt box's
    /// text stays; the shell uses this to apply an off-screen session's events, then swaps back.
    pub fn swap_view(&mut self, view: &mut SessionView, cx: &mut Context<Self>) {
        std::mem::swap(&mut self.transcript, &mut view.transcript);
        std::mem::swap(&mut self.list, &mut view.list);
        std::mem::swap(&mut self.prompt, &mut view.prompt);
        std::mem::swap(&mut self.header, &mut view.header);
        std::mem::swap(&mut self.modes, &mut view.modes);
        std::mem::swap(&mut self.config_options, &mut view.config_options);
        std::mem::swap(&mut self.picker_list, &mut view.picker_list);
        std::mem::swap(&mut self.pending, &mut view.pending);
        std::mem::swap(&mut self.open_picker, &mut view.open_picker);
        std::mem::swap(&mut self.turn_started, &mut view.turn_started);
        std::mem::swap(&mut self.disabled, &mut view.disabled);
        // The spinner's timer runs while the shown session's turn does.
        if self.running() {
            self.start_ticker(cx);
        } else {
            self.ticker = None;
        }
        cx.notify();
    }

    /// Show `view`'s session in place of the one shown, which goes into `view` with the prompt box's text (brief
    /// 0061): its own text comes back, its list scrolls to the end, and the open lists close.
    pub fn show_view(&mut self, view: &mut SessionView, cx: &mut Context<Self>) {
        let outgoing = self.prompt_text(cx);
        self.swap_view(view, cx);
        // The draft is not swapped with the rest: the incoming one is still in `view`.
        let incoming = std::mem::replace(&mut view.draft, outgoing);
        view.open_picker = None;
        self.input.update(cx, |i, cx| i.set_text(&incoming, cx));
        self.history_pos = None;
        self.menu_closed_for = None;
        self.picker_open = false;
        self.open_picker = None;
        self.sync(cx);
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
    }

    /// Scroll the transcript to its end and follow it.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
    }

    /// Show `transcript` in place of the shown one (a stored session's, rebuilt from its record; brief 0061).
    pub fn replace_transcript(&mut self, transcript: Transcript, cx: &mut Context<Self>) {
        self.transcript = transcript;
        self.list.reset(0);
        self.sync(cx);
        self.scroll_to_end(cx);
    }

    /// Whether the history list is open, and its highlighted row.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn history_open(&self) -> Option<usize> {
        self.history_open.then_some(self.history_selected)
    }

    /// The history list's rows (the shell keeps them up to date).
    pub fn set_history(&mut self, rows: Vec<HistoryRow>, more: bool, cx: &mut Context<Self>) {
        if self.history_rows != rows || self.history_more != more {
            self.history_rows = rows;
            self.history_more = more;
            self.history_selected = self
                .history_selected
                .min(self.history_rows.len().saturating_sub(1));
            cx.notify();
        }
    }

    /// Open or close the history list (the clock button); opening it highlights the shown session.
    pub fn toggle_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.history_open = !self.history_open;
        if self.history_open {
            self.picker_open = false;
            self.open_picker = None;
            self.history_selected = self
                .history_rows
                .iter()
                .position(|r| r.current)
                .unwrap_or(0);
            window.focus(&self.input.focus_handle(cx), cx);
            cx.emit(AgentsWindowEvent::HistoryOpened);
        }
        cx.notify();
    }

    fn move_history(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.history_rows.is_empty() {
            let n = self.history_rows.len() as isize;
            self.history_selected = (self.history_selected as isize + delta).rem_euclid(n) as usize;
            cx.notify();
        }
    }

    /// Show the history list's highlighted session.
    fn pick_history(&mut self, cx: &mut Context<Self>) {
        self.history_open = false;
        if let Some(row) = self.history_rows.get(self.history_selected) {
            cx.emit(AgentsWindowEvent::Switch(row.id.clone()));
        }
        cx.notify();
    }

    /// Whether the agent picker's list is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn picker_list_open(&self) -> bool {
        self.picker_open
    }

    /// Re-measure the rows that changed and redraw.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.transcript.take_splice() {
            self.list.splice(s.start..s.old_end, s.new_end - s.start);
        }
        cx.notify();
    }

    /// Scroll so row `ix` is visible (a debug row's link, in tests and from the shell).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn reveal(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Normal);
        self.list.scroll_to_reveal_item(ix);
        cx.notify();
    }

    pub fn set_header(&mut self, header: HeaderState, cx: &mut Context<Self>) {
        if self.header != header {
            self.header = header;
            // A turn starting closes the open picker (they are disabled while it runs).
            if self.running() {
                self.open_picker = None;
                self.turn_started.get_or_insert_with(Instant::now);
                self.start_ticker(cx);
            } else {
                self.turn_started = None;
                self.ticker = None;
            }
            cx.notify();
        }
    }

    /// Redraw the status line and the spinners every [`SPINNER_STEP`] while the turn runs; the timer ends with it.
    fn start_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SPINNER_STEP).await;
                let running = this.update(cx, |w, cx| {
                    if w.running() {
                        cx.notify();
                    }
                    w.running()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Whether the window redraws on its own (a turn runs); an idle window requests no frames.
    pub fn ticking(&self) -> bool {
        self.ticker.is_some()
    }

    /// How long the running turn has run.
    fn elapsed(&self) -> Option<Duration> {
        self.turn_started.map(|t| t.elapsed())
    }

    /// The selected agent's name.
    fn agent_name(&self) -> String {
        self.header
            .agents
            .get(self.header.selected)
            .cloned()
            .unwrap_or_else(|| "The agent".into())
    }

    /// The status line while a turn runs: `Claude Code is working… 0:12 · Esc to stop`.
    pub fn status_text(&self) -> Option<String> {
        let elapsed = self.elapsed()?;
        Some(format!(
            "{} is working\u{2026} {} \u{B7} Esc to stop",
            self.agent_name(),
            elapsed_text(elapsed)
        ))
    }

    /// What the usage strip shows: the session's last usage, `None` before the first.
    pub fn usage_strip(&self) -> Option<UsageStrip> {
        self.transcript.usage.as_ref().map(|u| UsageStrip {
            used: u.usage.used,
            size: u.usage.size,
            cost: u.usage.cost.clone(),
        })
    }

    /// The usage strip's text (`61k of 1M · $0.95`, or `No usage yet`).
    pub fn usage_text(&self) -> String {
        self.usage_strip()
            .map_or_else(|| eludite_ui::transcript::NO_USAGE.to_owned(), |s| s.text())
    }

    /// The agent's modes and config options changed (brief 0058): picks it now shows are answered.
    pub fn set_options(
        &mut self,
        modes: Option<SessionModeState>,
        config_options: Vec<SessionConfigOption>,
        cx: &mut Context<Self>,
    ) {
        self.picker_list = pickers(modes.as_ref(), &config_options);
        self.modes = modes;
        self.config_options = config_options;
        let pickers = &self.picker_list;
        self.pending.retain(|key, value| {
            pickers
                .iter()
                .any(|p| p.key == *key && p.current != *value && p.has(value))
        });
        if let Some((key, _)) = &self.open_picker
            && !pickers.iter().any(|p| p.key == *key)
        {
            self.open_picker = None;
        }
        cx.notify();
    }

    /// Forget the pick of `key` (the agent refused it, or the command failed): the picker shows the current choice.
    pub fn revert(&mut self, key: &str, cx: &mut Context<Self>) {
        self.pending.remove(key);
        cx.notify();
    }

    /// The footer's pickers.
    pub fn pickers(&self) -> &[Picker] {
        &self.picker_list
    }

    /// What picker `key`'s button says, and whether it is muted (a pick waiting for the agent, or a turn running).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn picker_label(&self, key: &str) -> Option<(String, bool)> {
        let p = self.pickers().iter().find(|p| p.key == key)?;
        let pending = self.pending.get(key);
        let name = p.name_of(pending.unwrap_or(&p.current));
        Some((
            format!("{name} \u{25BE}"),
            pending.is_some() || self.running(),
        ))
    }

    /// The open picker's key and highlighted row.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn open_picker(&self) -> Option<(String, usize)> {
        self.open_picker.clone()
    }

    fn toggle_picker(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.running() {
            return;
        }
        if self.open_picker.as_ref().is_some_and(|(k, _)| k == key) {
            self.open_picker = None;
        } else if let Some(p) = self.pickers().iter().find(|p| p.key == key) {
            let at = p.choices.iter().position(|c| c.value == p.current);
            self.open_picker = Some((key.to_owned(), at.unwrap_or(0)));
            // The prompt box keeps the focus; the footer takes Up, Down, Enter and Escape first.
            window.focus(&self.input.focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn move_picker(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some((key, at)) = self.open_picker.clone() else {
            return;
        };
        if let Some(p) = self.pickers().iter().find(|p| p.key == key)
            && !p.choices.is_empty()
        {
            let n = p.choices.len() as isize;
            self.open_picker = Some((key, (at as isize + delta).rem_euclid(n) as usize));
            cx.notify();
        }
    }

    /// Pick `value` of picker `key`: nothing when it is already current.
    pub fn pick(&mut self, key: &str, value: &str, cx: &mut Context<Self>) {
        self.open_picker = None;
        cx.notify();
        let Some(p) = self.pickers().iter().find(|p| p.key == key) else {
            return;
        };
        let shown = self.pending.get(key).unwrap_or(&p.current);
        if shown == value || self.running() {
            return;
        }
        self.pending.insert(key.to_owned(), value.to_owned());
        cx.emit(AgentsWindowEvent::Configure {
            option: key.to_owned(),
            value: value.to_owned(),
        });
    }

    /// Enter on the open picker: pick its highlighted row.
    fn pick_highlighted(&mut self, cx: &mut Context<Self>) {
        let Some((key, at)) = self.open_picker.clone() else {
            return;
        };
        if let Some(c) = self
            .pickers()
            .iter()
            .find(|p| p.key == key)
            .and_then(|p| p.choices.get(at).cloned())
        {
            self.pick(&key, &c.value, cx);
        }
    }

    fn running(&self) -> bool {
        self.header.state == StateKind::Running
    }

    /// The prompt box's text.
    pub fn prompt_text(&self, cx: &App) -> String {
        self.input.read(cx).text()
    }

    /// Send the prompt box's text: nothing while a turn runs or when it is only whitespace.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.prompt_text(cx).trim().to_owned();
        if text.is_empty() || self.running() || self.disabled.is_some() {
            return;
        }
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if self.history.len() > HISTORY_LIMIT {
                self.history.remove(0);
            }
        }
        self.history_pos = None;
        self.input.update(cx, |i, cx| i.clear(cx));
        cx.emit(AgentsWindowEvent::Prompt(text));
        cx.notify();
    }

    /// The prompts sent this session, oldest first.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn history(&self) -> &[String] {
        &self.history
    }

    fn on_input_event(&mut self, _: Entity<TextInput>, e: &TextInputEvent, cx: &mut Context<Self>) {
        match e {
            TextInputEvent::Submit => self.submit(cx),
            TextInputEvent::Escape => cx.emit(AgentsWindowEvent::Cancel),
            TextInputEvent::Up => self.recall(-1, cx),
            TextInputEvent::Down => self.recall(1, cx),
            TextInputEvent::Changed => {
                self.menu_selected = 0;
                cx.notify();
            }
        }
    }

    /// Up (`-1`) or Down (`1`) through the history, while the box is empty or shows a recalled prompt unedited.
    fn recall(&mut self, delta: i64, cx: &mut Context<Self>) {
        let text = self.prompt_text(cx);
        let unedited = match self.history_pos {
            Some(p) => self.history.get(p) == Some(&text),
            None => text.is_empty(),
        };
        if !unedited || self.history.is_empty() {
            return;
        }
        let next = match (self.history_pos, delta < 0) {
            (None, true) => Some(self.history.len() - 1),
            (None, false) => return,
            (Some(p), true) => Some(p.saturating_sub(1)),
            (Some(p), false) => (p + 1 < self.history.len()).then_some(p + 1),
        };
        if next == self.history_pos {
            return;
        }
        self.history_pos = next;
        let shown = next.map(|p| self.history[p].clone()).unwrap_or_default();
        self.input.update(cx, |i, cx| i.set_text(&shown, cx));
    }

    /// The slash menu's rows when it is open: the agent sent commands, the box starts with `/` and the caret is in the
    /// first word, it was not closed for this text, and something matches.
    fn menu(&self, cx: &App) -> Option<(String, Vec<Match<'_>>)> {
        if self.transcript.commands.is_empty() {
            return None;
        }
        let input = self.input.read(cx);
        let text = input.text();
        if self.menu_closed_for.as_ref() == Some(&text) {
            return None;
        }
        let query = slash_query(&text, input.caret())?.to_owned();
        let rows = filter_commands(&self.transcript.commands, &query);
        (!rows.is_empty()).then_some((query, rows))
    }

    /// Whether the slash menu is open.
    pub fn menu_open(&self, cx: &App) -> bool {
        self.menu(cx).is_some()
    }

    /// The slash menu's commands in order, and the selected one's index, when it is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn menu_items(&self, cx: &App) -> Option<(Vec<String>, usize)> {
        let (_, rows) = self.menu(cx)?;
        let names = rows.iter().map(|(c, _)| c.name.clone()).collect::<Vec<_>>();
        let selected = self.menu_selected.min(names.len() - 1);
        Some((names, selected))
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_closed_for = Some(self.prompt_text(cx));
        cx.notify();
    }

    fn move_menu(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some((_, rows)) = self.menu(cx) {
            let n = rows.len() as isize;
            let at = self.menu_selected.min(rows.len() - 1) as isize;
            self.menu_selected = (at + delta).rem_euclid(n) as usize;
            cx.notify();
        }
    }

    /// Replace the first word with `/name ` and put the caret after the space.
    fn complete(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.prompt_text(cx);
        let rest = text
            .find(char::is_whitespace)
            .map_or("", |i| text[i..].trim_start());
        let completed = format!("/{name} {rest}");
        let caret = name.len() + 2;
        self.input.update(cx, |i, cx| {
            i.set_text(&completed, cx);
            i.set_caret(caret, cx);
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Enter or Tab with the menu open: complete the selected command. Enter sends instead when the typed name is
    /// already the selected command's. Returns whether the key was taken.
    fn accept_menu(&mut self, enter: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((query, rows)) = self.menu(cx) else {
            return false;
        };
        let (c, _) = rows[self.menu_selected.min(rows.len() - 1)];
        if enter && c.name == query {
            return false;
        }
        let name = c.name.clone();
        self.complete(&name, window, cx);
        true
    }

    fn render_row(&mut self, ix: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let Some(row) = self.transcript.rows.get(ix) else {
            return div().into_any_element();
        };
        match row {
            Row::User { text, time } => user_prompt(text.clone(), time, &t).into_any_element(),
            Row::Agent(text) => {
                let mono = gpui::font(self.mono.clone());
                let this = cx.entity().downgrade();
                let on_link: eludite_ui::markdown::OnLink = Rc::new(move |url, _, cx| {
                    let url = url.to_owned();
                    let _ = this.update(cx, |_, cx| cx.emit(AgentsWindowEvent::OpenLink(url)));
                });
                let sel = agent_text(ix);
                agent_block(
                    sel.clone(),
                    &text.blocks,
                    &t,
                    &window.text_style().font(),
                    &mono,
                    on_link,
                )
                .debug_selector(move || sel)
                .into_any_element()
            }
            Row::Thought(th) => thought_block(thought(ix), &th.label(), &th.text, th.expanded, &t)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.transcript.toggle_thought(ix);
                    this.sync(cx);
                }))
                .into_any_element(),
            Row::Plan(entries) => plan_card(
                &entries
                    .iter()
                    .map(|e| (e.content.clone(), e.status.clone()))
                    .collect::<Vec<_>>(),
                &t,
            )
            .into_any_element(),
            Row::Usage(u) => usage_line(u.short(), &t).into_any_element(),
            Row::Notice(text) => notice(text.clone(), false, &t).into_any_element(),
            Row::Error(text) => notice(text.clone(), true, &t).into_any_element(),
            Row::Tool(tool) => {
                // Collapsed, the card is one line; expanded, its arguments (pretty-printed, at most 40 lines) and its
                // result (a debug command's: the summary the agent received, brief 0027) fold out under it.
                let expanded = tool.expanded;
                let arguments = if expanded {
                    tool.call
                        .raw_input
                        .as_ref()
                        .map(|v| clip_lines(&pretty(v), ARGUMENT_LINES))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let debug = tool.debug_line();
                let result = if expanded {
                    clip(&tool.call.content_text(), 1500)
                } else {
                    String::new()
                };
                let note = tool.note();
                let changes = tool.changes.clone();
                let status = tool.status();
                let spinner = (status == ToolStatus::Running)
                    .then(|| self.elapsed().map(spinner_frame))
                    .flatten();
                // Consecutive tool calls of a turn read as one group.
                let is_tool = |i: usize| matches!(self.transcript.rows.get(i), Some(Row::Tool(_)));
                let grouped = (ix > 0 && is_tool(ix - 1)) || is_tool(ix + 1);
                let tool_name = tool.tool_name();
                // Tracked for the real-input driver, which expands a card for its screenshot (brief 0059).
                let card = tracked(
                    &self.painted,
                    tool_card(ix),
                    tool_call_card(
                        ToolCard {
                            id: tool_card(ix).into(),
                            title: &tool.name(),
                            kind: tool.call.kind.as_deref().unwrap_or_default(),
                            status,
                            spinner,
                            expanded,
                            grouped,
                            arguments: &arguments,
                            result: &result,
                            note: note.as_deref(),
                        },
                        &t,
                        self.mono.clone(),
                    ),
                )
                .tooltip(tip(tool_name, t))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.transcript.toggle_tool(ix);
                    this.sync(cx);
                }));
                // Each change the call proposed links to its review view (and, once decided, what was applied).
                let painted = self.painted.clone();
                let links = changes.into_iter().map(|(id, path, state)| {
                    let name = std::path::Path::new(&path)
                        .file_name()
                        .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                    let sel = change_link(id);
                    tracked(
                        &painted,
                        sel.clone(),
                        div().id(SharedString::from(sel.clone())),
                    )
                    .debug_selector(move || sel)
                    .px_3()
                    .text_size(t.typography.small)
                    .text_color(t.accent)
                    .cursor_pointer()
                    .child(SharedString::from(format!(
                        "\u{2192} Change #{id}: {name} ({state})"
                    )))
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(AgentsWindowEvent::OpenChange(id))),
                    )
                });
                // The images its result carried, as thumbnails; a click opens the full image.
                let tool_call = tool.call.tool_call_id.clone();
                let thumbs = tool.images.iter().enumerate().map(|(n, image)| {
                    let sel = thumb(ix, n);
                    let (w, h) = image.thumb_size();
                    let tool_call = tool_call.clone();
                    tracked(
                        &painted,
                        sel.clone(),
                        div().id(SharedString::from(sel.clone())),
                    )
                    .debug_selector(move || sel)
                    .flex_none()
                    .border_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .child(
                        img(ImageSource::Render(image.render.clone()))
                            .w(px(w as f32))
                            .h(px(h as f32)),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(AgentsWindowEvent::OpenImage {
                            tool_call: tool_call.clone(),
                            index: n,
                        })
                    }))
                });
                let thumbs: Vec<_> = thumbs.collect();
                let debug_line = debug.map(|d| {
                    let sel = debug_row(ix);
                    let location = d.location.clone().map(|(path, line)| {
                        let sel = debug_location(ix);
                        let name = std::path::Path::new(&path)
                            .file_name()
                            .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                        tracked(
                            &painted,
                            sel.clone(),
                            div().id(SharedString::from(sel.clone())),
                        )
                        .debug_selector(move || sel)
                        .text_color(t.accent)
                        .cursor_pointer()
                        .child(SharedString::from(format!("{name}:{line}")))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(AgentsWindowEvent::OpenLocation {
                                path: path.clone(),
                                line,
                            })
                        }))
                    });
                    let toggle = {
                        let sel = debug_expand(ix);
                        tracked(
                            &painted,
                            sel.clone(),
                            div().id(SharedString::from(sel.clone())),
                        )
                        .debug_selector(move || sel)
                        .text_color(t.text_muted)
                        .cursor_pointer()
                        .child(if expanded {
                            "Hide snapshot"
                        } else {
                            "Show snapshot"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.transcript.toggle_tool(ix);
                            this.sync(cx);
                        }))
                    };
                    // The line, then its links under it (a narrow window wraps the text, never hides the links).
                    div()
                        .id(SharedString::from(sel.clone()))
                        .debug_selector(move || sel)
                        .flex()
                        .flex_col()
                        .px_3()
                        .pt_1()
                        .text_size(t.typography.small)
                        .text_color(t.text)
                        .child(
                            div()
                                .font_family(self.mono.clone())
                                .child(SharedString::from(d.text.clone())),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap_2()
                                .children(location)
                                .child(toggle),
                        )
                });
                let strip = (!thumbs.is_empty()).then(|| {
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .px_3()
                        .py_1()
                        .children(thumbs)
                });
                // The last call of a group leaves the 8 px gap before the next row.
                let last = !is_tool(ix + 1);
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .when(last, |d| d.pb_2())
                    .children(debug_line)
                    .child(card)
                    .children(links)
                    .children(strip)
                    .into_any_element()
            }
        }
    }

    /// One row (brief 0059): the agent picker, the state as a colored dot and word (its tooltip the agent's version
    /// and MCP endpoint), a spacer, and the Start/Restart icon button; a bordered block under it only for the login
    /// instructions and an error.
    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let history_menu = self.render_history(cx);
        let h = &self.header;
        let name = h
            .agents
            .get(h.selected)
            .cloned()
            .unwrap_or_else(|| "No agent".into());
        let restart = !matches!(h.state, StateKind::Stopped | StateKind::Error);
        // Tracked, with its rows, for the screenshot driver (brief 0060 picks a server from it).
        let painted = self.painted.clone();
        let picker = tracked(&painted, AGENT_PICKER, div().id(AGENT_PICKER))
            .debug_selector(|| AGENT_PICKER.into())
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(20.))
            .border_1()
            .border_color(t.border)
            .cursor_pointer()
            .hover(|s| s.bg(t.menu_hover))
            .child(SharedString::from(name))
            .child("\u{25BE}")
            .on_click(cx.listener(|this, _, _, cx| {
                this.picker_open = !this.picker_open;
                cx.notify();
            }));
        // Brief 0061: with a session shown, another agent starts a new session with it (the shown one keeps running).
        let session_agent = h
            .session
            .as_ref()
            .and_then(|_| h.agents.get(h.selected).cloned());
        let menu = self.picker_open.then(|| {
            let items = h.agents.iter().enumerate().map(|(ix, a)| {
                let sel = agent_item(ix);
                let a = a.clone();
                let session_agent = session_agent.clone();
                tracked(
                    &painted,
                    sel.clone(),
                    div().id(SharedString::from(sel.clone())),
                )
                .debug_selector(move || sel)
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(|s| s.bg(t.menu_hover))
                .child(SharedString::from(a.clone()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picker_open = false;
                    match &session_agent {
                        Some(current) if *current != a => cx.emit(AgentsWindowEvent::NewSession {
                            agent: Some(a.clone()),
                        }),
                        _ => cx.emit(AgentsWindowEvent::Start {
                            agent: Some(a.clone()),
                            restart: true,
                        }),
                    }
                }))
            });
            // Brief 0060: an OpenAI-compatible server is added from the list's last row.
            let add = tracked(&painted, ADD_SERVER_ITEM, div().id(ADD_SERVER_ITEM))
                .debug_selector(|| ADD_SERVER_ITEM.into())
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(t.border)
                .text_color(t.text_muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.menu_hover))
                .child("Add server\u{2026}")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picker_open = false;
                    cx.emit(AgentsWindowEvent::AddServer);
                    cx.notify();
                }));
            let items = items.chain(std::iter::once(add));
            deferred(
                anchored().child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("agents-picker-menu")
                        .occlude()
                        .min_w(px(200.))
                        .py_1()
                        .mt(px(22.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.picker_open = false;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1)
        });
        // The block under the row says what the person must do: log in, or read the error. Any other detail (the
        // agent's version and MCP endpoint, what is starting) is the state's tooltip and a line in Output > Agents.
        let block_detail = matches!(h.state, StateKind::NeedsLogin | StateKind::Error);
        let state_color = h.state.color(&t);
        let state = div()
            .id(STATE_LABEL)
            .debug_selector(|| STATE_LABEL.into())
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .text_color(state_color)
            .child("\u{25CF}")
            .child(h.state.label())
            .when(!block_detail && !h.detail.is_empty(), |d| {
                d.tooltip(tip(h.detail.clone(), t))
            });
        // Tracked for the real-input driver (brief 0057's run starts the agent before any prompt).
        let start = tracked(
            &self.painted,
            START_BUTTON,
            eludite_ui::icon_button(
                START_BUTTON,
                if restart { "\u{27F3}" } else { "\u{25B6}" },
                &t,
            )
            .debug_selector(|| START_BUTTON.into()),
        )
        .min_w(px(22.))
        .h(px(20.))
        .text_size(t.typography.ui)
        .text_color(t.text)
        .tooltip(tip(if restart { RESTART_TIP } else { START_TIP }, t))
        .on_click(cx.listener(move |_, _, _, cx| {
            cx.emit(AgentsWindowEvent::Start {
                agent: None,
                restart,
            })
        }));
        // Brief 0061: the history button (a clock) and New session, after the state.
        let history = tracked(
            &self.painted,
            HISTORY_BUTTON,
            eludite_ui::icon_button(HISTORY_BUTTON, "\u{25F7}", &t)
                .debug_selector(|| HISTORY_BUTTON.into()),
        )
        .min_w(px(22.))
        .h(px(20.))
        .text_size(t.typography.ui)
        .text_color(t.text)
        .tooltip(tip(HISTORY_TIP, t))
        .on_click(cx.listener(|this, _, window, cx| this.toggle_history(window, cx)));
        let new = tracked(
            &self.painted,
            NEW_BUTTON,
            eludite_ui::icon_button(NEW_BUTTON, "+", &t).debug_selector(|| NEW_BUTTON.into()),
        )
        .min_w(px(22.))
        .h(px(20.))
        .text_size(t.typography.ui)
        .text_color(t.text)
        .tooltip(tip(NEW_TIP, t))
        .on_click(cx.listener(|this, _, _, cx| {
            this.history_open = false;
            cx.emit(AgentsWindowEvent::NewSession { agent: None });
            cx.notify();
        }));
        let mut col = div()
            .flex()
            .flex_col()
            .flex_none()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.))
                    .px_3()
                    .child(div().relative().child(picker).children(menu))
                    .child(state)
                    .child(div().relative().child(history).children(history_menu))
                    .child(new)
                    .child(div().flex_1())
                    .child(start),
            );
        if h.state == StateKind::Error && !h.detail.is_empty() {
            col = col.child(
                div()
                    .debug_selector(|| "agents-error".into())
                    .mx_3()
                    .mb_2()
                    .p_2()
                    .border_1()
                    .border_color(state_color)
                    .text_color(t.text)
                    .child(SharedString::from(h.detail.clone())),
            );
        }
        if h.state == StateKind::NeedsLogin {
            let mut login = div()
                .debug_selector(|| "agents-login".into())
                .flex()
                .flex_col()
                .gap_1()
                .mx_3()
                .mb_2()
                .p_2()
                .border_1()
                .border_color(state_color)
                .text_color(t.text);
            if !h.detail.is_empty() {
                login = login.child(
                    div()
                        .text_color(t.text_muted)
                        .child(SharedString::from(h.detail.clone())),
                );
            }
            login =
                login.child("The agent is not logged in. Run this in a terminal, then Restart:");
            for m in &h.login {
                login = login.child(
                    div()
                        .font_family(self.mono.clone())
                        .text_size(t.typography.small)
                        .child(SharedString::from(m.command.clone())),
                );
            }
            col = col.child(login);
        }
        col.into_any_element()
    }

    /// The history list under its button (brief 0061): each session's title, then its agent and when it last
    /// changed, muted; a dot while it runs, `?` while it waits for an answer, the shown one checked.
    fn render_history(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.history_open {
            return None;
        }
        let t = self.theme;
        let painted = self.painted.clone();
        let rows = self.history_rows.iter().enumerate().map(|(ix, r)| {
            let sel = session_item(&r.id);
            let lit = ix == self.history_selected;
            let id = r.id.clone();
            let mark = if r.waiting {
                "?"
            } else if r.running {
                "\u{25CF}"
            } else {
                ""
            };
            tracked(
                &painted,
                sel.clone(),
                div().id(SharedString::from(sel.clone())),
            )
            .debug_selector(move || sel)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(34.))
            .cursor_pointer()
            .when(lit, |d| d.bg(t.accent).text_color(t.text_on_accent))
            .when(!lit, |d| d.hover(|s| s.bg(t.menu_hover)))
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .child(if r.current { "\u{2713}" } else { "" }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .child(div().truncate().child(SharedString::from(r.title.clone())))
                    .child(
                        div()
                            .truncate()
                            .text_size(t.typography.small)
                            .text_color(if lit { t.text_on_accent } else { t.text_muted })
                            .child(SharedString::from(format!("{} \u{B7} {}", r.agent, r.when))),
                    ),
            )
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .text_color(if lit {
                        t.text_on_accent
                    } else if r.waiting {
                        t.warning
                    } else {
                        ToolStatus::Running.color(&t)
                    })
                    .child(mark),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.history_open = false;
                cx.emit(AgentsWindowEvent::Switch(id.clone()));
                cx.notify();
            }))
        });
        let empty = self.history_rows.is_empty().then(|| {
            div()
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .text_color(t.text_muted)
                .child("No sessions yet")
        });
        let more = self.history_more.then(|| {
            div()
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(t.border)
                .text_size(t.typography.small)
                .text_color(t.text_muted)
                .child(OLDER_ON_DISK)
        });
        let painted = self.painted.clone();
        Some(
            deferred(
                anchored().child(
                    tracked(&painted, HISTORY_MENU, popup_panel(&t).id(HISTORY_MENU))
                        .debug_selector(|| HISTORY_MENU.into())
                        .occlude()
                        .w(px(320.))
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .py_1()
                        .mt(px(22.))
                        .on_mouse_down_out(cx.listener(
                            move |this, e: &gpui::MouseDownEvent, _, cx| {
                                // A click on the clock toggles it (its click handler closes it).
                                let on_button = painted
                                    .borrow()
                                    .get(HISTORY_BUTTON)
                                    .is_some_and(|b| b.contains(&e.position));
                                if !on_button {
                                    this.history_open = false;
                                    cx.notify();
                                }
                            },
                        ))
                        .children(rows)
                        .children(empty)
                        .children(more),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_prompt(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.theme;
        let p = self.prompt.clone()?;
        let painted = self.painted.clone();
        let button = |d: Decision, label: &'static str, default: bool| {
            tracked(
                &painted,
                decision_button(d),
                eludite_ui::push_button(decision_button(d), label, default, true, &t)
                    .min_w(px(48.))
                    .px_2(),
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(AgentsWindowEvent::Answer {
                    request: p.request,
                    decision: d,
                })
            }))
        };
        let mut buttons = div()
            .flex()
            .flex_none()
            .gap_2()
            .justify_end()
            .p_2()
            .child(button(Decision::Allow, "Allow", false));
        if p.can_persist {
            let label = if p.session {
                "Allow for this session"
            } else {
                "Always Allow"
            };
            buttons = buttons.child(button(Decision::AlwaysAllow, label, false));
        }
        buttons = buttons.child(button(Decision::Deny, "Deny", true));
        // `Claude Code wants to run **ls** (execute).`
        let (sentence, bold) =
            permission_sentence(&self.agent_name(), &p.title, &p.class, p.reason.as_deref());
        let sentence = StyledText::new(sentence).with_highlights([(
            bold,
            HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..HighlightStyle::default()
            },
        )]);
        // In a short window the call's detail gives up its lines first, so the buttons and the prompt box below stay
        // on screen.
        Some(
            eludite_ui::dialog_panel(&t, "Agent Permission")
                .debug_selector(|| "agents-permission-dialog".into())
                .flex_shrink(1.)
                .min_h_0()
                .overflow_hidden()
                .mx_3()
                .mb_2()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink(1.)
                        .min_h_0()
                        .overflow_hidden()
                        .gap_1()
                        .p_2()
                        .child(
                            div()
                                .flex_none()
                                .debug_selector(|| "agents-permission-text".into())
                                .child(sentence),
                        )
                        .child(
                            div()
                                .flex_shrink(1.)
                                .min_h_0()
                                .overflow_hidden()
                                .font_family(self.mono.clone())
                                .text_size(t.typography.small)
                                .text_color(t.text_muted)
                                .child(SharedString::from(clip(&p.detail, 800))),
                        ),
                )
                .child(buttons)
                .into_any_element(),
        )
    }

    /// The slash menu above the prompt box, at its left edge and as wide as the box (brief 0057): at most
    /// [`COMPLETION_ROWS`] rows around the selected one, each the name in the mono font with the match bold and the
    /// description muted after it, then the selected command's description and input hint on one muted line. The
    /// menu is laid out in the box's own wrapper, so it never leaves the Agents window however narrow the panel is;
    /// a description too long for it is cut with an ellipsis.
    fn render_menu(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.theme;
        let ui_font = window.text_style().font_family;
        let (_, rows) = self.menu(cx)?;
        let selected = self.menu_selected.min(rows.len() - 1);
        let first = (selected + 1).saturating_sub(COMPLETION_ROWS);
        let mono = self.mono.clone();
        let items: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .skip(first)
            .take(COMPLETION_ROWS)
            .map(|(ix, (c, matched))| {
                let sel = slash_item(&c.name);
                let name = c.name.clone();
                let is_selected = ix == selected;
                let label = format!("/{}", c.name);
                let matched = matched.start + 1..matched.end + 1;
                completion_row(
                    &t,
                    SharedString::from(sel.clone()),
                    CompletionKind::Keyword,
                    label,
                    &[matched],
                    is_selected,
                )
                .debug_selector(move || sel)
                .font_family(mono.clone())
                .cursor_pointer()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_size(t.typography.small)
                        .font_family(ui_font.clone())
                        .text_color(if is_selected {
                            t.text_on_accent
                        } else {
                            t.text_muted
                        })
                        .child(SharedString::from(c.description.clone())),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.complete(&name, window, cx)))
                .into_any_element()
            })
            .collect();
        let (c, _) = rows[selected];
        let detail = match c.hint() {
            Some(h) => format!("/{} {h}: {}", c.name, c.description),
            None => format!("/{}: {}", c.name, c.description),
        };
        Some(
            deferred(
                popup_panel(&t)
                    .id(SLASH_MENU)
                    .debug_selector(|| SLASH_MENU.into())
                    .occlude()
                    .absolute()
                    .left_0()
                    .bottom_full()
                    // The box's width (laid out in the box's wrapper, so in this frame), at most `SLASH_MENU_WIDTH`.
                    .w_full()
                    .max_w(px(SLASH_MENU_WIDTH))
                    .py_1()
                    .mb_1()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
                    .children(items)
                    .child(
                        div()
                            .debug_selector(|| SLASH_DETAIL.into())
                            .px_1()
                            .pt_1()
                            .mt_1()
                            .border_t_1()
                            .border_color(t.border)
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .truncate()
                            .child(SharedString::from(detail)),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// The footer under the prompt box (brief 0058): the pickers, a spacer, and `send`.
    fn render_footer(&mut self, send: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let running = self.running();
        let open = self.open_picker.clone();
        let mut menu = None;
        let mut row = tracked(&self.painted, FOOTER, div().id(FOOTER))
            .debug_selector(|| FOOTER.into())
            .flex()
            .items_center()
            .gap_1()
            .h(px(28.));
        for p in &self.picker_list {
            let id = p.button_id();
            let pending = self.pending.get(&p.key).cloned();
            let name = p.name_of(pending.as_deref().unwrap_or(&p.current));
            let muted = pending.is_some() || running;
            let key = p.key.clone();
            let button = tracked(
                &self.painted,
                id.clone(),
                div().id(SharedString::from(id.clone())),
            )
            .debug_selector({
                let id = id.clone();
                move || id
            })
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(20.))
            .text_color(if muted { t.text_muted } else { t.text })
            .child(SharedString::from(name))
            .child("\u{25BE}");
            let button =
                if running {
                    button.tooltip(tip(WAIT_FOR_TURN, t))
                } else {
                    button
                        .cursor_pointer()
                        .hover(|s| s.bg(t.menu_hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.toggle_picker(&key, window, cx)
                        }))
                };
            if let Some((_, highlighted)) = open.as_ref().filter(|(k, _)| *k == p.key && !running) {
                menu = Some(self.render_picker_menu(p, *highlighted, cx));
            }
            row = row.child(button);
        }
        row.children(menu)
            .child(div().flex_1())
            .child(send)
            .into_any_element()
    }

    /// The open picker's list, above its button: each choice's name (the current one checked) with its description
    /// muted after it, cut with an ellipsis when the panel is too narrow for it. The list is laid out across the
    /// footer, so it stays inside the Agents window and is never wider than the footer: its left edge is the
    /// button's, unless the list would then overflow the footer's right edge, when its right edge is the footer's.
    fn render_picker_menu(
        &self,
        p: &Picker,
        highlighted: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = self.theme;
        let button = p.button_id();
        let rows = p.choices.iter().enumerate().map(|(ix, c)| {
            let sel = p.row_id(&c.value);
            let lit = ix == highlighted;
            let (key, value) = (p.key.clone(), c.value.clone());
            // Tracked for the real-input driver (`--bounds-out`), which picks a model.
            tracked(
                &self.painted,
                sel.clone(),
                div().id(SharedString::from(sel.clone())),
            )
            .debug_selector(move || sel)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(22.))
            .cursor_pointer()
            .when(lit, |d| d.bg(t.accent).text_color(t.text_on_accent))
            .when(!lit, |d| d.hover(|s| s.bg(t.menu_hover)))
            .child(div().w(px(12.)).flex_none().child(if c.value == p.current {
                "\u{2713}"
            } else {
                ""
            }))
            .child(div().flex_none().child(SharedString::from(c.name.clone())))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_size(t.typography.small)
                    .text_color(if lit { t.text_on_accent } else { t.text_muted })
                    .children(c.description.clone().map(SharedString::from)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.pick(&key, &value, cx)))
        });
        // The button's offset in the footer and the footer's width, as last painted: the button was painted before
        // the click that opened the list, and its offset does not change when the panel is resized.
        let (offset, footer_width) = {
            let painted = self.painted.borrow();
            match (painted.get(FOOTER), painted.get(&button)) {
                (Some(f), Some(b)) => ((b.left() - f.left()).max(px(0.)), f.size.width),
                (Some(f), None) => (px(0.), f.size.width),
                _ => (px(0.), px(PICKER_MIN_WIDTH)),
            }
        };
        let painted = self.painted.clone();
        // A spacer as wide as the button's offset, then the list: the spacer shrinks when the list would overflow
        // the footer, which pushes the list left until its right edge is the footer's.
        deferred(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom_full()
                .flex()
                .items_end()
                .child(div().w(offset).min_w(px(0.)).flex_shrink(1.))
                .child(
                    popup_panel(&t)
                        .id(PICKER_MENU)
                        .debug_selector(|| PICKER_MENU.into())
                        .occlude()
                        .flex_none()
                        .min_w(px(PICKER_MIN_WIDTH).min(footer_width))
                        .max_w(relative(1.))
                        .py_1()
                        .on_mouse_down_out(cx.listener(
                            move |this, e: &gpui::MouseDownEvent, _, cx| {
                                // A click on the picker's own button toggles it (its click handler closes it).
                                let on_button = painted
                                    .borrow()
                                    .get(&button)
                                    .is_some_and(|b| b.contains(&e.position));
                                if !on_button {
                                    this.open_picker = None;
                                    cx.notify();
                                }
                            },
                        ))
                        .children(rows),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn render_changes(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = self.theme;
        let pending: Vec<ChangeItem> = self.changes.iter().filter(|c| c.pending).cloned().collect();
        if pending.is_empty() {
            return None;
        }
        let painted = self.painted.clone();
        let review =
            |change: Option<u64>, accept: bool, label: &'static str| {
                tracked(
                    &painted,
                    review_button(change, accept),
                    eludite_ui::push_button(review_button(change, accept), label, false, true, &t),
                )
                .min_w(px(50.))
                .h(px(20.))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(AgentsWindowEvent::Review { change, accept })
                }))
            };
        let mut col = div()
            .debug_selector(|| "agents-changes".into())
            .flex()
            .flex_col()
            .flex_none()
            .gap_px()
            .p_1()
            .border_t_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(
                        SharedString::from(format!("Pending changes ({})", pending.len())),
                    ))
                    .child(review(None, true, "Accept All"))
                    .child(review(None, false, "Reject All")),
            );
        for c in pending {
            let name = std::path::Path::new(&c.path)
                .file_name()
                .map_or(c.path.clone(), |n| n.to_string_lossy().into_owned());
            let sel = change_link(c.id);
            let id = c.id;
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id(SharedString::from(format!("{sel}-open")))
                            .debug_selector(move || format!("{sel}-open"))
                            .flex_1()
                            .overflow_hidden()
                            .cursor_pointer()
                            .text_color(t.accent)
                            .child(SharedString::from(format!("{name}  {}", c.summary)))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(AgentsWindowEvent::OpenChange(id))
                            })),
                    )
                    .child(review(Some(c.id), true, "Accept"))
                    .child(review(Some(c.id), false, "Reject")),
            );
        }
        Some(col.into_any_element())
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        s.chars().take(max).collect::<String>() + " \u{2026}"
    }
}

impl Focusable for AgentsWindow {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for AgentsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        {
            let p = self.probes.borrow();
            if p.recording {
                let start = Instant::now();
                let probes = self.probes.clone();
                // Runs after this frame is drawn and presented.
                cx.defer(move |_| {
                    probes
                        .borrow_mut()
                        .frame_work_ms
                        .push(start.elapsed().as_secs_f64() * 1e3);
                });
            }
        }
        let t = self.theme;
        // The tool cards drawn in this frame record their bounds again; the ones scrolled away drop out.
        self.painted
            .borrow_mut()
            .retain(|k, _| !k.starts_with("agents-tool-"));
        let focus = self.input.focus_handle(cx);
        let focused = focus.is_focused(window);
        let running = self.running();
        let header = self.render_header(cx);
        let prompt = self.render_prompt(cx);
        let changes = self.render_changes(cx);
        let menu = self.render_menu(window, cx);
        let input = tracked(&self.painted, PROMPT_BOX, div().id(PROMPT_BOX))
            .debug_selector(|| PROMPT_BOX.into())
            .key_context("AgentsPrompt")
            // The slash menu takes Up, Down, Tab, Enter and Escape while it is open (brief 0057).
            .capture_action(cx.listener(|this, _: &input_actions::MoveUp, _, cx| {
                if this.menu_open(cx) {
                    this.move_menu(-1, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input_actions::MoveDown, _, cx| {
                if this.menu_open(cx) {
                    this.move_menu(1, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input_actions::Tab, window, cx| {
                if this.accept_menu(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input_actions::Submit, window, cx| {
                if this.accept_menu(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input_actions::Escape, _, cx| {
                if this.menu_open(cx) {
                    this.close_menu(cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            }))
            .w_full()
            .px_1()
            .py(px(2.))
            .border_1()
            .border_color(if focused { t.accent } else { t.border })
            .bg(t.background)
            .text_color(t.text)
            // Brief 0061: a stored session its agent cannot resume says so in place of the box.
            .map(|d| match self.disabled.clone() {
                Some(why) => d
                    .debug_selector(|| "agents-prompt-disabled".into())
                    .min_h(px(36.))
                    .text_color(t.text_muted)
                    .child(SharedString::from(why)),
                None => d.child(self.input.clone()),
            });
        let input = div()
            .relative()
            .w_full()
            .min_w(px(0.))
            .children(menu)
            .child(input);
        let send = tracked(
            &self.painted,
            SEND_BUTTON,
            eludite_ui::push_button(
                SEND_BUTTON,
                if running { "Stop" } else { "Send" },
                !running,
                self.disabled.is_none() || running,
                &t,
            ),
        )
        .min_w(px(50.))
        .h(px(20.))
        .on_click(cx.listener(move |this, _, _, cx| {
            if running {
                cx.emit(AgentsWindowEvent::Cancel)
            } else {
                this.submit(cx)
            }
        }))
        .into_any_element();
        let footer = self.render_footer(send, cx);
        // Brief 0059: while a turn runs, its status line; then the usage strip, above the prompt box.
        let status = self.status_text().map(|text| {
            let spinner = spinner_frame(self.elapsed().unwrap_or_default());
            status_line(text, spinner, &t)
                .id(STATUS_LINE)
                .debug_selector(|| STATUS_LINE.into())
        });
        let strip = self.usage_strip();
        let last_turn = self.transcript.usage.as_ref().map(|u| u.text());
        let strip = tracked(
            &self.painted,
            USAGE_STRIP,
            usage_strip(strip.as_ref(), &t).id(USAGE_STRIP),
        )
        .debug_selector(|| USAGE_STRIP.into())
        .when_some(last_turn, |d, text| d.tooltip(tip(text, t)));
        div()
            .id("agents-window")
            .debug_selector(|| "agents-window".into())
            // The history list takes Up, Down, Enter and Escape while it is open (brief 0061), before the footer's
            // pickers and the prompt box.
            .capture_action(cx.listener(|this, _: &input_actions::MoveUp, _, cx| {
                if this.history_open {
                    this.move_history(-1, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input_actions::MoveDown, _, cx| {
                if this.history_open {
                    this.move_history(1, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input_actions::Submit, _, cx| {
                if this.history_open {
                    this.pick_history(cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input_actions::Escape, _, cx| {
                if this.history_open {
                    this.history_open = false;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.panel)
            .text_color(t.text)
            .text_size(t.typography.ui)
            .child(header)
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix, window, cx| this.render_row(ix, window, cx)),
                )
                .flex_1()
                .pt_2(),
            )
            .children(self.provider_dialog.clone())
            .children(status)
            .children(prompt)
            .children(changes)
            .child(strip)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .px_3()
                    .pt_1()
                    .border_t_1()
                    .border_color(t.border)
                    // An open picker takes Up, Down, Enter and Escape before the prompt box and its slash menu
                    // (brief 0058).
                    .capture_action(cx.listener(|this, _: &input_actions::MoveUp, _, cx| {
                        if this.open_picker.is_some() {
                            this.move_picker(-1, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &input_actions::MoveDown, _, cx| {
                        if this.open_picker.is_some() {
                            this.move_picker(1, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &input_actions::Submit, _, cx| {
                        if this.open_picker.is_some() {
                            this.pick_highlighted(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .capture_action(cx.listener(|this, _: &input_actions::Escape, _, cx| {
                        if this.open_picker.take().is_some() {
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .child(input)
                    .child(footer),
            )
    }
}
