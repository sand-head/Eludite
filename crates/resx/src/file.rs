//! One `.resx` file: its entries with their byte ranges, and the splices that edit it.

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// How the file's bytes encode its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8, with or without a byte order mark.
    Utf8 { bom: bool },
}

/// A replacement of a byte range of the file's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub range: Range<usize>,
    pub text: String,
}

/// What a `data` element holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// A string: no `type` and no `mimetype`, or a `System.String` type.
    String,
    /// Anything else (a `ResXFileRef` file, serialized binary data, WinForms designer state): listed, never edited.
    Other {
        type_name: Option<String>,
        mimetype: Option<String>,
    },
}

/// A `data` element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// The value, entities resolved; empty when the element has no `value`.
    pub value: String,
    /// The comment, entities resolved; `None` when the element has no `comment`.
    pub comment: Option<String>,
    /// 1-based.
    pub line: u32,
    /// The whole `data` element.
    range: Range<usize>,
    /// The `name` attribute's value, between its quotes.
    name_range: Range<usize>,
    /// The `value` element, and its content between the tags (`None` when self-closing).
    value_element: Option<Range<usize>>,
    value_inner: Option<Range<usize>>,
    /// The `comment` element, and its content between the tags.
    comment_element: Option<Range<usize>>,
    comment_inner: Option<Range<usize>>,
}

impl Entry {
    /// The byte range of the whole `data` element in the file's text.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// A string entry (editable) or not.
    pub fn is_string(&self) -> bool {
        self.kind == EntryKind::String
    }
}

/// Why a file could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// 1-based, when the parser knows it.
    pub line: Option<u32>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ParseError {}

/// A parsed `.resx` file.
#[derive(Debug, Clone)]
pub struct ResxFile {
    path: PathBuf,
    text: String,
    encoding: Encoding,
    newline: String,
    indent: String,
    inner_indent: String,
    entries: Vec<Entry>,
    /// Entry index by name.
    index: std::collections::HashMap<String, usize>,
    /// Where a new `data` element goes: right after the last element inside `root`, before `</root>`.
    insert_at: usize,
}

/// Visual Studio's empty `.resx`: the schema block and the four `resheader` entries, nothing else.
pub const TEMPLATE: &str = include_str!("template.resx");

