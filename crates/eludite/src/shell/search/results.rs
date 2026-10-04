//! The Find Results 1 and Find Results 2 tool windows (brief 0042): each search's block with Visual Studio's count
//! line, its matching files and their matching lines (the match highlighted), the files Replace All skipped, and
//! the elapsed time; Append adds a block under a separator. The rows are virtualized, so ten thousand matches cost
//! what the visible rows cost.
//!
//! The window is a model the shell feeds from the search's events, batched per frame: a block starts, files arrive
//! (each inserted in path order), the search finishes. Files of a search the window no longer shows (superseded or
//! cleared) are dropped, never drawn. The toolbar's Stop, Clear, Previous and Next and a double-click on a result
//! dispatch `eludite.search.*` like everything else.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eludite_commands::search::{
    self as cmds, BlockOut, CharRange, Navigate, ResultsOutput, RowOut,
};
use eludite_search::FileMatches;
use eludite_ui::{RunCommand, Theme};
use gpui::{
    App, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement, ParentElement,
    Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, StyledText,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use serde_json::{Value, json};

/// A row's height.
pub const ROW_HEIGHT: f32 = 20.;
/// A displayed line is cut to this many characters around its first match.
const DISPLAY_CHARS: usize = 400;

/// Element ids (and debug selectors).
pub fn window_selector(n: u8) -> String {
    format!("find-results-{n}")
}

pub fn stop_selector(n: u8) -> String {
    format!("find-results-{n}-stop")
}

pub fn clear_selector(n: u8) -> String {
    format!("find-results-{n}-clear")
}

pub fn row_selector(n: u8, row: usize) -> String {
    format!("find-results-{n}-row-{row}")
}

/// A matching line as the window draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct LineResult {
    pub line: u64,
    /// The first match's column, 1-based, in characters of the whole line, and its length in characters.
    pub column: u64,
    pub len: u64,
    /// The line as drawn (cut around the first match when long).
    pub text: SharedString,
    /// The matches' byte ranges in `text`.
    pub highlights: Vec<Range<usize>>,
    /// Every match on the line, in characters of the whole line (for the answer).
    pub ranges: Vec<CharRange>,
    /// The whole line's text (for the answer, which cuts it its own way).
    pub full: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

impl LineResult {
    pub fn from_match(l: &eludite_search::LineMatch) -> Self {
        let chars_before = |b: usize| l.text.get(..b).map_or(0, |s| s.chars().count());
        let ranges: Vec<CharRange> = l
            .ranges
            .iter()
            .map(|r| CharRange {
                start: chars_before(r.start),
                end: chars_before(r.end),
            })
            .collect();
        let first = ranges
            .first()
            .copied()
            .unwrap_or(CharRange { start: 0, end: 0 });
        // Cut long lines around the first match.
        let total = l.text.chars().count();
        let (text, highlights) = if total <= DISPLAY_CHARS {
            (l.text.clone(), l.ranges.clone())
        } else {
            let start = first.start.saturating_sub(DISPLAY_CHARS / 4);
            let end = (start + DISPLAY_CHARS).min(total);
            let byte = |c: usize| {
                l.text
                    .char_indices()
                    .nth(c)
                    .map_or(l.text.len(), |(i, _)| i)
            };
            let (b0, b1) = (byte(start), byte(end));
            let lead = if start > 0 { "\u{2026}" } else { "" };
            let text = format!(
                "{lead}{}{}",
                &l.text[b0..b1],
                if end < total { "\u{2026}" } else { "" }
            );
            let shift = lead.len();
            let highlights = l
                .ranges
                .iter()
                .filter(|r| r.start >= b0 && r.end <= b1)
                .map(|r| r.start - b0 + shift..r.end - b0 + shift)
                .collect();
            (text, highlights)
        };
        Self {
            line: l.line,
            column: first.start as u64 + 1,
            len: (first.end - first.start) as u64,
            text: text.into(),
            highlights,
            ranges,
            full: l.text.clone(),
            before: l.before.clone(),
            after: l.after.clone(),
        }
    }
}

/// A matching file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileResult {
    pub path: PathBuf,
    pub display: String,
    pub open: bool,
    pub lines: Vec<LineResult>,
    pub count: usize,
}

