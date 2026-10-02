//! A small Markdown subset for IntelliSense tooltips (Quick Info, completion documentation, Parameter Info):
//! paragraphs, fenced code blocks, inline code spans, bold and italic, backslash escapes, links (shown as their text)
//! and the HTML entities language servers emit. Everything else is shown as plain text. No HTML, tables or images.
//!
//! [`parse`] turns Markdown into [`Block`]s; [`plain_text`] flattens them (what an agent reads); [`render`] draws them
//! with the theme's tokens.

use gpui::{
    Div, Font, FontStyle, FontWeight, IntoElement, ParentElement, SharedString, Styled, StyledText,
    TextRun, div, px,
};

use crate::Theme;

/// How a run of inline text is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InlineStyle {
    /// An inline code span: monospace.
    pub code: bool,
    pub bold: bool,
    pub italic: bool,
}

/// A run of text with one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: InlineStyle,
}

/// A block of a Markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A paragraph: inline runs; `\n` in a run is a hard line break.
    Paragraph(Vec<Inline>),
    /// A fenced (or indented) code block, without the fences.
    Code(String),
}

/// Parse `markdown` into blocks. Never fails: what is not understood stays as text.
pub fn parse(markdown: &str) -> Vec<Block> {
    let text = markdown.replace("\r\n", "\n");
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut lines = text.lines().peekable();
    let flush = |para: &mut Vec<String>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            let joined = join_lines(para);
            let inlines = parse_inline(&joined);
            if inlines.iter().any(|i| !i.text.trim().is_empty()) {
                blocks.push(Block::Paragraph(inlines));
            }
            para.clear();
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(fence) = ["```", "~~~"].iter().find(|f| trimmed.starts_with(**f)) {
            flush(&mut para, &mut blocks);
            let mut code = Vec::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with(fence) {
                    break;
                }
                code.push(l);
            }
            blocks.push(Block::Code(code.join("\n")));
            continue;
        }
        if trimmed.is_empty() || is_rule(trimmed) {
            flush(&mut para, &mut blocks);
            continue;
        }
        let content = trimmed.trim_start_matches('#');
        let content = if content.len() != trimmed.len() && content.starts_with(' ') {
            // A heading: its own paragraph.
            flush(&mut para, &mut blocks);
            para.push(content.trim().to_owned());
            flush(&mut para, &mut blocks);
            continue;
        } else {
            line
        };
        let content = match trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            Some(item) => format!("\u{2022} {item}"),
            None => content.to_owned(),
        };
        para.push(content);
    }
    flush(&mut para, &mut blocks);
    blocks
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*' || c == '_'))
}

/// Joins paragraph lines: a soft break is a space; two trailing spaces, a trailing backslash or a list item make a hard
/// break.
fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        let hard = line.ends_with("  ") || line.ends_with('\\');
        let body = line.trim_end_matches('\\').trim_end();
        let body = if i == 0 { body } else { body.trim_start() };
        out.push_str(body);
        if i + 1 < lines.len() {
            let next_is_item = lines[i + 1].starts_with('\u{2022}');
            out.push(if hard || next_is_item { '\n' } else { ' ' });
        }
    }
    out
}

const ENTITIES: [(&str, &str); 7] = [
    ("&nbsp;", "\u{a0}"),
    ("&lt;", "<"),
    ("&gt;", ">"),
    ("&amp;", "&"),
    ("&quot;", "\""),
    ("&#39;", "'"),
    ("&apos;", "'"),
];

