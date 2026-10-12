using System.Diagnostics;
using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Eludite.Host.Projects;
using Eludite.TestBridge;
using Microsoft.Build.Evaluation;
using Microsoft.Build.Exceptions;
using MsbProject = Microsoft.Build.Evaluation.Project;

namespace Eludite.Host.Testing;

/// <summary>A test project's build output for one target framework, and the coverage collector it brings.</summary>
/// <param name="Program">The evaluated <c>TargetPath</c>: the test assembly, or the .exe of a .NET Framework executable.</param>
public sealed record TestProjectOutput(string TargetFramework, string Program)
{
    /// <summary>The coverage collector the output's evaluation references.</summary>
    public CoverageCollector Coverage { get; init; }

    /// <summary>A VSTest data collector's folder (<see cref="TestContainer.CoverageAdapterPath"/>).</summary>
    public string? CoverageAdapterPath { get; init; }
}

/// <summary>A test project as the Test Explorer runs it: its protocol and one output per target framework.</summary>
public sealed record TestProjectEvaluation(string Project, TestRunnerProtocol Protocol, IReadOnlyList<TestProjectOutput> Outputs)
{
    /// <summary>True when MSBuild evaluated it; false when the project file alone was read (<see cref="TestProjectInspector"/>).</summary>
    public bool Evaluated { get; init; }
}

/// <summary>
/// Brief 0076: the solution's test projects as MSBuild sees them. Each project is evaluated in-process with the .NET SDK's
/// MSBuild (the registration of brief 0003, under <see cref="MsBuildProjectTreeEvaluator.EvaluationLock"/>, one
/// <see cref="ProjectCollection"/> per call so the SDK's imports are parsed once) with <c>Configuration</c> and
/// <c>Platform</c> as global properties, then once per target framework with <c>TargetFramework</c> when it targets
/// several. A project is a test project when <c>IsTestProject</c> is true or it references a test package (wherever the
/// reference is declared: Directory.Build.props, an import, the project); the protocol follows
/// <see cref="TestProjectInspector.Detect(string)"/>'s precedence on the evaluated properties and package references
/// (TUnit is MTP); the program is the evaluated <c>TargetPath</c>, so <c>OutputPath</c>, <c>Platform</c>,
/// <c>RuntimeIdentifier</c>, <c>AppendTargetFrameworkToOutputPath</c> and the artifacts layout hold. When MSBuild is not
/// available, or a project does not evaluate, <see cref="TestProjectInspector"/> reads the project file and the program
/// is <c>bin/&lt;Configuration&gt;/&lt;tfm&gt;/</c>, as before brief 0076.
/// </summary>
public static class TestProjectEvaluator
{
    /// <summary>The MTP coverage extension's package (and its assembly beside a built test application).</summary>
    public const string MtpCoveragePackage = "Microsoft.Testing.Extensions.CodeCoverage";

    /// <summary>coverlet's VSTest data collector package.</summary>
    public const string CoverletPackage = "coverlet.collector";

    // Packages whose reference makes a project a test project besides the MTP ones and Microsoft.NET.Test.Sdk: the test
    // adapters (an MSTest, NUnit or xunit 2 project restored without Microsoft.NET.Test.Sdk is still one).
    private static readonly string[] TestPackages = ["MSTest", "MSTest.TestAdapter", "NUnit3TestAdapter", "xunit.runner.visualstudio"];

    /// <summary>
    /// The test projects among <paramref name="projects"/> (absolute paths, in their order; projects that are not test
    /// projects are left out) for <paramref name="configuration"/> and the solution <paramref name="platform"/> (null:
    /// the projects' default; <c>Any CPU</c> is MSBuild's <c>AnyCPU</c>). A project whose platforms do not include the
    /// one asked for is evaluated as a solution build maps it: its default platform when it has <c>AnyCPU</c>, else its
    /// first platform.
    /// </summary>
    public static IReadOnlyList<TestProjectEvaluation> Evaluate(IReadOnlyList<string> projects, string configuration, string? platform, TextWriter? log = null, CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(projects);
        ArgumentNullException.ThrowIfNull(configuration);
        var watch = Stopwatch.StartNew();
        List<TestProjectEvaluation> results;
        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            results = [.. projects.Select(p => FromFile(Path.GetFullPath(p), configuration)).OfType<TestProjectEvaluation>()];
            log?.WriteLine($"[test] no MSBuild: {results.Count} test project(s) of {projects.Count} read from their files");
            return results;
        }

        lock (MsBuildProjectTreeEvaluator.EvaluationLock)
        {
            results = EvaluateCore(projects, configuration, platform, log, cancellationToken);
        }

