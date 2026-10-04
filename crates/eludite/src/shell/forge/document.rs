//! The pull request document (opening one in the Pull Requests window, the status bar's pull request): its title,
//! state and branches; the description and conversation as Markdown (brief 0043's renderer, links clickable); the
//! commits; the files changed with their additions and deletions (a double-click compares the file's base and head
//! texts in brief 0040's Compare document); the review threads; the checks with their logs and reruns; and the
//! actions the forge allows: Check Out, comment, the review (pending comments collected, submitted together as
//! Approve, Request Changes or Comment), Mark Ready, Merge with a method, Close. Lists are virtualized, so a pull
//! request of hundreds of files and threads draws only what is on screen.

use std::ops::Range;
use std::rc::Rc;

use eludite_forge::{ItemRef, Pull};
use eludite_ui::markdown;
use eludite_ui::{RunCommand, Theme, push_button, selector_option, text_box};
use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, UniformListScrollHandle, Window, div, px, rgb, uniform_list,
};
use serde_json::{Value, json};

use super::widgets::{self, Edit};
use super::{Detected, Provenance, item_args};

pub const ROW_HEIGHT: f32 = 26.;
pub const REFRESH: &str = "forge-pr-refresh";
pub const CHECKOUT: &str = "forge-pr-checkout";
pub const READY: &str = "forge-pr-ready";
pub const MERGE: &str = "forge-pr-merge";
pub const CLOSE: &str = "forge-pr-close";
pub const COMMENT_BOX: &str = "forge-pr-comment-box";
pub const COMMENT: &str = "forge-pr-comment";
pub const START_REVIEW: &str = "forge-pr-start-review";
pub const MESSAGE: &str = "forge-pr-message";
pub const BANNER: &str = "forge-pr-banner";

pub fn tab_selector(tab: Tab) -> String {
    format!("forge-pr-tab-{}", tab.name())
}

pub fn submit_selector(event: &str) -> String {
    format!("forge-pr-submit-{event}")
}

pub fn method_selector(method: &str) -> String {
    format!("forge-pr-method-{method}")
}

pub fn file_selector(ix: usize) -> String {
    format!("forge-pr-file-{ix}")
}

pub fn check_log_selector(ix: usize) -> String {
    format!("forge-pr-check-log-{ix}")
}

