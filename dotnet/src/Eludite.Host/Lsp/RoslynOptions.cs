using System.Text.Json;

namespace Eludite.Host.Lsp;

/// <summary>
/// Brief 0069: Eludite's settings as Roslyn options. The shell sends the mapped settings in
/// <c>workspace/didChangeConfiguration</c> (<c>{ settings: { "dotnet.completion.showItemsFromUnimportedNamespaces": true } }</c>);
/// the host keeps them and answers Roslyn's <c>workspace/configuration</c> through <see cref="Table"/>, the one place
/// that names Roslyn's options (<c>DidChangeConfigurationNotificationHandler_OptionList.cs</c> at the pin). Roslyn asks
/// for a per-language option once per language, as <c>csharp|group.name</c> and <c>visual_basic|group.name</c>; both get
/// the same value. A setting the shell never sent answers <c>null</c> (Roslyn's default).
/// </summary>
public sealed class RoslynOptions
{
    /// <summary>One mapping: the Eludite setting, the Roslyn option (without the language prefix), and how a value maps.</summary>
    public sealed record Entry(string Setting, string Option, Func<JsonElement, JsonElement?> Map);

    private static JsonElement? Same(JsonElement v) => v.ValueKind is JsonValueKind.True or JsonValueKind.False or JsonValueKind.String or JsonValueKind.Number ? v : null;

    /// <summary><c>openFiles</c> or <c>fullSolution</c> as Roslyn's scope enums (parsed ignoring case).</summary>
    private static JsonElement? Scope(JsonElement v) => v.ValueKind == JsonValueKind.String
        ? JsonSerializer.SerializeToElement(v.GetString() == "fullSolution" ? "FullSolution" : "OpenFiles")
        : null;

    /// <summary>Eludite setting to Roslyn option. Settings of brief 0066 (inlay hints) are in their own block.</summary>
    public static IReadOnlyList<Entry> Table { get; } =
    [
        new("dotnet.completion.showItemsFromUnimportedNamespaces", "completion.dotnet_show_completion_items_from_unimported_namespaces", Same),
        new("dotnet.completion.showNameSuggestions", "completion.dotnet_show_name_completion_suggestions", Same),
        new("dotnet.backgroundAnalysis.scope", "background_analysis.dotnet_analyzer_diagnostics_scope", Scope),
        new("dotnet.backgroundAnalysis.scope", "background_analysis.dotnet_compiler_diagnostics_scope", Scope),
        new("dotnet.quickInfo.showRemarks", "quick_info.dotnet_show_remarks_in_quick_info", Same),
        new("dotnet.autoInsert", "auto_insert.dotnet_enable_auto_insert", Same),
        new("editor.codeLens.references", "code_lens.dotnet_enable_references_code_lens", Same),
        new("editor.codeLens.tests", "code_lens.dotnet_enable_tests_code_lens", Same),

        // Brief 0066's inlay hint settings (editor.inlayHints.*), mapped here by brief 0069.
        new("editor.inlayHints.parameters", "inlay_hints.dotnet_enable_inlay_hints_for_parameters", Same),
        new("editor.inlayHints.literalParameters", "inlay_hints.dotnet_enable_inlay_hints_for_literal_parameters", Same),
        new("editor.inlayHints.indexerParameters", "inlay_hints.dotnet_enable_inlay_hints_for_indexer_parameters", Same),
        new("editor.inlayHints.objectCreationParameters", "inlay_hints.dotnet_enable_inlay_hints_for_object_creation_parameters", Same),
        new("editor.inlayHints.otherParameters", "inlay_hints.dotnet_enable_inlay_hints_for_other_parameters", Same),
        new("editor.inlayHints.suppressForParametersThatDifferOnlyBySuffix", "inlay_hints.dotnet_suppress_inlay_hints_for_parameters_that_differ_only_by_suffix", Same),
        new("editor.inlayHints.suppressForParametersThatMatchMethodIntent", "inlay_hints.dotnet_suppress_inlay_hints_for_parameters_that_match_method_intent", Same),
        new("editor.inlayHints.suppressForParametersThatMatchArgumentName", "inlay_hints.dotnet_suppress_inlay_hints_for_parameters_that_match_argument_name", Same),
        new("editor.inlayHints.types", "inlay_hints.csharp_enable_inlay_hints_for_types", Same),
        new("editor.inlayHints.implicitVariableTypes", "inlay_hints.csharp_enable_inlay_hints_for_implicit_variable_types", Same),
        new("editor.inlayHints.lambdaParameterTypes", "inlay_hints.csharp_enable_inlay_hints_for_lambda_parameter_types", Same),
        new("editor.inlayHints.implicitObjectCreation", "inlay_hints.csharp_enable_inlay_hints_for_implicit_object_creation", Same),
        new("editor.inlayHints.collectionExpressions", "inlay_hints.csharp_enable_inlay_hints_for_collection_expressions", Same),
    ];

    /// <summary>The settings of <see cref="Table"/>, as the names the shell may send.</summary>
    public static IReadOnlyCollection<string> Settings { get; } = Table.Select(e => e.Setting).Distinct().ToArray();

    private readonly object _lock = new();
    private Dictionary<string, JsonElement> _values = new(StringComparer.Ordinal);

    /// <summary>
    /// <c>workspace/didChangeConfiguration</c> from the shell: replace the values with <c>params.settings</c> (unknown keys
    /// are ignored). Returns whether anything changed.
    /// </summary>
    public bool Update(JsonElement parameters)
    {
        var next = new Dictionary<string, JsonElement>(StringComparer.Ordinal);
        if (parameters.ValueKind == JsonValueKind.Object
            && parameters.TryGetProperty("settings", out var settings)
            && settings.ValueKind == JsonValueKind.Object)
        {
            foreach (var p in settings.EnumerateObject())
            {
                if (Settings.Contains(p.Name))
                {
                    next[p.Name] = p.Value.Clone();
                }
            }
        }

        lock (_lock)
        {
            var same = next.Count == _values.Count
                       && next.All(kv => _values.TryGetValue(kv.Key, out var v) && v.GetRawText() == kv.Value.GetRawText());
            _values = next;
            return !same;
        }
    }

    /// <summary>The answer for one <c>workspace/configuration</c> section, or null when Eludite has no value for it.</summary>
    public JsonElement? Answer(string section)
    {
        var bar = section.IndexOf('|', StringComparison.Ordinal);
        var option = bar >= 0 ? section[(bar + 1)..] : section;
        lock (_lock)
        {
            foreach (var entry in Table)
            {
                if (entry.Option == option && _values.TryGetValue(entry.Setting, out var value))
                {
                    return entry.Map(value);
                }
            }
        }

        return null;
    }
}
