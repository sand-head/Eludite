# corpus/web/blazor-app

A Blazor Web App trimmed by hand from `dotnet new blazor` (.NET 10), with a Razor Pages page and an MVC view added,
for brief 0079's Razor IntelliSense tests (`dotnet/tests/Eludite.Host.Tests/RazorTests.cs`). MIT, like the rest of
`corpus/`. It has no package references, so `dotnet restore` works offline; the tests copy it to a temporary folder
before restoring, so nothing is written here.

| File | What the tests use it for |
|---|---|
| `Components/_Imports.razor` | The component namespaces every `.razor` file sees |
| `Components/Pages/Counter.razor` | `@code` with a field and a `[Parameter] IncrementAmount`, `@onclick` |
| `Components/Pages/Home.razor` | `<Counter IncrementAmount="2" />` (component and parameter completion, go to definition), `@bind` |
| `Pages/Privacy.cshtml` | A Razor Pages page with `@model` and an `asp-page` tag helper |
| `Views/Home/Index.cshtml` | An MVC view with an `asp-controller` tag helper |
