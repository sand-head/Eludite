using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Threading.Channels;
using Eludite.Host.Build;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using Nerdbank.Streams;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0017: <c>eludite/build/*</c> end to end over the wire with real MSBuild runs: an SDK fixture that builds, one
/// that fails (diagnostics from the binary log), a build canceled mid-run (process tree killed, canceled within 2 s),
/// a concurrent start refused, and legacy builds on Mono (skipped without Mono or the corpus).
/// </summary>
public sealed class BuildServiceTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private static readonly TimeSpan BuildTimeout = TimeSpan.FromMinutes(3);

    [Fact]
    public async Task SdkFixture_Builds_StreamsOrderedOutput_AndReportsProjectsFromTheBinlog()
    {
        using var dir = new TempDir();
        var slnx = SdkFixture(dir.Path, "class Program { static void Main() => System.Console.WriteLine(Lib.Greeting.Text); }");
        await using var host = await WireHost.OpenAsync(slnx);

        var started = await host.StartAsync(new { target = "build" });
        Assert.Equal(1, started.GetProperty("buildId").GetInt64());
        Assert.Equal("dotnet", started.GetProperty("toolchain").GetProperty("kind").GetString());
        Assert.Equal("Debug", started.GetProperty("configuration").GetString());
        Assert.Contains("-bl:", started.GetProperty("commandLine").GetString(), StringComparison.Ordinal);
        var finished = await host.FinishedAsync();

        Assert.Equal("succeeded", finished.GetProperty("result").GetString());
        Assert.Equal(0, finished.GetProperty("exitCode").GetInt32());
        var projects = finished.GetProperty("projects").EnumerateArray().ToList();
        Assert.Equal(["App", "Lib"], projects.Select(p => p.GetProperty("name").GetString()).Order(StringComparer.Ordinal));
        Assert.All(projects, p => Assert.Equal("succeeded", p.GetProperty("result").GetString()));
        Assert.All(projects, p => Assert.True(p.GetProperty("elapsedMs").GetDouble() > 0));
        Assert.Equal(2, finished.GetProperty("summary").GetProperty("projectsSucceeded").GetInt32());
        Assert.True(File.Exists(finished.GetProperty("binlog").GetString()));

        // Ordered chunks from 0, whole lines; the host's start line first and the summary last.
        var chunks = host.Output;
        Assert.Equal(Enumerable.Range(0, chunks.Count).Select(i => (long)i), chunks.Select(c => c.GetProperty("seq").GetInt64()));
        var text = string.Concat(chunks.Select(c => c.GetProperty("text").GetString()));
        Assert.All(chunks, c => Assert.EndsWith("\n", c.GetProperty("text").GetString(), StringComparison.Ordinal));
        Assert.StartsWith("Build started at ", text, StringComparison.Ordinal);
        Assert.Contains("App -> ", text, StringComparison.Ordinal);
        Assert.Contains("========== Build: 2 succeeded, 0 failed ==========", text, StringComparison.Ordinal);
        Assert.Contains(host.Progress, p => p.GetProperty("projectsTotal").GetInt32() == 2);

        // The service is free again: a clean runs.
        await host.StartAsync(new { target = "clean" });
        Assert.Equal("succeeded", (await host.FinishedAsync()).GetProperty("result").GetString());
    }

    [Fact]
    public async Task FailingFixture_ReportsErrorsAndWarningsWithLocations_FromTheBinlog()
    {
        using var dir = new TempDir();
        var slnx = SdkFixture(dir.Path, "class Program\n{\n    static void Main()\n    {\n        int unused;\n        System.Console.WriteLine(missing);\n    }\n}\n");
        await using var host = await WireHost.OpenAsync(slnx);

        await host.StartAsync(new { target = "build", configuration = "Release" });
        var finished = await host.FinishedAsync();

        Assert.Equal("failed", finished.GetProperty("result").GetString());
        Assert.NotEqual(0, finished.GetProperty("exitCode").GetInt32());
        var diagnostics = finished.GetProperty("diagnostics").EnumerateArray().ToList();
        var error = Assert.Single(diagnostics, d => d.GetProperty("severity").GetString() == "error");
        Assert.Equal("CS0103", error.GetProperty("code").GetString());
        Assert.Contains("'missing'", error.GetProperty("message").GetString(), StringComparison.Ordinal);
        Assert.Equal(Path.Combine(dir.Path, "App", "Program.cs"), error.GetProperty("file").GetString());
        Assert.Equal(6, error.GetProperty("line").GetInt32());
        Assert.Equal(34, error.GetProperty("column").GetInt32());
        Assert.Equal(Path.Combine(dir.Path, "App", "App.csproj"), error.GetProperty("project").GetString());
        var warning = Assert.Single(diagnostics, d => d.GetProperty("code").GetString() == "CS0168");
        Assert.Equal("warning", warning.GetProperty("severity").GetString());
        Assert.Equal("error", diagnostics[0].GetProperty("severity").GetString());

        var summary = finished.GetProperty("summary");
        Assert.Equal(1, summary.GetProperty("errors").GetInt32());
        Assert.Equal(1, summary.GetProperty("projectsFailed").GetInt32());
        Assert.Equal(1, summary.GetProperty("projectsSucceeded").GetInt32());
        var app = finished.GetProperty("projects").EnumerateArray().Single(p => p.GetProperty("name").GetString() == "App");
        Assert.Equal("failed", app.GetProperty("result").GetString());
        Assert.Equal(1, app.GetProperty("errors").GetInt32());
        // MSBuild prints the error twice (inline and in its summary); the progress counts it once.
        Assert.Equal(1, host.Progress.Last().GetProperty("errors").GetInt32());
    }

    [Fact]
    public async Task Cancel_KillsTheProcessTree_AndReportsCanceledWithin2s()
    {
        using var dir = new TempDir();
        var marker = "eludite-slow-" + Guid.NewGuid().ToString("N")[..12];
        var slnx = SlowFixture(dir.Path, marker);
        await using var host = await WireHost.OpenAsync(slnx);

        await host.StartAsync(new { target = "build" });
        await host.WaitForOutputAsync(marker);
        await Task.Delay(300, Ct);
        if (OperatingSystem.IsLinux())
        {
            Assert.NotEmpty(ProcessesWith(marker));
        }

        var sw = Stopwatch.StartNew();
        var cancel = await host.Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/build/cancel", new { }, Ct);
        Assert.True(cancel.GetProperty("canceled").GetBoolean());
        Assert.Equal(1, cancel.GetProperty("buildId").GetInt64());
        var finished = await host.FinishedAsync();
        sw.Stop();

        Assert.Equal("canceled", finished.GetProperty("result").GetString());
        Assert.True(sw.Elapsed < TimeSpan.FromSeconds(2), $"canceled after {sw.ElapsedMilliseconds} ms");
        Assert.Equal(JsonValueKind.Undefined, finished.TryGetProperty("exitCode", out var code) ? code.ValueKind : JsonValueKind.Undefined);
        Assert.Contains("Build canceled.", host.Text, StringComparison.Ordinal);
        if (OperatingSystem.IsLinux())
        {
            Assert.Empty(ProcessesWith(marker));
        }

        // Nothing runs any more.
        var again = await host.Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/build/cancel", new { }, Ct);
        Assert.False(again.GetProperty("canceled").GetBoolean());
    }

    [Fact]
    public async Task SecondStart_WhileBuilding_IsRefusedWithBuildInProgress()
    {
        using var dir = new TempDir();
        var marker = "eludite-slow-" + Guid.NewGuid().ToString("N")[..12];
        var slnx = SlowFixture(dir.Path, marker);
        await using var host = await WireHost.OpenAsync(slnx);

        await host.StartAsync(new { target = "build" });
        var (code, data) = await TestRpc.ErrorOfAsync(() => host.StartAsync(new { target = "rebuild" }));
        Assert.Equal(BuildService.BuildInProgress, code);
        Assert.Equal(1, data!.Value.GetProperty("buildId").GetInt64());
        // A cancel naming another build changes nothing.
        var wrong = await host.Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/build/cancel", new { buildId = 7 }, Ct);
        Assert.False(wrong.GetProperty("canceled").GetBoolean());

        await host.Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/build/cancel", new { buildId = 1 }, Ct);
        Assert.Equal("canceled", (await host.FinishedAsync()).GetProperty("result").GetString());
        var next = await host.StartAsync(new { target = "clean" });
        Assert.Equal(2, next.GetProperty("buildId").GetInt64());
        await host.FinishedAsync();
    }

    [Fact]
    public async Task Start_IsRefused_BeforeInitialize_WithoutASolution_AndForAForeignProject()
    {
        await using var host = new WireHost();
        var (code, _) = await TestRpc.ErrorOfAsync(() => host.StartAsync(new { target = "build" }));
        Assert.Equal(HostErrors.ServerNotInitialized, code);
        await host.InitializeAsync();
        (code, _) = await TestRpc.ErrorOfAsync(() => host.StartAsync(new { target = "build" }));
        Assert.Equal(HostErrors.InvalidParams, code);

        using var dir = new TempDir();
        var slnx = SdkFixture(dir.Path, "class Program { static void Main() { } }");
        await host.Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = slnx }, Ct);
        (code, _) = await TestRpc.ErrorOfAsync(() => host.StartAsync(new { target = "publish" }));
        Assert.Equal(HostErrors.InvalidParams, code);
        (code, _) = await TestRpc.ErrorOfAsync(() => host.StartAsync(new { target = "build", project = "/elsewhere/Other.csproj" }));
        Assert.Equal(HostErrors.InvalidParams, code);
    }

    [Fact]
    public async Task ProjectBuild_BuildsThatProjectAndItsReferences()
    {
        using var dir = new TempDir();
        var slnx = SdkFixture(dir.Path, "class Program { static void Main() { } }");
        await using var host = await WireHost.OpenAsync(slnx);

        var lib = Path.Combine(dir.Path, "Lib", "Lib.csproj");
        var started = await host.StartAsync(new { target = "build", project = lib });
        Assert.Equal(lib, started.GetProperty("path").GetString());
        var finished = await host.FinishedAsync();
        Assert.Equal("succeeded", finished.GetProperty("result").GetString());
        Assert.Equal(["Lib"], finished.GetProperty("projects").EnumerateArray().Select(p => p.GetProperty("name").GetString()));
    }

    [Fact]
    public async Task LegacyProject_WithAWindowsBuildEvent_OnMono_GetsOneEludite0108PerProject()
    {
        Assert.SkipWhen(OperatingSystem.IsWindows(), "The Windows-only target messages apply off Windows.");
        var mono = MonoInstallation.Locate();
        Assert.SkipWhen(mono is null, "Mono's MSBuild not found (see tools/legacy-load/README.md, or set ELUDITE_MONO_PREFIX).");
        using var dir = new TempDir();
        var project = LegacyFixture(dir.Path);
        await using var host = await WireHost.OpenAsync(project);

        var started = await host.StartAsync(new { target = "build" });
        Assert.Equal("mono", started.GetProperty("toolchain").GetProperty("kind").GetString());
        var finished = await host.FinishedAsync();

        Assert.Equal("failed", finished.GetProperty("result").GetString());
        var diagnostics = finished.GetProperty("diagnostics").EnumerateArray().ToList();
        var eludite = Assert.Single(diagnostics, d => d.GetProperty("code").GetString() == WindowsOnlyTargets.BuildEvent);
        Assert.Equal("error", eludite.GetProperty("severity").GetString());
        Assert.Equal(project, eludite.GetProperty("project").GetString());
        Assert.Equal($"The pre-build event of Legacy is a Windows command script (cmd.exe) and cannot run on {WindowsOnlyTargets.ThisOs}.", eludite.GetProperty("message").GetString());
        Assert.DoesNotContain(diagnostics, d => d.GetProperty("code").GetString() == "MSB3073");
        Assert.Contains($"error {WindowsOnlyTargets.BuildEvent}: The pre-build event of Legacy", host.Text, StringComparison.Ordinal);
    }

    [Fact]
    public async Task LegacyCorpusProject_BuildsOnMono_WithoutStackTraces()
    {
        Assert.SkipWhen(OperatingSystem.IsWindows(), "Mono builds are the Linux and macOS path.");
        Assert.SkipWhen(MonoInstallation.Locate() is null, "Mono's MSBuild not found (see tools/legacy-load/README.md, or set ELUDITE_MONO_PREFIX).");
        var project = FindCorpusProject();
        Assert.SkipWhen(project is null, "Corpus checkout not found; run corpus/legacy/fetch.sh (or set ELUDITE_LEGACY_CORPUS).");
        await using var host = await WireHost.OpenAsync(project!);

        var started = await host.StartAsync(new { target = "build" });
        Assert.Equal("mono", started.GetProperty("toolchain").GetProperty("kind").GetString());
        var finished = await host.FinishedAsync();

        Assert.NotEqual("canceled", finished.GetProperty("result").GetString());
        Assert.NotEmpty(finished.GetProperty("projects").EnumerateArray());
        Assert.DoesNotContain(host.Text.Split('\n'), l => ConsoleLines.IsStackFrame(l));
    }

    [Fact]
    public async Task OutputPipe_SendsOrderedWholeLineChunks_AndCoalescesWhileTheSenderIsSlow()
    {
        var chunks = new List<(long Seq, string Text)>();
        var gate = new SemaphoreSlim(0);
        var slow = true;
        var pipe = new OutputPipe(async (seq, text) =>
        {
            lock (chunks)
            {
                chunks.Add((seq, text));
            }

            if (slow)
            {
                await gate.WaitAsync(Ct);
            }
        });
        await pipe.WriteLineAsync("first", Ct);
        // The first chunk is held by the sender; 5000 lines queue behind it.
        while (true)
        {
            lock (chunks)
            {
                if (chunks.Count == 1)
                {
                    break;
                }
            }

            await Task.Delay(1, Ct);
        }

        for (var i = 0; i < 5000; i++)
        {
            await pipe.WriteLineAsync($"line {i}", Ct);
        }

        slow = false;
        gate.Release();
        await pipe.CompleteAsync();

        Assert.Equal(Enumerable.Range(0, chunks.Count).Select(i => (long)i), chunks.Select(c => c.Seq));
        Assert.Equal("first\n", chunks[0].Text);
        var all = string.Concat(chunks.Select(c => c.Text));
        Assert.Equal("first\n" + string.Concat(Enumerable.Range(0, 5000).Select(i => $"line {i}\n")), all);
        Assert.All(chunks, c => Assert.EndsWith("\n", c.Text, StringComparison.Ordinal));
        // The backlog went out as one coalesced chunk (about 54 KB), larger than the 16 KiB of an unhurried chunk.
        Assert.True(chunks.Count <= 3, $"{chunks.Count} chunks");
        Assert.Contains(chunks, c => c.Text.Length > OutputPipe.ChunkBytes);
    }

    [Fact]
    public async Task OutputPipe_FlushesALoneLineWithinTheDelay_AndCapsUnhurriedChunks()
    {
        var times = new List<(TimeSpan At, string Text)>();
        var sw = Stopwatch.StartNew();
        var pipe = new OutputPipe((_, text) =>
        {
            lock (times)
            {
                times.Add((sw.Elapsed, text));
            }

            return Task.CompletedTask;
        });
        await pipe.WriteLineAsync("hello", Ct);
        while (true)
        {
            lock (times)
            {
                if (times.Count == 1)
                {
                    break;
                }
            }

            await Task.Delay(1, Ct);
        }

        Assert.True(times[0].At < TimeSpan.FromMilliseconds(500), $"{times[0].At.TotalMilliseconds} ms");
        var line = new string('x', 999);
        for (var i = 0; i < 100; i++)
        {
            await pipe.WriteLineAsync(line, Ct);
        }

        await pipe.CompleteAsync();
        Assert.All(times.Skip(1).SkipLast(1), t => Assert.InRange(t.Text.Length, OutputPipe.ChunkBytes, OutputPipe.ChunkBytes + 1000));
        Assert.Equal(100_000 + 6, times.Sum(t => t.Text.Length));
    }

    [Theory]
    [InlineData("/s/App/Program.cs(6,34): error CS0103: The name 'missing' does not exist in the current context [/s/App/App.csproj]", "error", "CS0103", "/s/App/Program.cs", 6, 34, "/s/App/App.csproj")]
    [InlineData("/s/App/Program.cs(5,13): warning CS0168: The variable 'unused' is declared but never used [/s/App/App.csproj]", "warning", "CS0168", "/s/App/Program.cs", 5, 13, "/s/App/App.csproj")]
    [InlineData("/s/App/App.csproj : error MSB4019: The imported project \"/x/Microsoft.WebApplication.targets\" was not found. [/s/App/App.csproj]", "error", "MSB4019", "/s/App/App.csproj", null, null, "/s/App/App.csproj")]
    [InlineData("MSBUILD : error MSB1009: Project file does not exist.", "error", "MSB1009", null, null, null, null)]
    [InlineData(@"C:\s\A.cs(1,2,3,4): error CS1002: ; expected [C:\s\A.csproj]", "error", "CS1002", @"C:\s\A.cs", 1, 2, @"C:\s\A.csproj")]
    public void ConsoleLines_ParseCanonicalDiagnostics(string line, string severity, string code, string? file, int? l, int? c, string? project)
    {
        var d = ConsoleLines.ParseDiagnostic(line);
        Assert.NotNull(d);
        Assert.Equal(severity, d.Severity);
        Assert.Equal(code, d.Code);
        Assert.Equal(file, d.File);
        Assert.Equal(l, d.Line);
        Assert.Equal(c, d.Column);
        Assert.Equal(project, d.Project);
    }

    [Fact]
    public void ConsoleLines_RecognizeProjectOutputsAndStackFrames()
    {
        Assert.Equal("Eludite.Web", ConsoleLines.ParseProjectOutput("  Eludite.Web -> /s/bin/Debug/net10.0/Eludite.Web.dll"));
        Assert.Null(ConsoleLines.ParseProjectOutput("  Determining projects to restore..."));
        Assert.Null(ConsoleLines.ParseDiagnostic("    0 Error(s)"));
        Assert.Null(ConsoleLines.ParseDiagnostic("Build succeeded."));
        Assert.True(ConsoleLines.IsStackFrame("   at Microsoft.Build.Tasks.ResolveComReference.Execute() in /src/x.cs:line 4"));
        Assert.True(ConsoleLines.IsStackFrame("   --- End of inner exception stack trace ---"));
        Assert.False(ConsoleLines.IsStackFrame("  App -> /s/App.dll"));
    }

    [Fact]
    public void WindowsOnlyTargets_ReplaceRawErrorsWithOneDiagnosticPerProject()
    {
        Assert.SkipWhen(OperatingSystem.IsWindows(), "Classification applies off Windows only.");
        BuildDiagnostic D(string code, string message, string project, string severity = "error") => new(severity, code, message) { Project = project };
        var raw = new List<BuildDiagnostic>
        {
            D("CS0103", "x", "/s/A.csproj"),
            D("MSB3073", "The command \"copy a b\" exited with code 127.", "/s/A.csproj"),
            D("MSB3073", "The command \"xcopy c d\" exited with code 127.", "/s/A.csproj"),
            D("MSB3283", "Cannot find wrapper assembly for type library \"IWshRuntimeLibrary\".", "/s/B.csproj", "warning"),
            D("MSB4019", "The imported project \"/x/Microsoft.Web.Publishing.targets\" was not found.", "/s/C.csproj"),
            D("MSB4019", "The imported project \"/x/Microsoft.WebApplication.targets\" was not found.", "/s/D.csproj"),
            D("MSB3073", "The command \"make\" exited with code 2.", "/s/E.csproj"),
        };
        var targets = new Dictionary<BuildDiagnostic, string?>(ReferenceEqualityComparer.Instance) { [raw[1]] = "PreBuildEvent", [raw[2]] = "PreBuildEvent" };
        var (diagnostics, replacements) = WindowsOnlyTargets.Replace(raw, d => targets.GetValueOrDefault(d));

        Assert.Equal(["CS0103", "ELUDITE0108", "ELUDITE0101", "ELUDITE0102", "ELUDITE0103", "MSB3073"], diagnostics.Select(d => d.Code));
        Assert.Equal(4, replacements.Count);
        var buildEvent = diagnostics[1];
        Assert.Equal("error", buildEvent.Severity);
        Assert.Equal($"The pre-build event of A is a Windows command script (cmd.exe) and cannot run on {WindowsOnlyTargets.ThisOs}.", buildEvent.Message);
        Assert.Equal("/s/A.csproj", buildEvent.Project);
        var com = diagnostics[2];
        Assert.Equal("warning", com.Severity);
        Assert.StartsWith("COM reference 'IWshRuntimeLibrary' was skipped", com.Message, StringComparison.Ordinal);
        Assert.StartsWith("C needs Visual Studio's web publishing targets", diagnostics[3].Message, StringComparison.Ordinal);
        // Without the log's target, exit code 127 (command not found) still means a Windows command.
        Assert.Equal(WindowsOnlyTargets.BuildEvent, WindowsOnlyTargets.Classify(D("MSB3073", "The command \"copy a b\" exited with code 127.", "/s/A.csproj"), null));
        Assert.Null(WindowsOnlyTargets.Classify(D("MSB3073", "The command \"make\" exited with code 2.", "/s/A.csproj"), "Build"));
    }

    // Fixtures.

    private static string SdkFixture(string root, string program)
    {
        Write(root, "App.slnx", "<Solution>\n  <Project Path=\"App/App.csproj\" />\n  <Project Path=\"Lib/Lib.csproj\" />\n</Solution>\n");
        Write(root, "Directory.Build.props", "<Project>\n  <PropertyGroup>\n    <TargetFramework>net10.0</TargetFramework>\n    <ImplicitUsings>disable</ImplicitUsings>\n    <Nullable>disable</Nullable>\n  </PropertyGroup>\n</Project>\n");
        Write(root, "App/App.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n  </PropertyGroup>\n  <ItemGroup>\n    <ProjectReference Include=\"../Lib/Lib.csproj\" />\n  </ItemGroup>\n</Project>\n");
        Write(root, "App/Program.cs", program);
        Write(root, "Lib/Lib.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\" />\n");
        Write(root, "Lib/Greeting.cs", "namespace Lib { public static class Greeting { public const string Text = \"hi\"; } }\n");
        return Path.Combine(root, "App.slnx");
    }

    private static string SlowFixture(string root, string marker)
    {
        var seconds = 60 + (Math.Abs(marker.GetHashCode(StringComparison.Ordinal)) % 1000 / 1000.0);
        var sleep = OperatingSystem.IsWindows()
            ? "ping -n 61 127.0.0.1 &gt; nul"
            : $"sh -c 'sleep {seconds:0.000}; true' {marker}";
        Write(root, "App.slnx", "<Solution>\n  <Project Path=\"App/App.csproj\" />\n</Solution>\n");
        Write(root, "App/App.csproj", $"""
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework>
                <OutputType>Exe</OutputType>
              </PropertyGroup>
              <Target Name="EluditeSlow" BeforeTargets="CoreCompile">
                <Message Importance="high" Text="{marker} sleeping" />
                <Exec Command="{sleep}" />
              </Target>
            </Project>
            """);
        Write(root, "App/Program.cs", "class Program { static void Main() { } }\n");
        return Path.Combine(root, "App.slnx");
    }

    private static string LegacyFixture(string root)
    {
        Write(root, "Legacy/Class1.cs", "namespace Legacy { public class Class1 { } }\n");
        Write(root, "Legacy/Legacy.csproj", """
            <?xml version="1.0" encoding="utf-8"?>
            <Project ToolsVersion="15.0" DefaultTargets="Build" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
              <Import Project="$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props" Condition="Exists('$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props')" />
              <PropertyGroup>
                <Configuration Condition=" '$(Configuration)' == '' ">Debug</Configuration>
                <Platform Condition=" '$(Platform)' == '' ">AnyCPU</Platform>
                <OutputType>Library</OutputType>
                <RootNamespace>Legacy</RootNamespace>
                <AssemblyName>Legacy</AssemblyName>
                <TargetFrameworkVersion>v4.8</TargetFrameworkVersion>
                <OutputPath>bin\$(Configuration)\</OutputPath>
              </PropertyGroup>
              <ItemGroup>
                <Reference Include="System" />
              </ItemGroup>
              <ItemGroup>
                <Compile Include="Class1.cs" />
              </ItemGroup>
              <Import Project="$(MSBuildToolsPath)\Microsoft.CSharp.targets" />
              <PropertyGroup>
                <PreBuildEvent>copy "$(ProjectDir)settings.template" "$(ProjectDir)settings.txt"</PreBuildEvent>
              </PropertyGroup>
            </Project>
            """);
        return Path.Combine(root, "Legacy", "Legacy.csproj");
    }

    private static void Write(string root, string relative, string text)
    {
        var path = Path.Combine(root, relative.Replace('/', Path.DirectorySeparatorChar));
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text, new UTF8Encoding(false));
    }

    private static List<int> ProcessesWith(string marker)
    {
        var found = new List<int>();
        foreach (var dir in Directory.EnumerateDirectories("/proc"))
        {
            if (!int.TryParse(Path.GetFileName(dir), out var pid))
            {
                continue;
            }

            try
            {
                if (File.ReadAllText(Path.Combine(dir, "cmdline")).Contains(marker, StringComparison.Ordinal))
                {
                    found.Add(pid);
                }
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
            }
        }

        return found;
    }

    private static string? FindCorpusProject()
    {
        const string relative = "aspnet-samples/samples/aspnet/Identity/ChangePK/PrimaryKeysConfigTest/PrimaryKeysConfigTest.csproj";
        var fromEnv = Environment.GetEnvironmentVariable("ELUDITE_LEGACY_CORPUS");
        if (!string.IsNullOrEmpty(fromEnv))
        {
            var p = Path.Combine(fromEnv, relative);
            return File.Exists(p) ? p : null;
        }

        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "corpus", "legacy", ".checkout", relative);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        return null;
    }

    private sealed class TempDir : IDisposable
    {
        public TempDir()
        {
            Path = System.IO.Path.Combine(System.IO.Path.GetTempPath(), "eludite-build-test-" + Guid.NewGuid().ToString("N")[..12]);
            Directory.CreateDirectory(Path);
        }

        public string Path { get; }

        public void Dispose()
        {
            try
            {
                Directory.Delete(Path, recursive: true);
            }
            catch (IOException)
            {
            }
            catch (UnauthorizedAccessException)
            {
            }
        }
    }

    /// <summary>A host without a language server over an in-memory stream, recording the build notifications.</summary>
    private sealed class WireHost : IAsyncDisposable
    {
        private readonly Task<int> _server;
        private readonly List<JsonElement> _output = [];
        private readonly List<JsonElement> _progress = [];
        private readonly Channel<JsonElement> _finished = Channel.CreateUnbounded<JsonElement>();

        public WireHost()
        {
            var (clientStream, serverStream) = FullDuplexStream.CreatePair();
            Target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, build: new BuildService(
                () => Target!.LanguageServer.CurrentSolution(),
                TextWriter.Null,
                logDirectory: System.IO.Path.Combine(System.IO.Path.GetTempPath(), "eludite-build-tests")));
            _server = HostServer.RunAsync(serverStream, serverStream, Target);
            Client = TestRpc.Create(clientStream);
            TestRpc.On(Client, "eludite/build/output", p => { lock (_output) { _output.Add(p.Clone()); } });
            TestRpc.On(Client, "eludite/build/progress", p => { lock (_progress) { _progress.Add(p.Clone()); } });
            TestRpc.On(Client, "eludite/build/finished", p => _finished.Writer.TryWrite(p.Clone()));
            Client.StartListening();
        }

        public static async Task<WireHost> OpenAsync(string solution)
        {
            var host = new WireHost();
            await host.InitializeAsync();
            await host.Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = solution }, Ct);
            return host;
        }

        public HostRpcTarget Target { get; }

        public JsonRpc Client { get; }

        public List<JsonElement> Output
        {
            get
            {
                lock (_output)
                {
                    return [.. _output];
                }
            }
        }

        public List<JsonElement> Progress
        {
            get
            {
                lock (_progress)
                {
                    return [.. _progress];
                }
            }
        }

        public string Text => string.Concat(Output.Select(c => c.GetProperty("text").GetString()));

        public Task InitializeAsync() =>
            Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "test", clientVersion = "0" }, Ct);

        public Task<JsonElement> StartAsync(object parameters) =>
            Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/build/start", parameters, Ct);

        public async Task<JsonElement> FinishedAsync() =>
            await _finished.Reader.ReadAsync(Ct).AsTask().WaitAsync(BuildTimeout, Ct);

        public async Task WaitForOutputAsync(string text)
        {
            var deadline = Stopwatch.StartNew();
            while (!Text.Contains(text, StringComparison.Ordinal))
            {
                Assert.True(deadline.Elapsed < BuildTimeout, $"no output line with {text}");
                await Task.Delay(20, Ct);
            }
        }

        public async ValueTask DisposeAsync()
        {
            await Target.Build.Running.WaitAsync(TimeSpan.FromSeconds(30));
            Client.Dispose();
            await _server.WaitAsync(TimeSpan.FromSeconds(10));
        }
    }
}
