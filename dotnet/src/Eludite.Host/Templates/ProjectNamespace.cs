using System.Text;
using System.Xml;
using System.Xml.Linq;
using Eludite.Host.Rpc;

namespace Eludite.Host.Templates;

/// <summary>
/// The namespace Visual Studio gives a new item (brief 0072): the project's <c>RootNamespace</c> (read from the project
/// file's XML, no evaluation; else the project file's name) plus each folder below the project, every folder segment
/// made an identifier (characters other than letters, digits and <c>_</c> to <c>_</c>, a leading digit prefixed with
/// <c>_</c>). A <c>RootNamespace</c> keeps its dots; the file-name fallback is made an identifier per dotted segment.
/// </summary>
public static class ProjectNamespace
{
    /// <summary><paramref name="text"/> made a C# or Visual Basic identifier.</summary>
    public static string Identifier(string text)
    {
        ArgumentNullException.ThrowIfNull(text);
        var builder = new StringBuilder(text.Length + 1);
        foreach (var c in text)
        {
            builder.Append(char.IsLetterOrDigit(c) || c == '_' ? c : '_');
        }

        if (builder.Length == 0 || char.IsDigit(builder[0]))
        {
            builder.Insert(0, '_');
        }

        return builder.ToString();
    }

    /// <summary>The project's <c>RootNamespace</c> as written (when it holds no MSBuild expression), else its file name made an identifier.</summary>
    public static string RootNamespace(string projectPath)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        try
        {
            var root = XDocument.Load(projectPath).Root;
            var value = root?.Descendants().Where(e => e.Name.LocalName == "RootNamespace")
                .Select(e => e.Value.Trim())
                .LastOrDefault(v => v.Length > 0 && !v.Contains("$(", StringComparison.Ordinal));
            if (value is not null)
            {
                return value;
            }
        }
        catch (Exception ex) when (ex is IOException or XmlException or UnauthorizedAccessException)
        {
        }

        return string.Join('.', Path.GetFileNameWithoutExtension(projectPath).Split('.').Select(Identifier));
    }

    /// <summary>The folder segments of <paramref name="folder"/> below the project's folder, each made an identifier.</summary>
    public static IReadOnlyList<string> FolderSegments(string projectPath, string folder)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        ArgumentNullException.ThrowIfNull(folder);
        var projectDir = Path.GetDirectoryName(Path.GetFullPath(projectPath))!;
        var relative = Path.GetRelativePath(projectDir, Path.GetFullPath(folder));
        if (relative == "." || relative.StartsWith("..", StringComparison.Ordinal) || Path.IsPathRooted(relative))
        {
            return [];
        }

        return relative.Split([Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar], StringSplitOptions.RemoveEmptyEntries)
            .Select(Identifier)
            .ToList();
    }

    /// <summary><see cref="RootNamespace"/> plus <see cref="FolderSegments"/>, joined with dots.</summary>
    public static string For(string projectPath, string folder) =>
        string.Join('.', new[] { RootNamespace(projectPath) }.Concat(FolderSegments(projectPath, folder)));

    /// <summary>True when <paramref name="folder"/> is the project's folder or below it.</summary>
    public static bool IsInProject(string projectPath, string folder)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        ArgumentNullException.ThrowIfNull(folder);
        var projectDir = Path.GetDirectoryName(Path.GetFullPath(projectPath))!;
        var relative = Path.GetRelativePath(projectDir, Path.GetFullPath(folder));
        return relative == "." || (!relative.StartsWith("..", StringComparison.Ordinal) && !Path.IsPathRooted(relative));
    }

    /// <summary>-32602 unless <paramref name="path"/> is absolute.</summary>
    internal static string Absolute(string? path, string what)
    {
        if (string.IsNullOrEmpty(path) || !Path.IsPathRooted(path))
        {
            throw HostErrors.BadParams($"{what} (an absolute path) is required");
        }

        return Path.GetFullPath(path);
    }
}
