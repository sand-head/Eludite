// The web corpus's minimal ASP.NET Core project (brief 0037): a page with a form that posts back to itself, and a
// JSON endpoint the page's button reads. F5 (or Ctrl+F5) in Eludite runs it with its launch profile and opens the
// page in the Web Browser window once Kestrel says "Now listening on:".
using System.Net;

var builder = WebApplication.CreateBuilder(args);
var app = builder.Build();

app.MapGet("/", () => Results.Content(Page.Render(null), "text/html; charset=utf-8"));
app.MapPost("/", async (HttpRequest request) =>
{
    var form = await request.ReadFormAsync();
    return Results.Content(Page.Render(form["name"].ToString()), "text/html; charset=utf-8");
});
app.MapGet("/api/time", () => Results.Json(new { utc = DateTimeOffset.UtcNow.ToString("O"), app = "MinimalApi" }));

app.Run();

/// <summary>The one page: a form greeting the name it was given, and a button that reads /api/time.</summary>
static class Page
{
    public static string Render(string? name)
    {
        var greeting = string.IsNullOrWhiteSpace(name)
            ? ""
            : $"<p id=\"greeting\">Hello, {WebUtility.HtmlEncode(name)}!</p>";
        return $$"""
            <!doctype html>
            <html lang="en">
            <head>
              <meta charset="utf-8">
              <title>Minimal API</title>
              <style>
                body { font-family: sans-serif; margin: 2rem; }
                label, input, button { font-size: 1rem; }
                #greeting { color: #1e7b34; }
              </style>
            </head>
            <body>
              <h1>Minimal API</h1>
              <form method="post" action="/">
                <label for="name">Name</label>
                <input id="name" name="name" type="text" autocomplete="off">
                <button type="submit">Greet</button>
              </form>
              {{greeting}}
              <p><button id="time" type="button">What time is it?</button> <span id="now"></span></p>
              <script>
                document.getElementById("time").addEventListener("click", async () => {
                  const r = await fetch("/api/time");
                  document.getElementById("now").textContent = (await r.json()).utc;
                });
              </script>
            </body>
            </html>
            """;
    }
}
