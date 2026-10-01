using System.Text.Json;
using Niello.Host.Lsp;

namespace Niello.Host.Tests;

/// <summary>The host's copy of open documents, used to replay them into a restarted language server.</summary>
public sealed class OpenDocumentsTests
{
    private static JsonElement Json(object value) => JsonSerializer.SerializeToElement(value);

    private static object Change(int line, int ch, int endLine, int endCh, string text) =>
        new { range = new { start = new { line, character = ch }, end = new { line = endLine, character = endCh } }, text };

    private static OpenDocuments OpenWith(string text)
    {
        var docs = new OpenDocuments();
        Assert.Equal("file:///a.cs", docs.Open(Json(new { textDocument = new { uri = "file:///a.cs", languageId = "csharp", version = 1, text } })));
        return docs;
    }

    [Fact]
    public void IncrementalEdits_AreAppliedInOrder()
    {
        var docs = OpenWith("class A\n{\n}\n");

        docs.Change(Json(new
        {
            textDocument = new { uri = "file:///a.cs", version = 2 },
            contentChanges = new[] { Change(1, 1, 1, 1, " int X;"), Change(0, 6, 0, 7, "B") },
        }));

        var doc = docs.Get("file:///a.cs")!;
        Assert.Equal(2, doc.Version);
        Assert.Equal("class B\n{ int X;\n}\n", doc.Text);
        Assert.Equal("csharp", doc.LanguageId);
    }

    [Fact]
    public void Positions_AreUtf16AndCrLfAware()
    {
        // "😀" is two UTF-16 code units, as in LSP's default position encoding.
        var docs = OpenWith("var s = \"😀x\";\r\nnext");

        docs.Change(Json(new { textDocument = new { uri = "file:///a.cs", version = 2 }, contentChanges = new[] { Change(0, 11, 0, 12, "y"), Change(0, 99, 1, 0, "|") } }));

        Assert.Equal("var s = \"😀y\";|next", docs.Get("file:///a.cs")!.Text);
    }

    [Fact]
    public void FullReplacement_AndClose()
    {
        var docs = OpenWith("old");

        docs.Change(Json(new { textDocument = new { uri = "file:///a.cs", version = 5 }, contentChanges = new[] { new { text = "new" } } }));
        Assert.Equal("new", docs.Get("file:///a.cs")!.Text);
        Assert.Null(docs.Change(Json(new { textDocument = new { uri = "file:///other.cs", version = 1 }, contentChanges = Array.Empty<object>() })));

        Assert.Equal("file:///a.cs", docs.Close(Json(new { textDocument = new { uri = "file:///a.cs" } })));
        Assert.Equal(0, docs.Count);
        Assert.Null(docs.Close(Json(new { textDocument = new { uri = "file:///a.cs" } })));
    }
}
