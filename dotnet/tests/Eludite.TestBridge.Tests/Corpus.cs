using System.Diagnostics;
using System.Globalization;
using System.Text;

namespace Eludite.TestBridge.Tests;

/// <summary>
/// Brief 0035's test corpus (<c>corpus/tests</c>) copied to a temp folder and built there once per test process, with a
/// generated <c>Corpus.Many</c> project (xunit.v3 on MTP, <see cref="ManyTests"/> facts) for the discovery budget. Shared
/// by the bridge's and the host's tests (the host's project links this file).
/// </summary>
internal static class Corpus
{
    /// <summary>How many facts <c>Corpus.Many</c> has.</summary>
    public const int ManyTests = 100;

    public static readonly string[] Projects = ["Corpus.XunitV3", "Corpus.Xunit2", "Corpus.MSTest", "Corpus.NUnit", "Corpus.Many"];

    private static readonly Lazy<Task<string>> Built = new(BuildAsync);

    /// <summary>The folder with the built corpus and its <c>Corpus.slnx</c>.</summary>
    public static Task<string> DirectoryAsync() => Built.Value;

    public static string Project(string root, string name) => Path.Combine(root, name, name + ".csproj");

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
            slnx.Append(CultureInfo.InvariantCulture, $"  <Project Path=\"{p}/{p}.csproj\" />\n");
        }

        slnx.Append("</Solution>\n");
        await File.WriteAllTextAsync(Path.Combine(root, "Corpus.slnx"), slnx.ToString());
        var psi = new ProcessStartInfo(Dotnet(), ["build", Path.Combine(root, "Corpus.slnx"), "--configuration", "Debug", "--nologo", "-v", "q"])
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
        await process.WaitForExitAsync();
        if (process.ExitCode != 0)
        {
            throw new InvalidOperationException($"building the corpus failed ({process.ExitCode}):\n{await stdout}\n{await stderr}");
        }

        return root;
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
