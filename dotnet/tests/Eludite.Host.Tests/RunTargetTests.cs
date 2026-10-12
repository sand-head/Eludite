using System.Diagnostics;
using System.Text.Json;
using Eludite.Host.Projects;
using Eludite.Host.Rpc;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0074: <c>eludite/project/runTarget</c> on <c>corpus/run</c> and <c>corpus/projects/Multi</c> (copied per test):
/// the artifacts layout, assembly name and run working directory set in <c>Directory.Build.props</c>, the run command,
/// <c>exists</c> before and after a real build, the framework rule of a multi-targeted project, the per-generation cache
/// with its warm latency, and the wire shape against the schema.
/// </summary>
public sealed class RunTargetTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private static string Norm(string path) => path.Replace('\\', '/').TrimEnd('/');

    private static ProjectPropertiesService Service(Func<long> generation, string solution, CancellationToken changed, TextWriter? log = null) =>
        new(() => (generation(), solution, changed), generation, _ => generation(), log ?? TextWriter.Null);

    [Fact]
    public async Task ArtifactsApp_EvaluatesTheArtifactsPathRunCommandAndWorkingDirectory_AndSeesTheBuild()
    {
        using var corpus = new ProjectCorpus("run");
        var app = corpus.Path("Artifacts/App/App.csproj");
        long generation = 1;
        using var cts = new CancellationTokenSource();
        using var log = new StringWriter();
        using var service = Service(() => generation, app, cts.Token, log);

        var sw = Stopwatch.StartNew();
        var target = await service.RunTargetAsync(new RunTargetParams(app) { Configuration = "Debug" }, Ct);
        var cold = sw.Elapsed;
        sw.Restart();
        var again = await service.RunTargetAsync(new RunTargetParams(app) { Configuration = "Debug" }, Ct);
        var warm = sw.Elapsed;

        Assert.Equal(1, service.Evaluations);
        Assert.Same(target, again);
        TestContext.Current.TestOutputHelper?.WriteLine($"[budget] runTarget: cold {cold.TotalMilliseconds:F1} ms, warm {warm.TotalMilliseconds:F2} ms");
        Budget.Assert("warm runTarget", warm, TimeSpan.FromMilliseconds(50));
        Assert.Contains("[runTarget] App.csproj Debug|AnyCPU|net10.0 evaluated in ", log.ToString(), StringComparison.Ordinal);

        var root = Norm(corpus.Path("Artifacts"));
        var debugDir = root + "/artifacts/bin/App/debug";
        Assert.Equal((1L, app, "Debug", "AnyCPU"), (target.Generation, target.Project, target.Configuration, target.Platform));
        Assert.Equal(debugDir + "/Corpus.App.dll", Norm(target.TargetPath));
        Assert.Equal("net10.0", target.Framework);
        Assert.Equal(["net10.0"], target.TargetFrameworks);
        Assert.Equal(".NETCoreApp", target.TargetFrameworkIdentifier);
        Assert.Equal("Exe", target.OutputType);
        Assert.False(target.IsWebProject);
        Assert.Equal(root + "/work", Norm(target.RunWorkingDirectory));
        Assert.NotEmpty(target.RunCommand);
        var command = Norm(target.RunCommand);
        var dotnetHost = Path.GetFileNameWithoutExtension(command) == "dotnet";
        Assert.True(dotnetHost || command.StartsWith(debugDir + "/", StringComparison.Ordinal), $"RunCommand {target.RunCommand}");
        if (!dotnetHost)
        {
            Assert.Equal(debugDir + "/Corpus.App" + (OperatingSystem.IsWindows() ? ".exe" : string.Empty), command);
        }

        Assert.False(target.Exists);

        // Built: exists is checked on every call, so the same generation's cached evaluation sees it.
        var (code, output) = await RunAsync("dotnet", ["build", app, "-nologo", "-v:q", "-tl:off", "-nr:false", "-c", "Debug"], null, null);
        Assert.True(code == 0, output);
        var built = await service.RunTargetAsync(new RunTargetParams(app) { Configuration = "Debug" }, Ct);
        Assert.True(built.Exists);
        Assert.Equal(1, service.Evaluations);
        Assert.True(File.Exists(built.TargetPath));

        // And the program runs as the evaluation says: its assembly name, in the run working directory.
        var args = new List<string>();
        var fileName = built.RunCommand;
        if (built.RunArguments.Length > 0)
        {
            args.AddRange(CommandLineArguments(built.RunArguments));
        }

        (code, output) = await RunAsync(fileName, args, built.RunWorkingDirectory, null);
        Assert.Equal(0, code);
        var lines = output.Split('\n', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);
        Assert.Equal("Corpus.App", lines[0]);
        Assert.Equal(Norm(Path.GetFullPath(built.RunWorkingDirectory)), Norm(lines[1]));

        // A new generation evaluates again.
        generation = 2;
        var next = await service.RunTargetAsync(new RunTargetParams(app) { Configuration = "Debug" }, Ct);
        Assert.Equal(2, service.Evaluations);
        Assert.Equal(2, next.Generation);
        Assert.True(next.Exists);
    }

    [Fact]
    public async Task MultiTargeted_RunsTheFirstFramework_OrTheOneAskedForWhenListed()
    {
        using var corpus = new ProjectCorpus();
        var multi = corpus.Path("Multi/Multi.csproj");
        long generation = 1;
        using var cts = new CancellationTokenSource();
        using var service = Service(() => generation, multi, cts.Token);

        var first = await service.RunTargetAsync(new RunTargetParams(multi), Ct);
        Assert.Equal("net8.0", first.Framework);
        Assert.Equal(["net8.0", "net10.0"], first.TargetFrameworks);
        Assert.Equal("Debug", first.Configuration);
        Assert.EndsWith("/bin/Debug/net8.0/Multi.dll", Norm(first.TargetPath), StringComparison.Ordinal);

        var ten = await service.RunTargetAsync(new RunTargetParams(multi) { Framework = "net10.0" }, Ct);
        Assert.Equal("net10.0", ten.Framework);
        Assert.EndsWith("/bin/Debug/net10.0/Multi.dll", Norm(ten.TargetPath), StringComparison.Ordinal);
        Assert.Equal(".NETCoreApp", ten.TargetFrameworkIdentifier);

        var unlisted = await service.RunTargetAsync(new RunTargetParams(multi) { Framework = "net6.0" }, Ct);
        Assert.Equal("net8.0", unlisted.Framework);
        Assert.Equal(3, service.Evaluations);
    }

    [Fact]
    public async Task Refusals_NotAProjectOfTheSolution_NotAbsolute_AndAProjectThatDoesNotEvaluate()
    {
        using var corpus = new ProjectCorpus("run");
        var app = corpus.Path("Artifacts/App/App.csproj");
        long generation = 1;
        using var cts = new CancellationTokenSource();
        using var service = Service(() => generation, app, cts.Token);

        var foreign = await Assert.ThrowsAsync<LocalRpcException>(() => service.RunTargetAsync(new RunTargetParams(corpus.Path("Echo/Echo.csproj")), Ct));
        Assert.Equal(HostErrors.InvalidParams, foreign.ErrorCode);
        var relative = await Assert.ThrowsAsync<LocalRpcException>(() => service.RunTargetAsync(new RunTargetParams("App.csproj"), Ct));
        Assert.Equal(HostErrors.InvalidParams, relative.ErrorCode);

        File.WriteAllText(app, "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><Broken></PropertyGroup></Project>");
        var broken = await Assert.ThrowsAsync<LocalRpcException>(() => service.RunTargetAsync(new RunTargetParams(app), Ct));
        Assert.Equal(HostErrors.InvalidParams, broken.ErrorCode);
    }

    [Fact]
    public async Task Result_SerializesToTheSchemasMembers()
    {
        using var corpus = new ProjectCorpus("run");
        var app = corpus.Path("Artifacts/App/App.csproj");
        long generation = 4;
        using var cts = new CancellationTokenSource();
        using var service = Service(() => generation, app, cts.Token);
        var target = await service.RunTargetAsync(new RunTargetParams(app), Ct);

        var json = JsonSerializer.SerializeToElement(target, HostServer.SerializerOptions);
        using var schema = JsonDocument.Parse(File.ReadAllText(SchemaPath()));
        var result = schema.RootElement.GetProperty("$defs").GetProperty("result");
        var allowed = result.GetProperty("properties").EnumerateObject().Select(p => p.Name).ToHashSet(StringComparer.Ordinal);
        var names = json.EnumerateObject().Select(p => p.Name).ToList();
        Assert.All(names, n => Assert.Contains(n, allowed));
        Assert.All(result.GetProperty("required").EnumerateArray().Select(r => r.GetString()!), r => Assert.Contains(r, names));
        Assert.Equal("net10.0", json.GetProperty("framework").GetString());
        Assert.Equal(4, json.GetProperty("generation").GetInt64());

        // And the params record reads the schema's members.
        var parameters = JsonSerializer.Deserialize<RunTargetParams>("""{"project":"/p/A.csproj","configuration":"Release","platform":"x64","framework":"net10.0"}""", HostServer.SerializerOptions)!;
        Assert.Equal(("/p/A.csproj", "Release", "x64", "net10.0"), (parameters.Project, parameters.Configuration, parameters.Platform, parameters.Framework));
    }

    private static string SchemaPath()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "protocol", "schemas", "host", "run-target.json");
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        throw new InvalidOperationException("protocol/schemas/host/run-target.json not found above " + AppContext.BaseDirectory);
    }

    /// <summary>Splits a <c>RunArguments</c> string (double quotes group, as <c>dotnet run</c> passes them).</summary>
    private static List<string> CommandLineArguments(string line)
    {
        var args = new List<string>();
        var current = new System.Text.StringBuilder();
        var quoted = false;
        var any = false;
        foreach (var ch in line)
        {
            if (ch == '"')
            {
                quoted = !quoted;
                any = true;
            }
            else if (char.IsWhiteSpace(ch) && !quoted)
            {
                if (any)
                {
                    args.Add(current.ToString());
                    current.Clear();
                    any = false;
                }
            }
            else
            {
                current.Append(ch);
                any = true;
            }
        }

        if (any)
        {
            args.Add(current.ToString());
        }

        return args;
    }

    /// <summary>Runs a program to exit (bounded only against a hang) and returns its exit code and output.</summary>
    internal static async Task<(int Code, string Output)> RunAsync(string fileName, IEnumerable<string> args, string? workingDirectory, string? stdin)
    {
        var info = new ProcessStartInfo(fileName)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            RedirectStandardInput = stdin is not null,
            UseShellExecute = false,
        };
        foreach (var a in args)
        {
            info.ArgumentList.Add(a);
        }

        if (workingDirectory is not null)
        {
            info.WorkingDirectory = workingDirectory;
        }

        info.Environment["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1";
        info.Environment["DOTNET_NOLOGO"] = "1";
        info.Environment["MSBUILDDISABLENODEREUSE"] = "1";
        using var process = Process.Start(info)!;
        if (stdin is not null)
        {
            await process.StandardInput.WriteAsync(stdin);
            process.StandardInput.Close();
        }

        var stdout = process.StandardOutput.ReadToEndAsync(Ct);
        var stderr = process.StandardError.ReadToEndAsync(Ct);
        try
        {
            await process.WaitForExitAsync(Ct).WaitAsync(Budget.Hang(TimeSpan.FromMinutes(3)), Ct);
        }
        catch (TimeoutException)
        {
            process.Kill(entireProcessTree: true);
            throw;
        }

        return (process.ExitCode, await stdout + await stderr);
    }
}
