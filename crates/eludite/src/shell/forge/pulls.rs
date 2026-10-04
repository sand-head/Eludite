//! The Pull Requests window (View > Other Windows > Pull Requests; Visual Studio's): the repository's pull requests
//! (merge requests on GitLab) with Mine, Review requested and All, the state, a search, Refresh, and a row per pull
//! request with its number, title, author, branches and checks. A double-click (or Open) opens its document. The
//! list draws from the cache at once ("Showing cached results from ..." while a refresh runs or when it failed); the
//! shell loads it when the window is drawn the first time and when a filter changes.

use std::ops::Range;
use std::time::Instant;

use eludite_forge::ItemRef;
use eludite_ui::{Theme, push_button, selector_option, text_box};
use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, UniformListScrollHandle, Window, div, px, rgb, uniform_list,
};
use serde_json::{Value, json};

use super::widgets::{self, Edit};
use super::{Detected, Provenance};

pub const ROW_HEIGHT: f32 = 40.;
pub const SEARCH_BOX: &str = "forge-pulls-search";
pub const REFRESH: &str = "forge-pulls-refresh";
pub const OPEN: &str = "forge-pulls-open";
pub const SIGN_IN: &str = "forge-pulls-sign-in";
pub const BANNER: &str = "forge-pulls-banner";

/// The selector of row `ix`.
pub fn row_selector(ix: usize) -> String {
    format!("forge-pull-{ix}")
}

/// The selector of filter `name` (`mine`, `review_requested`, `all`, and the states).
pub fn filter_selector(name: &str) -> String {
    format!("forge-pulls-filter-{name}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullsEvent {
    /// Read the list (from the cache, refreshing off-thread).
    Load,
    /// Refresh it now.
    Refresh,
    Open(ItemRef),
    SignIn,
}

pub struct PullsWindow {
    theme: Theme,
    pub detected: Option<Detected>,
    pub filter: &'static str,
    pub state: &'static str,
    pub text: String,
    pub items: Vec<Value>,
    pub next_cursor: Option<String>,
    pub provenance: Provenance,
    pub loading: bool,
    pub error: Option<String>,
    /// Load when drawn next.
    pub stale: bool,
    /// When it was last drawn (the refresh timer runs only for a visible window).
    pub drawn: Option<Instant>,
    pub selected: Option<usize>,
    search_focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<PullsEvent> for PullsWindow {}

impl Focusable for PullsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.search_focus.clone()
    }
}

