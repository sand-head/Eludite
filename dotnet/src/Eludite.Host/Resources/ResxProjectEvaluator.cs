using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Eludite.Host.Projects;
using Eludite.Host.Rpc;
using Microsoft.Build.Construction;
using Microsoft.Build.Evaluation;
using Microsoft.Build.Exceptions;

namespace Eludite.Host.Resources;

/// <summary>A project's <c>.resx</c> items and the properties the sets need, read from one MSBuild evaluation.</summary>
public sealed record ResxProjectInfo(string Project, bool Legacy, string RootNamespace, string? NeutralLanguage, IReadOnlyList<ResxItem> Items);

/// <summary>An <c>EmbeddedResource</c> item whose path ends in <c>.resx</c>, with the metadata the sets carry.</summary>
public sealed record ResxItem(string FullPath, string Include)
{
    public string? Generator { get; init; }

    public string? CustomToolNamespace { get; init; }

    public string? LastGenOutput { get; init; }

    public string? LogicalName { get; init; }

    public string? ManifestResourceName { get; init; }
}

/// <summary>
/// The MSBuild side of <c>eludite/resx/*</c> (proposal 0005): evaluates a project with the .NET SDK's MSBuild in-process
/// (brief 0003's locator registration) to list its <c>EmbeddedResource</c> items, and edits the item's <c>Generator</c>
/// and <c>LastGenOutput</c> metadata and the designer's <c>Compile</c> item with the construction model, formatting
/// preserved (brief 0049's <see cref="ProjectFileEditor"/> rules: every byte outside the changed elements is kept). One
/// <see cref="ProjectCollection"/> serves a solution generation; <see cref="Reset"/> drops it. Every MSBuild call runs
/// under one lock, off the RPC thread.
/// </summary>
public sealed class ResxProjectEvaluator : IDisposable
{
    private readonly Lock _lock = new();
    private readonly ReferenceAssemblies _referenceAssemblies;

    // A ProjectCollection, typed object so no Microsoft.Build type is loaded before the locator registers.
    private object? _collection;

    public ResxProjectEvaluator(ReferenceAssemblies? referenceAssemblies = null)
    {
        _referenceAssemblies = referenceAssemblies ?? new ReferenceAssemblies();
    }

    /// <summary>Evaluations run (for the cache tests).</summary>
    public int Evaluations { get; private set; }

    /// <summary>Drops the shared collection: the next evaluation reads every file again.</summary>
    public void Reset()
    {
        lock (_lock)
        {
            if (_collection is not null)
            {
                DisposeCollection();
            }
        }
    }

    public void Dispose()
    {
        Reset();
    }

    /// <summary>The project's <c>.resx</c> items; -32602 when the project does not evaluate.</summary>
    public ResxProjectInfo Evaluate(string projectPath, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        Registered();
        lock (_lock)
        {
            cancellationToken.ThrowIfCancellationRequested();
            Evaluations++;
            return EvaluateCore(Path.GetFullPath(projectPath));
        }
    }

    /// <summary>
    /// Sets the file's item metadata for <paramref name="generator"/> (<c>Generator</c> and <c>LastGenOutput</c>) and
    /// the designer's <c>Compile</c> item with <c>DesignTime</c>, <c>AutoGen</c> and <c>DependentUpon</c>, as Visual
    /// Studio writes them (an <c>Update</c> item for an SDK project, <c>Include</c> for a legacy one); a null
    /// <paramref name="generator"/> removes them. True when the project file changed.
    /// </summary>
    public bool SetGenerator(string projectPath, string resxPath, string designerPath, string? generator, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        ArgumentNullException.ThrowIfNull(resxPath);
        ArgumentNullException.ThrowIfNull(designerPath);
        var full = Path.GetFullPath(projectPath);
        Registered();
        lock (_lock)
        {
            cancellationToken.ThrowIfCancellationRequested();
            return SetGeneratorCore(full, Path.GetFullPath(resxPath), Path.GetFullPath(designerPath), generator, SolutionProjects.IsLegacy(full), cancellationToken);
        }
    }

