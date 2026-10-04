using System.Text;
using System.Xml;
using Eludite.Host.Rpc;
using Microsoft.Build.Construction;
using Microsoft.Build.Evaluation;

namespace Eludite.Host.Projects;

/// <summary>What <see cref="ProjectPropertyEvaluator.Edit"/> did: whether the file changed and one result per edit.</summary>
public sealed record ProjectEditOutcome(bool Written, IReadOnlyList<PropertyEditResult> Results);

/// <summary>
/// The formatting-preserving edit of a project file (brief 0049) on MSBuild's construction model
/// (<c>Microsoft.Build.Construction</c>, <c>preserveFormatting</c>): the condition rule, the default rule and the
/// inherited rule of host-rpc.md "Project properties". The file's own bytes outside the changed elements are kept: it
/// is parsed from its text, saved through <see cref="ProjectRootElement.Save(TextWriter)"/>, and laid out again with
/// <see cref="TextFileFormat"/> (byte order mark, line endings, declaration, trailing newlines).
/// </summary>
/// <remarks>
/// Brief 0048's NuGet edit of <c>PackageReference</c> items is the same kind of edit; the two should share one helper
/// once both are merged (this file is brief 0049's, written before 0048's landed).
/// </remarks>
internal sealed class ProjectFileEditor
{
    internal const ProjectLoadSettings LoadSettings =
        ProjectLoadSettings.IgnoreMissingImports | ProjectLoadSettings.IgnoreEmptyImports | ProjectLoadSettings.IgnoreInvalidImports;

    private readonly string _path;
    private readonly ProjectCollection _collection;
    private readonly Func<string?, string?, string?, Dictionary<string, string>> _globals;

    public ProjectFileEditor(string path, ProjectCollection collection, Func<string?, string?, string?, Dictionary<string, string>> globals)
    {
        _path = path;
        _collection = collection;
        _globals = globals;
    }

    public ProjectEditOutcome Apply(IReadOnlyList<PropertyEdit> edits, CancellationToken cancellationToken)
    {
        var bytes = File.ReadAllBytes(_path);
        var format = TextFileFormat.Detect(bytes, out var text);
        var root = Parse(text, _collection, preserveFormatting: true);
        root.FullPath = _path;
        var results = new List<PropertyEditResult>(edits.Count);
        foreach (var edit in edits)
        {
            cancellationToken.ThrowIfCancellationRequested();
            results.Add(ApplyOne(root, edit));
        }

        if (!results.Any(r => r.Status is "written" or "removed"))
        {
            return new ProjectEditOutcome(false, results);
        }

        var serialized = Serialize(root);
        if (root.Count == 0 && !text.TrimEnd().EndsWith("/>", StringComparison.Ordinal) && serialized.TrimEnd().EndsWith("/>", StringComparison.Ordinal))
        {
            // The last child went: the root would be written self-closing; keep its end tag as the file had it.
            var close = serialized.LastIndexOf("/>", StringComparison.Ordinal);
            serialized = serialized[..close].TrimEnd() + ">\n</Project>" + serialized[(close + 2)..];
        }

        var written = format.Encode(serialized);
        if (written.AsSpan().SequenceEqual(bytes))
        {
            return new ProjectEditOutcome(false, [.. results.Select(r => r.Status is "written" or "removed" ? r with { Status = "unchanged" } : r)]);
        }

        cancellationToken.ThrowIfCancellationRequested();
        TextFileFormat.WriteAtomically(_path, written);
        return new ProjectEditOutcome(true, Lines(format.Encoding.GetString(written, format.Preamble.Length, written.Length - format.Preamble.Length), edits, results));
    }

    internal static ProjectRootElement Parse(string text, ProjectCollection collection, bool preserveFormatting)
    {
        using var reader = XmlReader.Create(new StringReader(text), new XmlReaderSettings { DtdProcessing = DtdProcessing.Prohibit });
        return ProjectRootElement.Create(reader, collection, preserveFormatting);
    }

    internal static string Serialize(ProjectRootElement root)
    {
        using var writer = new Utf8StringWriter();
        root.Save(writer);
        return writer.ToString();
    }