pub fn resolve_selector(ix: usize) -> String {
    format!("forge-pr-resolve-{ix}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Commits,
    Files,
    Threads,
    Checks,
}

impl Tab {
    pub fn name(self) -> &'static str {
        match self {
            Tab::Overview => "overview",
            Tab::Commits => "commits",
            Tab::Files => "files",
            Tab::Threads => "threads",
            Tab::Checks => "checks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentEvent {
    /// Compare a file's base and head texts.
    Compare {
        path: String,
        base: Option<String>,
        head: Option<String>,
    },
    OpenLink(String),
}

pub struct PullDocument {
    theme: Theme,
    pub item: ItemRef,
    pub detected: Detected,
    pub pull: Option<Pull>,
    pub provenance: Provenance,
    pub message: Option<(String, bool)>,
    pub tab: Tab,
    pub comment: String,
    pub method: Option<String>,
    /// Comments of the review waiting to be submitted (what `pull` answered, plus those added since).
    pub pending: u64,
    /// The rendered description, parsed once per answer.
    description: Vec<markdown::Block>,
    conversation: Vec<(String, Vec<markdown::Block>)>,
    /// Where the tabs were drawn, while `--bounds-out` probes.
    pub probe: Option<eludite_ui::BoundsMap>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<DocumentEvent> for PullDocument {}

impl Focusable for PullDocument {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn run(command: &str, args: Value, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
}

impl PullDocument {
    pub fn new(theme: Theme, item: ItemRef, detected: Detected, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            item,
            detected,
            pull: None,
            provenance: Provenance::default(),
            message: None,
            tab: Tab::Overview,
            comment: String::new(),
            method: None,
            pending: 0,
            description: Vec::new(),
            conversation: Vec::new(),
            probe: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn set_pull(
        &mut self,
        pull: Option<Pull>,
        provenance: Provenance,
        detected: Detected,
        cx: &mut Context<Self>,
    ) {
        if detected.supported() {
            self.detected = detected;
        }
        if let Some(p) = &pull {
            self.description = markdown::parse(p.body.as_deref().unwrap_or(""));
            self.conversation = p
                .conversation
                .iter()
                .map(|c| (c.author.clone(), markdown::parse(&c.body)))
                .collect();
            self.pending = p.pending_review.as_ref().map(|r| r.comments).unwrap_or(0);
            if self.method.is_none() {
                self.method = p
                    .mergeable
                    .as_ref()
                    .and_then(|m| m.methods.first())
                    .map(|m| m.as_str().to_owned());
            }
        }
        self.pull = pull;
        self.provenance = provenance;
        cx.notify();
    }

    pub fn set_message(&mut self, text: String, failed: bool, cx: &mut Context<Self>) {
        self.message = Some((text, failed));
        cx.notify();
    }

    /// A write on this pull request finished.
    pub fn after_write(
        &mut self,
        command: &str,
        result: &Result<Value, eludite_commands::CommandError>,
        text: String,
        failed: bool,
        cx: &mut Context<Self>,
    ) {
        if let Ok(v) = result {
            if let Some(n) = v["pending_comments"].as_u64() {
                self.pending = n;
            }
            if command == eludite_commands::forge::PULL_COMMENT
                || command == eludite_commands::forge::PULL_REVIEW
            {
                self.comment.clear();
            }
        }
        self.set_message(text, failed, cx);
    }

    pub fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Type the comment (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_comment(&mut self, text: &str, cx: &mut Context<Self>) {
        self.comment = text.to_owned();
        cx.notify();
    }

    /// Scroll the list to row `ix` (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn scroll_to(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.scroll.scroll_to_item(ix, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    fn args(&self) -> Value {
        item_args(&self.item)
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match widgets::edit(&mut self.comment, e, true) {
            Edit::Enter => self.post_comment(window, cx),
            Edit::Escape => self.comment.clear(),
            Edit::Changed => {}
            Edit::Tab | Edit::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.comment.trim().to_owned();
        if body.is_empty() {
            return;
        }
        let mut a = self.args();
        a["body"] = json!(body);
        run("eludite.forge.pull_comment", a, window, cx);
    }

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        let Some(p) = &self.pull else {
            return Vec::new();
        };
        let caps = self.detected.capabilities.clone();
        let row = |id: String| {
            let sel = id.clone();
            div()
                .id(SharedString::from(id))
                .debug_selector(move || sel)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(ROW_HEIGHT))
                .px_2()
                .border_b_1()
                .border_color(t.border)
                .overflow_hidden()
                .whitespace_nowrap()
        };
        match self.tab {
            Tab::Commits => range
                .filter_map(|ix| {
                    let c = p.commits.get(ix)?;
                    Some(
                        row(format!("forge-pr-commit-{ix}"))
                            .child(
                                div()
                                    .text_color(t.text_muted)
                                    .child(c.sha.get(..7).unwrap_or(&c.sha).to_owned()),
                            )
                            .child(div().flex_1().child(c.title.clone()))
                            .child(
                                div()
                                    .text_color(t.text_muted)
                                    .child(c.author.clone().unwrap_or_default()),
                            )
                            .into_any_element(),
                    )
                })
                .collect(),
            Tab::Files => range
                .filter_map(|ix| {
                    let f = p.files.get(ix)?;
                    let (glyph, color) = match f.status {
                        eludite_forge::FileStatus::Added => ("+", rgb(0x4E_C9_4E)),
                        eludite_forge::FileStatus::Deleted => ("\u{2212}", rgb(0xF1_4C_4C)),
                        eludite_forge::FileStatus::Renamed => ("R", rgb(0x56_9C_D6)),
                        _ => ("\u{2713}", rgb(0xD7_BA_7D)),
                    };
                    let path = f.path.clone();
                    let base = p.base_sha.clone();
                    let head = p.head_sha.clone();
                    let threads = p
                        .threads
                        .iter()
                        .filter(|t| t.path.as_deref() == Some(f.path.as_str()))
                        .count();
                    Some(
                        row(file_selector(ix))
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, e: &ClickEvent, _, cx| {
                                if e.click_count() >= 2 {
                                    cx.emit(DocumentEvent::Compare {
                                        path: path.clone(),
                                        base: base.clone(),
                                        head: head.clone(),
                                    });
                                }
                            }))
                            .child(div().w(px(12.)).text_color(color).child(glyph))
                            .child(div().flex_1().child(match &f.old_path {
                                Some(o) => format!("{o} \u{2192} {}", f.path),
                                None => f.path.clone(),
                            }))
                            .children((threads > 0).then(|| {
                                div()
                                    .text_color(t.text_muted)
                                    .child(format!("\u{1F4AC} {threads}"))
                            }))
                            .child(
                                div()
                                    .text_color(rgb(0x4E_C9_4E))
                                    .child(format!("+{}", f.additions)),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0xF1_4C_4C))
                                    .child(format!("\u{2212}{}", f.deletions)),
                            )
                            .into_any_element(),
                    )
                })
                .collect(),
            Tab::Threads => range
                .filter_map(|ix| {
                    let th = p.threads.get(ix)?;
                    let first = th.comments.first()?;
                    let where_ = match (&th.path, th.line) {
                        (Some(path), Some(line)) => format!("{path}:{line}"),
                        (Some(path), None) => path.clone(),
                        _ => "conversation".into(),
                    };
                    let id = th.id.clone();
                    let args = self.args();
                    Some(
                        row(format!("forge-pr-thread-{ix}"))
                            .child(
                                div()
                                    .text_color(if th.resolved {
                                        t.text_muted
                                    } else {
                                        rgb(0xD7_BA_7D)
                                    })
                                    .child(if th.resolved { "Resolved" } else { "Open" }),
                            )
                            .child(div().text_color(t.text_muted).child(where_))
                            .child(div().flex_1().overflow_hidden().child(format!(
                                "{}: {}",
                                first.author,
                                first.body.lines().next().unwrap_or("")
                            )))
                            .child(
                                div()
                                    .text_color(t.text_muted)
                                    .child(format!("{}", th.comments.len())),
                            )
                            .children((caps.thread_resolution).then(|| {
                                let resolved = th.resolved;
                                push_button(
                                    resolve_selector(ix),
                                    if resolved { "Unresolve" } else { "Resolve" },
                                    false,
                                    true,
                                    &t,
                                )
                                .on_click(cx.listener(
                                    move |_, _, window, cx| {
                                        let mut args = args.clone();
                                        args["thread"] = json!(id);
                                        args["resolved"] = json!(!resolved);
                                        run("eludite.forge.thread_resolve", args, window, cx);
                                    },
                                ))
                            }))
                            .into_any_element(),
                    )
                })
                .collect(),
            Tab::Checks => range
                .filter_map(|ix| {
                    let c = p.check_items.get(ix)?;
                    let (glyph, color) = widgets::conclusion_glyph(
                        serde_json::to_value(c.status).ok()?.as_str()?,
                        serde_json::to_value(c.conclusion).ok()?.as_str()?,
                    );
                    let id = c.id.clone();
                    let name = c.name.clone();
                    let rid = c.id.clone();
                    Some(
                        row(format!("forge-pr-check-{ix}"))
                            .child(div().w(px(12.)).text_color(color).child(glyph))
                            .child(div().flex_1().child(c.name.clone()))
                            .children(
                                c.required
                                    .then(|| div().text_color(t.text_muted).child("Required")),
                            )
                            .children(c.duration_seconds.map(|d| {
                                div().text_color(t.text_muted).child(format!(
                                    "{}m {}s",
                                    d / 60,
                                    d % 60
                                ))
                            }))
                            .children(c.has_log.then(|| {
                                push_button(check_log_selector(ix), "Log", false, true, &t)
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        run(
                                            "eludite.forge.check_log",
                                            json!({"id": id, "name": name}),
                                            window,
                                            cx,
                                        );
                                    }))
                            }))
                            .children(c.can_rerun.then(|| {
                                push_button(
                                    format!("forge-pr-check-rerun-{ix}"),
                                    "Rerun",
                                    false,
                                    true,
                                    &t,
                                )
                                .on_click(cx.listener(
                                    move |_, _, window, cx| {
                                        run(
                                            "eludite.forge.check_rerun",
                                            json!({"id": rid}),
                                            window,
                                            cx,
                                        );
                                    },
                                ))
                            }))
                            .into_any_element(),
                    )
                })
                .collect(),
            Tab::Overview => Vec::new(),
        }
    }

    fn overview(&self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let Some(p) = &self.pull else {
            return div()
                .p_2()
                .text_color(t.text_muted)
                .child("Loading\u{2026}")
                .into_any_element();
        };
        let font = window.text_style().font();
        let mono = gpui::font(eludite_editor::default_font_family());
        let this = cx.entity().downgrade();
        let on_link: markdown::OnLink = Rc::new(move |url, _, cx| {
            let url = url.to_owned();
            let _ = this.update(cx, |_, cx| cx.emit(DocumentEvent::OpenLink(url)));
        });
        let mut col = div()
            .id("forge-pr-overview")
            .debug_selector(|| "forge-pr-overview".into())
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(markdown::render_linked(
                "forge-pr-description",
                &self.description,
                &t,
                &font,
                &mono,
                on_link.clone(),
            ));
        if !p.reviews.is_empty() {
            let mut reviews = div().flex().flex_col().gap_1().child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Reviewers"),
            );
            for r in &p.reviews {
                let state = serde_json::to_value(r.state)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                reviews = reviews.child(div().text_size(t.typography.small).child(format!(
                    "{} \u{00B7} {}",
                    r.author,
                    state.replace('_', " ")
                )));
            }
            col = col.child(reviews);
        }
        if let Some(a) = &p.approvals {
            col = col.child(
                div()
                    .text_size(t.typography.small)
                    .child(format!("Approvals: {} of {} required", a.given, a.required)),
            );
        }
        for (ix, (author, blocks)) in self.conversation.iter().enumerate() {
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
                            .child(author.clone()),
                    )
                    .child(markdown::render_linked(
                        format!("forge-pr-comment-{ix}"),
                        blocks,
                        &t,
                        &font,
                        &mono,
                        on_link.clone(),
                    )),
            );
        }
        col.into_any_element()
    }
}

