//! Emmet abbreviations on Tab (brief 0050): a small expander for the common forms, used in HTML and CSS documents when
//! no completion item is selected (the HTML and CSS language servers of `vscode-langservers-extracted` offer no Emmet
//! items of their own).
//!
//! HTML: element names (`div`), classes and ids (`div.card#main`, `.a` with an implicit tag: `li` in a list, `tr` in a
//! table, `td` in a row, `option` in a select, else `div`), attributes (`a[href=x]`), text (`p{Hello}`), the child
//! (`>`), sibling (`+`) and climb-up (`^`) operators, multiplication (`li*3`) with `$` numbering (`li.item$*2`), and
//! groups (`(dt+dd)*2`). An abbreviation is recognized only when its first element is a known HTML element (or a class
//! or id), so a plain word is never expanded. CSS: the property shorthands of [`CSS_PROPERTIES`] with a value
//! (`m10` is `margin: 10px;`, `w50p` `width: 50%;`, `m10-20` `margin: 10px 20px;`, `c#fff` `color: #fff;`), and
//! the keyword forms of [`CSS_KEYWORDS`] (`df` is `display: flex;`).

/// Which Emmet syntax a document uses (from its language, [`crate::syntax::LanguageConfig`]'s `emmet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmmetSyntax {
    Html,
    Css,
}

/// An expansion: the text that replaces the abbreviation, and where the caret goes in it (a byte offset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub caret: usize,
}

/// The abbreviation that ends at the end of `line_before` (the line's text before the caret): its start (a byte
/// offset in `line_before`) and its text. HTML abbreviations run back to whitespace or a `>` that closes a tag
/// (outside brackets and braces); CSS ones are the last word.
pub fn abbreviation_before(line_before: &str, syntax: EmmetSyntax) -> Option<(usize, &str)> {
    let start = match syntax {
        EmmetSyntax::Css => line_before
            .rfind(|c: char| c.is_whitespace() || c == '{' || c == ';' || c == ':')
            .map_or(0, |i| i + 1),
        EmmetSyntax::Html => {
            // After the last complete tag on the line (`<p>ul>li`); inside an unfinished tag there is none.
            let mut floor = 0;
            if let Some(lt) = line_before.rfind('<') {
                floor = lt + line_before[lt..].find('>')? + 1;
            }
            let mut depth = 0i32;
            let mut start = floor;
            for (i, c) in line_before[floor..].char_indices().rev() {
                match c {
                    ']' | '}' | ')' => depth += 1,
                    '[' | '{' | '(' => depth -= 1,
                    c if depth <= 0 && c.is_whitespace() => {
                        start = floor + i + c.len_utf8();
                        break;
                    }
                    _ => {}
                }
            }
            start
        }
    };
    let abbr = &line_before[start..];
    (!abbr.is_empty()).then_some((start, abbr))
}

/// Expand `abbr` in `syntax`; `indent` is the line's leading whitespace and `unit` one indentation step. `None` when
/// it is not an abbreviation Eludite recognizes.
pub fn expand(abbr: &str, syntax: EmmetSyntax, indent: &str, unit: &str) -> Option<Expansion> {
    match syntax {
        EmmetSyntax::Html => expand_html(abbr, indent, unit),
        EmmetSyntax::Css => expand_css(abbr),
    }
}

// ---- HTML ----

/// HTML element names the expander knows (an abbreviation must start with one, or with `.` or `#`).
const HTML_ELEMENTS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "code",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "label",
    "legend",
    "li",
    "link",
    "main",
    "meta",
    "nav",
    "noscript",
    "ol",
    "optgroup",
    "option",
    "p",
    "pre",
    "progress",
    "section",
    "select",
    "small",
    "source",
    "span",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "u",
    "ul",
    "video",
];

/// Elements without a closing tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source",
];

/// Attributes an element gets when it is written bare, as Emmet adds them.
fn default_attributes(tag: &str) -> &'static [(&'static str, &'static str)] {
    match tag {
        "a" => &[("href", "")],
        "img" => &[("src", ""), ("alt", "")],
        "input" => &[("type", "text")],
        "link" => &[("rel", "stylesheet"), ("href", "")],
        "script" => &[("src", "")],
        "form" => &[("action", "")],
        "label" => &[("for", "")],
        "button" => &[("type", "button")],
        _ => &[],
    }
}

