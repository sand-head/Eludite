using System.Globalization;
using System.Runtime.InteropServices;
using System.Text.Json;
using Nerdbank.Streams;
using Niello.Host.Rpc;
using Niello.Host.Sdk;
using StreamJsonRpc;
using StreamJsonRpc.Protocol;

namespace Niello.Host.Tests;

public sealed class HostRpcTargetTests : IAsyncDisposable
{
    private static readonly DateTimeOffset FixedNow = new(2026, 10, 1, 12, 34, 56, TimeSpan.Zero);

    private readonly FakeSdkDiscoverer _discoverer = new(
        new DotnetSdk("8.0.130", "/usr/share/dotnet/sdk"),
        new DotnetSdk("10.0.302", "/usr/share/dotnet/sdk"));

    private readonly HostRpcTarget _target;
    private readonly Task<int> _server;
    private readonly JsonRpc _client;

    public HostRpcTargetTests()
    {
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        _target = new HostRpcTarget(_discoverer, TextWriter.Null, new FixedTimeProvider(FixedNow));
        _server = HostServer.RunAsync(serverStream, serverStream, _target);
        _client = TestRpc.Create(clientStream);
        var onStatus = new Action<JsonElement>(p => _solutionStatuses.Writer.TryWrite(p.Clone()));
        _client.AddLocalRpcMethod(onStatus.Method, onStatus.Target, new JsonRpcMethodAttribute("niello/solution/status") { UseSingleObjectParameterDeserialization = true });
        _client.StartListening();
    }

    private readonly System.Threading.Channels.Channel<JsonElement> _solutionStatuses = System.Threading.Channels.Channel.CreateUnbounded<JsonElement>();

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task HostInitialize_ReturnsHostIdentityAndCapabilities()
    {
        var result = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "niello/host/initialize",
            new { clientName = "niello", clientVersion = "0.1.0" },
            Ct);

