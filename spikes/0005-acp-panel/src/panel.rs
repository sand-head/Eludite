//! The Agents panel: header with agent status, a virtualized transcript
//! (messages, tool calls with arguments and results, permission prompts) and
//! a prompt box. It only renders and routes clicks; all agent and MCP I/O
//! happens in `session` on other threads.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, FollowMode, IntoElement, KeyDownEvent,
    ListAlignment, ListState, ParentElement, Render, Rgba, SharedString, Styled, Task, Window, div,
    list, prelude::*, px, rgb,
};
use serde_json::Value;

use crate::bench::{Probes, ms, wall_ns};
use crate::session::{AgentSession, AgentStatus, PanelEvent, SessionConfig, Tagged};
use crate::transcript::{PermissionState, Row, Transcript, status_label};

pub struct Colors;
impl Colors {
    pub const BG: u32 = 0x1E1E1E;
    pub const PANEL: u32 = 0x252526;
    pub const HEADER: u32 = 0x2D2D30;
    pub const ACCENT: u32 = 0x007ACC;
    pub const BORDER: u32 = 0x3F3F46;
    pub const TEXT: u32 = 0xD4D4D4;
    pub const MUTED: u32 = 0x9D9D9D;
    pub const OK: u32 = 0x89D185;
    pub const WARN: u32 = 0xCCA700;
    pub const ERR: u32 = 0xF48771;
    pub const USER: u32 = 0x264F78;
}

fn c(hex: u32) -> Rgba {
    rgb(hex)
}

/// A harness hook run on the UI thread.
pub type Hook = Box<dyn FnMut(&mut Panel, &mut Context<Panel>)>;

/// Hooks for the harness: answer prompts automatically (the session may be
/// unattended) and observe turn ends.
#[derive(Default)]
pub struct Automation {
    /// Answer permission prompts after showing them for this long: (allow, delay).
    pub auto_answer: Option<(bool, Duration)>,
    pub on_turn_end: Option<Hook>,
    pub on_ready: Option<Hook>,
}

