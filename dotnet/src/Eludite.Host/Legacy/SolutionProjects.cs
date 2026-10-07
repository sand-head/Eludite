using System.Text.RegularExpressions;
using System.Xml.Linq;

namespace Eludite.Host.Legacy;

/// <summary>
/// Lists the MSBuild projects of a <c>.sln</c>, <c>.slnx</c> or a single project file: C# (<c>.csproj</c>), Visual Basic
/// (<c>.vbproj</c>) and F# (<c>.fsproj</c>) alike (brief 0063). Every reader of the solution (the tree, the properties,
/// the configurations, the tests, the build, NuGet and the legacy evaluator) sees the three through this class; MSBuild
/// evaluation is language-neutral.
/// </summary>
public static partial class SolutionProjects
{
    /// <summary>The project file extensions that count, with their dot.</summary>
    public static IReadOnlyList<string> ProjectExtensions { get; } = [".csproj", ".vbproj", ".fsproj"];

    [GeneratedRegex("""^Project\("\{(?<type>[^}]+)\}"\)\s*=\s*"[^"]*",\s*"(?<path>[^"]+\.(?:csproj|vbproj|fsproj))"\s*,""", RegexOptions.Multiline | RegexOptions.IgnoreCase)]
    private static partial Regex SlnProject();

    /// <summary>True when <paramref name="path"/> ends in one of <see cref="ProjectExtensions"/>, in any case.</summary>
    public static bool IsProjectFile(string path)
    {
        ArgumentNullException.ThrowIfNull(path);
        return ProjectExtensions.Any(e => path.EndsWith(e, StringComparison.OrdinalIgnoreCase));
    }

    /// <summary>
    /// Full paths of the <c>.csproj</c>, <c>.vbproj</c> and <c>.fsproj</c> files, in solution order; a project file's own
    /// path when <paramref name="path"/> is one. Missing files are skipped.
    /// </summary>
    public static IReadOnlyList<string> Read(string path)
    {
        ArgumentNullException.ThrowIfNull(path);
        var full = Path.GetFullPath(path);
        var dir = Path.GetDirectoryName(full)!;
        IEnumerable<string> relative;
        if (IsProjectFile(full))
        {
            return File.Exists(full) ? [full] : [];
        }
        else if (full.EndsWith(".slnx", StringComparison.OrdinalIgnoreCase))
        {
            relative = XDocument.Load(full).Descendants("Project").Select(p => (string?)p.Attribute("Path")).OfType<string>()
                .Where(IsProjectFile);
        }
        else
        {
            relative = SlnProject().Matches(File.ReadAllText(full)).Select(m => m.Groups["path"].Value);
        }

        return relative
            .Select(r => Path.GetFullPath(Path.Combine(dir, r.Replace('\\', Path.DirectorySeparatorChar))))
            .Where(File.Exists)
            .Distinct(StringComparer.Ordinal)
            .ToList();
    }

    /// <summary>True when the project file is legacy (non-SDK) MSBuild XML: no <c>Sdk</c> attribute or element.</summary>
    public static bool IsLegacy(string projectPath)
    {
        try
        {
            var root = XDocument.Load(projectPath).Root;
            return root is not null && root.Attribute("Sdk") is null && !root.Elements().Any(e => e.Name.LocalName == "Sdk");
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException)
        {
            return false;
        }
    }

    /// <summary>Cheap check for WebForms markup items before running an evaluation.</summary>
    public static bool MentionsMarkup(string projectPath)
    {
        try
        {
            var text = File.ReadAllText(projectPath);
            return text.Contains(".aspx\"", StringComparison.OrdinalIgnoreCase)
                || text.Contains(".ascx\"", StringComparison.OrdinalIgnoreCase)
                || text.Contains(".master\"", StringComparison.OrdinalIgnoreCase);
        }
        catch (IOException)
        {
            return false;
        }
    }
}
