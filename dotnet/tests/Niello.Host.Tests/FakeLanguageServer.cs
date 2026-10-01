using System.Text.Json;
using Nerdbank.Streams;
using Niello.Host.Lsp;
using Niello.Host.Rpc;
using StreamJsonRpc;

namespace Niello.Host.Tests;

/// <summary>
/// An in-memory LSP server standing in for Roslyn. Each <see cref="LaunchAsync"/> is a fresh server process; the
/// message log spans launches.
/// </summary>
internal sealed class FakeLanguageServer : ILanguageServerLauncher
{
    private readonly Lock _lock = new();
    private readonly List<(int Launch, string Method, JsonElement Params)> _received = [];
    private JsonRpc? _rpc;
    private int _launches;

    public int Launches => Volatile.Read(ref _launches);

    public TaskCompletionSource CompletionStarted { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

    public TaskCompletionSource CompletionCanceled { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

    public bool HangCompletion { get; set; }

    /// <summary>When set, <c>textDocument/diagnostic</c> waits for it before answering.</summary>
    public TaskCompletionSource? DiagnosticGate { get; set; }

    public bool Disposed { get; private set; }

    public List<string> Snapshot()
    {
        lock (_lock)
        {
            return [.. _received.Select(r => r.Method)];
        }
    }

    public List<(int Launch, string Method, JsonElement Params)> Messages()
    {
        lock (_lock)
        {
            return [.. _received];
        }
    }

    public int Count(string method)
    {
        lock (_lock)
        {
            return _received.Count(r => r.Method == method);
        }
    }

    public JsonElement? Last(string method)
    {
        lock (_lock)
        {
            var matches = _received.Where(r => r.Method == method).ToList();
            return matches.Count == 0 ? null : matches[^1].Params;
        }
    }

    /// <summary>Waits until <paramref name="method"/> has been received at least <paramref name="count"/> times.</summary>
    public async Task WaitForAsync(string method, int count = 1, TimeSpan? timeout = null)
    {
        var deadline = DateTime.UtcNow + (timeout ?? TimeSpan.FromSeconds(10));
        while (Count(method) < count)
        {
            if (DateTime.UtcNow > deadline)
            {
                throw new TimeoutException($"{method} x{count} not received; got [{string.Join(", ", Snapshot())}]");
            }

            await Task.Delay(5);
        }
    }

    public Task<LanguageServerConnection> LaunchAsync(CancellationToken cancellationToken)
    {
        var launch = Interlocked.Increment(ref _launches);
        var (host, server) = FullDuplexStream.CreatePair();
        var rpc = HostServer.CreateConnection(server, server);
        Add(rpc, "initialize", new Func<JsonElement, object>(p => Record(launch, "initialize", p, new
        {
            capabilities = new { completionProvider = new { resolveProvider = true }, hoverProvider = true },
            serverInfo = new { name = "fake-ls", version = "1.0" },
        })));
        Add(rpc, "initialized", new Action<JsonElement>(p => Record(launch, "initialized", p, 0)));
        Add(rpc, "solution/open", new Action<JsonElement>(p => Record(launch, "solution/open", p, 0)));
        Add(rpc, "project/open", new Action<JsonElement>(p => Record(launch, "project/open", p, 0)));
        foreach (var method in new[] { "textDocument/didOpen", "textDocument/didChange", "textDocument/didClose", "textDocument/didSave" })
        {
            Add(rpc, method, new Action<JsonElement>(p => Record(launch, method, p, 0)));
        }

        Add(rpc, "textDocument/completion", new Func<JsonElement, CancellationToken, Task<object>>((p, ct) => CompletionAsync(launch, p, ct)));
        Add(rpc, "textDocument/signatureHelp", new Func<JsonElement, object>(p => Record(launch, "textDocument/signatureHelp", p, new { signatures = new[] { new { label = "M(int x)" } } })));
        Add(rpc, "workspace/symbol", new Func<JsonElement, object>(p => Record(launch, "workspace/symbol", p, Array.Empty<object>())));
        Add(rpc, "textDocument/diagnostic", new Func<JsonElement, CancellationToken, Task<object>>((p, ct) => DiagnosticAsync(launch, p, ct)));
        Add(rpc, "shutdown", new Func<object?>(() => Record<object?>(launch, "shutdown", default, null)));
        Add(rpc, "exit", new Action(() => Record(launch, "exit", default, 0)));
        rpc.StartListening();
        _rpc = rpc;
        return Task.FromResult(new LanguageServerConnection(host, host, new Lifetime(this, rpc)));
    }

    public Task NotifyProjectsLoadedAsync() => _rpc!.NotifyAsync("workspace/projectInitializationComplete");

    public Task RequestDiagnosticRefreshAsync() => _rpc!.InvokeAsync<object?>("workspace/diagnostic/refresh");

    /// <summary>Simulates the server process dying: its end of the connection goes away.</summary>
    public void Crash() => _rpc!.Dispose();

    private async Task<object> CompletionAsync(int launch, JsonElement p, CancellationToken ct)
    {
        Record(launch, "textDocument/completion", p, 0);
        CompletionStarted.TrySetResult();
        if (HangCompletion)
        {
            try
            {
                await Task.Delay(Timeout.Infinite, ct);
            }
            catch (OperationCanceledException)
            {
                CompletionCanceled.TrySetResult();
                throw;
            }
        }

        return new { isIncomplete = false, items = new[] { new { label = "Compute" } } };
    }

    private async Task<object> DiagnosticAsync(int launch, JsonElement p, CancellationToken ct)
    {
        Record(launch, "textDocument/diagnostic", p, 0);
        if (DiagnosticGate is { } gate)
        {
            await gate.Task.WaitAsync(ct);
        }

        return new
        {
            kind = "full",
            resultId = "r1",
            items = new[]
            {
                new { range = new { start = new { line = 0, character = 0 }, end = new { line = 0, character = 1 } }, severity = 2, code = "CS0168", message = "warming" },
            },
        };
    }

    private static void Add(JsonRpc rpc, string method, Delegate handler)
    {
        var single = handler.Method.GetParameters().Any(p => p.ParameterType == typeof(JsonElement));
        rpc.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = single });
    }

    private T Record<T>(int launch, string method, JsonElement p, T value)
    {
        lock (_lock)
        {
            _received.Add((launch, method, p.ValueKind == JsonValueKind.Undefined ? p : p.Clone()));
        }

        return value;
    }

    private sealed class Lifetime(FakeLanguageServer owner, JsonRpc rpc) : IAsyncDisposable
    {
        public ValueTask DisposeAsync()
        {
            owner.Disposed = true;
            rpc.Dispose();
            return ValueTask.CompletedTask;
        }
    }
}
