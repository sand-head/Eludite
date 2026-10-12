using System.Diagnostics;
using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;

namespace Eludite.Host.Projects;

// Wire shapes of eludite/project/editItems and eludite/solution/reload (brief 0073;
// protocol/schemas/host/project-edit-items.json, solution-reload.json). camelCase, nulls omitted.

public sealed record ProjectEditItemsParams(string? Project, IReadOnlyList<ProjectItemEdit>? Edits);

/// <summary>One edit: <see cref="Op"/> is include, exclude, rename, remove, moveUp, moveDown or addFolder.</summary>
public sealed record ProjectItemEdit(string? Op, string? Path)
{
    /// <summary>rename: the new absolute path.</summary>
    public string? To { get; init; }

    /// <summary>include: the item type to add (default by extension).</summary>
    public string? ItemType { get; init; }
}

public sealed record ProjectItemEditResult(string Op, string Path, bool Applied)
{
    /// <summary>Why nothing changed, when it is worth saying.</summary>
    public string? Message { get; init; }
}

public sealed record ProjectEditItemsResult(bool Changed, long Generation, IReadOnlyList<ProjectItemEditResult> Edits);

public sealed record SolutionReloadParams(bool ProjectFiles);

/// <summary>
/// <c>eludite/project/editItems</c> and <c>eludite/solution/reload</c> (brief 0073): the Workspace window's project
/// edits (<see cref="ProjectItemEditor"/>, one at a time, off the RPC thread, under the in-process MSBuild's lock) and
/// the reload its watcher asks for. A write that changes the project file reopens the solution, as a property write
/// does (brief 0049), so the generation moves on and the shell's next <c>eludite/solution/tree</c> sees the edit.
/// </summary>
public sealed class ProjectItemsService : IDisposable
{
    private static readonly string[] Ops = ["include", "exclude", "rename", "remove", "moveUp", "moveDown", "addFolder"];
    private static readonly string[] ProjectExtensions = [".csproj", ".vbproj", ".fsproj"];

    private readonly Func<(long Generation, string? Path, CancellationToken GenerationChanged)> _current;
    private readonly Func<long> _generation;
    private readonly Func<string, long> _reload;
    private readonly Func<long> _advance;
    private readonly TextWriter _log;
    private readonly SemaphoreSlim _writes = new(1, 1);

    /// <param name="current">The current generation, the open solution and a token canceled when the generation moves on.</param>
    /// <param name="generation">The current generation.</param>
    /// <param name="reload">Reopens the solution (as <c>eludite/solution/open</c>); returns the new generation.</param>
    /// <param name="advance">Advances the generation without reopening (as after a NuGet change); returns the new generation.</param>
    public ProjectItemsService(
        Func<(long Generation, string? Path, CancellationToken GenerationChanged)> current,
        Func<long> generation,
        Func<string, long> reload,
        Func<long> advance,
        TextWriter log)
    {
        _current = current;
        _generation = generation;
        _reload = reload;
        _advance = advance;
        _log = log;
    }

    public void Dispose() => _writes.Dispose();

    public async Task<ProjectEditItemsResult> EditItemsAsync(ProjectEditItemsParams? parameters, CancellationToken cancellationToken)
    {
        var (project, edits) = Validate(parameters);
        await _writes.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var sw = Stopwatch.StartNew();
            var outcome = await Task.Run(() => Edit(project, edits, cancellationToken), cancellationToken).ConfigureAwait(false);
            var solution = _current().Path;
            var generation = outcome.Changed && solution is not null ? _reload(solution) : _generation();

            _log.WriteLine($"[items] {Path.GetFileName(project)}: {string.Join(", ", outcome.Results.Select(r => $"{r.Op} {(r.Applied ? "applied" : "unchanged")}"))} in {sw.ElapsedMilliseconds} ms (generation {generation})");
            return new ProjectEditItemsResult(outcome.Changed, generation, outcome.Results);
        }
        finally
        {
            _writes.Release();
        }
    }

    /// <summary>
    /// <c>projectFiles</c> true reopens the open solution (a project, props, targets or solution file changed); false
    /// advances the generation so the next tree evaluates the globs again. With no solution open, nothing happens.
    /// </summary>
    public GenerationResult Reload(SolutionReloadParams? parameters)
    {
        if (parameters is null)
        {
            throw HostErrors.BadParams("projectFiles is required");
        }

        var (generation, solution, _) = _current();
        if (solution is null)
        {
            return new GenerationResult(generation);
        }

        var next = parameters.ProjectFiles ? _reload(solution) : _advance();
        _log.WriteLine($"[items] reload ({(parameters.ProjectFiles ? "project files" : "files")}): generation {generation} -> {next}");
        return new GenerationResult(next);
    }

    /// <summary>The edits applied under the in-process MSBuild's lock (the service's work, for tests too).</summary>
    internal static ProjectItemsOutcome Edit(string project, IReadOnlyList<ProjectItemEdit> edits, CancellationToken cancellationToken)
    {
        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            throw HostErrors.BadParams("no .NET SDK found by Microsoft.Build.Locator: project edits need MSBuild");
        }

        lock (MsBuildProjectTreeEvaluator.EvaluationLock)
        {
            return EditCore(project, edits, cancellationToken);
        }
    }

    // Kept apart so no Microsoft.Build type is touched before the locator has registered.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static ProjectItemsOutcome EditCore(string project, IReadOnlyList<ProjectItemEdit> edits, CancellationToken cancellationToken) =>
        ProjectItemEditor.Apply(project, edits, cancellationToken);

    private static (string Project, IReadOnlyList<ProjectItemEdit> Edits) Validate(ProjectEditItemsParams? parameters)
    {
        if (parameters?.Project is not { Length: > 0 } project || !Path.IsPathRooted(project))
        {
            throw HostErrors.BadParams("project (an absolute project file path) is required");
        }

        var full = Path.GetFullPath(project);
        if (!ProjectExtensions.Contains(Path.GetExtension(full), StringComparer.OrdinalIgnoreCase))
        {
            throw HostErrors.BadParams($"{full}: expected a .csproj, .vbproj or .fsproj file");
        }

        if (!File.Exists(full))
        {
            throw HostErrors.BadParams($"{full} does not exist");
        }

        if (parameters.Edits is not { Count: > 0 and <= 1000 } edits)
        {
            throw HostErrors.BadParams("edits is required (1 to 1000)");
        }

        foreach (var edit in edits)
        {
            if (edit is null || !Ops.Contains(edit.Op, StringComparer.Ordinal))
            {
                throw HostErrors.BadParams($"op must be one of {string.Join(", ", Ops)}");
            }

            if (edit.Path is not { Length: > 0 } path || !Path.IsPathRooted(path))
            {
                throw HostErrors.BadParams($"{edit.Op}: path (an absolute path) is required");
            }

            if (edit.Op == "rename" && (edit.To is not { Length: > 0 } to || !Path.IsPathRooted(to)))
            {
                throw HostErrors.BadParams("rename: to (the new absolute path) is required");
            }
        }

        return (full, edits);
    }
}
