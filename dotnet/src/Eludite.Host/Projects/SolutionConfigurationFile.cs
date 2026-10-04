using System.Text;
using System.Text.RegularExpressions;
using System.Xml;
using System.Xml.Linq;
using Eludite.Host.Rpc;

namespace Eludite.Host.Projects;

/// <summary>
/// The solution's configurations and platforms and Configuration Manager's mapping (brief 0049), read from and written
/// to the solution file with its formatting kept: a <c>.sln</c>'s <c>SolutionConfigurationPlatforms</c> and
/// <c>ProjectConfigurationPlatforms</c> sections are edited line by line (tabs and line endings kept), a <c>.slnx</c>
/// through <see cref="XDocument"/> with whitespace preserved (only the project's rule elements change). A project file
/// opened as the solution maps each of its configurations to itself.
/// </summary>
public static partial class SolutionConfigurationFile
{
    private const string SolutionFolderType = "2150E333-8FDC-42A3-9474-1A3956D46DE8";

    [GeneratedRegex("""^Project\("\{(?<type>[^}]+)\}"\)\s*=\s*"(?<name>[^"]*)",\s*"(?<path>[^"]+)"\s*,\s*"\{(?<guid>[^}]+)\}"\s*$""", RegexOptions.IgnoreCase)]
    private static partial Regex SlnProject();

    /// <summary>The solution's lists and mapping (without the generation and the selection).</summary>
    public static SolutionConfigurationsResult Read(string path)
    {
        var full = Path.GetFullPath(path);
        return Path.GetExtension(full).ToLowerInvariant() switch
        {
            ".sln" => ReadSln(full, File.ReadAllText(full)),
            ".slnx" => ReadSlnx(full, Parse(File.ReadAllText(full))),
            _ => ReadProject(full),
        };
    }

    /// <summary>Applies <paramref name="edits"/> and writes the file when it changed. Returns whether it did.</summary>
    public static bool Edit(string path, IReadOnlyList<MappingEdit> edits)
    {
        ArgumentNullException.ThrowIfNull(edits);
        var full = Path.GetFullPath(path);
        var ext = Path.GetExtension(full).ToLowerInvariant();
        if (ext is not ".sln" and not ".slnx")
        {
            throw HostErrors.BadParams($"{full} is a project file: it has no solution mapping to edit");
        }

        var bytes = File.ReadAllBytes(full);
        var format = TextFileFormat.Detect(bytes, out var text);
        var current = ext == ".sln" ? ReadSln(full, text) : ReadSlnx(full, Parse(text));
        foreach (var e in edits)
        {
            Validate(current, e);
        }

        var serialized = ext == ".sln" ? EditSln(current, text, edits) : EditSlnx(current, text, edits);
        var written = format.Encode(serialized);
        if (written.AsSpan().SequenceEqual(bytes))
        {
            return false;
        }

        TextFileFormat.WriteAtomically(full, written);
        return true;
    }

    private static void Validate(SolutionConfigurationsResult current, MappingEdit e)
    {
        if (string.IsNullOrEmpty(e.Project) || string.IsNullOrEmpty(e.SolutionConfiguration) || string.IsNullOrEmpty(e.SolutionPlatform))
        {
            throw HostErrors.BadParams("a mapping edit needs project, solutionConfiguration and solutionPlatform");
        }

        if (Find(current, e.Project) is null)
        {
            throw HostErrors.BadParams($"{e.Project} is not a project of {current.Path}");
        }

        if (!current.Configurations.Contains(e.SolutionConfiguration, StringComparer.OrdinalIgnoreCase)
            || !current.Platforms.Contains(e.SolutionPlatform, StringComparer.OrdinalIgnoreCase))
        {
            throw HostErrors.BadParams($"{e.SolutionConfiguration}|{e.SolutionPlatform} is not a configuration and platform of {current.Path}");
        }
    }