impl ResxFile {
    /// Parse a file's bytes. UTF-8 with or without a byte order mark; anything else is an error (open as text).
    pub fn parse(path: impl Into<PathBuf>, bytes: &[u8]) -> Result<ResxFile, ParseError> {
        let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
            Some(rest) => (true, rest),
            None => (false, bytes),
        };
        if body.starts_with(&[0xFF, 0xFE]) || body.starts_with(&[0xFE, 0xFF]) {
            return Err(ParseError {
                message: "UTF-16 .resx files are not supported: open as text".into(),
                line: None,
            });
        }
        let text = std::str::from_utf8(body)
            .map_err(|e| ParseError {
                message: format!("not UTF-8: {e}"),
                line: None,
            })?
            .to_string();
        Self::from_text(path, text, Encoding::Utf8 { bom })
    }

    /// Parse text (no byte order mark) that will be written with `encoding`.
    pub fn from_text(
        path: impl Into<PathBuf>,
        text: String,
        encoding: Encoding,
    ) -> Result<ResxFile, ParseError> {
        let mut file = ResxFile {
            path: path.into(),
            text: String::new(),
            encoding,
            newline: String::new(),
            indent: String::new(),
            inner_indent: String::new(),
            entries: Vec::new(),
            index: std::collections::HashMap::new(),
            insert_at: 0,
        };
        file.reparse(text)?;
        Ok(file)
    }

    /// Visual Studio's empty file at `path`, with CRLF line endings and a byte order mark as Visual Studio writes.
    pub fn empty(path: impl Into<PathBuf>) -> ResxFile {
        Self::from_text(
            path,
            TEMPLATE
                .trim_end()
                .replace("\r\n", "\n")
                .replace('\n', "\r\n"),
            Encoding::Utf8 { bom: true },
        )
        .expect("the template parses")
    }

    fn reparse(&mut self, text: String) -> Result<(), ParseError> {
        let doc = roxmltree::Document::parse(&text).map_err(|e| ParseError {
            message: e.to_string(),
            line: Some(e.pos().row),
        })?;
        let root = doc.root_element();
        if root.tag_name().name() != "root" {
            return Err(ParseError {
                message: format!(
                    "the document element is `{}`, not `root`",
                    root.tag_name().name()
                ),
                line: Some(line_of(&text, root.range().start)),
            });
        }
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" }.to_string();
        let newlines = Lines::of(&text);
        let mut entries = Vec::new();
        let mut last_end = root_open_end(&text, root.range());
        let mut indent = None;
        let mut inner_indent = None;
        for node in root.children() {
            if !node.is_element() {
                continue;
            }
            last_end = node.range().end;
            if indent.is_none()
                && matches!(node.tag_name().name(), "data" | "resheader" | "assembly")
            {
                indent = Some(leading_whitespace(&text, node.range().start));
            }
            if node.tag_name().name() != "data" {
                continue;
            }
            let Some(name_attr) = node.attribute_node("name") else {
                continue;
            };
            let mut value_element = None;
            let mut value_inner = None;
            let mut value = String::new();
            let mut comment_element = None;
            let mut comment_inner = None;
            let mut comment = None;
            for child in node.children().filter(|c| c.is_element()) {
                match child.tag_name().name() {
                    "value" if value_element.is_none() => {
                        value_element = Some(child.range());
                        value_inner = inner_range(&text, child.range());
                        value = child.text().unwrap_or("").to_string();
                        if inner_indent.is_none() {
                            inner_indent = Some(leading_whitespace(&text, child.range().start));
                        }
                    }
                    "comment" if comment_element.is_none() => {
                        comment_element = Some(child.range());
                        comment_inner = inner_range(&text, child.range());
                        comment = Some(child.text().unwrap_or("").to_string());
                    }
                    _ => {}
                }
            }
            let type_name = node.attribute("type").map(str::to_string);
            let mimetype = node.attribute("mimetype").map(str::to_string);
            let kind = match (&type_name, &mimetype) {
                (None, None) => EntryKind::String,
                (Some(t), None) if t.starts_with("System.String") => EntryKind::String,
                _ => EntryKind::Other {
                    type_name,
                    mimetype,
                },
            };
            entries.push(Entry {
                name: name_attr.value().to_string(),
                kind,
                value,
                comment,
                line: newlines.line_of(node.range().start),
                range: node.range(),
                name_range: name_attr.range_value(),
                value_element,
                value_inner,
                comment_element,
                comment_inner,
            });
        }
        let indent = indent.unwrap_or_else(|| "  ".to_string());
        self.inner_indent = inner_indent.unwrap_or_else(|| format!("{indent}{indent}"));
        self.indent = indent;
        self.newline = newline;
        self.index = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.name.clone(), i))
            .collect();
        self.entries = entries;
        self.insert_at = last_end;
        self.text = text;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The text, without a byte order mark.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// The file's line ending (`\r\n` when it has one).
    pub fn newline(&self) -> &str {
        &self.newline
    }

    /// The bytes to write: the text with the byte order mark the file had.
    pub fn to_bytes(&self) -> Vec<u8> {
        let Encoding::Utf8 { bom } = self.encoding;
        let mut out = Vec::with_capacity(self.text.len() + 3);
        if bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(self.text.as_bytes());
        out
    }

    /// Every `data` element, in file order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The string entries, in file order.
    pub fn strings(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_string())
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.index.get(name).and_then(|i| self.entries.get(*i))
    }

    /// The 1-based line of a byte offset.
    pub fn line_of(&self, offset: usize) -> u32 {
        line_of(&self.text, offset)
    }

    /// The splice that gives `name` the value `value`. `None` when the entry is missing or not a string.
    pub fn set_value(&self, name: &str, value: &str) -> Option<Splice> {
        let entry = self.entry(name).filter(|e| e.is_string())?;
        let escaped = self.escape(value);
        Some(match (&entry.value_inner, &entry.value_element) {
            (Some(inner), _) => Splice {
                range: inner.clone(),
                text: escaped,
            },
            (None, Some(element)) => Splice {
                range: element.clone(),
                text: format!("<value>{escaped}</value>"),
            },
            (None, None) => {
                let at = data_close_start(&self.text, &entry.range);
                Splice {
                    range: at..at,
                    text: format!(
                        "{}<value>{escaped}</value>{}{}",
                        self.inner_indent, self.newline, self.indent
                    ),
                }
            }
        })
    }

    /// The splice that gives `name` the comment `comment` (`None` or empty removes the element). `None` when the
    /// entry is missing, not a string, or already has that comment.
    pub fn set_comment(&self, name: &str, comment: Option<&str>) -> Option<Splice> {
        let entry = self.entry(name).filter(|e| e.is_string())?;
        let comment = comment.filter(|c| !c.is_empty());
        match (comment, &entry.comment_element) {
            (None, None) => None,
            (None, Some(element)) => Some(Splice {
                range: line_range(&self.text, element),
                text: String::new(),
            }),
            (Some(c), Some(element)) => {
                let escaped = self.escape(c);
                Some(match &entry.comment_inner {
                    Some(inner) => Splice {
                        range: inner.clone(),
                        text: escaped,
                    },
                    None => Splice {
                        range: element.clone(),
                        text: format!("<comment>{escaped}</comment>"),
                    },
                })
            }
            (Some(c), None) => {
                let escaped = self.escape(c);
                let at = match &entry.value_element {
                    Some(v) => v.end,
                    None => data_close_start(&self.text, &entry.range),
                };
                Some(Splice {
                    range: at..at,
                    text: format!(
                        "{}{}<comment>{escaped}</comment>",
                        self.newline, self.inner_indent
                    ),
                })
            }
        }
    }

    /// The splice that appends a new string entry in Visual Studio's shape. `None` when the name exists.
    pub fn add(&self, name: &str, value: &str, comment: Option<&str>) -> Option<Splice> {
        if self.entry(name).is_some() {
            return None;
        }
        let nl = &self.newline;
        let (i1, i2) = (&self.indent, &self.inner_indent);
        let mut text = format!(
            "{nl}{i1}<data name=\"{}\" xml:space=\"preserve\">{nl}{i2}<value>{}</value>",
            escape_attribute(name),
            self.escape(value)
        );
        if let Some(c) = comment.filter(|c| !c.is_empty()) {
            text.push_str(&format!("{nl}{i2}<comment>{}</comment>", self.escape(c)));
        }
        text.push_str(&format!("{nl}{i1}</data>"));
        Some(Splice {
            range: self.insert_at..self.insert_at,
            text,
        })
    }

    /// The splice that removes `name`'s element and the line it sat on. `None` when it is missing.
    pub fn remove(&self, name: &str) -> Option<Splice> {
        let entry = self.entry(name)?;
        Some(Splice {
            range: line_range(&self.text, &entry.range),
            text: String::new(),
        })
    }

    /// The splice that renames `name` to `new_name`. `None` when `name` is missing or `new_name` exists.
    pub fn rename(&self, name: &str, new_name: &str) -> Option<Splice> {
        if self.entry(new_name).is_some() {
            return None;
        }
        let entry = self.entry(name)?;
        Some(Splice {
            range: entry.name_range.clone(),
            text: escape_attribute(new_name),
        })
    }

    /// The text with every `data` element removed: a new culture file's content, with this file's header.
    pub fn header_only(&self) -> String {
        let splices: Vec<Splice> = self
            .entries
            .iter()
            .filter_map(|e| self.remove(&e.name))
            .collect();
        apply(&self.text, &splices)
    }

    /// The text with the `data` elements sorted by name (ordinal, ignoring case), for `resx.sortOnSave`. `None`
    /// when something other than white space sits between two `data` elements, so nothing could be lost.
    pub fn sorted_text(&self) -> Option<String> {
        if self.entries.len() < 2 {
            return Some(self.text.clone());
        }
        let ranges: Vec<Range<usize>> = self
            .entries
            .iter()
            .map(|e| line_range(&self.text, &e.range))
            .collect();
        for pair in ranges.windows(2) {
            if !self.text[pair[0].end..pair[1].start].trim().is_empty() {
                return None;
            }
        }
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by(|a, b| {
            let (a, b) = (&self.entries[*a].name, &self.entries[*b].name);
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        });
        let mut out = String::with_capacity(self.text.len());
        out.push_str(&self.text[..ranges[0].start]);
        for (i, ix) in order.iter().enumerate() {
            out.push_str(&self.text[ranges[*ix].clone()]);
            if i + 1 < order.len() {
                // The gap after the element as it was, so blank lines between entries keep their count.
                let gap = &self.text[ranges[i].end..ranges[i + 1].start];
                out.push_str(gap);
            }
        }
        out.push_str(&self.text[ranges[ranges.len() - 1].end..]);
        Some(out)
    }

    /// Apply splices and reparse. The splices must not overlap.
    pub fn apply(&mut self, splices: &[Splice]) -> Result<(), ParseError> {
        if splices.is_empty() {
            return Ok(());
        }
        let text = apply(&self.text, splices);
        self.reparse(text)
    }

    /// Replace the whole text (an external change, or a sorted text) and reparse.
    pub fn set_text(&mut self, text: String) -> Result<(), ParseError> {
        self.reparse(text)
    }

    fn escape(&self, value: &str) -> String {
        escape_text(value, &self.newline)
    }
}

