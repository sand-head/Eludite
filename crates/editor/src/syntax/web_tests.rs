//! Brief 0050: the web grammars' highlights on fixtures (captures to the kinds the theme colors), HTML's `<script>`
//! and `<style>` injections, JSON with comments, the language table's ids, and the keystroke budget on a 10,000-line
//! TypeScript file. Brief 0056: Razor (`.razor` and `.cshtml`) on a Blazor component, its markup, directives and C#,
//! its `<script>` and `<style>` injections, and an edit in its `@code` block.

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
        ("Counter.razor", "razor"),
        ("Index.cshtml", "razor"),
        ("site.css", "css"),
        ("a.json", "json"),
        ("Program.cs", "csharp"),
        ("lib.rs", "rust"),
    ] {
        assert_eq!(id(file), Some(want), "{file}");
    }
    assert_eq!(id("README.md"), None);
}

/// A Blazor component with what the brief names: directives, a Razor comment, elements and components with plain and
/// directive attributes, an entity, an implicit expression, `@if` with markup in its body, `<style>`, `<script>` and an
/// `@code` block.
const COUNTER_RAZOR: &str = r#"@page "/counter"
@rendermode InteractiveServer
@using MudBlazor
@inject ILogger<Counter> Logger

<PageTitle>Counter</PageTitle>

@* The counter. *@
<h1 class="title">Counter &amp; more</h1>

<p role="status">Current count: @currentCount</p>

<MudButton Color="Color.Primary" @onclick="IncrementCount">Click me</MudButton>
<MudSwitch @bind-Value="_dense" @bind-Value:after="Save" @key="_dense" @ref="_switch" />

@if (currentCount > 3)
{
    <p>Many clicks</p>
}

<style>
    .title { color: red; }
</style>

<script>
    function go() { const n = 1; return n; }
</script>

@code {
    private int currentCount = 0;
    private bool _dense;
    private MudSwitch<bool> _switch = default!;

    private async Task IncrementCount()
    {
        currentCount++;
        await Task.Delay(1);
    }
}
"#;

/// The Razor grammar's tree of `source` has no `ERROR` or `MISSING` node.
fn assert_parses_cleanly(source: &str) {
    let razor = LanguageRegistry::with_builtins().by_id("razor").unwrap();
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(razor.grammar()).unwrap();
    let tree = parser.parse(source, None).unwrap();
    assert!(
        !tree.root_node().has_error(),
        "{}",
        tree.root_node().to_sexp()
    );
}

#[test]
fn razor_resolves_razor_and_cshtml_and_html_stays_html() {
    let registry = LanguageRegistry::with_builtins();
    let id = |f: &str| registry.for_path(Path::new(f)).map(|l| l.id());
    assert_eq!(id("/w/Components/Pages/Counter.razor"), Some("razor"));
    assert_eq!(id("/w/Views/Home/Index.cshtml"), Some("razor"));
    assert_eq!(id("/w/Pages/Shared/_Layout.CSHTML"), Some("razor"));
    assert_eq!(id("/w/wwwroot/index.html"), Some("html"));
    assert_eq!(id("/w/wwwroot/index.htm"), Some("html"));
    let razor = registry.by_id("razor").unwrap();
    assert_eq!(razor.name(), "Razor");
    assert_eq!(
        razor.config().emmet,
        Some(crate::intellisense::emmet::EmmetSyntax::Html)
    );
}

#[test]
fn razor_kinds_in_a_blazor_component() {
    assert_parses_cleanly(COUNTER_RAZOR);
    let u = highlight("razor", COUNTER_RAZOR);
    // Directives, transitions and control structures: `@` and its keyword together.
    assert_eq!(kind_at(&u, "@page"), Some(Keyword));
    assert_eq!(kind_at(&u, "page \""), Some(Keyword));
    assert_eq!(kind_at(&u, "\"/counter\""), Some(String));
    assert_eq!(kind_at(&u, "@rendermode"), Some(Keyword));
    assert_eq!(kind_at(&u, "InteractiveServer"), Some(Type));
    assert_eq!(kind_at(&u, "@using"), Some(Keyword));
    assert_eq!(kind_at(&u, "@inject"), Some(Keyword));
    assert_eq!(kind_at(&u, "@if"), Some(Keyword));
    assert_eq!(kind_at(&u, "if ("), Some(Keyword));
    assert_eq!(kind_at(&u, "@code"), Some(Keyword));
    assert_eq!(kind_at(&u, "code {"), Some(Keyword));
    assert_eq!(kind_at(&u, "@currentCount"), Some(Keyword));
    // A Razor comment.
    assert_eq!(kind_at(&u, "@* The"), Some(Comment));
    assert_eq!(kind_at(&u, "counter. *@"), Some(Comment));
    // Markup: tags (components too), attribute names, values and entities; text is the default color.
    assert_eq!(kind_at(&u, "h1 class"), Some(Tag));
    assert_eq!(kind_at(&u, "h1>"), Some(Tag));
    assert_eq!(kind_at(&u, "PageTitle>Counter"), Some(Tag));
    assert_eq!(kind_at(&u, "MudButton Color"), Some(Tag));
    assert_eq!(kind_at(&u, "MudSwitch"), Some(Tag));
    assert_eq!(kind_at(&u, "class="), Some(AttributeName));
    assert_eq!(kind_at(&u, "Color="), Some(AttributeName));
    assert_eq!(kind_at(&u, "title\">"), Some(String));
    assert_eq!(kind_at(&u, "\"title\""), Some(String), "the quotes too");
    assert_eq!(kind_at(&u, "status"), Some(String));
    assert_eq!(kind_at(&u, "&amp;"), Some(Escape));
    assert_eq!(kind_at(&u, "Current count"), None);
    assert_eq!(kind_at(&u, "Click me"), None);
    // Directive attributes, the `@` included.
    assert_eq!(kind_at(&u, "@onclick"), Some(AttributeName));
    assert_eq!(kind_at(&u, "onclick"), Some(AttributeName));
    assert_eq!(kind_at(&u, "@bind-Value"), Some(AttributeName));
    assert_eq!(kind_at(&u, "-Value"), Some(AttributeName));
    assert_eq!(kind_at(&u, "@key"), Some(AttributeName));
    assert_eq!(kind_at(&u, ":after"), Some(AttributeName));
    // `ref` itself is the C# keyword's anonymous node in the grammar, which the C# query (first) captures.
    assert_eq!(kind_at(&u, "@ref"), Some(AttributeName));
    // `@code` is C#, by the C# query.
    assert_eq!(kind_at(&u, "private int"), Some(Keyword));
    assert_eq!(kind_at(&u, "int currentCount"), Some(TypeBuiltin));
    assert_eq!(kind_at(&u, "0;"), Some(Number));
    assert_eq!(kind_at(&u, "async"), Some(Keyword));
    assert_eq!(kind_at(&u, "Task IncrementCount"), Some(Type));
    assert_eq!(kind_at(&u, "IncrementCount()"), Some(Function));
    assert_eq!(kind_at(&u, "await"), Some(Keyword));
    assert_eq!(kind_at(&u, "Delay"), Some(Function));
}