/// Inline runs of one paragraph.
fn parse_inline(text: &str) -> Vec<Inline> {
    let mut runs: Vec<Inline> = Vec::new();
    let mut style = InlineStyle::default();
    let mut buf = String::new();
    let push = |runs: &mut Vec<Inline>, buf: &mut String, style: InlineStyle| {
        if buf.is_empty() {
            return;
        }
        match runs.last_mut() {
            Some(last) if last.style == style => last.text.push_str(buf),
            _ => runs.push(Inline {
                text: std::mem::take(buf),
                style,
            }),
        }
        buf.clear();
    };
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        let rest = &text[at..];
        match c {
            '\\' if i + 1 < chars.len() && chars[i + 1].1.is_ascii_punctuation() => {
                buf.push(chars[i + 1].1);
                i += 2;
            }
            '`' => {
                let ticks = rest.chars().take_while(|&c| c == '`').count();
                let fence = &rest[..ticks];
                match rest[ticks..].find(fence) {
                    Some(end) => {
                        push(&mut runs, &mut buf, style);
                        let code = rest[ticks..ticks + end].trim();
                        buf.push_str(code);
                        push(
                            &mut runs,
                            &mut buf,
                            InlineStyle {
                                code: true,
                                ..style
                            },
                        );
                        let consumed = ticks + end + ticks;
                        i += rest[..consumed].chars().count();
                    }
                    None => {
                        buf.push_str(fence);
                        i += ticks;
                    }
                }
            }
            '*' | '_' if rest.starts_with("**") || rest.starts_with("__") => {
                push(&mut runs, &mut buf, style);
                style.bold = !style.bold;
                i += 2;
            }
            '*' => {
                push(&mut runs, &mut buf, style);
                style.italic = !style.italic;
                i += 1;
            }
            '[' => match link(rest) {
                Some((label, consumed)) => {
                    buf.push_str(label);
                    i += rest[..consumed].chars().count();
                }
                None => {
                    buf.push(c);
                    i += 1;
                }
            },
            '&' => match ENTITIES.iter().find(|(e, _)| rest.starts_with(e)) {
                Some((e, decoded)) => {
                    buf.push_str(decoded);
                    i += e.len();
                }
                None => {
                    buf.push(c);
                    i += 1;
                }
            },
            '<' if rest.starts_with("<br>")
                || rest.starts_with("<br/>")
                || rest.starts_with("<br />") =>
            {
                buf.push('\n');
                i += rest.find('>').map_or(1, |e| e + 1);
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    push(&mut runs, &mut buf, style);
    runs
}

/// `[label](target)` at the start of `s`: the label and the bytes consumed.
fn link(s: &str) -> Option<(&str, usize)> {
    let close = s.find("](")?;
    let label = &s[1..close];
    if label.contains('[') || label.contains('\n') {
        return None;
    }
    let end = s[close + 2..].find(')')? + close + 2;
    Some((label, end + 1))
}

/// The blocks as plain text: paragraphs and code blocks separated by blank lines.
pub fn plain_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(runs) => runs.iter().map(|r| r.text.as_str()).collect::<String>(),
            Block::Code(code) => code.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Draw `blocks` as a column: paragraphs in `font`, code (blocks and spans) in `mono`, in the theme's text colors.
pub fn render(blocks: &[Block], theme: &Theme, font: &Font, mono: &Font) -> Div {
    let mut col = div().flex().flex_col().gap(px(6.));
    for block in blocks {
        col = col.child(match block {
            Block::Paragraph(inlines) => {
                let mut text = String::new();
                let mut runs = Vec::new();
                for inline in inlines {
                    text.push_str(&inline.text);
                    let mut f = if inline.style.code {
                        mono.clone()
                    } else {
                        font.clone()
                    };
                    if inline.style.bold {
                        f.weight = FontWeight::BOLD;
                    }
                    if inline.style.italic {
                        f.style = FontStyle::Italic;
                    }
                    runs.push(TextRun {
                        len: inline.text.len(),
                        font: f,
                        color: theme.text.into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
                div()
                    .child(StyledText::new(SharedString::from(text)).with_runs(runs))
                    .into_any_element()
            }
            Block::Code(code) => div()
                .font(mono.clone())
                .text_color(theme.text)
                .child(SharedString::from(code.clone()))
                .into_any_element(),
        });
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(blocks: &[Block]) -> Vec<(String, InlineStyle)> {
        blocks
            .iter()
            .flat_map(|b| match b {
                Block::Paragraph(r) => r.iter().map(|i| (i.text.clone(), i.style)).collect(),
                Block::Code(c) => vec![(c.clone(), InlineStyle::default())],
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
    }

    #[test]
    fn breaks_lists_headings_and_unclosed_marks() {
        let blocks = parse(
            "# Title\nline one  \nline two\n- a\n- b\n\n---\nsnake_case `unclosed and [x] *it*",
        );
        assert_eq!(
            plain_text(&blocks),
            "Title\n\nline one\nline two\n\u{2022} a\n\u{2022} b\n\nsnake_case `unclosed and [x] it"
        );
        let it = runs(&blocks[2..]);
        assert!(it.iter().any(|(t, s)| t == "it" && s.italic));
        assert!(parse("").is_empty());
        assert!(parse("\n\n  \n").is_empty());
        assert_eq!(plain_text(&parse("a&nbsp;b<br>c")), "a\u{a0}b\nc");
        assert_eq!(plain_text(&parse("``a ` b``")), "a ` b");
    }
}
