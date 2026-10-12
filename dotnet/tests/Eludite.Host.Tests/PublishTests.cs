using System.Text.Json;
using System.Threading.Channels;
using Eludite.Host.Build;
using Eludite.Host.Rpc;
using Nerdbank.Streams;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0074: the build service's <c>publish</c> target. <c>corpus/run/Echo</c> (copied per test) published through a
/// folder profile to a temporary folder, streamed and finished like a build, and the published program run with a typed
/// line and its exit code; the plan's arguments (a configuration only when asked for, the profile) and its refusals.
/// </summary>
public sealed class PublishTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private static readonly BuildToolchains NoLegacyToolchains = new(null, null, null);

    [Fact]
    public async Task Publish_ThroughAFolderProfile_StreamsLikeABuild_AndThePublishedProgramRuns()
    {
        using var corpus = new ProjectCorpus("run");
        var echo = corpus.Path("Echo/Echo.csproj");
        var publishDir = Path.Combine(corpus.Root, "published") + Path.DirectorySeparatorChar;
        var profile = corpus.Path("Echo/Properties/PublishProfiles/FolderProfile.pubxml");
        Directory.CreateDirectory(Path.GetDirectoryName(profile)!);
        File.WriteAllText(profile, $"""
            <?xml version="1.0" encoding="utf-8"?>
            <Project>
              <PropertyGroup>
                <Configuration>Release</Configuration>
                <Platform>Any CPU</Platform>
                <PublishDir>{publishDir}</PublishDir>
                <PublishProtocol>FileSystem</PublishProtocol>
                <TargetFramework>net10.0</TargetFramework>
                <SelfContained>false</SelfContained>
              </PropertyGroup>
            </Project>
            """);

        await using var host = new PublishHost(echo);
        var started = host.Service.Start(new BuildStartParams(BuildTargets.Publish) { Project = echo, PublishProfile = profile });
        Assert.Equal("publish", started.Target);
        Assert.Equal(echo, started.Path);
        Assert.Equal("Release", started.Configuration);
        Assert.StartsWith("dotnet publish ", started.CommandLine, StringComparison.Ordinal);
        Assert.Contains("-p:PublishProfile=" + profile, started.CommandLine, StringComparison.Ordinal);
        Assert.DoesNotContain("-p:Configuration=", started.CommandLine, StringComparison.Ordinal);

        var finished = await host.FinishedAsync();
        var text = host.Text;
        Assert.True(finished.GetProperty("result").GetString() == "succeeded", text);
        Assert.Equal("publish", finished.GetProperty("target").GetString());
        Assert.Equal(0, finished.GetProperty("exitCode").GetInt32());
        Assert.StartsWith("Publish started at ", text, StringComparison.Ordinal);
        Assert.Contains("------ Publish started: Echo.csproj, Configuration: Release ------", text, StringComparison.Ordinal);
        Assert.Contains("========== Publish: 1 succeeded, 0 failed ==========", text, StringComparison.Ordinal);
        Assert.Equal("publish", host.Service.Status().Last!.Target);

        var dll = Path.Combine(publishDir, "Echo.dll");
        Assert.True(File.Exists(dll), $"{dll} not published:\n{text}");
        var (code, output) = await RunTargetTests.RunAsync("dotnet", [dll], publishDir, "Ada\n");
        Assert.Equal("Name? Hello, Ada!", output.Trim());
        Assert.Equal(3, code);
    }

    [Fact]
    public void Plan_PassesAConfigurationOnlyWhenAsked_AndTheProfile()
    {
        var project = Path.Combine(Path.GetTempPath(), "p", "Echo.csproj");
        var profile = Path.Combine(Path.GetTempPath(), "p", "Properties", "PublishProfiles", "FolderProfile.pubxml");

        var plain = BuildPlan.Create(BuildTargets.Publish, project, null, null, "/logs/b.binlog", false, NoLegacyToolchains, profile);
        Assert.Equal("dotnet", plain.FileName);
        Assert.Equal(["publish", project, "-tl:off", "-nologo", "-v:m", "-nr:false", "-clp:ForceNoAlign", "-bl:/logs/b.binlog", "-p:PublishProfile=" + profile], plain.Arguments);
        Assert.Equal("dotnet", plain.Toolchain.Kind);
        Assert.Empty(plain.Notes);

        var configured = BuildPlan.Create(BuildTargets.Publish, project, "Debug", "x64", null, false, NoLegacyToolchains, profile);
        Assert.Equal(["publish", project, "-tl:off", "-nologo", "-v:m", "-nr:false", "-clp:ForceNoAlign", "-p:Configuration=Debug", "-p:Platform=x64", "-p:PublishProfile=" + profile], configured.Arguments);

        var noProfile = BuildPlan.Create(BuildTargets.Publish, project, null, null, null, false, NoLegacyToolchains);
        Assert.DoesNotContain(noProfile.Arguments, a => a.StartsWith("-p:PublishProfile", StringComparison.Ordinal));

        // A build keeps its configuration, and ignores a profile.
        var build = BuildPlan.Create(BuildTargets.Build, project, "Debug", null, null, false, NoLegacyToolchains, profile);
        Assert.Equal(["build", project, "-tl:off", "-nologo", "-v:m", "-nr:false", "-clp:ForceNoAlign", "-p:Configuration=Debug"], build.Arguments);
    }

    [Fact]
    public void Plan_RefusesASolutionAndALegacyProject()
    {
        var solution = BuildPlanError(() => BuildPlan.Create(BuildTargets.Publish, "/s/App.slnx", null, null, null, false, NoLegacyToolchains));
        Assert.Equal(HostErrors.InvalidParams, solution.ErrorCode);
        Assert.Equal("publish needs a project", solution.Message);

        // Even with a legacy MSBuild at hand, a legacy project is not published.
        var mono = new BuildToolchains(new Legacy.MonoInstallation("/mono", "/mono/bin/mono", "/mono/lib/mono/msbuild/Current/bin/MSBuild.dll", "test", new Dictionary<string, string>()), null, null);
        var legacy = BuildPlanError(() => BuildPlan.Create(BuildTargets.Publish, "/s/Legacy/Legacy.csproj", null, null, null, true, mono));
        Assert.Equal(HostErrors.InvalidParams, legacy.ErrorCode);
        Assert.Contains("legacy (non-SDK) project", legacy.Message, StringComparison.Ordinal);
    }

    [Fact]
    public async Task Start_RefusesPublishWithoutAProject_AndOfALegacyProject()
    {
        using var corpus = new ProjectCorpus("run");
        var echo = corpus.Path("Echo/Echo.csproj");
        await using var host = new PublishHost(echo);
        var none = Assert.Throws<LocalRpcException>(() => host.Service.Start(new BuildStartParams(BuildTargets.Publish)));
        Assert.Equal(HostErrors.InvalidParams, none.ErrorCode);
        Assert.Equal("publish needs a project", none.Message);

        File.WriteAllText(echo, """
            <?xml version="1.0" encoding="utf-8"?>
            <Project ToolsVersion="15.0" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
              <PropertyGroup><OutputType>Exe</OutputType></PropertyGroup>
            </Project>
            """);
        var legacy = Assert.Throws<LocalRpcException>(() => host.Service.Start(new BuildStartParams(BuildTargets.Publish) { Project = echo }));
        Assert.Equal(HostErrors.InvalidParams, legacy.ErrorCode);
        Assert.Null(host.Service.Status().Running);
    }

    private static LocalRpcException BuildPlanError(Action create) => Assert.Throws<LocalRpcException>(create);

    /// <summary>A build service over an in-memory connection, recording its output and finished notifications.</summary>
    private sealed class PublishHost : IAsyncDisposable
    {
        private readonly List<JsonElement> _output = [];
        private readonly Channel<JsonElement> _finished = Channel.CreateUnbounded<JsonElement>();
        private readonly JsonRpc _server;
        private readonly JsonRpc _client;
        private readonly CancellationTokenSource _generation = new();
        private readonly string _logs = Path.Combine(Path.GetTempPath(), "eludite-publish-logs-" + Guid.NewGuid().ToString("N")[..8]);

        public PublishHost(string solution)
        {
            var (clientStream, serverStream) = FullDuplexStream.CreatePair();
            Service = new BuildService(() => (1, solution, _generation.Token), TextWriter.Null, () => NoLegacyToolchains, _logs);
            _server = HostServer.CreateConnection(serverStream, serverStream);
            Service.Attach(_server);
            _server.StartListening();
            _client = TestRpc.Create(clientStream);
            TestRpc.On(_client, "eludite/build/output", p => { lock (_output) { _output.Add(p.Clone()); } });
            TestRpc.On(_client, "eludite/build/progress", _ => { });
            TestRpc.On(_client, "eludite/build/finished", p => _finished.Writer.TryWrite(p.Clone()));
            _client.StartListening();
        }

        public BuildService Service { get; }

        public string Text
        {
            get
            {
                lock (_output)
                {
                    return string.Concat(_output.OrderBy(c => c.GetProperty("seq").GetInt64()).Select(c => c.GetProperty("text").GetString()));
                }
            }
        }

        public async Task<JsonElement> FinishedAsync() =>
            await _finished.Reader.ReadAsync(Ct).AsTask().WaitAsync(Budget.Hang(TimeSpan.FromMinutes(3)), Ct);

        public async ValueTask DisposeAsync()
        {
            try
            {
                await Service.DisposeAsync();
                await Service.Running.WaitAsync(Budget.Hang(TimeSpan.FromSeconds(30)));
            }
            finally
            {
                _client.Dispose();
                _server.Dispose();
                _generation.Dispose();
                TestDirectory.Delete(_logs);
            }
        }
    }
}
