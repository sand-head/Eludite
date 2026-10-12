using System.Globalization;
using System.Xml;
using System.Xml.Linq;

namespace Eludite.Host.Testing;

/// <summary>
/// Brief 0076: Cobertura reports (Microsoft.Testing.Extensions.CodeCoverage's, Microsoft.CodeCoverage's and coverlet's)
/// read and merged into the <c>coverage</c> update's shape. A report is
/// <c>coverage/sources/source</c> and <c>coverage/packages/package(name)/classes/class(name, filename)/methods/method(name,
/// signature)/lines/line(number, hits, branch, condition-coverage)</c>; a relative <c>filename</c> is resolved against the
/// sources (the first under which the file exists, else the first source). Several reports merge by file and line: hits
/// are summed, branch counts take the larger. Packages are modules (merged by name), classes merge by name within their
/// module; a class's and a method's counts are over their distinct coverable lines, a module's over its classes' lines and
/// the totals over every file's lines, all read from the merged lines.
/// </summary>
public static class Cobertura
{
    /// <summary>One report as read: its classes and the lines each covers.</summary>
    public sealed class Report
    {
        internal List<ClassEntry> Classes { get; } = [];
    }

    internal sealed record LineEntry(string File, int Number, long Hits, int? BranchesCovered, int? BranchesValid);

    internal sealed record MethodEntry(string Name, string? Signature, List<LineEntry> Lines);

    internal sealed record ClassEntry(string Module, string Name, string? File, List<MethodEntry> Methods, List<LineEntry> Lines);

    /// <summary>Reads the report at <paramref name="path"/>.</summary>
    /// <exception cref="IOException">The file cannot be read.</exception>
    /// <exception cref="XmlException">It is not well-formed XML.</exception>
    /// <exception cref="InvalidDataException">It is not a Cobertura report.</exception>
    public static Report Read(string path)
    {
        ArgumentNullException.ThrowIfNull(path);
        using var stream = File.OpenRead(path);
        return Parse(XDocument.Load(stream), Path.GetDirectoryName(Path.GetFullPath(path)));
    }

    /// <summary>Parses the report text <paramref name="xml"/>; relative file names without sources resolve against <paramref name="directory"/>.</summary>
    /// <exception cref="XmlException">It is not well-formed XML.</exception>
    /// <exception cref="InvalidDataException">It is not a Cobertura report.</exception>
    public static Report Parse(string xml, string? directory = null)
    {
        ArgumentNullException.ThrowIfNull(xml);
        return Parse(XDocument.Parse(xml), directory);
    }

