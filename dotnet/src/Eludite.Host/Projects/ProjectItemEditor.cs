using System.Runtime.CompilerServices;
using Microsoft.Build.Construction;
using Microsoft.Build.Evaluation;

namespace Eludite.Host.Projects;

/// <summary>What <see cref="ProjectItemEditor.Apply"/> did: whether the project file was written and one result per edit.</summary>
public sealed record ProjectItemsOutcome(bool Changed, IReadOnlyList<ProjectItemEditResult> Results);

/// <summary>
/// The formatting-preserving item edits of <c>eludite/project/editItems</c> (brief 0073;
/// protocol/schemas/host/project-edit-items.json): Include In Project, Exclude From Project, the project side of a file
/// rename, move or delete, an F# file's compile order and an SDK project's empty folder. The file is parsed from its
/// text with MSBuild's construction model (<c>preserveFormatting</c>, as brief 0049's <see cref="ProjectFileEditor"/>),
/// every edit is applied to that one in-memory project, and the file is written once at the end, laid out again with
/// <see cref="TextFileFormat"/> (byte order mark, line endings, declaration), when its bytes changed.
/// </summary>
/// <remarks>
/// An SDK-style project's items mostly come from the .NET SDK's globs, so include, exclude and addFolder evaluate the
/// in-memory project to see what the globs bring in: an explicit element is deleted, a globbed item gets a
/// <c>Remove</c>. A legacy project lists every file, so its edits are on the elements alone. Paths are written relative
/// to the project's folder with <c>\</c>, as Visual Studio writes them; a rewritten value keeps a value's <c>/</c>.
/// Items inside targets are run-time items and are left alone. The caller holds
/// <see cref="MsBuildProjectTreeEvaluator.EvaluationLock"/> and has registered the MSBuild locator.
/// </remarks>
internal sealed class ProjectItemEditor
{
    private static readonly StringComparison PathComparison = OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal;
    private static readonly StringComparer PathComparer = OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal;

    // Visual Studio's Content extensions in a web project (ASP.NET Core, Razor and WebForms).
    private static readonly string[] WebContent =
        [".cshtml", ".razor", ".html", ".htm", ".css", ".js", ".json", ".aspx", ".ascx", ".master", ".asax", ".ashx", ".asmx"];

    private readonly string _path;
    private readonly string _dir;
    private readonly bool _legacy;
    private readonly ProjectCollection _collection;
    private readonly ProjectRootElement _root;

    private ProjectItemEditor(string path, ProjectCollection collection, ProjectRootElement root)
    {
        _path = path;
        _dir = Path.GetDirectoryName(path)!;
        _legacy = Legacy.SolutionProjects.IsLegacy(path);
        _collection = collection;
        _root = root;
    }

    /// <summary>Applies <paramref name="edits"/> in order to <paramref name="projectPath"/> and writes it once when it changed.</summary>
    [MethodImpl(MethodImplOptions.NoInlining)]
    public static ProjectItemsOutcome Apply(string projectPath, IReadOnlyList<ProjectItemEdit> edits, CancellationToken cancellationToken)
    {
        var path = Path.GetFullPath(projectPath);
        var bytes = File.ReadAllBytes(path);
        var format = TextFileFormat.Detect(bytes, out var text);
        using var collection = new ProjectCollection(null, null, ToolsetDefinitionLocations.Default);
        var root = ProjectFileEditor.Parse(text, collection, preserveFormatting: true);
        root.FullPath = path;
        var editor = new ProjectItemEditor(path, collection, root);
        var results = new List<ProjectItemEditResult>(edits.Count);
        try
        {
            foreach (var edit in edits)
            {
                cancellationToken.ThrowIfCancellationRequested();
                results.Add(editor.ApplyOne(edit));
            }
        }
        finally
        {
            collection.UnloadAllProjects();
        }

        if (!results.Any(r => r.Applied))
        {
            return new ProjectItemsOutcome(false, results);
        }

        var written = format.Encode(ProjectFileEditor.Serialize(root));
        if (written.AsSpan().SequenceEqual(bytes))
        {
            return new ProjectItemsOutcome(false, results);
        }

        cancellationToken.ThrowIfCancellationRequested();
        TextFileFormat.WriteAtomically(path, written);
        return new ProjectItemsOutcome(true, results);
    }

