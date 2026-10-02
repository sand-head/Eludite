using System.Diagnostics;
using System.Globalization;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using StreamJsonRpc;

namespace Eludite.Host.Build;

/// <summary>
/// <c>eludite/build/start</c> and <c>eludite/build/cancel</c> (brief 0017; host-rpc.md, "Build"): one build at a time,
/// run out of process, its console output streamed as <c>eludite/build/output</c>, progress as
/// <c>eludite/build/progress</c>, and the result, read from the binary log, as <c>eludite/build/finished</c>.
/// </summary>
public sealed class BuildService : IAsyncDisposable
{
    /// <summary>Error code of a start while a build runs.</summary>
    public const int BuildInProgress = -32010;

    /// <summary>Diagnostics in one finished notification at most.</summary>
    public const int MaxDiagnostics = 5000;

    /// <summary>Binary logs kept in <see cref="LogDirectory"/>.</summary>
    private const int KeptLogs = 10;

    private readonly Func<(long Generation, string? Path, CancellationToken GenerationChanged)> _currentSolution;
    private readonly TextWriter _log;
    private readonly Lazy<BuildToolchains> _toolchains;
    private readonly Lock _lock = new();
    private JsonRpc? _shell;
    private BuildRun? _current;
    private long _nextId;

    /// <param name="currentSolution">The generation, the open solution (or null) and a token canceled when the generation moves on.</param>
    /// <param name="toolchains">Where the MSBuilds are; located on the first build when null.</param>
    /// <param name="logDirectory">Where binary logs go; default <c>&lt;temp&gt;/eludite-host/builds</c>.</param>
    public BuildService(Func<(long Generation, string? Path, CancellationToken GenerationChanged)> currentSolution, TextWriter log, Func<BuildToolchains>? toolchains = null, string? logDirectory = null)
    {
        _currentSolution = currentSolution;
        _log = log;
        _toolchains = new Lazy<BuildToolchains>(toolchains ?? BuildToolchains.Locate);
        LogDirectory = logDirectory ?? Path.Combine(Path.GetTempPath(), "eludite-host", "builds");
    }

    /// <summary>Where binary logs are written.</summary>
    public string LogDirectory { get; }

    /// <summary>The running build's completion (it has sent <c>eludite/build/finished</c>), or a completed task.</summary>
    public Task Running
    {
        get
        {
            lock (_lock)
            {
                return _current?.Completion ?? Task.CompletedTask;
            }
        }
    }

    /// <summary>Sends notifications on the shell connection. Call before it starts listening.</summary>
    public void Attach(JsonRpc shell) => _shell = shell;

    /// <summary><c>eludite/build/start</c>: validates, starts the build in the background and returns at once.</summary>
    public BuildStartResult Start(BuildStartParams? parameters)
    {
        if (parameters is null || !BuildTargets.IsValid(parameters.Target))
        {
            throw HostErrors.BadParams("target must be build, rebuild or clean");
        }

        var (generation, solution, generationChanged) = _currentSolution();
        if (solution is null)
        {
            throw HostErrors.BadParams("no solution is open");
        }

        var solutionProjects = ReadProjects(solution);
        string path;
        IReadOnlyList<string> projects;
        if (parameters.Project is { Length: > 0 } project)
        {
            var full = Path.GetFullPath(project);
            if (!solutionProjects.Any(p => PathEquals(p, full)))
            {
                throw HostErrors.BadParams($"{project} is not a project of the open solution");
            }

            path = full;
            projects = [full];
        }
        else
        {
            path = solution;
            projects = solutionProjects;
        }

        var configuration = parameters.Configuration is { Length: > 0 } c ? c : "Debug";
        var platform = parameters.Platform is { Length: > 0 } pl ? pl : null;
        var hasLegacy = projects.Any(SolutionProjects.IsLegacy);
        lock (_lock)
        {
            if (_current is { } running)
            {
                throw new LocalRpcException($"build {running.Id} is already running")
                {
                    ErrorCode = BuildInProgress,
                    ErrorData = new { buildId = running.Id },
                };
            }

            var id = ++_nextId;
            var binlog = BinlogPath(id);
            var plan = BuildPlan.Create(parameters.Target!, path, configuration, platform, binlog, hasLegacy, _toolchains.Value);
            var run = new BuildRun(this, id, generation, parameters.Target!, path, configuration, platform, projects.Count, plan, binlog, generationChanged);
            _current = run;
            run.Start();
            _log.WriteLine($"[build] #{id} {parameters.Target} {path}: {plan.CommandLine}");
            return new BuildStartResult(id, generation, path, parameters.Target!, configuration, plan.Toolchain, plan.CommandLine)
            {
                Platform = platform,
                Binlog = binlog,
            };
        }
    }

