namespace Eludite.TestBridge;

/// <summary>Where a runner reports what it finds and runs. Called on the runner's threads, in order per runner.</summary>
public interface ITestSink
{
    /// <summary>Tests discovered, in the runner's order.</summary>
    void Tests(IReadOnlyList<DiscoveredTest> tests);

    /// <summary>Tests that started or ended.</summary>
    void Results(IReadOnlyList<TestResult> results);

    /// <summary>A line of the runner's log (MTP <c>client/log</c>, VSTest <c>TestSession.Message</c>, the process's own output).</summary>
    void Log(string line);

    /// <summary>A debug run: start this command line under the debug adapter; the application connects back.</summary>
    void Launch(TestLaunch launch);

    /// <summary>A debug run: attach the debug adapter to this waiting process; answers whether it attached.</summary>
    Task<(bool Attached, string? Message)> AttachAsync(int processId, CancellationToken cancellationToken);
}

/// <summary>
/// Discovers and runs one container's tests out of process (PLAN.md 4.6, D3): <see cref="MtpRunner"/> for
/// Microsoft.Testing.Platform's server mode, <see cref="VsTestRunner"/> for the VSTest translation-layer protocol.
/// Canceling the token stops the request (the protocol's cancel, then the process tree is killed after
/// <see cref="CancelGrace"/>) and the call throws <see cref="OperationCanceledException"/>; a runner that cannot start
/// or fails throws <see cref="TestRunnerException"/>.
/// </summary>
public interface ITestRunner
{
    /// <summary>How long a canceled runner has to stop before its process tree is killed.</summary>
    public static readonly TimeSpan CancelGrace = TimeSpan.FromSeconds(2);

    /// <summary>How long a debug run waits for the application to connect, or for the attach answer.</summary>
    public static readonly TimeSpan DebugWait = TimeSpan.FromSeconds(60);

    Task DiscoverAsync(TestContainer container, ITestSink sink, CancellationToken cancellationToken);

    /// <summary>Runs <paramref name="tests"/> (null: all of the container's), under the debugger when <paramref name="debug"/>.</summary>
    Task RunAsync(TestContainer container, IReadOnlyList<DiscoveredTest>? tests, bool debug, ITestSink sink, CancellationToken cancellationToken);
}