/// The implicit element of a bare `.class` or `#id` inside `parent`.
fn implicit_tag(parent: Option<&str>) -> &'static str {
    match parent {
        Some("ul" | "ol") => "li",
        Some("table" | "tbody" | "thead" | "tfoot") => "tr",
        Some("tr") => "td",
        Some("select" | "optgroup") => "option",
        Some("dl") => "dt",
        _ => "div",
    }
}

#[derive(Debug, Clone, Default)]
struct Element {
    tag: Option<String>,
    classes: Vec<String>,
    id: Option<String>,
    attributes: Vec<(String, String)>,
    text: Option<String>,
    children: Vec<Node>,
}

#[derive(Debug, Clone)]
enum Node {
    Element(Element, u32),
    Group(Vec<Node>, u32),
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn name(&mut self) -> String {
        let start = self.at;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'$' || c == b':' {
                self.at += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.s[start..self.at]).into_owned()
    }

    fn until(&mut self, close: u8) -> Option<String> {
        let start = self.at;
        let end = self.s[start..].iter().position(|&c| c == close)? + start;
        self.at = end + 1;
        Some(String::from_utf8_lossy(&self.s[start..end]).into_owned())
    }

    fn count(&mut self) -> Option<u32> {
        if self.peek() != Some(b'*') {
            return Some(1);
        }
        self.at += 1;
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.at += 1;
        }
        let n: u32 = std::str::from_utf8(&self.s[start..self.at])
            .ok()?
            .parse()
            .ok()?;
        (1..=100).contains(&n).then_some(n)
    }

    /// One element or group with its multiplier.
    fn item(&mut self) -> Option<Node> {
        if self.peek() == Some(b'(') {
            self.at += 1;
            let nodes = self.sequence()?;
            if self.peek() != Some(b')') {
                return None;
            }
            self.at += 1;
            let n = self.count()?;
            return Some(Node::Group(nodes, n));
        }
        let mut e = Element::default();
        let tag = self.name();
        if !tag.is_empty() {
            e.tag = Some(tag);
        }
        loop {
            match self.peek() {
                Some(b'.') => {
                    self.at += 1;
                    let c = self.name();
                    if c.is_empty() {
                        return None;
                    }
                    e.classes.push(c);
                }
                Some(b'#') => {
                    self.at += 1;
                    let id = self.name();
                    if id.is_empty() {
                        return None;
                    }
                    e.id = Some(id);
                }
                Some(b'[') => {
                    self.at += 1;
                    let body = self.until(b']')?;
                    for part in body.split_whitespace() {
                        let (k, v) = part.split_once('=').unwrap_or((part, ""));
                        e.attributes
                            .push((k.to_owned(), v.trim_matches(['"', '\'']).to_owned()));
                    }
                }
                Some(b'{') => {
                    self.at += 1;
                    e.text = Some(self.until(b'}')?);
                }
                _ => break,
            }
        }
        if e.tag.is_none() && e.classes.is_empty() && e.id.is_none() {
            return None;
        }
        let n = self.count()?;
        Some(Node::Element(e, n))
    }

    /// Items joined by `+`, `>` and `^`, as a forest.
    fn sequence(&mut self) -> Option<Vec<Node>> {
        // A stack of sibling lists: `>` descends into the last item, `^` climbs back.
        let mut stack: Vec<Vec<Node>> = vec![Vec::new()];
        let first = self.item()?;
        stack.last_mut()?.push(first);
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.at += 1;
                    let item = self.item()?;
                    stack.last_mut()?.push(item);
                }
                Some(b'>') => {
                    self.at += 1;
                    let item = self.item()?;
                    stack.push(vec![item]);
                }
                Some(b'^') => {
                    let mut climbs = 0;
                    while self.peek() == Some(b'^') {
                        self.at += 1;
                        climbs += 1;
                    }
                    for _ in 0..climbs {
                        if stack.len() > 1 {
                            fold(&mut stack);
                        }
                    }
                    let item = self.item()?;
                    stack.last_mut()?.push(item);
                }
                _ => break,
            }
        }
        while stack.len() > 1 {
            fold(&mut stack);
        }
        stack.pop()
    }
}

/// Make the top sibling list the children of the last item below it.
fn fold(stack: &mut Vec<Vec<Node>>) {
    let children = stack.pop().unwrap_or_default();
    if let Some(parent) = stack.last_mut().and_then(|l| l.last_mut()) {
        match parent {
            Node::Element(e, _) => e.children.extend(children),
            Node::Group(nodes, _) => {
                // A group's children go to its last element.
                if let Some(Node::Element(e, _)) = nodes.last_mut() {
                    e.children.extend(children);
                }
            }
        }
    }
}

