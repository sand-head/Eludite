using System.Diagnostics;
using System.Globalization;
using System.Text;
using Eludite.Host.Legacy;
using Eludite.Host.Projects;
using Eludite.Host.Rpc;

namespace Eludite.Host.Resources;

/// <summary>
/// <c>eludite/resx/sets</c> and <c>eludite/resx/designer</c> (proposal 0005; host-rpc.md, "Resources"). The sets are
/// read from the evaluated projects of the generation (nothing inside a <c>.resx</c> is parsed) on the thread pool and
/// cached per generation and project; a read whose generation moves on fails with -32801. The designer actions carry
/// the generation they were read under, run one at a time, and <c>setModifier</c> reloads the solution after it
/// changed the project file (the generation moves on).
/// </summary>
public sealed class ResxService : IDisposable
{
    private const string Internal = "internal";
    private const string Public = "public";
    private const string None = "none";
    private const string InternalGenerator = "ResXFileCodeGenerator";
    private const string PublicGenerator = "PublicResXFileCodeGenerator";

    private static readonly StringComparison PathComparison = OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal;
    private static readonly StringComparer PathComparer = OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal;

    private readonly Func<(long Generation, string? Path, CancellationToken GenerationChanged)> _current;
    private readonly Func<long> _generation;
    private readonly Func<string, long> _reload;
    private readonly TextWriter _log;
    private readonly ResxProjectEvaluator _evaluator;
    private readonly Lock _lock = new();
    private readonly SemaphoreSlim _writes = new(1, 1);
    private readonly Dictionary<string, Task<ProjectSets>> _cache = new(StringComparer.Ordinal);
    private long _cacheGeneration = -1;

    /// <param name="current">The current generation, the open solution and a token canceled when the generation moves on.</param>
    /// <param name="generation">The current generation, for the error data of a stale request.</param>
    /// <param name="reload">Reopens the solution after a project write (as <c>eludite/solution/open</c>); returns the new generation.</param>
    public ResxService(
        Func<(long Generation, string? Path, CancellationToken GenerationChanged)> current,
        Func<long> generation,
        Func<string, long> reload,
        TextWriter log,
        ResxProjectEvaluator? evaluator = null)
    {
        _current = current;
        _generation = generation;
        _reload = reload;
        _log = log;
        _evaluator = evaluator ?? new ResxProjectEvaluator();
    }

    /// <summary>MSBuild evaluations run so far (the per-generation cache's tests).</summary>
    public int Evaluations => _evaluator.Evaluations;

    public void Dispose()
    {
        _evaluator.Dispose();
        _writes.Dispose();
    }

    public async Task<ResxSetsResult> SetsAsync(ResxSetsParams? parameters, CancellationToken cancellationToken)
    {
        var (generation, solution, changed) = _current();
        if (solution is null)
        {
            throw HostErrors.BadParams("no solution is open");
        }

        if (parameters?.Generation is not { } requested)
        {
            throw HostErrors.BadParams("generation is required");
        }

        if (requested != generation)
        {
            throw HostErrors.Stale(requested, generation);
        }

        var projects = ProjectsOf(solution, parameters.Projects);
        var sets = new List<ResxSet>();
        var skipped = new List<ResxSkippedProject>();
        foreach (var project in projects)
        {
            var listing = await ListAsync(generation, project, changed, cancellationToken).ConfigureAwait(false);
            sets.AddRange(listing.Sets);
            if (listing.Skipped is { } reason)
            {
                skipped.Add(new ResxSkippedProject(project, reason));
            }
        }

        sets.Sort(CompareSets);
        return new ResxSetsResult(generation, sets) { Skipped = skipped.Count > 0 ? skipped : null };
    }