    /// <summary><c>eludite/build/cancel</c>: cancels the running build (or the one named), if any.</summary>
    public BuildCancelResult Cancel(BuildCancelParams? parameters)
    {
        BuildRun? run;
        lock (_lock)
        {
            run = _current;
        }

        if (run is null || (parameters?.BuildId is { } wanted && wanted != run.Id))
        {
            return new BuildCancelResult(false);
        }

        run.Cancel("Build canceled.");
        return new BuildCancelResult(true) { BuildId = run.Id };
    }

    public async ValueTask DisposeAsync()
    {
        BuildRun? run;
        lock (_lock)
        {
            run = _current;
        }

        if (run is not null)
        {
            run.Cancel("eludite-host is exiting.");
            await run.Completion.WaitAsync(TimeSpan.FromSeconds(5)).ConfigureAwait(false);
        }
    }

    internal TextWriter Log => _log;

    internal void Finished(BuildRun run)
    {
        lock (_lock)
        {
            if (ReferenceEquals(_current, run))
            {
                _current = null;
            }
        }
    }

    internal async Task NotifyAsync(string method, object parameters)
    {
        if (_shell is not { } shell)
        {
            return;
        }

        try
        {
            await shell.NotifyWithParameterObjectAsync(method, parameters).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is ObjectDisposedException or ConnectionLostException or IOException)
        {
            // The shell went away.
        }
    }

    private string? BinlogPath(long id)
    {
        try
        {
            Directory.CreateDirectory(LogDirectory);
            foreach (var old in new DirectoryInfo(LogDirectory).GetFiles("*.binlog").OrderByDescending(f => f.LastWriteTimeUtc).Skip(KeptLogs))
            {
                old.Delete();
            }

            return Path.Combine(LogDirectory, string.Create(CultureInfo.InvariantCulture, $"build-{Environment.ProcessId}-{id}.binlog"));
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            _log.WriteLine($"[build] no binary log: {ex.Message}");
            return null;
        }
    }

    private static IReadOnlyList<string> ReadProjects(string solution)
    {
        try
        {
            return SolutionProjects.Read(solution);
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            return [];
        }
    }

    internal static bool PathEquals(string a, string b) =>
        string.Equals(Path.GetFullPath(a), Path.GetFullPath(b), OperatingSystem.IsWindows() || OperatingSystem.IsMacOS() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal);
}

/// <summary>One build: the MSBuild process, its output, its progress and its result.</summary>
internal sealed class BuildRun
{
    private static readonly TimeSpan ProgressInterval = TimeSpan.FromMilliseconds(100);

    private readonly BuildService _service;
    private readonly string _target;
    private readonly string _path;
    private readonly string _configuration;
    private readonly string? _platform;
    private readonly int _projectsTotal;
    private readonly BuildPlan _plan;
    private readonly string? _binlog;
    private readonly CancellationTokenSource _cancel = new();
    private readonly CancellationTokenRegistration _generationRegistration;
    private readonly TaskCompletionSource _done = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly Stopwatch _elapsed = Stopwatch.StartNew();
    private readonly Lock _lock = new();
    private readonly HashSet<string> _projectsCompleted = new(StringComparer.Ordinal);
    private readonly HashSet<string> _seenDiagnostics = new(StringComparer.Ordinal);
    private readonly List<BuildDiagnostic> _consoleDiagnostics = [];
    private string? _cancelMessage;
    private string? _currentProject;
    private int _errors;
    private int _warnings;
    private int _progressVersion;
    private bool _afterTaskFailure;
    private bool _stackNoted;

    public BuildRun(BuildService service, long id, long generation, string target, string path, string configuration, string? platform, int projectsTotal, BuildPlan plan, string? binlog, CancellationToken generationChanged)
    {
        _service = service;
        Id = id;
        Generation = generation;
        _target = target;
        _path = path;
        _configuration = configuration;
        _platform = platform;
        _projectsTotal = Math.Max(projectsTotal, 1);
        _plan = plan;
        _binlog = binlog;
        _generationRegistration = generationChanged.Register(() => Cancel("Build canceled: the solution changed."));
    }

    public long Id { get; }

    public long Generation { get; }

    public Task Completion => _done.Task;

    public void Start() => _ = Task.Run(RunAsync);

    public void Cancel(string message)
    {
        lock (_lock)
        {
            _cancelMessage ??= message;
        }

        try
        {
            _cancel.Cancel();
        }
        catch (ObjectDisposedException)
        {
        }
    }

