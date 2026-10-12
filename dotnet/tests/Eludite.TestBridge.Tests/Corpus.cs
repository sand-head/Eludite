using System.Diagnostics;
using System.Globalization;
using System.Text;

namespace Eludite.TestBridge.Tests;

/// <summary>
/// Brief 0035's test corpus (<c>corpus/tests</c>) copied to a temp folder and built there once per test process, with a
/// generated <c>Corpus.Many</c> project (xunit.v3 on MTP, <see cref="ManyTests"/> facts) for the discovery budget. Shared
/// by the bridge's and the host's tests (the host's project links this file). Brief 0063 adds the Visual Basic
/// (<c>.vbproj</c>) and F# (<c>.fsproj</c>) projects; <see cref="Project"/> knows each project's file extension. Brief
/// 0076's variants (<c>corpus/tests/Variants</c>) are copied and built in a folder of their own, on first use only
/// (<see cref="VariantsAsync"/>).
/// </summary>
internal static class Corpus
{
    /// <summary>How many facts <c>Corpus.Many</c> has.</summary>
    public const int ManyTests = 100;

    /// <summary>The projects in the generated solution's order.</summary>
    public static readonly string[] Projects = ["Corpus.XunitV3", "Corpus.Xunit2", "Corpus.MSTest", "Corpus.NUnit", "Corpus.VisualBasic", "Corpus.FSharp", "Corpus.Many"];

    /// <summary>The project file's extension per project: C# unless named here.</summary>
    private static readonly Dictionary<string, string> Extensions = new(StringComparer.Ordinal)
    {
        ["Corpus.VisualBasic"] = ".vbproj",
        ["Corpus.FSharp"] = ".fsproj",
    };

    /// <summary>Brief 0076: the variants' projects in their solution's order (<c>Corpus.Calculator</c> is not a test project).</summary>
    public static readonly string[] Variants = ["Corpus.PropsDriven", "Corpus.Platform64", "Corpus.Artifacts", "Corpus.Coverage", "Corpus.Coverlet", "Corpus.Calculator"];

    private static readonly Lazy<Task<string>> Built = new(BuildAsync);

    private static readonly Lazy<Task<string>> BuiltVariants = new(BuildVariantsAsync);

    /// <summary>The folder with the built corpus and its <c>Corpus.slnx</c>.</summary>
    public static Task<string> DirectoryAsync() => Built.Value;

    /// <summary>
    /// Brief 0076: the folder with the built variants (<c>Variants/</c> beside the corpus's Directory.Build.props and
    /// Directory.Packages.props) and their <c>Variants/Variants.slnx</c>; <c>Corpus.Platform64</c> is built with
    /// <c>Platform</c> x64.
    /// </summary>
    public static Task<string> VariantsAsync() => BuiltVariants.Value;

    /// <summary>The project file (<c>.csproj</c>, <c>.vbproj</c> or <c>.fsproj</c>) of the named corpus project.</summary>
    public static string Project(string root, string name) => Path.Combine(root, name, name + Extension(name));

    /// <summary>The project file extension of the named corpus project, with its dot.</summary>
    public static string Extension(string name) => Extensions.GetValueOrDefault(name, ".csproj");

    public static string Output(string root, string name, string tfm, string extension = ".dll") =>
        Path.Combine(root, name, "bin", "Debug", tfm, name + extension);