#[test]
fn a_razor_render_mode_is_a_type_by_name_and_csharp_as_an_expression() {
    let source = "@rendermode RenderMode.InteractiveWebAssembly\n<p>x</p>\n";
    assert_parses_cleanly(source);
    let u = highlight("razor", source);
    assert_eq!(kind_at(&u, "@rendermode"), Some(Keyword));
    assert_eq!(kind_at(&u, "RenderMode."), Some(Type));
    let source = "@rendermode @(new InteractiveServerRenderMode(prerender: false))\n<p>x</p>\n";
    assert_parses_cleanly(source);
    let u = highlight("razor", source);
    assert_eq!(kind_at(&u, "@("), Some(Keyword));
    assert_eq!(kind_at(&u, "(new"), None, "C#, not a type");
    assert_eq!(kind_at(&u, "new "), Some(Keyword));
    assert_eq!(kind_at(&u, "InteractiveServerRenderMode"), Some(Type));
    assert_eq!(kind_at(&u, "false"), Some(ConstantBuiltin));
}

#[test]
fn razor_script_and_style_bodies_are_javascript_and_css() {
    let u = highlight("razor", COUNTER_RAZOR);
    // <style>: CSS.
    assert_eq!(kind_at(&u, "title {"), Some(Selector));
    assert_eq!(kind_at(&u, "color: red"), Some(PropertyName));
    // <script>: JavaScript.
    assert_eq!(kind_at(&u, "function go"), Some(Keyword));
    assert_eq!(kind_at(&u, "go()"), Some(Function));
    assert_eq!(kind_at(&u, "const n"), Some(Keyword));
    assert_eq!(kind_at(&u, "1; return"), Some(Number));
    assert_eq!(kind_at(&u, "return n"), Some(Keyword));
    // The tags around them stay markup.
    assert_eq!(kind_at(&u, "style>\n"), Some(Tag));
    assert_eq!(kind_at(&u, "script>\n"), Some(Tag));
}

/// Typing a member into the `@code` block re-parses incrementally (the old tree reused), leaves no error node, and
/// highlights the same as a fresh parse of the new text.
#[test]
fn an_edit_in_a_razor_code_block_re_highlights_without_errors() {
    let registry = LanguageRegistry::with_builtins();
    let razor = registry.by_id("razor").unwrap();
    let mut h = Highlighter::new(razor.clone());
    let mut b = buffer(COUNTER_RAZOR);
    let step_all = |h: &mut Highlighter, b: &TextBuffer| loop {
        let update = h.step(b.snapshot(), 0..0).expect("not cancelled");
        if update.complete {
            break update;
        }
    };
    let first = step_all(&mut h, &b);
    assert!(first.stats.full_parse);
    let at = COUNTER_RAZOR.find("    private bool _dense;").unwrap();
    let typed = "    private string _label = \"Clicks\";\n";
    for (i, ch) in typed.char_indices() {
        b.edit([(at + i..at + i, ch.to_string())]);
        step_all(&mut h, &b);
    }
    let edited = step_all(&mut h, &b);
    assert!(!edited.stats.full_parse, "re-parse reuses the old tree");
    let text = b.snapshot().text();
    assert_parses_cleanly(&text);
    assert_eq!(kind_at(&edited, "string _label"), Some(TypeBuiltin));
    assert_eq!(kind_at(&edited, "\"Clicks\""), Some(String));
    assert_eq!(kind_at(&edited, "private bool"), Some(Keyword));
    assert_eq!(kind_at(&edited, "@code"), Some(Keyword));
    assert_eq!(kind_at(&edited, "function go"), Some(Keyword));
    let fresh = highlight("razor", &text);
    for row in 0..fresh.highlights.row_count() {
        assert_eq!(
            edited.highlights.spans(row),
            fresh.highlights.spans(row),
            "row {row}"
        );
    }
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
    super::assert_budget(
        "spans moved p99 on the UI thread",
        ui_p99,
        Duration::from_millis(8),
    );
}