    private bool Canceled => _cancel.IsCancellationRequested;

    private async Task RunAsync()
    {
        var pipe = new OutputPipe((seq, text) => _service.NotifyAsync("eludite/build/output", new BuildOutputParams(Id, seq, text)));
        int? exitCode = null;
        string? failure = null;
        try
        {
            // The first line goes out before MSBuild starts, so the shell shows output at once.
            var now = DateTime.Now.ToString("T", CultureInfo.CurrentCulture);
            var what = _target switch
            {
                BuildTargets.Rebuild => "Rebuild All",
                BuildTargets.Clean => "Clean",
                _ => "Build",
            };
            await pipe.WriteLineAsync($"{what} started at {now}...").ConfigureAwait(false);
            await pipe.WriteLineAsync($"------ {what} started: {Path.GetFileName(_path)}, Configuration: {_configuration}{(_platform is null ? string.Empty : " " + _platform)} ------").ConfigureAwait(false);
            await pipe.WriteLineAsync("> " + _plan.CommandLine).ConfigureAwait(false);
            (exitCode, failure) = await RunProcessAsync(pipe).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            failure = $"The build could not run: {ex.Message}";
            _service.Log.WriteLine($"[build] #{Id}: {ex}");
        }

        try
        {
            await FinishAsync(pipe, exitCode, failure).ConfigureAwait(false);
        }
        finally
        {
            await _generationRegistration.DisposeAsync().ConfigureAwait(false);
            _cancel.Dispose();
            _done.TrySetResult();
        }
    }

    private async Task<(int? ExitCode, string? Failure)> RunProcessAsync(OutputPipe pipe)
    {
        var psi = new ProcessStartInfo(_plan.FileName)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            WorkingDirectory = Path.GetDirectoryName(_path) ?? Environment.CurrentDirectory,
            StandardOutputEncoding = System.Text.Encoding.UTF8,
            StandardErrorEncoding = System.Text.Encoding.UTF8,
        };
        foreach (var name in DesignTimeProperties.LocatorVariables)
        {
            psi.Environment.Remove(name);
        }

        foreach (var (k, v) in _plan.Environment)
        {
            psi.Environment[k] = v;
        }

        foreach (var a in _plan.Arguments)
        {
            psi.ArgumentList.Add(a);
        }

        if (Canceled)
        {
            return (null, null);
        }

        using var process = new Process { StartInfo = psi };
        try
        {
            process.Start();
        }
        catch (System.ComponentModel.Win32Exception ex)
        {
            var message = $"Could not start {_plan.FileName}: {ex.Message}";
            await pipe.WriteLineAsync(message).ConfigureAwait(false);
            return (null, message);
        }

        process.StandardInput.Close();
        var stdout = PumpAsync(process.StandardOutput, pipe);
        var stderr = PumpAsync(process.StandardError, pipe);
        using var progressStop = new CancellationTokenSource();
        var progress = ProgressLoopAsync(progressStop.Token);
        try
        {
            await process.WaitForExitAsync(_cancel.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            var sw = Stopwatch.StartNew();
            ProcessTree.Kill(process, _service.Log);
            using var wait = new CancellationTokenSource(TimeSpan.FromMilliseconds(1000));
            try
            {
                await process.WaitForExitAsync(wait.Token).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                _service.Log.WriteLine($"[build] #{Id}: MSBuild did not exit within 1 s of the kill");
            }

            _service.Log.WriteLine($"[build] #{Id}: killed in {sw.ElapsedMilliseconds} ms");
        }

        // A grandchild that kept the pipes open (a build server) must not hold the build up.
        var readers = Task.WhenAll(stdout, stderr);
        if (await Task.WhenAny(readers, Task.Delay(Canceled ? TimeSpan.FromMilliseconds(300) : TimeSpan.FromSeconds(5))).ConfigureAwait(false) != readers)
        {
            _service.Log.WriteLine($"[build] #{Id}: output still open after MSBuild exited; not waiting for it");
        }

        await progressStop.CancelAsync().ConfigureAwait(false);
        await progress.ConfigureAwait(false);
        BuildProgressParams last;
        lock (_lock)
        {
            last = Progress();
        }

        // The final counts, also when the build ended within one progress interval of its last change.
        await _service.NotifyAsync("eludite/build/progress", last).ConfigureAwait(false);
        return (process.HasExited && !Canceled ? process.ExitCode : null, null);
    }