    /// <summary>The repository's <c>corpus/tests</c>, found above the test assembly.</summary>
    public static string Source()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "corpus", "tests");
            if (File.Exists(Path.Combine(candidate, "Directory.Packages.props")))
            {
                return candidate;
            }
        }

        throw new InvalidOperationException("corpus/tests not found above " + AppContext.BaseDirectory);
    }

    /// <summary>The <c>dotnet</c> running these tests (its SDK builds the corpus).</summary>
    public static string Dotnet()
    {
        if (Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") is { Length: > 0 } host && File.Exists(host))
        {
            return host;
        }

        if (Environment.GetEnvironmentVariable("DOTNET_ROOT") is { Length: > 0 } root
            && Path.Combine(root, OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet") is var exe
            && File.Exists(exe))
        {
            return exe;
        }

        return "dotnet";
    }

    private static async Task<string> BuildAsync()
    {
        var source = Source();
        var root = Path.Combine(Path.GetTempPath(), "eludite-test-corpus-" + Guid.NewGuid().ToString("N")[..12]);
        Copy(source, root);
        GenerateMany(root);
        var slnx = new StringBuilder("<Solution>\n");
        foreach (var p in Projects)
        {
            slnx.Append(CultureInfo.InvariantCulture, $"  <Project Path=\"{p}/{p}{Extension(p)}\" />\n");
        }

        slnx.Append("</Solution>\n");
        await File.WriteAllTextAsync(Path.Combine(root, "Corpus.slnx"), slnx.ToString());
        // A build that outlives this bound is hung (a stuck restore, a wedged MSBuild node), not slow: it is killed and
        // reported rather than left to hang every test that needs the corpus. One NuGet network failure (NU1301, the
        // feed unreachable) is retried once: that is nuget.org, not the code under test.
        var bound = Budget.Hang(TimeSpan.FromMinutes(10));
        for (var attempt = 1; ; attempt++)
        {
            var (code, output) = await DotnetAsync(root, bound, ["build", Path.Combine(root, "Corpus.slnx"), "--configuration", "Debug", "--nologo", "-v", "q"]);
            if (code == 0)
            {
                break;
            }

            if (attempt == 1 && output.Contains("NU1301", StringComparison.Ordinal))
            {
                Console.Error.WriteLine($"building the corpus hit a NuGet network failure, retrying once:\n{output}");
                continue;
            }

            throw new InvalidOperationException($"building the corpus failed ({code}):\n{output}");
        }

        return root;
    }

    /// <summary>Brief 0076: the variants copied beside the corpus's props and built, the x64 one for x64.</summary>
    private static async Task<string> BuildVariantsAsync()
    {
        var source = Source();
        var root = Path.Combine(Path.GetTempPath(), "eludite-test-variants-" + Guid.NewGuid().ToString("N")[..12]);
        Directory.CreateDirectory(root);
        foreach (var file in Directory.GetFiles(source))
        {
            File.Copy(file, Path.Combine(root, Path.GetFileName(file)));
        }

        var variants = Path.Combine(root, "Variants");
        CopyTree(Path.Combine(source, "Variants"), variants);
        var slnx = new StringBuilder("<Solution>\n");
        foreach (var p in Variants)
        {
            slnx.Append(CultureInfo.InvariantCulture, $"  <Project Path=\"{p}/{p}.csproj\" />\n");
        }

        slnx.Append("</Solution>\n");
        await File.WriteAllTextAsync(Path.Combine(variants, "Variants.slnx"), slnx.ToString());

        // Everything but the x64 project in one build (Corpus.Calculator through its references), then that one for x64.
        var anyCpu = new StringBuilder("<Solution>\n");
        foreach (var p in Variants.Where(v => v is not "Corpus.Platform64" and not "Corpus.Calculator"))
        {
            anyCpu.Append(CultureInfo.InvariantCulture, $"  <Project Path=\"{p}/{p}.csproj\" />\n");
        }

        anyCpu.Append("</Solution>\n");
        await File.WriteAllTextAsync(Path.Combine(variants, "Build.slnx"), anyCpu.ToString());
        var bound = Budget.Hang(TimeSpan.FromMinutes(10));
        string[][] builds =
        [
            ["build", Path.Combine(variants, "Build.slnx"), "--configuration", "Debug", "--nologo", "-v", "q"],
            ["build", Path.Combine(variants, "Corpus.Platform64", "Corpus.Platform64.csproj"), "--configuration", "Debug", "-p:Platform=x64", "--nologo", "-v", "q"],
        ];
        foreach (var args in builds)
        {
            for (var attempt = 1; ; attempt++)
            {
                var (code, output) = await DotnetAsync(variants, bound, args);
                if (code == 0)
                {
                    break;
                }

                if (attempt == 1 && output.Contains("NU1301", StringComparison.Ordinal))
                {
                    Console.Error.WriteLine($"building the variants hit a NuGet network failure, retrying once:\n{output}");
                    continue;
                }

                throw new InvalidOperationException($"building the variants failed ({code}):\n{output}");
            }
        }

        return root;
    }

    private static void CopyTree(string from, string to)
    {
        Directory.CreateDirectory(to);
        foreach (var file in Directory.GetFiles(from))
        {
            File.Copy(file, Path.Combine(to, Path.GetFileName(file)));
        }

        foreach (var dir in Directory.GetDirectories(from))
        {
            if (Path.GetFileName(dir) is "bin" or "obj" or "artifacts" or "TestResults")
            {
                continue;
            }

            CopyTree(dir, Path.Combine(to, Path.GetFileName(dir)));
        }
    }

    /// <summary>Runs <c>dotnet</c> in <paramref name="root"/>; the exit code and its output. Killed with its process tree after <paramref name="bound"/>.</summary>
    private static async Task<(int Code, string Output)> DotnetAsync(string root, TimeSpan bound, string[] args)
    {
        var psi = new ProcessStartInfo(Dotnet(), args)
        {
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            WorkingDirectory = root,
            Environment = { ["DOTNET_CLI_TELEMETRY_OPTOUT"] = "1", ["DOTNET_NOLOGO"] = "1", ["MSBUILDDISABLENODEREUSE"] = "1" },
        };
        using var process = Process.Start(psi)!;
        var stdout = process.StandardOutput.ReadToEndAsync();
        var stderr = process.StandardError.ReadToEndAsync();
        using var timeout = new CancellationTokenSource(bound);
        try
        {
            await process.WaitForExitAsync(timeout.Token);
        }
        catch (OperationCanceledException)
        {
            process.Kill(entireProcessTree: true);
            await process.WaitForExitAsync();
            throw new TimeoutException($"dotnet {string.Join(' ', args)} did not finish within {bound.TotalMinutes:F0} min:\n{await stdout}\n{await stderr}");
        }

        return (process.ExitCode, $"{await stdout}\n{await stderr}");
    }

    private static void Copy(string from, string to)
    {
        Directory.CreateDirectory(to);
        foreach (var file in Directory.GetFiles(from))
        {
            File.Copy(file, Path.Combine(to, Path.GetFileName(file)));
        }

        foreach (var dir in Directory.GetDirectories(from))
        {
            var name = Path.GetFileName(dir);
            if (name is "bin" or "obj" or "target" || !name.StartsWith("Corpus.", StringComparison.Ordinal))
            {
                continue;
            }

            Copy(dir, Path.Combine(to, name));
        }
    }

    /// <summary><c>Corpus.Many</c>: Corpus.XunitV3's project file for net10.0 only, and <see cref="ManyTests"/> generated facts.</summary>
    private static void GenerateMany(string root)
    {
        var dir = Path.Combine(root, "Corpus.Many");
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, "Corpus.Many.csproj"), """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <OutputType>Exe</OutputType>
                <TargetFramework>net10.0</TargetFramework>
                <UseMicrosoftTestingPlatformRunner>true</UseMicrosoftTestingPlatformRunner>
              </PropertyGroup>
              <ItemGroup>
                <PackageReference Include="xunit.v3" />
              </ItemGroup>
            </Project>
            """);
        var code = new StringBuilder("using Xunit;\n\nnamespace Corpus.Many\n{\n    public class ManyTests\n    {\n");
        for (var i = 0; i < ManyTests; i++)
        {
            code.Append(CultureInfo.InvariantCulture, $"        [Fact]\n        public void Test{i:D3}() {{ Assert.True({i} >= 0); }}\n\n");
        }

        code.Append("    }\n}\n");
        File.WriteAllText(Path.Combine(dir, "ManyTests.cs"), code.ToString());
    }
}

