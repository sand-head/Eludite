using System.Diagnostics;
using System.Text.Json;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;

namespace Eludite.Host.Projects;

/// <summary>
/// <c>eludite/project/*</c> and <c>eludite/solution/configurations</c>, <c>setConfiguration</c> (brief 0049; host-rpc.md,
/// "Project properties"). Reads run on the thread pool and are cached per generation, project, configuration, platform
/// and framework; a read whose generation moves on fails with -32801. Writes carry the generation they were read under,
/// run one at a time, and reload the solution when they change a project or solution file (the generation moves on).
/// </summary>
public sealed class ProjectPropertiesService : IDisposable
{
    private readonly Func<(long Generation, string? Path, CancellationToken GenerationChanged)> _current;
    private readonly Func<long> _generation;
    private readonly Func<string, long> _reload;
    private readonly TextWriter _log;
    private readonly ProjectPropertyEvaluator _evaluator;
    private readonly Lock _lock = new();
    private readonly SemaphoreSlim _writes = new(1, 1);
    private readonly Dictionary<string, Task<ProjectPropertiesResult>> _cache = new(StringComparer.Ordinal);
    private readonly Dictionary<string, Selection> _selection = new(StringComparer.Ordinal);
    private long _cacheGeneration = -1;
    private SolutionConfigurationsResult? _configurations;

    /// <param name="current">The current generation, the open solution and a token canceled when the generation moves on.</param>
    /// <param name="generation">The current generation, for the error data of a stale request.</param>
    /// <param name="reload">Reopens the solution after a write (as <c>eludite/solution/open</c>); returns the new generation.</param>
    public ProjectPropertiesService(
        Func<(long Generation, string? Path, CancellationToken GenerationChanged)> current,
        Func<long> generation,
        Func<string, long> reload,
        TextWriter log,
        ProjectPropertyEvaluator? evaluator = null)
    {
        _current = current;
        _generation = generation;
        _reload = reload;
        _log = log;
        _evaluator = evaluator ?? new ProjectPropertyEvaluator();
    }

    /// <summary>MSBuild evaluations run so far (the per-generation cache's tests).</summary>
    public int Evaluations => _evaluator.Evaluations;

    public void Dispose()
    {
        _evaluator.Dispose();
        _writes.Dispose();
    }

