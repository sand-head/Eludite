using System.Text;
using System.Text.Json;

namespace Eludite.Host.Lsp;

/// <summary>An open document as the shell last described it.</summary>
public sealed record OpenDocument(string Uri, string LanguageId, int Version, string Text);

/// <summary>
/// The host's copy of the documents the shell has open (LSP text synchronization). The host needs it to replay
/// <c>textDocument/didOpen</c> into a restarted language server and to know which documents to warm. Not
/// thread-safe; <see cref="LspProxy"/> serializes access.
/// </summary>
public sealed class OpenDocuments
{
    private readonly Dictionary<string, OpenDocument> _documents = new(StringComparer.Ordinal);

    public int Count => _documents.Count;

    public OpenDocument? Get(string uri) => _documents.GetValueOrDefault(uri);

    public IReadOnlyList<OpenDocument> Snapshot() => [.. _documents.Values];

    /// <summary>Applies <c>textDocument/didOpen</c> params. Returns the uri, or null when the params are malformed.</summary>
    public string? Open(JsonElement parameters)
    {
        if (!TryGetObject(parameters, "textDocument", out var doc) || !TryGetString(doc, "uri", out var uri))
        {
            return null;
        }

        _documents[uri] = new OpenDocument(
            uri,
            TryGetString(doc, "languageId", out var language) ? language : string.Empty,
            doc.TryGetProperty("version", out var v) && v.ValueKind == JsonValueKind.Number ? v.GetInt32() : 0,
            TryGetString(doc, "text", out var text) ? text : string.Empty);
        return uri;
    }

    /// <summary>
    /// Applies <c>textDocument/didChange</c> params: full replacements and incremental edits, with LSP positions in
    /// UTF-16 code units (the same units as .NET strings). Returns the uri, or null when the document is not open.
    /// </summary>
    public string? Change(JsonElement parameters)
    {
        if (!TryGetObject(parameters, "textDocument", out var id) || !TryGetString(id, "uri", out var uri)
            || !_documents.TryGetValue(uri, out var current))
        {
            return null;
        }

        var text = current.Text;
        if (parameters.TryGetProperty("contentChanges", out var changes) && changes.ValueKind == JsonValueKind.Array)
        {
            foreach (var change in changes.EnumerateArray())
            {
                var newText = TryGetString(change, "text", out var t) ? t : string.Empty;
                text = change.TryGetProperty("range", out var range) && range.ValueKind == JsonValueKind.Object
                    ? Splice(text, range, newText)
                    : newText;
            }
        }

        var version = id.TryGetProperty("version", out var v) && v.ValueKind == JsonValueKind.Number ? v.GetInt32() : current.Version + 1;
        _documents[uri] = current with { Version = version, Text = text };
        return uri;
    }

    /// <summary>Applies <c>textDocument/didClose</c> params. Returns the uri that was open, or null.</summary>
    public string? Close(JsonElement parameters) =>
        TryGetObject(parameters, "textDocument", out var id) && TryGetString(id, "uri", out var uri) && _documents.Remove(uri)
            ? uri
            : null;

    internal static string Splice(string text, JsonElement range, string newText)
    {
        var start = OffsetOf(text, range.GetProperty("start"));
        var end = Math.Max(start, OffsetOf(text, range.GetProperty("end")));
        return new StringBuilder(text.Length - (end - start) + newText.Length)
            .Append(text, 0, start).Append(newText).Append(text, end, text.Length - end).ToString();
    }

    /// <summary>Offset of an LSP position; positions past the end of a line or the text clamp to it.</summary>
    internal static int OffsetOf(string text, JsonElement position)
    {
        var line = position.GetProperty("line").GetInt32();
        var character = position.GetProperty("character").GetInt32();
        var offset = 0;
        for (var l = 0; l < line; l++)
        {
            var next = text.IndexOf('\n', offset);
            if (next < 0)
            {
                return text.Length;
            }

            offset = next + 1;
        }

        var lineEnd = text.IndexOf('\n', offset);
        if (lineEnd < 0)
        {
            lineEnd = text.Length;
        }
        else if (lineEnd > offset && text[lineEnd - 1] == '\r')
        {
            lineEnd--;
        }

        return Math.Min(offset + character, lineEnd);
    }

    private static bool TryGetObject(JsonElement e, string name, out JsonElement value)
    {
        if (e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out value) && value.ValueKind == JsonValueKind.Object)
        {
            return true;
        }

        value = default;
        return false;
    }

    private static bool TryGetString(JsonElement e, string name, out string value)
    {
        if (e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var p) && p.ValueKind == JsonValueKind.String)
        {
            value = p.GetString()!;
            return true;
        }

        value = string.Empty;
        return false;
    }
}