    private PropertyEditResult ApplyOne(ProjectRootElement root, PropertyEdit edit)
    {
        var entry = PropertyCatalog.Find(edit.Name)
            ?? throw HostErrors.BadParams($"{edit.Name} is not a property of the catalog (host-rpc.md, \"Project properties\")");
        if (entry.WindowsOnly && !OperatingSystem.IsWindows())
        {
            throw HostErrors.BadParams($"{entry.Name} (Win32 resources) is read-only off Windows");
        }

        var key = ConfigurationCondition.Key(edit.Configuration, edit.Platform, edit.Framework);
        if (edit.AllConfigurations && key.Count > 0)
        {
            throw HostErrors.BadParams("allConfigurations writes the unconditioned value: give no configuration, platform or framework");
        }

        if (entry.Target is { } target)
        {
            if (key.Count > 0)
            {
                throw HostErrors.BadParams($"{entry.Name} is not edited per configuration");
            }

            return SetEvent(root, entry, target, edit.Value);
        }

        var name = entry.Name;
        var removedConditions = new List<string>();
        if (edit.AllConfigurations)
        {
            foreach (var (element, elementCondition) in ConditionedElements(root, name))
            {
                Remove(root, element);
                removedConditions.Add(elementCondition);
            }
        }

        var condition = ConfigurationCondition.Build(key);
        var globals = _globals(edit.Configuration, edit.Platform, edit.Framework);

        // The inherited rule: what the project evaluates to now, and from where.
        var current = Evaluate(root, globals, project => new PropertySources(project).Property(name));
        if (current.Source == "inherited" && !edit.Override)
        {
            return new PropertyEditResult(name, "inherited") { InheritedFrom = current.InheritedFrom, Condition = condition };
        }

        var existing = Existing(root, name, key);
        var @default = DefaultValue(root, name, globals, existing);
        var value = edit.Value;
        var result = new PropertyEditResult(name, "unchanged")
        {
            Condition = condition,
            RemovedConditions = removedConditions.Count > 0 ? removedConditions : null,
        };
        if (value is null || Same(entry, value, @default))
        {
            if (existing is null)
            {
                return removedConditions.Count > 0 ? result with { Status = "removed" } : result;
            }

            Remove(root, existing);
            return result with { Status = "removed" };
        }

        if (existing is not null)
        {
            if (string.Equals(existing.Value, value, StringComparison.Ordinal))
            {
                return removedConditions.Count > 0 ? result with { Status = "written" } : result;
            }

            existing.Value = value;
            return result with { Status = "written" };
        }

        var group = Group(root, key, condition);
        group.AddProperty(name, value);
        return result with { Status = "written" };
    }

    /// <summary>The value the project has under <paramref name="globals"/> without <paramref name="existing"/>.</summary>
    private string DefaultValue(ProjectRootElement root, string name, Dictionary<string, string> globals, ProjectPropertyElement? existing)
    {
        if (existing is null)
        {
            return Evaluate(root, globals, project => project.GetPropertyValue(name));
        }

        // Switched off for one evaluation by its condition, which leaves the element and its whitespace in place.
        var condition = existing.Condition;
        existing.Condition = "false";
        try
        {
            return Evaluate(root, globals, project => project.GetPropertyValue(name));
        }
        finally
        {
            existing.Condition = condition;
        }
    }

    private T Evaluate<T>(ProjectRootElement root, Dictionary<string, string> globals, Func<Project, T> read)
    {
        var project = new Project(root, globals, toolsVersion: null, _collection, LoadSettings);
        try
        {
            return read(project);
        }
        finally
        {
            _collection.UnloadProject(project);
        }
    }

    private static bool Same(CatalogEntry entry, string value, string @default)
    {
        if (entry.Type == PropertyCatalog.Bool)
        {
            var off = entry.FalseValue ?? "false";
            static string Norm(string v, string off) => v.Length == 0 ? off : v;
            return string.Equals(Norm(value, off), Norm(@default, off), StringComparison.OrdinalIgnoreCase);
        }

        return entry.Type == PropertyCatalog.Enum
            ? string.Equals(value, @default, StringComparison.OrdinalIgnoreCase)
            : string.Equals(value, @default, StringComparison.Ordinal);
    }

