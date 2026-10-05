# tree-sitter-razor

The tree-sitter grammar for Razor (`.razor` Blazor components, `.cshtml` Razor Pages and MVC views): Razor's
directives, transitions, expressions and control structures over tree-sitter-c-sharp's C#, with HTML markup. It is the
owner's library, brought into this repository by brief 0056 so the editor highlights Razor at once and so fixes land
here with their tests.

License: MIT (`LICENSE`; the upstream author's copyright line is kept). It is not part of the GPL product's licensing
question: the grammar is linked into the shell like every other MIT grammar.

## Provenance

- Source: `https://git.sand.town/sand_head/tree-sitter-razor` at commit `bed3116ffead1dd67a9c702d5f85e74c429694cd`
  (2026), the owner's fork of [tris203/tree-sitter-razor](https://github.com/tris203/tree-sitter-razor) (MIT, Tristan
  Knight).
- `vendor/tree-sitter-c-sharp/grammar.js` is tree-sitter-c-sharp 0.23.5's grammar definition, copied verbatim (its
  `package.json` there says so) so `tree-sitter generate` needs no npm install. It is the same version the editor's
  `tree-sitter-c-sharp` crate is, so the C# node names match and the editor's C# highlight query applies to Razor's
  code regions unchanged. Bump both together.
