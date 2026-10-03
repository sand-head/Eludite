using System.Diagnostics;
using System.Threading.Channels;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using Eludite.TestBridge;
using StreamJsonRpc;

namespace Eludite.Host.Testing;

/// <summary>
/// <c>eludite/test/*</c> (brief 0035; host-rpc.md, "Tests"): discovers and runs the open solution's tests out of process
/// through <see cref="ITestRunner"/>s (Microsoft.Testing.Platform's server mode, VSTest's translation layer), answers
/// each request at once with its id and streams <c>eludite/test/update</c> notifications in <c>seq</c> order, tests and
/// results batched every <see cref="FlushInterval"/>. One run at a time per container; a new solution generation
/// cancels everything and forgets the discovered tests; <c>eludite/test/status</c> answers what is going with what it
/// reported so far, so a shell that reconnects can replay it.
/// </summary>
public sealed class TestService : IAsyncDisposable
{
    /// <summary>Error code of a run naming a container a run is running.</summary>
    public const int TestRunInProgress = -32012;

    /// <summary>How often buffered tests, results and output are sent.</summary>
    public static readonly TimeSpan FlushInterval = TimeSpan.FromMilliseconds(16);

    private readonly Func<(long Generation, string? Path, CancellationToken GenerationChanged)> _currentSolution;
    private readonly TextWriter _log;
    private readonly Func<TestRunnerProtocol, ITestRunner> _runners;
    private readonly Lazy<string?> _mono;
    private readonly Lazy<string> _dotnet;
    private readonly Lock _lock = new();
    private readonly Dictionary<long, TestJob> _jobs = [];
    private readonly Dictionary<string, IReadOnlyList<DiscoveredTest>> _discovered = new(StringComparer.Ordinal);
    private long _discoveredGeneration = -1;
    private TestStatusLast? _last;
    private long _nextId;
    private JsonRpc? _shell;

    /// <param name="currentSolution">The generation, the open solution (or null) and a token canceled when the generation moves on.</param>
    /// <param name="runners">The runner for a protocol; default <see cref="MtpRunner"/> and <see cref="VsTestRunner"/>.</param>
    /// <param name="mono">Finds <c>mono</c>; default <see cref="TestContainers.LocateMono"/>.</param>
    public TestService(Func<(long Generation, string? Path, CancellationToken GenerationChanged)> currentSolution, TextWriter log, Func<TestRunnerProtocol, ITestRunner>? runners = null, Func<string?>? mono = null)
    {
        _currentSolution = currentSolution;
        _log = log;
        _runners = runners ?? (p => p == TestRunnerProtocol.MicrosoftTestingPlatform ? new MtpRunner() : new VsTestRunner());
        _mono = new Lazy<string?>(mono ?? TestContainers.LocateMono);
        _dotnet = new Lazy<string>(TestContainers.LocateDotnet);
    }

    /// <summary>Completes when no discovery or run is going.</summary>
    public Task Idle
    {
        get
        {
            lock (_lock)
            {
                return Task.WhenAll(_jobs.Values.Select(j => j.Completion).ToList());
            }
        }
    }

    internal TextWriter Log => _log;

    /// <summary>Sends notifications on the shell connection. Call before it starts listening.</summary>
    public void Attach(JsonRpc shell) => _shell = shell;

    /// <summary><c>eludite/test/discover</c>.</summary>
    public TestDiscoverResult Discover(TestDiscoverParams? parameters)
    {
        parameters ??= new TestDiscoverParams();
        var (generation, solution, changed) = Solution();
        var containers = Containers(solution, parameters.Projects, parameters.Configuration, parameters.RunSettings, parameters.VstestConsolePath, parallel: false);
        lock (_lock)
        {
            Forget(generation);
            var job = new TestJob(this, ++_nextId, "discover", generation, debug: false, containers, null, changed);
            _jobs[job.Id] = job;
            job.Start(job.DiscoverAllAsync);
            _log.WriteLine($"[test] #{job.Id} discover {containers.Count} container(s)");
            return new TestDiscoverResult(job.Id, generation, containers.Select(c => c.Info).ToList());
        }
    }

