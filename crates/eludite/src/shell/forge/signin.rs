//! The sign-in dialog (Git > Sign in to the forge; the windows' Sign in button), per host: the methods the forge
//! takes (a browser through the OAuth device flow: GitHub, GitLab, Azure DevOps; a personal access token; the `gh` or
//! `glab` CLI's token; Tangled's handle and app password), the device code with its url and a Copy button while the
//! flow waits, and, when the operating system's credential store is unavailable, the consent to keep the token in a
//! file only the person can read. The token box shows bullets; nothing typed is kept by the dialog after it closes.

use eludite_commands::CommandError;
use eludite_forge::Family;
use eludite_ui::{Theme, check_box, dialog_panel, push_button, selector_option, text_box};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, point, px, rgb,
};
use serde_json::{Value, json};

use super::widgets::{self, Edit};

pub const DIALOG: &str = "forge-sign-in";
pub const TOKEN_BOX: &str = "forge-sign-in-token";
pub const USER_BOX: &str = "forge-sign-in-user";
pub const FILE_CONSENT: &str = "forge-sign-in-file";
pub const SIGN_IN: &str = "forge-sign-in-ok";
pub const CANCEL: &str = "forge-sign-in-cancel";
pub const COPY: &str = "forge-sign-in-copy";
pub const OPEN: &str = "forge-sign-in-open";
pub const CODE: &str = "forge-sign-in-code";
pub const MESSAGE: &str = "forge-sign-in-message";

pub fn method_selector(m: &str) -> String {
    format!("forge-sign-in-method-{m}")
}

const WIDTH: f32 = 500.;

#[derive(Debug, Clone, PartialEq)]
pub enum SignInEvent {
    Submit(Value),
    Cancel,
    Copy(String),
    OpenUrl(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Token,
    User,
}

pub struct SignInDialog {
    theme: Theme,
    pub host: String,
    pub family: Family,
    pub method: &'static str,
    token: String,
    user: String,
    field: Field,
    pub allow_file: bool,
    /// The credential store answered unavailable: offer the file.
    pub offer_file: bool,
    /// The device flow's code and url while it waits.
    pub device: Option<(String, String)>,
    pub message: Option<(String, bool)>,
    /// Where its controls were drawn, while `--bounds-out` probes.
    pub probe: Option<eludite_ui::BoundsMap>,
    focus: FocusHandle,
}

impl EventEmitter<SignInEvent> for SignInDialog {}

impl Focusable for SignInDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The methods `family` takes, in the order the dialog offers them: (input value, label).
pub fn methods(family: Family) -> Vec<(&'static str, &'static str)> {
    match family {
        Family::GitHub => vec![
            ("device", "Sign in with a browser"),
            ("token", "Personal access token"),
            ("cli", "GitHub CLI (gh)"),
        ],
        Family::GitLab => vec![
            ("device", "Sign in with a browser"),
            ("token", "Personal access token"),
            ("cli", "GitLab CLI (glab)"),
        ],
        Family::AzureDevOps => vec![
            ("device", "Sign in with a browser"),
            ("token", "Personal access token"),
        ],
        Family::Tangled => vec![("app_password", "App password")],
        _ => vec![("token", "Access token")],
    }
}

impl SignInDialog {
    pub fn new(theme: Theme, host: String, family: Family, cx: &mut Context<Self>) -> Self {
        let method = methods(family).first().map(|m| m.0).unwrap_or("token");
        Self {
            theme,
            host,
            family,
            method,
            token: String::new(),
            user: String::new(),
            field: if family == Family::Tangled {
                Field::User
            } else {
                Field::Token
            },
            allow_file: false,
            offer_file: false,
            device: None,
            message: None,
            probe: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn set_method(&mut self, method: &'static str, cx: &mut Context<Self>) {
        self.method = method;
        self.message = None;
        cx.notify();
    }

    /// Type the token and the user (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn type_token(&mut self, token: &str, user: Option<&str>, cx: &mut Context<Self>) {
        self.token = token.to_owned();
        if let Some(u) = user {
            self.user = u.to_owned();
        }
        cx.notify();
    }

    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let mut args = json!({"action": "sign_in", "host": self.host, "method": self.method});
        if self.allow_file {
            args["allow_file_store"] = json!(true);
        }
        match self.method {
            "token" | "app_password" => {
                if self.token.is_empty() {
                    self.field = Field::Token;
                    self.message = Some(("Enter the token.".into(), true));
                    cx.notify();
                    return;
                }
                args["token"] = json!(self.token);
                if self.method == "app_password" {
                    args["user"] = json!(self.user.trim());
                }
            }
            _ => {}
        }
        self.message = Some(("Signing in\u{2026}".into(), false));
        cx.emit(SignInEvent::Submit(args));
        cx.notify();
    }

