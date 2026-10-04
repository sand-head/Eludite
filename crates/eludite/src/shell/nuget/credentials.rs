//! NuGet's credential prompt (brief 0048, brief 0045's dialog pattern): when a private feed asks for credentials that
//! no credential provider answered during the person's call, the host sends `eludite/nuget/credentials` and this
//! dialog asks for the feed's user name and password or personal access token. OK answers the host, which keeps the
//! answer in memory for the session (never written); Cancel gives up. An agent's call never opens it: the host refuses
//! it with `credentials_required` instead.
//!
//! Keys: Tab moves between the boxes, Enter is OK, Escape is Cancel. The password box shows bullets.

use eludite_ui::{Theme, dialog_panel, push_button, text_box};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, point, px, rgb,
};

pub const DIALOG: &str = "nuget-credential-prompt";
pub const USER_BOX: &str = "nuget-credential-user";
pub const PASSWORD_BOX: &str = "nuget-credential-password";
pub const OK: &str = "nuget-credential-ok";
pub const CANCEL: &str = "nuget-credential-cancel";

const WIDTH: f32 = 460.;

/// Which box takes the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    User,
    Password,
}

/// What the dialog tells the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialEvent {
    Ok { username: String, password: String },
    Cancel,
}

/// The dialog's state; the shell owns what happens on OK and Cancel.
pub struct CredentialPrompt {
    theme: Theme,
    source: String,
    host: String,
    /// The answer given before was refused.
    refused: bool,
    username: String,
    password: String,
    field: Field,
    focus: FocusHandle,
}

impl CredentialPrompt {
    pub fn new(
        theme: Theme,
        source: String,
        host: String,
        refused: bool,
        username: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = if username.is_empty() {
            Field::User
        } else {
            Field::Password
        };
        Self {
            theme,
            source,
            host,
            refused,
            username,
            password: String::new(),
            field,
            focus: cx.focus_handle(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn host(&self) -> &str {
        &self.host
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn refused(&self) -> bool {
        self.refused
    }

    /// The line under the title: what the host asks, or that the last answer was refused.
    pub fn message(&self) -> String {
        if self.refused {
            format!(
                "{} refused the user name and password or token. Enter them again.",
                self.host
            )
        } else {
            format!(
                "The package source {} ({}) asks for a user name and a password or personal access token.",
                self.source, self.host
            )
        }
    }

    fn ok(&mut self, cx: &mut Context<Self>) {
        if self.username.is_empty() {
            self.field = Field::User;
            cx.notify();
            return;
        }
        cx.emit(CredentialEvent::Ok {
            username: self.username.clone(),
            password: self.password.clone(),
        });
    }

    fn text(&mut self) -> &mut String {
        match self.field {
            Field::User => &mut self.username,
            Field::Password => &mut self.password,
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "escape" => cx.emit(CredentialEvent::Cancel),
            "enter" => self.ok(cx),
            "tab" => {
                self.field = match self.field {
                    Field::User => Field::Password,
                    Field::Password => Field::User,
                };
            }
            "backspace" => {
                self.text().pop();
            }
            _ => {
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
                        self.text().push_str(&c);
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl EventEmitter<CredentialEvent> for CredentialPrompt {}

impl Focusable for CredentialPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CredentialPrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        let bullets = "\u{2022}".repeat(self.password.chars().count());
        let user_box = text_box(
            USER_BOX,
            &self.username,
            "",
            focused && self.field == Field::User,
            &t,
        )
        .w(px(300.))
        .on_click(cx.listener(|this, _, _, cx| {
            this.field = Field::User;
            cx.notify();
        }));
        let password_box = text_box(
            PASSWORD_BOX,
            &bullets,
            "",
            focused && self.field == Field::Password,
            &t,
        )
        .w(px(300.))
        .on_click(cx.listener(|this, _, _, cx| {
            this.field = Field::Password;
            cx.notify();
        }));
        let row = |label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .child(div().w(px(120.)).child(label))
        };
        let panel = dialog_panel(&t, format!("NuGet Credentials: {}", self.source))
            .id(DIALOG)
            .debug_selector(|| DIALOG.into())
            .track_focus(&self.focus)
            .key_context("NuGetCredentialPrompt")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .w(px(WIDTH))
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .text_color(if self.refused {
                        rgb(0xF1_4C_4C)
                    } else {
                        t.text
                    })
                    .child(self.message()),
            )
            .child(row("User name:").child(user_box))
            .child(row("Password or token:").child(password_box))
            .child(
                div()
                    .px_3()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(
                        "Kept in memory for this session and passed to restores; never written to disk.",
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(
                        push_button(OK, "OK", true, true, &t)
                            .on_click(cx.listener(|this, _, _, cx| this.ok(cx))),
                    )
                    .child(
                        push_button(CANCEL, "Cancel", false, true, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CredentialEvent::Cancel))),
                    ),
            );
        let viewport = window.viewport_size();
        let at = point(
            ((viewport.width - px(WIDTH)) / 2.).max(px(0.)),
            (viewport.height / 5.).max(px(0.)),
        );
        deferred(anchored().position(at).child(panel)).with_priority(5)
    }
}
