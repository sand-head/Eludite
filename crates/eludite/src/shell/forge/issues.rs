//! The Issues window (View > Other Windows > Issues; work items on Azure DevOps): the repository's issues with Mine,
//! Assigned and All, the state, a search and Refresh; New Issue; and for the selected issue its description and
//! comments, a comment box, Close or Reopen, labels, and Create Branch (`issue/<n>-<slug>`, linked on the forge where
//! it records links). Every button dispatches its `eludite.forge.*` command.

use std::ops::Range;
use std::time::Instant;

use eludite_ui::{RunCommand, Theme, push_button, selector_option, text_box};
use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, UniformListScrollHandle, Window, div, px, rgb, uniform_list,
};
use serde_json::{Value, json};

use super::widgets::{self, Edit};
use super::{Detected, Provenance};

pub const ROW_HEIGHT: f32 = 36.;
pub const SEARCH_BOX: &str = "forge-issues-search";
pub const REFRESH: &str = "forge-issues-refresh";
pub const NEW_TITLE: &str = "forge-issues-new-title";
pub const NEW_CREATE: &str = "forge-issues-new-create";
pub const COMMENT_BOX: &str = "forge-issue-comment-box";
pub const COMMENT: &str = "forge-issue-comment";
pub const CLOSE: &str = "forge-issue-close";
pub const BRANCH: &str = "forge-issue-branch";
pub const SIGN_IN: &str = "forge-issues-sign-in";
pub const BANNER: &str = "forge-issues-banner";
pub const MESSAGE: &str = "forge-issues-message";

pub fn row_selector(ix: usize) -> String {
    format!("forge-issue-{ix}")
}

pub fn filter_selector(name: &str) -> String {
    format!("forge-issues-filter-{name}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssuesEvent {
    Load,
    Refresh,
    SignIn,
}

/// Which box takes the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Search,
    NewTitle,
    Comment,
}

pub struct IssuesWindow {
    theme: Theme,
    pub detected: Option<Detected>,
    pub filter: &'static str,
    pub state: &'static str,
    pub text: String,
    pub items: Vec<Value>,
    pub provenance: Provenance,
    pub loading: bool,
    pub error: Option<String>,
    pub stale: bool,
    pub drawn: Option<Instant>,
    pub selected: Option<usize>,
    /// The selected issue, as `eludite.forge.issue` answered it.
    pub issue: Option<Value>,
    pub new_title: String,
    pub comment: String,
    pub field: Field,
    pub message: Option<(String, bool)>,
    /// Where rows were drawn, while `--bounds-out` probes.
    pub probe: Option<eludite_ui::BoundsMap>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<IssuesEvent> for IssuesWindow {}

impl Focusable for IssuesWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn run(command: &str, args: Value, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
}

impl IssuesWindow {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            detected: None,
            filter: "all",
            state: "open",
            text: String::new(),
            items: Vec::new(),
            provenance: Provenance::default(),
            loading: false,
            error: None,
            stale: true,
            drawn: None,
            selected: None,
            issue: None,
            new_title: String::new(),
            comment: String::new(),
            field: Field::Search,
            message: None,
            probe: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn query(&self) -> Value {
        let mut q = json!({"filter": self.filter, "state": self.state});
        if !self.text.trim().is_empty() {
            q["text"] = json!(self.text.trim());
        }
        q
    }

    pub fn set_detected(&mut self, d: Detected, cx: &mut Context<Self>) {
        if self.detected.as_ref() != Some(&d) {
            self.detected = Some(d);
            cx.notify();
        }
    }

    pub fn set_loading(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
    }

    pub fn set_answer(&mut self, v: &Value, cx: &mut Context<Self>) {
        self.items = v["items"].as_array().cloned().unwrap_or_default();
        self.provenance = Provenance::from_json(v);
        self.loading = false;
        self.error = None;
        cx.notify();
    }

    pub fn set_issue(&mut self, v: &Value, cx: &mut Context<Self>) {
        self.issue = Some(v.clone());
        cx.notify();
    }

