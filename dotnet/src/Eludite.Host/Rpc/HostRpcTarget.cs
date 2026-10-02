using System.Globalization;
using System.Reflection;
using System.Runtime.InteropServices;
using Eludite.Host.Lsp;
using Eludite.Host.Sdk;
using StreamJsonRpc;

namespace Eludite.Host.Rpc;

/// <summary>
/// The Eludite method surface of eludite-host (protocol/schemas/host-rpc.md, "Eludite methods"). Forwarded LSP methods
/// are registered by <see cref="LspProxy.Attach"/>.
/// </summary>
public sealed class HostRpcTarget
{
    public const string HostName = "eludite-host";

    private readonly ISdkDiscoverer _sdkDiscoverer;
    private readonly TimeProvider _timeProvider;
    private readonly TextWriter _log;
    private readonly TaskCompletionSource _exitRequested = new(TaskCreationOptions.RunContinuationsAsynchronously);

    /// <param name="languageServer">The LSP bridge; when null, one without a language server is created, so
    /// <c>eludite/solution/*</c> and forwarded requests answer with documented failures instead of MethodNotFound.</param>
    public HostRpcTarget(ISdkDiscoverer sdkDiscoverer, TextWriter log, TimeProvider? timeProvider = null, LspProxy? languageServer = null)
    {
        _sdkDiscoverer = sdkDiscoverer;
        _log = log;
        _timeProvider = timeProvider ?? TimeProvider.System;
        LanguageServer = languageServer ?? new LspProxy(null, log);
    }

    /// <summary>The LSP bridge.</summary>
    public LspProxy LanguageServer { get; }

    public static string HostVersion { get; } =
        typeof(HostRpcTarget).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
        ?? "0.0.0";

    /// <summary>True once the client has sent <c>eludite/host/initialize</c>.</summary>
    public bool Initialized { get; private set; }

    /// <summary>True once the client has sent <c>eludite/host/shutdown</c>.</summary>
    public bool ShutdownRequested { get; private set; }

    /// <summary>Completes when the client sends the <c>eludite/host/exit</c> notification.</summary>
    public Task ExitRequested => _exitRequested.Task;

    [JsonRpcMethod("eludite/host/initialize", UseSingleObjectParameterDeserialization = true)]
    public InitializeResult Initialize(InitializeParams parameters)
    {
        ArgumentNullException.ThrowIfNull(parameters);
        _log.WriteLine($"eludite/host/initialize from {parameters.ClientName} {parameters.ClientVersion}");
        Initialized = true;
        // Starts the language server in the background; initialize itself does not wait for it.
        LanguageServer.Start();
        return new InitializeResult(HostName, HostVersion, new HostCapabilities(LanguageServer.IsConfigured));
    }

    [JsonRpcMethod("eludite/ping")]
    public PingResult Ping()
    {
        var timestamp = _timeProvider.GetUtcNow().UtcDateTime.ToString("O", CultureInfo.InvariantCulture);
        return new PingResult(true, timestamp);
    }

    [JsonRpcMethod("eludite/host/info")]
    public async Task<HostInfoResult> GetHostInfoAsync(CancellationToken cancellationToken)
    {
        var sdks = await _sdkDiscoverer.DiscoverAsync(cancellationToken).ConfigureAwait(false);
        return new HostInfoResult(
            sdks.Select(s => new SdkInfo(s.Version, s.Path)).ToList(),
            RuntimeInformation.FrameworkDescription,
            RuntimeInformation.OSDescription);
    }

    [JsonRpcMethod("eludite/solution/open", UseSingleObjectParameterDeserialization = true)]
    public GenerationResult OpenSolution(SolutionOpenParams parameters)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return new GenerationResult(LanguageServer.OpenSolution(parameters?.Path));
    }

    [JsonRpcMethod("eludite/solution/close")]
    public GenerationResult CloseSolution()
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return new GenerationResult(LanguageServer.CloseSolution());
    }

    [JsonRpcMethod("eludite/host/shutdown")]
    public void Shutdown()
    {
        _log.WriteLine("eludite/host/shutdown requested");
        ShutdownRequested = true;
    }

    [JsonRpcMethod("eludite/host/exit")]
    public void Exit()
    {
        _log.WriteLine("eludite/host/exit requested");
        _exitRequested.TrySetResult();
    }
}