    /// <summary>
    /// The item type Include In Project gives a file: <c>Compile</c> for the project's own language (<c>.cs</c> in a
    /// <c>.csproj</c>, <c>.vb</c> in a <c>.vbproj</c>, <c>.fs</c> and <c>.fsi</c> in an <c>.fsproj</c>),
    /// <c>EmbeddedResource</c> for <c>.resx</c>, <c>Content</c> for web content in a web project, else <c>None</c>.
    /// </summary>
    internal static string ItemTypeFor(string projectPath, string file, bool web)
    {
        var ext = Path.GetExtension(file).ToLowerInvariant();
        var language = Path.GetExtension(projectPath).ToLowerInvariant();
        var compile = (ext, language) is (".cs", ".csproj") or (".vb", ".vbproj") or (".fs" or ".fsi", ".fsproj");
        return compile ? "Compile"
            : ext == ".resx" ? "EmbeddedResource"
            : web && WebContent.Contains(ext) ? "Content"
            : "None";
    }

    private ProjectItemEditResult ApplyOne(ProjectItemEdit edit)
    {
        var full = Path.GetFullPath(edit.Path!);
        var result = new ProjectItemEditResult(edit.Op!, edit.Path!, false);
        return edit.Op switch
        {
            "include" => result with { Applied = Include(full, edit.ItemType) },
            "exclude" => result with { Applied = Exclude(full) },
            "rename" => result with { Applied = Rename(full, Path.GetFullPath(edit.To!)) },
            "remove" => result with { Applied = Remove(full) },
            "moveUp" or "moveDown" => Move(full, edit.Op == "moveUp", result),
            "addFolder" => AddFolder(full, result),
            _ => throw new ArgumentException($"unknown op {edit.Op}", nameof(edit)),
        };
    }

    /// <summary>Include In Project: the file, or every file under the folder, becomes an item.</summary>
    private bool Include(string full, string? itemType)
    {
        var files = Directory.Exists(full) ? FilesUnder(full) : [full];
        var changed = false;
        HashSet<string> items;
        if (_legacy)
        {
            items = new HashSet<string>(StaticItems().SelectMany(e => Parts(e.Include)).Select(p => Full(Norm(p))), PathComparer);
        }
        else
        {
            // A Remove (of any item type) naming the path, or a path under the folder, is what excluded it.
            var rel = Rel(full);
            foreach (var element in StaticItems().Where(e => e.Remove.Length > 0).ToList())
            {
                changed |= DropParts(element, e => e.Remove, (e, v) => e.Remove = v, p => Names(p, rel));
            }

            items = EvaluatedItems(includeFolders: false).Keys.ToHashSet(PathComparer);
        }

        var web = IsWeb();
        foreach (var file in files)
        {
            if (!items.Contains(file))
            {
                Add(itemType ?? ItemTypeFor(_path, file, web), "Include", Rel(file));
                changed = true;
            }
        }

        return changed;
    }

    /// <summary>
    /// Exclude From Project: an explicit element naming the path (or a path under the folder) is deleted; in an SDK
    /// project an item a glob still brings in gets <c>&lt;T Remove="rel" /&gt;</c> (a folder: <c>rel\**</c>).
    /// </summary>
    private bool Exclude(string full)
    {
        var rel = Rel(full);
        var changed = false;
        foreach (var element in StaticItems().Where(e => e.Include.Length > 0).ToList())
        {
            changed |= DropParts(element, e => e.Include, (e, v) => e.Include = v, p => Names(p, rel));
        }

        if (_legacy)
        {
            return changed;
        }

        var folder = Directory.Exists(full);
        var under = full + Path.DirectorySeparatorChar;
        var types = EvaluatedItems(includeFolders: false)
            .Where(kv => PathComparer.Equals(kv.Key, full) || kv.Key.StartsWith(under, PathComparison))
            .SelectMany(kv => kv.Value)
            .Distinct(StringComparer.OrdinalIgnoreCase)
            .ToList();
        foreach (var type in types)
        {
            Add(type, "Remove", folder ? rel + @"\**" : rel);
            changed = true;
        }

        return changed;
    }

