using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Nodes;
using Niello.Host.Rpc;
using StreamJsonRpc;

namespace Niello.Host.Lsp;

/// <summary>
/// Forwards LSP 3.17 traffic between the shell connection and an upstream language server
/// (the Roslyn language server, spawned as a child process). Brief 0002 spike.
/// </summary>
/// <remarks>
/// <para>
/// The host owns the upstream LSP handshake: the shell's <c>initialize</c> is the niello-host
/// handshake, so the host sends LSP <c>initialize</c>/<c>initialized</c> itself and then Roslyn's
/// <c>solution/open</c> notification. Forwarded requests wait until that handshake is done.
/// </para>
/// <para>
/// Cancellation: an incoming <c>$/cancelRequest</c> cancels the handler's token. The handler stops
/// waiting at once (the shell gets error -32800 RequestCancelled, never a result) and StreamJsonRpc
/// sends <c>$/cancelRequest</c> upstream so Roslyn stops working on it.
/// </para>
/// <para>
/// Solution generation: <see cref="Generation"/> counts completed solution loads (each upstream
/// <c>workspace/projectInitializationComplete</c>). A forwarded request may carry
/// <c>params.nielloGeneration</c>; the host strips it before forwarding. If it does not match the
/// current generation, or the generation moves while the request is in flight, the request is
/// canceled upstream and fails with -32801 ContentModified, so a stale result is never delivered.
/// </para>
/// </remarks>
public sealed class LspProxy : IAsyncDisposable
{
    /// <summary>LSP error code for a result invalidated by a state change.</summary>
    public const int ContentModified = -32801;

    /// <summary>Property a shell may add to forwarded request params to pin a solution generation.</summary>
    public const string GenerationProperty = "nielloGeneration";

    /// <summary>Client-to-server requests forwarded verbatim (plain LSP method names).</summary>
    public static IReadOnlyList<string> ForwardedRequests { get; } =
    [
        "textDocument/completion",
        "completionItem/resolve",
        "textDocument/documentSymbol",
        "textDocument/hover",
        "textDocument/signatureHelp",
        "textDocument/definition",
        "textDocument/typeDefinition",
        "textDocument/implementation",
        "textDocument/references",
        "textDocument/documentHighlight",
        "textDocument/semanticTokens/full",
        "textDocument/semanticTokens/range",
        "textDocument/diagnostic",
        "textDocument/codeAction",
        "textDocument/formatting",
        "textDocument/rename",
        "workspace/symbol",
    ];

    /// <summary>Client-to-server notifications forwarded verbatim.</summary>
    public static IReadOnlyList<string> ForwardedNotifications { get; } =
    [
        "textDocument/didOpen",
        "textDocument/didChange",
        "textDocument/didClose",
        "textDocument/didSave",
        "workspace/didChangeWatchedFiles",
    ];

    /// <summary>Server-to-client notifications relayed to the shell.</summary>
    public static IReadOnlyList<string> RelayedServerNotifications { get; } =
    [
        ProjectInitializationComplete,
        "textDocument/publishDiagnostics",
        "window/showMessage",
        "$/progress",
    ];

    internal const string ProjectInitializationComplete = "workspace/projectInitializationComplete";

    private static readonly JsonElement JsonNull = JsonDocument.Parse("null").RootElement.Clone();

    private readonly ILanguageServerLauncher _launcher;
    private readonly TextWriter _log;
    private readonly TaskCompletionSource _projectsLoaded = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly Lock _generationLock = new();
    private CancellationTokenSource _generationCts = new();
    private long _generation;
    private JsonRpc? _shell;
    private Task<JsonRpc>? _upstream;
    private LanguageServerConnection? _connection;
    private readonly Func<string?, CancellationToken, Task>? _beforeLaunch;

    /// <param name="beforeLaunch">
    /// Runs before the server process starts, with the solution path (brief 0003: legacy design-time preparation).
    /// Its failure is logged and does not stop the server.
    /// </param>
    public LspProxy(ILanguageServerLauncher launcher, TextWriter log, Func<string?, CancellationToken, Task>? beforeLaunch = null)
    {
        _launcher = launcher;
        _log = log;
        _beforeLaunch = beforeLaunch;
    }

    /// <summary>Number of completed solution loads. 0 until the first load finishes.</summary>
    public long Generation => Interlocked.Read(ref _generation);

    /// <summary>Completes when the upstream server first reports that projects are loaded.</summary>
    public Task ProjectsLoaded => _projectsLoaded.Task;