/// Apply non-overlapping splices to `text`.
pub fn apply(text: &str, splices: &[Splice]) -> String {
    let mut sorted: Vec<&Splice> = splices.iter().collect();
    sorted.sort_by_key(|s| (s.range.start, s.range.end));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for s in sorted {
        let start = s.range.start.max(at);
        out.push_str(&text[at..start]);
        out.push_str(&s.text);
        at = s.range.end.max(start);
    }
    out.push_str(&text[at..]);
    out
}

/// XML text escaping as Visual Studio writes values: `&`, `<` and `>` as entities, a line break as the file's.
pub fn escape_text(value: &str, newline: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(newline);
            }
            '\n' => out.push_str(newline),
            c => out.push(c),
        }
    }
    out
}

fn escape_attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            '\t' => out.push_str("&#x9;"),
            c => out.push(c),
        }
    }
    out
}

fn line_of(text: &str, offset: usize) -> u32 {
    Lines::of(text).line_of(offset)
}

/// The byte offsets of a text's line breaks, for 1-based lines by binary search.
struct Lines(Vec<usize>);

impl Lines {
    fn of(text: &str) -> Lines {
        Lines(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i)
                .collect(),
        )
    }

    fn line_of(&self, offset: usize) -> u32 {
        self.0.partition_point(|nl| *nl < offset) as u32 + 1
    }
}