    /// <summary><c>eludite/test/run</c>.</summary>
    public TestRunResult Run(TestRunParams? parameters)
    {
        parameters ??= new TestRunParams();
        var (generation, solution, changed) = Solution();
        var all = Containers(solution, null, parameters.Configuration, parameters.RunSettings, parameters.VstestConsolePath, parameters.Parallel ?? false);
        lock (_lock)
        {
            Forget(generation);
            List<(TestContainer Container, TestContainerInfo Info)> chosen;
            Dictionary<string, IReadOnlyList<string>?> requested = new(StringComparer.Ordinal);
            if (parameters.Containers is { } named)
            {
                chosen = [];
                foreach (var c in named)
                {
                    var match = all.FirstOrDefault(a => a.Info.Id == c.Id);
                    if (match.Container is null)
                    {
                        throw HostErrors.BadParams($"{c.Id} is not a test container of the open solution");
                    }

                    if (!requested.ContainsKey(c.Id))
                    {
                        chosen.Add(match);
                    }

                    requested[c.Id] = c.Tests;
                }
            }
            else
            {
                // Every container of the last discovery under this generation, else every container.
                chosen = _discoveredGeneration == generation && _discovered.Count > 0
                    ? all.Where(a => _discovered.ContainsKey(a.Info.Id)).ToList()
                    : all;
                foreach (var c in chosen)
                {
                    requested[c.Info.Id] = null;
                }
            }

            var debug = parameters.Debug ?? false;
            if (debug && chosen.Count != 1)
            {
                throw HostErrors.BadParams("a debug run names exactly one container");
            }

            foreach (var job in _jobs.Values.Where(j => j.Kind == "run"))
            {
                if (chosen.FirstOrDefault(c => job.Covers(c.Info.Id)) is { Container: not null } busy)
                {
                    throw new LocalRpcException($"run {job.Id} is running {busy.Info.Name}")
                    {
                        ErrorCode = TestRunInProgress,
                        ErrorData = new { runId = job.Id, container = busy.Info.Id },
                    };
                }
            }

            var run = new TestJob(this, ++_nextId, "run", generation, debug, chosen, requested, changed);
            _jobs[run.Id] = run;
            run.Start(run.RunAllAsync);
            _log.WriteLine($"[test] #{run.Id} run {chosen.Count} container(s){(debug ? " under the debugger" : string.Empty)}");
            return new TestRunResult(run.Id, generation, chosen.Select(c => c.Info.Id).ToList()) { Debug = debug ? true : null };
        }
    }

    /// <summary><c>eludite/test/cancel</c>: the discovery or run named, else the last one started.</summary>
    public TestCancelResult Cancel(TestCancelParams? parameters)
    {
        TestJob? job;
        lock (_lock)
        {
            job = parameters?.RunId is { } id
                ? _jobs.GetValueOrDefault(id)
                : _jobs.Values.MaxBy(j => j.Id);
        }

        if (job is null)
        {
            return new TestCancelResult(false);
        }

        job.Cancel();
        return new TestCancelResult(true) { RunId = job.Id };
    }

    /// <summary><c>eludite/test/attached</c>: the shell's answer to an <c>attach</c> update.</summary>
    public TestAttachedResult Attached(TestAttachedParams parameters)
    {
        ArgumentNullException.ThrowIfNull(parameters);
        TestJob? job;
        lock (_lock)
        {
            job = _jobs.GetValueOrDefault(parameters.RunId);
        }

        return new TestAttachedResult(job?.Answer(parameters.ProcessId, parameters.Attached, parameters.Message) ?? false);
    }

    /// <summary><c>eludite/test/status</c>.</summary>
    public TestStatusResult Status()
    {
        lock (_lock)
        {
            return new TestStatusResult(_jobs.Values.OrderBy(j => j.Id).Select(j => j.Status()).ToList()) { Last = _last };
        }
    }

    public async ValueTask DisposeAsync()
    {
        List<TestJob> jobs;
        lock (_lock)
        {
            jobs = [.. _jobs.Values];
        }

        foreach (var job in jobs)
        {
            job.Cancel();
        }

        try
        {
            await Task.WhenAll(jobs.Select(j => j.Completion)).WaitAsync(TimeSpan.FromSeconds(10)).ConfigureAwait(false);
        }
        catch (TimeoutException)
        {
        }
    }

