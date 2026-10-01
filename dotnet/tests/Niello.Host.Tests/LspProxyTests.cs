using System.Diagnostics;
using System.Text.Json;
using Nerdbank.Streams;
using Niello.Host.Lsp;
using Niello.Host.Rpc;
using StreamJsonRpc;
using StreamJsonRpc.Protocol;

namespace Niello.Host.Tests;

/// <summary>LSP forwarding through niello-host against an in-memory fake language server.</summary>
public sealed class LspProxyTests : IAsyncDisposable
{
    private readonly FakeLanguageServer _fake = new();
    private readonly LspProxy _proxy;
    private readonly HostRpcTarget _target;
    private readonly Task<int> _server;
    private readonly JsonRpc _client;

    public LspProxyTests()
    {
        _proxy = new LspProxy(_fake, TextWriter.Null);
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        _target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, languageServer: _proxy);
        _server = HostServer.RunAsync(serverStream, serverStream, _target);
        _client = HostServer.CreateConnection(clientStream, clientStream);
        _client.AddLocalRpcMethod("workspace/projectInitializationComplete", new Action(() => _projectsLoadedSeen.TrySetResult()));
        _client.StartListening();
    }

    private readonly TaskCompletionSource _projectsLoadedSeen = new(TaskCreationOptions.RunContinuationsAsynchronously);

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private Task InitializeAsync(string? solution = "/src/App.slnx") =>
        _client.InvokeWithParameterObjectAsync<JsonElement>("initialize", new { clientName = "t", clientVersion = "0", solutionPath = solution }, Ct);

    [Fact]
    public async Task Initialize_PerformsUpstreamHandshakeAndOpensSolution()
    {
        await InitializeAsync();

        var opened = await _fake.SolutionOpened.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        Assert.Equal(new Uri("/src/App.slnx").AbsoluteUri, opened);
        Assert.Equal(["initialize", "initialized", "solution/open"], _fake.Snapshot().Take(3));
    }

    [Fact]
    public async Task Completion_IsForwardedWithParamsIntactAndResultReturned()
    {
        await InitializeAsync();

        var result = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion",
            new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 3, character = 7 } },
            Ct);

        Assert.Equal("Compute", result.GetProperty("items")[0].GetProperty("label").GetString());
        var forwarded = _fake.LastCompletionParams!.Value;
        Assert.Equal("file:///a.cs", forwarded.GetProperty("textDocument").GetProperty("uri").GetString());
        Assert.Equal(7, forwarded.GetProperty("position").GetProperty("character").GetInt32());
    }

    [Fact]
    public async Task DidOpen_IsForwardedAsNotification()
    {
        await InitializeAsync();

        await _client.NotifyWithParameterObjectAsync("textDocument/didOpen", new { textDocument = new { uri = "file:///a.cs", languageId = "csharp", version = 1, text = "class A {}" } });

        var p = await _fake.DidOpen.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        Assert.Equal("class A {}", p.GetProperty("textDocument").GetProperty("text").GetString());
    }

    [Fact]
    public async Task ProjectInitializationComplete_IsRelayedAndAdvancesGeneration()
    {
        await InitializeAsync();
        await _fake.SolutionOpened.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        Assert.Equal(0, _proxy.Generation);

        await _fake.NotifyProjectsLoadedAsync();

        await _projectsLoadedSeen.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        await _proxy.ProjectsLoaded.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        Assert.Equal(1, _proxy.Generation);
    }

    [Fact]
    public async Task CanceledRequest_ReturnsRequestCancelledWithin50msAndCancelsUpstream()
    {
        await InitializeAsync();
        _fake.HangCompletion = true;
        using var cts = new CancellationTokenSource();

        var call = _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion", new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 0, character = 0 } }, cts.Token);
        await _fake.CompletionStarted.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);

        var sw = Stopwatch.StartNew();
        await cts.CancelAsync();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => call);
        sw.Stop();

        Assert.True(sw.ElapsedMilliseconds < 50, $"cancellation took {sw.ElapsedMilliseconds} ms");
        await _fake.CompletionCanceled.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
    }

    [Fact]
    public async Task StaleGeneration_IsRejectedWithContentModified()
    {
        await InitializeAsync();
        await _fake.NotifyProjectsLoadedAsync();
        await _proxy.ProjectsLoaded.WaitAsync(TimeSpan.FromSeconds(10), Ct);

        var ex = await Assert.ThrowsAsync<RemoteInvocationException>(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion",
            new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 0, character = 0 }, nielloGeneration = 0 },
            Ct));

        Assert.Equal(LspProxy.ContentModified, ex.ErrorCode);
        Assert.Null(_fake.LastCompletionParams);
    }

    [Fact]
    public async Task GenerationChangeWhileInFlight_CancelsUpstreamAndReturnsContentModified()
    {
        await InitializeAsync();
        _fake.HangCompletion = true;

        var call = _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/completion",
            new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 0, character = 0 }, nielloGeneration = 0 },
            Ct);
        await _fake.CompletionStarted.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
        Assert.False(_fake.LastCompletionParams!.Value.TryGetProperty(LspProxy.GenerationProperty, out _));

        _proxy.BumpGeneration();

        var ex = await Assert.ThrowsAsync<RemoteInvocationException>(() => call);
        Assert.Equal(LspProxy.ContentModified, ex.ErrorCode);
        await _fake.CompletionCanceled.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);
    }

    [Fact]
    public async Task NielloMethodsStillWorkAndUnknownMethodsAreNotForwarded()
    {
        await InitializeAsync();

        var ping = await _client.InvokeWithCancellationAsync<JsonElement>("niello/ping", [], Ct);
        Assert.True(ping.GetProperty("pong").GetBoolean());
        var ex = await Assert.ThrowsAsync<RemoteMethodNotFoundException>(
            () => _client.InvokeWithCancellationAsync<JsonElement>("textDocument/notAThing", [], Ct));
        Assert.Equal(JsonRpcErrorCode.MethodNotFound, ex.ErrorCode);
    }

    [Fact]
    public async Task ShutdownExit_ShutsDownUpstream()
    {
        await InitializeAsync();
        await _fake.SolutionOpened.Task.WaitAsync(TimeSpan.FromSeconds(10), Ct);

        await _client.InvokeWithCancellationAsync<JsonElement>("shutdown", [], Ct);
        await _client.NotifyAsync("exit");

        Assert.Equal(0, await _server.WaitAsync(TimeSpan.FromSeconds(10), Ct));
        Assert.Contains("shutdown", _fake.Snapshot());
        Assert.True(_fake.Disposed);
    }

    public async ValueTask DisposeAsync()
    {
        _client.Dispose();
        await _server.WaitAsync(TimeSpan.FromSeconds(10));
    }

    /// <summary>An in-memory LSP server standing in for Roslyn.</summary>
    private sealed class FakeLanguageServer : ILanguageServerLauncher, IAsyncDisposable
    {
        private JsonRpc? _rpc;

        private List<string> Received { get; } = [];

        public List<string> Snapshot()
        {
            lock (Received)
            {
                return [.. Received];
            }
        }

        public TaskCompletionSource<string> SolutionOpened { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

        public TaskCompletionSource<JsonElement> DidOpen { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

        public TaskCompletionSource CompletionStarted { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

        public TaskCompletionSource CompletionCanceled { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

        public JsonElement? LastCompletionParams { get; private set; }

        public bool HangCompletion { get; set; }

        public bool Disposed { get; private set; }

        public Task<LanguageServerConnection> LaunchAsync(CancellationToken cancellationToken)
        {
            var (host, server) = FullDuplexStream.CreatePair();
            _rpc = HostServer.CreateConnection(server, server);
            Add("initialize", new Func<JsonElement, object>(_ => Record("initialize", new { capabilities = new { } })));
            Add("initialized", new Action<JsonElement>(_ => Record("initialized", 0)));
            Add("solution/open", new Action<JsonElement>(p => SolutionOpened.TrySetResult(Record("solution/open", p.GetProperty("solution").GetString()!))));
            Add("textDocument/didOpen", new Action<JsonElement>(p => DidOpen.TrySetResult(p.Clone())));
            Add("textDocument/completion", new Func<JsonElement, CancellationToken, Task<object>>(CompletionAsync));
            Add("shutdown", new Func<object?>(() => Record<object?>("shutdown", null)));
            Add("exit", new Action(() => Record("exit", 0)));
            _rpc.StartListening();
            return Task.FromResult(new LanguageServerConnection(host, host, this));
        }

        public Task NotifyProjectsLoadedAsync() => _rpc!.NotifyAsync("workspace/projectInitializationComplete");

        private async Task<object> CompletionAsync(JsonElement p, CancellationToken ct)
        {
            LastCompletionParams = p.Clone();
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

        private void Add(string method, Delegate handler)
        {
            var single = handler.Method.GetParameters().Any(p => p.ParameterType == typeof(JsonElement));
            _rpc!.AddLocalRpcMethod(handler.Method, handler.Target, new JsonRpcMethodAttribute(method) { UseSingleObjectParameterDeserialization = single });
        }

        private T Record<T>(string method, T value)
        {
            lock (Received)
            {
                Received.Add(method);
            }

            return value;
        }

        public ValueTask DisposeAsync()
        {
            Disposed = true;
            _rpc?.Dispose();
            return ValueTask.CompletedTask;
        }
    }
}
