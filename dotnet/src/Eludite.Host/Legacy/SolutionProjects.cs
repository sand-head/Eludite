using System.Text.RegularExpressions;
using System.Xml.Linq;

namespace Eludite.Host.Legacy;

/// <summary>Lists the C# projects of a <c>.sln</c>, <c>.slnx</c> or a single <c>.csproj</c>.</summary>
public static partial class SolutionProjects
{
    [GeneratedRegex("""^Project\("\{(?<type>[^}]+)\}"\)\s*=\s*"[^"]*",\s*"(?<path>[^"]+\.csproj)"\s*,""", RegexOptions.Multiline | RegexOptions.IgnoreCase)]
    private static partial Regex SlnProject();

    /// <summary>Full paths of the <c>.csproj</c> files, in solution order. Missing files are skipped.</summary>
    public static IReadOnlyList<string> Read(string path)
    {
        ArgumentNullException.ThrowIfNull(path);
        var full = Path.GetFullPath(path);
        var dir = Path.GetDirectoryName(full)!;
        IEnumerable<string> relative;
        if (full.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase))
        {
            return File.Exists(full) ? [full] : [];
        }
        else if (full.EndsWith(".slnx", StringComparison.OrdinalIgnoreCase))
        {
            relative = XDocument.Load(full).Descendants("Project").Select(p => (string?)p.Attribute("Path")).OfType<string>()
                .Where(p => p.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase));
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
