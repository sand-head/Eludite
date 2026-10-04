using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using Microsoft.Build.Construction;
using Microsoft.Build.Evaluation;
using Microsoft.Build.Exceptions;

namespace Eludite.Host.Projects;

/// <summary>
/// The project property pages' evaluation and edit (brief 0049): evaluates a project with the .NET SDK's MSBuild
/// in-process (the locator registration of brief 0003) with <c>Configuration</c>, <c>Platform</c> and
/// <c>TargetFramework</c> as global properties, and edits the project file with MSBuild's construction model
/// (<see cref="ProjectFileEditor"/>). One <see cref="Microsoft.Build.Evaluation.ProjectCollection"/> serves a whole
/// solution generation, so the .NET SDK's imports are parsed once; <see cref="Reset"/> drops it when the generation
/// moves on (the files may have changed). Every MSBuild call runs under one lock, off the RPC thread.
/// </summary>
public sealed class ProjectPropertyEvaluator : IDisposable
{
    private readonly Lock _lock = new();
    private readonly ReferenceAssemblies _referenceAssemblies;

    // A ProjectCollection, typed object so no Microsoft.Build type is loaded before the locator registers.
    private object? _collection;

    public ProjectPropertyEvaluator(ReferenceAssemblies? referenceAssemblies = null)
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
            // Never touches a Microsoft.Build type before an evaluation registered the locator.
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

    /// <summary>The catalog's values for <paramref name="projectPath"/> in one configuration, platform and framework.</summary>
    public ProjectPropertiesResult Evaluate(string projectPath, string? configuration, string? platform, string? framework, long generation, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        Registered();
        lock (_lock)
        {
            cancellationToken.ThrowIfCancellationRequested();
            Evaluations++;
            return EvaluateCore(Path.GetFullPath(projectPath), configuration, platform, framework, generation);
        }
    }

    /// <summary>
    /// Applies <paramref name="edits"/> to the project file by the condition and inherited rules and writes it when it
    /// changed. Legacy projects are refused (read-only, brief 0049).
    /// </summary>
    public ProjectEditOutcome Edit(string projectPath, IReadOnlyList<PropertyEdit> edits, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        ArgumentNullException.ThrowIfNull(edits);
        var full = Path.GetFullPath(projectPath);
        if (SolutionProjects.IsLegacy(full))
        {
            throw HostErrors.BadParams($"{full} is a legacy (non-SDK) project file: its properties are shown read-only");
        }

        Registered();
        lock (_lock)
        {
            cancellationToken.ThrowIfCancellationRequested();
            return EditCore(full, edits, cancellationToken);
        }
    }

