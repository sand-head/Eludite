using System.Globalization;
using System.Reflection;
using System.Runtime.InteropServices;
using Niello.Host.Lsp;
using Niello.Host.Sdk;
using StreamJsonRpc;

namespace Niello.Host.Rpc;

/// <summary>The JSON-RPC method surface of niello-host.</summary>
public sealed class HostRpcTarget
{
    public const string HostName = "niello-host";

    private readonly ISdkDiscoverer _sdkDiscoverer;
    private readonly TimeProvider _timeProvider;
    private readonly TextWriter _log;
    private readonly TaskCompletionSource _exitRequested = new(TaskCreationOptions.RunContinuationsAsynchronously);

    public HostRpcTarget(ISdkDiscoverer sdkDiscoverer, TextWriter log, TimeProvider? timeProvider = null, LspProxy? languageServer = null)
    {
        _sdkDiscoverer = sdkDiscoverer;
        _log = log;
        _timeProvider = timeProvider ?? TimeProvider.System;
        LanguageServer = languageServer;
    }

    /// <summary>The forwarded Roslyn language server, or null when none is configured.</summary>
    public LspProxy? LanguageServer { get; }

    public static string HostVersion { get; } =
        typeof(HostRpcTarget).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
        ?? "0.0.0";

    /// <summary>True once the client has sent <c>shutdown</c>.</summary>
    public bool ShutdownRequested { get; private set; }

    /// <summary>Completes when the client sends the <c>exit</c> notification.</summary>
    public Task ExitRequested => _exitRequested.Task;

    [JsonRpcMethod("initialize", UseSingleObjectParameterDeserialization = true)]
    public InitializeResult Initialize(InitializeParams parameters)
    {
        ArgumentNullException.ThrowIfNull(parameters);
        _log.WriteLine(
            $"initialize from {parameters.ClientName} {parameters.ClientVersion}" +
            (parameters.SolutionPath is null ? string.Empty : $" (solution: {parameters.SolutionPath})"));
        // Starts the language server in the background; initialize itself does not wait for it.
        LanguageServer?.Start(parameters.SolutionPath);
        return new InitializeResult(HostName, HostVersion, new HostCapabilities());
    }

    [JsonRpcMethod("niello/ping")]
    public PingResult Ping()
    {
        var timestamp = _timeProvider.GetUtcNow().UtcDateTime.ToString("O", CultureInfo.InvariantCulture);
        return new PingResult(true, timestamp);
    }

    [JsonRpcMethod("niello/host/info")]
    public async Task<HostInfoResult> GetHostInfoAsync(CancellationToken cancellationToken)
    {
        var sdks = await _sdkDiscoverer.DiscoverAsync(cancellationToken).ConfigureAwait(false);
        return new HostInfoResult(
            sdks.Select(s => new SdkInfo(s.Version, s.Path)).ToList(),
            RuntimeInformation.FrameworkDescription,
            RuntimeInformation.OSDescription);
    }

    [JsonRpcMethod("shutdown")]
    public void Shutdown()
    {
        _log.WriteLine("shutdown requested");
        ShutdownRequested = true;
    }

    [JsonRpcMethod("exit")]
    public void Exit()
    {
        _log.WriteLine("exit requested");
        _exitRequested.TrySetResult();
    }
}