/// What a finished search counted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counts {
    pub files_searched: u64,
    pub matching_files: u64,
    pub matching_lines: u64,
    pub total: u64,
    pub truncated: bool,
    pub canceled: bool,
    pub elapsed: Duration,
}

/// One search's block.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub search_id: u64,
    /// `Find all "x", Subfolders, Find Results 1, Entire Solution, "*.cs"`.
    pub title: String,
    pub files: Vec<FileResult>,
    pub running: bool,
    pub counts: Option<Counts>,
    pub skipped: Vec<(String, String)>,
    pub error: Option<String>,
}

impl Block {
    fn lines(&self) -> u64 {
        self.files.iter().map(|f| f.lines.len() as u64).sum()
    }

    /// Visual Studio's count line.
    pub fn summary(&self) -> String {
        if let Some(e) = &self.error {
            return format!("{} \u{2014} {e}", self.title);
        }
        match (&self.counts, self.running) {
            (Some(c), false) => format!(
                "{} \u{2014} Matching lines: {} Matching files: {} Total files searched: {}",
                self.title, c.matching_lines, c.matching_files, c.files_searched
            ),
            _ => format!(
                "{} \u{2014} Searching\u{2026} {} matching lines so far",
                self.title,
                self.lines()
            ),
        }
    }

    /// The line under the count line once the search is done: the elapsed time, and why it stopped early.
    pub fn footer(&self) -> Option<String> {
        let c = self.counts.as_ref()?;
        let mut s = format!("Elapsed time: {:.2} s", c.elapsed.as_secs_f64());
        if c.canceled {
            s.push_str(". The search was stopped; these are the matches found until then.");
        } else if c.truncated {
            s.push_str(&format!(
                ". Stopped at {} matches: there are more (narrow the search).",
                c.total
            ));
        }
        Some(s)
    }
}

/// A drawn row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Separator,
    Header(usize),
    File(usize, usize),
    /// Block, file, line, and its index among the matches.
    Line(usize, usize, usize, usize),
    Skipped(usize, usize),
    Footer(usize),
}

/// Where a result opens the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub line: u64,
    pub column: u64,
    pub len: u64,
}

/// A Find Results window.
pub struct FindResults {
    theme: Theme,
    pub number: u8,
    pub blocks: Vec<Block>,
    rows: Vec<Row>,
    /// The Line rows in order: (row index, block, file, line).
    matches: Vec<(usize, usize, usize, usize)>,
    /// Index into `matches`.
    selected: Option<usize>,
    root: Option<PathBuf>,
    scroll: UniformListScrollHandle,
    mono: SharedString,
}

/// A path as the window shows it: relative to the workspace's folder with `/` when inside it.
pub fn display_path(path: &Path, root: Option<&Path>) -> String {
    match root.and_then(|r| path.strip_prefix(r).ok()) {
        Some(rel) => rel.to_string_lossy().replace('\\', "/"),
        None => path.to_string_lossy().into_owned(),
    }
}

impl FindResults {
    pub fn new(theme: Theme, number: u8) -> Self {
        Self {
            theme,
            number,
            blocks: Vec::new(),
            rows: Vec::new(),
            matches: Vec::new(),
            selected: None,
            root: None,
            scroll: UniformListScrollHandle::new(),
            mono: eludite_editor::default_font_family(),
        }
    }

    pub fn set_root(&mut self, root: Option<PathBuf>) {
        self.root = root;
    }