    pub fn set_error(&mut self, e: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(e);
        cx.notify();
    }

    pub fn set_message(&mut self, m: Option<(String, bool)>, cx: &mut Context<Self>) {
        if m.as_ref().is_some_and(|(_, failed)| !failed) {
            self.comment.clear();
            self.new_title.clear();
        }
        self.message = m;
        cx.notify();
    }

    pub fn request_refresh(&mut self, cx: &mut Context<Self>) {
        cx.emit(IssuesEvent::Refresh);
    }

    /// The selected issue's `number` or `id` input.
    pub fn selected_args(&self) -> Option<Value> {
        let i = self.items.get(self.selected?)?;
        Some(match i["number"].as_u64() {
            Some(n) => json!({"number": n}),
            None => json!({"id": i["id"]}),
        })
    }

    /// Select row `ix` and load the issue.
    pub fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        self.issue = None;
        if let Some(args) = self.selected_args() {
            run("eludite.forge.issue", args, window, cx);
        }
        cx.notify();
    }

    /// The titles on screen (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn titles(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|p| p["title"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    /// Create the issue whose title is in the box.
    pub fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.new_title.trim().to_owned();
        if title.is_empty() {
            self.field = Field::NewTitle;
            cx.notify();
            return;
        }
        run(
            "eludite.forge.issue_create",
            json!({"title": title}),
            window,
            cx,
        );
    }

    /// Post the comment in the box on the selected issue.
    pub fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.comment.trim().to_owned();
        let Some(mut args) = self.selected_args() else {
            return;
        };
        if body.is_empty() {
            self.field = Field::Comment;
            cx.notify();
            return;
        }
        args["body"] = json!(body);
        run("eludite.forge.issue_comment", args, window, cx);
    }

    /// Type into a field (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_into(&mut self, field: Field, text: &str, cx: &mut Context<Self>) {
        match field {
            Field::Search => self.text = text.to_owned(),
            Field::NewTitle => self.new_title = text.to_owned(),
            Field::Comment => self.comment = text.to_owned(),
        }
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.field;
        let text = match field {
            Field::Search => &mut self.text,
            Field::NewTitle => &mut self.new_title,
            Field::Comment => &mut self.comment,
        };
        match widgets::edit(text, e, field == Field::Comment) {
            Edit::Enter => match field {
                Field::Search => cx.emit(IssuesEvent::Load),
                Field::NewTitle => self.create(window, cx),
                Field::Comment => self.post_comment(window, cx),
            },
            Edit::Escape => text.clear(),
            Edit::Tab => {
                self.field = match field {
                    Field::Search => Field::NewTitle,
                    Field::NewTitle => Field::Comment,
                    Field::Comment => Field::Search,
                }
            }
            Edit::Changed => {}
            Edit::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        range
            .filter_map(|ix| {
                let i = self.items.get(ix)?;
                let label = match i["number"].as_u64() {
                    Some(n) => format!("#{n}"),
                    None => i["id"]
                        .as_str()
                        .unwrap_or("")
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_owned(),
                };
                let mut detail = Vec::new();
                if let Some(k) = i["type"].as_str() {
                    detail.push(k.to_owned());
                }
                if i["state"] == "closed" {
                    detail.push("Closed".into());
                }
                if let Some(a) = i["author"].as_str() {
                    detail.push(a.to_owned());
                }
                let labels: Vec<&str> = i["labels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if !labels.is_empty() {
                    detail.push(labels.join(", "));
                }
                if let Some(m) = i["milestone"].as_str() {
                    detail.push(m.to_owned());
                }
                let sel = row_selector(ix);
                let selected = self.selected == Some(ix);
                let probed = eludite_ui::bounds_canvas(self.probe.as_ref(), sel.clone());
                let row = div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .relative()
                    .children(probed)
                    .flex()
                    .flex_col()
                    .justify_center()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .border_b_1()
                    .border_color(t.border)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.select(ix, window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(div().text_color(t.text_muted).child(label))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(i["title"].as_str().unwrap_or_default().to_owned()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .child(detail.join(" \u{00B7} ")),
                    );
                Some(if selected { row.bg(t.menu_hover) } else { row }.into_any_element())
            })
            .collect()
    }

    fn detail(&self, focused: bool, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = self.theme;
        let args = self.selected_args()?;
        let caps = self
            .detected
            .as_ref()
            .map(|d| d.capabilities.clone())
            .unwrap_or_default();
        let issue = self.issue.clone();
        let closed = issue.as_ref().is_some_and(|i| i["state"] == "closed");
        let mut col = div()
            .id("forge-issue-detail")
            .debug_selector(|| "forge-issue-detail".into())
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .max_h(gpui::relative(0.6))
            .flex_shrink(1.)
            .min_h(px(60.))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(t.border);
        match &issue {
            None => col = col.child(div().text_color(t.text_muted).child("Loading\u{2026}")),
            Some(i) => {
                col = col.child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(i["title"].as_str().unwrap_or_default().to_owned()),
                );
                if let Some(fields) = i["fields"].as_object() {
                    for (k, v) in fields.iter().take(12) {
                        col = col.child(
                            div()
                                .text_size(t.typography.small)
                                .text_color(t.text_muted)
                                .child(format!("{k}: {}", v.as_str().unwrap_or_default())),
                        );
                    }
                }
                col = col.child(div().child(i["body"].as_str().unwrap_or("").to_owned()));
                for c in i["conversation"].as_array().into_iter().flatten() {
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .p_1()
                            .border_l_2()
                            .border_color(t.border)
                            .child(
                                div()
                                    .text_size(t.typography.small)
                                    .text_color(t.text_muted)
                                    .child(c["author"].as_str().unwrap_or_default().to_owned()),
                            )
                            .child(c["body"].as_str().unwrap_or_default().to_owned()),
                    );
                }
            }
        }
        let a1 = args.clone();
        let a2 = args.clone();
        col = col
            .child(
                text_box(
                    COMMENT_BOX,
                    &self.comment,
                    "Comment",
                    focused && self.field == Field::Comment,
                    &t,
                )
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.field = Field::Comment;
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(
                        push_button(COMMENT, "Comment", true, true, &t).on_click(
                            cx.listener(|this, _, window, cx| this.post_comment(window, cx)),
                        ),
                    )
                    .child(
                        push_button(
                            CLOSE,
                            if closed { "Reopen" } else { "Close issue" },
                            false,
                            true,
                            &t,
                        )
                        .on_click(cx.listener(move |_, _, window, cx| {
                            let mut a = a1.clone();
                            a["state"] = json!(if closed { "open" } else { "closed" });
                            run("eludite.forge.issue_update", a, window, cx);
                        })),
                    )
                    .children(caps.branch_from_issue.then(|| {
                        push_button(BRANCH, "Create Branch", false, true, &t).on_click(cx.listener(
                            move |_, _, window, cx| {
                                run("eludite.forge.branch_from_issue", a2.clone(), window, cx);
                            },
                        ))
                    })),
            );
        Some(col.into_any_element())
    }
}

