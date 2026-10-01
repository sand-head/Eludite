using System.Diagnostics;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;

namespace Eludite.Host.Projects;

/// <summary>
/// <c>eludite/solution/tree</c>: evaluates the open solution's projects once per generation, in the background, and
/// answers every request for that generation from the same result. A request whose generation changes before the
/// tree is ready fails with ContentModified (-32801).
/// </summary>
public sealed class SolutionTreeProvider
{
    private readonly IProjectTreeEvaluator _evaluator;
    private readonly TextWriter _log;
    private readonly Lock _lock = new();
    private long _cachedGeneration = -1;
    private Task<SolutionTree>? _cached;

    public SolutionTreeProvider(IProjectTreeEvaluator evaluator, TextWriter log)
    {
        _evaluator = evaluator;
        _log = log;
    }

    /// <summary>Evaluations started (one per generation that was asked for).</summary>
    public int Evaluations { get; private set; }

    /// <param name="generation">The current solution generation.</param>
    /// <param name="path">The open solution or project file, or null.</param>
    /// <param name="generationChanged">Canceled when the generation moves on.</param>
    /// <param name="currentGeneration">Reads the generation at the time of a failure, for the error data.</param>
    public async Task<SolutionTree> GetAsync(long generation, string? path, CancellationToken generationChanged, Func<long> currentGeneration, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(currentGeneration);
        if (path is null)
        {
            return new SolutionTree(generation, null, []);
        }

        Task<SolutionTree> task;
        lock (_lock)
        {
            if (_cached is null || _cachedGeneration != generation)
            {
                _cachedGeneration = generation;
                Evaluations++;
                _cached = Task.Run(() => Build(generation, path, generationChanged), CancellationToken.None);
            }

            task = _cached;
        }

        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, generationChanged);
        try
        {
            return await task.WaitAsync(linked.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            throw HostErrors.Stale(generation, currentGeneration());
        }
    }

    private SolutionTree Build(long generation, string path, CancellationToken generationChanged)
    {
        var sw = Stopwatch.StartNew();
        IReadOnlyList<string> projects;
        try
        {
            projects = SolutionProjects.Read(path);
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            _log.WriteLine($"[tree] cannot read {path}: {ex.Message}");
            projects = [];
        }

        var result = _evaluator.Evaluate(projects, generationChanged);
        _log.WriteLine($"[tree] generation {generation}: {result.Count} projects, {result.Sum(p => p.Files.Count)} files in {sw.ElapsedMilliseconds} ms");
        return new SolutionTree(generation, path, result);
    }
}
