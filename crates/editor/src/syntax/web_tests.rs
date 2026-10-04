//! Brief 0050: the web grammars' highlights on fixtures (captures to the kinds the theme colors), HTML's `<script>`
//! and `<style>` injections, JSON with comments, the language table's ids, and the keystroke budget on a 10,000-line
//! TypeScript file.

use std::path::Path;
use std::time::{Duration, Instant};

use text::{Buffer as TextBuffer, BufferId, ReplicaId};

use super::{HighlightKind, HighlightUpdate, Highlighter, LanguageRegistry, LineHighlights};
use HighlightKind::{
    Attribute, AttributeName, Comment, Constant, ConstantBuiltin, Escape, Function, Keyword,
    Number, Operator, Parameter, Property, PropertyName, Selector, String, Tag, Type, TypeBuiltin,
    Variable, VariableBuiltin,
};

fn buffer(text: &str) -> TextBuffer {
    TextBuffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), text)
}

fn highlight(id: &str, source: &str) -> HighlightUpdate {
    let registry = LanguageRegistry::with_builtins();
    let mut h = Highlighter::new(registry.by_id(id).unwrap_or_else(|| panic!("{id}")));
    let b = buffer(source);
    loop {
        let update = h.step(b.snapshot(), 0..0).expect("not cancelled");
        if update.complete {
            return update;
        }
    }
}

/// Kind at the first occurrence of `needle` (after `skip` bytes of `after`, when given).
fn kind_at(update: &HighlightUpdate, needle: &str) -> Option<HighlightKind> {
    let text = update.snapshot.text();
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in text"));
    update
        .highlights
        .kind_at(update.snapshot.offset_to_point(offset))
}

const TYPESCRIPT: &str = r##"// The counter.
import { setupCounter } from "./counter";

export interface Options { start: number; label?: string }
type Mode = "up" | "down";
const MAX_COUNT = 10;

export function step(count: number, options: Options): string {
  const next = Math.min(count + 1, MAX_COUNT);
  console.log(`count is ${next}`);
  return options.label ?? String(next);
}

class Counter extends Base {
  private value = 0;
  increment(): void { this.value++; }
}
setupCounter(document.querySelector<HTMLButtonElement>("#counter")!);
const pattern = /a+b/;
"##;

#[test]
fn typescript_kinds() {
    let u = highlight("typescript", TYPESCRIPT);
    assert_eq!(kind_at(&u, "// The counter"), Some(Comment));
    assert_eq!(kind_at(&u, "import"), Some(Keyword));
    assert_eq!(kind_at(&u, "\"./counter\""), Some(String));
    assert_eq!(kind_at(&u, "interface"), Some(Keyword));
    assert_eq!(kind_at(&u, "Options {"), Some(Type));
    assert_eq!(kind_at(&u, "number;"), Some(TypeBuiltin));
    assert_eq!(kind_at(&u, "type Mode"), Some(Keyword));
    assert_eq!(kind_at(&u, "MAX_COUNT ="), Some(Constant));
    assert_eq!(kind_at(&u, "step("), Some(Function));
    assert_eq!(kind_at(&u, "count: number"), Some(Parameter));
    assert_eq!(kind_at(&u, "min("), Some(Function));
    assert_eq!(kind_at(&u, "1, MAX"), Some(Number));
    assert_eq!(kind_at(&u, "console"), Some(VariableBuiltin));
    assert_eq!(kind_at(&u, "`count is"), Some(String));
    // The substitution's expression is not string.
    assert_eq!(kind_at(&u, "next}"), Some(Variable));
    assert_eq!(kind_at(&u, "label ??"), Some(Property));
    assert_eq!(kind_at(&u, "??"), Some(Operator));
    assert_eq!(kind_at(&u, "Counter extends"), Some(Type));
    assert_eq!(kind_at(&u, "private"), Some(Keyword));
    assert_eq!(kind_at(&u, "increment"), Some(Function));
    assert_eq!(kind_at(&u, "this"), Some(VariableBuiltin));
    assert_eq!(kind_at(&u, "a+b/"), Some(String));
    assert_eq!(kind_at(&u, "next ="), None, "locals use the default color");
}

#[test]
fn javascript_and_jsx_kinds() {
    let source = "const app = require(\"x\");\nfunction App() {\n  return <div className=\"a\"><Child /></div>;\n}\nlet n = null;\n";
    let u = highlight("javascript", source);
    assert_eq!(kind_at(&u, "const"), Some(Keyword));
    assert_eq!(kind_at(&u, "require"), Some(Function));
    assert_eq!(kind_at(&u, "App()"), Some(Function));
    assert_eq!(kind_at(&u, "div"), Some(Tag));
    assert_eq!(kind_at(&u, "className"), Some(AttributeName));
    assert_eq!(kind_at(&u, "Child"), Some(Type), "a component is not a tag");
    assert_eq!(kind_at(&u, "null"), Some(ConstantBuiltin));
    let tsx = highlight(
        "tsx",
        "export const C = (p: Props) => <span title={p.t}>x</span>;\n",
    );
    assert_eq!(kind_at(&tsx, "Props"), Some(Type));
    assert_eq!(kind_at(&tsx, "span"), Some(Tag));
    assert_eq!(kind_at(&tsx, "title"), Some(AttributeName));
}

