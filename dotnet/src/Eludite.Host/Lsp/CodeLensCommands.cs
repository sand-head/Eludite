using System.Text.Json;
using System.Text.Json.Nodes;

namespace Eludite.Host.Lsp;

/// <summary>
/// Brief 0052: maps the pinned Roslyn language server's CodeLens commands to Eludite's in the answers to
/// <c>textDocument/codeLens</c> and <c>codeLens/resolve</c> (protocol/schemas/host-rpc.md, "CodeLens";
/// protocol/schemas/host/code-lens.json). <c>roslyn.client.peekReferences</c> <c>[uri, position]</c> becomes
/// <c>eludite.editor.find_references</c> <c>[{ uri, path, position }]</c>; <c>dotnet.test.run</c>
/// <c>[{ textDocument, range, attachDebugger }]</c> becomes <c>eludite.test.run</c> or <c>eludite.test.debug</c>
/// <c>[{ uri, path, range, member }]</c>, where <c>member</c> is the identifier the range covers in the host's copy of
/// the document (Roslyn's lens names no test). Titles, ranges and data are unchanged; other commands pass through.
/// </summary>
public static class CodeLensCommands
{
    /// <summary>Roslyn's references lens command (a client command: the client shows the references).</summary>
    public const string RoslynReferences = "roslyn.client.peekReferences";

    /// <summary>Roslyn's run and debug test lens command.</summary>
    public const string RoslynRunTests = "dotnet.test.run";

    public const string FindReferences = "eludite.editor.find_references";
    public const string TestRun = "eludite.test.run";
    public const string TestDebug = "eludite.test.debug";

    /// <summary>
    /// Maps the commands of a <c>textDocument/codeLens</c> result (an array or null) or a <c>codeLens/resolve</c>
    /// result (one lens). <paramref name="textOf"/> gives the host's copy of a document's text by uri, or null.
    /// </summary>
    public static JsonElement Map(JsonElement result, Func<string, string?> textOf)
    {
        switch (result.ValueKind)
        {
            case JsonValueKind.Array:
            {
                var lenses = JsonNode.Parse(result.GetRawText())!.AsArray();
                var changed = false;
                foreach (var lens in lenses)
                {
                    if (lens is JsonObject o)
                    {
                        changed |= MapLens(o, textOf);
                    }
                }

                return changed ? JsonSerializer.SerializeToElement(lenses) : result;
            }

            case JsonValueKind.Object:
            {
                var lens = JsonNode.Parse(result.GetRawText())!.AsObject();
                return MapLens(lens, textOf) ? JsonSerializer.SerializeToElement(lens) : result;
            }

            default:
                return result;
        }
    }

    /// <summary>Rewrites one lens's command in place; returns whether it changed.</summary>
    internal static bool MapLens(JsonObject lens, Func<string, string?> textOf)
    {
        if (lens["command"] is not JsonObject command || command["command"]?.GetValueKind() != JsonValueKind.String)
        {
            return false;
        }

        var id = command["command"]!.GetValue<string>();
        var args = command["arguments"] as JsonArray;
        switch (id)
        {
            case RoslynReferences when args is [JsonValue uriValue, JsonObject position, ..]
                                      && uriValue.GetValueKind() == JsonValueKind.String:
            {
                var uri = uriValue.GetValue<string>();
                command["command"] = FindReferences;
                command["arguments"] = new JsonArray(new JsonObject
                {
                    ["uri"] = uri,
                    ["path"] = PathOf(uri),
                    ["position"] = position.DeepClone(),
                });
                return true;
            }

            case RoslynRunTests when args is [JsonObject run, ..]:
            {
                var uri = (run["textDocument"] as JsonObject)?["uri"]?.GetValue<string>();
                if (uri is null || run["range"] is not JsonObject range)
                {
                    return false;
                }

                var debug = run["attachDebugger"]?.GetValueKind() == JsonValueKind.True;
                command["command"] = debug ? TestDebug : TestRun;
                command["arguments"] = new JsonArray(new JsonObject
                {
                    ["uri"] = uri,
                    ["path"] = PathOf(uri),
                    ["range"] = range.DeepClone(),
                    ["member"] = MemberAt(textOf(uri), range),
                });
                return true;
            }

            default:
                return false;
        }
    }

    /// <summary>The absolute path of a <c>file:</c> uri (the uri itself for another scheme).</summary>
    internal static string PathOf(string uri) =>
        System.Uri.TryCreate(uri, UriKind.Absolute, out var u) && u.IsFile ? u.LocalPath : uri;

    /// <summary>
    /// The text <paramref name="range"/> covers in <paramref name="text"/> when it is one line (an identifier), else
    /// the identifier that starts there; empty when the text is unknown.
    /// </summary>
    internal static string MemberAt(string? text, JsonObject range)
    {
        if (text is null || range["start"] is not JsonObject start || range["end"] is not JsonObject end)
        {
            return string.Empty;
        }

        var from = OpenDocuments.OffsetOf(text, JsonSerializer.SerializeToElement(start));
        var to = OpenDocuments.OffsetOf(text, JsonSerializer.SerializeToElement(end));
        if (to <= from)
        {
            to = from;
            while (to < text.Length && (char.IsLetterOrDigit(text[to]) || text[to] == '_'))
            {
                to++;
            }
        }

        var member = text[from..to];
        var newline = member.IndexOfAny(['\r', '\n']);
        return (newline < 0 ? member : member[..newline]).TrimStart('@');
    }
}