pub struct Panel {
    config: SessionConfig,
    session: Option<AgentSession>,
    generation: u64,
    pub status: AgentStatus,
    pub transcript: Transcript,
    list: ListState,
    pub input: String,
    focus: FocusHandle,
    pub running: bool,
    sink: async_channel::Sender<Tagged>,
    _pump: Task<()>,
    pub probes: Rc<RefCell<Probes>>,
    pub timings: Vec<(&'static str, f64)>,
    pub opened_at: Instant,
    pub mcp_calls: Vec<String>,
    pub last_stop: Option<Result<String, String>>,
    pub automation: Automation,
    font: SharedString,
    mono: SharedString,
}

impl Panel {
    pub fn new(config: SessionConfig, cx: &mut Context<Self>) -> Self {
        let (sink, rx) = async_channel::unbounded::<Tagged>();
        // The pump: wakes on the UI thread when events arrive and applies
        // everything queued as one batch, so a burst costs one frame.
        let pump = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |p, cx| p.apply_batch(batch, cx)).is_err() {
                    break;
                }
            }
        });
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        list.set_follow_mode(FollowMode::Tail);
        let (font, mono) = fonts();
        Self {
            config,
            session: None,
            generation: 0,
            status: AgentStatus::NotStarted,
            transcript: Transcript::default(),
            list,
            input: String::new(),
            focus: cx.focus_handle(),
            running: false,
            sink,
            _pump: pump,
            probes: Rc::default(),
            timings: Vec::new(),
            opened_at: Instant::now(),
            mcp_calls: Vec::new(),
            last_stop: None,
            automation: Automation::default(),
            font,
            mono,
        }
    }

    /// Spawn the agent (if not running) without prompting. The panel does
    /// this on the first Send; the ready-time benchmark calls it directly.
    pub fn ensure_session(&mut self, cx: &mut Context<Self>) {
        if self
            .session
            .as_ref()
            .is_some_and(|_| !matches!(self.status, AgentStatus::Exited | AgentStatus::Failed(_)))
        {
            return;
        }
        if let Some(old) = self.session.take() {
            old.shutdown();
        }
        self.generation += 1;
        self.timings.clear();
        self.opened_at = Instant::now();
        self.session = Some(AgentSession::start(
            self.config.clone(),
            self.generation,
            self.sink.clone(),
        ));
        cx.notify();
    }

    /// Send the prompt box (or `text`) to the agent.
    pub fn send(&mut self, text: Option<String>, cx: &mut Context<Self>) {
        let text = text.unwrap_or_else(|| std::mem::take(&mut self.input));
        let text = text.trim().to_owned();
        if text.is_empty() || self.running {
            return;
        }
        self.ensure_session(cx);
        self.transcript.user(&text);
        self.running = true;
        if let Some(s) = &self.session {
            s.prompt(&text);
        }
        self.sync_list();
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.session {
            s.cancel();
        }
        for key in self.transcript.pending_permissions() {
            self.transcript.answer_permission(key, false, "Cancelled");
        }
        self.sync_list();
        cx.notify();
    }

    /// The user's answer to a permission prompt.
    pub fn answer(&mut self, key: u64, allow: bool, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        if let Some(option) = session.answer(key, allow) {
            eprintln!(
                "[audit] permission {} by user: {option}",
                if allow { "allowed" } else { "denied" }
            );
            self.transcript.answer_permission(key, allow, &option);
            self.sync_list();
            cx.notify();
        }
    }

    fn sync_list(&mut self) {
        if let Some(s) = self.transcript.take_splice() {
            self.list.splice(s.start..s.old_end, s.new_end - s.start);
        }
    }

    fn apply_batch(&mut self, batch: Vec<Tagged>, cx: &mut Context<Self>) {
        let t = Instant::now();
        let n = batch.len();
        let recording = self.probes.borrow().recording;
        let mut turn_ended = false;
        let mut became_ready = false;
        for Tagged { generation, event } in batch {
            if generation != self.generation {
                continue; // stale: from a session this panel already replaced
            }
            match event {
                PanelEvent::Status(s) => {
                    became_ready |= matches!(s, AgentStatus::Ready { .. });
                    if let AgentStatus::LoginRequired { label, methods } = &s {
                        let mut text = format!(
                            "Claude Code is not logged in ({label}). Log in from a terminal, then send again:"
                        );
                        for (name, desc, cmd) in methods {
                            text.push_str(&format!("\n  {name}: {desc}\n    {cmd}"));
                        }
                        self.transcript.notice(text);
                    }
                    if let AgentStatus::Failed(e) = &s {
                        self.transcript.error(e.clone());
                        self.running = false;
                    }
                    if s == AgentStatus::Exited && self.running {
                        self.transcript.error("The agent process exited.");
                        self.running = false;
                    }
                    self.status = s;
                }
                PanelEvent::Timing { name, ms } => self.timings.push((name, ms)),
                PanelEvent::Update { update, sent_at_ns } => {
                    if recording && let Some(sent) = sent_at_ns {
                        let mut p = self.probes.borrow_mut();
                        p.chunk_to_apply_ms
                            .push((wall_ns().saturating_sub(sent)) as f64 / 1e6);
                        p.unpresented.push(sent);
                    }
                    self.transcript.apply(&update);
                }
                PanelEvent::PermissionPrompt { key, request } => {
                    eprintln!(
                        "[ui] permission prompt shown: {}",
                        request
                            .tool_call
                            .agent_tool_name()
                            .or(request.tool_call.title.as_deref())
                            .unwrap_or("?")
                    );
                    self.transcript.permission(key, &request, None);
                    if let Some((allow, delay)) = self.automation.auto_answer {
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(delay).await;
                            let _ = this.update(cx, |p, cx| p.answer(key, allow, cx));
                        })
                        .detach();
                    }
                }
                PanelEvent::AutoAllowed {
                    key,
                    request,
                    command,
                } => {
                    eprintln!("[audit] permission auto-allowed: {command} is class read");
                    self.transcript.permission(key, &request, Some(command));
                }
                PanelEvent::McpCall {
                    command,
                    permission,
                    arguments,
                    thread,
                    ok,
                    ms,
                } => {
                    self.mcp_calls.push(format!("{command} ({permission}) args={arguments} ok={ok} {ms:.2} ms thread={thread}"));
                }
                PanelEvent::TurnEnded(r) => {
                    self.running = false;
                    match &r {
                        Ok(stop) => self.transcript.notice(format!("Turn ended: {stop:?}")),
                        Err(e) => self.transcript.error(format!("Prompt failed: {e}")),
                    }
                    self.last_stop = Some(r.map(|s| format!("{s:?}")));
                    turn_ended = true;
                }
                PanelEvent::Stderr(line) => {
                    if std::env::var_os("SPIKE_AGENT_STDERR").is_some() {
                        eprintln!("[agent] {line}");
                    }
                }
            }
        }
        self.sync_list();
        if recording {
            let mut p = self.probes.borrow_mut();
            p.apply_ms.push(ms(t.elapsed()));
            p.batch_sizes.push(n);
        }
        cx.notify();
        if became_ready && let Some(mut f) = self.automation.on_ready.take() {
            f(self, cx);
            self.automation.on_ready = Some(f);
        }
        if turn_ended && let Some(mut f) = self.automation.on_turn_end.take() {
            f(self, cx);
            self.automation.on_turn_end = Some(f);
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        match ks.key.as_str() {
            "enter" => self.send(None, cx),
            "backspace" => {
                self.input.pop();
            }
            "escape" => self.cancel(cx),
            _ => match &ks.key_char {
                Some(s) if !m.control && !m.alt && !m.platform && !s.is_empty() => {
                    self.input.push_str(s)
                }
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn status_line(&self) -> (String, u32) {
        match &self.status {
            AgentStatus::NotStarted => (
                "Not started. The agent starts when you send a prompt.".into(),
                Colors::MUTED,
            ),
            AgentStatus::Starting => ("Starting the agent...".into(), Colors::WARN),
            AgentStatus::Ready {
                agent,
                version,
                protocol,
                mcp,
            } => (
                format!("{agent} ready ({version}, ACP v{protocol}). MCP: {mcp}"),
                Colors::OK,
            ),
            AgentStatus::LoginRequired { label, .. } => {
                (format!("Login required: {label}"), Colors::ERR)
            }
            AgentStatus::Failed(e) => (format!("Failed: {e}"), Colors::ERR),
            AgentStatus::Exited => ("Agent exited.".into(), Colors::MUTED),
        }
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.transcript.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let base = div().w_full().px_3();
        match row {
            Row::User(t) => base
                .py_2()
                .child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(c(Colors::USER))
                        .text_color(gpui::white())
                        .child(SharedString::from(t)),
                )
                .into_any_element(),
            Row::Agent(t) => {
                if t.is_empty() {
                    base.h(px(10.)).into_any_element()
                } else {
                    base.child(SharedString::from(t)).into_any_element()
                }
            }
            Row::Thought(t) => base
                .italic()
                .text_color(c(Colors::MUTED))
                .child(SharedString::from(t))
                .into_any_element(),
            Row::Notice(t) => base
                .py_1()
                .text_sm()
                .text_color(c(Colors::MUTED))
                .font_family(self.mono.clone())
                .child(SharedString::from(t))
                .into_any_element(),
            Row::Error(t) => base
                .py_1()
                .text_color(c(Colors::ERR))
                .child(SharedString::from(t))
                .into_any_element(),
            Row::Tool(t) => {
                let name = t
                    .agent_tool_name()
                    .or(t.title.as_deref())
                    .unwrap_or("tool")
                    .to_owned();
                let status = status_label(t.status);
                let color = match status {
                    "completed" => Colors::OK,
                    "failed" => Colors::ERR,
                    _ => Colors::WARN,
                };
                let args = t
                    .raw_input
                    .as_ref()
                    .map(pretty)
                    .unwrap_or_else(|| "(none)".into());
                let mut result = t.content_text();
                if result.chars().count() > 3000 {
                    result = result.chars().take(3000).collect::<String>() + " ...";
                }
                base.py_1()
                    .child(
                        div()
                            .id(("tool", ix))
                            .debug_selector(move || format!("tool-{ix}"))
                            .border_1()
                            .border_color(c(Colors::BORDER))
                            .rounded_md()
                            .bg(c(Colors::PANEL))
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(div().text_color(c(Colors::ACCENT)).child("Tool call"))
                                    .child(
                                        div()
                                            .font_family(self.mono.clone())
                                            .child(SharedString::from(name)),
                                    )
                                    .child(div().text_color(c(Colors::MUTED)).child(
                                        SharedString::from(t.kind.clone().unwrap_or_default()),
                                    ))
                                    .child(div().text_color(c(color)).child(status)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(c(Colors::MUTED))
                                    .child("Arguments"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(self.mono.clone())
                                    .child(SharedString::from(args)),
                            )
                            .when(!result.is_empty(), |d| {
                                d.child(
                                    div().text_sm().text_color(c(Colors::MUTED)).child("Result"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(self.mono.clone())
                                        .child(SharedString::from(result)),
                                )
                            }),
                    )
                    .into_any_element()
            }
            Row::Permission(p) => {
                let args = p.arguments.as_ref().map(pretty).unwrap_or_default();
                let key = p.key;
                let mut card = div()
                    .border_1()
                    .rounded_md()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().child(SharedString::from(format!(
                        "Permission requested: {}",
                        p.title
                    ))))
                    .child(
                        div()
                            .text_sm()
                            .font_family(self.mono.clone())
                            .child(SharedString::from(args)),
                    );
                card =
                    match &p.state {
                        PermissionState::Pending => {
                            card.border_color(c(Colors::WARN))
                                .child(div().flex().gap_2().children(
                                    p.options.iter().enumerate().map(|(i, o)| {
                                        let allow = o.kind.is_allow();
                                        let sel = if allow {
                                            format!("perm-{key}-allow-{i}")
                                        } else {
                                            format!("perm-{key}-deny")
                                        };
                                        div()
                                            .id(SharedString::from(sel.clone()))
                                            .debug_selector(move || sel.clone())
                                            .px_2()
                                            .py_1()
                                            .rounded_sm()
                                            .cursor_pointer()
                                            .bg(c(if allow {
                                                Colors::ACCENT
                                            } else {
                                                Colors::HEADER
                                            }))
                                            .text_color(gpui::white())
                                            .child(SharedString::from(o.name.clone()))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.answer(key, allow, cx)
                                            }))
                                    }),
                                ))
                        }
                        PermissionState::Answered { allowed, option } => {
                            card.border_color(c(Colors::BORDER)).child(
                                div()
                                    .text_color(c(if *allowed { Colors::OK } else { Colors::ERR }))
                                    .child(SharedString::from(format!(
                                        "{} by you: {option}",
                                        if *allowed { "Allowed" } else { "Denied" }
                                    ))),
                            )
                        }
                        PermissionState::Auto { command } => card
                            .border_color(c(Colors::BORDER))
                            .child(div().text_color(c(Colors::OK)).child(SharedString::from(
                                format!("Allowed without prompt: `{command}` is class read"),
                            ))),
                    };
                base.py_1().child(card).into_any_element()
            }
        }
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn fonts() -> (SharedString, SharedString) {
    let ui = std::env::var("SPIKE_FONT").unwrap_or_else(|_| {
        if cfg!(windows) {
            "Segoe UI".into()
        } else if cfg!(target_os = "macos") {
            ".SystemUIFont".into()
        } else {
            "Noto Sans".into()
        }
    });
    let mono = if cfg!(windows) {
        "Consolas"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "Noto Sans Mono"
    };
    (ui.into(), mono.into())
}

impl Focusable for Panel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::env::var_os("SPIKE_DEBUG").is_some() {
            eprintln!(
                "[render] rows={} running={} status={:?}",
                self.transcript.rows.len(),
                self.running,
                self.status_line().0
            );
        }
        {
            let mut p = self.probes.borrow_mut();
            if p.recording {
                let start = Instant::now();
                let unpresented = std::mem::take(&mut p.unpresented);
                let probes = self.probes.clone();
                // Runs after this frame is drawn and presented.
                cx.defer(move |_| {
                    let mut p = probes.borrow_mut();
                    p.frames += 1;
                    p.frame_work_ms.push(ms(start.elapsed()));
                    let now = wall_ns();
                    for sent in unpresented {
                        p.chunk_to_present_ms
                            .push(now.saturating_sub(sent) as f64 / 1e6);
                    }
                });
            }
        }
        let (status, status_color) = self.status_line();
        let running = self.running;
        let input = if self.input.is_empty() && !running {
            div()
                .text_color(c(Colors::MUTED))
                .child("Ask the agent... (Enter to send, Esc to stop)")
        } else {
            div()
                .child(SharedString::from(self.input.clone()))
                .child(div().w(px(2.)).h(px(18.)).bg(c(Colors::TEXT)))
        };
        div()
            .id("panel")
            .key_context("AgentPanel")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .flex()
            .flex_col()
            .bg(c(Colors::BG))
            .text_color(c(Colors::TEXT))
            .font_family(self.font.clone())
            .text_size(px(14.))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .bg(c(Colors::HEADER))
                    .border_b_1()
                    .border_color(c(Colors::BORDER))
                    .child(div().child(SharedString::from(format!(
                        "Agents: {}",
                        self.config.agent.name
                    ))))
                    .child(
                        div()
                            .debug_selector(|| "status".into())
                            .text_sm()
                            .text_color(c(status_color))
                            .child(SharedString::from(status)),
                    ),
            )
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix, _window, cx| this.render_row(ix, cx)),
                )
                .flex_1()
                .py_2(),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(c(Colors::BORDER))
                    .bg(c(Colors::PANEL))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(c(Colors::ACCENT))
                            .rounded_sm()
                            .child(input),
                    )
                    .child(
                        div()
                            .id("send")
                            .debug_selector(|| "send".into())
                            .px_3()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(c(if running {
                                Colors::HEADER
                            } else {
                                Colors::ACCENT
                            }))
                            .text_color(gpui::white())
                            .child(if running { "Stop" } else { "Send" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.running {
                                    this.cancel(cx)
                                } else {
                                    this.send(None, cx)
                                }
                            })),
                    ),
            )
    }
}
