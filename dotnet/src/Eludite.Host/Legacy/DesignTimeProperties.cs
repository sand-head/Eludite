using System.Text.RegularExpressions;

namespace Eludite.Host.Legacy;

/// <summary>Global properties shared by every evaluator: a design-time pass, never a compile.</summary>
public static partial class DesignTimeProperties
{
    /// <summary>
    /// Properties Visual Studio's design-time builds set. <c>SkipCompilerExecution</c> and
    /// <c>BuildingProject=false</c> keep anything from compiling; only evaluation and the reference-resolution
    /// targets run.
    /// </summary>
    public static IReadOnlyDictionary<string, string> Create(string? targetFrameworkRootPath)
    {
        var props = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase)
        {
            ["DesignTimeBuild"] = "true",
            ["BuildingProject"] = "false",
            ["BuildProjectReferences"] = "false",
            ["SkipCompilerExecution"] = "true",
            ["ProvideCommandLineArgs"] = "true",
            ["Configuration"] = "Debug",
            ["GenerateResourceMSBuildArchitecture"] = "CurrentArchitecture",
        };
        if (targetFrameworkRootPath is not null)
        {
            props["TargetFrameworkRootPath"] = targetFrameworkRootPath;
        }

        return props;
    }

    /// <summary>Environment variables MSBuildLocator sets in this process that must not leak into child MSBuilds.</summary>
    public static IReadOnlyList<string> LocatorVariables { get; } = ["MSBUILD_EXE_PATH", "MSBuildExtensionsPath", "MSBuildSDKsPath", "MSBuildLoadMicrosoftTargetsReadOnly"];

    [GeneratedRegex(@"<TargetFrameworkVersion>\s*(?<v>v[\d.]+)\s*</TargetFrameworkVersion>", RegexOptions.IgnoreCase)]
    private static partial Regex TargetFrameworkVersion();

    /// <summary>Cheap pre-evaluation read of <c>TargetFrameworkVersion</c> from the project XML (first occurrence).</summary>
    public static string? ReadTargetFrameworkVersion(string projectPath)
    {
        try
        {
            var m = TargetFrameworkVersion().Match(File.ReadAllText(projectPath));
            return m.Success ? m.Groups["v"].Value : null;
        }
        catch (IOException)
        {
            return null;
        }
    }

    /// <summary>True for WebForms markup files that get a designer partial.</summary>
    public static bool IsMarkup(string path) =>
        path.EndsWith(".aspx", StringComparison.OrdinalIgnoreCase)
        || path.EndsWith(".ascx", StringComparison.OrdinalIgnoreCase)
        || path.EndsWith(".master", StringComparison.OrdinalIgnoreCase);
}
