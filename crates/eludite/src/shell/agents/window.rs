//! The Agents tool window (View > Agents, Ctrl+\, Ctrl+C): the agent picker and its state, login instructions, the
//! virtualized transcript, the permission prompt, the pending changes, and the prompt box (Enter sends, Shift+Enter
//! starts a new line, Escape cancels the turn). It renders and emits [`AgentsWindowEvent`]s; the shell turns them
//! into `eludite.agents.*` commands, and all agent and MCP I/O happens on other threads. A tool call's images show
//! as thumbnails under its card (brief 0024); clicking one asks the shell to open the full image. An agent's debug
//! command reads as one line above its card (brief 0027): the action and the result as the person would see them, the
//! stop's location a link that opens the file at the line, and the summary the agent received folded until expanded.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use eludite_acp::LoginMethod;
use eludite_ui::Theme;
use eludite_ui::transcript::{
    ToolCard, agent_line, notice, plan_card, thought_block, tool_call_card, user_prompt,
};
use gpui::{
    AnyElement, App, Bounds, Context, Div, EventEmitter, FocusHandle, Focusable, FollowMode,
    FontWeight, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, ListAlignment,
    ListState, ParentElement, Pixels, Render, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, anchored, canvas, deferred, div, img, list, px,
    rgb,
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

pub struct AgentsWindow {
    theme: Theme,
    pub transcript: Transcript,
    list: ListState,
    pub input: String,
    focus: FocusHandle,
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
        Self {
            theme,
            transcript: Transcript::default(),
            list,
            input: String::new(),
            focus: cx.focus_handle(),
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

    /// Send the prompt box's text.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.input.trim().to_owned();
        if text.is_empty() || self.running() {
            return;
        }
        self.input.clear();
        cx.emit(AgentsWindowEvent::Prompt(text));
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "enter" if k.modifiers.shift => self.input.push('\n'),
            "enter" => self.submit(cx),
            "escape" => cx.emit(AgentsWindowEvent::Cancel),
            "backspace" => {
                self.input.pop();
            }
            "space" => self.input.push(' '),
            _ => {
                // Platforms report the typed text in `key_char`; a bare one-character key (tests) is the text too.
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if k.modifiers.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                        self.input.push_str(&c)
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = self.theme;
        let Some(row) = self.transcript.rows.get(ix) else {
            return div().into_any_element();
        };
        match row {
            Row::User(text) => user_prompt(text.clone(), &t).into_any_element(),
            Row::Agent(text) => agent_line(text.clone(), &t).into_any_element(),
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
        let start = eludite_ui::push_button(
            START_BUTTON,
            if restart { "Restart" } else { "Start" },
            false,
            true,
            &t,
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
            buttons = buttons.child(button(Decision::AlwaysAllow, "Always Allow", false));
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
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
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
        let focused = self.focus.is_focused(window);
        let running = self.running();
        let shown: SharedString = if self.input.is_empty() && !focused {
            "Ask the agent\u{2026} (Enter to send, Esc to stop)".into()
        } else if focused {
            format!("{}\u{2502}", self.input).into()
        } else {
            self.input.clone().into()
        };
        let header = self.render_header(cx);
        let prompt = self.render_prompt(cx);
        let changes = self.render_changes(cx);
        let input = tracked(&self.painted, PROMPT_BOX, div().id(PROMPT_BOX))
            .debug_selector(|| PROMPT_BOX.into())
            .key_context("AgentsPrompt")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_click(cx.listener(|this, _, window, cx| {
                this.focus.focus(window, cx);
                cx.notify();
            }))
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .min_h(px(40.))
            .px_1()
            .border_1()
            .border_color(if focused { t.accent } else { t.border })
            .bg(t.background)
            .text_color(if self.input.is_empty() && !focused {
                t.text_muted
            } else {
                t.text
            })
            .child(shown);
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
                    cx.processor(|this, ix, _, cx| this.render_row(ix, cx)),
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