    internal ITestRunner Runner(TestRunnerProtocol protocol) => _runners(protocol);

    /// <summary>The tests discovered for a container under <paramref name="generation"/>, if any.</summary>
    internal IReadOnlyList<DiscoveredTest>? Discovered(long generation, string container)
    {
        lock (_lock)
        {
            return _discoveredGeneration == generation ? _discovered.GetValueOrDefault(container) : null;
        }
    }

    internal void Remember(long generation, string container, IReadOnlyList<DiscoveredTest> tests)
    {
        lock (_lock)
        {
            if (generation >= _discoveredGeneration)
            {
                Forget(generation);
                _discovered[container] = tests;
            }
        }
    }

    internal void Finished(TestJob job, TestStatusLast last)
    {
        lock (_lock)
        {
            _jobs.Remove(job.Id);
            _last = last;
        }
    }

    internal async Task NotifyAsync(TestUpdateParams update)
    {
        if (_shell is not { } shell)
        {
            return;
        }

        try
        {
            await shell.NotifyWithParameterObjectAsync("eludite/test/update", update).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is ObjectDisposedException or ConnectionLostException or IOException)
        {
            // The shell went away.
        }
    }

    /// <summary>Under <see cref="_lock"/>: a new generation forgets the old one's discovered tests.</summary>
    private void Forget(long generation)
    {
        if (_discoveredGeneration != generation)
        {
            _discovered.Clear();
            _discoveredGeneration = generation;
        }
    }

    private (long Generation, string Solution, CancellationToken Changed) Solution()
    {
        var (generation, solution, changed) = _currentSolution();
        if (solution is null)
        {
            throw HostErrors.BadParams("no solution is open");
        }

        return (generation, solution, changed);
    }

    private List<(TestContainer Container, TestContainerInfo Info)> Containers(string solution, IReadOnlyList<string>? projects, string? configuration, string? runSettings, string? vstestConsole, bool parallel)
    {
        IReadOnlyList<string> solutionProjects;
        try
        {
            solutionProjects = SolutionProjects.Read(solution);
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            throw HostErrors.BadParams($"the solution could not be read: {ex.Message}");
        }

        IEnumerable<string> chosen = solutionProjects;
        if (projects is { Count: > 0 })
        {
            var list = new List<string>();
            foreach (var p in projects)
            {
                var full = Path.GetFullPath(p);
                var match = solutionProjects.FirstOrDefault(s => Build.BuildService.PathEquals(s, full))
                    ?? throw HostErrors.BadParams($"{p} is not a project of the open solution");
                if (TestProjectInspector.Inspect(match) is null)
                {
                    throw HostErrors.BadParams($"{p} is not a test project");
                }

                list.Add(match);
            }

            chosen = list;
        }

        var settings = new TestContainers.Settings(_mono.Value, _dotnet.Value)
        {
            RunSettings = runSettings is { Length: > 0 } ? Path.GetFullPath(runSettings) : null,
            VsTestConsole = vstestConsole is { Length: > 0 } ? vstestConsole : null,
            Parallel = parallel,
        };
        return [.. TestContainers.Of(chosen, configuration is { Length: > 0 } c ? c : "Debug", settings)];
    }
}

/// <summary>One discovery or run: its containers, its update stream and what it reported so far.</summary>
internal sealed class TestJob
{
    private readonly TestService _service;
    private readonly IReadOnlyList<(TestContainer Container, TestContainerInfo Info)> _containers;
    private readonly Dictionary<string, IReadOnlyList<string>?>? _requested;
    private readonly CancellationTokenSource _cancel;
    private readonly Stopwatch _elapsed = Stopwatch.StartNew();
    private readonly Lock _lock = new();
    private readonly Channel<TestUpdateParams> _updates = Channel.CreateUnbounded<TestUpdateParams>(new UnboundedChannelOptions { SingleReader = true });
    private readonly TaskCompletionSource _done = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly Dictionary<string, Pending> _pending = new(StringComparer.Ordinal);
    private readonly List<TestStatusTest> _tests = [];
    private readonly Dictionary<(string Container, string Id), TestStatusResultItem> _results = [];
    private readonly List<(string Container, string Id)> _resultOrder = [];
    private readonly Dictionary<string, HashSet<string>> _known = new(StringComparer.Ordinal);
    private readonly Dictionary<int, TaskCompletionSource<(bool, string?)>> _attaches = [];
    private readonly Dictionary<string, int> _expected = new(StringComparer.Ordinal);
    private long _seq;