    /// <summary>
    /// The project side of a rename or move: every Include, Remove and Update value naming <paramref name="from"/>
    /// (or a path under it) and every DependentUpon naming it point at <paramref name="to"/>.
    /// </summary>
    private bool Rename(string from, string to)
    {
        var oldRel = Rel(from);
        var newRel = Rel(to);
        string? Renamed(string full) =>
            PathComparer.Equals(full, from) ? to
            : full.StartsWith(from + Path.DirectorySeparatorChar, PathComparison) ? to + full[from.Length..]
            : null;

        var changed = false;
        foreach (var element in StaticItems().ToList())
        {
            // DependentUpon is relative to the item's own folder, which may move with it.
            if (element.Include.Length > 0 && !HasWildcard(element.Include) && element.Metadata.FirstOrDefault(m => string.Equals(m.Name, "DependentUpon", StringComparison.OrdinalIgnoreCase)) is { } dependentUpon)
            {
                var item = Full(Norm(Parts(element.Include).First()));
                var parent = Path.GetFullPath(Path.Combine(Path.GetDirectoryName(item)!, dependentUpon.Value.Trim().Replace('\\', Path.DirectorySeparatorChar)));
                if (Renamed(parent) is { } newParent)
                {
                    var itemFolder = Path.GetDirectoryName(Renamed(item) ?? item)!;
                    dependentUpon.Value = Path.GetRelativePath(itemFolder, newParent).Replace('/', '\\');
                    changed = true;
                }
            }

            changed |= RewriteParts(element, e => e.Include, (e, v) => e.Include = v, oldRel, newRel);
            changed |= RewriteParts(element, e => e.Remove, (e, v) => e.Remove = v, oldRel, newRel);
            changed |= RewriteParts(element, e => e.Update, (e, v) => e.Update = v, oldRel, newRel);
        }

        return changed;
    }

    /// <summary>After a delete: every Include, Remove and Update value naming the path (or a path under it) goes.</summary>
    private bool Remove(string full)
    {
        var rel = Rel(full);
        var changed = false;
        foreach (var element in StaticItems().ToList())
        {
            changed |= DropParts(element, e => e.Include, (e, v) => e.Include = v, p => Names(p, rel))
                || DropParts(element, e => e.Remove, (e, v) => e.Remove = v, p => Names(p, rel))
                || DropParts(element, e => e.Update, (e, v) => e.Update = v, p => Names(p, rel));
        }

        return changed;
    }

    /// <summary>F# compile order: the file's Compile element moves before the previous, or after the next, Compile element.</summary>
    private ProjectItemEditResult Move(string full, bool up, ProjectItemEditResult result)
    {
        var rel = Rel(full);
        var compile = StaticItems()
            .Where(e => string.Equals(e.ItemType, "Compile", StringComparison.OrdinalIgnoreCase) && e.Include.Length > 0)
            .ToList();
        var index = compile.FindIndex(e => string.Equals(Norm(e.Include), rel, PathComparison));
        if (index < 0)
        {
            return result with { Message = $"{rel} is not a Compile item of the project" };
        }

        if (up ? index == 0 : index == compile.Count - 1)
        {
            return result with { Message = up ? $"{rel} is already first" : $"{rel} is already last" };
        }

        var element = compile[index];
        var other = compile[up ? index - 1 : index + 1];
        Delete(element);
        if (up)
        {
            other.Parent.InsertBeforeChild(element, other);
        }
        else
        {
            other.Parent.InsertAfterChild(element, other);
        }

        return result with { Applied = true };
    }