    private async Task PumpAsync(StreamReader reader, OutputPipe pipe)
    {
        try
        {
            while (await reader.ReadLineAsync().ConfigureAwait(false) is { } line)
            {
                if (Observe(line) is { } shown)
                {
                    await pipe.WriteLineAsync(shown).ConfigureAwait(false);
                }
            }
        }
        catch (Exception ex) when (ex is IOException or ObjectDisposedException or InvalidOperationException)
        {
        }
    }

    /// <summary>Counts progress from one console line; returns the line to show, or null to drop it.</summary>
    private string? Observe(string line)
    {
        lock (_lock)
        {
            if (ConsoleLines.IsStackFrame(line) && _afterTaskFailure)
            {
                if (_stackNoted)
                {
                    return null;
                }

                _stackNoted = true;
                return "    (stack trace omitted; it is in the binary log)";
            }

            if (ConsoleLines.ParseProjectOutput(line) is { } project)
            {
                _projectsCompleted.Add(project);
                _currentProject = project;
                _progressVersion++;
                _afterTaskFailure = false;
            }
            else if (ConsoleLines.ParseDiagnostic(line) is { } d)
            {
                var key = Key(d);
                if (_seenDiagnostics.Add(key))
                {
                    _consoleDiagnostics.Add(d);
                    if (d.Severity == "error")
                    {
                        _errors++;
                    }
                    else
                    {
                        _warnings++;
                    }

                    _progressVersion++;
                }

                _afterTaskFailure = d.Code is "MSB4018" or "MSB4061" or "MSB4062";
                _stackNoted = false;
            }

            return line;
        }
    }

    private async Task ProgressLoopAsync(CancellationToken stop)
    {
        var sent = -1;
        while (!stop.IsCancellationRequested)
        {
            try
            {
                await Task.Delay(ProgressInterval, stop).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                break;
            }

            BuildProgressParams? p = null;
            lock (_lock)
            {
                if (_progressVersion != sent)
                {
                    sent = _progressVersion;
                    p = Progress();
                }
            }

            if (p is not null)
            {
                await _service.NotifyAsync("eludite/build/progress", p).ConfigureAwait(false);
            }
        }
    }

    private BuildProgressParams Progress() =>
        new(Id, _elapsed.Elapsed.TotalMilliseconds, _projectsTotal, Math.Min(_projectsCompleted.Count, _projectsTotal), _errors, _warnings)
        {
            CurrentProject = _currentProject,
        };

