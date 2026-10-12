using System.Text.Json;
using Eludite.Host.Lsp;

namespace Eludite.Host.Tests;

/// <summary>Brief 0069: the mapping from Eludite's settings to Roslyn's options.</summary>
public sealed class RoslynOptionsTests
{
    private static JsonElement Settings(object settings) => JsonSerializer.SerializeToElement(new { settings });

    [Fact]
    public void EveryLanguagePrefixGetsTheSameValue_AndUnsentSettingsAreRoslynsDefault()
    {
        var options = new RoslynOptions();
        Assert.Null(options.Answer("csharp|completion.dotnet_show_name_completion_suggestions"));
        Assert.True(options.Update(Settings(new Dictionary<string, object>
        {
            ["dotnet.completion.showNameSuggestions"] = false,
            ["dotnet.backgroundAnalysis.scope"] = "openFiles",
            ["editor.inlayHints.parameters"] = true,
        })));
        Assert.Equal("false", options.Answer("csharp|completion.dotnet_show_name_completion_suggestions")!.Value.GetRawText());
        Assert.Equal("false", options.Answer("visual_basic|completion.dotnet_show_name_completion_suggestions")!.Value.GetRawText());
        Assert.Equal("\"OpenFiles\"", options.Answer("csharp|background_analysis.dotnet_analyzer_diagnostics_scope")!.Value.GetRawText());
        Assert.Equal("\"OpenFiles\"", options.Answer("csharp|background_analysis.dotnet_compiler_diagnostics_scope")!.Value.GetRawText());
        Assert.Equal("true", options.Answer("csharp|inlay_hints.dotnet_enable_inlay_hints_for_parameters")!.Value.GetRawText());
        Assert.Null(options.Answer("csharp|quick_info.dotnet_show_remarks_in_quick_info"));
        Assert.False(options.Update(Settings(new Dictionary<string, object>
        {
            ["dotnet.completion.showNameSuggestions"] = false,
            ["dotnet.backgroundAnalysis.scope"] = "openFiles",
            ["editor.inlayHints.parameters"] = true,
        })));
        // A setting no longer sent goes back to Roslyn's default.
        Assert.True(options.Update(Settings(new Dictionary<string, object>())));
        Assert.Null(options.Answer("csharp|completion.dotnet_show_name_completion_suggestions"));
    }

    [Fact]
    public void TheTableNamesOptionsOnce_AndOnlyRoslynsOptionGroups()
    {
        var groups = new[] { "completion.", "background_analysis.", "quick_info.", "auto_insert.", "code_lens.", "inlay_hints." };
        foreach (var entry in RoslynOptions.Table)
        {
            Assert.Contains(groups, g => entry.Option.StartsWith(g, StringComparison.Ordinal));
        }

        Assert.Equal(RoslynOptions.Table.Count, RoslynOptions.Table.Select(e => e.Option).Distinct().Count());
    }
}
