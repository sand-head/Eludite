using System.Globalization;
using System.Reflection;
using System.Runtime.InteropServices;
using Eludite.Host.Build;
using Eludite.Host.Lsp;
using Eludite.Host.NuGet;
using Eludite.Host.Projects;
using Eludite.Host.Sdk;
using Eludite.Host.Testing;
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
    /// <param name="tree">Answers <c>eludite/solution/tree</c>; when null, one backed by the in-process MSBuild evaluator.</param>
    /// <param name="build">Runs <c>eludite/build/*</c>; when null, one that locates the MSBuilds on its first build.</param>
    /// <param name="tests">Runs <c>eludite/test/*</c>; when null, one with the MTP and VSTest runners.</param>
    /// <param name="nuget">Runs <c>eludite/nuget/*</c>; when null, one on NuGet's own configuration.</param>
    public HostRpcTarget(ISdkDiscoverer sdkDiscoverer, TextWriter log, TimeProvider? timeProvider = null, LspProxy? languageServer = null, SolutionTreeProvider? tree = null, BuildService? build = null, TestService? tests = null, NuGetService? nuget = null)
    {
        _sdkDiscoverer = sdkDiscoverer;
        _log = log;
        _timeProvider = timeProvider ?? TimeProvider.System;
        LanguageServer = languageServer ?? new LspProxy(null, log);
        Tree = tree ?? new SolutionTreeProvider(new MsBuildProjectTreeEvaluator(), log);
        Build = build ?? new BuildService(LanguageServer.CurrentSolution, log);
        Tests = tests ?? new TestService(LanguageServer.CurrentSolution, log);
        NuGet = nuget ?? new NuGetService(LanguageServer.CurrentSolution, LanguageServer.AdvanceGeneration, log);
    }

    /// <summary>The <c>eludite/nuget/*</c> service (brief 0048).</summary>
    public NuGetService NuGet { get; }

    /// <summary>The <c>eludite/test/*</c> service (brief 0035).</summary>
    public TestService Tests { get; }

    /// <summary>The <c>eludite/build/*</c> service (brief 0017).</summary>
    public BuildService Build { get; }

    /// <summary>The <c>eludite/solution/tree</c> provider.</summary>
    public SolutionTreeProvider Tree { get; }

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

    [JsonRpcMethod("eludite/solution/tree")]
    public Task<SolutionTree> GetSolutionTreeAsync(CancellationToken cancellationToken)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        var (generation, path, changed) = LanguageServer.CurrentSolution();
        return Tree.GetAsync(generation, path, changed, () => LanguageServer.Generation, cancellationToken);
    }

    [JsonRpcMethod("eludite/build/start", UseSingleObjectParameterDeserialization = true)]
    public BuildStartResult StartBuild(BuildStartParams parameters)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return Build.Start(parameters);
    }

    [JsonRpcMethod("eludite/build/cancel", UseSingleObjectParameterDeserialization = true)]
    public BuildCancelResult CancelBuild(BuildCancelParams? parameters = null)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return Build.Cancel(parameters);
    }

    /// <summary>Allowed before <c>eludite/host/initialize</c>: no build can run before it, so the answer is empty.</summary>
    [JsonRpcMethod("eludite/build/status", UseSingleObjectParameterDeserialization = true)]
    public BuildStatusResult BuildStatus(object? parameters = null) => Build.Status();

    [JsonRpcMethod("eludite/test/discover", UseSingleObjectParameterDeserialization = true)]
    public TestDiscoverResult DiscoverTests(TestDiscoverParams? parameters = null)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return Tests.Discover(parameters);
    }

    [JsonRpcMethod("eludite/test/run", UseSingleObjectParameterDeserialization = true)]
    public TestRunResult RunTests(TestRunParams? parameters = null)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return Tests.Run(parameters);
    }

    [JsonRpcMethod("eludite/test/cancel", UseSingleObjectParameterDeserialization = true)]
    public TestCancelResult CancelTests(TestCancelParams? parameters = null)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return Tests.Cancel(parameters);
    }

    [JsonRpcMethod("eludite/test/attached", UseSingleObjectParameterDeserialization = true)]
    public TestAttachedResult TestAttached(TestAttachedParams parameters)
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }

        return Tests.Attached(parameters);
    }

    /// <summary>Allowed before <c>eludite/host/initialize</c>: nothing can run before it, so the answer is empty.</summary>
    [JsonRpcMethod("eludite/test/status", UseSingleObjectParameterDeserialization = true)]
    public TestStatusResult TestStatus(object? parameters = null) => Tests.Status();

    [JsonRpcMethod("eludite/nuget/search", UseSingleObjectParameterDeserialization = true)]
    public Task<NuGetSearchResult> NuGetSearchAsync(NuGetSearchParams? parameters, CancellationToken cancellationToken)
    {
        RequireInitialized();
        return NuGet.SearchAsync(parameters, cancellationToken);
    }

    [JsonRpcMethod("eludite/nuget/installed", UseSingleObjectParameterDeserialization = true)]
    public Task<NuGetInstalledResult> NuGetInstalledAsync(NuGetInstalledParams? parameters, CancellationToken cancellationToken)
    {
        RequireInitialized();
        return NuGet.InstalledAsync(parameters, cancellationToken);
    }

    [JsonRpcMethod("eludite/nuget/updates", UseSingleObjectParameterDeserialization = true)]
    public Task<NuGetUpdatesResult> NuGetUpdatesAsync(NuGetUpdatesParams? parameters, CancellationToken cancellationToken)
    {
        RequireInitialized();
        return NuGet.UpdatesAsync(parameters, cancellationToken);
    }

    [JsonRpcMethod("eludite/nuget/change", UseSingleObjectParameterDeserialization = true)]
    public Task<NuGetChangeResult> NuGetChangeAsync(NuGetChangeParams? parameters, CancellationToken cancellationToken)
    {
        RequireInitialized();
        return NuGet.ChangeAsync(parameters, cancellationToken);
    }

    [JsonRpcMethod("eludite/nuget/sources", UseSingleObjectParameterDeserialization = true)]
    public Task<NuGetSourcesResult> NuGetSourcesAsync(NuGetSourcesParams? parameters, CancellationToken cancellationToken)
    {
        RequireInitialized();
        return NuGet.SourcesAsync(parameters, cancellationToken);
    }

    [JsonRpcMethod("eludite/nuget/restore", UseSingleObjectParameterDeserialization = true)]
    public Task<NuGetRestoreResult> NuGetRestoreAsync(NuGetRestoreParams? parameters, CancellationToken cancellationToken)
    {
        RequireInitialized();
        return NuGet.RestoreAsync(parameters, cancellationToken);
    }

    [JsonRpcMethod("eludite/nuget/icon", UseSingleObjectParameterDeserialization = true)]
    public Task<NuGetIconResult> NuGetIconAsync(NuGetIconParams? parameters, CancellationToken cancellationToken)
    {
        RequireInitialized();
        return NuGet.IconAsync(parameters, cancellationToken);
    }

    [JsonRpcMethod("eludite/host/shutdown")]
    public void Shutdown()
    {
        _log.WriteLine("eludite/host/shutdown requested");
        ShutdownRequested = true;
    }

    private void RequireInitialized()
    {
        if (!Initialized)
        {
            throw HostErrors.NotInitialized();
        }
    }

    [JsonRpcMethod("eludite/host/exit")]
    public void Exit()
    {
        _log.WriteLine("eludite/host/exit requested");
        _exitRequested.TrySetResult();
    }
}
