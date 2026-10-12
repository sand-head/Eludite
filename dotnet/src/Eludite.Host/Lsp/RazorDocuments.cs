using System.Text.Json;
using System.Text.Json.Nodes;
using StreamJsonRpc;

namespace Eludite.Host.Lsp;

/// <summary>
/// Brief 0079: the host's side of Razor cohosting (ADR-0015). The pinned Roslyn language server composes the Razor
/// extension (<c>Microsoft.VisualStudioCode.RazorExtension</c>), which serves <c>.razor</c> and <c>.cshtml</c>
/// documents itself and asks the client for the HTML parts on a generated HTML projection of each document:
/// <list type="bullet">
/// <item><c>razor/updateHtml</c> <c>{ textDocument, checksum, text }</c> publishes a projection; it is kept here, one
/// per Razor document, and answered <c>null</c> at once.</item>
/// <item>An HTML-forwarded request (<see cref="HtmlMethods"/>, params <c>{ textDocument, checksum, request }</c>) is
/// relayed to the shell as <c>eludite/razor/html</c> <c>{ method, uri, checksum, text, params, eluditeGeneration }</c>
/// when the projection with that checksum is here, and the shell's result is returned unchanged; anything else (no
/// projection, another checksum, a shell error, no shell, a generation change) answers <c>null</c>, which Razor reads
/// as "no HTML result".</item>
/// </list>
/// Contract: protocol/schemas/host-rpc.md, "Razor"; schema protocol/schemas/host/razor-html.json. Thread-safe.
/// </summary>
public sealed class RazorDocuments
{
    /// <summary>The request the host sends the shell for an HTML-forwarded request.</summary>
    public const string ShellHtmlRequest = "eludite/razor/html";

    /// <summary>Razor's notification of a new HTML projection (sent as a request).</summary>
    public const string UpdateHtml = "razor/updateHtml";

    /// <summary>What the host tells Roslyn when the shell closes a Razor document (VS Code's client does the same).</summary>
    public const string DocumentClosed = "razor/documentClosed";

    /// <summary>
    /// The LSP methods Razor forwards to the client's HTML server at the pinned commit (the callers of
    /// <c>IHtmlRequestInvoker.MakeHtmlLspRequestAsync</c>).
    /// </summary>
    public static IReadOnlyList<string> HtmlMethods { get; } =
    [
        "textDocument/completion",
        "completionItem/resolve",
        "textDocument/hover",
        "textDocument/signatureHelp",
        "textDocument/definition",
        "textDocument/implementation",
        "textDocument/documentHighlight",
        "textDocument/references",
        "textDocument/prepareRename",
        "textDocument/rename",
        "textDocument/formatting",
        "textDocument/rangeFormatting",
        "textDocument/onTypeFormatting",
        "textDocument/foldingRange",
        "textDocument/documentColor",
        "textDocument/colorPresentation",
        "textDocument/codeAction",
        "codeAction/resolve",
        "textDocument/linkedEditingRange",
        "textDocument/_vs_onAutoInsert",
        "textDocument/_vs_textPresentation",
        "textDocument/_vs_uriPresentation",
    ];

    /// <summary>
    /// Requests the shell forwards for Razor documents beyond the C# list (Format Selection, typing, folding, linked
    /// tag editing); <see cref="LspProxy"/> adds the ones no other list has.
    /// </summary>
    public static IReadOnlyList<string> ForwardedRequests { get; } =
    [
        "textDocument/rangeFormatting",
        "textDocument/onTypeFormatting",
        "textDocument/foldingRange",
        "textDocument/linkedEditingRange",
    ];

    private static readonly JsonElement JsonNull = JsonDocument.Parse("null").RootElement.Clone();

    private readonly Lock _lock = new();
    private readonly Dictionary<string, Projection> _projections = new(StringComparer.Ordinal);
    private long _relayed;

    /// <summary>An HTML projection as Razor last published it.</summary>
    public sealed record Projection(string Checksum, string Text);

