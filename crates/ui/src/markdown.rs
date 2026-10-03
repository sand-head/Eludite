//! Markdown for IntelliSense tooltips (Quick Info, completion documentation, Parameter Info) and the Agents window's
//! transcript, parsed by `pulldown-cmark` (CommonMark with GitHub's tables, strikethrough and task lists):
//! paragraphs, headings, fenced and indented code, bullet and numbered lists, block quotes, rules, tables, code spans,
//! bold, italic, strikethrough, links and entities. HTML is shown as its text, except `<br>`. Images show their alt
//! text.
//!
//! [`parse`] turns Markdown into [`Block`]s; [`plain_text`] flattens them (what an agent reads); [`render`] draws them
//! with the theme's tokens, links in the accent color; [`render_linked`] also makes the links clickable.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, Font, FontStyle, FontWeight, InteractiveElement as _,
    InteractiveText, IntoElement, ParentElement, Pixels, Rgba, SharedString, Stateful,
    StrikethroughStyle, Styled, StyledText, TextRun, UnderlineStyle, Window, div, px,
};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::Theme;

/// How a run of inline text is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InlineStyle {
    /// An inline code span: monospace.
    pub code: bool,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    /// A link's text (drawn in the accent color); its target is [`Inline::url`].
    pub link: bool,
}

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: InlineStyle,
    /// A link's target, as written (`https://…`, `src/a.rs#L3`).
    pub url: Option<String>,
}

/// A block of a Markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A paragraph: inline runs; `\n` in a run is a hard line break.
    Paragraph(Vec<Inline>),
    /// A heading, `level` 1 to 6.
    Heading { level: u8, inlines: Vec<Inline> },
    /// A fenced (or indented) code block, without the fences.
    Code(String),
    /// A list: `start` is the first number of a numbered list, `None` for bullets. Each item is a list of blocks.
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    /// A block quote.
    Quote(Vec<Block>),
    /// A thematic break (`---`).
    Rule,
    /// A table: the header row's cells, then each body row's cells.
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
}

/// The parser's options: everything [`parse`] understands.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// Parse `markdown` into blocks. Never fails: what is not understood stays as text.
pub fn parse(markdown: &str) -> Vec<Block> {
    let mut b = Builder {
        open: vec![Open::Blocks(Vec::new())],
        ..Builder::default()
    };
    for event in Parser::new_ext(markdown, options()) {
        b.event(event);
    }
    b.flush_inline();
    match b.open.into_iter().next() {
        Some(Open::Blocks(blocks)) => blocks,
        _ => Vec::new(),
    }
}

/// The byte offsets where `markdown`'s top-level blocks start, in order. The Agents transcript splits streamed text with it,
/// up to the last line break: every block but the last is complete once the next one has started on a complete line (a
/// partial line can still change what the line before it is, as a lone `|` under a table row ends the table).
pub fn block_starts(markdown: &str) -> Vec<usize> {
    let mut depth = 0usize;
    let mut starts = Vec::new();
    for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    starts.push(range.start);
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Rule if depth == 0 => starts.push(range.start),
            _ => {}
        }
    }
    starts
}

/// A container being built.
#[derive(Debug)]
enum Open {
    /// The document, a block quote or a list item.
    Blocks(Vec<Block>),
    Quote(Vec<Block>),
    List(Option<u64>, Vec<Vec<Block>>),
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        in_head: bool,
    },
}

/// What the inline runs being collected belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineOwner {
    Paragraph,
    /// Text directly in a tight list item: a paragraph without the event.
    Implicit,
    Heading(u8),
    Cell,
}

#[derive(Debug, Default)]
struct Builder {
    open: Vec<Open>,
    inlines: Option<(InlineOwner, Vec<Inline>)>,
    code: Option<String>,
    bold: u32,
    italic: u32,
    strikethrough: u32,
    /// The targets of the links being read, innermost last; images push `None`.
    links: Vec<Option<String>>,
}