impl PullsWindow {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            detected: None,
            filter: "all",
            state: "open",
            text: String::new(),
            items: Vec::new(),
            next_cursor: None,
            provenance: Provenance::default(),
            loading: false,
            error: None,
            stale: true,
            drawn: None,
            selected: None,
            search_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// The `eludite.forge.pulls` input for the filters on screen.
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
        self.next_cursor = v["next_cursor"].as_str().map(str::to_owned);
        self.provenance = Provenance::from_json(v);
        self.loading = false;
        self.error = None;
        if self.selected.is_some_and(|s| s >= self.items.len()) {
            self.selected = None;
        }
        cx.notify();
    }

    pub fn set_error(&mut self, e: String, cx: &mut Context<Self>) {
        self.loading = false;
        self.error = Some(e);
        cx.notify();
    }

    /// Ask for a refresh the next time it is drawn (the refresh timer, a write).
    pub fn request_refresh(&mut self, cx: &mut Context<Self>) {
        cx.emit(PullsEvent::Refresh);
    }

    fn set_filter(&mut self, filter: &'static str, cx: &mut Context<Self>) {
        self.filter = filter;
        self.selected = None;
        cx.emit(PullsEvent::Load);
        cx.notify();
    }

    fn set_state(&mut self, state: &'static str, cx: &mut Context<Self>) {
        self.state = state;
        self.selected = None;
        cx.emit(PullsEvent::Load);
        cx.notify();
    }

    fn item(&self, ix: usize) -> Option<ItemRef> {
        let p = self.items.get(ix)?;
        Some(match p["number"].as_u64() {
            Some(n) => ItemRef::Number(n),
            None => ItemRef::Id(p["id"].as_str()?.to_owned()),
        })
    }

    /// Open row `ix` (tests and the keyboard).
    pub fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(item) = self.item(ix) {
            cx.emit(PullsEvent::Open(item));
        }
    }

    fn search_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match widgets::edit(&mut self.text, e, false) {
            Edit::Enter => cx.emit(PullsEvent::Load),
            Edit::Escape => {
                self.text.clear();
                cx.emit(PullsEvent::Load);
            }
            Edit::Changed => {}
            Edit::Tab | Edit::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Type `text` into the search box and apply it (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn search(&mut self, text: &str, cx: &mut Context<Self>) {
        self.text = text.to_owned();
        cx.emit(PullsEvent::Load);
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

    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        range
            .filter_map(|ix| {
                let p = self.items.get(ix)?;
                let label = match p["number"].as_u64() {
                    Some(n) => format!("#{n}"),
                    None => p["id"]
                        .as_str()
                        .unwrap_or("")
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_owned(),
                };
                let (glyph, color) = widgets::checks_glyph(p["checks"].as_str());
                let draft = if p["draft"] == true {
                    "Draft \u{00B7} "
                } else {
                    ""
                };
                let state = match p["state"].as_str() {
                    Some("merged") => "Merged \u{00B7} ",
                    Some("closed") => "Closed \u{00B7} ",
                    _ => "",
                };
                let detail = format!(
                    "{state}{draft}{} \u{00B7} {} \u{2192} {}{}",
                    p["author"].as_str().unwrap_or_default(),
                    p["head"].as_str().unwrap_or_default(),
                    p["base"].as_str().unwrap_or_default(),
                    p["updated_at"]
                        .as_str()
                        .map(|u| format!(" \u{00B7} updated {u}"))
                        .unwrap_or_default()
                );
                let selected = self.selected == Some(ix);
                let sel = row_selector(ix);
                Some(
                    div()
                        .id(SharedString::from(sel.clone()))
                        .debug_selector(move || sel)
                        .flex()
                        .flex_col()
                        .justify_center()
                        .h(px(ROW_HEIGHT))
                        .px_2()
                        .border_b_1()
                        .border_color(t.border)
                        .when_selected(selected, &t)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                            this.selected = Some(ix);
                            if e.click_count() >= 2 {
                                this.open(ix, cx);
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(div().text_color(t.text_muted).child(label))
                                .child(
                                    div()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(p["title"].as_str().unwrap_or_default().to_owned()),
                                )
                                .child(div().text_color(color).child(glyph)),
                        )
                        .child(
                            div()
                                .text_size(t.typography.small)
                                .text_color(t.text_muted)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(detail),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}

/// Selection background for a row.
trait Selected {
    fn when_selected(self, selected: bool, t: &Theme) -> Self;
}

impl<E: Styled> Selected for E {
    fn when_selected(self, selected: bool, t: &Theme) -> Self {
        if selected {
            self.bg(t.menu_hover)
        } else {
            self
        }
    }
}

impl Render for PullsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        self.drawn = Some(Instant::now());
        if self.stale && !self.loading {
            self.stale = false;
            self.loading = true;
            cx.defer_in(window, |_, _, cx| cx.emit(PullsEvent::Load));
        }
        let detected = self.detected.clone();
        let noun = detected
            .as_ref()
            .map(|d| d.family.pull_noun())
            .unwrap_or("pull request");
        let filters = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(t.border)
            .children(
                [
                    ("mine", "Mine"),
                    ("review_requested", "Review requested"),
                    ("all", "All"),
                ]
                .map(|(f, label)| {
                    selector_option(filter_selector(f), label, self.filter == f, &t)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_filter(f, cx)))
                }),
            )
            .child(div().w(px(8.)))
            .children(
                [
                    ("open", "Open"),
                    ("closed", "Closed"),
                    ("merged", "Merged"),
                    ("all_states", "All"),
                ]
                .map(|(s, label)| {
                    let state: &'static str = if s == "all_states" { "all" } else { s };
                    selector_option(filter_selector(s), label, self.state == state, &t)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_state(state, cx)))
                }),
            )
            .child(
                text_box(
                    SEARCH_BOX,
                    &self.text,
                    "Search",
                    self.search_focus.is_focused(window),
                    &t,
                )
                .w(px(160.))
                .track_focus(&self.search_focus)
                .key_context("ForgeSearch")
                .on_key_down(cx.listener(Self::search_key))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.search_focus.focus(window, cx);
                    cx.notify();
                })),
            )
            .child(
                push_button(REFRESH, "Refresh", false, true, &t)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PullsEvent::Refresh))),
            )
            // Always drawn (disabled without a selection): a selection never moves the list.
            .child(
                push_button(OPEN, "Open", true, self.selected.is_some(), &t).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(ix) = this.selected {
                            this.open(ix, cx);
                        }
                    },
                )),
            );
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
            } else if !d.signed_in {
                notes.push(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .p_1()
                        .text_size(t.typography.small)
                        .text_color(t.text_muted)
                        .child(format!(
                            "Not signed in to {} ({}): public data only.",
                            d.host,
                            d.family.display()
                        ))
                        .child(
                            push_button(
                                SIGN_IN,
                                format!("Sign in to {}", d.family.display()),
                                false,
                                true,
                                &t,
                            )
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PullsEvent::SignIn))),
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
                    .text_color(t.text)
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
        let count = self.items.len();
        let empty = count == 0
            && !self.loading
            && self.error.is_none()
            && detected.as_ref().is_some_and(|d| d.supported());
        div()
            .id("forge-pulls")
            .debug_selector(|| "forge-pulls".into())
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .text_size(t.typography.ui)
            .text_color(t.text)
            .child(filters)
            .children(notes)
            .children(empty.then(|| {
                div()
                    .p_2()
                    .text_color(t.text_muted)
                    .child(format!("No {noun}s match."))
            }))
            .child(
                uniform_list(
                    "forge-pulls-list",
                    count,
                    cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
    }
}
