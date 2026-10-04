//! The Checks log document: a check's log (a job's or a run's), read-only, at most 2 MB of it; the rest is on the
//! forge, linked. Lines are virtualized.

use std::ops::Range;

use eludite_ui::Theme;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    UniformListScrollHandle, Window, div, px, rgb, uniform_list,
};
use serde_json::Value;

pub const ROW_HEIGHT: f32 = 18.;

pub struct CheckLog {
    theme: Theme,
    /// The check's id (`eludite.forge.check_log`'s input).
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    pub name: String,
    pub lines: Vec<SharedString>,
    pub truncated: bool,
    pub url: Option<String>,
    pub error: Option<String>,
    pub loading: bool,
    scroll: UniformListScrollHandle,
}

impl CheckLog {
    pub fn new(theme: Theme, id: String, name: String, _cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            id,
            name,
            lines: Vec::new(),
            truncated: false,
            url: None,
            error: None,
            loading: true,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn set_text(&mut self, v: &Value, cx: &mut Context<Self>) {
        self.lines = v["text"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .map(|l| SharedString::from(l.to_owned()))
            .collect();
        self.truncated = v["truncated"] == true;
        self.url = v["url"].as_str().map(str::to_owned);
        self.loading = false;
        cx.notify();
    }

    pub fn set_error(&mut self, e: String, cx: &mut Context<Self>) {
        self.error = Some(e);
        self.loading = false;
        cx.notify();
    }
}

impl Render for CheckLog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mono = eludite_editor::default_font_family();
        let header = format!(
            "{}{}",
            self.name,
            if self.loading {
                " (loading\u{2026})"
            } else {
                ""
            }
        );
        let tail = self.truncated.then(|| {
            format!(
                "The log continues past 2 MB: {}",
                self.url
                    .clone()
                    .unwrap_or_else(|| "open it on the forge".into())
            )
        });
        div()
            .id("forge-log")
            .debug_selector(|| "forge-log".into())
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .child(
                div()
                    .p_1()
                    .border_b_1()
                    .border_color(t.border)
                    .child(header),
            )
            .children(
                self.error
                    .clone()
                    .map(|e| div().p_2().text_color(rgb(0xF1_4C_4C)).child(e)),
            )
            .child(
                uniform_list(
                    "forge-log-lines",
                    self.lines.len(),
                    cx.processor(move |this, range: Range<usize>, _, _| {
                        range
                            .map(|ix| {
                                div()
                                    .h(px(ROW_HEIGHT))
                                    .px_2()
                                    .font_family(mono.clone())
                                    .text_size(px(12.))
                                    .whitespace_nowrap()
                                    .child(this.lines[ix].clone())
                                    .into_any_element()
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .children(tail.map(|m| {
                div()
                    .p_1()
                    .border_t_1()
                    .border_color(t.border)
                    .text_color(t.text_muted)
                    .child(m)
            }))
    }
}