    /// The search drawing in this window, while it runs.
    pub fn running(&self) -> Option<u64> {
        self.blocks.iter().find(|b| b.running).map(|b| b.search_id)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// A new search draws here: under the earlier blocks with `append`, else in their place.
    pub fn start(&mut self, search_id: u64, title: String, append: bool, cx: &mut Context<Self>) {
        if !append {
            self.blocks.clear();
            self.selected = None;
        }
        self.blocks.push(Block {
            search_id,
            title,
            files: Vec::new(),
            running: true,
            counts: None,
            skipped: Vec::new(),
            error: None,
        });
        self.rebuild();
        cx.notify();
    }

    fn block_mut(&mut self, search_id: u64) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|b| b.search_id == search_id)
    }

    /// Files of `search_id`, each inserted in path order. False (and nothing drawn) when the window does not show
    /// that search: it was superseded or cleared.
    pub fn add_files(
        &mut self,
        search_id: u64,
        files: Vec<FileMatches>,
        cx: &mut Context<Self>,
    ) -> bool {
        let root = self.root.clone();
        let Some(block) = self.block_mut(search_id).filter(|b| b.running) else {
            return false;
        };
        for f in files {
            let result = FileResult {
                display: display_path(&f.path, root.as_deref()),
                open: f.open,
                count: f.count(),
                lines: f.lines.iter().map(LineResult::from_match).collect(),
                path: f.path,
            };
            let at = block
                .files
                .binary_search_by(|x| x.path.cmp(&result.path))
                .unwrap_or_else(|i| i);
            block.files.insert(at, result);
        }
        self.rebuild();
        cx.notify();
        true
    }

    /// A file Replace All left alone, and why.
    pub fn skip(&mut self, search_id: u64, path: String, reason: String, cx: &mut Context<Self>) {
        if let Some(b) = self.block_mut(search_id) {
            b.skipped.push((path, reason));
            self.rebuild();
            cx.notify();
        }
    }

    pub fn finish(
        &mut self,
        search_id: u64,
        counts: Option<Counts>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(b) = self.block_mut(search_id) {
            b.running = false;
            b.counts = counts;
            b.error = error;
            self.rebuild();
            cx.notify();
        }
    }

    /// Clear All.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.blocks.clear();
        self.selected = None;
        self.rebuild();
        cx.notify();
    }

    fn rebuild(&mut self) {
        self.rows.clear();
        self.matches.clear();
        for (bi, b) in self.blocks.iter().enumerate() {
            if bi > 0 {
                self.rows.push(Row::Separator);
            }
            self.rows.push(Row::Header(bi));
            for (fi, f) in b.files.iter().enumerate() {
                self.rows.push(Row::File(bi, fi));
                for li in 0..f.lines.len() {
                    let m = self.matches.len();
                    self.matches.push((self.rows.len(), bi, fi, li));
                    self.rows.push(Row::Line(bi, fi, li, m));
                }
            }
            for si in 0..b.skipped.len() {
                self.rows.push(Row::Skipped(bi, si));
            }
            if b.counts.is_some() || b.error.is_some() {
                self.rows.push(Row::Footer(bi));
            }
        }
        if self.selected.is_some_and(|s| s >= self.matches.len()) {
            self.selected = None;
        }
    }

    fn target(&self, m: usize) -> Option<Target> {
        let &(_, b, f, l) = self.matches.get(m)?;
        let file = &self.blocks[b].files[f];
        let line = &file.lines[l];
        Some(Target {
            path: file.path.clone(),
            line: line.line,
            column: line.column,
            len: line.len,
        })
    }

    /// Select match `m` (and scroll to it); its target.
    pub fn select(&mut self, m: usize, cx: &mut Context<Self>) -> Option<Target> {
        let target = self.target(m)?;
        self.selected = Some(m);
        let row = self.matches[m].0;
        self.scroll.scroll_to_item(row, ScrollStrategy::Center);
        cx.notify();
        Some(target)
    }

    /// F8, Shift+F8 and Skip File: the next, previous or next file's first match, wrapping around.
    pub fn step(&mut self, nav: Navigate, cx: &mut Context<Self>) -> Option<Target> {
        let n = self.matches.len();
        if n == 0 {
            return None;
        }
        let next = match (nav, self.selected) {
            (Navigate::Next, None) | (Navigate::NextFile, None) => 0,
            (Navigate::Previous, None) => n - 1,
            (Navigate::Next, Some(s)) => (s + 1) % n,
            (Navigate::Previous, Some(s)) => (s + n - 1) % n,
            (Navigate::NextFile, Some(s)) => {
                let (_, b, f, _) = self.matches[s];
                (s + 1..n)
                    .find(|&i| (self.matches[i].1, self.matches[i].2) != (b, f))
                    .unwrap_or(0)
            }
        };
        self.select(next, cx)
    }

    /// A page of the matches, as `eludite.search.results` answers.
    pub fn page(&self, offset: usize, limit: usize) -> ResultsOutput {
        let mut blocks = Vec::new();
        let mut first = 0u64;
        for b in &self.blocks {
            let total: u64 = b.files.iter().map(|f| f.lines.len() as u64).sum();
            blocks.push(BlockOut {
                search_id: b.search_id,
                summary: b.summary(),
                running: b.running,
                canceled: b.counts.as_ref().is_some_and(|c| c.canceled),
                truncated: b.counts.as_ref().is_some_and(|c| c.truncated),
                first: Some(first),
                total: Some(total),
            });
            first += total;
        }
        let matches = self
            .matches
            .iter()
            .enumerate()
            .skip(offset)
            .take(limit)
            .map(|(i, &(_, b, f, l))| {
                let file = &self.blocks[b].files[f];
                let line = &file.lines[l];
                row_out(file, line, b as u64, i as u64)
            })
            .collect();
        let total = self.matches.len() as u64;
        let end = offset.saturating_add(limit) as u64;
        ResultsOutput {
            results_window: Some(self.number),
            blocks,
            matches,
            total,
            offset: offset as u64,
            next_offset: (end < total).then_some(end),
            selected: self.selected.map(|s| s as u64),
        }
    }

    fn run(command: &'static str, args: Value, window: &mut Window, cx: &mut App) {
        window.dispatch_action(Box::new(RunCommand::new(command, args)), cx);
    }
}

