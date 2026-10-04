//! The Output window (PLAN.md 4.4, brief 0017): Visual Studio's "Show output from" dropdown with five sources, Build
//! (the build log as `eludite-host` or cargo streams it), Debug (the program's output and the debugger's messages,
//! brief 0020), Host (`eludite-host`'s own log), Language Servers (the servers the shell runs itself, brief 0019)
//! Browser (the browser of `eludite.browser.*`: launch, tabs, navigation failures, console errors, brief 0023) and
//! Tests (the test runners' output: eludite-host's MTP and VSTest logs, cargo test's output, brief 0035),
//! Clear All, and a virtualized, append-only text view.
//!
//! - **Storage.** One `String` per source plus the byte offset of each line start, so 100k lines cost their bytes and
//!   one `usize` each. ANSI escape sequences and carriage returns are removed as text arrives.
//! - **Rendering.** A `uniform_list`: a frame draws the visible lines only, whatever the line count.
//! - **Auto-scroll.** The view follows new output while it is scrolled to the end. Scrolling up pauses it (the
//!   window says so); scrolling back to the end resumes it.
//! - **Commands.** The dropdown runs `eludite.output.show`, Clear All runs `eludite.output.clear`.

use std::ops::Range;

use eludite_commands::build::{self as build_commands, OutputSource};
use eludite_ui::{RunCommand, Theme, toggle_button};
use gpui::{
    App, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, UniformListScrollHandle,
    Window, anchored, deferred, div, px, uniform_list,
};
use serde_json::json;

/// Height of one line.
const LINE_HEIGHT: f32 = 17.;

/// Debug selectors.
pub const SOURCE_BUTTON: &str = "output-source";
pub const CLEAR_BUTTON: &str = "output-clear";
pub const LINES: &str = "output-lines";
pub const BODY: &str = "output-body";
pub const PAUSED_LABEL: &str = "output-paused";

/// Debug selector of entry `ix` of the source dropdown (0 Build, 1 Debug, 2 Host, 3 Language Servers, 4 Browser,
/// 5 Tests).
pub fn source_item_selector(ix: usize) -> String {
    format!("output-source-{ix}")
}

/// The text of one source.
#[derive(Debug, Default, Clone)]
pub struct OutputPane {
    text: String,
    /// Byte offset of each complete line's start.
    starts: Vec<usize>,
    /// The text after the last line break (a line still being written).
    partial: String,
}

impl OutputPane {
    /// Append `text`; complete lines (ending in `\n`) become rows at once, a trailing partial line when it ends.
    pub fn append(&mut self, text: &str) {
        let cleaned = strip_ansi(text);
        let mut rest = cleaned.as_str();
        while let Some(i) = rest.find('\n') {
            let (line, after) = rest.split_at(i);
            self.starts.push(self.text.len());
            self.text.push_str(&self.partial);
            self.partial.clear();
            self.text.push_str(line);
            self.text.push('\n');
            rest = &after[1..];
        }
        self.partial.push_str(rest);
    }

    /// Append one line (without its line break).
    #[cfg(test)]
    pub fn push_line(&mut self, line: &str) {
        self.append(line);
        self.append("\n");
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Line `ix` without its line break.
    pub fn line(&self, ix: usize) -> Option<&str> {
        let start = *self.starts.get(ix)?;
        let end = self.starts.get(ix + 1).copied().unwrap_or(self.text.len());
        Some(self.text[start..end].trim_end_matches('\n'))
    }

    /// The last `n` lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<String> {
        let from = self.len().saturating_sub(n);
        (from..self.len())
            .filter_map(|i| self.line(i).map(str::to_owned))
            .collect()
    }

    /// Clear; returns how many lines there were.
    pub fn clear(&mut self) -> usize {
        let n = self.len();
        self.text.clear();
        self.starts.clear();
        self.partial.clear();
        n
    }
}

/// `text` without ANSI escape sequences (CSI `ESC [ ... final`, OSC `ESC ] ... BEL|ESC \`, two-byte escapes) and
/// without carriage returns.
pub fn strip_ansi(text: &str) -> String {
    if !text.contains(['\u{1b}', '\r']) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {}
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    // Parameters and intermediates, then one final byte in @..~.
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            },
            c => out.push(c),
        }
    }
    out
}