    public async Task<ResxDesignerResult> DesignerAsync(ResxDesignerParams? parameters, CancellationToken cancellationToken)
    {
        if (parameters is null)
        {
            throw HostErrors.BadParams("generation, path and action are required");
        }

        var path = parameters.Path;
        if (string.IsNullOrEmpty(path) || !Path.IsPathRooted(path) || !path.EndsWith(".resx", StringComparison.OrdinalIgnoreCase))
        {
            throw HostErrors.BadParams("path (an absolute .resx file path) is required");
        }

        path = Path.GetFullPath(path);
        var action = parameters.Action;
        if (action is not ("generate" or "setModifier" or "delete"))
        {
            throw HostErrors.BadParams("action must be generate, setModifier or delete");
        }

        var modifier = parameters.Modifier;
        if (modifier is not (null or Internal or Public or None))
        {
            throw HostErrors.BadParams("modifier must be internal, public or none");
        }

        if (action == "setModifier" && modifier is null)
        {
            throw HostErrors.BadParams("modifier is required for setModifier");
        }

        var (generation, solution) = Pinned(parameters.Generation);
        await _writes.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            Pinned(parameters.Generation);
            var (_, _, changed) = _current();
            var set = await FindSetAsync(generation, solution, path, changed, cancellationToken).ConfigureAwait(false);
            if (set is null)
            {
                return new ResxDesignerResult(generation, path, None, None);
            }

            var sw = Stopwatch.StartNew();
            var result = action switch
            {
                "generate" => Generate(generation, set, modifier),
                "delete" => Delete(generation, set),
                _ => await SetModifierAsync(generation, solution, set, modifier!, cancellationToken).ConfigureAwait(false),
            };
            _log.WriteLine($"[resx] {action} {Path.GetFileName(set.Path)}: {result.Status} ({result.Modifier}) in {sw.ElapsedMilliseconds} ms (generation {generation} -> {result.Generation})");
            return result;
        }
        finally
        {
            _writes.Release();
        }
    }

    private static ResxDesignerResult Generate(long generation, ResxSet set, string? requested)
    {
        // The designer is read from disk, not the cached set: an earlier generate in this generation may have written it.
        var designer = set.Designer ?? DefaultDesigner(set);
        var exists = File.Exists(designer);
        var modifier = ModifierOf(set.Generator)
            ?? (exists ? DesignerGenerator.ReadClassModifier(designer) : null)
            ?? (set.Generator is null && !exists ? requested ?? Internal : None);
        if (modifier == None)
        {
            return new ResxDesignerResult(generation, set.Path, None, None) { Designer = exists ? designer : null };
        }

        var (status, className) = Write(set, designer, modifier);
        return new ResxDesignerResult(generation, set.Path, status, modifier) { Designer = designer, ClassName = className, Namespace = set.Namespace };
    }

    private static ResxDesignerResult Delete(long generation, ResxSet set)
    {
        var modifier = ModifierOf(set.Generator) ?? None;
        var designer = set.Designer ?? DefaultDesigner(set);
        if (File.Exists(designer))
        {
            File.Delete(designer);
            return new ResxDesignerResult(generation, set.Path, "deleted", modifier) { Designer = designer };
        }

        return new ResxDesignerResult(generation, set.Path, None, modifier) { Designer = set.Designer };
    }

    private async Task<ResxDesignerResult> SetModifierAsync(long generation, string solution, ResxSet set, string modifier, CancellationToken cancellationToken)
    {
        var designer = set.Designer ?? DefaultDesigner(set);
        var generator = modifier switch
        {
            Internal => InternalGenerator,
            Public => PublicGenerator,
            _ => null,
        };
        var projectWritten = await Task.Run(() => _evaluator.SetGenerator(set.Project, set.Path, designer, generator, cancellationToken), cancellationToken).ConfigureAwait(false);
        string status;
        string? className = null;
        if (generator is null)
        {
            status = "none";
            if (File.Exists(designer))
            {
                File.Delete(designer);
                status = "deleted";
            }
        }
        else
        {
            (status, className) = Write(set, designer, modifier);
        }

        var next = projectWritten ? Reload(solution) : generation;
        return new ResxDesignerResult(next, set.Path, status, modifier)
        {
            Designer = designer,
            ProjectWritten = projectWritten,
            ClassName = className,
            Namespace = generator is null ? null : set.Namespace,
        };
    }

    /// <summary>Generates the designer and writes it when its bytes differ from the file's.</summary>
    private static (string Status, string ClassName) Write(ResxSet set, string designer, string modifier)
    {
        if (set.Project.EndsWith(".vbproj", StringComparison.OrdinalIgnoreCase))
        {
            throw HostErrors.BadParams($"{set.Project}: Visual Basic designer files are not generated");
        }

        var className = DesignerGenerator.MakeIdentifier(set.BaseName)
            ?? throw HostErrors.BadParams($"{set.BaseName} cannot be made a class name");
        var bytes = DesignerGenerator.Bytes(DesignerGenerator.Generate(set.Path, set.Namespace, className, set.ManifestName, modifier));
        if (File.Exists(designer) && File.ReadAllBytes(designer).AsSpan().SequenceEqual(bytes))
        {
            return ("unchanged", className);
        }

        TextFileFormat.WriteAtomically(designer, bytes);
        return ("written", className);
    }

    private static string DefaultDesigner(ResxSet set) =>
        Path.Combine(Path.GetDirectoryName(set.Path)!, set.BaseName + (set.Project.EndsWith(".vbproj", StringComparison.OrdinalIgnoreCase) ? ".Designer.vb" : ".Designer.cs"));

    /// <summary>The set <paramref name="path"/> names: its neutral file, or one of its culture files.</summary>
    private async Task<ResxSet?> FindSetAsync(long generation, string solution, string path, CancellationToken changed, CancellationToken cancellationToken)
    {
        foreach (var project in ProjectsOf(solution, null))
        {
            var listing = await ListAsync(generation, project, changed, cancellationToken).ConfigureAwait(false);
            var set = listing.Sets.FirstOrDefault(s => string.Equals(s.Path, path, PathComparison) || s.Cultures.Any(c => string.Equals(c.Path, path, PathComparison)));
            if (set is not null)
            {
                return set;
            }
        }

        return null;
    }

    private async Task<ProjectSets> ListAsync(long generation, string project, CancellationToken changed, CancellationToken cancellationToken)
    {
        Task<ProjectSets> task;
        lock (_lock)
        {
            Invalidate(generation);
            if (!_cache.TryGetValue(project, out var cached) || cached.IsFaulted || cached.IsCanceled)
            {
                cached = Task.Run(() => Evaluate(project, generation, changed), CancellationToken.None);
                _cache[project] = cached;
            }

            task = cached;
        }

        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, changed);
        try
        {
            return await task.WaitAsync(linked.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            throw HostErrors.Stale(generation, _generation());
        }
    }

    private ProjectSets Evaluate(string project, long generation, CancellationToken changed)
    {
        var sw = Stopwatch.StartNew();
        ResxProjectInfo info;
        try
        {
            info = _evaluator.Evaluate(project, changed);
        }
        catch (StreamJsonRpc.LocalRpcException ex) when (ex.ErrorCode == HostErrors.InvalidParams)
        {
            _log.WriteLine($"[resx] {Path.GetFileName(project)} skipped: {ex.Message}");
            return new ProjectSets([], ex.Message);
        }

        var sets = SetsOf(info);
        _log.WriteLine($"[resx] {Path.GetFileName(project)}: {sets.Count} sets in {sw.ElapsedMilliseconds} ms (generation {generation})");
        return new ProjectSets(sets, null);
    }

    /// <summary>The sets of one evaluated project (the grouping and naming rules of host-rpc.md, "Resources").</summary>
    internal static List<ResxSet> SetsOf(ResxProjectInfo info)
    {
        var projectDir = Path.GetDirectoryName(info.Project)!;
        var projectName = Path.GetFileNameWithoutExtension(info.Project);
        var kind = info.Legacy ? "legacy" : "sdk";
        var vb = info.Project.EndsWith(".vbproj", StringComparison.OrdinalIgnoreCase);
        var neutralItems = new Dictionary<string, ResxItem>(PathComparer);
        var cultureItems = new Dictionary<string, List<(string Name, string Path)>>(PathComparer);
        var order = new List<string>();
        foreach (var item in info.Items)
        {
            var (neutral, culture) = Classify(item.FullPath);
            if (!neutralItems.ContainsKey(neutral) && !cultureItems.ContainsKey(neutral))
            {
                order.Add(neutral);
            }

            if (culture is null)
            {
                neutralItems.TryAdd(neutral, item);
            }
            else
            {
                if (!cultureItems.TryGetValue(neutral, out var list))
                {
                    list = [];
                    cultureItems[neutral] = list;
                }

                list.Add((culture, item.FullPath));
            }
        }

        var sets = new List<ResxSet>(order.Count);
        foreach (var neutral in order)
        {
            neutralItems.TryGetValue(neutral, out var item);
            var dir = Path.GetDirectoryName(neutral)!;
            var baseName = Path.GetFileNameWithoutExtension(neutral);
            var listed = cultureItems.TryGetValue(neutral, out var l) ? l : [];
            var cultures = listed.Select(c => new ResxCulture(c.Name, c.Path, true)).ToList();
            if (Directory.Exists(dir))
            {
                foreach (var file in Directory.EnumerateFiles(dir, baseName + ".*.resx"))
                {
                    var full = Path.GetFullPath(file);
                    var (n, culture) = Classify(full);
                    if (culture is not null && string.Equals(n, neutral, PathComparison) && !cultures.Any(c => string.Equals(c.Path, full, PathComparison)))
                    {
                        cultures.Add(new ResxCulture(culture, full, false));
                    }
                }
            }

            cultures.Sort((a, b) => StringComparer.OrdinalIgnoreCase.Compare(a.Name, b.Name));

            var folders = Folders(projectDir, dir, item?.Include);
            var designer = item?.LastGenOutput is { } output
                ? Path.GetFullPath(Path.Combine(dir, output.Replace('\\', Path.DirectorySeparatorChar)))
                : Path.Combine(dir, baseName + (vb ? ".Designer.vb" : ".Designer.cs")) is var candidate && File.Exists(candidate) ? candidate : null;
            var accessModifier = ModifierOf(item?.Generator)
                ?? (designer is not null && File.Exists(designer) ? DesignerGenerator.ReadClassModifier(designer) : null)
                ?? None;
            var manifestName = item?.LogicalName ?? item?.ManifestResourceName ?? Join(info.RootNamespace, folders, baseName);
            var @namespace = item?.CustomToolNamespace ?? Join(info.RootNamespace, folders, null);
            sets.Add(new ResxSet(info.Project, projectName, kind, neutral, baseName, info.RootNamespace, cultures, accessModifier, manifestName, @namespace.Length > 0 ? @namespace : info.RootNamespace)
            {
                NeutralLanguage = info.NeutralLanguage,
                Generator = item?.Generator,
                CustomToolNamespace = item?.CustomToolNamespace,
                LastGenOutput = item?.LastGenOutput,
                Designer = designer,
            });
        }

        return sets;
    }

    /// <summary>The neutral file's path and the culture name: <c>Name.&lt;culture&gt;.resx</c> when the suffix is a .NET culture.</summary>
    internal static (string Neutral, string? Culture) Classify(string path)
    {
        var name = Path.GetFileNameWithoutExtension(path);
        var dot = name.LastIndexOf('.');
        if (dot > 0 && dot < name.Length - 1)
        {
            var suffix = name[(dot + 1)..];
            if (IsCulture(suffix))
            {
                return (Path.Combine(Path.GetDirectoryName(path)!, name[..dot] + ".resx"), suffix);
            }
        }

        return (path, null);
    }

    internal static bool IsCulture(string name)
    {
        try
        {
            CultureInfo.GetCultureInfo(name, predefinedOnly: true);
            return true;
        }
        catch (CultureNotFoundException)
        {
            return false;
        }
    }

    private static string? ModifierOf(string? generator) => generator switch
    {
        InternalGenerator => Internal,
        PublicGenerator => Public,
        _ => null,
    };

    /// <summary>
    /// The folder segments between the project folder and the file (from the item's Include when the file is outside
    /// the project folder), each made an identifier as MSBuild's manifest resource naming does.
    /// </summary>
    private static List<string> Folders(string projectDir, string fileDir, string? include)
    {
        var relative = Path.GetRelativePath(projectDir, fileDir);
        if (relative == "." || relative.StartsWith("..", StringComparison.Ordinal) || Path.IsPathRooted(relative))
        {
            var includeDir = include is null || Path.IsPathRooted(include) ? null : Path.GetDirectoryName(include.Replace('\\', Path.DirectorySeparatorChar));
            if (string.IsNullOrEmpty(includeDir) || includeDir.StartsWith("..", StringComparison.Ordinal))
            {
                return [];
            }

            relative = includeDir;
        }

        return [.. relative.Split([Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar], StringSplitOptions.RemoveEmptyEntries).Select(EverettIdentifier)];
    }

    /// <summary>A folder name as a namespace segment: each dot-separated part with its invalid characters replaced by <c>_</c>.</summary>
    private static string EverettIdentifier(string folder)
    {
        var sb = new StringBuilder(folder.Length + 1);
        foreach (var part in folder.Split('.'))
        {
            if (sb.Length > 0)
            {
                sb.Append('.');
            }

            if (part.Length > 0 && !(char.IsLetter(part[0]) || part[0] == '_'))
            {
                sb.Append('_');
            }

            foreach (var c in part)
            {
                sb.Append(char.IsLetterOrDigit(c) || c == '_' ? c : '_');
            }
        }

        return sb.ToString();
    }

    private static string Join(string rootNamespace, List<string> folders, string? baseName)
    {
        var parts = new List<string>(folders.Count + 2);
        if (rootNamespace.Length > 0)
        {
            parts.Add(rootNamespace);
        }

        parts.AddRange(folders);
        if (baseName is not null)
        {
            parts.Add(baseName);
        }

        return string.Join('.', parts);
    }

    private static int CompareSets(ResxSet a, ResxSet b)
    {
        var byProject = string.CompareOrdinal(a.Project, b.Project);
        return byProject != 0 ? byProject : string.CompareOrdinal(a.Path, b.Path);
    }

    /// <summary>The current generation and solution, checked against the one a write was read under.</summary>
    private (long Generation, string Solution) Pinned(long? requested)
    {
        var (generation, solution, _) = _current();
        if (requested is null)
        {
            throw HostErrors.BadParams("generation (the generation the set was read under) is required");
        }

        if (requested != generation)
        {
            throw HostErrors.Stale(requested.Value, generation);
        }

        if (solution is null)
        {
            throw HostErrors.BadParams("no solution is open");
        }

        return (generation, solution);
    }

    private long Reload(string solution)
    {
        lock (_lock)
        {
            _cache.Clear();
            _cacheGeneration = -1;
        }

        _evaluator.Reset();
        return _reload(solution);
    }

    private void Invalidate(long generation)
    {
        if (_cacheGeneration != generation)
        {
            _cache.Clear();
            _cacheGeneration = generation;
            _evaluator.Reset();
        }
    }

    /// <summary>The solution's project files, or the <paramref name="requested"/> ones checked to be among them.</summary>
    private static List<string> ProjectsOf(string solution, IReadOnlyList<string>? requested)
    {
        List<string> projects;
        try
        {
            projects = solution.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase) || solution.EndsWith(".vbproj", StringComparison.OrdinalIgnoreCase)
                ? [Path.GetFullPath(solution)]
                : [.. SolutionProjects.Read(solution)];
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            throw HostErrors.BadParams($"cannot read {solution}: {ex.Message}");
        }

        if (requested is null)
        {
            return projects;
        }

        var chosen = new List<string>(requested.Count);
        foreach (var project in requested)
        {
            if (string.IsNullOrEmpty(project) || !Path.IsPathRooted(project))
            {
                throw HostErrors.BadParams("projects must be absolute project file paths");
            }

            var full = Path.GetFullPath(project);
            if (!projects.Any(p => string.Equals(p, full, PathComparison)))
            {
                throw HostErrors.BadParams($"{full} is not a project of {solution}");
            }

            if (!chosen.Contains(full, PathComparer))
            {
                chosen.Add(full);
            }
        }

        return chosen;
    }

    private sealed record ProjectSets(List<ResxSet> Sets, string? Skipped);
}
