using Eludite.Host.Legacy;
using Eludite.TestBridge;

namespace Eludite.Host.Testing;

/// <summary>
/// The test containers of a solution (host-rpc.md, "Tests", Containers): each test project
/// (<see cref="TestProjectEvaluator"/>, brief 0076) once per target framework, with its build output.
/// </summary>
public static class TestContainers
{
    /// <summary>The containers of the evaluated test projects <paramref name="projects"/> (in solution order).</summary>
    public static IReadOnlyList<(TestContainer Container, TestContainerInfo Info)> Of(IEnumerable<TestProjectEvaluation> projects, Settings settings)
    {
        ArgumentNullException.ThrowIfNull(projects);
        ArgumentNullException.ThrowIfNull(settings);
        var list = new List<(TestContainer, TestContainerInfo)>();
        foreach (var evaluation in projects)
        {
            var project = evaluation.Project;
            var name = Path.GetFileNameWithoutExtension(project);
            var frameworks = evaluation.Outputs;
            foreach (var output in frameworks)
            {
                var tfm = output.TargetFramework;
                var program = output.Program;
                var netfx = TestProjectInspector.IsNetFramework(tfm);
                var mtp = evaluation.Protocol == TestRunnerProtocol.MicrosoftTestingPlatform;
                var runtime = !netfx ? TestRuntime.Dotnet : OperatingSystem.IsWindows() ? TestRuntime.NetFx : TestRuntime.Mono;
                string? error = null;
                if (!File.Exists(program))
                {
                    error = $"not built: {program} does not exist; build the project first";
                }
                else if (runtime == TestRuntime.Mono && !mtp)
                {
                    error = "VSTest runs .NET Framework tests on Windows only; move the project to Microsoft.Testing.Platform to run it under Mono";
                }
                else if (runtime == TestRuntime.Mono && settings.Mono is null)
                {
                    error = ".NET Framework tests need Mono off Windows, and no mono was found (ELUDITE_MONO_PREFIX, PATH, /usr)";
                }

                var display = frameworks.Count > 1 ? $"{name} ({tfm})" : name;
                var id = $"{project}|{tfm}";
                var container = new TestContainer(id, project, tfm, evaluation.Protocol, program, runtime)
                {
                    Name = display,
                    MonoExecutable = settings.Mono,
                    DotnetExecutable = settings.Dotnet,
                    RunSettings = settings.RunSettings,
                    VsTestConsole = settings.VsTestConsole,
                    Parallel = settings.Parallel,
                    Coverage = output.Coverage,
                    CoverageAdapterPath = output.CoverageAdapterPath,
                };
                list.Add((container, new TestContainerInfo(id, display, project, tfm, mtp ? "mtp" : "vstest")
                {
                    Runtime = runtime switch
                    {
                        TestRuntime.Mono => "mono",
                        TestRuntime.NetFx => "netfx",
                        _ => "dotnet",
                    },
                    Program = program,
                    Error = error,
                    Coverage = CoverageCollection.Name(output.Coverage),
                }));
            }
        }

        return list;
    }

    /// <summary>The located <c>mono</c>: <c>ELUDITE_MONO_PREFIX/bin/mono</c>, <c>mono</c> on PATH, then the usual prefixes.</summary>
    public static string? LocateMono()
    {
        if (OperatingSystem.IsWindows())
        {
            return null;
        }

        var candidates = new List<string>();
        if (Environment.GetEnvironmentVariable("ELUDITE_MONO_PREFIX") is { Length: > 0 } prefix)
        {
            candidates.Add(Path.Combine(prefix, "bin", "mono"));
        }

        candidates.AddRange((Environment.GetEnvironmentVariable("PATH") ?? string.Empty)
            .Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries)
            .Select(d => Path.Combine(d, "mono")));
        candidates.Add(Path.Combine(MonoInstallation.UserSpacePrefix, "bin", "mono"));
        candidates.AddRange(["/usr/bin/mono", "/usr/local/bin/mono", "/Library/Frameworks/Mono.framework/Versions/Current/bin/mono"]);
        return candidates.FirstOrDefault(File.Exists);
    }

    /// <summary>The <c>dotnet</c> that runs CoreCLR test applications and vstest.console.</summary>
    public static string LocateDotnet()
    {
        if (Environment.GetEnvironmentVariable("DOTNET_HOST_PATH") is { Length: > 0 } hostPath && File.Exists(hostPath))
        {
            return hostPath;
        }

        if (Environment.ProcessPath is { } self && Path.GetFileNameWithoutExtension(self) == "dotnet")
        {
            return self;
        }

        if (Environment.GetEnvironmentVariable("DOTNET_ROOT") is { Length: > 0 } root)
        {
            var exe = Path.Combine(root, OperatingSystem.IsWindows() ? "dotnet.exe" : "dotnet");
            if (File.Exists(exe))
            {
                return exe;
            }
        }

        return "dotnet";
    }

    /// <summary>Where the runners come from for one request.</summary>
    public sealed record Settings(string? Mono, string Dotnet)
    {
        /// <summary>Brief 0076: the solution platform (<c>x64</c>, <c>Any CPU</c>); null for the projects' default.</summary>
        public string? Platform { get; init; }

        public string? RunSettings { get; init; }

        public string? VsTestConsole { get; init; }

        public bool Parallel { get; init; }
    }
}