    private static void Registered()
    {
        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            throw HostErrors.BadParams("no .NET SDK found by Microsoft.Build.Locator: resource sets need MSBuild");
        }
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private void DisposeCollection()
    {
        if (_collection is ProjectCollection c)
        {
            c.UnloadAllProjects();
            c.Dispose();
        }

        _collection = null;
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private ProjectCollection Collection()
    {
        if (_collection is not ProjectCollection c)
        {
            c = new ProjectCollection(null, null, ToolsetDefinitionLocations.Default);
            _collection = c;
        }

        return c;
    }

    private Dictionary<string, string> Globals(string full, bool legacy)
    {
        var globals = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        if (legacy)
        {
            var root = DesignTimeProperties.ReadTargetFrameworkVersion(full) is { } tfv ? _referenceAssemblies.RootFor(tfv) : null;
            foreach (var (k, v) in DesignTimeProperties.Create(root))
            {
                globals[k] = v;
            }
        }

        return globals;
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private ResxProjectInfo EvaluateCore(string full)
    {
        if (!File.Exists(full))
        {
            throw HostErrors.BadParams($"{full} does not exist");
        }

        var legacy = SolutionProjects.IsLegacy(full);
        var collection = Collection();
        var globals = Globals(full, legacy);
        Project project;
        try
        {
            project = new Project(full, globals, toolsVersion: null, collection, ProjectFileEditor.LoadSettings);
            if (!legacy)
            {
                var frameworks = ProjectPropertyEvaluator.SplitList(project.GetPropertyValue("TargetFrameworks"));
                if (frameworks.Count > 0 && project.GetPropertyValue("TargetFramework").Length == 0)
                {
                    // A multi-targeted project's outer evaluation has no default items; read the first inner one.
                    collection.UnloadProject(project);
                    globals["TargetFramework"] = frameworks[0];
                    project = new Project(full, globals, toolsVersion: null, collection, ProjectFileEditor.LoadSettings);
                }
            }
        }
        catch (InvalidProjectFileException ex)
        {
            throw HostErrors.BadParams($"{full} does not evaluate: {ex.Message}");
        }

        try
        {
            var rootNamespace = project.GetPropertyValue("RootNamespace");
            if (rootNamespace.Length == 0)
            {
                rootNamespace = project.GetPropertyValue("AssemblyName");
            }

            if (rootNamespace.Length == 0)
            {
                rootNamespace = Path.GetFileNameWithoutExtension(full);
            }

            var neutralLanguage = project.GetPropertyValue("NeutralLanguage");
            var dir = Path.GetDirectoryName(full)!;
            var excluded = new[] { "obj", "bin" }.Select(d => Path.Combine(dir, d) + Path.DirectorySeparatorChar).ToArray();
            var seen = new HashSet<string>(StringComparer.Ordinal);
            var items = new List<ResxItem>();
            foreach (var item in project.GetItems("EmbeddedResource"))
            {
                var path = Path.GetFullPath(item.GetMetadataValue("FullPath"));
                if (!path.EndsWith(".resx", StringComparison.OrdinalIgnoreCase)
                    || excluded.Any(x => path.StartsWith(x, StringComparison.Ordinal))
                    || !seen.Add(path))
                {
                    continue;
                }

                items.Add(new ResxItem(path, item.EvaluatedInclude)
                {
                    Generator = Metadata(item, "Generator"),
                    CustomToolNamespace = Metadata(item, "CustomToolNamespace"),
                    LastGenOutput = Metadata(item, "LastGenOutput"),
                    LogicalName = Metadata(item, "LogicalName"),
                    ManifestResourceName = Metadata(item, "ManifestResourceName"),
                });
            }

            return new ResxProjectInfo(full, legacy, rootNamespace, neutralLanguage.Length > 0 ? neutralLanguage : null, items);
        }
        finally
        {
            collection.UnloadProject(project);
        }
    }

    private static string? Metadata(ProjectItem item, string name) =>
        item.GetMetadataValue(name) is { Length: > 0 } v ? v : null;

    [MethodImpl(MethodImplOptions.NoInlining)]
    private bool SetGeneratorCore(string projectPath, string resxPath, string designerPath, string? generator, bool legacy, CancellationToken cancellationToken)
    {
        var bytes = File.ReadAllBytes(projectPath);
        var format = TextFileFormat.Detect(bytes, out var text);
        var root = ProjectFileEditor.Parse(text, Collection(), preserveFormatting: true);
        root.FullPath = projectPath;
        var dir = Path.GetDirectoryName(projectPath)!;
        var resxRelative = Relative(dir, resxPath);
        var designerRelative = Relative(dir, designerPath);
        var resource = Find(root, "EmbeddedResource", resxRelative);
        var compile = Find(root, "Compile", designerRelative);
        if (generator is not null)
        {
            resource ??= AddItem(root, "EmbeddedResource", resxRelative, legacy);
            SetMetadata(resource, "Generator", generator);
            SetMetadata(resource, "LastGenOutput", Path.GetFileName(designerPath));
            if (compile is null)
            {
                compile = AddItem(root, "Compile", designerRelative, legacy);
                // Visual Studio's order: DesignTime, AutoGen, DependentUpon in an SDK project; AutoGen, DesignTime, DependentUpon in a legacy one.
                foreach (var name in legacy ? new[] { "AutoGen", "DesignTime" } : ["DesignTime", "AutoGen"])
                {
                    compile.AddMetadata(name, "True", expressAsAttribute: false);
                }

                compile.AddMetadata("DependentUpon", Path.GetFileName(resxPath), expressAsAttribute: false);
            }
            else
            {
                SetMetadata(compile, "DesignTime", "True");
                SetMetadata(compile, "AutoGen", "True");
                SetMetadata(compile, "DependentUpon", Path.GetFileName(resxPath));
            }
        }
        else
        {
            if (resource is not null)
            {
                RemoveMetadata(resource, "Generator");
                RemoveMetadata(resource, "LastGenOutput");
                if (resource.Update.Length > 0 && resource.Include.Length == 0 && resource.Count == 0)
                {
                    // An Update item with nothing left to say.
                    RemoveItem(resource);
                }
            }

            if (compile is not null)
            {
                RemoveItem(compile);
            }
        }

        var written = format.Encode(ProjectFileEditor.Serialize(root));
        if (written.AsSpan().SequenceEqual(bytes))
        {
            return false;
        }

        cancellationToken.ThrowIfCancellationRequested();
        TextFileFormat.WriteAtomically(projectPath, written);
        return true;
    }

    /// <summary>The project-relative path with backslashes, as the project file writes it.</summary>
    private static string Relative(string projectDirectory, string path) =>
        Path.GetRelativePath(projectDirectory, path).Replace('/', '\\');

    private static string Normalize(string itemSpec) => itemSpec.Replace('/', '\\').Trim();

    /// <summary>The item of <paramref name="itemType"/> whose Include or Update is <paramref name="relative"/>.</summary>
    private static ProjectItemElement? Find(ProjectRootElement root, string itemType, string relative) =>
        root.Items.FirstOrDefault(i => string.Equals(i.ItemType, itemType, StringComparison.OrdinalIgnoreCase)
            && (string.Equals(Normalize(i.Include), relative, StringComparison.OrdinalIgnoreCase)
                || string.Equals(Normalize(i.Update), relative, StringComparison.OrdinalIgnoreCase)));

    private static ProjectMetadataElement? FindMetadata(ProjectItemElement item, string name) =>
        item.Metadata.FirstOrDefault(m => string.Equals(m.Name, name, StringComparison.OrdinalIgnoreCase));

    private static void SetMetadata(ProjectItemElement item, string name, string value)
    {
        if (FindMetadata(item, name) is { } existing)
        {
            if (!string.Equals(existing.Value, value, StringComparison.Ordinal))
            {
                existing.Value = value;
            }

            return;
        }

        item.AddMetadata(name, value, expressAsAttribute: false);
    }

    private static void RemoveMetadata(ProjectItemElement item, string name)
    {
        if (FindMetadata(item, name) is { } existing)
        {
            item.RemoveChild(existing);
        }
    }

    /// <summary>
    /// Adds an item to the first unconditioned ItemGroup holding items of its type, else to a new ItemGroup: an
    /// <c>Include</c> item in a legacy project, an <c>Update</c> item in an SDK project (the glob lists the file).
    /// </summary>
    private static ProjectItemElement AddItem(ProjectRootElement root, string itemType, string relative, bool include)
    {
        var group = root.ItemGroups.FirstOrDefault(g => g.Condition.Length == 0 && g.Items.Any(i => string.Equals(i.ItemType, itemType, StringComparison.OrdinalIgnoreCase)))
            ?? root.AddItemGroup();
        if (include)
        {
            return group.AddItem(itemType, relative);
        }

        var item = root.CreateItemElement(itemType);
        item.Update = relative;
        group.AppendChild(item);
        return item;
    }

    private static void RemoveItem(ProjectItemElement item)
    {
        var group = item.Parent;
        group.RemoveChild(item);
        if (group is ProjectItemGroupElement { Count: 0 } empty)
        {
            empty.Parent.RemoveChild(empty);
        }
    }
}