    public async Task<ProjectPropertiesResult> PropertiesAsync(ProjectPropertiesParams? parameters, CancellationToken cancellationToken)
    {
        var (generation, solution, changed) = _current();
        var project = ProjectOf(solution, parameters?.Project);
        var configuration = parameters?.Configuration;
        var platform = parameters?.Platform;
        if (configuration is null || platform is null)
        {
            var mapped = Mapped(generation, solution!, project);
            configuration ??= mapped?.Configuration;
            platform ??= mapped?.Platform;
        }

        var framework = parameters?.Framework;
        var key = string.Join('|', project, configuration ?? string.Empty, platform is null ? string.Empty : ConfigurationCondition.ProjectPlatform(platform), framework ?? string.Empty);
        Task<ProjectPropertiesResult> task;
        lock (_lock)
        {
            Invalidate(generation);
            if (!_cache.TryGetValue(key, out var cached) || cached.IsFaulted || cached.IsCanceled)
            {
                cached = Task.Run(() => Evaluate(project, configuration, platform, framework, generation, changed), CancellationToken.None);
                _cache[key] = cached;
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

    private ProjectPropertiesResult Evaluate(string project, string? configuration, string? platform, string? framework, long generation, CancellationToken changed)
    {
        var sw = Stopwatch.StartNew();
        var result = _evaluator.Evaluate(project, configuration, platform, framework, generation, changed);
        _log.WriteLine($"[properties] {Path.GetFileName(project)} {result.Configuration}|{result.Platform}{(framework is null ? string.Empty : "|" + framework)} evaluated in {sw.ElapsedMilliseconds} ms (generation {generation})");
        return result;
    }

    public async Task<ProjectSetPropertyResult> SetPropertyAsync(ProjectSetPropertyParams? parameters, CancellationToken cancellationToken)
    {
        if (parameters?.Edits is not { Count: > 0 } edits)
        {
            throw HostErrors.BadParams("edits is required (one or more)");
        }

        var (generation, solution) = Pinned(parameters.Generation);
        var project = ProjectOf(solution, parameters.Project);
        await _writes.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            Pinned(parameters.Generation);
            var sw = Stopwatch.StartNew();
            var outcome = await Task.Run(() => _evaluator.Edit(project, edits, cancellationToken), cancellationToken).ConfigureAwait(false);
            var next = generation;
            if (outcome.Written)
            {
                next = Reload(solution!);
            }

            _log.WriteLine($"[properties] {Path.GetFileName(project)}: {string.Join(", ", outcome.Results.Select(r => $"{r.Name} {r.Status}"))} in {sw.ElapsedMilliseconds} ms (generation {generation} -> {next})");
            return new ProjectSetPropertyResult(next, project, outcome.Written, outcome.Results);
        }
        finally
        {
            _writes.Release();
        }
    }

    public LaunchProfilesResult LaunchProfiles(LaunchProfilesParams? parameters)
    {
        var (generation, solution, _) = _current();
        var project = ProjectOf(solution, parameters?.Project);
        var (exists, profiles) = LaunchSettingsFile.Read(project);
        return new LaunchProfilesResult(generation, project, LaunchSettingsFile.PathFor(project), exists, profiles);
    }

    public async Task<LaunchProfilesResult> SetLaunchProfileAsync(SetLaunchProfileParams? parameters, CancellationToken cancellationToken)
    {
        if (parameters is null || string.IsNullOrEmpty(parameters.Action) || string.IsNullOrEmpty(parameters.Profile))
        {
            throw HostErrors.BadParams("action and profile are required");
        }

        CheckValues(parameters.Values);
        var (generation, solution) = Pinned(parameters.Generation);
        var project = ProjectOf(solution, parameters.Project);
        await _writes.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var web = File.ReadAllText(project).Contains("Microsoft.NET.Sdk.Web", StringComparison.OrdinalIgnoreCase);
            var profiles = LaunchSettingsFile.Apply(project, parameters.Action, parameters.Profile, parameters.NewName, parameters.Values, web);
            _log.WriteLine($"[properties] {Path.GetFileName(project)}: launch profile {parameters.Action} {parameters.Profile}");
            return new LaunchProfilesResult(generation, project, LaunchSettingsFile.PathFor(project), true, profiles);
        }
        finally
        {
            _writes.Release();
        }
    }

    public SolutionConfigurationsResult Configurations()
    {
        var (generation, solution, _) = _current();
        if (solution is null)
        {
            return new SolutionConfigurationsResult(generation, null, [], [], new Selection("Debug", "Any CPU"), []);
        }

        var model = Model(generation, solution);
        return model with { Generation = generation, Active = Active(solution, model) };
    }

    public async Task<SolutionSetConfigurationResult> SetConfigurationAsync(SolutionSetConfigurationParams? parameters, CancellationToken cancellationToken)
    {
        if (parameters is null)
        {
            throw HostErrors.BadParams("generation is required");
        }

        var (generation, solution) = Pinned(parameters.Generation);
        await _writes.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var model = Model(generation, solution!);
            if (parameters.Select is { } select)
            {
                var c = model.Configurations.FirstOrDefault(x => string.Equals(x, select.Configuration, StringComparison.OrdinalIgnoreCase));
                var p = model.Platforms.FirstOrDefault(x => string.Equals(x, select.Platform, StringComparison.OrdinalIgnoreCase));
                if (c is null || p is null)
                {
                    throw HostErrors.BadParams($"{select.Configuration}|{select.Platform} is not a configuration and platform of {solution}");
                }

                lock (_lock)
                {
                    _selection[solution!] = new Selection(c, p);
                }
            }

            var written = parameters.Mappings is { Count: > 0 } mappings && SolutionConfigurationFile.Edit(solution!, mappings);
            var next = written ? Reload(solution!) : generation;
            return new SolutionSetConfigurationResult(next, solution, written, Active(solution!, model));
        }
        finally
        {
            _writes.Release();
        }
    }

