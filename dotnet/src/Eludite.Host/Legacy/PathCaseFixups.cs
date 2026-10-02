namespace Niello.Host.Legacy;

/// <summary>A Compile item whose declared path differs only in letter case from the file on disk.</summary>
public sealed record PathCaseFixup(string Declared, string OnDisk);

/// <summary>
/// Legacy projects are written on Windows, where paths are case-insensitive. On a case-sensitive file system a
/// <c>&lt;Compile Include="Foo\BarBase.cs" /&gt;</c> for <c>BarBase.cs</c> stored as <c>Barbase.cs</c> silently drops
/// the file, and everything that uses its types breaks (brief 0003: Umbraco 7). This finds the file on disk so the
/// design-time inputs can be corrected; the project file itself is never edited.
/// </summary>
public static class PathCaseFixups
{
    /// <summary>Fixups for every Compile item that does not exist as written but exists with different casing.</summary>
    public static IReadOnlyList<PathCaseFixup> Find(LegacyProjectEvaluation evaluation)
    {
        ArgumentNullException.ThrowIfNull(evaluation);
        if (OperatingSystem.IsWindows() || OperatingSystem.IsMacOS())
        {
            return [];
        }

        var result = new List<PathCaseFixup>();
        foreach (var item in evaluation.CompileItems)
        {
            if (!File.Exists(item) && Resolve(item) is { } onDisk)
            {
                result.Add(new PathCaseFixup(item, onDisk));
            }
        }

        return result;
    }

    /// <summary>
    /// Returns the existing file whose path equals <paramref name="path"/> ignoring case, segment by segment, or null
    /// when there is none (or more than one candidate at some level, which is ambiguous).
    /// </summary>
    public static string? Resolve(string path)
    {
        ArgumentNullException.ThrowIfNull(path);
        if (File.Exists(path))
        {
            return path;
        }

        var full = Path.GetFullPath(path);
        var root = Path.GetPathRoot(full)!;
        var current = root;
        var segments = full[root.Length..].Split(Path.DirectorySeparatorChar, StringSplitOptions.RemoveEmptyEntries);
        for (var i = 0; i < segments.Length; i++)
        {
            var exact = Path.Combine(current, segments[i]);
            var last = i == segments.Length - 1;
            if (last ? File.Exists(exact) : Directory.Exists(exact))
            {
                current = exact;
                continue;
            }

            if (!Directory.Exists(current))
            {
                return null;
            }

            var candidates = (last ? Directory.EnumerateFiles(current) : Directory.EnumerateDirectories(current))
                .Where(c => string.Equals(Path.GetFileName(c), segments[i], StringComparison.OrdinalIgnoreCase))
                .Take(2)
                .ToList();
            if (candidates.Count != 1)
            {
                return null;
            }

            current = candidates[0];
        }

        return current;
    }
}