        log?.WriteLine($"[test] evaluated {projects.Count} project(s), {results.Count} test project(s) ({watch.ElapsedMilliseconds} ms)");
        return results;
    }

    /// <summary>
    /// The fallback: <see cref="TestProjectInspector"/> on the project file, the program in <c>bin/&lt;configuration&gt;/&lt;tfm&gt;/</c>
    /// (<c>net10.0</c> when the file names no framework); null when it is not a test project.
    /// </summary>
    public static TestProjectEvaluation? FromFile(string project, string configuration)
    {
        ArgumentNullException.ThrowIfNull(project);
        if (TestProjectInspector.Inspect(project) is not { } info)
        {
            return null;
        }

        var dir = Path.GetDirectoryName(project)!;
        var frameworks = info.TargetFrameworks.Count > 0 ? info.TargetFrameworks : ["net10.0"];
        var outputs = frameworks.Select(tfm =>
        {
            var extension = TestProjectInspector.IsNetFramework(tfm) && info.IsExe ? ".exe" : ".dll";
            var program = Path.Combine(dir, "bin", configuration, tfm, info.AssemblyName + extension);
            return new TestProjectOutput(tfm, program)
            {
                Coverage = info.Protocol == TestRunnerProtocol.MicrosoftTestingPlatform && HasMtpCoverage(program) ? CoverageCollector.Mtp : CoverageCollector.None,
            };
        }).ToList();
        return new TestProjectEvaluation(project, info.Protocol, outputs);
    }

    // Kept out of Evaluate so no Microsoft.Build type is touched before the locator has registered.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static List<TestProjectEvaluation> EvaluateCore(IReadOnlyList<string> projects, string configuration, string? platform, TextWriter? log, CancellationToken cancellationToken)
    {
        var results = new List<TestProjectEvaluation>();
        using var collection = new ProjectCollection(null, null, ToolsetDefinitionLocations.Default);
        foreach (var path in projects)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var full = Path.GetFullPath(path);
            if (!File.Exists(full))
            {
                continue;
            }

            if (SolutionProjects.IsLegacy(full))
            {
                // Legacy (non-SDK) projects need the design-time properties of brief 0003 to evaluate off Windows; their
                // test packages and output are read from the file, as before.
                if (FromFile(full, configuration) is { } legacy)
                {
                    results.Add(legacy);
                }

                continue;
            }

            try
            {
                if (EvaluateOne(collection, full, configuration, platform) is { } evaluation)
                {
                    results.Add(evaluation);
                }
            }
            catch (Exception ex) when (ex is InvalidProjectFileException or IOException or UnauthorizedAccessException)
            {
                log?.WriteLine($"[test] {full} does not evaluate, reading the project file: {ex.Message}");
                if (FromFile(full, configuration) is { } fallback)
                {
                    results.Add(fallback);
                }
            }
        }

        collection.UnloadAllProjects();
        return results;
    }

    private static TestProjectEvaluation? EvaluateOne(ProjectCollection collection, string full, string configuration, string? platform)
    {
        var globals = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase) { ["Configuration"] = configuration };
        if (platform is { Length: > 0 })
        {
            globals["Platform"] = ConfigurationCondition.ProjectPlatform(platform);
        }

        var project = Load(collection, full, globals);
        if (globals.TryGetValue("Platform", out var asked))
        {
            var platforms = Split(project.GetPropertyValue("Platforms"));
            if (platforms.Count > 0 && !platforms.Any(p => string.Equals(ConfigurationCondition.ProjectPlatform(p), asked, StringComparison.OrdinalIgnoreCase)))
            {
                // A solution build maps the project to a platform it has.
                collection.UnloadProject(project);
                if (platforms.Any(p => string.Equals(ConfigurationCondition.ProjectPlatform(p), "AnyCPU", StringComparison.OrdinalIgnoreCase)))
                {
                    globals.Remove("Platform");
                }
                else
                {
                    globals["Platform"] = ConfigurationCondition.ProjectPlatform(platforms[0]);
                }

                project = Load(collection, full, globals);
            }
        }

        if (Protocol(project) is not { } protocol)
        {
            collection.UnloadProject(project);
            return null;
        }

        var outputs = new List<TestProjectOutput>();
        var frameworks = Split(project.GetPropertyValue("TargetFrameworks"));
        if (project.GetPropertyValue("TargetFramework") is { Length: > 0 } single || frameworks.Count == 0)
        {
            outputs.Add(Output(project, project.GetPropertyValue("TargetFramework"), protocol));
            collection.UnloadProject(project);
        }
        else
        {
            collection.UnloadProject(project);
            foreach (var tfm in frameworks)
            {
                var inner = Load(collection, full, new Dictionary<string, string>(globals, StringComparer.OrdinalIgnoreCase) { ["TargetFramework"] = tfm });
                outputs.Add(Output(inner, tfm, protocol));
                collection.UnloadProject(inner);
            }
        }

        return new TestProjectEvaluation(full, protocol, outputs) { Evaluated = true };
    }

    private static MsbProject Load(ProjectCollection collection, string full, Dictionary<string, string> globals) =>
        new(full, globals, toolsVersion: null, collection, ProjectLoadSettings.IgnoreMissingImports | ProjectLoadSettings.IgnoreEmptyImports | ProjectLoadSettings.IgnoreInvalidImports);

    /// <summary>The protocol of an evaluated project, or null when it is not a test project.</summary>
    private static TestRunnerProtocol? Protocol(MsbProject project)
    {
        bool IsTrue(string name) => bool.TryParse(project.GetPropertyValue(name).Trim(), out var value) && value;
        var isTestProject = project.GetPropertyValue("IsTestProject").Trim();
        if (string.Equals(isTestProject, "false", StringComparison.OrdinalIgnoreCase))
        {
            return null;
        }

        var packages = Packages(project);
        var msTestSdk = (project.Xml.Sdk ?? string.Empty).Contains("MSTest.Sdk", StringComparison.OrdinalIgnoreCase)
            || project.Imports.Any(i => i.ImportedProject.FullPath.Contains("mstest.sdk", StringComparison.OrdinalIgnoreCase));
        var mtpPackage = packages.Any(TestProjectInspector.IsMtpPackage);
        var mtpProperty = TestProjectInspector.MtpProperties.Any(IsTrue);
        var exe = project.GetPropertyValue("OutputType").Trim() is var outputType
            && (string.Equals(outputType, "Exe", StringComparison.OrdinalIgnoreCase) || string.Equals(outputType, "WinExe", StringComparison.OrdinalIgnoreCase));
        var isTest = string.Equals(isTestProject, "true", StringComparison.OrdinalIgnoreCase)
            || msTestSdk
            || mtpPackage
            || packages.Contains(TestProjectInspector.VsTestPackage)
            || TestPackages.Any(packages.Contains)
            // An MTP runner property alone marks an executable only: Directory.Build.props often sets it for every project.
            || (mtpProperty && exe);
        if (!isTest)
        {
            return null;
        }

        if (IsTrue("UseVSTest"))
        {
            return TestRunnerProtocol.VsTest;
        }

        return msTestSdk || mtpProperty || mtpPackage ? TestRunnerProtocol.MicrosoftTestingPlatform : TestRunnerProtocol.VsTest;
    }

    private static TestProjectOutput Output(MsbProject project, string tfm, TestRunnerProtocol protocol)
    {
        var dir = project.DirectoryPath;
        var targetPath = project.GetPropertyValue("TargetPath");
        string program;
        if (targetPath.Length > 0)
        {
            program = Path.GetFullPath(Path.Combine(dir, targetPath.Replace('\\', Path.DirectorySeparatorChar)));
        }
        else
        {
            // No TargetPath (the SDK's targets did not import): the conventional output.
            var name = project.GetPropertyValue("AssemblyName") is { Length: > 0 } a ? a : Path.GetFileNameWithoutExtension(project.FullPath);
            program = Path.Combine(dir, "bin", project.GetPropertyValue("Configuration"), tfm, name + ".dll");
        }

        if (tfm.Length == 0)
        {
            tfm = project.GetPropertyValue("TargetFramework") is { Length: > 0 } t ? t : "net10.0";
        }

        var packages = Packages(project);
        CoverageCollector coverage;
        string? adapter = null;
        if (protocol == TestRunnerProtocol.MicrosoftTestingPlatform)
        {
            // The extension also comes through the MSTest metapackage and MSTest.Sdk: its assembly beside the application says so.
            coverage = packages.Contains(MtpCoveragePackage) || HasMtpCoverage(program) ? CoverageCollector.Mtp : CoverageCollector.None;
        }
        else if (packages.Contains(CoverletPackage))
        {
            coverage = CoverageCollector.Coverlet;
            adapter = project.Imports
                .Select(i => i.ImportedProject.FullPath)
                .FirstOrDefault(f => string.Equals(Path.GetFileName(f), "coverlet.collector.targets", StringComparison.OrdinalIgnoreCase)) is { } targets
                ? Path.GetDirectoryName(targets)
                : null;
        }
        else if (packages.Contains(TestProjectInspector.VsTestPackage) || packages.Contains("Microsoft.CodeCoverage")
            || project.GetPropertyValue("TraceDataCollectorDirectoryPath").Length > 0)
        {
            coverage = CoverageCollector.Microsoft;
            adapter = project.GetPropertyValue("TraceDataCollectorDirectoryPath") is { Length: > 0 } trace ? Path.TrimEndingDirectorySeparator(trace) : null;
        }
        else
        {
            coverage = CoverageCollector.None;
        }

        return new TestProjectOutput(tfm, program) { Coverage = coverage, CoverageAdapterPath = adapter };
    }

    private static HashSet<string> Packages(MsbProject project) =>
        project.GetItems("PackageReference").Select(i => i.EvaluatedInclude.Trim()).ToHashSet(StringComparer.OrdinalIgnoreCase);

    private static bool HasMtpCoverage(string program) =>
        Path.GetDirectoryName(program) is { } dir && File.Exists(Path.Combine(dir, MtpCoveragePackage + ".dll"));

    private static List<string> Split(string value) =>
        [.. value.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)];
}