    /// <summary>The current generation and solution, checked against the one a write was read under.</summary>
    private (long Generation, string? Solution) Pinned(long? requested)
    {
        var (generation, solution, _) = _current();
        if (requested is null)
        {
            throw HostErrors.BadParams("generation (the generation the values were read under) is required");
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
            _configurations = null;
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
            _configurations = null;
            _cacheGeneration = generation;
            _evaluator.Reset();
        }
    }

    private SolutionConfigurationsResult Model(long generation, string solution)
    {
        lock (_lock)
        {
            Invalidate(generation);
            if (_configurations is null || _configurations.Path != solution)
            {
                try
                {
                    _configurations = SolutionConfigurationFile.Read(solution);
                }
                catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
                {
                    throw HostErrors.BadParams($"cannot read {solution}: {ex.Message}");
                }
            }

            return _configurations;
        }
    }

    private Selection Active(string solution, SolutionConfigurationsResult model)
    {
        lock (_lock)
        {
            if (_selection.TryGetValue(solution, out var s)
                && model.Configurations.Contains(s.Configuration, StringComparer.OrdinalIgnoreCase)
                && model.Platforms.Contains(s.Platform, StringComparer.OrdinalIgnoreCase))
            {
                return s;
            }
        }

        return new Selection(model.Configurations.FirstOrDefault() ?? "Debug", model.Platforms.FirstOrDefault() ?? "Any CPU");
    }

    /// <summary>The project configuration and platform the active selection maps <paramref name="project"/> to.</summary>
    private ConfigurationMapping? Mapped(long generation, string solution, string project)
    {
        try
        {
            var model = Model(generation, solution);
            var active = Active(solution, model);
            return model.Projects.FirstOrDefault(p => SamePath(p.Path, project))?.Mappings
                .FirstOrDefault(m => string.Equals(m.SolutionConfiguration, active.Configuration, StringComparison.OrdinalIgnoreCase)
                    && string.Equals(m.SolutionPlatform, active.Platform, StringComparison.OrdinalIgnoreCase));
        }
        catch (StreamJsonRpc.LocalRpcException)
        {
            return null;
        }
    }

    private static bool SamePath(string a, string b) =>
        string.Equals(Path.GetFullPath(a), Path.GetFullPath(b), OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal);

    /// <summary>The absolute project path, checked to be a project of the open solution.</summary>
    private static string ProjectOf(string? solution, string? project)
    {
        if (solution is null)
        {
            throw HostErrors.BadParams("no solution is open");
        }

        if (string.IsNullOrEmpty(project) || !Path.IsPathRooted(project))
        {
            throw HostErrors.BadParams("project (an absolute project file path) is required");
        }

        var full = Path.GetFullPath(project);
        if (SamePath(full, solution))
        {
            return full;
        }

        IReadOnlyList<string> projects;
        try
        {
            projects = SolutionProjects.Read(solution);
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            throw HostErrors.BadParams($"cannot read {solution}: {ex.Message}");
        }

        if (!projects.Any(p => SamePath(p, full)) && !SolutionConfigurationFile.Read(solution).Projects.Any(p => SamePath(p.Path, full)))
        {
            throw HostErrors.BadParams($"{full} is not a project of {solution}");
        }

        return full;
    }

    /// <summary><c>values</c> must be an object when given.</summary>
    private static void CheckValues(JsonElement? values)
    {
        if (values is { } v && v.ValueKind is not JsonValueKind.Object and not JsonValueKind.Null and not JsonValueKind.Undefined)
        {
            throw HostErrors.BadParams("values must be an object");
        }
    }
}