    /// <summary>
    /// The project XML <paramref name="text"/> parsed with MSBuild's construction model (formatting preserved) and saved
    /// again, unedited: what an edit keeps of the file outside its element (the round-trip tests).
    /// </summary>
    public static string Reserialize(string text)
    {
        Registered();
        return ReserializeCore(text);
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static string ReserializeCore(string text)
    {
        using var collection = new ProjectCollection();
        return ProjectFileEditor.Serialize(ProjectFileEditor.Parse(text, collection, preserveFormatting: true));
    }

    private static void Registered()
    {
        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            throw HostErrors.BadParams("no .NET SDK found by Microsoft.Build.Locator: project properties need MSBuild");
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

    internal Dictionary<string, string> Globals(string full, bool legacy, string? configuration, string? platform, string? framework)
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

        if (!string.IsNullOrEmpty(configuration))
        {
            globals["Configuration"] = configuration;
        }

        if (!string.IsNullOrEmpty(platform))
        {
            globals["Platform"] = ConfigurationCondition.ProjectPlatform(platform);
        }

        if (!string.IsNullOrEmpty(framework))
        {
            globals["TargetFramework"] = framework;
        }

        return globals;
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private ProjectPropertiesResult EvaluateCore(string full, string? configuration, string? platform, string? framework, long generation)
    {
        if (!File.Exists(full))
        {
            throw HostErrors.BadParams($"{full} does not exist");
        }

        var legacy = SolutionProjects.IsLegacy(full);
        var collection = Collection();
        Project project;
        try
        {
            project = new Project(full, Globals(full, legacy, configuration, platform, framework), toolsVersion: null, collection, ProjectFileEditor.LoadSettings);
        }
        catch (InvalidProjectFileException ex)
        {
            throw HostErrors.BadParams($"{full} does not evaluate: {ex.Message}");
        }

        try
        {
            var sources = new PropertySources(project);
            var properties = new List<PropertyValue>(PropertyCatalog.Entries.Count);
            foreach (var entry in PropertyCatalog.Entries)
            {
                properties.Add(Read(entry, project, sources, legacy));
            }

            var configurations = SplitList(project.GetPropertyValue("Configurations"));
            var platforms = SplitList(project.GetPropertyValue("Platforms"));
            var frameworks = SplitList(project.GetPropertyValue("TargetFrameworks"));
            if (frameworks.Count == 0)
            {
                frameworks = SplitList(project.GetPropertyValue("TargetFramework"));
            }

            if (legacy && frameworks.Count == 0 && MsBuildProjectTreeEvaluator.ShortFramework(project.GetPropertyValue("TargetFrameworkVersion")) is { } f)
            {
                frameworks = [f];
            }

            return new ProjectPropertiesResult(
                generation,
                full,
                legacy ? "legacy" : "sdk",
                project.GetPropertyValue("Configuration"),
                project.GetPropertyValue("Platform"),
                configurations.Count > 0 ? configurations : ["Debug", "Release"],
                platforms.Count > 0 ? platforms : ["AnyCPU"],
                frameworks,
                [.. PropertyCatalog.Pages.Select(p => new PageInfo(p.Id, p.Title, p.State) { Note = p.Note })],
                properties)
            {
                Framework = string.IsNullOrEmpty(framework) ? null : framework,
            };
        }
        finally
        {
            collection.UnloadProject(project);
        }
    }

    internal static List<string> SplitList(string value) =>
        [.. value.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).Distinct(StringComparer.OrdinalIgnoreCase)];

    private static PropertyValue Read(CatalogEntry entry, Project project, PropertySources sources, bool legacy)
    {
        var (value, source, definedIn, raw, inheritedFrom) = entry.Target is { } target && !legacy
            ? sources.Event(entry.Name, target)
            : sources.Property(entry.Name);
        var readOnlyReason = legacy
            ? "Legacy (non-SDK) project files are shown read-only."
            : entry.WindowsOnly && !OperatingSystem.IsWindows()
                ? "Win32 resources are edited on Windows."
                : null;
        return new PropertyValue(
            entry.Name,
            entry.Page,
            entry.Label,
            entry.Type,
            entry.PerConfiguration,
            value,
            source,
            definedIn?.Condition is { Length: > 0 },
            source == "inherited")
        {
            Section = entry.Section,
            Description = entry.Description,
            Values = entry.Values,
            TrueValue = entry.Type == PropertyCatalog.Bool ? entry.TrueValue ?? "true" : null,
            FalseValue = entry.Type == PropertyCatalog.Bool ? entry.FalseValue ?? "false" : null,
            Target = legacy ? null : entry.Target,
            Raw = raw,
            DefinedIn = definedIn,
            InheritedFrom = inheritedFrom,
            Conditions = sources.Conditions(entry.Name) is { Count: > 0 } c ? c : null,
            ReadOnly = readOnlyReason is not null,
            ReadOnlyReason = readOnlyReason,
        };
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private ProjectEditOutcome EditCore(string full, IReadOnlyList<PropertyEdit> edits, CancellationToken cancellationToken)
    {
        var editor = new ProjectFileEditor(full, Collection(), (cfg, plat, fw) => Globals(full, legacy: false, cfg, plat, fw));
        return editor.Apply(edits, cancellationToken);
    }
}

/// <summary>
/// Where a project's property values come from: the project file (unconditioned or conditioned), an inherited file
/// (an import that is not the .NET SDK's or MSBuild's, nor NuGet's generated <c>obj/</c> files), or the default.
/// </summary>
internal sealed class PropertySources
{
    private readonly Project _project;
    private readonly string _projectPath;
    private readonly string[] _defaultRoots;

    public PropertySources(Project project)
    {
        _project = project;
        _projectPath = project.FullPath;
        var roots = new List<string>();
        foreach (var name in new[] { "MSBuildExtensionsPath", "MSBuildSDKsPath", "MSBuildBinPath", "NuGetPackageRoot", "MSBuildProjectExtensionsPath", "BaseIntermediateOutputPath" })
        {
            var v = project.GetPropertyValue(name);
            if (v.Length > 0)
            {
                roots.Add(Normalize(Path.IsPathRooted(v) ? v : Path.Combine(project.DirectoryPath, v)));
            }
        }

        // The .NET installation (sdk/, sdk-manifests/, packs/): two levels above MSBuildBinPath (<root>/sdk/<version>/).
        var bin = project.GetPropertyValue("MSBuildBinPath");
        if (bin.Length > 0 && Directory.GetParent(bin.TrimEnd('/', '\\'))?.Parent is { } dotnetRoot)
        {
            roots.Add(Normalize(dotnetRoot.FullName));
        }

        _defaultRoots = [.. roots.Where(r => r.Length > 1)];
    }

    private static string Normalize(string path) =>
        Path.GetFullPath(path).TrimEnd('/', '\\') + Path.DirectorySeparatorChar;

    private bool IsProjectFile(string? file) =>
        file is not null && string.Equals(Path.GetFullPath(file), _projectPath, OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal);

    /// <summary>True for the SDK's, MSBuild's and NuGet's own files: their values are defaults.</summary>
    public bool IsDefaultFile(string? file)
    {
        if (string.IsNullOrEmpty(file))
        {
            return true;
        }

        var full = Path.GetFullPath(file);
        return _defaultRoots.Any(r => full.StartsWith(r, OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal));
    }

    public (string Value, string Source, DefinedIn? DefinedIn, string? Raw, string? InheritedFrom) Property(string name)
    {
        var p = _project.GetProperty(name);
        if (p is null || p.IsEnvironmentProperty || p.IsGlobalProperty || p.IsReservedProperty)
        {
            return (p?.EvaluatedValue ?? string.Empty, "default", null, null, null);
        }

        // The definition the person made (the project or an inherited file), even when the SDK's targets reassign it
        // afterwards (OutputPath, DefineConstants).
        var user = p;
        var guard = 0;
        while (user is not null && user.Xml is not null && IsDefaultFile(user.Xml.ContainingProject?.FullPath) && guard++ < 64)
        {
            user = user.Predecessor;
        }

        if (user?.Xml is null)
        {
            return (p.EvaluatedValue, "default", null, null, null);
        }

        var file = user.Xml.ContainingProject?.FullPath ?? _projectPath;
        var condition = Condition(user.Xml);
        var definedIn = new DefinedIn(file, Math.Max(1, user.Xml.Location.Line)) { Condition = condition.Length > 0 ? condition : null };
        if (IsProjectFile(file))
        {
            return (user.EvaluatedValue, condition.Length > 0 ? "conditioned" : "project", definedIn, user.UnevaluatedValue, null);
        }

        return (user.EvaluatedValue, "inherited", definedIn, user.UnevaluatedValue, file);
    }

    /// <summary>An SDK-style project's build event: the <c>Exec</c> of the <paramref name="target"/> target, else the property.</summary>
    public (string Value, string Source, DefinedIn? DefinedIn, string? Raw, string? InheritedFrom) Event(string property, string target)
    {
        foreach (var t in _project.Xml.Targets)
        {
            if (!string.Equals(t.Name, target, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }

            var exec = t.Tasks.FirstOrDefault(x => string.Equals(x.Name, "Exec", StringComparison.OrdinalIgnoreCase));
            if (exec is not null)
            {
                var command = exec.GetParameter("Command");
                return (command, "project", new DefinedIn(_projectPath, Math.Max(1, exec.Location.Line)), command, null);
            }
        }

        return Property(property);
    }

    /// <summary>The configuration conditions under which the project file itself defines <paramref name="name"/>.</summary>
    public List<string> Conditions(string name)
    {
        var list = new List<string>();
        foreach (var group in _project.Xml.PropertyGroups)
        {
            foreach (var p in group.Properties)
            {
                if (string.Equals(p.Name, name, StringComparison.OrdinalIgnoreCase)
                    && Condition(p) is { Length: > 0 } c
                    && ConfigurationCondition.IsConfigurationCondition(c)
                    && !list.Contains(c, StringComparer.Ordinal))
                {
                    list.Add(c);
                }
            }
        }

        return list;
    }

    /// <summary>The element's condition, else its property group's.</summary>
    public static string Condition(ProjectPropertyElement element) =>
        element.Condition.Length > 0 ? element.Condition : element.Parent?.Condition ?? string.Empty;
}
