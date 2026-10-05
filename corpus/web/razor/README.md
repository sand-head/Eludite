# corpus/web/razor/

Hand-written Razor fixtures, MIT: Blazor components (`.razor`), Razor Pages and MVC views (`.cshtml`) shaped like real
ones, each exercising constructs the Razor grammar (`grammars/razor/`, brief 0056) parses. Used by
`grammars/razor/tests/fixtures.rs` (every file parses with no `ERROR` or `MISSING` node) and by the editor's Razor
highlighting tests (`crates/editor/src/syntax/web_tests.rs`).

| File | What it exercises |
|---|---|
| `Sample.razor` | The library's own sample: directives, markup, entities, void elements, a Razor comment in markup, nested quotes in an attribute expression, a template, a code block. |
