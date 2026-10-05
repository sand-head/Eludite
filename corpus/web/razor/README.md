# corpus/web/razor/

Hand-written Razor fixtures, MIT: Blazor components (`.razor`), Razor Pages and MVC views (`.cshtml`) shaped like real
ones, each exercising constructs the Razor grammar (`grammars/razor/`, brief 0056) parses. Used by
`grammars/razor/tests/fixtures.rs` (every file parses with no `ERROR` or `MISSING` node) and by the editor's Razor
highlighting tests (`crates/editor/src/syntax/web_tests.rs`).

| File | What it exercises |
|---|---|
| `Sample.razor` | The library's own sample: directives, markup, entities, void elements, a Razor comment in markup, nested quotes in an attribute expression, a template, a code block. |
| `Components/Pages/Counter.razor` | A Blazor component: `@page` with a route, `@rendermode @(new InteractiveServerRenderMode(prerender: false))`, `@attribute`, `@inject` of a generic type, a `@* *@` comment, directive attributes with `@` values (`@onclick="@IncrementCount"`, `@key="@item.Id"`, `@bind-Value="@filter"`, `@bind-Value:after="@ApplyFilter"`) and `@ref`, a fully qualified (dotted) component tag, `@if`/`else` with text and elements, `@foreach`, an entity, and `@code` with `#nullable enable` and a `RenderFragment` template. |
| `Pages/Index.cshtml` | A Razor Pages page: a bare `@page`, `@model IndexModel`, `@using X;` with its semicolon, a `@{ }` block, a `<style>` block (with `@@media`), markup text at the top level, `@foreach`, `@await Html.PartialAsync(...)`, and `@section Scripts { <script> ... </script> }` whose JavaScript has a `for (var i = 0; i < ca.length; i++)` loop. |
| `Views/Home/Index.cshtml` | An MVC view that starts with a UTF-8 byte order mark: `@model`, `@{ ViewData["Title"] = "Home"; }`, a `<!DOCTYPE html>` `_Layout.cshtml`-style skeleton with `@RenderBody()` and `@RenderSection("Scripts", required: false)`, an unquoted attribute value (`class=navbar`), `asp-for` and other tag helper attributes, entities, and `@Html.DisplayFor(model => model.UserName)`. |