    private static SolutionProjectConfigurations? Find(SolutionConfigurationsResult s, string project)
    {
        var full = Path.GetFullPath(project);
        return s.Projects.FirstOrDefault(p => string.Equals(p.Path, full, OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal));
    }

    /// <summary>The solution file's spelling of a project platform: <c>Any CPU</c> for MSBuild's <c>AnyCPU</c>.</summary>
    public static string SolutionPlatform(string platform) =>
        ConfigurationCondition.ProjectPlatform(platform) == "AnyCPU" ? "Any CPU" : platform;

    /// <summary>A project's own configurations and platforms, read from its file's text (no evaluation).</summary>
    internal static (List<string> Configurations, List<string> Platforms) ProjectLists(string projectPath)
    {
        string text;
        try
        {
            text = File.ReadAllText(projectPath);
        }
        catch (IOException)
        {
            return (["Debug", "Release"], ["Any CPU"]);
        }

        static List<string> Element(string text, string name)
        {
            var m = Regex.Match(text, $"<{name}>([^<]*)</{name}>");
            return m.Success ? ProjectPropertyEvaluator.SplitList(m.Groups[1].Value) : [];
        }

        var configurations = Element(text, "Configurations");
        var platforms = Element(text, "Platforms").Select(SolutionPlatform).ToList();
        return (configurations.Count > 0 ? configurations : ["Debug", "Release"], platforms.Count > 0 ? platforms : ["Any CPU"]);
    }

    private static SolutionConfigurationsResult ReadProject(string full)
    {
        var (configurations, platforms) = ProjectLists(full);
        var mappings = configurations.SelectMany(c => platforms.Select(p => new ConfigurationMapping(c, p, c, p, true))).ToList();
        return new SolutionConfigurationsResult(0, full, configurations, platforms, new Selection(configurations[0], platforms[0]),
            [new SolutionProjectConfigurations(Path.GetFileNameWithoutExtension(full), full, configurations, platforms, mappings)])
        {
            Format = "project",
        };
    }

    // ------------------------------------------------------------------ .sln

    private sealed record SlnProjectLine(string Name, string Path, string Guid);