    /// The answer of a sign-in.
    pub fn set_answer(&mut self, result: &Result<Value, CommandError>, cx: &mut Context<Self>) {
        match result {
            Ok(v) if v["pending"] == true => {
                let code = v
                    .pointer("/device/user_code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let url = v
                    .pointer("/device/verification_uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.message = Some((
                    format!(
                        "Enter the code at {url}, then come back: Eludite finishes on its own."
                    ),
                    false,
                ));
                self.device = Some((code, url));
            }
            Ok(v) if v["signed_in"] == true => {
                self.token.clear();
                self.message = Some((
                    v["message"].as_str().unwrap_or("Signed in").to_owned(),
                    false,
                ));
            }
            Ok(_) => {}
            Err(e) => {
                let text = e.to_string();
                if text.contains("store_unavailable") {
                    self.offer_file = true;
                }
                self.message = Some((text.trim_start_matches("command failed: ").to_owned(), true));
            }
        }
        cx.notify();
    }

    /// The device flow ended (the hub's event).
    pub fn finished(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        self.device = None;
        self.message = Some(match result {
            Ok(login) => (format!("Signed in as {login}"), false),
            Err(e) => (e, true),
        });
        cx.notify();
    }

    fn key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let field = self.field;
        let text = match field {
            Field::Token => &mut self.token,
            Field::User => &mut self.user,
        };
        match widgets::edit(text, e, false) {
            Edit::Enter => self.submit(cx),
            Edit::Escape => cx.emit(SignInEvent::Cancel),
            Edit::Tab => {
                self.field = if field == Field::Token && self.method == "app_password" {
                    Field::User
                } else {
                    Field::Token
                }
            }
            Edit::Changed => {}
            Edit::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for SignInDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        let mut panel = dialog_panel(
            &t,
            format!("Sign in to {} ({})", self.family.display(), self.host),
        )
        .id(DIALOG)
        .debug_selector(|| DIALOG.into())
        .track_focus(&self.focus)
        .key_context("ForgeSignIn")
        .on_key_down(cx.listener(Self::key))
        .occlude()
        .w(px(WIDTH));
        let mut choices = div().flex().flex_row().flex_wrap().gap_1().px_3().pt_3();
        for (m, label) in methods(self.family) {
            choices = choices.child(
                selector_option(method_selector(m), label, self.method == m, &t)
                    .relative()
                    .children(eludite_ui::bounds_canvas(
                        self.probe.as_ref(),
                        method_selector(m),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_method(m, cx))),
            );
        }
        panel = panel.child(choices);
        let row = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .child(div().w(px(130.)).child(label))
        };
        if self.method == "app_password" {
            panel = panel.child(
                row("Handle:").child(
                    text_box(
                        USER_BOX,
                        &self.user,
                        "alice.example.com",
                        focused && self.field == Field::User,
                        &t,
                    )
                    .w(px(300.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.field = Field::User;
                        cx.notify();
                    })),
                ),
            );
        }
        if matches!(self.method, "token" | "app_password") {
            let bullets = "\u{2022}".repeat(self.token.chars().count());
            panel = panel.child(
                row(if self.method == "app_password" {
                    "App password:"
                } else {
                    "Token:"
                })
                .child(
                    text_box(
                        TOKEN_BOX,
                        &bullets,
                        "",
                        focused && self.field == Field::Token,
                        &t,
                    )
                    .w(px(300.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.field = Field::Token;
                        cx.notify();
                    })),
                ),
            );
        }
        if let Some((code, url)) = self.device.clone() {
            let (c, u) = (code.clone(), url.clone());
            panel = panel.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .id(CODE)
                            .debug_selector(|| CODE.into())
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(code),
                    )
                    .child(push_button(COPY, "Copy", false, true, &t).on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(SignInEvent::Copy(c.clone()))),
                    ))
                    .child(
                        push_button(OPEN, "Open the page", false, true, &t).on_click(
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(SignInEvent::OpenUrl(u.clone()))
                            }),
                        ),
                    ),
            );
        }
        if self.offer_file {
            panel = panel.child(div().px_3().py_1().child(
                check_box(FILE_CONSENT, "The credential store is unavailable: keep the token in a file only I can read", self.allow_file, &t)
                    .relative()
                    .children(eludite_ui::bounds_canvas(self.probe.as_ref(), FILE_CONSENT))
                    .on_click(cx.listener(|this, _, _, cx| {
                    this.allow_file = !this.allow_file;
                    cx.notify();
                })),
            ));
        }
        panel = panel
            .child(div().px_3().text_size(t.typography.small).text_color(t.text_muted).child(
                "The token is kept in the operating system's credential store, never in a log or a workspace file.",
            ))
            .children(self.message.clone().map(|(m, failed)| {
                div().id(MESSAGE).debug_selector(|| MESSAGE.into()).px_3().py_1().text_color(if failed { rgb(0xF1_4C_4C) } else { t.text }).child(m)
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(push_button(SIGN_IN, "Sign In", true, self.device.is_none(), &t)
                        .relative()
                        .children(eludite_ui::bounds_canvas(self.probe.as_ref(), SIGN_IN))
                        .on_click(cx.listener(|this, _, _, cx| {
                        if this.device.is_none() {
                            this.submit(cx)
                        }
                    })))
                    .child(push_button(CANCEL, "Cancel", false, true, &t).on_click(cx.listener(|_, _, _, cx| cx.emit(SignInEvent::Cancel)))),
            );
        let viewport = window.viewport_size();
        let at = point(
            ((viewport.width - px(WIDTH)) / 2.).max(px(0.)),
            (viewport.height / 5.).max(px(0.)),
        );
        deferred(anchored().position(at).child(panel)).with_priority(5)
    }
}