    /// <summary>Registers the forwarded methods on the shell connection. Call before it starts listening.</summary>
    public void Attach(JsonRpc shell)
    {
        ArgumentNullException.ThrowIfNull(shell);
        _shell = shell;
        foreach (var method in ForwardedRequests)
        {
            var handler = new Func<JsonElement, CancellationToken, Task<JsonElement>>((p, ct) => ForwardRequestAsync(method, p, ct));
            shell.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = true });
        }

        foreach (var method in ForwardedNotifications)
        {
            var handler = new Func<JsonElement, Task>(p => ForwardNotificationAsync(method, p));
            shell.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = true });
        }
    }

    /// <summary>Starts the upstream server in the background and opens <paramref name="solutionPath"/> if given.</summary>
    public void Start(string? solutionPath)
    {
        if (_upstream is not null)
        {
            return;
        }

        _upstream = StartUpstreamAsync(solutionPath);
        _upstream.ContinueWith(
            t => _log.WriteLine($"roslyn-ls failed to start: {t.Exception?.GetBaseException().Message}"),
            CancellationToken.None,
            TaskContinuationOptions.OnlyOnFaulted,
            TaskScheduler.Default);
    }

    /// <summary>Advances the solution generation and cancels every in-flight generation-pinned request.</summary>
    public long BumpGeneration()
    {
        CancellationTokenSource old;
        long next;
        lock (_generationLock)
        {
            old = _generationCts;
            _generationCts = new CancellationTokenSource();
            next = Interlocked.Increment(ref _generation);
        }

        old.Cancel();
        old.Dispose();
        return next;
    }

    private async Task<JsonRpc> StartUpstreamAsync(string? solutionPath)
    {
        // Run off the caller's thread: the shell's initialize must not wait for the child process.
        await Task.Yield();
        if (_beforeLaunch is not null)
        {
            try
            {
                await _beforeLaunch(solutionPath, CancellationToken.None).ConfigureAwait(false);
            }
            catch (Exception ex) when (ex is not OutOfMemoryException)
            {
                await _log.WriteLineAsync($"pre-launch preparation failed (continuing): {ex.Message}").ConfigureAwait(false);
            }
        }

        _connection = await _launcher.LaunchAsync(CancellationToken.None).ConfigureAwait(false);
        var upstream = HostServer.CreateConnection(_connection.ToServer, _connection.FromServer);
        // NIELLO_LSP_TRACE=1 traces every upstream message to the host log; =warn only warnings. Off by default
        // because StreamJsonRpc's built-in $/progress handling logs an error for every LSP string progress token.
        upstream.TraceSource = new TraceSource("roslyn-ls-rpc", Environment.GetEnvironmentVariable("NIELLO_LSP_TRACE") switch
        {
            "1" => SourceLevels.Verbose,
            "warn" => SourceLevels.Warning,
            _ => SourceLevels.Off,
        });
        upstream.TraceSource.Listeners.Clear();
        upstream.TraceSource.Listeners.Add(new TextWriterTraceListener(_log));
        RegisterServerToClientHandlers(upstream);
        upstream.StartListening();

        var root = solutionPath is null ? null : Path.GetDirectoryName(Path.GetFullPath(solutionPath));
        var rootUri = root is null ? null : new Uri(root + Path.DirectorySeparatorChar).AbsoluteUri;
        await upstream.InvokeWithParameterObjectAsync<JsonElement>(
            "initialize",
            new
            {
                processId = Environment.ProcessId,
                clientInfo = new { name = HostRpcTarget.HostName, version = HostRpcTarget.HostVersion },
                rootUri,
                workspaceFolders = rootUri is null ? null : new[] { new { uri = rootUri, name = Path.GetFileName(root) } },
                capabilities = ClientCapabilities,
            }).ConfigureAwait(false);
        await upstream.NotifyWithParameterObjectAsync("initialized", new { }).ConfigureAwait(false);

        if (solutionPath is not null && solutionPath.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase))
        {
            // Roslyn extension: open individual projects when there is no solution file (brief 0003 corpus).
            await upstream.NotifyWithParameterObjectAsync(
                "project/open",
                new { projects = new[] { new Uri(Path.GetFullPath(solutionPath)).AbsoluteUri } }).ConfigureAwait(false);
        }
        else if (solutionPath is not null)
        {
            // Roslyn extension: open a solution explicitly (used by the VS Code C# extension and roslyn.nvim).
            await upstream.NotifyWithParameterObjectAsync(
                "solution/open",
                new { solution = new Uri(Path.GetFullPath(solutionPath)).AbsoluteUri }).ConfigureAwait(false);
        }

        _log.WriteLine("roslyn-ls initialized" + (solutionPath is null ? string.Empty : $"; opening {solutionPath}"));
        return upstream;
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

    private void RegisterServerToClientHandlers(JsonRpc upstream)
    {
        // Server-to-client requests the host answers itself for the spike.
        AddUpstream(upstream, "workspace/configuration", new Func<JsonElement, JsonElement>(AnswerConfiguration));
        foreach (var method in new[]
                 {
                     "client/registerCapability", "client/unregisterCapability", "window/workDoneProgress/create",
                     "window/showMessageRequest", "workspace/semanticTokens/refresh", "workspace/diagnostic/refresh",
                     "workspace/codeLens/refresh", "workspace/inlayHint/refresh",
                 })
        {
            AddUpstream(upstream, method, new Func<JsonElement, JsonElement>(_ => JsonNull));
        }

        AddUpstream(upstream, "window/logMessage", new Func<JsonElement, Task>(p =>
            _log.WriteLineAsync($"[roslyn-ls log] {(p.TryGetProperty("message", out var m) ? m.GetString() : p.GetRawText())}")));
        AddUpstream(upstream, "telemetry/event", new Func<JsonElement, Task>(_ => Task.CompletedTask));

        foreach (var method in RelayedServerNotifications)
        {
            AddUpstream(upstream, method, new Func<JsonElement, Task>(p => RelayServerNotificationAsync(method, p)));

            // Roslyn sends workspace/projectInitializationComplete with no params at all, which does not bind to
            // the single-object overload above; register a parameterless overload too.
            var bare = new Func<Task>(() => RelayServerNotificationAsync(method, default));
            upstream.AddLocalRpcMethod(bare.Method, bare.Target, new JsonRpcMethodAttribute(method));
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

    private static void AddUpstream(JsonRpc upstream, string method, Delegate handler) =>
        upstream.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = true });

    private async Task RelayServerNotificationAsync(string method, JsonElement parameters)
    {
        if (method == ProjectInitializationComplete)
        {
            var generation = BumpGeneration();
            _projectsLoaded.TrySetResult();
            await _log.WriteLineAsync($"roslyn-ls: projects loaded (solution generation {generation})").ConfigureAwait(false);
        }

        if (_shell is { } shell)
        {
            await shell.NotifyWithParameterObjectAsync(method, parameters.ValueKind == JsonValueKind.Undefined ? null : parameters).ConfigureAwait(false);
        }
    }

    private async Task<JsonElement> ForwardRequestAsync(string method, JsonElement parameters, CancellationToken cancellationToken)
    {
        var (forwarded, pinned) = ExtractGeneration(parameters);
        CancellationToken generationToken;
        lock (_generationLock)
        {
            if (pinned is { } g && g != _generation)
            {
                throw StaleGeneration(g);
            }

            generationToken = pinned is null ? CancellationToken.None : _generationCts.Token;
        }

        using var linked = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken, generationToken);
        try
        {
            var upstream = await (_upstream ?? throw new InvalidOperationException("language server not started; send initialize first"))
                .WaitAsync(linked.Token).ConfigureAwait(false);

            // WaitAsync returns as soon as the token fires; StreamJsonRpc separately sends $/cancelRequest upstream.
            return await upstream.InvokeWithParameterObjectAsync<JsonElement>(method, forwarded, linked.Token)
                .WaitAsync(linked.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested && generationToken.IsCancellationRequested)
        {
            throw StaleGeneration(pinned!.Value);
        }
    }

    private async Task ForwardNotificationAsync(string method, JsonElement parameters)
    {
        if (_upstream is null)
        {
            await _log.WriteLineAsync($"dropping {method}: language server not started").ConfigureAwait(false);
            return;
        }

        var upstream = await _upstream.ConfigureAwait(false);
        await upstream.NotifyWithParameterObjectAsync(method, parameters).ConfigureAwait(false);
    }

    private LocalRpcException StaleGeneration(long requested) =>
        new($"solution generation {requested} is stale (current {Generation})") { ErrorCode = ContentModified };

    internal static (JsonElement Forwarded, long? Generation) ExtractGeneration(JsonElement parameters)
    {
        if (parameters.ValueKind != JsonValueKind.Object || !parameters.TryGetProperty(GenerationProperty, out var g))
        {
            return (parameters, null);
        }

        var node = JsonNode.Parse(parameters.GetRawText())!.AsObject();
        node.Remove(GenerationProperty);
        return (JsonSerializer.SerializeToElement(node), g.GetInt64());
    }

    public async ValueTask DisposeAsync()
    {
        if (_upstream is { IsCompletedSuccessfully: true } started)
        {
            var upstream = started.Result;
            try
            {
                using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await upstream.InvokeWithCancellationAsync<JsonElement>("shutdown", [], timeout.Token).ConfigureAwait(false);
                await upstream.NotifyAsync("exit").ConfigureAwait(false);
            }
            catch (Exception ex) when (ex is OperationCanceledException or ConnectionLostException or RemoteInvocationException or ObjectDisposedException)
            {
                await _log.WriteLineAsync($"roslyn-ls shutdown: {ex.Message}").ConfigureAwait(false);
            }

            upstream.Dispose();
        }
        else if (_upstream is not null)
        {
            try
            {
                await _upstream.ConfigureAwait(false);
            }
            catch (Exception ex) when (ex is not OutOfMemoryException)
            {
                // Startup failed; nothing to shut down beyond the process.
            }
        }

        if (_connection is not null)
        {
            await _connection.Lifetime.DisposeAsync().ConfigureAwait(false);
        }

        _generationCts.Dispose();
    }
}
