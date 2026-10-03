using Eludite.TestBridge;

namespace Eludite.Host.Testing;

// Wire shapes of eludite/test/* (protocol/schemas/host/test-*.json; host-rpc.md, "Tests"). camelCase, nulls omitted.
// The discovered test (TestItem) and a result (TestResult) are the bridge's model records, serialized as they are.

/// <summary><c>eludite/test/discover</c> params.</summary>
public sealed record TestDiscoverParams
{
    public IReadOnlyList<string>? Projects { get; init; }

    public string? Configuration { get; init; }

    public string? RunSettings { get; init; }

    public string? VstestConsolePath { get; init; }
}

/// <summary>A container of <c>eludite/test/discover</c>'s result and <c>eludite/test/status</c>.</summary>
public sealed record TestContainerInfo(string Id, string Name, string Project, string TargetFramework, string Protocol)
{
    /// <summary><c>dotnet</c>, <c>mono</c> or <c>netfx</c>.</summary>
    public string? Runtime { get; init; }

    public string? Program { get; init; }

    public string? Error { get; init; }
}

/// <summary><c>eludite/test/discover</c> result.</summary>
public sealed record TestDiscoverResult(long RunId, long Generation, IReadOnlyList<TestContainerInfo> Containers);

/// <summary>A container of <c>eludite/test/run</c>: its id and, optionally, which of its tests.</summary>
public sealed record TestRunContainer(string Id)
{
    public IReadOnlyList<string>? Tests { get; init; }
}

/// <summary><c>eludite/test/run</c> params.</summary>
public sealed record TestRunParams
{
    public IReadOnlyList<TestRunContainer>? Containers { get; init; }

    public bool? Debug { get; init; }

    public bool? Parallel { get; init; }

    public string? Configuration { get; init; }

    public string? RunSettings { get; init; }

    public string? VstestConsolePath { get; init; }
}

/// <summary><c>eludite/test/run</c> result.</summary>
public sealed record TestRunResult(long RunId, long Generation, IReadOnlyList<string> Containers)
{
    public bool? Debug { get; init; }
}

public sealed record TestCancelParams(long? RunId);

public sealed record TestCancelResult(bool Canceled)
{
    public long? RunId { get; init; }
}

public sealed record TestAttachedParams(long RunId, int ProcessId, bool Attached)
{
    public string? Message { get; init; }
}

public sealed record TestAttachedResult(bool Accepted);

/// <summary>Counts over every test a discovery or run covered.</summary>
public sealed record TestSummary(int Total, int Passed, int Failed, int Skipped, int NotRun);

/// <summary>A debug run's launch (<c>launch</c> update).</summary>
public sealed record TestLaunchInfo(string Program, IReadOnlyList<string> Args, string Cwd, IReadOnlyDictionary<string, string> Env)
{
    public string? Runtime { get; init; }
}

/// <summary><c>eludite/test/update</c> params: <see cref="Kind"/> says which members are present.</summary>
public sealed record TestUpdateParams(long RunId, long Generation, long Seq, string Kind)
{
    public string? Container { get; init; }

    public IReadOnlyList<TestItem>? Tests { get; init; }

    public IReadOnlyList<TestResult>? Results { get; init; }

    public string? Text { get; init; }

    public TestLaunchInfo? Launch { get; init; }

    public int? ProcessId { get; init; }

    public string? State { get; init; }

    public int? Count { get; init; }

    public TestSummary? Summary { get; init; }

    public double? ElapsedMs { get; init; }

    public string? Message { get; init; }
}

/// <summary>A test of a running discovery, with its container (<c>eludite/test/status</c>).</summary>
public sealed record TestStatusTest(string Container, TestItem Test);

/// <summary>A result of a running run with its container (<c>eludite/test/status</c>).</summary>
public sealed record TestStatusResultItem(string Id, string Container, string Outcome)
{
    public double? DurationMs { get; init; }

    public string? Message { get; init; }

    public string? StackTrace { get; init; }

    public string? Output { get; init; }

    public string? DisplayName { get; init; }

    public string? FullyQualifiedName { get; init; }
}

public sealed record TestStatusRunning(long RunId, string Kind, long Generation, IReadOnlyList<TestContainerInfo> Containers, double ElapsedMs, long NextSeq, IReadOnlyList<TestStatusTest> Tests, IReadOnlyList<TestStatusResultItem> Results)
{
    public bool? Debug { get; init; }
}

public sealed record TestStatusLast(long RunId, string Kind, long Generation, string State, TestSummary Summary, double ElapsedMs);

/// <summary><c>eludite/test/status</c> result.</summary>
public sealed record TestStatusResult(IReadOnlyList<TestStatusRunning> Running)
{
    public TestStatusLast? Last { get; init; }
}

/// <summary>The update kinds and states (host-rpc.md, "Tests").</summary>
public static class TestUpdateKinds
{
    public const string Discovered = "discovered";
    public const string Results = "results";
    public const string Output = "output";
    public const string Launch = "launch";
    public const string Attach = "attach";
    public const string ContainerFinished = "containerFinished";
    public const string Finished = "finished";

    public const string Completed = "completed";
    public const string Failed = "failed";
    public const string Canceled = "canceled";
}