struct Writer<'a> {
    out: String,
    caret: Option<usize>,
    indent: &'a str,
    unit: &'a str,
}

impl Writer<'_> {
    fn mark(&mut self) {
        if self.caret.is_none() {
            self.caret = Some(self.out.len());
        }
    }

    fn nodes(
        &mut self,
        nodes: &[Node],
        parent: Option<&str>,
        depth: usize,
        number: u32,
        inline: bool,
    ) {
        for (i, node) in nodes.iter().enumerate() {
            match node {
                Node::Element(e, n) => {
                    for k in 1..=*n {
                        let num = if *n > 1 { k } else { number };
                        if !inline && (i > 0 || k > 1 || depth > 0) {
                            self.out.push('\n');
                            self.out.push_str(self.indent);
                            for _ in 0..depth {
                                self.out.push_str(self.unit);
                            }
                        }
                        self.element(e, parent, depth, num);
                    }
                }
                Node::Group(inner, n) => {
                    for k in 1..=*n {
                        let num = if *n > 1 { k } else { number };
                        if !inline && (i > 0 || k > 1 || depth > 0) {
                            self.out.push('\n');
                            self.out.push_str(self.indent);
                            for _ in 0..depth {
                                self.out.push_str(self.unit);
                            }
                        }
                        self.nodes(inner, parent, depth, num, true);
                    }
                }
            }
        }
    }

    fn element(&mut self, e: &Element, parent: Option<&str>, depth: usize, number: u32) {
        let numbered = |s: &str| s.replace('$', &number.to_string());
        let tag = e
            .tag
            .clone()
            .unwrap_or_else(|| implicit_tag(parent).to_owned());
        self.out.push('<');
        self.out.push_str(&tag);
        if let Some(id) = &e.id {
            self.out.push_str(&format!(" id=\"{}\"", numbered(id)));
        }
        if !e.classes.is_empty() {
            let classes: Vec<String> = e.classes.iter().map(|c| numbered(c)).collect();
            self.out
                .push_str(&format!(" class=\"{}\"", classes.join(" ")));
        }
        let mut attributes: Vec<(String, String)> = e.attributes.clone();
        for (k, v) in default_attributes(&tag) {
            if !attributes.iter().any(|(a, _)| a == k) {
                attributes.push(((*k).to_owned(), (*v).to_owned()));
            }
        }
        for (k, v) in &attributes {
            self.out.push_str(&format!(" {k}=\""));
            if v.is_empty() {
                self.mark();
            }
            self.out.push_str(&numbered(v));
            self.out.push('"');
        }
        self.out.push('>');
        if VOID.contains(&tag.as_str()) {
            return;
        }
        if let Some(text) = &e.text {
            self.out.push_str(&numbered(text));
        }
        if e.children.is_empty() {
            if e.text.is_none() {
                self.mark();
            }
        } else {
            self.nodes(&e.children, Some(&tag), depth + 1, number, false);
            self.out.push('\n');
            self.out.push_str(self.indent);
            for _ in 0..depth {
                self.out.push_str(self.unit);
            }
        }
        self.out.push_str(&format!("</{tag}>"));
    }
}

fn first_tag(nodes: &[Node]) -> Option<Option<&str>> {
    match nodes.first()? {
        Node::Element(e, _) => Some(e.tag.as_deref()),
        Node::Group(inner, _) => first_tag(inner),
    }
}

fn expand_html(abbr: &str, indent: &str, unit: &str) -> Option<Expansion> {
    let mut p = Parser {
        s: abbr.as_bytes(),
        at: 0,
    };
    let nodes = p.sequence()?;
    if p.at != abbr.len() {
        return None;
    }
    // A plain word is text, not an abbreviation.
    if let Some(Some(tag)) = first_tag(&nodes)
        && !HTML_ELEMENTS.contains(&tag.to_ascii_lowercase().as_str())
    {
        return None;
    }
    let mut w = Writer {
        out: String::new(),
        caret: None,
        indent,
        unit,
    };
    w.nodes(&nodes, None, 0, 1, false);
    let caret = w.caret.unwrap_or(w.out.len());
    Some(Expansion { text: w.out, caret })
}

// ---- CSS ----