pub struct OutputWindow {
    theme: Theme,
    mono: SharedString,
    build: OutputPane,
    host: OutputPane,
    /// Brief 0019: the language servers the shell runs itself.
    servers: OutputPane,
    /// Brief 0020: the program's output and the debugger's messages.
    debug: OutputPane,
    /// Brief 0023: the browser of `eludite.browser.*`.
    browser: OutputPane,
    /// Brief 0035: the test runners.
    tests: OutputPane,
    package_manager: OutputPane,
    selected: OutputSource,
    scroll: UniformListScrollHandle,
    /// Auto-scroll with new output (false after the user scrolled up, until they are back at the end).
    following: bool,
    menu: bool,
}

impl OutputWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            mono: eludite_editor::default_font_family(),
            build: OutputPane::default(),
            host: OutputPane::default(),
            servers: OutputPane::default(),
            debug: OutputPane::default(),
            browser: OutputPane::default(),
            tests: OutputPane::default(),
            package_manager: OutputPane::default(),
            selected: OutputSource::Build,
            scroll: UniformListScrollHandle::new(),
            following: true,
            menu: false,
        }
    }

    pub fn pane(&self, source: OutputSource) -> &OutputPane {
        match source {
            OutputSource::Build => &self.build,
            OutputSource::Host => &self.host,
            OutputSource::LanguageServers => &self.servers,
            OutputSource::Debug => &self.debug,
            OutputSource::Browser => &self.browser,
            OutputSource::Tests => &self.tests,
            OutputSource::PackageManager => &self.package_manager,
        }
    }

    fn pane_mut(&mut self, source: OutputSource) -> &mut OutputPane {
        match source {
            OutputSource::Build => &mut self.build,
            OutputSource::Host => &mut self.host,
            OutputSource::LanguageServers => &mut self.servers,
            OutputSource::Debug => &mut self.debug,
            OutputSource::Browser => &mut self.browser,
            OutputSource::Tests => &mut self.tests,
            OutputSource::PackageManager => &mut self.package_manager,
        }
    }

    pub fn selected(&self) -> OutputSource {
        self.selected
    }

    pub fn following(&self) -> bool {
        self.following
    }

    /// Append streamed text to `source`; the view follows it when it is the selected source and following.
    pub fn append(&mut self, source: OutputSource, text: &str, cx: &mut Context<Self>) {
        self.pane_mut(source).append(text);
        if source == self.selected {
            if self.following {
                self.scroll.scroll_to_bottom();
            }
            cx.notify();
        }
    }

    /// Select `source` (the dropdown); the view goes to its end and follows it.
    pub fn select(&mut self, source: OutputSource, cx: &mut Context<Self>) {
        if self.selected != source {
            self.selected = source;
            self.following = true;
            self.scroll.scroll_to_bottom();
        }
        self.menu = false;
        cx.notify();
    }

    /// Clear `source`; returns the lines removed.
    pub fn clear(&mut self, source: OutputSource, cx: &mut Context<Self>) -> usize {
        let n = self.pane_mut(source).clear();
        if source == self.selected {
            self.following = true;
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
        n
    }

    /// The index of the topmost line drawn, from the scroll offset of the last layout (tests).
    #[cfg(test)]
    pub fn top_line(&self) -> usize {
        let offset = self.scroll.0.borrow().base_handle.offset();
        (-offset.y / px(LINE_HEIGHT)).floor().max(0.) as usize
    }

    fn on_wheel(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        // A positive delta scrolls toward the top: the user wants to read; stop following. Any wheel re-renders, so
        // reaching the end again resumes following (see `render`).
        if e.delta.pixel_delta(px(LINE_HEIGHT)).y > px(0.) {
            self.following = false;
        }
        cx.notify();
    }

    fn run(&self, command: &str, args: serde_json::Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
    }

    fn rows(&self, range: Range<usize>) -> Vec<gpui::AnyElement> {
        let pane = self.pane(self.selected);
        range
            .filter_map(|ix| {
                let line = pane.line(ix)?;
                Some(
                    div()
                        .h(px(LINE_HEIGHT))
                        .px_1()
                        .whitespace_nowrap()
                        .child(SharedString::from(line.to_owned()))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for OutputWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Back at the end after scrolling up: follow again.
        if !self.following && self.scroll.is_scrolled_to_end().unwrap_or(true) {
            self.following = true;
        }
        let t = self.theme;
        let label = |s: OutputSource| match s {
            OutputSource::Build => "Build",
            OutputSource::Host => "Host",
            OutputSource::LanguageServers => "Language Servers",
            OutputSource::Debug => "Debug",
            OutputSource::Browser => "Browser",
            OutputSource::Tests => "Tests",
            OutputSource::PackageManager => "Package Manager",
        };
        let source_button = toggle_button(
            SOURCE_BUTTON,
            format!("{} \u{25BE}", label(self.selected)),
            false,
            &t,
        )
        .min_w(px(140.))
        .on_click(cx.listener(|this, _, _, cx| {
            this.menu = !this.menu;
            cx.notify();
        }));
        let menu = self.menu.then(|| {
            let items = [
                OutputSource::Build,
                OutputSource::Debug,
                OutputSource::Host,
                OutputSource::LanguageServers,
                OutputSource::Browser,
                OutputSource::Tests,
                OutputSource::PackageManager,
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, s)| {
                let sel = source_item_selector(ix);
                div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .px_2()
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|st| st.bg(t.menu_hover))
                    .child(label(s))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.menu = false;
                        this.run(
                            build_commands::OUTPUT_SHOW,
                            json!({"source": s.as_str(), "tail": 0}),
                            window,
                            cx,
                        );
                    }))
            });
            deferred(
                anchored().child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("output-source-menu")
                        .occlude()
                        .min_w(px(140.))
                        .py_1()
                        .mt(px(22.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = false;
                            cx.notify();
                        }))
                        .children(items),
                ),
            )
            .with_priority(1)
        });
        let selected = self.selected;
        let clear = toggle_button(CLEAR_BUTTON, "Clear All", false, &t).on_click(cx.listener(
            move |this, _, window, cx| {
                this.run(
                    build_commands::OUTPUT_CLEAR,
                    json!({"source": selected.as_str()}),
                    window,
                    cx,
                )
            },
        ));
        let paused = (!self.following).then(|| {
            div()
                .id(PAUSED_LABEL)
                .debug_selector(|| PAUSED_LABEL.into())
                .text_color(t.text_muted)
                .child("Auto-scroll paused")
        });
        let toolbar = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(26.))
            .px_2()
            .text_size(t.typography.ui)
            .child(
                div()
                    .text_color(t.text_muted)
                    .font_weight(FontWeight::NORMAL)
                    .child("Show output from:"),
            )
            .child(div().relative().child(source_button).children(menu))
            .child(clear)
            .child(div().flex_1())
            .children(paused);
        let count = self.pane(self.selected).len();
        div()
            .id("output")
            .debug_selector(|| "output".into())
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(
                div()
                    .id("output-body")
                    .debug_selector(|| BODY.into())
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(t.border)
                    .bg(t.background)
                    .font_family(self.mono.clone())
                    .text_size(t.typography.small)
                    .text_color(t.text)
                    .on_scroll_wheel(cx.listener(Self::on_wheel))
                    .child(
                        uniform_list(
                            LINES,
                            count,
                            cx.processor(|this, range, _, _| this.rows(range)),
                        )
                        .track_scroll(&self.scroll)
                        .size_full(),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panes_store_lines_strip_ansi_and_keep_partial_lines() {
        let mut p = OutputPane::default();
        p.append("Build started\n\u{1b}[31merror\u{1b}[0m CS0103\r\npart");
        assert_eq!(p.len(), 2);
        assert_eq!(p.line(0), Some("Build started"));
        assert_eq!(p.line(1), Some("error CS0103"));
        p.append("ial\n");
        assert_eq!(p.line(2), Some("partial"));
        assert_eq!(p.tail(2), ["error CS0103", "partial"]);
        p.push_line("\u{1b}]0;title\u{7}done");
        assert_eq!(p.line(3), Some("done"));
        assert_eq!(p.clear(), 4);
        assert!(p.is_empty());
        assert_eq!(p.line(0), None);
    }

    #[test]
    fn a_hundred_thousand_lines_cost_their_bytes() {
        let mut p = OutputPane::default();
        let chunk: String = (0..1000)
            .map(|i| format!("  line {i} of a build\n"))
            .collect();
        let t = std::time::Instant::now();
        for _ in 0..100 {
            p.append(&chunk);
        }
        assert_eq!(p.len(), 100_000);
        assert_eq!(p.line(99_999), Some("  line 999 of a build"));
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
    }
}