        Assert.Equal("niello-host", result.GetProperty("hostName").GetString());
        Assert.False(string.IsNullOrEmpty(result.GetProperty("hostVersion").GetString()));
        Assert.Equal(JsonValueKind.Object, result.GetProperty("capabilities").ValueKind);
        Assert.False(result.GetProperty("capabilities").GetProperty("languageServer").GetBoolean());
    }

    [Fact]
    public async Task HostInitialize_IgnoresUnknownMembers()
    {
        var result = await _client.InvokeWithParameterObjectAsync<InitializeResult>(
            "niello/host/initialize", new { clientName = "niello", clientVersion = "0.1.0", solutionPath = "/src/App.slnx" }, Ct);

        Assert.Equal(HostRpcTarget.HostName, result.HostName);
        Assert.Equal(HostRpcTarget.HostVersion, result.HostVersion);
        Assert.True(_target.Initialized);
    }

    [Fact]
    public async Task Ping_ReturnsPongWithIso8601UtcTimestamp()
    {
        var result = await _client.InvokeWithCancellationAsync<JsonElement>("niello/ping", [], Ct);

        Assert.True(result.GetProperty("pong").GetBoolean());
        var timestamp = result.GetProperty("timestamp").GetString();
        Assert.Equal("2026-10-01T12:34:56.0000000Z", timestamp);
        var parsed = DateTimeOffset.Parse(timestamp!, CultureInfo.InvariantCulture);
        Assert.Equal(FixedNow, parsed);
    }

    [Fact]
    public async Task HostInfo_ReportsSdksFromDiscovererAndRuntime()
    {
        var result = await _client.InvokeWithCancellationAsync<JsonElement>("niello/host/info", [], Ct);

        var sdks = result.GetProperty("dotnetSdks").EnumerateArray().ToList();
        Assert.Equal(2, sdks.Count);
        Assert.Equal("10.0.302", sdks[1].GetProperty("version").GetString());
        Assert.Equal("/usr/share/dotnet/sdk", sdks[1].GetProperty("path").GetString());
        Assert.Equal(RuntimeInformation.FrameworkDescription, result.GetProperty("runtime").GetString());
        Assert.Equal(RuntimeInformation.OSDescription, result.GetProperty("os").GetString());
        Assert.Equal(1, _discoverer.Calls);
    }

    [Fact]
    public async Task HostShutdownThenExit_ReturnsNullAndStopsServerWithExitCodeZero()
    {
        var result = await _client.InvokeWithCancellationAsync<JsonElement>("niello/host/shutdown", [], Ct);
        Assert.Equal(JsonValueKind.Null, result.ValueKind);
        Assert.True(_target.ShutdownRequested);

        await _client.NotifyAsync("niello/host/exit");

        Assert.Equal(0, await _server.WaitAsync(TimeSpan.FromSeconds(10), Ct));
    }

    [Fact]
    public async Task ExitWithoutShutdown_StopsServerWithExitCodeOne()
    {
        await _client.NotifyAsync("niello/host/exit");

        Assert.Equal(1, await _server.WaitAsync(TimeSpan.FromSeconds(10), Ct));
    }

    [Fact]
    public async Task UnknownMethod_ReturnsMethodNotFound()
    {
        var ex = await Assert.ThrowsAsync<RemoteMethodNotFoundException>(
            () => _client.InvokeWithCancellationAsync<JsonElement>("niello/nope", [], Ct));

        Assert.Equal(JsonRpcErrorCode.MethodNotFound, ex.ErrorCode);
    }

    [Theory]
    [InlineData("initialize")]
    [InlineData("shutdown")]
    public async Task PlainLspLifecycleNames_AreNotHostMethods(string method)
    {
        var ex = await Assert.ThrowsAsync<RemoteMethodNotFoundException>(
            () => _client.InvokeWithParameterObjectAsync<JsonElement>(method, new { clientName = "niello", clientVersion = "0.1.0" }, Ct));

        Assert.Equal(JsonRpcErrorCode.MethodNotFound, ex.ErrorCode);
        Assert.False(_target.Initialized);
    }

    [Fact]
    public async Task SolutionOpen_BeforeInitialize_IsServerNotInitialized()
    {
        var path = Path.GetTempFileName();
        try
        {
            var ex = await Assert.ThrowsAsync<RemoteInvocationException>(
                () => _client.InvokeWithParameterObjectAsync<JsonElement>("niello/solution/open", new { path }, Ct));
            Assert.Equal(HostErrors.ServerNotInitialized, ex.ErrorCode);
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Theory]
    [InlineData("/definitely/not/here/App.sln")]
    [InlineData("")]
    public async Task SolutionOpen_WithBadPath_IsInvalidParams(string path)
    {
        await InitializeAsync();

        var (code, _) = await TestRpc.ErrorOfAsync(
            () => _client.InvokeWithParameterObjectAsync<JsonElement>("niello/solution/open", new { path }, Ct));

        Assert.Equal(HostErrors.InvalidParams, code);
    }

    [Fact]
    public async Task SolutionOpen_WithoutLanguageServer_ReturnsNewGenerationThenFailedStatus()
    {
        await InitializeAsync();
        var dir = Directory.CreateTempSubdirectory("niello-0007-");
        try
        {
            var sln = Path.Combine(dir.FullName, "App.slnx");
            await File.WriteAllTextAsync(sln, "<Solution />", Ct);

            var opened = await _client.InvokeWithParameterObjectAsync<JsonElement>("niello/solution/open", new { path = sln }, Ct);
            Assert.Equal(1, opened.GetProperty("generation").GetInt64());

            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(Ct);
            timeout.CancelAfter(TimeSpan.FromSeconds(10));
            var loading = await _solutionStatuses.Reader.ReadAsync(timeout.Token);
            Assert.Equal("loading", loading.GetProperty("state").GetString());
            var status = await _solutionStatuses.Reader.ReadAsync(timeout.Token);
            Assert.Equal("failed", status.GetProperty("state").GetString());
            Assert.Equal(1, status.GetProperty("generation").GetInt64());
            Assert.Equal(sln, status.GetProperty("path").GetString());
            Assert.Equal("NIELLO0001", status.GetProperty("diagnostics")[0].GetProperty("code").GetString());
            Assert.Equal("error", status.GetProperty("diagnostics")[0].GetProperty("severity").GetString());
            var closed = await _client.InvokeWithCancellationAsync<JsonElement>("niello/solution/close", [], Ct);
            Assert.Equal(2, closed.GetProperty("generation").GetInt64());
            var closedStatus = await _solutionStatuses.Reader.ReadAsync(timeout.Token);
            Assert.Equal("closed", closedStatus.GetProperty("state").GetString());
            Assert.Equal(2, closedStatus.GetProperty("generation").GetInt64());
            var again = await _client.InvokeWithCancellationAsync<JsonElement>("niello/solution/close", [], Ct);
            Assert.Equal(2, again.GetProperty("generation").GetInt64());
        }
        finally
        {
            dir.Delete(recursive: true);
        }
    }

    [Fact]
    public async Task ForwardedRequest_WithoutLanguageServer_IsRequestFailed()
    {
        await InitializeAsync();

        var ex = await Assert.ThrowsAsync<RemoteInvocationException>(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "textDocument/hover",
            new { textDocument = new { uri = "file:///a.cs" }, position = new { line = 0, character = 0 }, nielloGeneration = 0 },
            Ct));

        Assert.Equal(HostErrors.RequestFailed, ex.ErrorCode);
        Assert.Equal("languageServerUnavailable", ((JsonElement)ex.ErrorData!).GetProperty("reason").GetString());
        Assert.Equal(0, _discoverer.Calls);
    }

    [Fact]
    public async Task ForwardedRequest_BeforeInitialize_IsServerNotInitialized()
    {
        var ex = await Assert.ThrowsAsync<RemoteInvocationException>(() => _client.InvokeWithParameterObjectAsync<JsonElement>(
            "workspace/symbol", new { query = "W", nielloGeneration = 0 }, Ct));

        Assert.Equal(HostErrors.ServerNotInitialized, ex.ErrorCode);
    }

    private Task InitializeAsync() =>
        _client.InvokeWithParameterObjectAsync<JsonElement>("niello/host/initialize", new { clientName = "t", clientVersion = "0" }, Ct);

    public async ValueTask DisposeAsync()
    {
        _client.Dispose();
        await _server.WaitAsync(TimeSpan.FromSeconds(10));
    }

    private sealed class FixedTimeProvider(DateTimeOffset now) : TimeProvider
    {
        public override DateTimeOffset GetUtcNow() => now;
    }
}
