//! The Create Pull Request form (Git > Create Pull Request; the Git Changes window's "Create a Pull Request" link):
//! a document prefilled from the current branch: the title from its single commit (or the branch's name), the
//! description from its commits, the base from the remote's default branch; Draft, reviewers and labels where the
//! forge offers them. Create runs `eludite.forge.pull_create`.

use eludite_forge::{Capabilities, Family};
use eludite_ui::{Theme, check_box, push_button, text_box};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use serde_json::{Value, json};

use super::widgets::{self, Edit};

pub const TITLE_BOX: &str = "forge-create-title";
pub const BODY_BOX: &str = "forge-create-body";
pub const BASE_BOX: &str = "forge-create-base";
pub const REVIEWERS_BOX: &str = "forge-create-reviewers";
pub const LABELS_BOX: &str = "forge-create-labels";
pub const DRAFT: &str = "forge-create-draft";
pub const CREATE: &str = "forge-create-create";
pub const MESSAGE: &str = "forge-create-message";

#[derive(Debug, Clone, PartialEq)]
pub enum CreateEvent {
    /// Create with this `eludite.forge.pull_create` input.
    Create(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Body,
    Base,
    Reviewers,
    Labels,
}

pub struct CreatePullForm {
    theme: Theme,
    pub head: Option<String>,
    pub base: String,
    pub title: String,
    pub body: String,
    pub reviewers: String,
    pub labels: String,
    pub draft: bool,
    pub family: Family,
    pub capabilities: Capabilities,
    pub prefilled: bool,
    pub message: Option<String>,
    pub field: Field,
    focus: FocusHandle,
}

impl EventEmitter<CreateEvent> for CreatePullForm {}

impl Focusable for CreatePullForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CreatePullForm {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            head: None,
            base: String::new(),
            title: String::new(),
            body: String::new(),
            reviewers: String::new(),
            labels: String::new(),
            draft: false,
            family: Family::None,
            capabilities: Capabilities::default(),
            prefilled: false,
            message: None,
            field: Field::Title,
            focus: cx.focus_handle(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prefill(
        &mut self,
        head: Option<String>,
        base: String,
        title: String,
        body: String,
        family: Family,
        capabilities: Capabilities,
        cx: &mut Context<Self>,
    ) {
        if head.is_none() {
            self.message = Some("HEAD is detached: check out the branch to propose first.".into());
        } else if family == Family::None {
            self.message = Some("No supported forge for this repository's remote.".into());
        } else if !capabilities.pull_create {
            self.message = Some(format!(
                "{} does not support creating pull requests from Eludite.",
                family.display()
            ));
        }
        self.head = head;
        self.base = base;
        self.title = title;
        self.body = body;
        self.family = family;
        self.capabilities = capabilities;
        self.prefilled = true;
        cx.notify();
    }

    pub fn set_message(&mut self, m: String, cx: &mut Context<Self>) {
        self.message = Some(m);
        cx.notify();
    }

    /// The input Create sends.
    pub fn input(&self) -> Value {
        let list = |s: &str| -> Vec<String> {
            s.split([',', ' '])
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let mut v =
            json!({"title": self.title.trim(), "body": self.body, "base": self.base.trim()});
        if let Some(h) = &self.head {
            v["head"] = json!(h);
        }
        if self.draft && self.capabilities.draft_pull_requests {
            v["draft"] = json!(true);
        }
        let reviewers = list(&self.reviewers);
        if !reviewers.is_empty() {
            v["reviewers"] = json!(reviewers);
        }
        let labels = list(&self.labels);
        if !labels.is_empty() && self.capabilities.labels {
            v["labels"] = json!(labels);
        }
        v
    }

    pub fn create(&mut self, cx: &mut Context<Self>) {
        if self.title.trim().is_empty() {
            self.field = Field::Title;
            self.message = Some("A title is needed.".into());
            cx.notify();
            return;
        }
        self.message = Some("Creating\u{2026}".into());
        cx.emit(CreateEvent::Create(self.input()));
        cx.notify();
    }

    fn text(&mut self) -> &mut String {
        match self.field {
            Field::Title => &mut self.title,
            Field::Body => &mut self.body,
            Field::Base => &mut self.base,
            Field::Reviewers => &mut self.reviewers,
            Field::Labels => &mut self.labels,
        }
    }

    fn key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let multiline = self.field == Field::Body;
        let field = self.field;
        match widgets::edit(self.text(), e, multiline) {
            Edit::Enter => self.create(cx),
            Edit::Tab => {
                self.field = match field {
                    Field::Title => Field::Body,
                    Field::Body => Field::Base,
                    Field::Base => Field::Reviewers,
                    Field::Reviewers => Field::Labels,
                    Field::Labels => Field::Title,
                }
            }
            Edit::Escape | Edit::Changed => {}
            Edit::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for CreatePullForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        let noun = self.family.pull_noun();
        let row = |label: &'static str| {
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .px_3()
                .py_1()
                .child(div().w(px(110.)).child(label))
        };
        let field_box = |this: &Self,
                         id: &'static str,
                         text: &str,
                         placeholder: &str,
                         f: Field,
                         cx: &mut Context<Self>| {
            text_box(id, text, placeholder, focused && this.field == f, &t)
                .w(px(460.))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.field = f;
                    this.focus.focus(window, cx);
                    cx.notify();
                }))
        };
        let body_lines: Vec<String> = self.body.lines().map(str::to_owned).collect();
        let body = div()
            .id(BODY_BOX)
            .debug_selector(|| BODY_BOX.into())
            .w(px(460.))
            .min_h(px(120.))
            .p_1()
            .border_1()
            .border_color(if focused && self.field == Field::Body {
                t.accent
            } else {
                t.border
            })
            .bg(t.background)
            .cursor_text()
            .on_click(cx.listener(|this, _, window, cx| {
                this.field = Field::Body;
                this.focus.focus(window, cx);
                cx.notify();
            }))
            .children(body_lines.into_iter().map(|l| div().child(l)));
        let head = self.head.clone().unwrap_or_else(|| "(detached)".into());
        let can = self.prefilled && self.head.is_some() && self.capabilities.pull_create;
        div()
            .id("forge-create")
            .debug_selector(|| "forge-create".into())
            .track_focus(&self.focus)
            .key_context("ForgeCreatePull")
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .text_size(t.typography.ui)
            .text_color(t.text)
            .bg(t.background)
            .child(div().text_size(px(16.)).child(format!("Create a {noun}")))
            .child(
                div()
                    .px_3()
                    .text_color(t.text_muted)
                    .child(format!("From {head} into {}", self.base)),
            )
            .child(row("Title:").child(field_box(
                self,
                TITLE_BOX,
                &self.title.clone(),
                "Title",
                Field::Title,
                cx,
            )))
            .child(row("Description:").child(body))
            .child(row("Base:").child(field_box(
                self,
                BASE_BOX,
                &self.base.clone(),
                "main",
                Field::Base,
                cx,
            )))
            .children(self.capabilities.request_review.then(|| {
                row("Reviewers:").child(field_box(
                    self,
                    REVIEWERS_BOX,
                    &self.reviewers.clone(),
                    "logins, separated by commas",
                    Field::Reviewers,
                    cx,
                ))
            }))
            .children(self.capabilities.labels.then(|| {
                row("Labels:").child(field_box(
                    self,
                    LABELS_BOX,
                    &self.labels.clone(),
                    "labels",
                    Field::Labels,
                    cx,
                ))
            }))
            .children(self.capabilities.draft_pull_requests.then(|| {
                div().px_3().child(
                    check_box(DRAFT, "Create as draft", self.draft, &t).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.draft = !this.draft;
                            cx.notify();
                        },
                    )),
                )
            }))
            .children(self.message.clone().map(|m| {
                div()
                    .id(MESSAGE)
                    .debug_selector(|| MESSAGE.into())
                    .px_3()
                    .text_color(rgb(0xD7_BA_7D))
                    .child(m)
            }))
            .child(div().px_3().py_2().child(
                push_button(CREATE, "Create", true, can, &t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if can {
                            this.create(cx)
                        }
                    },
                )),
            ))
    }
}