/// A row of `search-results.output.json`.
pub fn row_out(file: &FileResult, line: &LineResult, block: u64, index: u64) -> RowOut {
    let byte_ranges: Vec<Range<usize>> = line
        .ranges
        .iter()
        .map(|r| {
            let b = |c: usize| {
                line.full
                    .char_indices()
                    .nth(c)
                    .map_or(line.full.len(), |(i, _)| i)
            };
            b(r.start)..b(r.end)
        })
        .collect();
    let (text, ranges, column) = cmds::clip_line(&line.full, &byte_ranges);
    RowOut {
        path: file.display.clone(),
        line: line.line,
        column,
        text,
        ranges,
        before: line.before.clone(),
        after: line.after.clone(),
        block,
        index,
    }
}

fn toolbar_button(
    id: String,
    label: &'static str,
    enabled: bool,
    t: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let sel = id.clone();
    let el = div()
        .id(SharedString::from(id))
        .debug_selector(move || sel.clone())
        .flex()
        .flex_none()
        .items_center()
        .h(px(20.))
        .px_2()
        .text_size(t.typography.ui);
    if enabled {
        el.text_color(t.text)
            .cursor_pointer()
            .hover(|s| s.bg(t.menu_hover))
            .child(label)
    } else {
        el.text_color(t.text_disabled).child(label)
    }
}

impl Render for FindResults {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let n = self.number;
        let running = self.running();
        let mut stop = toolbar_button(
            stop_selector(n),
            "\u{25A0} Stop Search",
            running.is_some(),
            &t,
        );
        if let Some(id) = running {
            stop = stop.on_click(cx.listener(move |_, _, window, cx| {
                Self::run(cmds::CANCEL, json!({ "search_id": id }), window, cx)
            }));
        }
        let has_rows = !self.blocks.is_empty();
        let mut clear = toolbar_button(clear_selector(n), "Clear All", has_rows, &t);
        if has_rows {
            clear = clear.on_click(cx.listener(move |_, _, window, cx| {
                Self::run(
                    cmds::RESULTS,
                    json!({ "results_window": n, "clear": true }),
                    window,
                    cx,
                )
            }));
        }
        let has_matches = !self.matches.is_empty();
        let nav = |dir: &'static str, label: &'static str, cx: &mut Context<Self>| {
            let el = toolbar_button(format!("find-results-{n}-{dir}"), label, has_matches, &t);
            if has_matches {
                el.on_click(cx.listener(move |_, _, window, cx| {
                    Self::run(
                        cmds::RESULTS,
                        json!({ "results_window": n, "navigate": dir }),
                        window,
                        cx,
                    )
                }))
            } else {
                el
            }
        };
        let toolbar = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(24.))
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .child(nav("previous", "\u{2191} Previous (Shift+F8)", cx))
            .child(nav("next", "\u{2193} Next (F8)", cx))
            .child(clear)
            .child(stop);
        let body = if self.rows.is_empty() {
            div()
                .p_2()
                .text_color(t.text_muted)
                .text_size(t.typography.ui)
                .child("Use Edit > Find and Replace > Find in Files (Ctrl+Shift+F) to search the workspace's files.")
                .into_any_element()
        } else {
            uniform_list(
                SharedString::from(format!("find-results-{n}-rows")),
                self.rows.len(),
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range.filter_map(|ix| this.render_row(ix, cx)).collect()
                }),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .into_any_element()
        };
        let sel = window_selector(n);
        div()
            .id(SharedString::from(sel.clone()))
            .debug_selector(move || sel)
            .flex()
            .flex_col()
            .size_full()
            .bg(t.panel)
            .text_color(t.text)
            .child(toolbar)
            .child(body)
    }
}