    /// <summary>The merged coverage of <paramref name="reports"/> with the run's <paramref name="containers"/>.</summary>
    public static TestCoverageResult Merge(IEnumerable<Report> reports, IReadOnlyList<CoverageContainerReport> containers)
    {
        ArgumentNullException.ThrowIfNull(reports);
        ArgumentNullException.ThrowIfNull(containers);

        // Every line of every report, merged by file and line.
        var lines = new Dictionary<(string File, int Line), Merged>();
        var classes = new Dictionary<(string Module, string Name), ClassAcc>();
        foreach (var report in reports)
        {
            foreach (var cls in report.Classes)
            {
                // A line can be listed under the class and under its method: it counts once per report.
                foreach (var line in cls.Lines.Concat(cls.Methods.SelectMany(m => m.Lines)).DistinctBy(l => (l.File, l.Number)))
                {
                    var key = (line.File, line.Number);
                    lines[key] = lines.TryGetValue(key, out var m) ? m.With(line) : new Merged(line.Hits, line.BranchesCovered, line.BranchesValid);
                }

                if (!classes.TryGetValue((cls.Module, cls.Name), out var acc))
                {
                    acc = new ClassAcc(cls.File);
                    classes[(cls.Module, cls.Name)] = acc;
                }

                acc.File ??= cls.File;
                foreach (var line in cls.Lines.Concat(cls.Methods.SelectMany(m => m.Lines)))
                {
                    acc.Lines.Add((line.File, line.Number));
                }

                foreach (var method in cls.Methods)
                {
                    var methodKey = (method.Name, method.Signature ?? string.Empty);
                    if (!acc.Methods.TryGetValue(methodKey, out var methodLines))
                    {
                        methodLines = [];
                        acc.Methods[methodKey] = methodLines;
                    }

                    foreach (var line in method.Lines)
                    {
                        methodLines.Add((line.File, line.Number));
                    }
                }
            }
        }

        var modules = classes
            .GroupBy(c => c.Key.Module, StringComparer.Ordinal)
            .OrderBy(g => g.Key, StringComparer.Ordinal)
            .Select(g =>
            {
                var moduleLines = new HashSet<(string, int)>();
                var moduleClasses = new List<CoverageClass>();
                foreach (var (key, acc) in g.OrderBy(c => c.Key.Name, StringComparer.Ordinal))
                {
                    moduleLines.UnionWith(acc.Lines);
                    var classCounts = Count(acc.Lines, lines);
                    var methods = acc.Methods
                        .Select(m =>
                        {
                            var counts = Count(m.Value, lines);
                            return new CoverageMethod(m.Key.Name, counts.Covered, counts.Valid)
                            {
                                Signature = m.Key.Signature.Length > 0 ? m.Key.Signature : null,
                                Line = m.Value.Count > 0 ? m.Value.Min(l => l.Line) : null,
                                BranchesCovered = counts.BranchesCovered,
                                BranchesValid = counts.BranchesValid,
                            };
                        })
                        .OrderBy(m => m.Line ?? int.MaxValue)
                        .ThenBy(m => m.Name, StringComparer.Ordinal)
                        .ToList();
                    moduleClasses.Add(new CoverageClass(key.Name, classCounts.Covered, classCounts.Valid, methods)
                    {
                        Namespace = NamespaceOf(key.Name),
                        File = acc.File,
                        BranchesCovered = classCounts.BranchesCovered,
                        BranchesValid = classCounts.BranchesValid,
                    });
                }

                var moduleCounts = Count(moduleLines, lines);
                return new CoverageModule(g.Key, moduleCounts.Covered, moduleCounts.Valid, moduleClasses)
                {
                    BranchesCovered = moduleCounts.BranchesCovered,
                    BranchesValid = moduleCounts.BranchesValid,
                };
            })
            .ToList();

        var files = lines
            .GroupBy(l => l.Key.File, StringComparer.Ordinal)
            .OrderBy(g => g.Key, StringComparer.Ordinal)
            .Select(g => new CoverageFile(g.Key, g
                .OrderBy(l => l.Key.Line)
                .Select(l => (IReadOnlyList<long>)(l.Value.BranchesValid is { } valid
                    ? [l.Key.Line, Clamp(l.Value.Hits), l.Value.BranchesCovered ?? 0, valid]
                    : [l.Key.Line, Clamp(l.Value.Hits)]))
                .ToList()))
            .ToList();
        var total = Count(lines.Keys, lines);
        return new TestCoverageResult(total.Covered, total.Valid, containers, modules, files)
        {
            BranchesCovered = total.BranchesCovered,
            BranchesValid = total.BranchesValid,
        };
    }

    /// <summary><c>N.S.Class</c> to <c>N.S</c> (nested and generated names, <c>N.Outer/Inner</c>, <c>N.C&lt;M&gt;d__0</c>, by their outer part).</summary>
    public static string? NamespaceOf(string className)
    {
        ArgumentNullException.ThrowIfNull(className);
        var end = className.IndexOfAny(['/', '<', '+', '`']);
        var outer = end < 0 ? className : className[..end];
        var dot = outer.LastIndexOf('.');
        return dot > 0 ? outer[..dot] : null;
    }

    /// <summary><c>50% (1/2)</c> to (1, 2); null when it is not that shape.</summary>
    public static (int Covered, int Valid)? ConditionCoverage(string? value)
    {
        if (value is null)
        {
            return null;
        }

        var open = value.IndexOf('(', StringComparison.Ordinal);
        var slash = value.IndexOf('/', StringComparison.Ordinal);
        var close = value.IndexOf(')', StringComparison.Ordinal);
        if (open < 0 || slash < open || close < slash)
        {
            return null;
        }

        return int.TryParse(value.AsSpan(open + 1, slash - open - 1), NumberStyles.Integer, CultureInfo.InvariantCulture, out var covered)
            && int.TryParse(value.AsSpan(slash + 1, close - slash - 1), NumberStyles.Integer, CultureInfo.InvariantCulture, out var valid)
            ? (covered, valid)
            : null;
    }

    private static long Clamp(long hits) => Math.Clamp(hits, 0, uint.MaxValue);