/// The white space between the start of the line and `offset`.
fn leading_whitespace(text: &str, offset: usize) -> String {
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let ws = &text[line_start..offset];
    if ws.chars().all(|c| c == ' ' || c == '\t') {
        ws.to_string()
    } else {
        "  ".to_string()
    }
}

/// The range to remove for an element that sits on its own line: from the line break before its indentation to its
/// end, so the line disappears. An element sharing its line with something else loses only itself.
fn line_range(text: &str, element: &Range<usize>) -> Range<usize> {
    let mut start = element.start;
    while start > 0 && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    if start > 0 && text.as_bytes()[start - 1] == b'\n' {
        start -= 1;
        if start > 0 && text.as_bytes()[start - 1] == b'\r' {
            start -= 1;
        }
        start..element.end
    } else if start == 0 {
        0..element.end
    } else {
        element.clone()
    }
}

/// The content range of an element between `<tag ...>` and `</tag>`; `None` for a self-closing one.
fn inner_range(text: &str, element: Range<usize>) -> Option<Range<usize>> {
    let slice = &text[element.clone()];
    if slice.ends_with("/>") && !slice.contains("</") {
        return None;
    }
    let open_end = element.start + slice.find('>')? + 1;
    let close_start = element.start + slice.rfind("</")?;
    (open_end <= close_start).then_some(open_end..close_start)
}

/// Where `</data>` starts inside a `data` element (the element's end when it is self-closing).
fn data_close_start(text: &str, element: &Range<usize>) -> usize {
    let slice = &text[element.clone()];
    match slice.rfind("</data") {
        Some(i) => element.start + i,
        None => element.end,
    }
}