/// CSS property shorthands that take a value: `m10` is `margin: 10px;`.
pub const CSS_PROPERTIES: &[(&str, &str)] = &[
    ("bd", "border"),
    ("bdrs", "border-radius"),
    ("bg", "background"),
    ("bgc", "background-color"),
    ("b", "bottom"),
    ("c", "color"),
    ("fz", "font-size"),
    ("fw", "font-weight"),
    ("gap", "gap"),
    ("h", "height"),
    ("l", "left"),
    ("lh", "line-height"),
    ("m", "margin"),
    ("mb", "margin-bottom"),
    ("ml", "margin-left"),
    ("mr", "margin-right"),
    ("mt", "margin-top"),
    ("maw", "max-width"),
    ("mah", "max-height"),
    ("miw", "min-width"),
    ("mih", "min-height"),
    ("op", "opacity"),
    ("p", "padding"),
    ("pb", "padding-bottom"),
    ("pl", "padding-left"),
    ("pr", "padding-right"),
    ("pt", "padding-top"),
    ("r", "right"),
    ("t", "top"),
    ("w", "width"),
    ("z", "z-index"),
];

/// CSS shorthands that are a whole declaration: `df` is `display: flex;`.
pub const CSS_KEYWORDS: &[(&str, &str)] = &[
    ("aic", "align-items: center"),
    ("db", "display: block"),
    ("df", "display: flex"),
    ("dg", "display: grid"),
    ("di", "display: inline"),
    ("dib", "display: inline-block"),
    ("dn", "display: none"),
    ("fwb", "font-weight: bold"),
    ("jcc", "justify-content: center"),
    ("jcsb", "justify-content: space-between"),
    ("posa", "position: absolute"),
    ("posf", "position: fixed"),
    ("posr", "position: relative"),
    ("poss", "position: sticky"),
    ("tac", "text-align: center"),
    ("tal", "text-align: left"),
    ("tar", "text-align: right"),
];

/// Properties whose bare numbers have no unit.
const UNITLESS: &[&str] = &["opacity", "z-index", "line-height", "font-weight"];