    private static List<SlnProjectLine> SlnProjects(string full, string text)
    {
        var dir = Path.GetDirectoryName(full)!;
        var list = new List<SlnProjectLine>();
        foreach (var line in text.Split('\n'))
        {
            var m = SlnProject().Match(line.TrimEnd('\r'));
            if (!m.Success || string.Equals(m.Groups["type"].Value, SolutionFolderType, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            var rel = m.Groups["path"].Value;
            if (!rel.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase) && !rel.EndsWith(".vbproj", StringComparison.OrdinalIgnoreCase)
                && !rel.EndsWith(".fsproj", StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            list.Add(new SlnProjectLine(m.Groups["name"].Value, Path.GetFullPath(Path.Combine(dir, rel.Replace('\\', Path.DirectorySeparatorChar))), m.Groups["guid"].Value));
        }

        return list;
    }

    /// <summary>The lines of a section (between <c>GlobalSection(name)</c> and <c>EndGlobalSection</c>), as indexes.</summary>
    private static (int Start, int End) Section(IReadOnlyList<string> lines, string name)
    {
        var start = -1;
        for (var i = 0; i < lines.Count; i++)
        {
            var l = lines[i].Trim();
            if (start < 0 && l.StartsWith($"GlobalSection({name})", StringComparison.Ordinal))
            {
                start = i;
            }
            else if (start >= 0 && l.StartsWith("EndGlobalSection", StringComparison.Ordinal))
            {
                return (start, i);
            }
        }

        return (-1, -1);
    }

    private sealed record SlnCfgLine(string Guid, string Solution, string Kind, string Value);

    /// <summary><c>{GUID}.Debug|Any CPU.ActiveCfg = Debug|Any CPU</c>; Kind is ActiveCfg, Build.0 or Deploy.0.</summary>
    private static SlnCfgLine? ParseCfg(string line)
    {
        var l = line.Trim();
        var eq = l.IndexOf(" = ", StringComparison.Ordinal);
        if (!l.StartsWith('{') || eq < 0)
        {
            return null;
        }

        var key = l[..eq];
        var value = l[(eq + 3)..].Trim();
        var close = key.IndexOf("}.", StringComparison.Ordinal);
        if (close < 0)
        {
            return null;
        }

        var guid = key[1..close];
        var rest = key[(close + 2)..];
        foreach (var kind in new[] { ".ActiveCfg", ".Build.0", ".Deploy.0" })
        {
            if (rest.EndsWith(kind, StringComparison.Ordinal))
            {
                return new SlnCfgLine(guid, rest[..^kind.Length], kind[1..], value);
            }
        }

        return null;
    }

    private static (string Configuration, string Platform) Pair(string value)
    {
        var bar = value.IndexOf('|', StringComparison.Ordinal);
        return bar < 0 ? (value.Trim(), "Any CPU") : (value[..bar].Trim(), value[(bar + 1)..].Trim());
    }

    private static SolutionConfigurationsResult ReadSln(string full, string text)
    {
        var lines = text.Split('\n').Select(l => l.TrimEnd('\r')).ToList();
        var configurations = new List<string>();
        var platforms = new List<string>();
        var (s, e) = Section(lines, "SolutionConfigurationPlatforms");
        for (var i = s + 1; s >= 0 && i < e; i++)
        {
            var left = lines[i].Split('=')[0].Trim();
            if (left.Length == 0)
            {
                continue;
            }

            var (c, p) = Pair(left);
            if (!configurations.Contains(c, StringComparer.OrdinalIgnoreCase))
            {
                configurations.Add(c);
            }

            if (!platforms.Contains(p, StringComparer.OrdinalIgnoreCase))
            {
                platforms.Add(p);
            }
        }

        var cfg = new List<SlnCfgLine>();
        var (ps, pe) = Section(lines, "ProjectConfigurationPlatforms");
        for (var i = ps + 1; ps >= 0 && i < pe; i++)
        {
            if (ParseCfg(lines[i]) is { } c)
            {
                cfg.Add(c);
            }
        }

        var projects = new List<SolutionProjectConfigurations>();
        foreach (var p in SlnProjects(full, text))
        {
            var mine = cfg.Where(c => string.Equals(c.Guid, p.Guid, StringComparison.OrdinalIgnoreCase)).ToList();
            var mappings = new List<ConfigurationMapping>();
            foreach (var sc in configurations)
            {
                foreach (var sp in platforms)
                {
                    var solution = $"{sc}|{sp}";
                    var active = mine.FirstOrDefault(c => c.Kind == "ActiveCfg" && string.Equals(c.Solution, solution, StringComparison.OrdinalIgnoreCase));
                    if (active is null)
                    {
                        continue;
                    }

                    var (pc, pp) = Pair(active.Value);
                    var build = mine.Any(c => c.Kind == "Build.0" && string.Equals(c.Solution, solution, StringComparison.OrdinalIgnoreCase));
                    var deploy = mine.Any(c => c.Kind == "Deploy.0" && string.Equals(c.Solution, solution, StringComparison.OrdinalIgnoreCase));
                    mappings.Add(new ConfigurationMapping(sc, sp, pc, pp, build) { Deploy = deploy });
                }
            }

            var (pcs, pps) = ProjectLists(p.Path);
            projects.Add(new SolutionProjectConfigurations(p.Name, p.Path, Merge(pcs, mappings.Select(m => m.Configuration)), Merge(pps, mappings.Select(m => m.Platform)), mappings));
        }

        if (configurations.Count == 0)
        {
            configurations = ["Debug", "Release"];
        }

        if (platforms.Count == 0)
        {
            platforms = ["Any CPU"];
        }

        return new SolutionConfigurationsResult(0, full, configurations, platforms, new Selection(configurations[0], platforms[0]), projects) { Format = "sln" };
    }

    private static List<string> Merge(IEnumerable<string> a, IEnumerable<string> b)
    {
        var list = new List<string>();
        foreach (var x in a.Concat(b))
        {
            if (!list.Contains(x, StringComparer.OrdinalIgnoreCase))
            {
                list.Add(x);
            }
        }

        return list;
    }

    private static string EditSln(SolutionConfigurationsResult current, string text, IReadOnlyList<MappingEdit> edits)
    {
        // Lines with their own terminators, so an edited file keeps every line ending.
        var lines = Regex.Split(text, "(?<=\n)").Where(l => l.Length > 0).ToList();
        var guids = SlnProjects(current.Path!, text).ToDictionary(p => p.Path, p => p.Guid, OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal);
        var nl = text.Contains("\r\n", StringComparison.Ordinal) ? "\r\n" : "\n";
        foreach (var e in edits)
        {
            var project = Find(current, e.Project!)!;
            var guid = guids[project.Path];
            var solution = $"{Exact(current.Configurations, e.SolutionConfiguration!)}|{Exact(current.Platforms, e.SolutionPlatform!)}";
            var (ps, pe) = Section([.. lines.Select(l => l.TrimEnd('\r', '\n'))], "ProjectConfigurationPlatforms");
            if (ps < 0)
            {
                throw HostErrors.BadParams($"{current.Path} has no ProjectConfigurationPlatforms section");
            }

            int Index(string kind)
            {
                for (var i = ps + 1; i < pe; i++)
                {
                    if (ParseCfg(lines[i]) is { } c && c.Kind == kind && string.Equals(c.Guid, guid, StringComparison.OrdinalIgnoreCase)
                        && string.Equals(c.Solution, solution, StringComparison.OrdinalIgnoreCase))
                    {
                        return i;
                    }
                }

                return -1;
            }

            var indent = ps + 1 < pe ? Regex.Match(lines[ps + 1], @"^\s*").Value.TrimEnd('\r', '\n') : "\t\t";
            var activeIx = Index("ActiveCfg");
            var (oldCfg, oldPlat) = activeIx >= 0 ? Pair(ParseCfg(lines[activeIx])!.Value) : Pair(solution);
            var value = $"{e.Configuration ?? oldCfg}|{SolutionPlatform(e.Platform ?? oldPlat)}";
            var line = (string kind) => $"{indent}{{{guid}}}.{solution}.{kind} = {value}{nl}";
            var inserted = activeIx < 0;
            if (inserted)
            {
                lines.Insert(pe, line("ActiveCfg"));
                activeIx = pe;
                pe++;
            }
            else
            {
                lines[activeIx] = Replace(lines[activeIx], value);
            }

            var buildIx = Index("Build.0");
            var build = e.Build ?? (buildIx >= 0 || inserted);
            if (build && buildIx >= 0)
            {
                lines[buildIx] = Replace(lines[buildIx], value);
            }
            else if (build)
            {
                lines.Insert(activeIx + 1, line("Build.0"));
            }
            else if (buildIx >= 0)
            {
                lines.RemoveAt(buildIx);
            }
        }

        return string.Concat(lines);
    }

    /// <summary>A <c>key = value</c> line with a new value, its indentation and terminator kept.</summary>
    private static string Replace(string line, string value)
    {
        var eq = line.IndexOf(" = ", StringComparison.Ordinal);
        var end = line.EndsWith("\r\n", StringComparison.Ordinal) ? "\r\n" : line.EndsWith('\n') ? "\n" : string.Empty;
        return line[..(eq + 3)] + value + end;
    }

    private static string Exact(IReadOnlyList<string> list, string value) =>
        list.FirstOrDefault(x => string.Equals(x, value, StringComparison.OrdinalIgnoreCase)) ?? value;

    // ------------------------------------------------------------------ .slnx

    private static XDocument Parse(string text) =>
        XDocument.Parse(text, LoadOptions.PreserveWhitespace | LoadOptions.SetLineInfo);

    private static IEnumerable<XElement> SlnxProjects(XDocument doc) =>
        doc.Root?.Descendants("Project").Where(p => (string?)p.Attribute("Path") is { } path
            && (path.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase) || path.EndsWith(".vbproj", StringComparison.OrdinalIgnoreCase)
                || path.EndsWith(".fsproj", StringComparison.OrdinalIgnoreCase))) ?? [];

    private static string ProjectPathOf(string solution, XElement project) =>
        Path.GetFullPath(Path.Combine(Path.GetDirectoryName(solution)!, ((string)project.Attribute("Path")!).Replace('\\', Path.DirectorySeparatorChar)));

    /// <summary>Whether a rule's <c>Solution="Release|*"</c> applies to a configuration and platform.</summary>
    private static bool RuleApplies(XElement rule, string configuration, string platform)
    {
        var solution = (string?)rule.Attribute("Solution") ?? "*|*";
        var bar = solution.IndexOf('|', StringComparison.Ordinal);
        var c = bar < 0 ? solution : solution[..bar];
        var p = bar < 0 ? "*" : solution[(bar + 1)..];
        return (c == "*" || string.Equals(c, configuration, StringComparison.OrdinalIgnoreCase))
            && (p == "*" || string.Equals(ConfigurationCondition.ProjectPlatform(p), ConfigurationCondition.ProjectPlatform(platform), StringComparison.OrdinalIgnoreCase));
    }

    private static ConfigurationMapping SlnxMapping(XElement project, IReadOnlyList<string> projectPlatforms, string sc, string sp, XElement? without = null)
    {
        var rules = project.Elements().Where(r => r != without).ToList();
        var configuration = sc;
        var platform = projectPlatforms.Any(p => string.Equals(ConfigurationCondition.ProjectPlatform(p), ConfigurationCondition.ProjectPlatform(sp), StringComparison.OrdinalIgnoreCase))
            ? sp
            : "Any CPU";
        var build = true;
        var deploy = false;
        foreach (var r in rules.Where(r => RuleApplies(r, sc, sp)))
        {
            var value = (string?)r.Attribute("Project");
            switch (r.Name.LocalName)
            {
                case "BuildType" when value is not null:
                    configuration = value;
                    break;
                case "Platform" when value is not null:
                    platform = value;
                    break;
                case "Build":
                    build = !string.Equals(value, "false", StringComparison.OrdinalIgnoreCase);
                    break;
                case "Deploy":
                    deploy = !string.Equals(value, "false", StringComparison.OrdinalIgnoreCase);
                    break;
            }
        }

        return new ConfigurationMapping(sc, sp, configuration, platform, build) { Deploy = deploy };
    }

    private static SolutionConfigurationsResult ReadSlnx(string full, XDocument doc)
    {
        var configs = doc.Root?.Element("Configurations");
        var configurations = configs?.Elements("BuildType").Select(e => (string?)e.Attribute("Name")).OfType<string>().ToList() ?? [];
        var platforms = configs?.Elements("Platform").Select(e => (string?)e.Attribute("Name")).OfType<string>().ToList() ?? [];
        if (configurations.Count == 0)
        {
            configurations = ["Debug", "Release"];
        }

        if (platforms.Count == 0)
        {
            platforms = ["Any CPU"];
        }

        var projects = new List<SolutionProjectConfigurations>();
        foreach (var p in SlnxProjects(doc))
        {
            var path = ProjectPathOf(full, p);
            var (pcs, pps) = ProjectLists(path);
            var mappings = configurations.SelectMany(c => platforms.Select(pl => SlnxMapping(p, pps, c, pl))).ToList();
            projects.Add(new SolutionProjectConfigurations(Path.GetFileNameWithoutExtension(path), path, Merge(pcs, mappings.Select(m => m.Configuration)), Merge(pps, mappings.Select(m => m.Platform)), mappings));
        }

        return new SolutionConfigurationsResult(0, full, configurations, platforms, new Selection(configurations[0], platforms[0]), projects) { Format = "slnx" };
    }

    private static string EditSlnx(SolutionConfigurationsResult current, string text, IReadOnlyList<MappingEdit> edits)
    {
        var doc = Parse(text);
        var unit = Unit(doc);
        foreach (var e in edits)
        {
            var target = Find(current, e.Project!)!;
            var project = SlnxProjects(doc).First(p => string.Equals(ProjectPathOf(current.Path!, p), target.Path, StringComparison.Ordinal));
            var sc = Exact(current.Configurations, e.SolutionConfiguration!);
            var sp = Exact(current.Platforms, e.SolutionPlatform!);
            var solution = $"{sc}|{sp}";
            if (e.Configuration is { } c)
            {
                SetRule(project, "BuildType", solution, c, m => m.Configuration, target.Platforms, sc, sp, unit);
            }

            if (e.Platform is { } pl)
            {
                SetRule(project, "Platform", solution, SolutionPlatform(pl), m => m.Platform, target.Platforms, sc, sp, unit);
            }

            if (e.Build is { } b)
            {
                SetRule(project, "Build", solution, b ? "true" : "false", m => m.Build ? "true" : "false", target.Platforms, sc, sp, unit);
            }
        }

        using var sw = new StringWriter(System.Globalization.CultureInfo.InvariantCulture);
        using (var w = XmlWriter.Create(sw, new XmlWriterSettings { OmitXmlDeclaration = doc.Declaration is null, Indent = false, NewLineHandling = NewLineHandling.None, Encoding = new UTF8Encoding(false) }))
        {
            doc.Save(w);
        }

        return sw.ToString();
    }

    /// <summary>
    /// Sets the project's exact rule <c>&lt;{name} Solution="{solution}" Project="{value}" /&gt;</c>, or removes it when
    /// the mapping without it already gives <paramref name="value"/>.
    /// </summary>
    private static void SetRule(XElement project, string name, string solution, string value, Func<ConfigurationMapping, string> read, IReadOnlyList<string> platforms, string sc, string sp, string unit)
    {
        var exact = project.Elements(name).LastOrDefault(r => string.Equals((string?)r.Attribute("Solution"), solution, StringComparison.OrdinalIgnoreCase));
        var without = read(SlnxMapping(project, platforms, sc, sp, exact));
        if (string.Equals(without, value, StringComparison.OrdinalIgnoreCase))
        {
            if (exact is not null)
            {
                RemoveRule(project, exact);
            }

            return;
        }

        if (exact is not null)
        {
            exact.SetAttributeValue("Project", value);
            return;
        }

        var rule = new XElement(name, new XAttribute("Solution", solution), new XAttribute("Project", value));
        var indent = IndentOf(project);
        if (project.Elements().LastOrDefault() is { } last)
        {
            last.AddAfterSelf(new XText("\n" + indent + unit), rule);
        }
        else
        {
            project.RemoveNodes();
            project.Add(new XText("\n" + indent + unit), rule, new XText("\n" + indent));
        }
    }

    private static void RemoveRule(XElement project, XElement rule)
    {
        if (rule.PreviousNode is XText { } ws && string.IsNullOrWhiteSpace(ws.Value))
        {
            ws.Remove();
        }

        rule.Remove();
        if (!project.Elements().Any())
        {
            project.RemoveNodes();
        }
    }

    /// <summary>The whitespace before an element on its line.</summary>
    private static string IndentOf(XElement e) =>
        e.PreviousNode is XText t && t.Value.LastIndexOf('\n') is var nl and >= 0 ? t.Value[(nl + 1)..] : string.Empty;

    /// <summary>The file's indentation step: the root's first child's indentation (2 spaces by default).</summary>
    private static string Unit(XDocument doc)
    {
        var first = doc.Root?.Elements().FirstOrDefault();
        var indent = first is null ? string.Empty : IndentOf(first);
        return indent.Length > 0 ? indent : "  ";
    }
}