/// <summary>A sink that records everything, for the runners' tests.</summary>
internal sealed class RecordingSink : ITestSink
{
    private readonly Lock _lock = new();
    private readonly List<DiscoveredTest> _tests = [];
    private readonly List<TestResult> _results = [];
    private readonly List<string> _log = [];
    private readonly TaskCompletionSource<TestLaunch> _launch = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly TaskCompletionSource<int> _attach = new(TaskCreationOptions.RunContinuationsAsynchronously);
    private readonly TaskCompletionSource _firstResult = new(TaskCreationOptions.RunContinuationsAsynchronously);

    /// <summary>What <see cref="AttachAsync"/> answers.</summary>
    public (bool, string?) AttachAnswer { get; set; } = (true, null);

    public List<DiscoveredTest> Found
    {
        get
        {
            lock (_lock)
            {
                return [.. _tests];
            }
        }
    }

    public List<TestResult> AllResults
    {
        get
        {
            lock (_lock)
            {
                return [.. _results];
            }
        }
    }

    /// <summary>The last result per test.</summary>
    public Dictionary<string, TestResult> Final
    {
        get
        {
            lock (_lock)
            {
                var d = new Dictionary<string, TestResult>(StringComparer.Ordinal);
                foreach (var r in _results.Where(r => r.Outcome != TestOutcomes.Running))
                {
                    d[r.Id] = r;
                }

                return d;
            }
        }
    }

    public string LogText
    {
        get
        {
            lock (_lock)
            {
                return string.Join('\n', _log);
            }
        }
    }

    public Task<TestLaunch> Launched => _launch.Task;

    public Task<int> Attached => _attach.Task;

    public Task FirstResult => _firstResult.Task;

    public void Tests(IReadOnlyList<DiscoveredTest> tests)
    {
        lock (_lock)
        {
            _tests.AddRange(tests);
        }
    }

    public void Results(IReadOnlyList<TestResult> results)
    {
        lock (_lock)
        {
            _results.AddRange(results);
        }

        _firstResult.TrySetResult();
    }

    public void Log(string line)
    {
        lock (_lock)
        {
            _log.Add(line);
        }
    }

    public void Launch(TestLaunch launch) => _launch.TrySetResult(launch);

    public Task<(bool Attached, string? Message)> AttachAsync(int processId, CancellationToken cancellationToken)
    {
        _attach.TrySetResult(processId);
        return Task.FromResult(AttachAnswer);
    }
}