    /// <summary>The project file's element for <paramref name="name"/> under exactly <paramref name="key"/> (the last one, which wins).</summary>
    private static ProjectPropertyElement? Existing(ProjectRootElement root, string name, IReadOnlyDictionary<string, string> key)
    {
        ProjectPropertyElement? found = null;
        foreach (var group in root.PropertyGroups)
        {
            foreach (var p in group.Properties)
            {
                if (!string.Equals(p.Name, name, StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                var condition = PropertySources.Condition(p);
                var match = key.Count == 0
                    ? condition.Length == 0
                    : (p.Condition.Length == 0 || group.Condition.Length == 0) && ConfigurationCondition.Matches(condition, key);
                if (match)
                {
                    found = p;
                }
            }
        }

        return found;
    }

    /// <summary>Every element of the project file for <paramref name="name"/> under a configuration condition.</summary>
    private static List<(ProjectPropertyElement Element, string Condition)> ConditionedElements(ProjectRootElement root, string name)
    {
        var list = new List<(ProjectPropertyElement, string)>();
        foreach (var group in root.PropertyGroups)
        {
            foreach (var p in group.Properties)
            {
                if (string.Equals(p.Name, name, StringComparison.OrdinalIgnoreCase)
                    && PropertySources.Condition(p) is { Length: > 0 } c
                    && ConfigurationCondition.IsConfigurationCondition(c))
                {
                    list.Add((p, c));
                }
            }
        }

        return list;
    }

    /// <summary>Removes the element, and its conditioned property group when that is left empty.</summary>
    private static void Remove(ProjectRootElement root, ProjectPropertyElement element)
    {
        var group = (ProjectPropertyGroupElement)element.Parent;
        group.RemoveChild(element);
        if (group.Count == 0 && group.Condition.Length > 0 && group.Parent == root)
        {
            root.RemoveChild(group);
        }
    }

    /// <summary>
    /// The group a new element goes to: the first unconditioned property group, or the first with the key's
    /// condition; created when there is none (after the last unconditioned group, else after the leading imports).
    /// </summary>
    private static ProjectPropertyGroupElement Group(ProjectRootElement root, IReadOnlyDictionary<string, string> key, string? condition)
    {
        var groups = root.PropertyGroups.ToList();
        var found = key.Count == 0
            ? groups.FirstOrDefault(g => g.Condition.Length == 0)
            : groups.FirstOrDefault(g => ConfigurationCondition.Matches(g.Condition, key));
        if (found is not null)
        {
            return found;
        }

        var group = root.CreatePropertyGroupElement();
        var lastUnconditioned = groups.LastOrDefault(g => g.Condition.Length == 0);
        if (lastUnconditioned is not null)
        {
            root.InsertAfterChild(group, lastUnconditioned);
        }
        else if (root.Children.FirstOrDefault(c => c is not ProjectImportElement and not ProjectSdkElement) is { } first)
        {
            root.InsertBeforeChild(group, first);
        }
        else
        {
            root.AppendChild(group);
        }

        if (condition is not null)
        {
            group.Condition = condition;
        }

        return group;
    }

    /// <summary>
    /// An SDK-style project's build event, as Visual Studio writes it: <c>&lt;Target Name="PreBuild"
    /// BeforeTargets="PreBuildEvent"&gt;&lt;Exec Command="..." /&gt;&lt;/Target&gt;</c> (PostBuild: AfterTargets="PostBuildEvent").
    /// An empty value removes the target.
    /// </summary>
    private static PropertyEditResult SetEvent(ProjectRootElement root, CatalogEntry entry, string targetName, string? value)
    {
        var target = root.Targets.FirstOrDefault(t => string.Equals(t.Name, targetName, StringComparison.OrdinalIgnoreCase));
        var exec = target?.Tasks.FirstOrDefault(t => string.Equals(t.Name, "Exec", StringComparison.OrdinalIgnoreCase));
        if (string.IsNullOrEmpty(value))
        {
            if (target is null)
            {
                return new PropertyEditResult(entry.Name, "unchanged");
            }

            root.RemoveChild(target);
            return new PropertyEditResult(entry.Name, "removed");
        }

        if (exec is not null)
        {
            if (string.Equals(exec.GetParameter("Command"), value, StringComparison.Ordinal))
            {
                return new PropertyEditResult(entry.Name, "unchanged");
            }

            exec.SetParameter("Command", value);
            return new PropertyEditResult(entry.Name, "written");
        }

        if (target is null)
        {
            target = root.AddTarget(targetName);
            if (targetName == "PreBuild")
            {
                target.BeforeTargets = "PreBuildEvent";
            }
            else
            {
                target.AfterTargets = "PostBuildEvent";
            }
        }

        target.AddTask("Exec").SetParameter("Command", value);
        return new PropertyEditResult(entry.Name, "written");
    }

    /// <summary>The 1-based lines of the written elements, read back from the written text.</summary>
    private static List<PropertyEditResult> Lines(string text, IReadOnlyList<PropertyEdit> edits, List<PropertyEditResult> results)
    {
        using var collection = new ProjectCollection();
        var root = Parse(text, collection, preserveFormatting: false);
        var lines = new List<PropertyEditResult>(results.Count);
        for (var i = 0; i < results.Count; i++)
        {
            var r = results[i];
            if (r.Status != "written")
            {
                lines.Add(r);
                continue;
            }

            var entry = PropertyCatalog.Find(r.Name)!;
            int? line = null;
            if (entry.Target is { } t)
            {
                line = root.Targets.FirstOrDefault(x => string.Equals(x.Name, t, StringComparison.OrdinalIgnoreCase))?.Tasks.FirstOrDefault()?.Location.Line;
            }
            else
            {
                var key = ConfigurationCondition.Key(edits[i].Configuration, edits[i].Platform, edits[i].Framework);
                line = Existing(root, r.Name, key)?.Location.Line;
            }

            lines.Add(line is > 0 ? r with { Line = line } : r);
        }

        return lines;
    }

    private sealed class Utf8StringWriter : StringWriter
    {
        public Utf8StringWriter()
            : base(System.Globalization.CultureInfo.InvariantCulture)
        {
        }

        public override Encoding Encoding => new UTF8Encoding(false);
    }
}
