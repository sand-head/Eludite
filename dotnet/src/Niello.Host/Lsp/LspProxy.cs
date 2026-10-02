using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Nodes;
using Niello.Host.Legacy;
using Niello.Host.Rpc;
using StreamJsonRpc;

namespace Niello.Host.Lsp;

/// <summary>
/// The LSP bridge between the shell connection and an upstream language server (the Roslyn language server,
/// spawned as a child process). Contract: protocol/schemas/host-rpc.md.
/// </summary>
/// <remarks>
/// <para>
/// Lifecycle: <see cref="Start"/> (on <c>niello/host/initialize</c>) launches the server in the background and performs
/// the LSP handshake with it. <see cref="OpenSolution"/> and <see cref="CloseSolution"/> implement
/// <c>niello/solution/open</c> and <c>close</c>; a second open (or a close) restarts the server, because Roslyn cannot
/// unload a solution, and replays the shell's open documents into it.
/// </para>
/// <para>
/// Ordering: document notifications, restarts and the warming pulls go through one ordered chain
/// (<see cref="_tail"/>), and forwarded requests wait for the chain as it was when they arrived, so a completion
/// after a <c>didChange</c> always sees the new text.
/// </para>
/// <para>
/// Generations: every forwarded request must carry <c>params.nielloGeneration</c>; the host strips it. A missing value
/// is -32602, a stale one -32801 and is never forwarded, and a generation change while the request is in flight
/// cancels it upstream and answers -32801.
/// </para>
/// <para>
/// Cancellation: <c>$/cancelRequest</c> from the shell cancels the handler's token; the handler stops waiting at once
/// (the shell gets -32800, never a result) and StreamJsonRpc sends <c>$/cancelRequest</c> upstream.
/// </para>
/// <para>
/// Warming: <see cref="DiagnosticsWarmer"/> pulls <c>textDocument/diagnostic</c> for open documents so Roslyn's
/// frozen-partial completion has full semantics to use (brief 0002); results are published to the shell as
/// <c>textDocument/publishDiagnostics</c>.
/// </para>
/// </remarks>
public sealed class LspProxy : IAsyncDisposable
{
    /// <summary>LSP error code for a result invalidated by a state change.</summary>
    public const int ContentModified = HostErrors.ContentModified;

    /// <summary>Member every forwarded request carries in its params.</summary>
    public const string GenerationProperty = "nielloGeneration";

    /// <summary>Forwarded requests typed in niello-protocol and validated before forwarding.</summary>
    public static IReadOnlyList<string> TypedRequests { get; } =
    [
        "textDocument/completion",
        "completionItem/resolve",
        "textDocument/hover",
        "textDocument/definition",
        "textDocument/references",
        "textDocument/documentSymbol",
        "workspace/symbol",
        "textDocument/diagnostic",
    ];

    /// <summary>Forwarded requests passed through untyped (still generation-checked).</summary>
    public static IReadOnlyList<string> UntypedRequests { get; } =
    [
        "textDocument/signatureHelp",
        "textDocument/typeDefinition",
        "textDocument/implementation",
        "textDocument/documentHighlight",
        "textDocument/semanticTokens/full",
        "textDocument/semanticTokens/range",
        "textDocument/codeAction",
        "textDocument/formatting",
        "textDocument/rename",
    ];

    /// <summary>All forwarded requests.</summary>
    public static IReadOnlyList<string> ForwardedRequests { get; } = [.. TypedRequests, .. UntypedRequests];

    /// <summary>Forwarded notifications passed through untyped.</summary>
    public static IReadOnlyList<string> UntypedNotifications { get; } =
    [
        "textDocument/didSave",
        "workspace/didChangeWatchedFiles",
    ];

    /// <summary>Language server notifications relayed to the shell unchanged.</summary>
    public static IReadOnlyList<string> RelayedServerNotifications { get; } =
    [
        "window/showMessage",
        "$/progress",
    ];

    internal const string ProjectInitializationComplete = "workspace/projectInitializationComplete";

    private static readonly JsonElement JsonNull = JsonDocument.Parse("null").RootElement.Clone();

    private static readonly string[] SupportedExtensions = [".sln", ".slnx", ".csproj", ".vbproj"];

    private readonly ILanguageServerLauncher? _launcher;
    private readonly TextWriter _log;
    private readonly ISolutionPreparer? _preparer;
    private readonly TaskCompletionSource _projectsLoaded = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly Lock _lock = new();
    private readonly OpenDocuments _documents = new();
    private readonly DiagnosticsWarmer _warmer;