fn css_value(property: &str, raw: &str) -> Option<String> {
    if raw.starts_with('#') {
        let hex = &raw[1..];
        return (matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| raw.to_owned());
    }
    let parts: Vec<String> = raw
        .split('-')
        .filter(|s| !s.is_empty())
        .map(|v| {
            let digits = v
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(v.len());
            let (num, unit) = v.split_at(digits);
            if num.is_empty() || num.parse::<f64>().is_err() {
                return None;
            }
            let unit = match unit {
                "" if UNITLESS.contains(&property) || num == "0" => "",
                "" => "px",
                "p" => "%",
                "e" => "em",
                "r" => "rem",
                "x" => "ex",
                u if ["px", "em", "rem", "vh", "vw", "pt", "fr", "ch"].contains(&u) => u,
                _ => return None,
            };
            Some(format!("{num}{unit}"))
        })
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn expand_css(abbr: &str) -> Option<Expansion> {
    if let Some((_, decl)) = CSS_KEYWORDS.iter().find(|(k, _)| *k == abbr) {
        let text = format!("{decl};");
        return Some(Expansion {
            caret: text.len(),
            text,
        });
    }
    // The longest property shorthand the abbreviation starts with, the rest its value.
    let (short, property) = CSS_PROPERTIES
        .iter()
        .filter(|(k, _)| abbr.starts_with(k))
        .max_by_key(|(k, _)| k.len())?;
    let rest = &abbr[short.len()..];
    if rest.is_empty() {
        let text = format!("{property}: ;");
        return Some(Expansion {
            caret: text.len() - 1,
            text,
        });
    }
    let value = css_value(property, rest)?;
    let text = format!("{property}: {value};");
    Some(Expansion {
        caret: text.len(),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(abbr: &str) -> Option<String> {
        expand(abbr, EmmetSyntax::Html, "", "  ").map(|e| {
            let mut t = e.text.clone();
            t.insert(e.caret, '|');
            t
        })
    }

    fn css(abbr: &str) -> Option<String> {
        expand(abbr, EmmetSyntax::Css, "", "  ").map(|e| {
            let mut t = e.text.clone();
            t.insert(e.caret, '|');
            t
        })
    }

    #[test]
    fn html_elements_classes_ids_attributes_and_text() {
        assert_eq!(html("div").unwrap(), "<div>|</div>");
        assert_eq!(
            html("div.a#b").unwrap(),
            "<div id=\"b\" class=\"a\">|</div>"
        );
        assert_eq!(
            html(".card.wide").unwrap(),
            "<div class=\"card wide\">|</div>"
        );
        assert_eq!(html("#main").unwrap(), "<div id=\"main\">|</div>");
        assert_eq!(html("p{Hello, world}").unwrap(), "<p>Hello, world</p>|");
        assert_eq!(html("a").unwrap(), "<a href=\"|\"></a>");
        assert_eq!(
            html("a[href=/x title=\"T\"]{go}").unwrap(),
            "<a href=\"/x\" title=\"T\">go</a>|"
        );
        assert_eq!(html("img").unwrap(), "<img src=\"|\" alt=\"\">");
        assert_eq!(html("br").unwrap(), "<br>|");
        assert_eq!(html("input").unwrap(), "<input type=\"text\">|");
    }

    #[test]
    fn html_child_sibling_climb_multiply_and_groups() {
        assert_eq!(
            html("ul>li*3").unwrap(),
            "<ul>\n  <li>|</li>\n  <li></li>\n  <li></li>\n</ul>"
        );
        assert_eq!(
            html("ul>.item$*2").unwrap(),
            "<ul>\n  <li class=\"item1\">|</li>\n  <li class=\"item2\"></li>\n</ul>"
        );
        assert_eq!(html("h1+p").unwrap(), "<h1>|</h1>\n<p></p>");
        assert_eq!(
            html("div>p>span^h2").unwrap(),
            "<div>\n  <p>\n    <span>|</span>\n  </p>\n  <h2></h2>\n</div>"
        );
        assert_eq!(
            html("table>.row>.cell").unwrap(),
            "<table>\n  <tr class=\"row\">\n    <td class=\"cell\">|</td>\n  </tr>\n</table>"
        );
        assert_eq!(
            html("dl>(dt+dd)*2").unwrap(),
            "<dl>\n  <dt>|</dt><dd></dd>\n  <dt></dt><dd></dd>\n</dl>"
        );
        // Indented lines keep their indentation.
        let e = expand("ul>li", EmmetSyntax::Html, "    ", "  ").unwrap();
        assert_eq!(e.text, "<ul>\n      <li></li>\n    </ul>");
    }

    #[test]
    fn plain_words_and_broken_abbreviations_are_not_expanded() {
        assert_eq!(html("hello"), None);
        assert_eq!(html("const"), None);
        assert_eq!(html("div>"), None);
        assert_eq!(html("li*0"), None);
        assert_eq!(html("a[href"), None);
        assert_eq!(css("hello"), None);
        assert_eq!(css("m10q"), None);
    }

    #[test]
    fn css_shorthands() {
        assert_eq!(css("m10").unwrap(), "margin: 10px;|");
        assert_eq!(css("m10-20").unwrap(), "margin: 10px 20px;|");
        assert_eq!(css("w50p").unwrap(), "width: 50%;|");
        assert_eq!(css("fz1.5e").unwrap(), "font-size: 1.5em;|");
        assert_eq!(css("p0").unwrap(), "padding: 0;|");
        assert_eq!(css("mt2rem").unwrap(), "margin-top: 2rem;|");
        assert_eq!(css("c#fff").unwrap(), "color: #fff;|");
        assert_eq!(css("z10").unwrap(), "z-index: 10;|");
        assert_eq!(css("op0.5").unwrap(), "opacity: 0.5;|");
        assert_eq!(css("df").unwrap(), "display: flex;|");
        assert_eq!(css("posa").unwrap(), "position: absolute;|");
        assert_eq!(css("m").unwrap(), "margin: |;");
    }

    #[test]
    fn abbreviations_end_at_the_caret() {
        use EmmetSyntax::*;
        assert_eq!(
            abbreviation_before("    ul>li*3", Html),
            Some((4, "ul>li*3"))
        );
        assert_eq!(abbreviation_before("<p>div.a", Html), Some((3, "div.a")));
        assert_eq!(
            abbreviation_before("x p{two words}", Html),
            Some((2, "p{two words}"))
        );
        assert_eq!(
            abbreviation_before("a[href=x title=y]", Html),
            Some((0, "a[href=x title=y]"))
        );
        assert_eq!(abbreviation_before("  ", Html), None);
        assert_eq!(abbreviation_before("<a href=\"x", Html), None);
        assert_eq!(abbreviation_before(".card { m10", Css), Some((8, "m10")));
        assert_eq!(
            abbreviation_before("  color: red; df", Css),
            Some((14, "df"))
        );
    }
}
