namespace Eludite.TestBridge;

/// <summary>What runs a test container's program.</summary>
public enum TestRuntime
{
    /// <summary>CoreCLR: the apphost, or <c>dotnet &lt;dll&gt;</c>.</summary>
    Dotnet,

    /// <summary>.NET Framework off Windows: <c>mono &lt;exe&gt;</c>.</summary>
    Mono,

    /// <summary>.NET Framework on Windows: the .exe.</summary>
    NetFx,
}

/// <summary>
/// One test container: a test project built for one target framework (host-rpc.md, "Tests", Containers).
/// </summary>
/// <param name="Id"><c>&lt;absolute project path&gt;|&lt;target framework&gt;</c>.</param>
/// <param name="Program">MTP: the test application (the DLL, or the .exe for .NET Framework). VSTest: the test assembly.</param>
public sealed record TestContainer(string Id, string Project, string TargetFramework, TestRunnerProtocol Protocol, string Program, TestRuntime Runtime)
{
    /// <summary>The <c>mono</c> to run a <see cref="TestRuntime.Mono"/> program with.</summary>
    public string? MonoExecutable { get; init; }

    /// <summary>The <c>dotnet</c> to run CoreCLR programs and vstest.console with; default <c>dotnet</c> on PATH.</summary>
    public string? DotnetExecutable { get; init; }

    /// <summary>A .runsettings file: VSTest gets its contents; MTP gets <c>--settings &lt;path&gt;</c>.</summary>
    public string? RunSettings { get; init; }

    /// <summary>The vstest.console.dll to run (VSTest only); default the newest SDK's.</summary>
    public string? VsTestConsole { get; init; }

    /// <summary>VSTest: run test assemblies side by side (<c>MaxCpuCount</c> 0).</summary>
    public bool Parallel { get; init; }

    /// <summary>Variables added to the runner's environment (a relocated Mono's, a test's own).</summary>
    public IReadOnlyDictionary<string, string>? Environment { get; init; }

    /// <summary>The project's name as the Test Explorer shows it.</summary>
    public string Name { get; init; } = Path.GetFileNameWithoutExtension(Project);
}

/// <summary>How to start a test application under a debug adapter (a debug run).</summary>
public sealed record TestLaunch(string Program, IReadOnlyList<string> Args, string Cwd, IReadOnlyDictionary<string, string> Env, TestRuntime Runtime);

/// <summary>A test runner could not start, crashed or answered with an error.</summary>
public sealed class TestRunnerException : Exception
{
    public TestRunnerException()
    {
    }

    public TestRunnerException(string message)
        : base(message)
    {
    }

    public TestRunnerException(string message, Exception inner)
        : base(message, inner)
    {
    }
}