impl Builder {
    fn style(&self) -> InlineStyle {
        InlineStyle {
            code: false,
            bold: self.bold > 0,
            italic: self.italic > 0,
            strikethrough: self.strikethrough > 0,
            link: self.links.iter().any(Option::is_some),
        }
    }

    fn url(&self) -> Option<String> {
        self.links.iter().rev().flatten().next().cloned()
    }

    /// Add `block` to the innermost container that holds blocks.
    fn push_block(&mut self, block: Block) {
        match self.open.last_mut() {
            Some(Open::Blocks(blocks) | Open::Quote(blocks)) => blocks.push(block),
            Some(Open::List(_, items)) => match items.last_mut() {
                Some(item) => item.push(block),
                None => items.push(vec![block]),
            },
            Some(Open::Table { .. }) | None => {}
        }
    }

    fn start_inline(&mut self, owner: InlineOwner) {
        self.flush_inline();
        self.inlines = Some((owner, Vec::new()));
    }

    /// End the inline runs being collected: a paragraph or heading becomes a block (unless blank), a cell joins its
    /// table.
    fn flush_inline(&mut self) {
        let Some((owner, inlines)) = self.inlines.take() else {
            return;
        };
        let blank = inlines.iter().all(|i| i.text.trim().is_empty());
        match owner {
            InlineOwner::Paragraph | InlineOwner::Implicit if !blank => {
                self.push_block(Block::Paragraph(trim_runs(inlines)))
            }
            InlineOwner::Heading(level) => self.push_block(Block::Heading {
                level,
                inlines: trim_runs(inlines),
            }),
            InlineOwner::Cell => {
                if let Some(Open::Table {
                    header,
                    rows,
                    in_head,
                }) = self.open.last_mut()
                {
                    let cell = trim_runs(inlines);
                    match rows.last_mut() {
                        Some(row) if !*in_head => row.push(cell),
                        _ => header.push(cell),
                    }
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str, style: InlineStyle) {
        if let Some(code) = &mut self.code {
            code.push_str(text);
            return;
        }
        if self.inlines.is_none() {
            self.inlines = Some((InlineOwner::Implicit, Vec::new()));
        }
        let url = if style.link { self.url() } else { None };
        let Some((_, runs)) = &mut self.inlines else {
            return;
        };
        match runs.last_mut() {
            Some(last) if last.style == style && last.url == url => last.text.push_str(text),
            _ if text.is_empty() => {}
            _ => runs.push(Inline {
                text: text.to_owned(),
                style,
                url,
            }),
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t, self.style()),
            Event::Code(t) => self.text(
                &t,
                InlineStyle {
                    code: true,
                    ..self.style()
                },
            ),
            Event::Html(t) | Event::InlineHtml(t) => {
                let t = t.trim_end_matches('\n');
                let br = ["<br>", "<br/>", "<br />"]
                    .iter()
                    .any(|b| t.eq_ignore_ascii_case(b));
                self.text(if br { "\n" } else { t }, self.style());
            }
            Event::SoftBreak => self.text(" ", self.style()),
            Event::HardBreak => self.text("\n", self.style()),
            Event::Rule => {
                self.flush_inline();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => {
                self.text(if done { "\u{2611} " } else { "\u{2610} " }, self.style())
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.start_inline(InlineOwner::Paragraph),
            Tag::Heading { level, .. } => self.start_inline(InlineOwner::Heading(match level {
                HeadingLevel::H1 => 1,
                HeadingLevel::H2 => 2,
                HeadingLevel::H3 => 3,
                HeadingLevel::H4 => 4,
                HeadingLevel::H5 => 5,
                HeadingLevel::H6 => 6,
            })),
            Tag::CodeBlock(_) => {
                self.flush_inline();
                self.code = Some(String::new());
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.open.push(Open::Quote(Vec::new()));
            }
            Tag::List(start) => {
                self.flush_inline();
                self.open.push(Open::List(start, Vec::new()));
            }
            Tag::Item => {
                self.flush_inline();
                if let Some(Open::List(_, items)) = self.open.last_mut() {
                    items.push(Vec::new());
                }
            }
            Tag::Table(_) => {
                self.flush_inline();
                self.open.push(Open::Table {
                    header: Vec::new(),
                    rows: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(Open::Table { in_head, .. }) = self.open.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(Open::Table { rows, in_head, .. }) = self.open.last_mut()
                    && !*in_head
                {
                    rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.start_inline(InlineOwner::Cell),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strikethrough += 1,
            Tag::Link { dest_url, .. } => self.links.push(Some(dest_url.into_string())),
            Tag::Image { .. } => self.links.push(None),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph
            | TagEnd::HtmlBlock
            | TagEnd::Heading(_)
            | TagEnd::TableCell
            | TagEnd::Item => self.flush_inline(),
            TagEnd::CodeBlock => {
                if let Some(code) = self.code.take() {
                    self.push_block(Block::Code(code.trim_end_matches('\n').to_owned()));
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                if let Some(Open::Quote(blocks)) = self.open.pop() {
                    self.push_block(Block::Quote(blocks));
                }
            }
            TagEnd::List(_) => {
                self.flush_inline();
                if let Some(Open::List(start, items)) = self.open.pop() {
                    self.push_block(Block::List { start, items });
                }
            }
            TagEnd::TableHead => {
                if let Some(Open::Table { in_head, .. }) = self.open.last_mut() {
                    *in_head = false;
                }
            }
            TagEnd::Table => {
                self.flush_inline();
                if let Some(Open::Table { header, rows, .. }) = self.open.pop() {
                    self.push_block(Block::Table { header, rows });
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strikethrough = self.strikethrough.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                self.links.pop();
            }
            _ => {}
        }
    }
}

/// Drops the leading whitespace of the first run and the trailing whitespace of the last, and runs left empty.
fn trim_runs(mut runs: Vec<Inline>) -> Vec<Inline> {
    if let Some(first) = runs.first_mut() {
        first.text = first.text.trim_start().to_owned();
    }
    if let Some(last) = runs.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
    runs.retain(|r| !r.text.is_empty());
    runs
}

fn runs_text(runs: &[Inline]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// `text` with `first` before its first line and `rest` before the others.
fn indent(text: &str, first: &str, rest: &str) -> String {
    text.split('\n')
        .enumerate()
        .map(|(i, l)| format!("{}{l}", if i == 0 { first } else { rest }))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The blocks as plain text: blocks separated by blank lines, list items and the blocks in them by line breaks (`•` or `1.` before each),
/// quotes with `> `, table cells with ` | `. Rules are left out.
pub fn plain_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(runs) | Block::Heading { inlines: runs, .. } => Some(runs_text(runs)),
            Block::Code(code) => Some(code.clone()),
            Block::List { start, items } => Some(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        let marker = marker(*start, i);
                        let pad = " ".repeat(marker.chars().count());
                        let item = item.iter().map(|b| plain_text(std::slice::from_ref(b)));
                        let item = item.filter(|t| !t.is_empty()).collect::<Vec<_>>();
                        indent(&item.join("\n"), &marker, &pad)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            Block::Quote(inner) => Some(indent(&plain_text(inner), "> ", "> ")),
            Block::Rule => None,
            Block::Table { header, rows } => Some(
                std::iter::once(header)
                    .chain(rows)
                    .map(|row| {
                        row.iter()
                            .map(|c| runs_text(c))
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Item `i`'s marker with its trailing space: `• ` or `3. `.
fn marker(start: Option<u64>, i: usize) -> String {
    match start {
        Some(n) => format!("{}. ", n + i as u64),
        None => "\u{2022} ".to_owned(),
    }
}

/// What a click on a link does, given its target.
pub type OnLink = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// How [`render`] draws text.
#[derive(Clone)]
struct Look<'a> {
    theme: &'a Theme,
    font: &'a Font,
    mono: &'a Font,
    color: Rgba,
    /// Clickable links: the handler, and the count of clickable paragraphs drawn so far (their element ids).
    links: Option<(OnLink, Rc<Cell<usize>>)>,
}

/// Draw `blocks` as a column: text in `font`, code (blocks and spans) in `mono`, in the theme's text colors.
pub fn render(blocks: &[Block], theme: &Theme, font: &Font, mono: &Font) -> Div {
    column(
        blocks,
        &Look {
            theme,
            font,
            mono,
            color: theme.text,
            links: None,
        },
    )
}

/// [`render`] with clickable links: a click on one calls `on_link` with its target. `id` must be unique among its
/// siblings; the paragraphs' ids are scoped under it.
pub fn render_linked(
    id: impl Into<ElementId>,
    blocks: &[Block],
    theme: &Theme,
    font: &Font,
    mono: &Font,
    on_link: OnLink,
) -> Stateful<Div> {
    div().id(id).child(column(
        blocks,
        &Look {
            theme,
            font,
            mono,
            color: theme.text,
            links: Some((on_link, Rc::new(Cell::new(0)))),
        },
    ))
}

/// The links of `inlines`: each one's byte range in their joined text, and its target.
pub fn link_ranges(inlines: &[Inline]) -> Vec<(Range<usize>, String)> {
    let mut out: Vec<(Range<usize>, String)> = Vec::new();
    let mut at = 0;
    for inline in inlines {
        let end = at + inline.text.len();
        if let Some(url) = &inline.url {
            match out.last_mut() {
                // A link with bold or code inside is one link of several runs.
                Some((range, last)) if range.end == at && last == url => range.end = end,
                _ => out.push((at..end, url.clone())),
            }
        }
        at = end;
    }
    out
}

fn column(blocks: &[Block], look: &Look<'_>) -> Div {
    let mut col = div().flex().flex_col().gap(px(6.));
    for block in blocks {
        col = col.child(block_element(block, look));
    }
    col
}

/// Heading sizes, H1 to H3; smaller headings keep the surrounding size.
fn heading_size(level: u8, theme: &Theme) -> Option<Pixels> {
    let body = theme.typography.body;
    match level {
        1 => Some(body + px(4.)),
        2 => Some(body + px(2.)),
        3 => Some(body),
        _ => None,
    }
}

fn block_element(block: &Block, look: &Look<'_>) -> Div {
    let theme = look.theme;
    match block {
        Block::Paragraph(inlines) => div().child(text_element(inlines, look, false)),
        Block::Heading { level, inlines } => {
            let el = div().child(text_element(inlines, look, true));
            match heading_size(*level, theme) {
                Some(size) => el.text_size(size),
                None => el,
            }
        }
        Block::Code(code) => div()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .font(look.mono.clone())
            .text_color(look.color)
            .child(SharedString::from(code.clone())),
        Block::List { start, items } => {
            let mut col = div().flex().flex_col().gap(px(2.));
            for (i, item) in items.iter().enumerate() {
                col = col.child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex_none()
                                .text_color(theme.text_muted)
                                .child(marker(*start, i).trim_end().to_owned()),
                        )
                        .child(div().flex_1().min_w_0().child(column(item, look))),
                );
            }
            col
        }
        Block::Quote(inner) => div()
            .border_l_2()
            .border_color(theme.border)
            .pl_2()
            .child(column(
                inner,
                &Look {
                    color: theme.text_muted,
                    ..look.clone()
                },
            )),
        Block::Rule => div().h(px(1.)).w_full().bg(theme.border),
        Block::Table { header, rows } => {
            let columns = std::iter::once(header)
                .chain(rows)
                .map(Vec::len)
                .max()
                .unwrap_or(0);
            let row_el = |cells: &[Vec<Inline>], head: bool| {
                let mut r = div().flex();
                for c in 0..columns {
                    let cell = div().flex_1().min_w_0().px_2().py_1();
                    r = r.child(match cells.get(c) {
                        Some(inlines) => cell.child(text_element(inlines, look, head)),
                        None => cell,
                    });
                }
                r
            };
            let mut table = div()
                .flex()
                .flex_col()
                .border_1()
                .border_color(theme.border)
                .child(row_el(header, true));
            for row in rows {
                table = table.child(row_el(row, false).border_t_1().border_color(theme.border));
            }
            table
        }
    }
}

/// One paragraph's runs, clickable where they are links and the look has a handler.
fn text_element(inlines: &[Inline], look: &Look<'_>, bold: bool) -> AnyElement {
    let text = styled(inlines, look, bold);
    let links = link_ranges(inlines);
    match &look.links {
        Some((on_link, count)) if !links.is_empty() => {
            let n = count.get();
            count.set(n + 1);
            let on_link = on_link.clone();
            let (ranges, urls): (Vec<_>, Vec<_>) = links.into_iter().unzip();
            InteractiveText::new(ElementId::Integer(n as u64), text)
                .on_click(ranges, move |ix, window, cx| on_link(&urls[ix], window, cx))
                .into_any_element()
        }
        _ => text.into_any_element(),
    }
}

/// One paragraph's runs as styled text; `bold` makes all of it bold (headings, table headers).
fn styled(inlines: &[Inline], look: &Look<'_>, bold: bool) -> StyledText {
    let mut text = String::new();
    let mut runs = Vec::new();
    for inline in inlines {
        text.push_str(&inline.text);
        let mut f = if inline.style.code {
            look.mono.clone()
        } else {
            look.font.clone()
        };
        if bold || inline.style.bold {
            f.weight = FontWeight::BOLD;
        }
        if inline.style.italic {
            f.style = FontStyle::Italic;
        }
        let color = if inline.style.link {
            look.theme.accent
        } else {
            look.color
        };
        runs.push(TextRun {
            len: inline.text.len(),
            font: f,
            color: color.into(),
            background_color: None,
            underline: (inline.style.link && look.links.is_some()).then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(color.into()),
                wavy: false,
            }),
            strikethrough: inline.style.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(color.into()),
            }),
        });
    }
    StyledText::new(SharedString::from(text)).with_runs(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(blocks: &[Block]) -> Vec<(String, InlineStyle)> {
        blocks
            .iter()
            .flat_map(|b| match b {
                Block::Paragraph(r) | Block::Heading { inlines: r, .. } => {
                    r.iter().map(|i| (i.text.clone(), i.style)).collect()
                }
                Block::Code(c) => vec![(c.clone(), InlineStyle::default())],
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn roslyn_hover() {
        let md = "```csharp\nclass StreamJsonRpc.JsonRpc\n```\n  \nManages a JSON\\-RPC connection with another entity over a \
                  `Stream`\\.\nSecond line\\.\n\n**Remarks** see [docs](https://example.org) &amp; more";
        let blocks = parse(md);
        assert_eq!(blocks[0], Block::Code("class StreamJsonRpc.JsonRpc".into()));
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            plain_text(&blocks),
            "class StreamJsonRpc.JsonRpc\n\nManages a JSON-RPC connection with another entity over a Stream. Second \
             line.\n\nRemarks see docs & more"
        );
        let r = runs(&blocks[1..2]);
        assert_eq!(r[1].0, "Stream");
        assert!(r[1].1.code);
        assert!(!r[2].1.code);
        let r = runs(&blocks[2..]);
        assert_eq!(
            r[0],
            (
                "Remarks".into(),
                InlineStyle {
                    bold: true,
                    ..Default::default()
                }
            )
        );
        assert!(r.iter().any(|(t, s)| t == "docs" && s.link));
    }

    #[test]
    fn links_keep_their_targets() {
        let blocks = parse(
            "See [the **docs**](https://example.org/a) and [`a.rs`](src/a.rs#L3), ![img](x.png) <https://b.example>.",
        );
        let Block::Paragraph(inlines) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let text: String = inlines.iter().map(|i| i.text.as_str()).collect();
        let links: Vec<(&str, String)> = link_ranges(inlines)
            .into_iter()
            .map(|(r, url)| (&text[r], url))
            .collect();
        assert_eq!(
            links,
            [
                ("the docs", "https://example.org/a".to_owned()),
                ("a.rs", "src/a.rs#L3".to_owned()),
                ("https://b.example", "https://b.example".to_owned()),
            ]
        );
        // An image is not a link: its alt text has no target.
        assert!(
            inlines
                .iter()
                .any(|i| i.text.contains("img") && i.url.is_none() && !i.style.link)
        );
    }

    #[test]
    fn breaks_lists_headings_and_unclosed_marks() {
        let blocks = parse(
            "# Title\nline one  \nline two\n- a\n- b\n\n---\nsnake_case `unclosed and [x] *it*",
        );
        assert!(matches!(blocks[0], Block::Heading { level: 1, .. }));
        assert_eq!(
            plain_text(&blocks),
            "Title\n\nline one\nline two\n\n\u{2022} a\n\u{2022} b\n\nsnake_case `unclosed and [x] it"
        );
        assert_eq!(blocks[3], Block::Rule);
        let it = runs(&blocks[4..]);
        assert!(it.iter().any(|(t, s)| t == "it" && s.italic));
        assert!(parse("").is_empty());
        assert!(parse("\n\n  \n").is_empty());
        assert_eq!(plain_text(&parse("a&nbsp;b<br>c")), "a\u{a0}b\nc");
        assert_eq!(plain_text(&parse("``a ` b``")), "a ` b");
    }

    #[test]
    fn nested_lists_quotes_and_tasks() {
        let blocks = parse(
            "1. first\n2. second\n   - inner *one*\n   - inner two\n\n> quoted\n> **still**\n\n- [x] done\n- [ ] open",
        );
        assert_eq!(blocks.len(), 3);
        let Block::List { start, items } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(*start, Some(1));
        assert!(matches!(items[1][1], Block::List { start: None, .. }));
        assert_eq!(
            plain_text(&blocks),
            "1. first\n2. second\n   \u{2022} inner one\n   \u{2022} inner two\n\n> quoted still\n\n\
             \u{2022} \u{2611} done\n\u{2022} \u{2610} open"
        );
        // A loose list: each item is a paragraph, the same blocks as a tight one.
        assert_eq!(parse("- a\n\n- b"), parse("- a\n- b"));
    }

    #[test]
    fn tables_strikethrough_and_html() {
        let blocks = parse(
            "| Name | Size |\n|---|--:|\n| `a.rs` | 1 |\n| b | ~~2~~ |\n\n<details>raw</details>",
        );
        let Block::Table { header, rows } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(header.len(), 2);
        assert_eq!(rows.len(), 2);
        assert!(rows[0][0][0].style.code);
        assert!(rows[1][1][0].style.strikethrough);
        assert_eq!(
            plain_text(&blocks),
            "Name | Size\na.rs | 1\nb | 2\n\n<details>raw</details>"
        );
    }

    #[test]
    fn block_starts_split_top_level_blocks() {
        let md = "Intro line\nsame paragraph\n\n```rust\nfn a() {}\n\nfn b() {}\n```\n- x\n- y\n\n---\n## Tail";
        let starts = block_starts(md);
        let pieces: Vec<&str> = starts
            .iter()
            .zip(starts.iter().skip(1).chain([&md.len()]))
            .map(|(&a, &b)| &md[a..b])
            .collect();
        assert_eq!(
            pieces,
            [
                "Intro line\nsame paragraph\n\n",
                "```rust\nfn a() {}\n\nfn b() {}\n```\n",
                "- x\n- y\n\n",
                "---\n",
                "## Tail"
            ]
        );
        // An unclosed fence runs to the end: one block.
        assert_eq!(block_starts("```\ncode\n\nmore"), [0]);
        assert!(block_starts("  \n").is_empty());
    }
}