    /// <summary>Requests relayed to the shell (for tests and the log).</summary>
    public long Relayed => Interlocked.Read(ref _relayed);

    /// <summary>Whether <paramref name="uri"/> names a Razor document (<c>.razor</c> or <c>.cshtml</c>).</summary>
    public static bool IsRazor(string? uri)
    {
        if (string.IsNullOrEmpty(uri))
        {
            return false;
        }

        var path = uri;
        var cut = path.IndexOfAny(['?', '#']);
        if (cut >= 0)
        {
            path = path[..cut];
        }

        return path.EndsWith(".razor", StringComparison.OrdinalIgnoreCase)
               || path.EndsWith(".cshtml", StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>The projection for <paramref name="uri"/>, or null.</summary>
    public Projection? Get(string uri)
    {
        lock (_lock)
        {
            return _projections.GetValueOrDefault(uri);
        }
    }

    /// <summary><c>razor/updateHtml</c>: keeps the projection and answers null.</summary>
    public JsonElement Update(JsonElement parameters)
    {
        if (UriOf(parameters) is { } uri
            && parameters.TryGetProperty("checksum", out var checksum) && checksum.ValueKind == JsonValueKind.String
            && parameters.TryGetProperty("text", out var text) && text.ValueKind == JsonValueKind.String)
        {
            lock (_lock)
            {
                _projections[uri] = new Projection(checksum.GetString()!, text.GetString()!);
            }
        }

        return JsonNull;
    }

    /// <summary>Forgets the projection of a closed document.</summary>
    public void Close(string uri)
    {
        lock (_lock)
        {
            _projections.Remove(uri);
        }
    }

    /// <summary>Forgets every projection (the language server restarted; Razor publishes again on demand).</summary>
    public void Clear()
    {
        lock (_lock)
        {
            _projections.Clear();
        }
    }

    /// <summary>
    /// An HTML-forwarded request from Razor: relayed to <paramref name="shell"/> as <see cref="ShellHtmlRequest"/>
    /// under <paramref name="generation"/>, or null (see the class docs). <paramref name="generationChanged"/> fires when
    /// the generation moves on; <paramref name="cancellationToken"/> is Roslyn's cancellation.
    /// </summary>
    public async Task<JsonElement> RelayAsync(
        string method,
        JsonElement parameters,
        JsonRpc? shell,
        long generation,
        CancellationToken generationChanged,
        CancellationToken cancellationToken)
    {
        if (shell is null || UriOf(parameters) is not { } uri
            || !parameters.TryGetProperty("checksum", out var checksum) || checksum.ValueKind != JsonValueKind.String)
        {
            return JsonNull;
        }

        var projection = Get(uri);
        if (projection is null || projection.Checksum != checksum.GetString())
        {
            return JsonNull;
        }

        var request = new JsonObject
        {
            ["method"] = method,
            ["uri"] = uri,
            ["checksum"] = projection.Checksum,
            ["text"] = projection.Text,
            ["params"] = parameters.TryGetProperty("request", out var inner) ? JsonNode.Parse(inner.GetRawText()) : null,
            [LspProxy.GenerationProperty] = generation,
        };
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, generationChanged);
        try
        {
            Interlocked.Increment(ref _relayed);
            return await shell.InvokeWithParameterObjectAsync<JsonElement>(ShellHtmlRequest, request, linked.Token)
                .WaitAsync(linked.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            // The generation moved on: the projection belongs to a solution that is gone.
            return JsonNull;
        }
        catch (Exception ex) when (ex is RemoteInvocationException or ConnectionLostException or ObjectDisposedException or IOException)
        {
            return JsonNull;
        }
    }

    private static string? UriOf(JsonElement parameters) =>
        parameters.ValueKind == JsonValueKind.Object
        && parameters.TryGetProperty("textDocument", out var document) && document.ValueKind == JsonValueKind.Object
        && document.TryGetProperty("uri", out var uri) && uri.ValueKind == JsonValueKind.String
            ? uri.GetString()
            : null;
}