    private static Report Parse(XDocument document, string? directory)
    {
        var root = document.Root;
        if (root is null || root.Name.LocalName != "coverage")
        {
            throw new InvalidDataException("not a Cobertura report: the root element is not <coverage>");
        }

        var sources = root.Elements("sources").Elements("source")
            .Select(s => s.Value.Trim())
            .Where(s => s.Length > 0)
            .ToList();
        var resolved = new Dictionary<string, string>(StringComparer.Ordinal);
        string Resolve(string filename)
        {
            if (resolved.TryGetValue(filename, out var known))
            {
                return known;
            }

            var name = filename.Replace('\\', '/');
            string full;
            if (Path.IsPathRooted(name) || (name.Length > 2 && name[1] == ':'))
            {
                full = name;
            }
            else
            {
                var candidates = sources.Select(s => Path.Combine(s, name)).ToList();
                full = candidates.FirstOrDefault(File.Exists)
                    ?? candidates.FirstOrDefault()
                    ?? (directory is not null ? Path.Combine(directory, name) : name);
            }

            try
            {
                full = Path.GetFullPath(full.Replace('/', Path.DirectorySeparatorChar));
            }
            catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException)
            {
            }

            resolved[filename] = full;
            return full;
        }

        var report = new Report();
        foreach (var package in root.Elements("packages").Elements("package"))
        {
            var module = (string?)package.Attribute("name") ?? string.Empty;
            foreach (var cls in package.Elements("classes").Elements("class"))
            {
                var name = (string?)cls.Attribute("name") ?? string.Empty;
                var file = (string?)cls.Attribute("filename") is { Length: > 0 } f ? Resolve(f) : null;
                if (file is null)
                {
                    continue;
                }

                var methods = cls.Elements("methods").Elements("method")
                    .Select(m => new MethodEntry(
                        (string?)m.Attribute("name") ?? string.Empty,
                        (string?)m.Attribute("signature") is { Length: > 0 } sig ? sig : null,
                        Lines(m, file)))
                    .ToList();
                report.Classes.Add(new ClassEntry(module, name, file, methods, Lines(cls, file)));
            }
        }

        return report;
    }

    private static List<LineEntry> Lines(XElement parent, string file)
    {
        var list = new List<LineEntry>();
        foreach (var line in parent.Elements("lines").Elements("line"))
        {
            if (!int.TryParse((string?)line.Attribute("number"), NumberStyles.Integer, CultureInfo.InvariantCulture, out var number) || number <= 0)
            {
                continue;
            }

            _ = long.TryParse((string?)line.Attribute("hits"), NumberStyles.Integer, CultureInfo.InvariantCulture, out var hits);
            int? covered = null;
            int? valid = null;
            if (string.Equals((string?)line.Attribute("branch"), "true", StringComparison.OrdinalIgnoreCase)
                && ConditionCoverage((string?)line.Attribute("condition-coverage")) is { } condition)
            {
                (covered, valid) = condition;
            }

            list.Add(new LineEntry(file, number, Math.Max(0, hits), covered, valid));
        }

        return list;
    }

    private static (int Covered, int Valid, int? BranchesCovered, int? BranchesValid) Count(IEnumerable<(string File, int Line)> keys, Dictionary<(string File, int Line), Merged> lines)
    {
        int covered = 0, valid = 0;
        int? branchesCovered = null, branchesValid = null;
        foreach (var key in keys.Distinct())
        {
            if (!lines.TryGetValue(key, out var line))
            {
                continue;
            }

            valid++;
            if (line.Hits > 0)
            {
                covered++;
            }

            if (line.BranchesValid is { } v)
            {
                branchesValid = (branchesValid ?? 0) + v;
                branchesCovered = (branchesCovered ?? 0) + (line.BranchesCovered ?? 0);
            }
        }

        return (covered, valid, branchesCovered, branchesValid);
    }

    private sealed record Merged(long Hits, int? BranchesCovered, int? BranchesValid)
    {
        public Merged With(LineEntry line) => new(
            Hits + line.Hits,
            Max(BranchesCovered, line.BranchesCovered),
            Max(BranchesValid, line.BranchesValid));

        private static int? Max(int? a, int? b) => a is null ? b : b is null ? a : Math.Max(a.Value, b.Value);
    }

    private sealed class ClassAcc(string? file)
    {
        public string? File { get; set; } = file;

        public HashSet<(string File, int Line)> Lines { get; } = [];

        public Dictionary<(string Name, string Signature), HashSet<(string File, int Line)>> Methods { get; } = [];
    }
}
