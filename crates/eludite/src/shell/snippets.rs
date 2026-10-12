//! Code snippets (brief 0069): Visual Studio's `.snippet` files as LSP snippets, the built-in C# and Visual Basic
//! set (`crates/eludite/snippets/`, embedded) and the person's own (`editor.snippets.paths`), Tab-Tab, snippet
//! entries in the completion list, and the Insert Snippet (Ctrl+K, Ctrl+X) and Surround With (Ctrl+K, Ctrl+S)
//! pickers.
//!
//! - **The reader** maps a snippet's `Literal` and `Object` declarations to tab stops in the order they first appear
//!   in its code (a repeated one is linked), `$end$` to `$0`, `$selected$` to the selection and `$$` to `$`;
//!   the functions `ClassName()` (the type declared last before the caret) and `SimpleTypeName(global::A.B)` (`B`)
//!   give a literal its text. `Imports`, `References` and `Kind` are read and ignored.
//! - **The library** is the built-in set and the user's files (read off the UI thread when the setting changes); a
//!   user snippet replaces a built-in one with the same shortcut and language.
//! - **The pickers** list the document language's snippets (Surround With: the `SurroundsWith` ones) in a popup at
//!   the caret; typing filters, Up and Down select, Enter inserts through `eludite.editor.insert_snippet` or
//!   `surround_with`, Escape closes.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use eludite_ui::{Theme, menu_row};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, Point, Render, StatefulInteractiveElement, Styled, Window,
    anchored, deferred, div, px,
};

/// The built-in snippets' files (MIT, written for Eludite in Visual Studio's format).
const BUILT_IN: &[(&str, &str)] = &[
    ("csharp", include_str!("../../snippets/csharp/prop.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/propfull.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/propg.snippet"),
    ),
    ("csharp", include_str!("../../snippets/csharp/ctor.snippet")),
    ("csharp", include_str!("../../snippets/csharp/cw.snippet")),
    ("csharp", include_str!("../../snippets/csharp/for.snippet")),
    ("csharp", include_str!("../../snippets/csharp/forr.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/foreach.snippet"),
    ),
    ("csharp", include_str!("../../snippets/csharp/if.snippet")),
    ("csharp", include_str!("../../snippets/csharp/else.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/while.snippet"),
    ),
    ("csharp", include_str!("../../snippets/csharp/do.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/switch.snippet"),
    ),
    ("csharp", include_str!("../../snippets/csharp/try.snippet")),
    ("csharp", include_str!("../../snippets/csharp/tryf.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/using.snippet"),
    ),
    ("csharp", include_str!("../../snippets/csharp/lock.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/class.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/interface.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/struct.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/record.snippet"),
    ),
    ("csharp", include_str!("../../snippets/csharp/enum.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/namespace.snippet"),
    ),
    ("csharp", include_str!("../../snippets/csharp/svm.snippet")),
    ("csharp", include_str!("../../snippets/csharp/sim.snippet")),
    (
        "csharp",
        include_str!("../../snippets/csharp/equals.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/exception.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/attribute.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/indexer.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/iterator.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/testx.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/testn.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/testm.snippet"),
    ),
    (
        "csharp",
        include_str!("../../snippets/csharp/region.snippet"),
    ),
    ("vb", include_str!("../../snippets/vb/Property.snippet")),
    ("vb", include_str!("../../snippets/vb/If.snippet")),
    ("vb", include_str!("../../snippets/vb/For.snippet")),
    ("vb", include_str!("../../snippets/vb/ForEach.snippet")),
    ("vb", include_str!("../../snippets/vb/Try.snippet")),
    ("vb", include_str!("../../snippets/vb/Using.snippet")),
    ("vb", include_str!("../../snippets/vb/Select.snippet")),
    ("vb", include_str!("../../snippets/vb/Sub.snippet")),
    ("vb", include_str!("../../snippets/vb/Function.snippet")),
    ("vb", include_str!("../../snippets/vb/Class.snippet")),
    ("vb", include_str!("../../snippets/vb/While.snippet")),
];

/// What a snippet is for (Visual Studio's `SnippetType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetKind {
    Expansion,
    SurroundsWith,
}