impl FindResults {
    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = self.theme;
        let n = self.number;
        let row = *self.rows.get(ix)?;
        let sel = row_selector(n, ix);
        let base = div()
            .id(SharedString::from(sel.clone()))
            .debug_selector(move || sel)
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT))
            .w_full()
            .px_2()
            .whitespace_nowrap()
            .overflow_hidden()
            .text_size(t.typography.ui);
        Some(match row {
            Row::Separator => base
                .child(div().h(px(1.)).w_full().bg(t.border))
                .into_any_element(),
            Row::Header(b) => base
                .font_weight(FontWeight::SEMIBOLD)
                .child(self.blocks[b].summary())
                .into_any_element(),
            Row::File(b, f) => {
                let file = &self.blocks[b].files[f];
                base.pl(px(16.))
                    .child(format!(
                        "{}{} ({})",
                        file.display,
                        if file.open { " [open]" } else { "" },
                        file.count
                    ))
                    .into_any_element()
            }
            Row::Line(b, f, l, m) => {
                let line = &self.blocks[b].files[f].lines[l];
                let selected = self.selected == Some(m);
                let mut bg = t.accent;
                bg.a = 0.35;
                let style = HighlightStyle {
                    background_color: Some(bg.into()),
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                };
                let highlights: Vec<(Range<usize>, HighlightStyle)> = line
                    .highlights
                    .iter()
                    .filter(|r| r.end <= line.text.len())
                    .map(|r| (r.clone(), style))
                    .collect();
                let mut el = base
                    .pl(px(32.))
                    .gap_2()
                    .cursor_pointer()
                    .child(
                        div()
                            .flex_none()
                            .text_color(t.text_muted)
                            .child(format!("({}, {}):", line.line, line.column)),
                    )
                    .child(
                        div()
                            .font_family(self.mono.clone())
                            .child(StyledText::new(line.text.clone()).with_highlights(highlights)),
                    );
                if selected {
                    el = el.bg(t.menu_hover);
                } else {
                    el = el.hover(|s| s.bg(t.menu_hover));
                }
                el = el.on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    if e.click_count() >= 2 {
                        Self::run(
                            cmds::RESULTS,
                            json!({ "results_window": this.number, "select": m }),
                            window,
                            cx,
                        );
                    } else {
                        this.selected = Some(m);
                        cx.notify();
                    }
                }));
                el.into_any_element()
            }
            Row::Skipped(b, s) => {
                let (path, reason) = &self.blocks[b].skipped[s];
                base.pl(px(16.))
                    .text_color(gpui::rgb(0xCCA700))
                    .child(format!("{path}: skipped, {reason}"))
                    .into_any_element()
            }
            Row::Footer(b) => base
                .text_color(t.text_muted)
                .child(self.blocks[b].footer().unwrap_or_default())
                .into_any_element(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn line(text: &str, ranges: Vec<Range<usize>>) -> eludite_search::LineMatch {
        eludite_search::LineMatch {
            line: 3,
            offset: 0,
            text: text.into(),
            ranges,
            before: vec![],
            after: vec![],
        }
    }

    #[test]
    fn lines_count_columns_in_characters_and_cut_long_lines() {
        let l = LineResult::from_match(&line("é = Order;", vec![5..10]));
        assert_eq!((l.line, l.column, l.len), (3, 5, 5));
        assert_eq!(&l.text[l.highlights[0].clone()], "Order");
        let long = format!("{}Order{}", "x".repeat(2000), "y".repeat(2000));
        let l = LineResult::from_match(&line(&long, vec![2000..2005]));
        assert_eq!(l.column, 2001);
        assert!(l.text.chars().count() <= DISPLAY_CHARS + 2);
        assert_eq!(&l.text[l.highlights[0].clone()], "Order");
        assert_eq!(
            display_path(Path::new("/w/src/A.cs"), Some(Path::new("/w"))),
            "src/A.cs"
        );
        assert_eq!(
            display_path(Path::new("/x/A.cs"), Some(Path::new("/w"))),
            "/x/A.cs"
        );
    }
}
