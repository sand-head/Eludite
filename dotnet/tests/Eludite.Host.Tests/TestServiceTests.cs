using System.Diagnostics;
using System.Text.Json;
using System.Threading.Channels;
using Eludite.Host.Build;
using Eludite.Host.Rpc;
using Eludite.Host.Testing;
using Eludite.TestBridge;
using Eludite.TestBridge.Tests;
using Nerdbank.Streams;
using StreamJsonRpc;
using TestResult = Eludite.TestBridge.TestResult;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0035: <c>eludite/test/*</c> over the wire. Against the corpus built in a temp folder: discovery of every
/// container (MTP and VSTest, one per target framework) and a run of all of it with the expected outcomes; a run of
/// ids before any discovery (discovered silently first). With a scripted runner: the update stream's order and
/// batching, one run per container (-32012), cancel, the generation rule, the status replay, the debug hand-offs
/// (launch, attach and eludite/test/attached) and the not-built container.
/// </summary>
public sealed class TestServiceTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private static readonly TimeSpan Limit = TimeSpan.FromMinutes(3);

    [Fact]
    public async Task Corpus_DiscoversEveryContainer_ThenRunsAll_WithTheExpectedOutcomes()
    {
        var root = await Corpus.DirectoryAsync();
        await using var host = await WireHost.OpenAsync(Path.Combine(root, "Corpus.slnx"));

        var discover = await host.CallAsync("eludite/test/discover", new { });
        var containers = discover.GetProperty("containers").EnumerateArray().ToList();
        var names = containers.Select(c => c.GetProperty("name").GetString()).ToList();
        Assert.Equal(["Corpus.XunitV3 (net10.0)", "Corpus.XunitV3 (net472)", "Corpus.Xunit2", "Corpus.MSTest", "Corpus.NUnit", "Corpus.Many"], names);
        Assert.Equal(["mtp", "mtp", "vstest", "mtp", "vstest", "mtp"], containers.Select(c => c.GetProperty("protocol").GetString()));
        Assert.Equal(OperatingSystem.IsWindows() ? "netfx" : "mono", containers[1].GetProperty("runtime").GetString());
        var runId = discover.GetProperty("runId").GetInt64();
        var finished = await host.FinishedAsync(runId);
        var updates = host.Updates(runId);
        Assert.Equal(Enumerable.Range(0, updates.Count).Select(i => (long)i), updates.Select(u => u.GetProperty("seq").GetInt64()));
        Assert.Single(updates, u => u.GetProperty("kind").GetString() == "finished");
        // The net472 container runs on Windows as it is, elsewhere under Mono when there is one.
        var monoFound = OperatingSystem.IsWindows() || File.Exists("/usr/bin/mono") || File.Exists("/usr/local/bin/mono");
        var expected = 7 + (monoFound ? 7 : 0) + 5 + 6 + 6 + Corpus.ManyTests;
        Assert.Equal(expected, finished.GetProperty("summary").GetProperty("total").GetInt32());
        var perContainer = updates.Where(u => u.GetProperty("kind").GetString() == "containerFinished").ToDictionary(u => u.GetProperty("container").GetString()!, u => u.GetProperty("count").GetInt32());
        Assert.Equal(6, perContainer.Count);
        Assert.Equal(5, perContainer[containers[2].GetProperty("id").GetString()!]);
        var discovered = updates.Where(u => u.GetProperty("kind").GetString() == "discovered").SelectMany(u => u.GetProperty("tests").EnumerateArray()).ToList();
        Assert.Equal(expected, discovered.Count);
        Assert.Contains(discovered, t => t.GetProperty("fullyQualifiedName").GetString() == "Corpus.Xunit2.GreeterTests.Waits"
            && t.GetProperty("traits")[0].GetProperty("value").GetString() == "Slow");
        Assert.Contains(updates, u => u.GetProperty("kind").GetString() == "output");

        // Run All: every container of the discovery.
        var watch = Stopwatch.StartNew();
        var run = await host.CallAsync("eludite/test/run", new { });
        var runFinished = await host.FinishedAsync(run.GetProperty("runId").GetInt64());
        TestContext.Current.SendDiagnosticMessage($"Run All of the .NET corpus: {watch.ElapsedMilliseconds} ms");
        Assert.Equal("completed", runFinished.GetProperty("state").GetString());
        var summary = runFinished.GetProperty("summary");
        Assert.Equal(expected, summary.GetProperty("total").GetInt32());
        // One failure and one skip per project (xunit.v3 twice with Mono), none in Corpus.Many.
        var projects = monoFound ? 5 : 4;
        Assert.Equal(projects, summary.GetProperty("failed").GetInt32());
        Assert.Equal(projects, summary.GetProperty("skipped").GetInt32());
        Assert.Equal(0, summary.GetProperty("notRun").GetInt32());
        var results = host.Updates(run.GetProperty("runId").GetInt64())
            .Where(u => u.GetProperty("kind").GetString() == "results")
            .SelectMany(u => u.GetProperty("results").EnumerateArray()).ToList();
        var failure = Assert.Single(results, r => r.GetProperty("outcome").GetString() == "failed" && r.GetProperty("stackTrace").GetString()!.Contains("GreeterTests.cs:line 26", StringComparison.Ordinal));
        Assert.False(failure.TryGetProperty("displayName", out _));
        Assert.Contains(results, r => r.TryGetProperty("output", out var o) && o.GetString()!.Contains("Hello from NUnit", StringComparison.Ordinal));
    }

    [Fact]
    public async Task Corpus_RunOfIds_BeforeAnyDiscovery_DiscoversSilentlyFirst()
    {
        var root = await Corpus.DirectoryAsync();
        await using var host = await WireHost.OpenAsync(Path.Combine(root, "Corpus.slnx"));
        var mstest = $"{Corpus.Project(root, "Corpus.MSTest")}|net10.0";

        // The ids come from another host's discovery (a shell that restarted its host keeps them).
        var sink = new RecordingSink();
        var container = new TestContainer(mstest, Corpus.Project(root, "Corpus.MSTest"), "net10.0", TestRunnerProtocol.MicrosoftTestingPlatform, Corpus.Output(root, "Corpus.MSTest", "net10.0"), TestRuntime.Dotnet) { DotnetExecutable = Corpus.Dotnet() };
        await new MtpRunner().DiscoverAsync(container, sink, Ct);
        var negatives = sink.Found.Single(t => t.Item.Method == "ParsesNegatives").Item.Id;

        var run = await host.CallAsync("eludite/test/run", new { containers = new[] { new { id = mstest, tests = new[] { negatives } } } });
        var finished = await host.FinishedAsync(run.GetProperty("runId").GetInt64());
        Assert.Equal(1, finished.GetProperty("summary").GetProperty("total").GetInt32());
        Assert.Equal(1, finished.GetProperty("summary").GetProperty("failed").GetInt32());
        var updates = host.Updates(run.GetProperty("runId").GetInt64());
        Assert.DoesNotContain(updates, u => u.GetProperty("kind").GetString() == "discovered");
    }

    [Fact]
    public async Task Status_IsAnsweredBeforeInitialize_AndErrorsFollowTheContract()
    {
        await using var host = new WireHost(new ScriptedRunner());
        var status = await host.CallAsync("eludite/test/status", null);
        Assert.Empty(status.GetProperty("running").EnumerateArray());
        Assert.False(status.TryGetProperty("last", out _));
        var (code, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/test/discover", new { }));
        Assert.Equal(-32002, code);
        await host.InitializeAsync();
        (code, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/test/discover", new { }));
        Assert.Equal(-32602, code);
    }

    [Fact]
    public async Task ScriptedRuns_StreamInOrder_RefuseASecondRunOfAContainer_ReplayStatus_AndCancel()
    {
        using var dir = new Fixture();
        var runner = new ScriptedRunner();
        await using var host = await WireHost.OpenAsync(dir.Solution, runner);

        var discover = await host.CallAsync("eludite/test/discover", new { });
        var id = discover.GetProperty("containers")[0].GetProperty("id").GetString()!;
        await host.FinishedAsync(discover.GetProperty("runId").GetInt64());

        var run = await host.CallAsync("eludite/test/run", new { containers = new[] { new { id } } });
        var runId = run.GetProperty("runId").GetInt64();
        await runner.Running.WaitAsync(Limit, Ct);
        runner.Report(new TestResult("t1", TestOutcomes.Running), new TestResult("t1", TestOutcomes.Passed) { DurationMs = 2 });
        await host.WaitForAsync(runId, u => u.GetProperty("kind").GetString() == "results");

        // A second run of the same container is refused with the running one's id.
        var (code, data) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/test/run", new { containers = new[] { new { id } } }));
        Assert.Equal(TestService.TestRunInProgress, code);
        Assert.Equal(runId, data!.Value.GetProperty("runId").GetInt64());
        Assert.Equal(id, data.Value.GetProperty("container").GetString());

        // Status: what was sent so far, with nextSeq after it.
        var status = await host.CallAsync("eludite/test/status", new { });
        var running = Assert.Single(status.GetProperty("running").EnumerateArray());
        Assert.Equal(runId, running.GetProperty("runId").GetInt64());
        Assert.Equal("run", running.GetProperty("kind").GetString());
        var latest = Assert.Single(running.GetProperty("results").EnumerateArray());
        Assert.Equal(("t1", "passed", id), (latest.GetProperty("id").GetString(), latest.GetProperty("outcome").GetString(), latest.GetProperty("container").GetString()));
        Assert.Equal(host.Updates(runId).Count, running.GetProperty("nextSeq").GetInt64());

        // Cancel ends it canceled; t2 never ran.
        var canceled = await host.CallAsync("eludite/test/cancel", new { runId });
        Assert.True(canceled.GetProperty("canceled").GetBoolean());
        var finished = await host.FinishedAsync(runId);
        Assert.Equal("canceled", finished.GetProperty("state").GetString());
        Assert.Equal(1, finished.GetProperty("summary").GetProperty("notRun").GetInt32());
        status = await host.CallAsync("eludite/test/status", new { });
        Assert.Empty(status.GetProperty("running").EnumerateArray());
        Assert.Equal("canceled", status.GetProperty("last").GetProperty("state").GetString());
        Assert.False((await host.CallAsync("eludite/test/cancel", new { })).GetProperty("canceled").GetBoolean());
    }

    [Fact]
    public async Task GenerationRule_ANewSolutionCancelsTheRun_UnderItsOldGeneration_AndForgetsTheDiscovery()
    {
        using var dir = new Fixture();
        var runner = new ScriptedRunner();
        await using var host = await WireHost.OpenAsync(dir.Solution, runner);
        var discover = await host.CallAsync("eludite/test/discover", new { });
        await host.FinishedAsync(discover.GetProperty("runId").GetInt64());
        Assert.Equal(1, runner.Discoveries);
        var run = await host.CallAsync("eludite/test/run", new { });
        var generation = run.GetProperty("generation").GetInt64();
        await runner.Running.WaitAsync(Limit, Ct);

        await host.CallAsync("eludite/solution/open", new { path = dir.Solution });
        var finished = await host.FinishedAsync(run.GetProperty("runId").GetInt64());
        Assert.Equal("canceled", finished.GetProperty("state").GetString());
        Assert.Equal(generation, finished.GetProperty("generation").GetInt64());

        // The discovery is forgotten: a run under the new generation discovers again (silently).
        runner.HoldRuns = false;
        var again = await host.CallAsync("eludite/test/run", new { });
        Assert.Equal(generation + 1, again.GetProperty("generation").GetInt64());
        await host.FinishedAsync(again.GetProperty("runId").GetInt64());
        Assert.Equal(2, runner.Discoveries);
    }

    [Fact]
    public async Task DebugRuns_SendLaunchAndAttach_AndTheAttachedAnswerReachesTheRunner()
    {
        using var dir = new Fixture();
        var runner = new ScriptedRunner { HoldRuns = false, Attach = true };
        await using var host = await WireHost.OpenAsync(dir.Solution, runner);
        var discover = await host.CallAsync("eludite/test/discover", new { });
        var id = discover.GetProperty("containers")[0].GetProperty("id").GetString()!;
        await host.FinishedAsync(discover.GetProperty("runId").GetInt64());

        var (code, _) = await TestRpc.ErrorOfAsync(() => host.CallAsync("eludite/test/run", new { debug = true, containers = Array.Empty<object>() }));
        Assert.Equal(-32602, code);
        var run = await host.CallAsync("eludite/test/run", new { debug = true, containers = new[] { new { id, tests = new[] { "t1" } } } });
        Assert.True(run.GetProperty("debug").GetBoolean());
        var runId = run.GetProperty("runId").GetInt64();
        var launch = await host.WaitForAsync(runId, u => u.GetProperty("kind").GetString() == "launch");
        Assert.Equal("/x/App.dll", launch.GetProperty("launch").GetProperty("program").GetString());
        Assert.Equal("dotnet", launch.GetProperty("launch").GetProperty("runtime").GetString());
        var attach = await host.WaitForAsync(runId, u => u.GetProperty("kind").GetString() == "attach");
        Assert.Equal(4242, attach.GetProperty("processId").GetInt32());
        Assert.False((await host.CallAsync("eludite/test/attached", new { runId, processId = 1, attached = true })).GetProperty("accepted").GetBoolean());
        Assert.True((await host.CallAsync("eludite/test/attached", new { runId, processId = 4242, attached = true })).GetProperty("accepted").GetBoolean());
        var finished = await host.FinishedAsync(runId);
        Assert.Equal("completed", finished.GetProperty("state").GetString());
        Assert.Equal((true, (string?)null), runner.AttachAnswer);
        Assert.Equal(["t1"], runner.LastRun);
    }

    [Fact]
    public async Task NotBuiltContainers_FinishFailed_WithTheReason()
    {
        using var dir = new Fixture(built: false);
        await using var host = await WireHost.OpenAsync(dir.Solution, new ScriptedRunner());
        var discover = await host.CallAsync("eludite/test/discover", new { });
        var container = discover.GetProperty("containers")[0];
        Assert.StartsWith("not built: ", container.GetProperty("error").GetString(), StringComparison.Ordinal);
        var finished = await host.FinishedAsync(discover.GetProperty("runId").GetInt64());
        Assert.Equal("failed", finished.GetProperty("state").GetString());
        var done = Assert.Single(host.Updates(discover.GetProperty("runId").GetInt64()), u => u.GetProperty("kind").GetString() == "containerFinished");
        Assert.Equal("failed", done.GetProperty("state").GetString());
        Assert.StartsWith("not built: ", done.GetProperty("message").GetString(), StringComparison.Ordinal);
    }

    /// <summary>A solution with one MTP test project (and its output, unless <c>built</c> is false).</summary>
    private sealed class Fixture : IDisposable
    {
        public Fixture(bool built = true)
        {
            Root = Path.Combine(Path.GetTempPath(), "eludite-tests-" + Guid.NewGuid().ToString("N")[..12]);
            var project = Path.Combine(Root, "App.Tests", "App.Tests.csproj");
            Directory.CreateDirectory(Path.GetDirectoryName(project)!);
            File.WriteAllText(project, """
                <Project Sdk="Microsoft.NET.Sdk">
                  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup>
                  <ItemGroup><PackageReference Include="xunit.v3" /></ItemGroup>
                </Project>
                """);
            if (built)
            {
                var output = Path.Combine(Root, "App.Tests", "bin", "Debug", "net10.0");
                Directory.CreateDirectory(output);
                File.WriteAllText(Path.Combine(output, "App.Tests.dll"), "not really");
            }

            Solution = Path.Combine(Root, "App.slnx");
            File.WriteAllText(Solution, "<Solution><Project Path=\"App.Tests/App.Tests.csproj\" /></Solution>");
        }

        public string Root { get; }

        public string Solution { get; }

        public void Dispose()
        {
            try
            {
                Directory.Delete(Root, recursive: true);
            }
            catch (IOException)
            {
            }
        }
    }

    /// <summary>A runner the test drives: two tests, runs that wait for <see cref="Report"/> and cancel.</summary>
    private sealed class ScriptedRunner : ITestRunner
    {
        private readonly Channel<TestResult[]> _reports = Channel.CreateUnbounded<TestResult[]>();
        private readonly TaskCompletionSource _running = new(TaskCreationOptions.RunContinuationsAsynchronously);
        private int _discoveries;

        public bool HoldRuns { get; set; } = true;

        public bool Attach { get; set; }

        public (bool, string?)? AttachAnswer { get; private set; }

        public List<string>? LastRun { get; private set; }

        public int Discoveries => _discoveries;

        public Task Running => _running.Task;

        public void Report(params TestResult[] results) => _reports.Writer.TryWrite(results);

        public Task DiscoverAsync(TestContainer container, ITestSink sink, CancellationToken cancellationToken)
        {
            Interlocked.Increment(ref _discoveries);
            sink.Log("discovering");
            sink.Tests([Test("t1", "Adds"), Test("t2", "Subtracts")]);
            return Task.CompletedTask;
        }

        public async Task RunAsync(TestContainer container, IReadOnlyList<DiscoveredTest>? tests, bool debug, ITestSink sink, CancellationToken cancellationToken)
        {
            LastRun = tests?.Select(t => t.Item.Id).ToList();
            if (debug)
            {
                sink.Launch(new TestLaunch("/x/App.dll", ["--server"], "/x", new Dictionary<string, string>(), TestRuntime.Dotnet));
                if (Attach)
                {
                    AttachAnswer = await sink.AttachAsync(4242, cancellationToken);
                }
            }

            _running.TrySetResult();
            if (!HoldRuns)
            {
                sink.Results([.. (tests ?? []).Select(t => new TestResult(t.Item.Id, TestOutcomes.Passed))]);
                return;
            }

            while (true)
            {
                var batch = await _reports.Reader.ReadAsync(cancellationToken);
                sink.Results(batch);
            }
        }

        private static DiscoveredTest Test(string id, string method) =>
            new(new TestItem(id, method, "App.Tests." + method) { Namespace = "App", ClassName = "Tests", Method = method }, JsonSerializer.SerializeToElement(new { uid = id }));
    }

    private sealed class WireHost : IAsyncDisposable
    {
        private readonly Task<int> _server;
        private readonly List<JsonElement> _updates = [];

        public WireHost(ITestRunner? runner = null)
        {
            var (clientStream, serverStream) = FullDuplexStream.CreatePair();
            Target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, build: new BuildService(() => Target!.LanguageServer.CurrentSolution(), TextWriter.Null),
                tests: new TestService(() => Target!.LanguageServer.CurrentSolution(), TextWriter.Null, runner is null ? null : _ => runner));
            _server = HostServer.RunAsync(serverStream, serverStream, Target);
            Client = TestRpc.Create(clientStream);
            TestRpc.On(Client, "eludite/test/update", p =>
            {
                lock (_updates)
                {
                    _updates.Add(p.Clone());
                }
            });
            Client.StartListening();
        }

        public static async Task<WireHost> OpenAsync(string solution, ITestRunner? runner = null)
        {
            var host = new WireHost(runner);
            await host.InitializeAsync();
            await host.CallAsync("eludite/solution/open", new { path = solution });
            return host;
        }

        public HostRpcTarget Target { get; }

        public JsonRpc Client { get; }

        public Task InitializeAsync() => CallAsync("eludite/host/initialize", new { clientName = "test", clientVersion = "0" });

        public Task<JsonElement> CallAsync(string method, object? parameters) =>
            Client.InvokeWithParameterObjectAsync<JsonElement>(method, parameters, Ct);

        public List<JsonElement> Updates(long runId)
        {
            lock (_updates)
            {
                return _updates.Where(u => u.GetProperty("runId").GetInt64() == runId).OrderBy(u => u.GetProperty("seq").GetInt64()).ToList();
            }
        }

        public async Task<JsonElement> WaitForAsync(long runId, Func<JsonElement, bool> predicate)
        {
            var watch = Stopwatch.StartNew();
            while (true)
            {
                if (Updates(runId).FirstOrDefault(predicate) is { ValueKind: JsonValueKind.Object } found)
                {
                    return found;
                }

                Assert.True(watch.Elapsed < Limit, "the update did not come");
                await Task.Delay(10, Ct);
            }
        }

        public Task<JsonElement> FinishedAsync(long runId) => WaitForAsync(runId, u => u.GetProperty("kind").GetString() == "finished");

        public async ValueTask DisposeAsync()
        {
            await Target.Tests.Idle.WaitAsync(TimeSpan.FromSeconds(30));
            Client.Dispose();
            await _server.WaitAsync(TimeSpan.FromSeconds(10));
        }
    }
}