- Dropped from the library's tree: the Node, Python, Go, Swift and C bindings, `package.json`, `binding.gyp`,
  `CMakeLists.txt`, `Makefile`, `Package.swift`, `pyproject.toml`, `setup.py`. The Rust binding is `src/lib.rs` and
  `build.rs`. Kept: `grammar.js`, `queries/` (the library's own queries, for reference and for `tree-sitter test`'s
  highlight assertions under `test/highlight/`; the editor's queries are `crates/editor/queries/razor/`),
  `test/corpus/`, `tree-sitter.json`, `src/` (generated).

## Layout

| Path | What |
|---|---|
| `grammar.js` | The grammar definition. Extends `vendor/tree-sitter-c-sharp/grammar.js`. |
| `src/grammar.json` | The grammar as JSON, written from `grammar.js` by `generate.sh`; never edited by hand. The input of `build.rs`. |
| `src/scanner.c` | tree-sitter-c-sharp's external scanner (the interpolated and raw string tokens), carried by the grammar. |
| `src/lib.rs`, `build.rs`, `Cargo.toml` | The Rust crate `tree-sitter-razor`: `LANGUAGE` and `NODE_TYPES`. `build.rs` generates `parser.c`, `node-types.json` and `tree_sitter/*.h` from `src/grammar.json` into `OUT_DIR` with `tree-sitter-generate` (the CLI's own generator, pinned to `PIN`'s version), caches them under `~/.cache/eludite/grammars/razor/<key>/` keyed by the inputs, and compiles them with the scanner (ADR-0012). Nothing generated is checked in. |
| `test/corpus/*.txt` | The grammar's cases, in `tree-sitter test`'s format (`markup.txt` holds the markup cases Eludite added). `tests/corpus.rs` replays them with `cargo test`, so CI needs no CLI. |
| `tests/fixtures.rs` | Every file under `corpus/web/razor/` parses with no error node. |
| `generate.sh`, `generate.ps1`, `PIN` | Regeneration of `src/grammar.json` with the pinned CLI (`npx`), then the CLI's own test run; the parser the CLI writes for that run is removed after. |

## Regenerating

```
grammars/razor/generate.sh      # generate.ps1 on Windows
cargo test -p tree-sitter-razor
```

The CLI version is `PIN` (0.27.0, matching the editor's `tree-sitter` 0.27 runtime; the parser's ABI is 15), and
`Cargo.toml` pins the `tree-sitter-generate` build dependency at the same version (`tests/pin.rs` checks), so the
parser `build.rs` writes is the one the CLI's test run proved. A different generator writes a different parser for the
same grammar, so bump `PIN`, the build dependency and the runtime crate together. Generation takes about 15 s once per
cache key; `cargo build` then reuses it from `target/` and, in a fresh checkout, from the cache folder
(`ELUDITE_CACHE_DIR`, else `$XDG_CACHE_HOME/eludite`, else `~/.cache/eludite`).

## Changes from the library (`bed3116`)

Each is meant to go back upstream; the corpus case that proves it is named.

Cases are named `file > case`, all under `test/corpus/`.

- **Named markup nodes.** Start, end, void and self-closing tags carry a `tag_name`; plain attributes an
  `attribute_name` and, for the text of a quoted or unquoted value, an `attribute_value` (quotes stay anonymous; a value
  that is only Razor expressions has none); element content has `text` and `entity` where the library had hidden tokens.
  Why: a highlight query needs nodes to capture, and the library's trees were a bare `(element)`. Every `razor_*` node
  and `at_*` alias is unchanged. Proof: every markup case, e.g. `html.txt > HTML with attributes`.
- **`script_element` and `style_element`.** Their bodies are one `raw_text` token (everything up to `</script` or
  `</style`, case-blind), never markup or C#, as in tree-sitter-html, so an editor can inject JavaScript and CSS. Their
  attributes parse as on any element. Proof: `markup.txt > Script and style bodies are raw text`,
  `razor_html.txt > razor section`.
- **Tag names by longest match.** `_tag_name` is lexed at the void names' precedence and defined after the void, script
  and style names: a longer name wins by length (`<StyleSheet>`, `<inputs>`), an exact match goes to the special name.
  Dots and underscores are part of a name, for fully qualified components and `<svg:path>`. Proof:
  `markup.txt > Dotted and namespaced tag names`, `markup.txt > A tag that only starts like script or style`.
- **Directive attribute values.** A value may be an implicit expression (`@bind-Value="@_dense"`, `@key="@item.Id"`,
  `@onclick="@Go"`, also after a modifier), or unquoted: `@onclick=@(...)`, `@ref=_grid`,
  `@bind-Value=context.Item.Value`. An unquoted implicit expression takes no `@await` (see the next change for
  why that matters to the parser's size). Proof: `razor_html.txt > Implicit expressions as directive attribute values`,
  `razor_html.txt > Unquoted directive attribute values`.
- **`@await` takes an implicit chain.** `@await Html.PartialAsync("_Nav")` is `await` followed by the same space-free
  chain as any implicit expression, as Razor parses it, instead of C#'s whole `await_expression`. That stops an await
  from running on into the prose after it, and the full C# expression automaton is no longer copied for every context
  an implicit expression stands in: the parser has 14,026 states instead of the library's 19,714 (`parser.c` 40 MB
  instead of 57 MB) with every change here included. The `await_expression` node stays, with the chain's children:
  this changes the tree of the library's `expressions.txt > Await Implicit Razor Expression`.
- **Text-valued directive attributes.** `@formname`, and `@bind` with `:event` or `:format`, take markup text
  (`@formname="disable-2fa"` is not C#); the value is an `attribute_value`. This changes the tree of the library's
  `razor_html.txt > Directive attributes: rendermode, bind-Value, bind:event`, whose `oninput` was an `identifier`.
  `:suppressField` joins the modifiers. Proof: `razor_html.txt > Directive attributes whose value is text`.
- **Bare `@page`.** The route is optional (Razor Pages). Proof: `directives.txt > A bare @page beside a routed one`.
- **Trailing `;`.** `@using`, `@inherits`, `@implements`, `@inject`, `@model`, `@layout` and `@namespace` take an
  optional `;` (`@model` right-associatively, so inside `@{ }` it is not an empty statement). Proof:
  `directives.txt > A trailing semicolon after a directive`.
- **`@namespace` with one identifier.** Any name, not only a qualified one. Proof:
  `directives.txt > Namespace with a single identifier`.
- **`@typeparam` constraints.** `@typeparam T where T : Enum` takes C#'s `type_parameter_constraints_clause`. Proof:
  `directives.txt > A type parameter with a constraint`.
- **`@rendermode @(...)`.** A render mode may be an explicit expression. Proof:
  `directives.txt > Render mode as an explicit expression`.
- **Byte order mark.** U+FEFF is whitespace to the extras (tree-sitter's `\s` lacks it) and cannot start a text run.
  Visual Studio writes one on every file it creates. Proof: `directives.txt > A leading byte order mark is an extra`.
- **Directives anywhere at the top level, and `@model` inside `@{ }`.** Razor accepts both; MudBlazor puts directives
  after a `@{ #pragma ... }` block and eShopOnWeb puts `@model` in `@{ }`. Proof:
  `directives.txt > Directives after a code block`, `directives.txt > A model directive inside a code block`.
- **Line directives in code.** `#nullable`, `#pragma`, `#region`/`#endregion`, `#line`, `#error`, `#warning`,
  `#define` and `#undef` are nodes wherever C# members or statements stand in `@code`, `@functions` and `@{ }`, and in a
  class body there; they stay out of the extras so `href="#line-5"` is not a directive. Proof:
  `codeblocks.txt > Preprocessor lines in @code`, `> Preprocessor lines in @functions`,
  `> Preprocessor lines in a code block`, `> Regions inside a class in @code`.
- **Markup in a C# `switch`.** A `case` inside `@{ }` may render an element before its `break;`. Proof:
  `codeblocks.txt > Markup in a switch section inside a code block`.
- **Unquoted attribute values.** `class=foo`, `Value=@role`, `CanDrop=@((x) => false)`; a `/` before `>` closes the
  tag (`Editable=false/>`). Proof: `markup.txt > Unquoted attribute values`.
- **An attribute value starting with `/`.** Lexed above C#'s comment, so `src="//cdn.example.com/x.js"` is not a `//`
  comment. Proof: `markup.txt > An attribute value that starts with two slashes`.
- **A type in `@(...)`.** `T="@(int?)"` (a generic component's type argument) parses as a type, at a lower dynamic
  precedence than an expression. Proof: `razor_html.txt > A type or null as an explicit expression's content`.
- **Verbatim identifiers in C#.** `@name` inside a C# expression (`!@ClickPropagation`, `(@operator ?? "")`,
  `F(@context.Items)`) is an `identifier`; the C# grammar's own form is gone because `@` is Razor's token. Where an
  implicit expression could also stand, the implicit expression wins (dynamic precedence). Proof:
  `razor_html.txt > A verbatim identifier inside C#`.
- **A null-forgiving `!` in an implicit expression.** `@Template!(item)`, `@Item!.Name`, `@Items![0]`; a `!` not
  followed by a call, member access or index stays text (`Hello @name!`). Proof:
  `expressions.txt > A null-forgiving operator in an implicit expression`.
- **Markup text outside elements.** Text at the top level of a file, also right after a directive; a `@section` body is markup as Razor reads it;
  inside `@if`, `@else`, `@foreach`, `@for`, `@while`, `@using (...)`, `@lock`, `@try` and the other C# bodies, text
  that starts with something C# cannot start (`© 2023`), a word followed by a transition (`Hello @name`) and a lone
  word before the closing brace are `text`. Text directly in a `{ }` body stops at braces, and the bodies may be
  empty. A sentence that begins with a word (`Welcome to the app.`) inside those C# bodies is not supported: Razor
  reads it as C#, and the lexer cannot tell it from a statement. Proof: `markup.txt > Text at the top level`,
  `directives.txt > Text right after a directive`,
  `> Text in an @if and @else body`, `> Text in loop bodies`, `> Text in a section body`,
  `> Text in using, lock and try bodies`.
- **`&` and `@` in text.** An `&` that cannot begin an entity is text (`Backers & Sponsors`, `&nbsp` without `;`);
  `&amp;` is still an `entity`. An `@` between a letter or digit and anything but `(` is text, in element content and
  in attribute values (Razor's e-mail rule: `support@example.com`, `wght@400`), while `Age@(age)` is still a
  transition. Proof: `markup.txt > An ampersand that begins no entity is text`,
  `markup.txt > An @ after a letter or digit is text`.