impl SnippetKind {
    pub fn name(self) -> &'static str {
        match self {
            SnippetKind::Expansion => "expansion",
            SnippetKind::SurroundsWith => "surroundsWith",
        }
    }
}

/// One declared literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal {
    pub id: String,
    pub default: String,
    pub function: Option<String>,
    pub editable: bool,
}

/// One code snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    pub shortcut: String,
    pub title: String,
    pub description: String,
    /// `csharp` or `vb`.
    pub language: &'static str,
    pub kinds: Vec<SnippetKind>,
    /// `builtIn`, or the file it was read from.
    pub source: String,
    pub code: String,
    pub delimiter: char,
    pub literals: Vec<Literal>,
}

/// The language a `Code Language` attribute names (`csharp`, `VB`), as Eludite's ids.
pub fn language_of_attribute(attribute: &str) -> Option<&'static str> {
    match attribute.to_ascii_lowercase().as_str() {
        "csharp" | "c#" => Some("csharp"),
        "vb" | "visualbasic" => Some("vb"),
        _ => None,
    }
}

/// The snippet language of a document (`.cs`, `.vb`).
pub fn language_of_path(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "cs" | "csx" => Some("csharp"),
        "vb" => Some("vb"),
        _ => None,
    }
}

/// Read a `.snippet` file's snippets (one `CodeSnippets` root with `CodeSnippet` elements, or one `CodeSnippet`).
pub fn read_snippets(xml: &str, source: &str) -> Result<Vec<Snippet>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
        n.children()
            .find(|c| c.is_element() && c.tag_name().name() == name)
    }
    let text = |n: Option<roxmltree::Node<'_, '_>>| {
        n.map(|n| n.text().unwrap_or_default().trim().to_owned())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for snippet in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "CodeSnippet")
    {
        let header = child(snippet, "Header");
        let body = child(snippet, "Snippet").ok_or("a CodeSnippet has no Snippet element")?;
        let code = child(body, "Code").ok_or("a Snippet has no Code element")?;
        let Some(language) = code.attribute("Language").and_then(language_of_attribute) else {
            continue;
        };
        let shortcut = text(header.and_then(|h| child(h, "Shortcut")));
        if shortcut.is_empty() {
            continue;
        }
        let kinds: Vec<SnippetKind> = header
            .and_then(|h| child(h, "SnippetTypes"))
            .map(|t| {
                t.children()
                    .filter(|c| c.is_element())
                    .filter_map(|c| match c.text().unwrap_or_default().trim() {
                        "Expansion" => Some(SnippetKind::Expansion),
                        "SurroundsWith" => Some(SnippetKind::SurroundsWith),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let literals = child(body, "Declarations")
            .map(|d| {
                d.children()
                    .filter(|c| {
                        c.is_element() && matches!(c.tag_name().name(), "Literal" | "Object")
                    })
                    .map(|l| Literal {
                        id: text(child(l, "ID")),
                        default: text(child(l, "Default")),
                        function: Some(text(child(l, "Function"))).filter(|f| !f.is_empty()),
                        editable: l.attribute("Editable") != Some("false"),
                    })
                    .filter(|l| !l.id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        out.push(Snippet {
            title: Some(text(header.and_then(|h| child(h, "Title"))))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| shortcut.clone()),
            description: text(header.and_then(|h| child(h, "Description"))),
            shortcut,
            language,
            kinds: if kinds.is_empty() {
                vec![SnippetKind::Expansion]
            } else {
                kinds
            },
            source: source.to_owned(),
            code: code.text().unwrap_or_default().to_owned(),
            delimiter: code
                .attribute("Delimiter")
                .and_then(|d| d.chars().next())
                .unwrap_or('$'),
            literals,
        });
    }
    Ok(out)
}

/// Escape text for LSP snippet syntax.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '$' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The type declared last in `text_before` (`class`, `struct`, `record`, `interface`; VB's `Class`, `Structure`).
pub fn enclosing_type(text_before: &str) -> Option<String> {
    let words: Vec<&str> = text_before
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    words.windows(2).rev().find_map(|w| {
        matches!(
            w[0],
            "class" | "struct" | "record" | "interface" | "Class" | "Structure" | "Module"
        )
        .then(|| w[1].to_owned())
    })
}

impl Snippet {
    pub fn is(&self, kind: SnippetKind) -> bool {
        self.kinds.contains(&kind)
    }

    /// The snippet as LSP snippet text; `text_before` is the document before the caret (for `ClassName()`).
    pub fn to_lsp(&self, text_before: &str) -> String {
        let d = self.delimiter;
        let mut out = String::with_capacity(self.code.len() + 16);
        let mut stops: Vec<String> = Vec::new();
        let mut rest = self.code.as_str();
        while let Some(start) = rest.find(d) {
            out.push_str(&escape(&rest[..start]));
            let after = &rest[start + d.len_utf8()..];
            let Some(end) = after.find(d) else {
                out.push_str(&escape(&rest[start..]));
                rest = "";
                break;
            };
            let id = &after[..end];
            rest = &after[end + d.len_utf8()..];
            match id {
                "" => out.push_str(&escape(&d.to_string())),
                "end" => out.push_str("$0"),
                "selected" => out.push_str("${TM_SELECTED_TEXT}"),
                _ => match self.literals.iter().find(|l| l.id == id) {
                    Some(l) => {
                        let value = match l.function.as_deref() {
                            Some(f) if f.starts_with("ClassName") => {
                                enclosing_type(text_before).unwrap_or_else(|| l.default.clone())
                            }
                            Some(f) if f.starts_with("SimpleTypeName") => f
                                .trim_start_matches("SimpleTypeName(")
                                .trim_end_matches(')')
                                .rsplit(['.', ':'])
                                .next()
                                .unwrap_or(&l.default)
                                .to_owned(),
                            _ => l.default.clone(),
                        };
                        let fixed = !l.editable
                            || l.function
                                .as_deref()
                                .is_some_and(|f| f.starts_with("ClassName"));
                        if fixed {
                            out.push_str(&escape(&value));
                        } else if let Some(n) = stops.iter().position(|s| s == id) {
                            out.push_str(&format!("${}", n + 1));
                        } else {
                            stops.push(id.to_owned());
                            out.push_str(&format!("${{{}:{}}}", stops.len(), escape(&value)));
                        }
                    }
                    // An undeclared id is text, as Visual Studio leaves it.
                    None => out.push_str(&escape(&format!("{d}{id}{d}"))),
                },
            }
        }
        out.push_str(&escape(rest));
        out
    }
}

/// The built-in snippets, read once.
pub fn built_in() -> &'static [Snippet] {
    static SET: OnceLock<Vec<Snippet>> = OnceLock::new();
    SET.get_or_init(|| {
        BUILT_IN
            .iter()
            .flat_map(|(_, xml)| read_snippets(xml, "builtIn").unwrap_or_default())
            .collect()
    })
}

/// Every `*.snippet` under the `;`-separated folders (read on a background thread), with the problems found.
pub fn read_user_snippets(paths: &str) -> (Vec<Snippet>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = paths
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .collect();
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            problems.push(format!("{}: not a readable folder", dir.display()));
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            seen += 1;
            if seen > 10_000 {
                return (out, problems);
            }
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("snippet"))
            {
                match std::fs::read_to_string(&p)
                    .map_err(|e| e.to_string())
                    .and_then(|t| {
                        read_snippets(t.trim_start_matches('\u{feff}'), &p.display().to_string())
                    }) {
                    Ok(s) => out.extend(s),
                    Err(e) => problems.push(format!("{}: {e}", p.display())),
                }
            }
        }
    }
    (out, problems)
}

/// The built-in snippets and the user's, user first.
#[derive(Debug, Clone, Default)]
pub struct Library {
    user: Arc<Vec<Snippet>>,
}

impl Library {
    pub fn with_user(user: Vec<Snippet>) -> Self {
        Self {
            user: Arc::new(user),
        }
    }

    /// The snippets of `language` (every language when `None`), a user one hiding a built-in one of its shortcut.
    pub fn list(&self, language: Option<&str>) -> Vec<&Snippet> {
        let mut out: Vec<&Snippet> = Vec::new();
        for s in self.user.iter().chain(built_in()) {
            if language.is_some_and(|l| l != s.language) {
                continue;
            }
            if !out
                .iter()
                .any(|o| o.language == s.language && o.shortcut == s.shortcut)
            {
                out.push(s);
            }
        }
        out.sort_by(|a, b| {
            a.shortcut
                .to_lowercase()
                .cmp(&b.shortcut.to_lowercase())
                .then(a.language.cmp(b.language))
        });
        out
    }

    /// The snippet with `shortcut` in `language` (Visual Basic's shortcuts ignore case).
    pub fn find(&self, language: &str, shortcut: &str) -> Option<&Snippet> {
        self.user.iter().chain(built_in()).find(|s| {
            s.language == language
                && if language == "vb" {
                    s.shortcut.eq_ignore_ascii_case(shortcut)
                } else {
                    s.shortcut == shortcut
                }
        })
    }
}

// ----- the picker -----

pub const SNIPPET_PICKER: &str = "snippet-picker";

/// The Insert Snippet or Surround With picker.
pub struct SnippetPicker {
    theme: Theme,
    /// (shortcut, title)
    items: Vec<(String, String)>,
    filter: String,
    selected: usize,
    anchor: Point<Pixels>,
    focus: FocusHandle,
    pub surround: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEvent {
    Chosen(String),
    Dismissed,
}

impl SnippetPicker {
    pub fn new(
        theme: Theme,
        items: Vec<(String, String)>,
        surround: bool,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme,
            items,
            filter: String::new(),
            selected: 0,
            anchor,
            focus: cx.focus_handle(),
            surround,
        }
    }

    /// The rows shown: the items whose shortcut or title contains the filter (ignoring case), shortcut prefixes
    /// first.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        let mut rows: Vec<usize> = (0..self.items.len())
            .filter(|&i| {
                let (s, t) = &self.items[i];
                s.to_lowercase().contains(&f) || t.to_lowercase().contains(&f)
            })
            .collect();
        rows.sort_by_key(|&i| !self.items[i].0.to_lowercase().starts_with(&f));
        rows
    }

    pub fn selected_shortcut(&self) -> Option<String> {
        self.visible()
            .get(self.selected)
            .map(|&i| self.items[i].0.clone())
    }

    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.filter.push_str(text);
        self.selected = 0;
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        let n = self.visible().len();
        match k.key.as_str() {
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            "enter" | "tab" => match self.selected_shortcut() {
                Some(s) => cx.emit(PickerEvent::Chosen(s)),
                None => cx.emit(PickerEvent::Dismissed),
            },
            "escape" => cx.emit(PickerEvent::Dismissed),
            "backspace" => {
                self.filter.pop();
                self.selected = 0;
            }
            _ => match k.key_char.as_deref() {
                Some(t) if !t.is_empty() && !t.chars().any(char::is_control) => {
                    self.filter.push_str(t);
                    self.selected = 0;
                }
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl EventEmitter<PickerEvent> for SnippetPicker {}

impl Focusable for SnippetPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SnippetPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let heading = if self.surround {
            "Surround With: "
        } else {
            "Insert Snippet: "
        };
        let mut children = vec![
            div()
                .px_2()
                .py_1()
                .text_color(t.text_muted)
                .child(format!("{heading}{}", self.filter))
                .into_any_element(),
        ];
        for (row, i) in self.visible().into_iter().take(40).enumerate() {
            let (shortcut, title) = &self.items[i];
            let label = format!("{shortcut}  {title}");
            let chosen = shortcut.clone();
            children.push(
                menu_row(
                    format!("snippet-{shortcut}"),
                    label,
                    0,
                    row == self.selected,
                    false,
                    false,
                    &t,
                )
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(PickerEvent::Chosen(chosen.clone()))),
                )
                .into_any_element(),
            );
        }
        let panel = eludite_ui::popup::popup_panel(&t)
            .id(SNIPPET_PICKER)
            .debug_selector(|| SNIPPET_PICKER.into())
            .track_focus(&self.focus)
            .key_context("SnippetPicker")
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Dismissed)))
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(280.))
            .max_w(px(600.))
            .py_1()
            .children(children);
        deferred(
            anchored()
                .position(self.anchor)
                .snap_to_window_with_margin(px(4.))
                .child(panel),
        )
        .with_priority(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_snippet_reads_and_the_brief_set_is_there() {
        assert_eq!(built_in().len(), BUILT_IN.len());
        let lib = Library::default();
        for s in [
            "prop",
            "propfull",
            "propg",
            "ctor",
            "cw",
            "for",
            "forr",
            "foreach",
            "if",
            "else",
            "while",
            "do",
            "switch",
            "try",
            "tryf",
            "using",
            "lock",
            "class",
            "interface",
            "struct",
            "record",
            "enum",
            "namespace",
            "svm",
            "sim",
            "equals",
            "exception",
            "attribute",
            "indexer",
            "iterator",
            "testx",
            "testn",
            "testm",
        ] {
            assert!(lib.find("csharp", s).is_some(), "{s}");
        }
        assert!(lib.find("vb", "property").is_some(), "VB ignores case");
        assert!(lib.find("csharp", "Prop").is_none());
        // Every snippet's LSP text parses into stops that end at $0.
        for s in built_in() {
            let lsp = s.to_lsp("class Foo {");
            let parsed = eludite_editor::snippet_session::parse(&lsp, &|_| Some(String::new()));
            assert!(
                !parsed.text.contains('$') || s.shortcut == "#region",
                "{}: {}",
                s.shortcut,
                parsed.text
            );
        }
    }

    #[test]
    fn literals_become_linked_stops_and_functions_fill_in() {
        let lib = Library::default();
        assert_eq!(
            lib.find("csharp", "propfull").unwrap().to_lsp(""),
            "private ${1:int} ${2:myVar};\n\npublic $1 ${3:MyProperty}\n{\n    get { return $2; }\n    set { $2 = value; }\n}\n$0"
        );
        assert_eq!(
            lib.find("csharp", "ctor")
                .unwrap()
                .to_lsp("namespace N;\npublic sealed class Customer\n{\n    "),
            "public Customer()\n{\n    $0\n}"
        );
        assert_eq!(
            lib.find("csharp", "if").unwrap().to_lsp(""),
            "if (${1:true})\n{\n    ${TM_SELECTED_TEXT}$0\n}"
        );
        assert!(
            lib.find("csharp", "try")
                .unwrap()
                .to_lsp("")
                .contains("${1:Exception}")
        );
        assert!(
            lib.find("csharp", "if")
                .unwrap()
                .is(SnippetKind::SurroundsWith)
        );
        assert!(
            !lib.find("csharp", "prop")
                .unwrap()
                .is(SnippetKind::SurroundsWith)
        );
    }

    #[test]
    fn user_snippets_override_built_ins_and_bad_files_are_reported() {
        let dir =
            std::env::temp_dir().join(format!("eludite-0069-snippets-{}", std::process::id()));
        let nested = dir.join("more");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("cw.snippet"),
            r#"<CodeSnippets xmlns="http://schemas.microsoft.com/VisualStudio/2005/CodeSnippet"><CodeSnippet Format="1.0.0"><Header><Shortcut>cw</Shortcut><Title>Mine</Title></Header><Snippet><Code Language="CSharp"><![CDATA[Debug.WriteLine($end$); // $$]]></Code></Snippet></CodeSnippet></CodeSnippets>"#,
        )
        .unwrap();
        std::fs::write(dir.join("bad.snippet"), "<not xml").unwrap();
        let (user, problems) = read_user_snippets(&format!("{};/no/such/folder", dir.display()));
        assert_eq!(user.len(), 1);
        assert_eq!(problems.len(), 2, "{problems:?}");
        let lib = Library::with_user(user);
        let cw = lib.find("csharp", "cw").unwrap();
        assert_eq!(cw.title, "Mine");
        assert_eq!(cw.to_lsp(""), "Debug.WriteLine($0); // \\$");
        assert_eq!(
            lib.list(Some("csharp"))
                .iter()
                .filter(|s| s.shortcut == "cw")
                .count(),
            1
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_enclosing_type_is_the_last_declared() {
        assert_eq!(enclosing_type("class A { } struct B {"), Some("B".into()));
        assert_eq!(
            enclosing_type("Public Class Widget\n"),
            Some("Widget".into())
        );
        assert_eq!(enclosing_type("var x = 1;"), None);
    }
}
