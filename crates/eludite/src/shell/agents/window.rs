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

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use eludite_acp::LoginMethod;
use eludite_acp::protocol::AvailableCommand;
use eludite_editor::{EditorStyle, TextInput, TextInputEvent, input_actions};
use eludite_ui::Theme;
use eludite_ui::popup::{COMPLETION_ROWS, CompletionKind, completion_row, popup_panel};
use eludite_ui::transcript::{
    ToolCard, agent_block, notice, plan_card, thought_block, tool_call_card, user_prompt,
};
use gpui::{
    Anchor, AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, FollowMode, FontWeight, ImageSource, InteractiveElement, IntoElement,
    ListAlignment, ListState, ParentElement, Pixels, Render, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Subscription, Window, anchored, canvas, deferred, div, img,
    list, px, rgb,
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

    fn color(self) -> Rgba {
        match self {
            StateKind::Ready => rgb(0x89D185),
            StateKind::Running | StateKind::Starting => rgb(0x3794FF),
            StateKind::NeedsLogin => rgb(0xCCA700),
            StateKind::Error => rgb(0xF48771),
            StateKind::Stopped => rgb(0x9D9D9D),
        }
    }
}

/// What the header shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeaderState {
    pub agents: Vec<String>,
    pub selected: usize,
    pub state: StateKind,
    /// The agent's own name and version, the MCP endpoint, or the error.
    pub detail: String,
    pub login: Vec<LoginMethod>,
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
    pub tool: String,
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

pub fn agent_item(ix: usize) -> String {
    format!("agents-agent-{ix}")
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
    mono: SharedString,
    pub probes: Rc<RefCell<Probes>>,
    pub painted: Painted,
}

impl EventEmitter<AgentsWindowEvent> for AgentsWindow {}

impl AgentsWindow {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        let input = cx.new(|cx| {
            let mut input = TextInput::new(true, cx);
            input.set_placeholder(PLACEHOLDER, cx);
            input.set_style(
                EditorStyle {
                    theme,
                    ..EditorStyle::default()
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
            mono: eludite_editor::default_font_family(),
            probes: Rc::default(),
            painted: Rc::default(),
        }
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
            cx.notify();
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
        if text.is_empty() || self.running() {
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
            Row::User(text) => user_prompt(text.clone(), &t).into_any_element(),
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
            Row::Thought { text, expanded } => thought_block(thought(ix), text, *expanded, &t)
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
            Row::Usage(u) => notice(u.text(), false, &t).into_any_element(),
            Row::Notice(text) => notice(text.clone(), false, &t).into_any_element(),
            Row::Error(text) => notice(text.clone(), true, &t).into_any_element(),
            Row::Tool(tool) => {
                let arguments = tool
                    .call
                    .raw_input
                    .as_ref()
                    .map(compact)
                    .unwrap_or_default();
                // A debug command's result (the summary the agent received) is folded under its line (brief 0027).
                let debug = tool.mcp.as_ref().and_then(|m| m.debug.clone());
                let result = if debug.is_some() && !tool.expanded {
                    String::new()
                } else {
                    clip(&tool.call.content_text(), 1500)
                };
                let note = tool.note();
                let changes = tool.changes.clone();
                let card = tool_call_card(
                    ToolCard {
                        id: tool_card(ix).into(),
                        name: &tool.name(),
                        kind: tool.call.kind.as_deref().unwrap_or_default(),
                        status: tool.status(),
                        arguments: &clip(&arguments, 600),
                        result: &result,
                        note: note.as_deref(),
                    },
                    &t,
                    self.mono.clone(),
                );
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
                let expanded = tool.expanded;
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
                            this.transcript.toggle_result(ix);
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
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .children(debug_line)
                    .child(card)
                    .children(links)
                    .children(strip)
                    .into_any_element()
            }
        }
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let h = &self.header;
        let name = h
            .agents
            .get(h.selected)
            .cloned()
            .unwrap_or_else(|| "No agent".into());
        let restart = !matches!(h.state, StateKind::Stopped | StateKind::Error);
        let picker = div()
            .id(AGENT_PICKER)
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
        let menu = self.picker_open.then(|| {
            let items = h.agents.iter().enumerate().map(|(ix, a)| {
                let sel = agent_item(ix);
                let a = a.clone();
                div()
                    .id(SharedString::from(sel.clone()))
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
                        cx.emit(AgentsWindowEvent::Start {
                            agent: Some(a.clone()),
                            restart: true,
                        });
                    }))
            });
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
        // Tracked for the real-input driver (brief 0057's run starts the agent before any prompt).
        let start = tracked(
            &self.painted,
            START_BUTTON,
            eludite_ui::push_button(
                START_BUTTON,
                if restart { "Restart" } else { "Start" },
                false,
                true,
                &t,
            ),
        )
        .min_w(px(56.))
        .h(px(20.))
        .on_click(cx.listener(move |_, _, _, cx| {
            cx.emit(AgentsWindowEvent::Start {
                agent: None,
                restart,
            })
        }));
        let mut col = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().relative().child(picker).children(menu))
                    .child(
                        div()
                            .debug_selector(|| "agents-state".into())
                            .text_color(h.state.color())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(h.state.label()),
                    )
                    .child(div().flex_1())
                    .child(start),
            );
        if !h.detail.is_empty() {
            col = col.child(
                div()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(SharedString::from(h.detail.clone())),
            );
        }
        if h.state == StateKind::NeedsLogin {
            let mut login = div()
                .debug_selector(|| "agents-login".into())
                .flex()
                .flex_col()
                .p_1()
                .border_1()
                .border_color(StateKind::NeedsLogin.color())
                .child("The agent is not logged in. Run this in a terminal, then Restart:");
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
        let mut buttons =
            div()
                .flex()
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
        Some(
            eludite_ui::dialog_panel(&t, "Agent Permission")
                .debug_selector(|| "agents-permission-dialog".into())
                .flex_none()
                .m_1()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .child(SharedString::from(format!(
                            "The agent wants to run {} (class {}{}).",
                            p.tool,
                            p.class,
                            p.reason
                                .as_deref()
                                .map(|r| format!(": {r}"))
                                .unwrap_or_default()
                        )))
                        .child(
                            div()
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

    /// The slash menu above the prompt box, anchored to its top-left (brief 0057): at most
    /// [`COMPLETION_ROWS`] rows around the selected one, each the name in the mono font with the match bold and the
    /// description muted after it, then the selected command's description and input hint on one muted line.
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
                        .overflow_hidden()
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
                anchored().anchor(Anchor::BottomLeft).child(
                    popup_panel(&t)
                        .id(SLASH_MENU)
                        .debug_selector(|| SLASH_MENU.into())
                        .occlude()
                        .w(px(480.))
                        .max_w(px(640.))
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
                                .overflow_hidden()
                                .child(SharedString::from(detail)),
                        ),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
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

fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
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
            .child(self.input.clone());
        let input = div()
            .relative()
            .flex_1()
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
                true,
                &t,
            ),
        )
        .min_w(px(50.))
        .on_click(cx.listener(move |this, _, _, cx| {
            if running {
                cx.emit(AgentsWindowEvent::Cancel)
            } else {
                this.submit(cx)
            }
        }));
        div()
            .id("agents-window")
            .debug_selector(|| "agents-window".into())
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
                .py_1(),
            )
            .children(prompt)
            .children(changes)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap_1()
                    .p_1()
                    .border_t_1()
                    .border_color(t.border)
                    .child(input)
                    .child(send),
            )
    }
}