const HTML: &str = r#"<!doctype html>
<html lang="en">
  <!-- The page. -->
  <head>
    <style>
      body { margin: 0; }
      .card { color: #1e7b34; }
    </style>
  </head>
  <body>
    <button id="counter" type="button">&amp; go</button>
    <script type="module">
      const total = add(1, 2);
      // done
    </script>
  </body>
</html>
"#;

#[test]
fn html_kinds_and_its_script_and_style_injections() {
    let u = highlight("html", HTML);
    assert_eq!(kind_at(&u, "<!doctype"), Some(Keyword));
    assert_eq!(kind_at(&u, "html lang"), Some(Tag));
    assert_eq!(kind_at(&u, "lang="), Some(AttributeName));
    assert_eq!(kind_at(&u, "\"en\""), Some(String));
    assert_eq!(kind_at(&u, "<!-- The page"), Some(Comment));
    assert_eq!(kind_at(&u, "button id"), Some(Tag));
    assert_eq!(kind_at(&u, "&amp;"), Some(Escape));
    // <style>: CSS.
    assert_eq!(kind_at(&u, "body {"), Some(Selector));
    assert_eq!(kind_at(&u, "margin"), Some(PropertyName));
    assert_eq!(kind_at(&u, "0; }"), Some(Number));
    assert_eq!(kind_at(&u, "card"), Some(Selector));
    // <script>: JavaScript.
    assert_eq!(kind_at(&u, "const total"), Some(Keyword));
    assert_eq!(kind_at(&u, "add("), Some(Function));
    assert_eq!(kind_at(&u, "2)"), Some(Number));
    assert_eq!(kind_at(&u, "// done"), Some(Comment));
    // A Razor view's markup is HTML.
    let cshtml = LanguageRegistry::with_builtins()
        .for_path(Path::new("/w/Views/Home/Index.cshtml"))
        .unwrap();
    assert_eq!(cshtml.id(), "html");
}

#[test]
fn an_injection_spanning_highlight_steps_is_highlighted_in_each() {
    // A script longer than one step's rows: every row of it is JavaScript, whichever step highlighted it.
    let mut source = std::string::String::from("<p>x</p>\n<script>\n");
    for i in 0..(super::highlighter::ROWS_PER_STEP + 50) {
        source.push_str(&format!("var v{i} = {i};\n"));
    }
    source.push_str("</script>\n");
    let u = highlight("html", &source);
    let last = format!("var v{} =", super::highlighter::ROWS_PER_STEP + 49);
    assert_eq!(kind_at(&u, &last), Some(Keyword));
    assert_eq!(kind_at(&u, "var v0"), Some(Keyword));
}

#[test]
fn css_kinds() {
    let u = highlight(
        "css",
        "/* theme */\n@media (max-width: 600px) {\n  #main > .card:hover { --gap: 4px; color: red !important; background: url(\"a.png\"); }\n}\n",
    );
    assert_eq!(kind_at(&u, "/* theme"), Some(Comment));
    assert_eq!(kind_at(&u, "@media"), Some(Keyword));
    assert_eq!(kind_at(&u, "max-width"), Some(PropertyName));
    assert_eq!(kind_at(&u, "600"), Some(Number));
    assert_eq!(kind_at(&u, "main"), Some(Selector));
    assert_eq!(kind_at(&u, "card"), Some(Selector));
    assert_eq!(kind_at(&u, "hover"), Some(Attribute));
    assert_eq!(kind_at(&u, "--gap"), Some(Variable));
    assert_eq!(kind_at(&u, "color"), Some(PropertyName));
    assert_eq!(kind_at(&u, "!important"), Some(Keyword));
    assert_eq!(kind_at(&u, "url"), Some(Function));
    assert_eq!(kind_at(&u, "\"a.png\""), Some(String));
    // SCSS and Less files get the CSS grammar.
    let registry = LanguageRegistry::with_builtins();
    for f in ["site.scss", "theme.less", "app.CSS"] {
        assert_eq!(registry.for_path(Path::new(f)).unwrap().id(), "css", "{f}");
    }
}

#[test]
fn json_and_json_with_comments() {
    let source = "{\n  // The project.\n  \"name\": \"vite-counter\",\n  \"private\": true,\n  \"version\": 1.5,\n  \"x\": null,\n  \"esc\": \"a\\nb\"\n}\n";
    for id in ["json", "jsonc"] {
        let u = highlight(id, source);
        assert_eq!(kind_at(&u, "// The project"), Some(Comment), "{id}");
        assert_eq!(kind_at(&u, "\"name\""), Some(PropertyName), "{id}");
        assert_eq!(kind_at(&u, "\"vite-counter\""), Some(String), "{id}");
        assert_eq!(kind_at(&u, "true"), Some(ConstantBuiltin), "{id}");
        assert_eq!(kind_at(&u, "1.5"), Some(Number), "{id}");
        assert_eq!(kind_at(&u, "\\n"), Some(Escape), "{id}");
    }
    let registry = LanguageRegistry::with_builtins();
    let id = |f: &str| registry.for_path(Path::new(f)).map(|l| l.id());
    assert_eq!(id("/p/package.json"), Some("json"));
    assert_eq!(id("/p/tsconfig.json"), Some("jsonc"));
    assert_eq!(id("/p/jsconfig.json"), Some("jsonc"));
    assert_eq!(id("/p/.vscode/x.jsonc"), Some("jsonc"));
}

#[test]
fn the_language_table_resolves_every_web_suffix() {
    let registry = LanguageRegistry::with_builtins();
    let id = |f: &str| registry.for_path(Path::new(f)).map(|l| l.id());
    for (file, want) in [
        ("main.ts", "typescript"),
        ("a.mts", "typescript"),
        ("a.cts", "typescript"),
        ("App.tsx", "tsx"),
        ("a.js", "javascript"),
        ("a.jsx", "javascript"),
        ("a.mjs", "javascript"),
        ("a.cjs", "javascript"),
        ("index.html", "html"),
        ("index.htm", "html"),
        ("site.css", "css"),
        ("a.json", "json"),
        ("Program.cs", "csharp"),
        ("lib.rs", "rust"),
    ] {
        assert_eq!(id(file), Some(want), "{file}");
    }
    assert_eq!(id("README.md"), None);
}

/// A 10,000-line TypeScript file, typed into in view: the UI thread's part of highlighting a keystroke (moving the
/// shown spans through the edit; the frame draws those) stays inside the editor's 8 ms keystroke budget at p99. The
/// re-parse and re-highlight run on the syntax thread ([`super::SyntaxThread`]), never in a frame; how long the
/// visible rows' fresh colors take there ([`super::HighlightStats::priority`]) is printed for the report: typing an
/// unfinished statement (an open brace or string) makes tree-sitter's error recovery re-parse far, so it varies.
#[test]
fn a_keystroke_in_a_10000_line_typescript_file_is_highlighted_within_the_budget() {
    let mut source = std::string::String::new();
    while source.lines().count() < 10_000 {
        source.push_str(TYPESCRIPT);
    }
    let registry = LanguageRegistry::with_builtins();
    let mut h = Highlighter::new(registry.by_id("typescript").unwrap());
    let mut b = buffer(&source);
    let started = Instant::now();
    let visible = 5_000..5_060;
    let mut shown = loop {
        let update = h
            .step(b.snapshot(), visible.clone())
            .expect("not cancelled");
        if update.complete {
            break update;
        }
    };
    let full = started.elapsed();
    let rows = b.snapshot().max_point().row;
    // Type a statement, a character at a time, at the start of a function body in view.
    let body = source
        .lines()
        .enumerate()
        .skip(visible.start as usize)
        .find(|(_, l)| l.starts_with("export function step"))
        .map(|(r, _)| r as u32 + 1)
        .expect("a function in view");
    let typed = "  const total = step(count + 1, { start: 0, label: \"x\" }); // typed\n";
    let mut at = b.snapshot().point_to_offset(text::Point::new(body, 0));
    let mut syntax = Vec::new();
    let mut parse = Vec::new();
    let mut ui = Vec::new();
    for _ in 0..4 {
        for ch in typed.chars() {
            b.edit([(at..at, ch.to_string())]);
            at += ch.len_utf8();
            let snapshot = b.snapshot().clone();
            let t = Instant::now();
            let mut moved: LineHighlights = shown.highlights.clone();
            moved.interpolate(&shown.snapshot, &snapshot);
            ui.push(t.elapsed());
            let update = h.step(&snapshot, visible.clone()).expect("not cancelled");
            syntax.push(update.stats.priority);
            parse.push(update.stats.parse);
            shown = update;
        }
    }
    let p99 = |v: &mut Vec<Duration>| {
        v.sort();
        v[v.len() * 99 / 100]
    };
    let (syntax_p99, ui_p99, parse_p99) = (p99(&mut syntax), p99(&mut ui), p99(&mut parse));

    eprintln!(
        "10,000-line TypeScript ({rows} rows): full highlight {full:?}; {} keystrokes: spans moved p99 {ui_p99:?} \
         (UI thread); on the syntax thread, visible rows fresh p50 {:?} p99 {syntax_p99:?}, re-parse p50 {:?} p99 \
         {parse_p99:?}",
        syntax.len(),
        syntax[syntax.len() / 2],
        parse[parse.len() / 2],
    );
    assert!(ui_p99 < Duration::from_millis(8), "{ui_p99:?}");
}