    /// <summary>An SDK project's empty folder: <c>&lt;Folder Include="rel\" /&gt;</c>, unless an item lies under it.</summary>
    private ProjectItemEditResult AddFolder(string full, ProjectItemEditResult result)
    {
        if (_legacy)
        {
            return result with { Message = "a legacy project lists files only: the folder shows once it holds an item" };
        }

        var under = full + Path.DirectorySeparatorChar;
        if (EvaluatedItems(includeFolders: true).Keys.Any(p => PathComparer.Equals(p, full) || p.StartsWith(under, PathComparison)))
        {
            return result;
        }

        Add("Folder", "Include", Rel(full) + @"\");
        return result with { Applied = true };
    }

    /// <summary>The project's item elements outside targets, in document order.</summary>
    private IEnumerable<ProjectItemElement> StaticItems() =>
        _root.AllChildren.OfType<ProjectItemElement>().Where(e => e.Parent?.Parent is not ProjectTargetElement);

    /// <summary>
    /// A new item element at the end of the last unconditioned item group holding items of <paramref name="itemType"/>,
    /// or in a new item group.
    /// </summary>
    private void Add(string itemType, string attribute, string value)
    {
        var group = _root.Children.OfType<ProjectItemGroupElement>()
            .LastOrDefault(g => g.Condition.Length == 0 && g.Items.Any(i => string.Equals(i.ItemType, itemType, StringComparison.OrdinalIgnoreCase)))
            ?? _root.AddItemGroup();
        var element = _root.CreateItemElement(itemType);
        if (attribute == "Include")
        {
            element.Include = value;
        }
        else
        {
            element.Remove = value;
        }

        group.AppendChild(element);
    }

    /// <summary>Deletes the element, and its item group when that is left empty.</summary>
    private static void Delete(ProjectItemElement element)
    {
        var parent = element.Parent;
        parent.RemoveChild(element);
        if (parent is ProjectItemGroupElement { Count: 0 } group && group.Parent is { } owner)
        {
            owner.RemoveChild(group);
        }
    }

    /// <summary>Drops the matching values of one attribute; the element goes when none is left.</summary>
    private static bool DropParts(ProjectItemElement element, Func<ProjectItemElement, string> get, Action<ProjectItemElement, string> set, Func<string, bool> match)
    {
        var value = get(element);
        if (value.Length == 0)
        {
            return false;
        }

        var parts = value.Split(';');
        var kept = parts.Where(p => p.Trim().Length > 0 && !match(p)).ToList();
        if (kept.Count == parts.Count(p => p.Trim().Length > 0))
        {
            return false;
        }

        if (kept.Count == 0)
        {
            Delete(element);
        }
        else
        {
            set(element, string.Join(';', kept));
        }

        return true;
    }

    /// <summary>Rewrites the values of one attribute naming <paramref name="oldRel"/> or a path under it.</summary>
    private static bool RewriteParts(ProjectItemElement element, Func<ProjectItemElement, string> get, Action<ProjectItemElement, string> set, string oldRel, string newRel)
    {
        var value = get(element);
        if (value.Length == 0)
        {
            return false;
        }

        var changed = false;
        var parts = value.Split(';');
        for (var i = 0; i < parts.Length; i++)
        {
            var part = parts[i];
            if (!Names(part, oldRel))
            {
                continue;
            }

            var trimmed = part.Trim();
            var slash = trimmed.Contains('/', StringComparison.Ordinal) && !trimmed.Contains('\\', StringComparison.Ordinal);
            var dot = trimmed.StartsWith(@".\", StringComparison.Ordinal) || trimmed.StartsWith("./", StringComparison.Ordinal) ? @".\" : string.Empty;
            var trailing = trimmed.EndsWith('\\') || trimmed.EndsWith('/') ? @"\" : string.Empty;
            var rewritten = dot + newRel + Norm(trimmed)[oldRel.Length..] + trailing;
            parts[i] = part.Replace(trimmed, slash ? rewritten.Replace('\\', '/') : rewritten, StringComparison.Ordinal);
            changed = true;
        }

        if (changed)
        {
            set(element, string.Join(';', parts));
        }

        return changed;
    }

    /// <summary>True when the value names <paramref name="rel"/> or a path under it.</summary>
    private static bool Names(string part, string rel)
    {
        var n = Norm(part);
        return string.Equals(n, rel, PathComparison) || n.StartsWith(rel + @"\", PathComparison);
    }

    /// <summary>A value as a comparable relative path: trimmed, <c>\</c> separators, no leading <c>.\</c> or trailing separator.</summary>
    private static string Norm(string part)
    {
        var n = part.Trim().Replace('/', '\\');
        while (n.StartsWith(@".\", StringComparison.Ordinal))
        {
            n = n[2..];
        }

        return n.TrimEnd('\\');
    }

    private static IEnumerable<string> Parts(string value) =>
        value.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);

    private static bool HasWildcard(string value) => value.Contains('*', StringComparison.Ordinal) || value.Contains('?', StringComparison.Ordinal);

    /// <summary>The project-relative path with <c>\</c>, as Visual Studio writes it.</summary>
    private string Rel(string full) => Path.GetRelativePath(_dir, full).Replace('/', '\\');

    private string Full(string rel) => Path.GetFullPath(Path.Combine(_dir, rel.Replace('\\', Path.DirectorySeparatorChar)));

    /// <summary>Every file under <paramref name="folder"/>, except the project's <c>bin</c> and <c>obj</c> folders.</summary>
    private List<string> FilesUnder(string folder)
    {
        var skip = new[] { "bin", "obj" }.Select(d => Path.Combine(_dir, d) + Path.DirectorySeparatorChar).ToArray();
        return [.. Directory.EnumerateFiles(folder, "*", SearchOption.AllDirectories)
            .Where(f => !skip.Any(s => f.StartsWith(s, PathComparison)))
            .Order(StringComparer.Ordinal)];
    }

    /// <summary>A web project (ASP.NET Core or Razor SDK, or a legacy web project type), whose web files are Content.</summary>
    private bool IsWeb()
    {
        var sdk = _root.Sdk;
        if (sdk.Contains("Microsoft.NET.Sdk.Web", StringComparison.OrdinalIgnoreCase) || sdk.Contains("Microsoft.NET.Sdk.Razor", StringComparison.OrdinalIgnoreCase)
            || sdk.Contains("BlazorWebAssembly", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        var guids = string.Join(';', _root.Properties.Where(p => string.Equals(p.Name, "ProjectTypeGuids", StringComparison.OrdinalIgnoreCase)).Select(p => p.Value));
        return MsBuildProjectTreeEvaluator.WebProjectTypes.Any(g => guids.Contains(g, StringComparison.OrdinalIgnoreCase));
    }

    /// <summary>
    /// The in-memory project's items of the types Solution Explorer lists (Folder only when asked), by full path, each
    /// with its item types. A multi-targeted project is read under its first target framework, as the tree is.
    /// </summary>
    private Dictionary<string, List<string>> EvaluatedItems(bool includeFolders)
    {
        var globals = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var project = new Project(_root, globals, toolsVersion: null, _collection, ProjectFileEditor.LoadSettings);
        try
        {
            var frameworks = project.GetPropertyValue("TargetFrameworks");
            if (frameworks.Length > 0 && project.GetPropertyValue("TargetFramework").Length == 0)
            {
                _collection.UnloadProject(project);
                globals["TargetFramework"] = frameworks.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)[0];
                project = new Project(_root, globals, toolsVersion: null, _collection, ProjectFileEditor.LoadSettings);
            }

            var items = new Dictionary<string, List<string>>(PathComparer);
            foreach (var type in MsBuildProjectTreeEvaluator.VisibleItemTypes(project))
            {
                if (!includeFolders && string.Equals(type, "Folder", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }

                foreach (var item in project.GetItems(type))
                {
                    var path = Path.TrimEndingDirectorySeparator(Path.GetFullPath(item.GetMetadataValue("FullPath")));
                    if (!items.TryGetValue(path, out var types))
                    {
                        items[path] = types = [];
                    }

                    if (!types.Contains(item.ItemType, StringComparer.OrdinalIgnoreCase))
                    {
                        types.Add(item.ItemType);
                    }
                }
            }

            return items;
        }
        finally
        {
            _collection.UnloadProject(project);
        }
    }
}
