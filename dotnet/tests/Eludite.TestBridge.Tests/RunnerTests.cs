using System.Diagnostics;

namespace Eludite.TestBridge.Tests;

/// <summary>
/// Brief 0035: the runners against the corpus built in a temp folder: Microsoft.Testing.Platform's server mode
/// (xunit.v3, MSTest; xunit.v3 for net472 under Mono when it is installed) and VSTest's translation layer (xunit 2,
/// NUnit): discovery, runs with outcomes, messages, stack traces, output and durations, cancel mid-run, and the debug
/// hand-offs (MTP's launch, VSTest's attach).
/// </summary>
public sealed class RunnerTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private static readonly TimeSpan Limit = TimeSpan.FromMinutes(2);

    private static async Task<TestContainer> ContainerAsync(string name, TestRunnerProtocol protocol, string tfm = "net10.0", IReadOnlyDictionary<string, string>? env = null)
    {
        var root = await Corpus.DirectoryAsync();
        var netfx = TestProjectInspector.IsNetFramework(tfm);
        var program = Corpus.Output(root, name, tfm, netfx ? ".exe" : ".dll");
        return new TestContainer($"{Corpus.Project(root, name)}|{tfm}", Corpus.Project(root, name), tfm, protocol, program, netfx ? TestRuntime.Mono : TestRuntime.Dotnet)
        {
            DotnetExecutable = Corpus.Dotnet(),
            MonoExecutable = Mono(),
            Environment = env,
        };
    }

    private static string? Mono() =>
        new[] { "/usr/bin/mono", "/usr/local/bin/mono" }.FirstOrDefault(File.Exists);

    private static ITestRunner Runner(TestRunnerProtocol p) => p == TestRunnerProtocol.MicrosoftTestingPlatform ? new MtpRunner() : new VsTestRunner();

    private static async Task<(RecordingSink Discovery, RecordingSink Run)> DiscoverAndRunAsync(TestContainer container)
    {
        var runner = Runner(container.Protocol);
        var discovery = new RecordingSink();
        await runner.DiscoverAsync(container, discovery, Ct).WaitAsync(Limit, Ct);
        var run = new RecordingSink();
        await runner.RunAsync(container, discovery.Found, debug: false, run, Ct).WaitAsync(Limit, Ct);
        return (discovery, run);
    }

    private static TestResult ByMethod(RecordingSink discovery, RecordingSink run, string method)
    {
        var id = Assert.Single(discovery.Found, t => t.Item.Method == method).Item.Id;
        return run.Final[id];
    }

    [Fact]
    public async Task Mtp_XunitV3_DiscoversAndRuns_WithOutcomesOutputAndLocations()
    {
        var container = await ContainerAsync("Corpus.XunitV3", TestRunnerProtocol.MicrosoftTestingPlatform);
        var (discovery, run) = await DiscoverAndRunAsync(container);

        Assert.Equal(7, discovery.Found.Count);
        var adds = Assert.Single(discovery.Found, t => t.Item.Method == "Adds").Item;
        Assert.Equal("Corpus.XunitV3.CalculatorTests.Adds", adds.FullyQualifiedName);
        Assert.Equal("Corpus.XunitV3", adds.Namespace);
        Assert.Equal("CalculatorTests", adds.ClassName);
        Assert.EndsWith("CalculatorTests.cs", adds.Source, StringComparison.Ordinal);
        Assert.Equal(14, adds.Line);
        var rows = discovery.Found.Where(t => t.Item.Method == "AddsPairs").Select(t => t.Item).ToList();
        Assert.Equal(2, rows.Count);
        Assert.All(rows, r => Assert.Equal("Corpus.XunitV3.CalculatorTests.AddsPairs", r.FullyQualifiedName));
        Assert.Contains(rows, r => r.DisplayName.Contains("a: 1, b: 1, sum: 2", StringComparison.Ordinal));
        Assert.All(rows, r => Assert.Equal([new TestTrait("Category", "Math")], r.Traits));

        Assert.Equal(7, run.Final.Count);
        Assert.Equal(TestOutcomes.Passed, ByMethod(discovery, run, "Adds").Outcome);
        var failed = ByMethod(discovery, run, "Subtracts");
        Assert.Equal(TestOutcomes.Failed, failed.Outcome);
        Assert.Contains("Assert.Equal() Failure", failed.Message, StringComparison.Ordinal);
        Assert.Contains("CalculatorTests.cs:line 25", failed.StackTrace, StringComparison.Ordinal);
        var skipped = ByMethod(discovery, run, "Divides");
        Assert.Equal(TestOutcomes.Skipped, skipped.Outcome);
        Assert.Equal("Division is not written yet", skipped.Message);
        Assert.Contains("Hello from xunit.v3", ByMethod(discovery, run, "WritesOutput").Output, StringComparison.Ordinal);
        Assert.All(run.Final.Values, r => Assert.True(r.DurationMs >= 0));
    }

    [Fact]
    public async Task Mtp_MSTest_DiscoversAndRuns_CategoriesAsTraits()
    {
        var container = await ContainerAsync("Corpus.MSTest", TestRunnerProtocol.MicrosoftTestingPlatform);
        var (discovery, run) = await DiscoverAndRunAsync(container);

        Assert.Equal(6, discovery.Found.Count);
        var rows = discovery.Found.Where(t => t.Item.Method == "ParsesRows").ToList();
        Assert.Equal(2, rows.Count);
        Assert.All(rows, r => Assert.Equal([new TestTrait("Category", "Rows")], r.Item.Traits));
        Assert.Equal("Corpus.MSTest.ParserTests.ParsesNumbers", Assert.Single(discovery.Found, t => t.Item.Method == "ParsesNumbers").Item.FullyQualifiedName);

        Assert.Equal(TestOutcomes.Passed, ByMethod(discovery, run, "ParsesNumbers").Outcome);
        var failed = ByMethod(discovery, run, "ParsesNegatives");
        Assert.Equal(TestOutcomes.Failed, failed.Outcome);
        Assert.Contains("ParserTests.cs:line 21", failed.StackTrace, StringComparison.Ordinal);
        Assert.Equal("Hexadecimal comes later", ByMethod(discovery, run, "ParsesHex").Message);
        var output = ByMethod(discovery, run, "WritesOutput").Output;
        Assert.Contains("Hello from MSTest", output, StringComparison.Ordinal);
        Assert.Contains("Console from MSTest", output, StringComparison.Ordinal);
    }

    [Fact]
    public async Task VsTest_Xunit2_DiscoversAndRuns()
    {
        var container = await ContainerAsync("Corpus.Xunit2", TestRunnerProtocol.VsTest);
        var (discovery, run) = await DiscoverAndRunAsync(container);

        Assert.Equal(5, discovery.Found.Count);
        var greets = Assert.Single(discovery.Found, t => t.Item.Method == "Greets").Item;
        Assert.Equal("Corpus.Xunit2.GreeterTests.Greets", greets.FullyQualifiedName);
        Assert.EndsWith("GreeterTests.cs", greets.Source, StringComparison.Ordinal);
        Assert.Equal(17, greets.Line);
        Assert.Equal([new TestTrait("Category", "Slow")], Assert.Single(discovery.Found, t => t.Item.Method == "Waits").Item.Traits);
        Assert.Contains("vstest.console speaks protocol version 7", discovery.LogText, StringComparison.Ordinal);

        Assert.Equal(5, run.Final.Count);
        Assert.Equal(TestOutcomes.Passed, ByMethod(discovery, run, "Greets").Outcome);
        var failed = ByMethod(discovery, run, "GreetsNobody");
        Assert.Equal(TestOutcomes.Failed, failed.Outcome);
        Assert.Contains("Hello, stranger!", failed.Message, StringComparison.Ordinal);
        Assert.Contains("GreeterTests.cs:line 26", failed.StackTrace, StringComparison.Ordinal);
        var skipped = ByMethod(discovery, run, "SaysGoodbye");
        Assert.Equal(TestOutcomes.Skipped, skipped.Outcome);
        Assert.Equal("Farewells come later", skipped.Message);
        Assert.Contains("Hello from xunit 2", ByMethod(discovery, run, "WritesOutput").Output, StringComparison.Ordinal);
        Assert.All(run.Final.Values, r => Assert.True(r.DurationMs >= 0));
    }

    [Fact]
    public async Task VsTest_NUnit_DiscoversAndRuns_ASelection()
    {
        var container = await ContainerAsync("Corpus.NUnit", TestRunnerProtocol.VsTest);
        var runner = new VsTestRunner();
        var discovery = new RecordingSink();
        await runner.DiscoverAsync(container, discovery, Ct).WaitAsync(Limit, Ct);

        Assert.Equal(6, discovery.Found.Count);
        var pairs = discovery.Found.Where(t => t.Item.Method == "PushesPairs").ToList();
        Assert.Equal(2, pairs.Count);
        Assert.All(pairs, p => Assert.Equal("Corpus.NUnit.StackTests.PushesPairs", p.Item.FullyQualifiedName));
        Assert.All(pairs, p => Assert.Equal([new TestTrait("Category", "Pairs")], p.Item.Traits));

        // A selection: two tests only.
        var chosen = discovery.Found.Where(t => t.Item.Method is "Pushes" or "Pops").ToList();
        var run = new RecordingSink();
        await runner.RunAsync(container, chosen, debug: false, run, Ct).WaitAsync(Limit, Ct);
        Assert.Equal(2, run.Final.Count);
        Assert.Equal(TestOutcomes.Passed, ByMethod(discovery, run, "Pushes").Outcome);
        var pops = ByMethod(discovery, run, "Pops");
        Assert.Equal(TestOutcomes.Failed, pops.Outcome);
        Assert.Contains("StackTests.cs:line", pops.StackTrace, StringComparison.Ordinal);

        // All of it, by source: the skip reason and the output.
        var all = new RecordingSink();
        await runner.RunAsync(container, null, debug: false, all, Ct).WaitAsync(Limit, Ct);
        Assert.Equal(6, all.Final.Count);
        Assert.Contains("Peeking is not decided yet", ByMethod(discovery, all, "Peeks").Message, StringComparison.Ordinal);
        Assert.Contains("Hello from NUnit", ByMethod(discovery, all, "WritesOutput").Output, StringComparison.Ordinal);
    }

    [Theory]
    [InlineData("Corpus.XunitV3", TestRunnerProtocol.MicrosoftTestingPlatform)]
    [InlineData("Corpus.Xunit2", TestRunnerProtocol.VsTest)]
    public async Task Cancel_MidRun_StopsWithinTheGrace(string name, TestRunnerProtocol protocol)
    {
        var container = await ContainerAsync(name, protocol, env: new Dictionary<string, string> { ["CORPUS_SLOW_MS"] = "60000" });
        var runner = Runner(protocol);
        var discovery = new RecordingSink();
        await runner.DiscoverAsync(container, discovery, Ct).WaitAsync(Limit, Ct);
        var waits = discovery.Found.Where(t => t.Item.Method == "Waits").ToList();
        using var cancel = new CancellationTokenSource();
        var run = new RecordingSink();
        var task = runner.RunAsync(container, waits, debug: false, run, cancel.Token);
        await Task.Delay(TimeSpan.FromSeconds(1.5), Ct);
        Assert.False(task.IsCompleted);
        var watch = Stopwatch.StartNew();
        await cancel.CancelAsync();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => task.WaitAsync(TimeSpan.FromSeconds(15), Ct));
        Assert.True(watch.Elapsed < TimeSpan.FromSeconds(6), $"canceled in {watch.Elapsed}");
        Assert.DoesNotContain(run.Final.Values, r => r.Outcome == TestOutcomes.Passed);
    }

    [Fact]
    public async Task Mtp_DebugRun_HandsTheLaunchToTheSink_AndResultsFlowWhenItConnects()
    {
        var container = await ContainerAsync("Corpus.XunitV3", TestRunnerProtocol.MicrosoftTestingPlatform);
        var runner = new MtpRunner();
        var discovery = new RecordingSink();
        await runner.DiscoverAsync(container, discovery, Ct).WaitAsync(Limit, Ct);
        var adds = discovery.Found.Where(t => t.Item.Method == "Adds").ToList();
        var run = new RecordingSink();
        var task = runner.RunAsync(container, adds, debug: true, run, Ct);
        var launch = await run.Launched.WaitAsync(Limit, Ct);
        Assert.Equal(container.Program, launch.Program);
        Assert.Equal(TestRuntime.Dotnet, launch.Runtime);
        Assert.Contains("--server", launch.Args);
        Assert.Equal("1", launch.Env["TESTINGPLATFORM_TELEMETRY_OPTOUT"]);

        // The shell's adapter would run `dotnet <dll> <args>`; here the test does.
        var psi = new ProcessStartInfo(Corpus.Dotnet(), [launch.Program, .. launch.Args]) { WorkingDirectory = launch.Cwd, UseShellExecute = false, RedirectStandardOutput = true };
        foreach (var (k, v) in launch.Env)
        {
            psi.Environment[k] = v;
        }

        using var app = Process.Start(psi)!;
        await task.WaitAsync(Limit, Ct);
        Assert.Equal(TestOutcomes.Passed, Assert.Single(run.Final.Values).Outcome);
        await app.WaitForExitAsync(Ct).WaitAsync(TimeSpan.FromSeconds(20), Ct);
    }

    [Fact]
    public async Task VsTest_DebugRun_AsksTheSinkToAttach_ThenRuns()
    {
        var container = await ContainerAsync("Corpus.Xunit2", TestRunnerProtocol.VsTest);
        var runner = new VsTestRunner();
        var discovery = new RecordingSink();
        await runner.DiscoverAsync(container, discovery, Ct).WaitAsync(Limit, Ct);
        var greets = discovery.Found.Where(t => t.Item.Method == "Greets").ToList();
        var run = new RecordingSink();
        await runner.RunAsync(container, greets, debug: true, run, Ct).WaitAsync(Limit, Ct);
        var pid = await run.Attached.WaitAsync(TimeSpan.FromSeconds(1), Ct);
        Assert.True(pid > 0);
        Assert.Equal(TestOutcomes.Passed, Assert.Single(run.Final.Values).Outcome);
    }

    [Fact]
    public async Task Mtp_XunitV3_RunsOnMono_ForNet472()
    {
        Assert.SkipWhen(Mono() is null, "Mono is not installed");
        var container = await ContainerAsync("Corpus.XunitV3", TestRunnerProtocol.MicrosoftTestingPlatform, tfm: "net472");
        Assert.Equal(TestRuntime.Mono, container.Runtime);
        var (discovery, run) = await DiscoverAndRunAsync(container);
        Assert.Equal(7, discovery.Found.Count);
        Assert.Equal(TestOutcomes.Failed, ByMethod(discovery, run, "Subtracts").Outcome);
        Assert.Equal(TestOutcomes.Passed, ByMethod(discovery, run, "Adds").Outcome);
        var (fileName, args) = MtpRunner.CommandLine(container);
        Assert.Equal(container.MonoExecutable, fileName);
        Assert.Equal(["--debug", container.Program], args);
    }

    [Fact]
    public async Task Mtp_Discovers100Tests_UnderThreeSecondsWarm()
    {
        var container = await ContainerAsync("Corpus.Many", TestRunnerProtocol.MicrosoftTestingPlatform);
        var runner = new MtpRunner();
        await runner.DiscoverAsync(container, new RecordingSink(), Ct).WaitAsync(Limit, Ct);
        var times = new List<double>();
        for (var i = 0; i < 3; i++)
        {
            var sink = new RecordingSink();
            var watch = Stopwatch.StartNew();
            await runner.DiscoverAsync(container, sink, Ct).WaitAsync(Limit, Ct);
            times.Add(watch.Elapsed.TotalMilliseconds);
            Assert.Equal(Corpus.ManyTests, sink.Found.Count);
        }

        TestContext.Current.SendDiagnosticMessage($"discovery of {Corpus.ManyTests} tests, warm: {string.Join(", ", times.Select(t => $"{t:0} ms"))}");
        Assert.True(times.Min() < 3000, $"discovery took {times.Min():0} ms");
    }

    [Fact]
    public async Task Inspect_TheCorpusProjects()
    {
        var root = await Corpus.DirectoryAsync();
        var v3 = TestProjectInspector.Inspect(Corpus.Project(root, "Corpus.XunitV3"))!;
        Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, v3.Protocol);
        Assert.Equal(["net10.0", "net472"], v3.TargetFrameworks);
        Assert.True(v3.IsExe);
        Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, TestProjectInspector.Inspect(Corpus.Project(root, "Corpus.MSTest"))!.Protocol);
        var x2 = TestProjectInspector.Inspect(Corpus.Project(root, "Corpus.Xunit2"))!;
        Assert.Equal(TestRunnerProtocol.VsTest, x2.Protocol);
        Assert.False(x2.IsExe);
        Assert.Equal("Corpus.Xunit2", x2.AssemblyName);
        Assert.Equal(TestRunnerProtocol.VsTest, TestProjectInspector.Inspect(Corpus.Project(root, "Corpus.NUnit"))!.Protocol);
        Assert.Null(TestProjectInspector.Inspect(Path.Combine(root, "missing.csproj")));
    }
}