impl Render for IssuesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        self.drawn = Some(Instant::now());
        if self.stale && !self.loading {
            self.stale = false;
            self.loading = true;
            cx.defer_in(window, |_, _, cx| cx.emit(IssuesEvent::Load));
        }
        let focused = self.focus.is_focused(window);
        let detected = self.detected.clone();
        let noun = detected
            .as_ref()
            .map(|d| d.family.issue_noun())
            .unwrap_or("issue");
        let caps = detected
            .as_ref()
            .map(|d| d.capabilities.clone())
            .unwrap_or_default();
        let toolbar = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(t.border)
            .children(
                [("mine", "Mine"), ("assigned", "Assigned"), ("all", "All")].map(|(f, label)| {
                    selector_option(filter_selector(f), label, self.filter == f, &t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.filter = f;
                            this.selected = None;
                            cx.emit(IssuesEvent::Load);
                            cx.notify();
                        }),
                    )
                }),
            )
            .child(div().w(px(8.)))
            .children(
                [
                    ("open", "Open"),
                    ("closed", "Closed"),
                    ("all_states", "All"),
                ]
                .map(|(s, label)| {
                    let state: &'static str = if s == "all_states" { "all" } else { s };
                    selector_option(filter_selector(s), label, self.state == state, &t).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.state = state;
                            this.selected = None;
                            cx.emit(IssuesEvent::Load);
                            cx.notify();
                        }),
                    )
                }),
            )
            .child(
                text_box(
                    SEARCH_BOX,
                    &self.text,
                    "Search",
                    focused && self.field == Field::Search,
                    &t,
                )
                .w(px(140.))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.field = Field::Search;
                    this.focus.focus(window, cx);
                    cx.notify();
                })),
            )
            .child(
                push_button(REFRESH, "Refresh", false, true, &t)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(IssuesEvent::Refresh))),
            );
        let new_issue = caps.issue_create.then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .p_1()
                .child(
                    text_box(
                        NEW_TITLE,
                        &self.new_title,
                        &format!("New {noun} title"),
                        focused && self.field == Field::NewTitle,
                        &t,
                    )
                    .flex_1()
                    .min_w(px(80.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.field = Field::NewTitle;
                        this.focus.focus(window, cx);
                        cx.notify();
                    })),
                )
                .child(
                    push_button(
                        NEW_CREATE,
                        format!("New {}", capital(noun)),
                        false,
                        true,
                        &t,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                )
        });
        let mut notes: Vec<gpui::AnyElement> = Vec::new();
        if let Some(d) = &detected {
            if let Some(m) = &d.message {
                notes.push(
                    div()
                        .p_2()
                        .text_color(t.text_muted)
                        .child(m.clone())
                        .into_any_element(),
                );
            } else if !d.capabilities.issues {
                notes.push(
                    div()
                        .p_2()
                        .text_color(t.text_muted)
                        .child(format!("{} has no issues here.", d.family.display()))
                        .into_any_element(),
                );
            } else if !d.signed_in {
                notes.push(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .p_1()
                        .text_size(t.typography.small)
                        .text_color(t.text_muted)
                        .child(format!("Not signed in to {}.", d.host))
                        .child(
                            push_button(
                                SIGN_IN,
                                format!("Sign in to {}", d.family.display()),
                                false,
                                true,
                                &t,
                            )
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(IssuesEvent::SignIn))),
                        )
                        .into_any_element(),
                );
            }
        }
        if let Some(b) = self.provenance.banner(self.loading) {
            notes.push(
                div()
                    .id(BANNER)
                    .debug_selector(|| BANNER.into())
                    .px_2()
                    .py_1()
                    .text_size(t.typography.small)
                    .bg(rgb(0x3A_3D_41))
                    .child(b)
                    .into_any_element(),
            );
        }
        if let Some(e) = &self.error {
            notes.push(
                div()
                    .p_2()
                    .text_color(rgb(0xF1_4C_4C))
                    .child(e.clone())
                    .into_any_element(),
            );
        }
        if let Some((m, failed)) = &self.message {
            notes.push(
                div()
                    .id(MESSAGE)
                    .debug_selector(|| MESSAGE.into())
                    .px_2()
                    .text_size(t.typography.small)
                    .text_color(if *failed {
                        rgb(0xF1_4C_4C)
                    } else {
                        t.text_muted
                    })
                    .child(m.clone())
                    .into_any_element(),
            );
        }
        let count = self.items.len();
        let detail = self.detail(focused, cx);
        div()
            .id("forge-issues")
            .debug_selector(|| "forge-issues".into())
            .track_focus(&self.focus)
            .key_context("ForgeIssues")
            .on_key_down(cx.listener(Self::key))
            .overflow_hidden()
            .size_full()
            .flex()
            .flex_col()
            .text_size(t.typography.ui)
            .text_color(t.text)
            .child(toolbar)
            .children(new_issue)
            .children(notes)
            .child(
                uniform_list(
                    "forge-issues-list",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                // Three rows stay visible beside an open issue.
                .min_h(px(ROW_HEIGHT * 3.)),
            )
            .children(detail)
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