    private async Task FinishAsync(OutputPipe pipe, int? exitCode, string? failure)
    {
        var canceled = Canceled;
        IReadOnlyList<BinlogDiagnostic> raw;
        IReadOnlyList<BinlogProject> logProjects = [];
        var usedLog = false;
        if (!canceled && _binlog is not null && File.Exists(_binlog))
        {
            try
            {
                var read = Stopwatch.StartNew();
                var log = BinlogReader.Read(_binlog, CancellationToken.None);
                raw = log.Diagnostics;
                logProjects = log.Projects;
                usedLog = true;
                _service.Log.WriteLine($"[build] #{Id}: binary log read in {read.ElapsedMilliseconds} ms ({log.Diagnostics.Count} diagnostics, {log.Projects.Count} projects)");
            }
            catch (Exception ex) when (ex is not OutOfMemoryException)
            {
                _service.Log.WriteLine($"[build] #{Id}: binary log not read ({ex.GetType().Name}: {ex.Message}); using the console output");
                raw = ConsoleFallback();
            }
        }
        else
        {
            raw = ConsoleFallback();
        }

        var targets = new Dictionary<BuildDiagnostic, string?>(ReferenceEqualityComparer.Instance);
        var deduped = new List<BuildDiagnostic>();
        var seen = new HashSet<string>(StringComparer.Ordinal);
        foreach (var r in raw)
        {
            if (seen.Add(Key(r.Diagnostic)))
            {
                deduped.Add(r.Diagnostic);
                targets[r.Diagnostic] = r.Target;
            }
        }

        var (classified, replacements) = WindowsOnlyTargets.Replace(deduped, d => targets.GetValueOrDefault(d));
        var all = new List<BuildDiagnostic>(_plan.Notes);
        all.AddRange(classified.Where(d => d.Severity == "error"));
        all.AddRange(classified.Where(d => d.Severity != "error"));
        var truncated = all.Count > BuildService.MaxDiagnostics;
        if (truncated)
        {
            all.RemoveRange(BuildService.MaxDiagnostics, all.Count - BuildService.MaxDiagnostics);
        }

        foreach (var note in _plan.Notes.Concat(replacements))
        {
            await pipe.WriteLineAsync($"{note.File ?? _path} : {note.Severity} {note.Code}: {note.Message}").ConfigureAwait(false);
        }

        var result = canceled ? BuildResults.Canceled : exitCode == 0 ? BuildResults.Succeeded : BuildResults.Failed;
        var projects = logProjects
            .Where(p => BinlogReader.IsProject(p.Path))
            .Select(p =>
            {
                var errors = all.Count(d => d.Severity == "error" && d.Project is { } dp && BuildService.PathEquals(dp, p.Path));
                var warnings = all.Count(d => d.Severity == "warning" && d.Project is { } dp && BuildService.PathEquals(dp, p.Path));
                return new BuildProjectResult(Path.GetFileNameWithoutExtension(p.Path), p.Path, p.Succeeded && errors == 0 ? BuildResults.Succeeded : BuildResults.Failed, errors, warnings)
                {
                    ElapsedMs = Math.Max(0, (p.Finished - p.Started).TotalMilliseconds),
                };
            })
            .OrderBy(p => p.Path, StringComparer.Ordinal)
            .ToList();
        if (!usedLog)
        {
            projects = ConsoleProjects(result, all);
        }

        var summary = new BuildSummary(
            projects.Count(p => p.Result == BuildResults.Succeeded),
            projects.Count(p => p.Result == BuildResults.Failed),
            all.Count(d => d.Severity == "error"),
            all.Count(d => d.Severity == "warning"));
        string? message;
        lock (_lock)
        {
            message = canceled ? _cancelMessage : failure;
        }

        var elapsed = _elapsed.Elapsed;
        if (canceled)
        {
            await pipe.WriteLineAsync(message ?? "Build canceled.").ConfigureAwait(false);
        }

        var verb = _target switch
        {
            BuildTargets.Rebuild => "Rebuild All",
            BuildTargets.Clean => "Clean",
            _ => "Build",
        };
        await pipe.WriteLineAsync(canceled
            ? $"========== {verb}: canceled =========="
            : $"========== {verb}: {summary.ProjectsSucceeded} succeeded, {summary.ProjectsFailed} failed =========="
            ).ConfigureAwait(false);
        await pipe.WriteLineAsync(string.Create(CultureInfo.InvariantCulture, $"========== {verb} {(canceled ? "canceled" : "completed")} at {DateTime.Now.ToString("T", CultureInfo.CurrentCulture)} and took {elapsed.TotalSeconds:00.000} seconds ==========")).ConfigureAwait(false);
        await pipe.CompleteAsync().ConfigureAwait(false);

        var finished = new BuildFinishedParams(Id, Generation, _target, _path, result, elapsed.TotalMilliseconds, summary, projects, all)
        {
            ExitCode = exitCode,
            DiagnosticsTruncated = truncated,
            Binlog = usedLog ? _binlog : null,
            Message = message,
        };
        _service.Log.WriteLine($"[build] #{Id} {result} in {elapsed.TotalMilliseconds:0} ms: {summary.Errors} errors, {summary.Warnings} warnings");
        // Free the service first: a client that starts the next build on this notification is not refused.
        _service.Finished(this);
        await _service.NotifyAsync("eludite/build/finished", finished).ConfigureAwait(false);
    }

    private List<BinlogDiagnostic> ConsoleFallback()
    {
        lock (_lock)
        {
            return [.. _consoleDiagnostics.Select(d => new BinlogDiagnostic(d, null))];
        }
    }

    /// <summary>Without a log, projects are those whose output line appeared, plus those with errors.</summary>
    private List<BuildProjectResult> ConsoleProjects(string result, IReadOnlyList<BuildDiagnostic> diagnostics)
    {
        var byPath = diagnostics.Where(d => d.Project is not null).GroupBy(d => d.Project!, StringComparer.Ordinal);
        var list = byPath.Select(g => new BuildProjectResult(
            Path.GetFileNameWithoutExtension(g.Key),
            g.Key,
            result == BuildResults.Canceled ? BuildResults.Canceled : g.Any(d => d.Severity == "error") ? BuildResults.Failed : BuildResults.Succeeded,
            g.Count(d => d.Severity == "error"),
            g.Count(d => d.Severity == "warning"))).ToList();
        lock (_lock)
        {
            foreach (var name in _projectsCompleted.Where(n => !list.Any(p => p.Name == n)))
            {
                list.Add(new BuildProjectResult(name, name, BuildResults.Succeeded, 0, 0));
            }
        }

        return list;
    }

    private static string Key(BuildDiagnostic d) =>
        string.Join('\u001f', d.Severity, d.Code, d.Message, d.File ?? string.Empty, d.Line ?? 0, d.Column ?? 0, d.Project ?? string.Empty);
}