/// The end of the root element's start tag.
fn root_open_end(text: &str, root: Range<usize>) -> usize {
    root.start + text[root.clone()].find('>').map(|i| i + 1).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<root>\r\n  <resheader name=\"resmimetype\">\r\n    <value>text/microsoft-resx</value>\r\n  </resheader>\r\n  <data name=\"Hello\" xml:space=\"preserve\">\r\n    <value>Hello &amp; welcome</value>\r\n    <comment>A greeting</comment>\r\n  </data>\r\n  <data name=\"Empty\" xml:space=\"preserve\">\r\n    <value />\r\n  </data>\r\n  <data name=\"Icon\" type=\"System.Resources.ResXFileRef, System.Windows.Forms\">\r\n    <value>app.ico;System.Drawing.Icon, System.Drawing</value>\r\n  </data>\r\n</root>";

    fn small() -> ResxFile {
        ResxFile::parse("Resources.resx", SMALL.as_bytes()).unwrap()
    }

    #[test]
    fn parses_entries_with_kinds_values_comments_and_lines() {
        let f = small();
        assert_eq!(f.newline(), "\r\n");
        assert_eq!(f.encoding(), Encoding::Utf8 { bom: false });
        let names: Vec<&str> = f.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Hello", "Empty", "Icon"]);
        let hello = f.entry("Hello").unwrap();
        assert_eq!(hello.value, "Hello & welcome");
        assert_eq!(hello.comment.as_deref(), Some("A greeting"));
        assert_eq!(hello.line, 6);
        assert!(hello.is_string());
        assert_eq!(f.entry("Empty").unwrap().value, "");
        assert!(!f.entry("Icon").unwrap().is_string());
        assert_eq!(f.strings().count(), 2);
    }

    #[test]
    fn a_bom_is_kept_and_utf16_refused() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(SMALL.as_bytes());
        let f = ResxFile::parse("r.resx", &bytes).unwrap();
        assert_eq!(f.encoding(), Encoding::Utf8 { bom: true });
        assert_eq!(f.to_bytes(), bytes);
        let err = ResxFile::parse("r.resx", &[0xFF, 0xFE, 0x3C, 0x00]).unwrap_err();
        assert!(err.message.contains("UTF-16"));
        let err = ResxFile::parse("r.resx", b"<root><data").unwrap_err();
        assert_eq!(err.line, Some(1));
    }

    #[test]
    fn set_value_splices_only_the_text() {
        let mut f = small();
        let s = f.set_value("Hello", "Hi <you>").unwrap();
        assert_eq!(s.text, "Hi &lt;you&gt;");
        f.apply(&[s]).unwrap();
        assert_eq!(f.entry("Hello").unwrap().value, "Hi <you>");
        assert_eq!(
            f.text().replace("Hi &lt;you&gt;", "Hello &amp; welcome"),
            SMALL
        );
        // A self-closing value gets a full element.
        let s = f.set_value("Empty", "x").unwrap();
        assert_eq!(s.text, "<value>x</value>");
        f.apply(&[s]).unwrap();
        assert!(f.text().contains("    <value>x</value>\r\n  </data>"));
        assert!(f.set_value("Icon", "x").is_none());
        assert!(f.set_value("Missing", "x").is_none());
        // A line break in a value is written as the file's.
        let s = f.set_value("Hello", "a\nb").unwrap();
        assert_eq!(s.text, "a\r\nb");
    }

    #[test]
    fn comments_are_set_added_and_removed() {
        let mut f = small();
        f.apply(&[f.set_comment("Hello", Some("Changed")).unwrap()])
            .unwrap();
        assert_eq!(
            f.entry("Hello").unwrap().comment.as_deref(),
            Some("Changed")
        );
        f.apply(&[f.set_comment("Hello", None).unwrap()]).unwrap();
        assert_eq!(f.entry("Hello").unwrap().comment, None);
        assert!(
            f.text()
                .contains("<value>Hello &amp; welcome</value>\r\n  </data>")
        );
        assert!(f.set_comment("Hello", None).is_none());
        f.apply(&[f.set_comment("Hello", Some("Back")).unwrap()])
            .unwrap();
        assert!(f.text().contains(
            "<value>Hello &amp; welcome</value>\r\n    <comment>Back</comment>\r\n  </data>"
        ));
        f.apply(&[f.set_comment("Empty", Some("On empty")).unwrap()])
            .unwrap();
        assert!(
            f.text()
                .contains("<value />\r\n    <comment>On empty</comment>\r\n  </data>")
        );
    }

    #[test]
    fn add_appends_in_visual_studio_shape_and_remove_takes_the_line() {
        let mut f = small();
        assert!(f.add("Hello", "x", None).is_none());
        f.apply(&[f.add("New", "A & B", Some("c")).unwrap()])
            .unwrap();
        assert!(f.text().ends_with(
            "  </data>\r\n  <data name=\"New\" xml:space=\"preserve\">\r\n    <value>A &amp; B</value>\r\n    <comment>c</comment>\r\n  </data>\r\n</root>"
        ));
        assert_eq!(f.entry("New").unwrap().line, 16);
        f.apply(&[f.remove("New").unwrap()]).unwrap();
        assert_eq!(f.text(), SMALL);
        f.apply(&[f.remove("Hello").unwrap(), f.remove("Icon").unwrap()])
            .unwrap();
        let names: Vec<&str> = f.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Empty"]);
        assert!(f.text().contains("</resheader>\r\n  <data name=\"Empty\""));
        assert!(f.text().ends_with("  </data>\r\n</root>"));
    }

    #[test]
    fn rename_changes_the_attribute_only() {
        let mut f = small();
        assert!(f.rename("Hello", "Empty").is_none());
        assert!(f.rename("Missing", "X").is_none());
        f.apply(&[f.rename("Hello", "Hi \"there\"").unwrap()])
            .unwrap();
        assert_eq!(f.entry("Hi \"there\"").unwrap().value, "Hello & welcome");
        assert!(
            f.text()
                .contains("<data name=\"Hi &quot;there&quot;\" xml:space=\"preserve\">")
        );
    }

    #[test]
    fn header_only_keeps_everything_but_data() {
        let f = small();
        let h =
            ResxFile::from_text("x.resx", f.header_only(), Encoding::Utf8 { bom: false }).unwrap();
        assert!(h.entries().is_empty());
        assert!(h.text().contains("<resheader name=\"resmimetype\">"));
        assert_eq!(
            h.text(),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<root>\r\n  <resheader name=\"resmimetype\">\r\n    <value>text/microsoft-resx</value>\r\n  </resheader>\r\n</root>"
        );
        let added = h.add("First", "1", None).unwrap();
        assert!(apply(h.text(), &[added]).ends_with("</resheader>\r\n  <data name=\"First\" xml:space=\"preserve\">\r\n    <value>1</value>\r\n  </data>\r\n</root>"));
    }

    #[test]
    fn sorted_text_orders_entries_and_refuses_when_something_sits_between() {
        let f = small();
        let sorted = f.sorted_text().unwrap();
        let s = ResxFile::from_text("x", sorted, Encoding::Utf8 { bom: false }).unwrap();
        let names: Vec<&str> = s.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Empty", "Hello", "Icon"]);
        assert_eq!(s.text().len(), SMALL.len());
        let between = SMALL.replace(
            "  </data>\r\n  <data name=\"Empty\"",
            "  </data>\r\n  <!-- between -->\r\n  <data name=\"Empty\"",
        );
        let f = ResxFile::parse("x", between.as_bytes()).unwrap();
        assert!(f.sorted_text().is_none());
    }

    #[test]
    fn the_template_is_an_empty_visual_studio_file() {
        let f = ResxFile::empty("New.resx");
        assert!(f.entries().is_empty());
        assert_eq!(f.newline(), "\r\n");
        assert_eq!(f.encoding(), Encoding::Utf8 { bom: true });
        assert!(f.text().contains("<resheader name=\"writer\">"));
        let s = f.add("K", "v", None).unwrap();
        let t = apply(f.text(), &[s]);
        assert!(t.ends_with("  </resheader>\r\n  <data name=\"K\" xml:space=\"preserve\">\r\n    <value>v</value>\r\n  </data>\r\n</root>"));
    }

    #[test]
    fn lf_files_and_odd_indentation_are_followed() {
        let text = "<root>\n\t<data name=\"A\">\n\t\t<value>a</value>\n\t</data>\n</root>\n";
        let f = ResxFile::parse("x", text.as_bytes()).unwrap();
        assert_eq!(f.newline(), "\n");
        let t = apply(f.text(), &[f.add("B", "b", None).unwrap()]);
        assert_eq!(
            t,
            "<root>\n\t<data name=\"A\">\n\t\t<value>a</value>\n\t</data>\n\t<data name=\"B\" xml:space=\"preserve\">\n\t\t<value>b</value>\n\t</data>\n</root>\n"
        );
        let t = apply(f.text(), &[f.remove("A").unwrap()]);
        assert_eq!(t, "<root>\n</root>\n");
    }
}