impl Render for PullDocument {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let caps = self.detected.capabilities.clone();
        let focused = self.focus.is_focused(window);
        let args = self.args();
        let header = match &self.pull {
            None => div().p_2().child(format!(
                "{} {}",
                self.detected.family.pull_noun(),
                self.item.label()
            )),
            Some(p) => {
                let s = &p.summary;
                let state = match (s.state, s.draft) {
                    (eludite_forge::State::Merged, _) => "Merged",
                    (eludite_forge::State::Closed, _) => "Closed",
                    (_, true) => "Draft",
                    _ => "Open",
                };
                let (glyph, color) = widgets::checks_glyph(
                    s.checks
                        .and_then(|c| serde_json::to_value(c).ok())
                        .as_ref()
                        .and_then(Value::as_str),
                );
                let mergeable = p.mergeable.as_ref().map(|m| {
                    serde_json::to_value(m.state)
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
                        .unwrap_or_default()
                });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("{} {}", s.label(), s.title)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .child(state)
                            .child(format!(
                                "{} wants to merge {} into {}",
                                s.author, s.head, s.base
                            ))
                            .child(div().text_color(color).child(glyph))
                            .children(mergeable.map(|m| format!("Mergeable: {m}")))
                            .children(
                                p.unresolved_threads
                                    .filter(|n| *n > 0)
                                    .map(|n| format!("{n} unresolved thread(s)")),
                            )
                            .children(p.iterations.map(|n| format!("{n} iteration(s)"))),
                    )
            }
        };
        let open = self
            .pull
            .as_ref()
            .is_some_and(|p| p.summary.state == eludite_forge::State::Open);
        let draft = self.pull.as_ref().is_some_and(|p| p.summary.draft);
        let methods: Vec<String> = self
            .pull
            .as_ref()
            .and_then(|p| p.mergeable.as_ref())
            .map(|m| m.methods.iter().map(|x| x.as_str().to_owned()).collect())
            .filter(|m: &Vec<String>| !m.is_empty())
            .unwrap_or_else(|| {
                caps.merge_methods
                    .iter()
                    .map(|m| m.as_str().to_owned())
                    .collect()
            });
        let mut toolbar = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(t.border)
            .child(
                push_button(REFRESH, "Refresh", false, true, &t).on_click(cx.listener(
                    |this, _, window, cx| {
                        let mut a = this.args();
                        a["what"] = json!(["pull"]);
                        run("eludite.forge.refresh", a, window, cx);
                    },
                )),
            )
            .child({
                let a = args.clone();
                push_button(CHECKOUT, "Check Out", false, true, &t).on_click(cx.listener(
                    move |_, _, window, cx| {
                        run("eludite.forge.pull_checkout", a.clone(), window, cx)
                    },
                ))
            });
        if open && draft && caps.draft_pull_requests {
            let a = args.clone();
            toolbar = toolbar.child(
                push_button(READY, "Ready for review", false, true, &t).on_click(cx.listener(
                    move |_, _, window, cx| run("eludite.forge.pull_ready", a.clone(), window, cx),
                )),
            );
        }
        if open && !methods.is_empty() {
            for m in &methods {
                let mm = m.clone();
                toolbar = toolbar.child(
                    selector_option(
                        method_selector(m),
                        m.replace('_', " "),
                        self.method.as_deref() == Some(m.as_str()),
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.method = Some(mm.clone());
                        cx.notify();
                    })),
                );
            }
            let a = args.clone();
            toolbar = toolbar.child(push_button(MERGE, "Merge", true, true, &t).on_click(
                cx.listener(move |this, _, window, cx| {
                    let mut a = a.clone();
                    a["method"] = json!(this.method.clone().unwrap_or_else(|| "merge".into()));
                    run("eludite.forge.pull_merge", a, window, cx);
                }),
            ));
        }
        if open {
            let a = args.clone();
            toolbar = toolbar.child(push_button(CLOSE, "Close", false, true, &t).on_click(
                cx.listener(move |_, _, window, cx| {
                    run("eludite.forge.pull_close", a.clone(), window, cx)
                }),
            ));
        } else if self.pull.is_some() {
            let mut a = args.clone();
            a["reopen"] = json!(true);
            toolbar = toolbar.child(push_button(CLOSE, "Reopen", false, true, &t).on_click(
                cx.listener(move |_, _, window, cx| {
                    run("eludite.forge.pull_close", a.clone(), window, cx)
                }),
            ));
        }
        let review = caps.reviews.then(|| {
            let mut r = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_1()
                .p_1()
                .border_b_1()
                .border_color(t.border)
                .child(
                    div()
                        .text_color(t.text_muted)
                        .child(if caps.pending_reviews {
                            format!("Review: {} pending comment(s)", self.pending)
                        } else {
                            "Review (each comment posts at once)".to_owned()
                        }),
                );
            let a = args.clone();
            r = r.child(
                push_button(START_REVIEW, "Start Review", false, true, &t).on_click(cx.listener(
                    move |_, _, window, cx| {
                        let mut a = a.clone();
                        a["action"] = json!("start");
                        run("eludite.forge.pull_review", a, window, cx);
                    },
                )),
            );
            for (event, label) in [
                ("approve", "Approve"),
                ("request_changes", "Request Changes"),
                ("comment", "Comment"),
            ] {
                let a = args.clone();
                r = r.child(
                    push_button(
                        submit_selector(event),
                        format!("Submit: {label}"),
                        false,
                        true,
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let mut a = a.clone();
                        a["action"] = json!("submit");
                        a["event"] = json!(event);
                        let body = this.comment.trim().to_owned();
                        if !body.is_empty() {
                            a["body"] = json!(body);
                        }
                        run("eludite.forge.pull_review", a, window, cx);
                    })),
                );
            }
            r
        });
        let count = |tab: Tab| -> usize {
            self.pull
                .as_ref()
                .map(|p| match tab {
                    Tab::Commits => p.commits.len(),
                    Tab::Files => p.files.len(),
                    Tab::Threads => p.threads.len(),
                    Tab::Checks => p.check_items.len(),
                    Tab::Overview => 0,
                })
                .unwrap_or(0)
        };
        let mut tabs = div()
            .flex()
            .flex_row()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(t.border);
        for tab in [
            Tab::Overview,
            Tab::Commits,
            Tab::Files,
            Tab::Threads,
            Tab::Checks,
        ] {
            if (tab == Tab::Threads && !caps.review_threads) || (tab == Tab::Checks && !caps.checks)
            {
                continue;
            }
            let label = match tab {
                Tab::Overview => "Overview".to_owned(),
                other => format!("{} ({})", capital(other.name()), count(other)),
            };
            tabs = tabs.child(
                selector_option(tab_selector(tab), label, self.tab == tab, &t)
                    .relative()
                    .children(eludite_ui::bounds_canvas(
                        self.probe.as_ref(),
                        tab_selector(tab),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_tab(tab, cx))),
            );
        }
        let body = if self.tab == Tab::Overview {
            self.overview(window, cx)
        } else {
            let n = count(self.tab);
            uniform_list(
                "forge-pr-list",
                n,
                cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        let comment = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .p_1()
            .border_t_1()
            .border_color(t.border)
            .child(
                text_box(
                    COMMENT_BOX,
                    &self.comment,
                    "Leave a comment (Shift+Enter for a new line)",
                    focused,
                    &t,
                )
                .flex_1()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.focus.focus(window, cx);
                    cx.notify();
                })),
            )
            .child(
                push_button(COMMENT, "Comment", true, true, &t)
                    .on_click(cx.listener(|this, _, window, cx| this.post_comment(window, cx))),
            );
        div()
            .id("forge-pr")
            .debug_selector(|| "forge-pr".into())
            .track_focus(&self.focus)
            .key_context("ForgePullRequest")
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .flex_col()
            .text_size(t.typography.ui)
            .text_color(t.text)
            .bg(t.background)
            .child(header)
            .children(self.provenance.banner(false).map(|b| {
                div()
                    .id(BANNER)
                    .debug_selector(|| BANNER.into())
                    .px_2()
                    .py_1()
                    .text_size(t.typography.small)
                    .bg(rgb(0x3A_3D_41))
                    .child(b)
            }))
            .children(self.message.clone().map(|(m, failed)| {
                div()
                    .id(MESSAGE)
                    .debug_selector(|| MESSAGE.into())
                    .px_2()
                    .py_1()
                    .text_size(t.typography.small)
                    .text_color(if failed {
                        rgb(0xF1_4C_4C)
                    } else {
                        t.text_muted
                    })
                    .child(m)
            }))
            .child(toolbar)
            .children(review)
            .child(tabs)
            .child(body)
            .child(comment)
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
