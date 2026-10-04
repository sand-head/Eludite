//! Tools > Options > NuGet Package Manager > Package Sources (brief 0048, Visual Studio's page of that name): the
//! sources of the NuGet.config chain with a check box each (enabled), where each is defined, and Name and Source boxes
//! with Add, and Remove for the selected source. Every change is a [`SourcesEvent`] the shell runs as
//! `eludite.nuget.sources` off the UI thread (the user's NuGet.config is the one written); the answer comes back
//! through [`PackageSourcesPage::set_sources`].

use eludite_commands::nuget::SourcesOutput;
use eludite_ui::{Theme, check_box, push_button, text_box};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div, px,
};

pub const PAGE: &str = "nuget-sources-page";
pub const NAME_BOX: &str = "nuget-sources-name";
pub const URL_BOX: &str = "nuget-sources-url";
pub const ADD: &str = "nuget-sources-add";
pub const REMOVE: &str = "nuget-sources-remove";

pub fn source_selector(name: &str) -> String {
    format!("nuget-sources-row-{name}")
}

pub fn enabled_selector(name: &str) -> String {
    format!("nuget-sources-enabled-{name}")
}

/// What the page asks the shell to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourcesEvent {
    Add { name: String, url: String },
    Remove { name: String },
    SetEnabled { name: String, enabled: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Url,
}

pub struct PackageSourcesPage {
    theme: Theme,
    sources: Option<SourcesOutput>,
    selected: Option<String>,
    name: String,
    url: String,
    field: Field,
    message: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<SourcesEvent> for PackageSourcesPage {}

impl PackageSourcesPage {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            sources: None,
            selected: None,
            name: String::new(),
            url: String::new(),
            field: Field::Name,
            message: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn set_sources(&mut self, out: SourcesOutput, cx: &mut Context<Self>) {
        if self
            .selected
            .as_ref()
            .is_some_and(|s| !out.sources.iter().any(|x| &x.name == s))
        {
            self.selected = None;
        }
        self.sources = Some(out);
        self.message = None;
        cx.notify();
    }

    pub fn set_message(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        self.message = message;
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sources(&self) -> Option<&SourcesOutput> {
        self.sources.as_ref()
    }

    /// Fill the boxes (tests and the Xvfb driver type into them as the person does).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_boxes(&mut self, name: &str, url: &str, cx: &mut Context<Self>) {
        self.name = name.to_owned();
        self.url = url.to_owned();
        cx.notify();
    }

    pub fn select(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        self.selected = name;
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let (name, url) = (self.name.trim().to_owned(), self.url.trim().to_owned());
        if name.is_empty() || url.is_empty() {
            self.message =
                Some("A source needs a name and a source (an address or a folder).".into());
            cx.notify();
            return;
        }
        self.name.clear();
        self.url.clear();
        cx.emit(SourcesEvent::Add { name, url });
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        let text = match self.field {
            Field::Name => &mut self.name,
            Field::Url => &mut self.url,
        };
        match k.key.as_str() {
            "tab" => {
                self.field = match self.field {
                    Field::Name => Field::Url,
                    Field::Url => Field::Name,
                }
            }
            "enter" => {
                self.add(cx);
                cx.stop_propagation();
                return;
            }
            "backspace" => {
                text.pop();
            }
            _ => match k.key_char.clone() {
                Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => text.push_str(&c),
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Focusable for PackageSourcesPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PackageSourcesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        let mut list = div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(t.border)
            .bg(t.background)
            .min_h(px(120.));
        match &self.sources {
            None => {
                list = list.child(
                    div()
                        .p_1()
                        .text_color(t.text_muted)
                        .child("Reading the NuGet.config files\u{2026}"),
                )
            }
            Some(out) => {
                for s in &out.sources {
                    let (name, enabled) = (s.name.clone(), s.enabled);
                    let select = name.clone();
                    let sel = source_selector(&s.name);
                    let row =
                        div()
                            .id(SharedString::from(sel.clone()))
                            .debug_selector(move || sel)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_1()
                            .cursor_pointer()
                            .child(
                                check_box(enabled_selector(&s.name), s.name.clone(), enabled, &t)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(SourcesEvent::SetEnabled {
                                            name: name.clone(),
                                            enabled: !enabled,
                                        })
                                    })),
                            )
                            .child(div().flex_1().text_color(t.text_muted).child(s.url.clone()))
                            .child(
                                div()
                                    .text_size(t.typography.small)
                                    .text_color(t.text_muted)
                                    .child(s.scope.clone()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select(Some(select.clone()), cx)
                            }));
                    list = list.child(if self.selected.as_deref() == Some(s.name.as_str()) {
                        row.bg(t.menu_hover)
                    } else {
                        row
                    });
                }
            }
        }
        let user = self
            .sources
            .as_ref()
            .map(|s| format!("Changes are written to {}.", s.user_config))
            .unwrap_or_default();
        let name_box = text_box(
            NAME_BOX,
            &self.name,
            "Name",
            focused && self.field == Field::Name,
            &t,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.field = Field::Name;
            this.focus.focus(window, cx);
            cx.notify();
        }));
        let url_box = text_box(
            URL_BOX,
            &self.url,
            "Source",
            focused && self.field == Field::Url,
            &t,
        )
        .w(px(300.))
        .on_click(cx.listener(|this, _, window, cx| {
            this.field = Field::Url;
            this.focus.focus(window, cx);
            cx.notify();
        }));
        let can_remove = self.selected.is_some();
        div()
            .id(PAGE)
            .debug_selector(|| PAGE.into())
            .track_focus(&self.focus)
            .key_context("NuGetPackageSources")
            .on_key_down(cx.listener(Self::key_down))
            .flex()
            .flex_col()
            .gap_2()
            .text_size(t.typography.ui)
            .child("Available package sources:")
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .child("Name:")
                    .child(name_box),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .child("Source:")
                    .child(url_box),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        push_button(ADD, "Add", false, true, &t)
                            .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                    )
                    .child({
                        let b = push_button(REMOVE, "Remove", false, can_remove, &t);
                        if can_remove {
                            b.on_click(cx.listener(|this, _, _, cx| {
                                if let Some(name) = this.selected.clone() {
                                    cx.emit(SourcesEvent::Remove { name });
                                }
                            }))
                        } else {
                            b
                        }
                    }),
            )
            .children(
                self.message
                    .clone()
                    .map(|m| div().text_color(t.text_muted).child(m)),
            )
            .child(
                div()
                    .text_size(t.typography.small)
                    .text_color(t.text_muted)
                    .child(user),
            )
    }
}
