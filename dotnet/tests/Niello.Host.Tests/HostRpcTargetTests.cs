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
        _client = HostServer.CreateConnection(clientStream, clientStream);
        _client.StartListening();
    }

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task Initialize_ReturnsHostIdentityAndEmptyCapabilities()
    {
        var result = await _client.InvokeWithParameterObjectAsync<JsonElement>(
            "initialize",
            new { clientName = "niello", clientVersion = "0.1.0", solutionPath = "/src/App.slnx" },
            Ct);

        Assert.Equal("niello-host", result.GetProperty("hostName").GetString());
        Assert.False(string.IsNullOrEmpty(result.GetProperty("hostVersion").GetString()));
        Assert.Equal(JsonValueKind.Object, result.GetProperty("capabilities").ValueKind);
        Assert.Empty(result.GetProperty("capabilities").EnumerateObject());
    }

    [Fact]
    public async Task Initialize_AcceptsMissingSolutionPath()
    {
        var result = await _client.InvokeWithParameterObjectAsync<InitializeResult>(
            "initialize", new { clientName = "niello", clientVersion = "0.1.0" }, Ct);

        Assert.Equal(HostRpcTarget.HostName, result.HostName);
        Assert.Equal(HostRpcTarget.HostVersion, result.HostVersion);
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
    public async Task ShutdownThenExit_ReturnsNullAndStopsServerWithExitCodeZero()
    {
        var result = await _client.InvokeWithCancellationAsync<JsonElement>("shutdown", [], Ct);
        Assert.Equal(JsonValueKind.Null, result.ValueKind);
        Assert.True(_target.ShutdownRequested);

        await _client.NotifyAsync("exit");

        Assert.Equal(0, await _server.WaitAsync(TimeSpan.FromSeconds(10), Ct));
    }

    [Fact]
    public async Task ExitWithoutShutdown_StopsServerWithExitCodeOne()
    {
        await _client.NotifyAsync("exit");

        Assert.Equal(1, await _server.WaitAsync(TimeSpan.FromSeconds(10), Ct));
    }

    [Fact]
    public async Task UnknownMethod_ReturnsMethodNotFound()
    {
        var ex = await Assert.ThrowsAsync<RemoteMethodNotFoundException>(
            () => _client.InvokeWithCancellationAsync<JsonElement>("niello/nope", [], Ct));

        Assert.Equal(JsonRpcErrorCode.MethodNotFound, ex.ErrorCode);
    }

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