    public TestJob(TestService service, long id, string kind, long generation, bool debug, IReadOnlyList<(TestContainer, TestContainerInfo)> containers, Dictionary<string, IReadOnlyList<string>?>? requested, CancellationToken generationChanged)
    {
        _service = service;
        Id = id;
        Kind = kind;
        Generation = generation;
        Debug = debug;
        _containers = containers;
        _requested = requested;
        _cancel = CancellationTokenSource.CreateLinkedTokenSource(generationChanged);
    }

    public long Id { get; }

    public string Kind { get; }

    public long Generation { get; }

    public bool Debug { get; }

    /// <summary>Completes after the <c>finished</c> update was sent.</summary>
    public Task Completion => _done.Task;

    public bool Covers(string container) => _containers.Any(c => c.Info.Id == container);

    public void Start(Func<CancellationToken, Task<string>> body)
    {
        _ = Task.Run(SendLoopAsync);
        _ = Task.Run(FlushLoopAsync);
        _ = Task.Run(async () =>
        {
            string state;
            string? message = null;
            try
            {
                state = await body(_cancel.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                state = TestUpdateKinds.Canceled;
            }
            catch (Exception ex) when (ex is not OutOfMemoryException)
            {
                _service.Log.WriteLine($"[test] #{Id}: {ex}");
                state = TestUpdateKinds.Failed;
                message = ex.Message;
            }

            if (_cancel.IsCancellationRequested)
            {
                state = TestUpdateKinds.Canceled;
            }

            Finish(state, message);
        });
    }

    public void Cancel()
    {
        try
        {
            _cancel.Cancel();
        }
        catch (ObjectDisposedException)
        {
        }

        lock (_lock)
        {
            foreach (var (_, tcs) in _attaches)
            {
                tcs.TrySetResult((false, "the run was canceled"));
            }
        }
    }

    public bool Answer(int processId, bool attached, string? message)
    {
        lock (_lock)
        {
            return _attaches.Remove(processId, out var tcs) && tcs.TrySetResult((attached, message));
        }
    }

    public TestStatusRunning Status()
    {
        lock (_lock)
        {
            return new TestStatusRunning(
                Id,
                Kind,
                Generation,
                _containers.Select(c => c.Info).ToList(),
                _elapsed.Elapsed.TotalMilliseconds,
                _seq,
                [.. _tests],
                _resultOrder.Select(k => _results[k]).ToList())
            {
                Debug = Debug ? true : null,
            };
        }
    }

    /// <summary>A discovery: every container at once.</summary>
    public async Task<string> DiscoverAllAsync(CancellationToken cancellationToken)
    {
        var states = await Task.WhenAll(_containers.Select(c => DiscoverContainerAsync(c.Container, c.Info, silent: false, cancellationToken))).ConfigureAwait(false);
        return Overall(states.Select(s => s.State));
    }

    /// <summary>A run: the containers one after the other (side by side with <see cref="TestContainer.Parallel"/>).</summary>
    public async Task<string> RunAllAsync(CancellationToken cancellationToken)
    {
        var parallel = !Debug && _containers.Count > 1 && _containers[0].Container.Parallel;
        if (parallel)
        {
            var states = await Task.WhenAll(_containers.Select(c => RunContainerAsync(c.Container, c.Info, cancellationToken))).ConfigureAwait(false);
            return Overall(states);
        }

        var list = new List<string>();
        foreach (var (container, info) in _containers)
        {
            cancellationToken.ThrowIfCancellationRequested();
            list.Add(await RunContainerAsync(container, info, cancellationToken).ConfigureAwait(false));
        }

        return Overall(list);
    }

    private static string Overall(IEnumerable<string> states)
    {
        var list = states.ToList();
        return list.Contains(TestUpdateKinds.Canceled) ? TestUpdateKinds.Canceled
            : list.Contains(TestUpdateKinds.Failed) ? TestUpdateKinds.Failed
            : TestUpdateKinds.Completed;
    }

    private async Task<(string State, IReadOnlyList<DiscoveredTest> Tests)> DiscoverContainerAsync(TestContainer container, TestContainerInfo info, bool silent, CancellationToken cancellationToken)
    {
        var sink = new Sink(this, info.Id, silent);
        string state;
        string? message = null;
        if (info.Error is { } error)
        {
            state = TestUpdateKinds.Failed;
            message = error;
        }
        else
        {
            try
            {
                await _service.Runner(container.Protocol).DiscoverAsync(container, sink, cancellationToken).ConfigureAwait(false);
                state = TestUpdateKinds.Completed;
            }
            catch (OperationCanceledException)
            {
                state = TestUpdateKinds.Canceled;
            }
            catch (TestRunnerException ex)
            {
                state = TestUpdateKinds.Failed;
                message = ex.Message;
            }
        }

        var tests = sink.Found;
        if (state == TestUpdateKinds.Completed)
        {
            _service.Remember(Generation, info.Id, tests);
        }

        if (!silent)
        {
            ContainerFinished(info.Id, state, tests.Count, message);
        }
        else if (state != TestUpdateKinds.Completed)
        {
            Output(info.Id, $"Discovery before the run: {message ?? state}");
        }

        return (state, tests);
    }

    private async Task<string> RunContainerAsync(TestContainer container, TestContainerInfo info, CancellationToken cancellationToken)
    {
        if (info.Error is { } error)
        {
            ContainerFinished(info.Id, TestUpdateKinds.Failed, 0, error);
            return TestUpdateKinds.Failed;
        }

        var ids = _requested?.GetValueOrDefault(info.Id);
        IReadOnlyList<DiscoveredTest>? tests = null;
        var discovered = _service.Discovered(Generation, info.Id);
        if (container.Protocol == TestRunnerProtocol.MicrosoftTestingPlatform || ids is not null)
        {
            // MTP runs name their nodes, and ids need the discovered objects: discover first, silently, when this
            // generation has not.
            if (discovered is null)
            {
                var (state, found) = await DiscoverContainerAsync(container, info, silent: true, cancellationToken).ConfigureAwait(false);
                if (state != TestUpdateKinds.Completed)
                {
                    ContainerFinished(info.Id, state, 0, $"discovery failed: {state}");
                    return state;
                }

                discovered = found;
            }

            if (ids is null)
            {
                tests = discovered;
            }
            else
            {
                var set = new HashSet<string>(ids, StringComparer.Ordinal);
                tests = discovered.Where(t => set.Contains(t.Item.Id)).ToList();
                var unknown = ids.Where(i => discovered.All(t => t.Item.Id != i)).ToList();
                if (unknown.Count > 0)
                {
                    Output(info.Id, $"Not discovered, not run: {string.Join(", ", unknown)}");
                }
            }
        }

        lock (_lock)
        {
            _expected[info.Id] = tests?.Count ?? discovered?.Count ?? 0;
            _known[info.Id] = new HashSet<string>((discovered ?? []).Select(t => t.Item.Id), StringComparer.Ordinal);
        }

        if (tests is { Count: 0 })
        {
            ContainerFinished(info.Id, TestUpdateKinds.Completed, 0, null);
            return TestUpdateKinds.Completed;
        }

        var sink = new Sink(this, info.Id, silent: false);
        string result;
        string? message = null;
        try
        {
            await _service.Runner(container.Protocol).RunAsync(container, tests, Debug, sink, cancellationToken).ConfigureAwait(false);
            result = TestUpdateKinds.Completed;
        }
        catch (OperationCanceledException)
        {
            result = TestUpdateKinds.Canceled;
        }
        catch (TestRunnerException ex)
        {
            result = TestUpdateKinds.Failed;
            message = ex.Message;
        }

        int count;
        lock (_lock)
        {
            count = _resultOrder.Count(k => k.Container == info.Id);
        }

        ContainerFinished(info.Id, result, count, message);
        return result;
    }

    // ---- The update stream. Everything below that emits takes _lock: seq order is emission order, and what status
    // ---- answers is exactly what was emitted before nextSeq.

    private sealed class Pending
    {
        public List<TestItem> Tests { get; } = [];

        public List<TestResult> Results { get; } = [];

        public System.Text.StringBuilder Output { get; } = new();
    }

    private Pending PendingFor(string container)
    {
        if (!_pending.TryGetValue(container, out var p))
        {
            p = new Pending();
            _pending[container] = p;
        }

        return p;
    }

    internal void AddTests(string container, IReadOnlyList<DiscoveredTest> tests)
    {
        lock (_lock)
        {
            PendingFor(container).Tests.AddRange(tests.Select(t => t.Item));
        }
    }

    internal void AddResults(string container, IReadOnlyList<TestResult> results)
    {
        lock (_lock)
        {
            PendingFor(container).Results.AddRange(results);
        }
    }

    internal void Output(string container, string line)
    {
        lock (_lock)
        {
            PendingFor(container).Output.Append(line).Append('\n');
        }
    }

    internal void Launch(string container, TestLaunch launch)
    {
        lock (_lock)
        {
            FlushLocked();
            Emit(new TestUpdateParams(Id, Generation, 0, TestUpdateKinds.Launch)
            {
                Container = container,
                Launch = new TestLaunchInfo(launch.Program, launch.Args, launch.Cwd, launch.Env) { Runtime = Runtime(launch.Runtime) },
            });
        }
    }

    internal async Task<(bool Attached, string? Message)> AttachAsync(string container, int processId, CancellationToken cancellationToken)
    {
        var tcs = new TaskCompletionSource<(bool, string?)>(TaskCreationOptions.RunContinuationsAsynchronously);
        lock (_lock)
        {
            _attaches[processId] = tcs;
            FlushLocked();
            Emit(new TestUpdateParams(Id, Generation, 0, TestUpdateKinds.Attach) { Container = container, ProcessId = processId });
        }

        try
        {
            return await tcs.Task.WaitAsync(ITestRunner.DebugWait, cancellationToken).ConfigureAwait(false);
        }
        catch (TimeoutException)
        {
            lock (_lock)
            {
                _attaches.Remove(processId);
            }

            return (false, "the debugger did not attach within 60 s");
        }
    }

    private static string Runtime(TestRuntime runtime) => runtime switch
    {
        TestRuntime.Mono => "mono",
        TestRuntime.NetFx => "netfx",
        _ => "dotnet",
    };

    private void ContainerFinished(string container, string state, int count, string? message)
    {
        lock (_lock)
        {
            FlushLocked();
            Emit(new TestUpdateParams(Id, Generation, 0, TestUpdateKinds.ContainerFinished)
            {
                Container = container,
                State = state,
                Count = count,
                Message = message,
            });
        }
    }

    private void Finish(string state, string? message)
    {
        TestSummary summary;
        double elapsed;
        lock (_lock)
        {
            FlushLocked();
            summary = Summary();
            elapsed = _elapsed.Elapsed.TotalMilliseconds;
        }

        // The service forgets the job before its finished update goes out (status then answers it as `last`).
        _service.Finished(this, new TestStatusLast(Id, Kind, Generation, state, summary, elapsed));
        lock (_lock)
        {
            FlushLocked();
            Emit(new TestUpdateParams(Id, Generation, 0, TestUpdateKinds.Finished)
            {
                State = state,
                Summary = summary,
                ElapsedMs = elapsed,
                Message = message,
            });
            _updates.Writer.TryComplete();
        }

        _service.Log.WriteLine($"[test] #{Id} {Kind} {state}: {summary.Total} tests, {summary.Passed} passed, {summary.Failed} failed, {summary.Skipped} skipped ({elapsed:0} ms)");
    }

    private TestSummary Summary()
    {
        if (Kind == "discover")
        {
            return new TestSummary(_tests.Count, 0, 0, 0, 0);
        }

        int passed = 0, failed = 0, skipped = 0;
        foreach (var r in _results.Values)
        {
            switch (r.Outcome)
            {
                case TestOutcomes.Passed:
                    passed++;
                    break;
                case TestOutcomes.Failed:
                    failed++;
                    break;
                case TestOutcomes.Skipped:
                    skipped++;
                    break;
            }
        }

        var total = Math.Max(_expected.Values.Sum(), _results.Count);
        return new TestSummary(total, passed, failed, skipped, Math.Max(0, total - passed - failed - skipped));
    }

    private void FlushLocked()
    {
        foreach (var (container, p) in _pending)
        {
            if (p.Tests.Count > 0)
            {
                var tests = p.Tests.ToList();
                p.Tests.Clear();
                _tests.AddRange(tests.Select(t => new TestStatusTest(container, t)));
                Emit(new TestUpdateParams(Id, Generation, 0, TestUpdateKinds.Discovered) { Container = container, Tests = tests });
            }

            if (p.Results.Count > 0)
            {
                var known = _known.GetValueOrDefault(container);
                var results = p.Results
                    .Select(r => known is not null && known.Contains(r.Id) ? r with { DisplayName = null, FullyQualifiedName = null } : r)
                    .ToList();
                p.Results.Clear();
                foreach (var r in results)
                {
                    var key = (container, r.Id);
                    if (!_results.ContainsKey(key))
                    {
                        _resultOrder.Add(key);
                    }

                    _results[key] = new TestStatusResultItem(r.Id, container, r.Outcome)
                    {
                        DurationMs = r.DurationMs,
                        Message = r.Message,
                        StackTrace = r.StackTrace,
                        Output = r.Output,
                        DisplayName = r.DisplayName,
                        FullyQualifiedName = r.FullyQualifiedName,
                    };
                }

                Emit(new TestUpdateParams(Id, Generation, 0, TestUpdateKinds.Results) { Container = container, Results = results });
            }

            if (p.Output.Length > 0)
            {
                var text = p.Output.ToString();
                p.Output.Clear();
                Emit(new TestUpdateParams(Id, Generation, 0, TestUpdateKinds.Output) { Container = container, Text = text });
            }
        }
    }

    /// <summary>Under <see cref="_lock"/>: numbers the update and queues it.</summary>
    private void Emit(TestUpdateParams update) => _updates.Writer.TryWrite(update with { Seq = _seq++ });

    private async Task FlushLoopAsync()
    {
        while (!_updates.Reader.Completion.IsCompleted)
        {
            await Task.Delay(TestService.FlushInterval).ConfigureAwait(false);
            lock (_lock)
            {
                if (!_updates.Reader.Completion.IsCompleted)
                {
                    FlushLocked();
                }
            }
        }
    }

    private async Task SendLoopAsync()
    {
        try
        {
            await foreach (var update in _updates.Reader.ReadAllAsync().ConfigureAwait(false))
            {
                await _service.NotifyAsync(update).ConfigureAwait(false);
            }
        }
        finally
        {
            _cancel.Dispose();
            _done.TrySetResult();
        }
    }

    /// <summary>A container's sink: everything it reports goes into the job's buffers.</summary>
    private sealed class Sink(TestJob job, string container, bool silent) : ITestSink
    {
        private readonly List<DiscoveredTest> _found = [];

        public IReadOnlyList<DiscoveredTest> Found
        {
            get
            {
                lock (_found)
                {
                    return [.. _found];
                }
            }
        }

        public void Tests(IReadOnlyList<DiscoveredTest> tests)
        {
            lock (_found)
            {
                _found.AddRange(tests);
            }

            if (!silent)
            {
                job.AddTests(container, tests);
            }
        }

        public void Results(IReadOnlyList<TestResult> results) => job.AddResults(container, results);

        public void Log(string line) => job.Output(container, line);

        public void Launch(TestLaunch launch) => job.Launch(container, launch);

        public Task<(bool Attached, string? Message)> AttachAsync(int processId, CancellationToken cancellationToken) =>
            job.AttachAsync(container, processId, cancellationToken);
    }
}