    // Guarded by _lock.
    private CancellationTokenSource _generationCts = new();
    private long _generation;
    private bool _started;
    private bool _solutionEverOpened;
    private bool _disposing;
    private SolutionLoad? _load;
    private Task<UpstreamSession?> _session = Task.FromResult<UpstreamSession?>(null);
    private Task _tail = Task.CompletedTask;

    private JsonRpc? _shell;
    private long _pullsCompleted;

    /// <param name="launcher">Starts the language server; null when none is configured (requests then fail with -32803).</param>
    /// <param name="preparer">Runs before the server opens a solution (brief 0003 legacy preparation).</param>
    /// <param name="debounce">Warming debounce after a change; default <see cref="DiagnosticsWarmer.DefaultDebounce"/>.</param>
    public LspProxy(ILanguageServerLauncher? launcher, TextWriter log, ISolutionPreparer? preparer = null, TimeSpan? debounce = null, TimeProvider? timeProvider = null)
    {
        _launcher = launcher;
        _log = log;
        _preparer = preparer;
        _warmer = new DiagnosticsWarmer(PullDiagnosticsAsync, log, debounce ?? DiagnosticsWarmer.DebounceFromEnvironment(), timeProvider);
    }

    /// <summary>True when a language server is configured.</summary>
    public bool IsConfigured => _launcher is not null;

    /// <summary>The current solution generation.</summary>
    public long Generation
    {
        get
        {
            lock (_lock)
            {
                return _generation;
            }
        }
    }

    /// <summary>Completes when the upstream server first reports that projects are loaded.</summary>
    public Task ProjectsLoaded => _projectsLoaded.Task;

    /// <summary>The warming scheduler (exposed for tests and diagnostics).</summary>
    public DiagnosticsWarmer Warmer => _warmer;

    /// <summary>Warming pulls that returned a report.</summary>
    public long WarmingPullsCompleted => Interlocked.Read(ref _pullsCompleted);

    /// <summary>Registers the forwarded methods on the shell connection. Call before it starts listening.</summary>
    public void Attach(JsonRpc shell)
    {
        ArgumentNullException.ThrowIfNull(shell);
        _shell = shell;
        foreach (var method in ForwardedRequests)
        {
            var handler = new Func<JsonElement, CancellationToken, Task<JsonElement>>((p, ct) => ForwardRequestAsync(method, p, ct));
            AddMethod(shell, method, handler);
        }

        AddMethod(shell, "textDocument/didOpen", new Action<JsonElement>(DidOpen));
        AddMethod(shell, "textDocument/didChange", new Action<JsonElement>(DidChange));
        AddMethod(shell, "textDocument/didClose", new Action<JsonElement>(DidClose));
        foreach (var method in UntypedNotifications)
        {
            AddMethod(shell, method, new Action<JsonElement>(p => ForwardNotification(method, p)));
        }
    }

    /// <summary>Starts the language server in the background (called on <c>niello/host/initialize</c>).</summary>
    public void Start()
    {
        lock (_lock)
        {
            if (_started)
            {
                return;
            }

            _started = true;
            if (_launcher is not null)
            {
                _session = LaunchSessionAsync(null, []);
                _tail = _session;
                return;
            }
        }

        _ = NotifyShellAsync("niello/languageServer/status", new LanguageServerStatus("unavailable")
        {
            Message = "No language server is configured (build it with tools/roslyn-pin/build.sh, or pass --roslyn-ls).",
        });
    }

    /// <summary><c>niello/solution/open</c>: bumps the generation, returns it, and loads in the background.</summary>
    public long OpenSolution(string? path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            throw HostErrors.BadParams("path is required");
        }

        var full = Path.GetFullPath(path);
        if (!SupportedExtensions.Contains(Path.GetExtension(full), StringComparer.OrdinalIgnoreCase))
        {
            throw HostErrors.BadParams($"{full}: expected a .sln, .slnx, .csproj or .vbproj file");
        }

        if (!File.Exists(full))
        {
            throw HostErrors.BadParams($"{full} does not exist");
        }

        SolutionLoad load;
        bool restart;
        CancellationTokenSource stale;
        lock (_lock)
        {
            if (!_started)
            {
                throw HostErrors.NotInitialized();
            }

            stale = BumpGenerationLocked();
            restart = _solutionEverOpened && _launcher is not null;
            _solutionEverOpened = true;
            load = new SolutionLoad(_generation, full);
            _load = load;
            if (restart)
            {
                EnqueueRestartLocked(full);
            }
        }

        CancelStale(stale);
        _ = Task.Run(() => LoadAsync(load));
        return load.Generation;
    }

    /// <summary><c>niello/solution/close</c>: bumps the generation when a solution is open and returns the current one.</summary>
    public long CloseSolution()
    {
        SolutionLoad? closed;
        CancellationTokenSource? stale = null;
        long generation;
        lock (_lock)
        {
            if (!_started)
            {
                throw HostErrors.NotInitialized();
            }

            closed = _load;
            if (closed is not null)
            {
                stale = BumpGenerationLocked();
                _load = null;
                if (_launcher is not null)
                {
                    EnqueueRestartLocked(null);
                }
            }

            generation = _generation;
        }

        if (stale is not null)
        {
            CancelStale(stale);
            _ = NotifyShellAsync("niello/solution/status", new SolutionStatus(generation, closed!.Path, SolutionStates.Closed));
        }

        return generation;
    }

    /// <summary>Advances the solution generation and cancels every in-flight request pinned to the old one.</summary>
    public long BumpGeneration()
    {
        CancellationTokenSource stale;
        long next;
        lock (_lock)
        {
            stale = BumpGenerationLocked();
            next = _generation;
        }

        CancelStale(stale);
        return next;
    }

    private CancellationTokenSource BumpGenerationLocked()
    {
        var old = _generationCts;
        _generationCts = new CancellationTokenSource();
        _generation++;
        return old;
    }

    private static void CancelStale(CancellationTokenSource stale)
    {
        // Outside the lock: cancellation runs continuations synchronously.
        stale.Cancel();
        stale.Dispose();
    }

    // ------------------------------------------------------------------ shell -> host -> server

    private async Task<JsonElement> ForwardRequestAsync(string method, JsonElement parameters, CancellationToken cancellationToken)
    {
        var (forwarded, pinned) = ExtractGeneration(parameters);
        if (pinned is not { } requested)
        {
            throw HostErrors.BadParams($"{method}: params.{GenerationProperty} (a non-negative integer) is required");
        }

        if (ValidateTyped(method, forwarded) is { } problem)
        {
            throw HostErrors.BadParams($"{method}: {problem}");
        }

        Task tail;
        Task<UpstreamSession?> sessionTask;
        CancellationToken generationToken;
        lock (_lock)
        {
            if (!_started)
            {
                throw HostErrors.NotInitialized();
            }

            if (requested != _generation)
            {
                throw HostErrors.Stale(requested, _generation);
            }

            generationToken = _generationCts.Token;
            tail = _tail;
            sessionTask = _session;
        }

        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, generationToken);
        try
        {
            await tail.WaitAsync(linked.Token).ConfigureAwait(false);
            var session = await sessionTask.WaitAsync(linked.Token).ConfigureAwait(false);
            if (session is null || session.Exited)
            {
                throw HostErrors.LanguageServerUnavailable(_launcher is null ? "none is configured" : "it is not running");
            }

            // WaitAsync returns as soon as the token fires; StreamJsonRpc separately sends $/cancelRequest upstream.
            return await session.Rpc.InvokeWithParameterObjectAsync<JsonElement>(method, forwarded, linked.Token)
                .WaitAsync(linked.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested && generationToken.IsCancellationRequested)
        {
            throw HostErrors.Stale(requested, Generation);
        }
        catch (ConnectionLostException)
        {
            throw HostErrors.LanguageServerUnavailable("it exited");
        }
        catch (RemoteInvocationException ex)
        {
            // Pass the language server's own error code through (for example its ContentModified or RequestFailed).
            throw new LocalRpcException(ex.Message) { ErrorCode = ex.ErrorCode, ErrorData = ex.ErrorData };
        }
    }

    /// <summary>Minimal shape check for typed requests; returns a problem description or null.</summary>
    internal static string? ValidateTyped(string method, JsonElement p)
    {
        static bool HasString(JsonElement e, string name) =>
            e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String;

        return method switch
        {
            "workspace/symbol" => HasString(p, "query") ? null : "params.query (string) is required",
            "completionItem/resolve" => HasString(p, "label") ? null : "params.label (string) is required",
            _ when TypedRequests.Contains(method) =>
                p.ValueKind == JsonValueKind.Object && p.TryGetProperty("textDocument", out var td) && HasString(td, "uri")
                    ? null
                    : "params.textDocument.uri (string) is required",
            _ => null,
        };
    }

    private void DidOpen(JsonElement parameters)
    {
        string? uri;
        lock (_lock)
        {
            uri = _documents.Open(parameters);
            EnqueueNotificationLocked("textDocument/didOpen", parameters);
        }

        if (uri is not null)
        {
            _warmer.Opened(uri);
        }
    }

    private void DidChange(JsonElement parameters)
    {
        string? uri;
        lock (_lock)
        {
            uri = _documents.Change(parameters);
            EnqueueNotificationLocked("textDocument/didChange", parameters);
        }

        if (uri is not null)
        {
            _warmer.Changed(uri);
        }
    }

    private void DidClose(JsonElement parameters)
    {
        string? uri;
        long generation;
        lock (_lock)
        {
            uri = _documents.Close(parameters);
            generation = _generation;
            EnqueueNotificationLocked("textDocument/didClose", parameters);
        }

        if (uri is not null)
        {
            _warmer.Closed(uri);
            _ = NotifyShellAsync("textDocument/publishDiagnostics", new { uri, diagnostics = Array.Empty<object>(), nielloGeneration = generation });
        }
    }

    private void ForwardNotification(string method, JsonElement parameters)
    {
        lock (_lock)
        {
            EnqueueNotificationLocked(method, parameters);
        }
    }

    private void EnqueueNotificationLocked(string method, JsonElement parameters)
    {
        if (!_started)
        {
            _log.WriteLine($"dropping {method}: niello/host/initialize has not been received");
            return;
        }

        var payload = StripGeneration(parameters);
        var previous = _tail;
        var session = _session;
        _tail = SendAfterAsync(previous, session, method, payload);
    }

    private async Task SendAfterAsync(Task previous, Task<UpstreamSession?> sessionTask, string method, JsonElement payload)
    {
        try
        {
            await previous.ConfigureAwait(false);
            var session = await sessionTask.ConfigureAwait(false);
            if (session is null || session.Exited)
            {
                return;
            }

            await session.Rpc.NotifyWithParameterObjectAsync(method, payload).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            await _log.WriteLineAsync($"forwarding {method} failed: {ex.GetBaseException().Message}").ConfigureAwait(false);
        }
    }

    private void EnqueueRestartLocked(string? solutionPath)
    {
        var previous = _tail;
        var old = _session;
        var replay = _documents.Snapshot();
        var next = RestartAfterAsync(previous, old, solutionPath, replay);
        _session = next;
        _tail = next;
    }

    private async Task<UpstreamSession?> RestartAfterAsync(Task previous, Task<UpstreamSession?> old, string? solutionPath, IReadOnlyList<OpenDocument> replay)
    {
        try
        {
            await previous.ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            // The chain never faults; be defensive anyway.
        }

        await NotifyShellAsync("niello/languageServer/status", new LanguageServerStatus("restarting")).ConfigureAwait(false);
        if (await old.ConfigureAwait(false) is { } session)
        {
            await DisposeSessionAsync(session).ConfigureAwait(false);
        }

        return await LaunchSessionAsync(solutionPath, replay).ConfigureAwait(false);
    }

    // ------------------------------------------------------------------ solution load

    private async Task LoadAsync(SolutionLoad load)
    {
        try
        {
            var (projects, legacy) = CountProjects(load.Path);
            load.Counts = new ProjectCounts(projects, legacy, 0);
            var prepare = _preparer is not null && legacy > 0;
            await NotifyStatusAsync(load, SolutionStates.Loading, phase: prepare ? "legacyEvaluation" : "projectLoad").ConfigureAwait(false);

            if (_launcher is null)
            {
                await FailAsync(load, "NIELLO0001", "No language server is configured; the solution is shown without semantic features.").ConfigureAwait(false);
                return;
            }

            if (prepare)
            {
                load.Preparation = await _preparer!.PrepareAsync(load.Path, CancellationToken.None).ConfigureAwait(false);
                load.Counts = load.Counts with { LegacyEvaluationFailures = load.Preparation.LegacyEvaluationFailures };
                if (IsSuperseded(load))
                {
                    return;
                }

                await NotifyStatusAsync(load, SolutionStates.Loading, phase: "projectLoad").ConfigureAwait(false);
            }

            Task tail;
            Task<UpstreamSession?> sessionTask;
            lock (_lock)
            {
                if (!ReferenceEquals(_load, load))
                {
                    return;
                }

                tail = _tail;
                sessionTask = _session;
            }

            await tail.ConfigureAwait(false);
            var session = await sessionTask.ConfigureAwait(false);
            if (session is null || session.Exited)
            {
                await FailAsync(load, "NIELLO0001", "The language server failed to start; see the host log.").ConfigureAwait(false);
                return;
            }

            lock (_lock)
            {
                if (!ReferenceEquals(_load, load))
                {
                    return;
                }

                load.Session = session;
            }

            if (Path.GetExtension(load.Path) is ".csproj" or ".vbproj")
            {
                // Roslyn extension: open individual projects when there is no solution file.
                await session.Rpc.NotifyWithParameterObjectAsync("project/open", new { projects = new[] { new Uri(load.Path).AbsoluteUri } }).ConfigureAwait(false);
            }
            else
            {
                // Roslyn extension: open a solution explicitly (used by the VS Code C# extension and roslyn.nvim).
                await session.Rpc.NotifyWithParameterObjectAsync("solution/open", new { solution = new Uri(load.Path).AbsoluteUri }).ConfigureAwait(false);
            }

            await _log.WriteLineAsync($"opening {load.Path} (solution generation {load.Generation})").ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            await _log.WriteLineAsync($"loading {load.Path} failed: {ex}").ConfigureAwait(false);
            await FailAsync(load, "NIELLO0001", $"Loading failed: {ex.GetBaseException().Message}").ConfigureAwait(false);
        }
    }

    private static (int Projects, int Legacy) CountProjects(string path)
    {
        try
        {
            var projects = SolutionProjects.Read(path);
            return (projects.Count, projects.Count(SolutionProjects.IsLegacy));
        }
        catch (Exception ex) when (ex is IOException or System.Xml.XmlException or UnauthorizedAccessException)
        {
            return (0, 0);
        }
    }

    private bool IsSuperseded(SolutionLoad load)
    {
        lock (_lock)
        {
            return !ReferenceEquals(_load, load);
        }
    }

    private async Task FailAsync(SolutionLoad load, string code, string message)
    {
        lock (_lock)
        {
            if (!ReferenceEquals(_load, load) || load.Done)
            {
                return;
            }

            load.Done = true;
        }

        await NotifyStatusAsync(load, SolutionStates.Failed, diagnostics: [new HostDiagnostic("error", code, message)]).ConfigureAwait(false);
    }

    private Task NotifyStatusAsync(SolutionLoad load, string state, string? phase = null, IReadOnlyList<HostDiagnostic>? diagnostics = null)
    {
        var prep = load.Preparation;
        var status = new SolutionStatus(load.Generation, load.Path, state)
        {
            Phase = phase,
            Counts = state == SolutionStates.Loaded ? load.Counts : null,
            Msbuild = state is SolutionStates.Loaded or SolutionStates.Failed ? prep.MsBuild : null,
            Corrections = state == SolutionStates.Loaded && prep.Corrections.Count > 0 ? prep.Corrections : null,
            Diagnostics = diagnostics ?? (state == SolutionStates.Loaded && prep.Diagnostics.Count > 0 ? prep.Diagnostics : null),
            ElapsedMs = Math.Round(load.Elapsed.Elapsed.TotalMilliseconds, 1),
        };
        return NotifyShellAsync("niello/solution/status", status);
    }

    private async Task OnProjectsLoadedAsync(UpstreamSession session)
    {
        SolutionLoad? loaded = null;
        IReadOnlyList<string> open;
        lock (_lock)
        {
            if (_load is { Done: false } load && ReferenceEquals(load.Session, session))
            {
                load.Done = true;
                loaded = load;
            }

            open = _documents.Snapshot().Select(d => d.Uri).ToList();
        }

        _projectsLoaded.TrySetResult();
        if (loaded is not null)
        {
            await _log.WriteLineAsync($"roslyn-ls: projects loaded (solution generation {loaded.Generation}, {loaded.Elapsed.ElapsedMilliseconds} ms)").ConfigureAwait(false);
            await NotifyStatusAsync(loaded, SolutionStates.Loaded).ConfigureAwait(false);
        }

        // Documents opened before the load moved from Roslyn's miscellaneous files to their projects: warm them.
        _warmer.PullAll(open);
    }

    // ------------------------------------------------------------------ warming

    private async Task PullDiagnosticsAsync(string uri, CancellationToken cancellationToken)
    {
        Task tail;
        Task<UpstreamSession?> sessionTask;
        long generation;
        int version;
        lock (_lock)
        {
            if (_documents.Get(uri) is not { } document)
            {
                return;
            }

            version = document.Version;
            generation = _generation;
            tail = _tail;
            sessionTask = _session;
        }

        await tail.WaitAsync(cancellationToken).ConfigureAwait(false);
        var session = await sessionTask.WaitAsync(cancellationToken).ConfigureAwait(false);
        if (session is null || session.Exited)
        {
            return;
        }

        var report = await session.Rpc.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/diagnostic", new { textDocument = new { uri } }, cancellationToken).WaitAsync(cancellationToken).ConfigureAwait(false);
        Interlocked.Increment(ref _pullsCompleted);
        if (report.ValueKind != JsonValueKind.Object || !report.TryGetProperty("kind", out var kind) || kind.GetString() != "full"
            || !report.TryGetProperty("items", out var items))
        {
            return;
        }

        lock (_lock)
        {
            if (generation != _generation || _documents.Get(uri)?.Version != version)
            {
                return;
            }
        }

        await NotifyShellAsync("textDocument/publishDiagnostics", new { uri, version, diagnostics = items, nielloGeneration = generation }).ConfigureAwait(false);
    }

    // ------------------------------------------------------------------ server -> host

    private async Task<UpstreamSession?> LaunchSessionAsync(string? solutionPath, IReadOnlyList<OpenDocument> replay)
    {
        // Run off the caller's thread: niello/host/initialize must not wait for the child process.
        await Task.Yield();
        await NotifyShellAsync("niello/languageServer/status", new LanguageServerStatus("starting")).ConfigureAwait(false);
        LanguageServerConnection? connection = null;
        try
        {
            connection = await _launcher!.LaunchAsync(CancellationToken.None).ConfigureAwait(false);
            var rpc = HostServer.CreateConnection(connection.ToServer, connection.FromServer);
            var session = new UpstreamSession(rpc, connection);
            // NIELLO_LSP_TRACE=1 traces every upstream message to the host log; =warn only warnings. Off by default
            // because StreamJsonRpc's built-in $/progress handling logs an error for every LSP string progress token.
            rpc.TraceSource = new TraceSource("roslyn-ls-rpc", Environment.GetEnvironmentVariable("NIELLO_LSP_TRACE") switch
            {
                "1" => SourceLevels.Verbose,
                "warn" => SourceLevels.Warning,
                _ => SourceLevels.Off,
            });
            rpc.TraceSource.Listeners.Clear();
            rpc.TraceSource.Listeners.Add(new TextWriterTraceListener(_log));
            RegisterServerToClientHandlers(session);
            rpc.Disconnected += (_, e) => _ = OnDisconnectedAsync(session, e.Description);
            rpc.StartListening();

            var root = solutionPath is null ? null : Path.GetDirectoryName(solutionPath);
            var rootUri = root is null ? null : new Uri(root + Path.DirectorySeparatorChar).AbsoluteUri;
            var init = await rpc.InvokeWithParameterObjectAsync<JsonElement>(
                "initialize",
                new
                {
                    processId = Environment.ProcessId,
                    clientInfo = new { name = HostRpcTarget.HostName, version = HostRpcTarget.HostVersion },
                    rootUri,
                    workspaceFolders = rootUri is null ? null : new[] { new { uri = rootUri, name = Path.GetFileName(root) } },
                    capabilities = ClientCapabilities,
                }).ConfigureAwait(false);
            await rpc.NotifyWithParameterObjectAsync("initialized", new { }).ConfigureAwait(false);
            foreach (var document in replay)
            {
                await rpc.NotifyWithParameterObjectAsync("textDocument/didOpen", new
                {
                    textDocument = new { uri = document.Uri, languageId = document.LanguageId, version = document.Version, text = document.Text },
                }).ConfigureAwait(false);
            }

            await _log.WriteLineAsync($"roslyn-ls initialized ({replay.Count} document(s) replayed)").ConfigureAwait(false);
            await NotifyShellAsync("niello/languageServer/status", new LanguageServerStatus("running")
            {
                ServerInfo = init.TryGetProperty("serverInfo", out var info) ? info.Clone() : null,
                Capabilities = init.TryGetProperty("capabilities", out var caps) ? caps.Clone() : null,
            }).ConfigureAwait(false);
            return session;
        }
        catch (Exception ex) when (ex is not OutOfMemoryException)
        {
            await _log.WriteLineAsync($"roslyn-ls failed to start: {ex.GetBaseException().Message}").ConfigureAwait(false);
            if (connection is not null)
            {
                await connection.Lifetime.DisposeAsync().ConfigureAwait(false);
            }

            await NotifyShellAsync("niello/languageServer/status", new LanguageServerStatus("unavailable")
            {
                Message = $"The language server failed to start: {ex.GetBaseException().Message}",
            }).ConfigureAwait(false);
            return null;
        }
    }

    private async Task OnDisconnectedAsync(UpstreamSession session, string description)
    {
        SolutionLoad? failed = null;
        lock (_lock)
        {
            if (session.Deliberate || _disposing)
            {
                return;
            }

            session.Exited = true;
            if (_load is { Done: false } load && ReferenceEquals(load.Session, session))
            {
                failed = load;
            }
        }

        await _log.WriteLineAsync($"roslyn-ls exited: {description}").ConfigureAwait(false);
        await NotifyShellAsync("niello/languageServer/status", new LanguageServerStatus("exited") { Message = description }).ConfigureAwait(false);
        if (failed is not null)
        {
            await FailAsync(failed, "NIELLO0002", $"The language server exited while the solution was loading: {description}").ConfigureAwait(false);
        }
    }

    private static readonly object ClientCapabilities = new
    {
        workspace = new
        {
            configuration = true,
            workspaceFolders = true,
            didChangeWatchedFiles = new { dynamicRegistration = false },
        },
        textDocument = new
        {
            synchronization = new { didSave = true },
            completion = new
            {
                contextSupport = true,
                completionItem = new
                {
                    snippetSupport = true,
                    insertReplaceSupport = true,
                    labelDetailsSupport = true,
                    resolveSupport = new { properties = new[] { "documentation", "detail", "additionalTextEdits" } },
                },
                completionList = new { itemDefaults = new[] { "commitCharacters", "editRange", "insertTextFormat", "data" } },
            },
            documentSymbol = new { hierarchicalDocumentSymbolSupport = true },
            hover = new { contentFormat = new[] { "markdown", "plaintext" } },
            signatureHelp = new { },
            definition = new { },
            references = new { },
            publishDiagnostics = new { },
            diagnostic = new { dynamicRegistration = false },
        },
        window = new { workDoneProgress = true },
    };

    private void RegisterServerToClientHandlers(UpstreamSession session)
    {
        var upstream = session.Rpc;
        AddMethod(upstream, "workspace/configuration", new Func<JsonElement, JsonElement>(AnswerConfiguration));
        foreach (var method in new[]
                 {
                     "client/registerCapability", "client/unregisterCapability", "window/workDoneProgress/create",
                     "window/showMessageRequest", "workspace/semanticTokens/refresh", "workspace/codeLens/refresh",
                     "workspace/inlayHint/refresh",
                 })
        {
            AddMethod(upstream, method, new Func<JsonElement, JsonElement>(_ => JsonNull));
        }

        AddMethod(upstream, "workspace/diagnostic/refresh", new Func<JsonElement, JsonElement>(_ =>
        {
            List<string> open;
            lock (_lock)
            {
                open = _documents.Snapshot().Select(d => d.Uri).ToList();
            }

            _warmer.PullAll(open);
            return JsonNull;
        }));
        var refreshBare = new Func<object?>(() =>
        {
            List<string> open;
            lock (_lock)
            {
                open = _documents.Snapshot().Select(d => d.Uri).ToList();
            }

            _warmer.PullAll(open);
            return null;
        });
        upstream.AddLocalRpcMethod(refreshBare.Method, refreshBare.Target, new JsonRpcMethodAttribute("workspace/diagnostic/refresh"));

        AddMethod(upstream, "window/logMessage", new Func<JsonElement, Task>(p =>
            _log.WriteLineAsync($"[roslyn-ls log] {(p.TryGetProperty("message", out var m) ? m.GetString() : p.GetRawText())}")));
        AddMethod(upstream, "telemetry/event", new Func<JsonElement, Task>(_ => Task.CompletedTask));

        // Roslyn sends workspace/projectInitializationComplete with no params at all, which does not bind to a
        // single-object handler; register both shapes.
        AddMethod(upstream, ProjectInitializationComplete, new Func<JsonElement, Task>(_ => OnProjectsLoadedAsync(session)));
        var loadedBare = new Func<Task>(() => OnProjectsLoadedAsync(session));
        upstream.AddLocalRpcMethod(loadedBare.Method, loadedBare.Target, new JsonRpcMethodAttribute(ProjectInitializationComplete));

        foreach (var method in RelayedServerNotifications)
        {
            AddMethod(upstream, method, new Func<JsonElement, Task>(p => NotifyShellAsync(method, p.Clone())));
        }
    }

    /// <summary>
    /// Settings the host supplies to Roslyn's <c>workspace/configuration</c> requests; everything else is null
    /// (Roslyn's default). File-based programs are off: with them on, a document opened before the solution
    /// finishes loading is captured by Roslyn's "canonical misc" project and never moves to its real project,
    /// so semantic completion in it stays empty for the life of the session (observed at the pinned commit).
    /// </summary>
    internal static IReadOnlyDictionary<string, JsonElement> ConfigurationOverrides { get; } = new Dictionary<string, JsonElement>
    {
        ["projects.dotnet_enable_file_based_programs"] = JsonSerializer.SerializeToElement(false),
    };

    internal static JsonElement AnswerConfiguration(JsonElement parameters)
    {
        var answers = new List<JsonElement>();
        if (parameters.TryGetProperty("items", out var items) && items.ValueKind == JsonValueKind.Array)
        {
            foreach (var item in items.EnumerateArray())
            {
                var section = item.TryGetProperty("section", out var s) ? s.GetString() : null;
                answers.Add(section is not null && ConfigurationOverrides.TryGetValue(section, out var value) ? value : JsonNull);
            }
        }

        return JsonSerializer.SerializeToElement(answers);
    }

    private static void AddMethod(JsonRpc rpc, string method, Delegate handler) =>
        rpc.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = true });

    private async Task NotifyShellAsync(string method, object? parameters)
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
            // The shell went away; nothing to tell.
        }
    }

    /// <summary>Splits <c>nielloGeneration</c> off request params. The returned params are a detached copy.</summary>
    internal static (JsonElement Forwarded, long? Generation) ExtractGeneration(JsonElement parameters)
    {
        if (parameters.ValueKind != JsonValueKind.Object)
        {
            return (parameters.ValueKind == JsonValueKind.Undefined ? parameters : parameters.Clone(), null);
        }

        long? generation = parameters.TryGetProperty(GenerationProperty, out var g) && g.ValueKind == JsonValueKind.Number
                                                                                   && g.TryGetInt64(out var value) && value >= 0
            ? value
            : null;
        return (StripGeneration(parameters), generation);
    }

    /// <summary>A detached copy of <paramref name="parameters"/> without <c>nielloGeneration</c>.</summary>
    internal static JsonElement StripGeneration(JsonElement parameters)
    {
        if (parameters.ValueKind != JsonValueKind.Object || !parameters.TryGetProperty(GenerationProperty, out _))
        {
            return parameters.ValueKind == JsonValueKind.Undefined ? parameters : parameters.Clone();
        }

        var node = JsonNode.Parse(parameters.GetRawText())!.AsObject();
        node.Remove(GenerationProperty);
        return JsonSerializer.SerializeToElement(node);
    }

    private async Task DisposeSessionAsync(UpstreamSession session)
    {
        session.Deliberate = true;
        if (!session.Exited)
        {
            try
            {
                using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await session.Rpc.InvokeWithCancellationAsync<JsonElement>("shutdown", [], timeout.Token).ConfigureAwait(false);
                await session.Rpc.NotifyAsync("exit").ConfigureAwait(false);
            }
            catch (Exception ex) when (ex is OperationCanceledException or ConnectionLostException or RemoteInvocationException or ObjectDisposedException or IOException)
            {
                await _log.WriteLineAsync($"roslyn-ls shutdown: {ex.Message}").ConfigureAwait(false);
            }
        }

        session.Rpc.Dispose();
        await session.Connection.Lifetime.DisposeAsync().ConfigureAwait(false);
    }

    public async ValueTask DisposeAsync()
    {
        Task<UpstreamSession?> sessionTask;
        Task tail;
        lock (_lock)
        {
            _disposing = true;
            sessionTask = _session;
            tail = _tail;
        }

        _warmer.Dispose();
        try
        {
            await tail.WaitAsync(TimeSpan.FromSeconds(30)).ConfigureAwait(false);
        }
        catch (TimeoutException)
        {
            await _log.WriteLineAsync("pending language server work did not finish in 30 s").ConfigureAwait(false);
        }

        if (sessionTask.IsCompletedSuccessfully && sessionTask.Result is { } session)
        {
            await DisposeSessionAsync(session).ConfigureAwait(false);
        }

        _generationCts.Dispose();
    }

    private sealed class UpstreamSession(JsonRpc rpc, LanguageServerConnection connection)
    {
        public JsonRpc Rpc { get; } = rpc;

        public LanguageServerConnection Connection { get; } = connection;

        /// <summary>Set when the server went away on its own.</summary>
        public volatile bool Exited;

        /// <summary>Set when the host stops the server on purpose.</summary>
        public volatile bool Deliberate;
    }

    private sealed class SolutionLoad(long generation, string path)
    {
        public long Generation { get; } = generation;

        public string Path { get; } = path;

        public Stopwatch Elapsed { get; } = Stopwatch.StartNew();

        public ProjectCounts Counts { get; set; } = new(0, 0, 0);

        public SolutionPreparation Preparation { get; set; } = SolutionPreparation.None;

        public UpstreamSession? Session { get; set; }

        public bool Done { get; set; }
    }
}
