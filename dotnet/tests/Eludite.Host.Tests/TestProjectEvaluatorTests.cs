using System.Diagnostics;
using Eludite.Host.Testing;
using Eludite.TestBridge;
using Eludite.TestBridge.Tests;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0076: test projects as MSBuild sees them. Against the built variants: a project made a test project by its
/// Directory.Build.props (the project file alone says nothing), the x64 project's <c>bin/x64/Debug/</c> output, the
/// artifacts layout, and each coverage collector. Against the corpus: the evaluation agrees with the file-only inspector.
/// On synthetic, unrestored project files: TUnit by package, multi-targeting, <c>RuntimeIdentifier</c>, <c>UseVSTest</c>,
/// <c>IsTestProject</c> false, and an MTP runner property set for every project in a Directory.Build.props.
/// </summary>
public sealed class TestProjectEvaluatorTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    [Fact]
    public async Task Variants_AreFoundAsMSBuildSeesThem_WithTheirOutputsAndCollectors()
    {
        var root = await Corpus.VariantsAsync();
        var variants = Path.Combine(root, "Variants");
        var projects = Corpus.Variants.Select(v => Path.Combine(variants, v, v + ".csproj")).ToList();

        var watch = Stopwatch.StartNew();
        var evaluated = TestProjectEvaluator.Evaluate(projects, "Debug", "x64", cancellationToken: Ct);
        TestContext.Current.SendDiagnosticMessage($"evaluation of the {projects.Count} variants: {watch.ElapsedMilliseconds} ms");

        // Corpus.Calculator is a library: not a test project.
        Assert.Equal(["Corpus.PropsDriven", "Corpus.Platform64", "Corpus.Artifacts", "Corpus.Coverage", "Corpus.Coverlet"], evaluated.Select(e => Path.GetFileNameWithoutExtension(e.Project)));
        Assert.All(evaluated, e => Assert.True(e.Evaluated));
        var byName = evaluated.ToDictionary(e => Path.GetFileNameWithoutExtension(e.Project));

        // Everything that makes it a test project is in its Directory.Build.props: the file alone is not one.
        var props = byName["Corpus.PropsDriven"];
        Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, props.Protocol);
        Assert.Null(TestProjectInspector.Inspect(props.Project));
        // An AnyCPU project under the x64 solution platform keeps its default output.
        Assert.Equal(Path.Combine(variants, "Corpus.PropsDriven", "bin", "Debug", "net10.0", "Corpus.PropsDriven.dll"), Assert.Single(props.Outputs).Program);

        var x64 = Assert.Single(byName["Corpus.Platform64"].Outputs);
        Assert.Equal(TestRunnerProtocol.VsTest, byName["Corpus.Platform64"].Protocol);
        Assert.Equal(Path.Combine(variants, "Corpus.Platform64", "bin", "x64", "Debug", "net10.0", "Corpus.Platform64.dll"), x64.Program);
        Assert.True(File.Exists(x64.Program));
        Assert.Equal(CoverageCollector.Microsoft, x64.Coverage);
        Assert.True(File.Exists(Path.Combine(x64.CoverageAdapterPath!, "Microsoft.VisualStudio.TraceDataCollector.dll")), x64.CoverageAdapterPath);

        var artifacts = Assert.Single(byName["Corpus.Artifacts"].Outputs);
        Assert.Equal(Path.Combine(variants, "Corpus.Artifacts", "artifacts", "bin", "Corpus.Artifacts", "debug", "Corpus.Artifacts.dll"), artifacts.Program);
        Assert.True(File.Exists(artifacts.Program));

        var mtp = Assert.Single(byName["Corpus.Coverage"].Outputs);
        Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, byName["Corpus.Coverage"].Protocol);
        Assert.Equal(CoverageCollector.Mtp, mtp.Coverage);
        var coverlet = Assert.Single(byName["Corpus.Coverlet"].Outputs);
        Assert.Equal(TestRunnerProtocol.VsTest, byName["Corpus.Coverlet"].Protocol);
        Assert.Equal(CoverageCollector.Coverlet, coverlet.Coverage);
        Assert.True(File.Exists(Path.Combine(coverlet.CoverageAdapterPath!, "coverlet.collector.dll")), coverlet.CoverageAdapterPath);

        // Any CPU, which it does not have: the x64-only project maps to x64, as a solution build maps it. No platform: the
        // project's default, MSBuild's AnyCPU, as `dotnet build` of the project alone builds it.
        var anyCpu = TestProjectEvaluator.Evaluate([projects[1]], "Debug", "Any CPU", cancellationToken: Ct).Single().Outputs.Single();
        Assert.Equal(x64.Program, anyCpu.Program);
        var none = TestProjectEvaluator.Evaluate([projects[1]], "Debug", null, cancellationToken: Ct).Single().Outputs.Single();
        Assert.Equal(Path.Combine(variants, "Corpus.Platform64", "bin", "Debug", "net10.0", "Corpus.Platform64.dll"), none.Program);
    }

    [Fact]
    public async Task Corpus_EvaluationAgreesWithTheInspector()
    {
        var root = await Corpus.DirectoryAsync();
        var projects = Corpus.Projects.Select(p => Corpus.Project(root, p)).ToList();
        var watch = Stopwatch.StartNew();
        var evaluated = TestProjectEvaluator.Evaluate(projects, "Debug", null, cancellationToken: Ct);
        TestContext.Current.SendDiagnosticMessage($"evaluation of the {projects.Count} corpus projects: {watch.ElapsedMilliseconds} ms");
        Assert.Equal(projects, evaluated.Select(e => e.Project));
        foreach (var e in evaluated)
        {
            var name = Path.GetFileNameWithoutExtension(e.Project);
            var inspected = TestProjectInspector.Inspect(e.Project)!;
            Assert.True(e.Evaluated, name);
            Assert.Equal(inspected.Protocol, e.Protocol);
            var frameworks = inspected.TargetFrameworks.Count > 0 ? inspected.TargetFrameworks : ["net10.0"];
            Assert.Equal(frameworks, e.Outputs.Select(o => o.TargetFramework));
            foreach (var o in e.Outputs)
            {
                var extension = TestProjectInspector.IsNetFramework(o.TargetFramework) && inspected.IsExe ? ".exe" : ".dll";
                Assert.Equal(Corpus.Output(root, name, o.TargetFramework, extension), o.Program);
            }
        }

        var byName = evaluated.ToDictionary(e => Path.GetFileNameWithoutExtension(e.Project));
        // The MSTest metapackage brings Microsoft's MTP coverage extension; Microsoft.NET.Test.Sdk brings Microsoft.CodeCoverage.
        Assert.Equal(CoverageCollector.Mtp, byName["Corpus.MSTest"].Outputs[0].Coverage);
        Assert.Equal(CoverageCollector.Microsoft, byName["Corpus.Xunit2"].Outputs[0].Coverage);
    }

    [Fact]
    public void SyntheticProjects_TUnit_MultiTargeting_RuntimeIdentifier_AndTheNegatives()
    {
        var dir = Path.Combine(Path.GetTempPath(), "eludite-test-evaluator-" + Guid.NewGuid().ToString("N")[..12]);
        try
        {
            // Every project of this tree gets an MTP runner property, as a repository moving to MTP writes it.
            Directory.CreateDirectory(dir);
            File.WriteAllText(Path.Combine(dir, "Directory.Build.props"), """
                <Project>
                  <PropertyGroup><TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport></PropertyGroup>
                </Project>
                """);
            string Write(string name, string body)
            {
                var path = Path.Combine(dir, name, name + ".csproj");
                Directory.CreateDirectory(Path.GetDirectoryName(path)!);
                File.WriteAllText(path, $"""
                    <Project Sdk="Microsoft.NET.Sdk">
                      {body}
                    </Project>
                    """);
                return path;
            }

            var tunit = Write("UsesTUnit", """<PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup><ItemGroup><PackageReference Include="TUnit" Version="1.0.0" /></ItemGroup>""");
            var engine = Write("UsesTUnitEngine", """<PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup><ItemGroup><PackageReference Include="TUnit.Engine" Version="1.0.0" /></ItemGroup>""");
            var multi = Write("Multi", """<PropertyGroup><TargetFrameworks>net8.0;net10.0</TargetFrameworks><TestingPlatformDotnetTestSupport>false</TestingPlatformDotnetTestSupport></PropertyGroup><ItemGroup><PackageReference Include="Microsoft.NET.Test.Sdk" Version="18.10.1" /></ItemGroup>""");
            var rid = Write("WithRid", """<PropertyGroup><TargetFramework>net10.0</TargetFramework><RuntimeIdentifier>linux-x64</RuntimeIdentifier><UseVSTest>true</UseVSTest></PropertyGroup><ItemGroup><PackageReference Include="xunit.v3" Version="4.0.1" /></ItemGroup>""");
            var notTest = Write("NotATest", """<PropertyGroup><TargetFramework>net10.0</TargetFramework><IsTestProject>false</IsTestProject></PropertyGroup><ItemGroup><PackageReference Include="Microsoft.NET.Test.Sdk" Version="18.10.1" /></ItemGroup>""");
            var library = Write("Library", """<PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup>""");

            var evaluated = TestProjectEvaluator.Evaluate([tunit, engine, multi, rid, notTest, library], "Release", null, cancellationToken: Ct).ToDictionary(e => Path.GetFileNameWithoutExtension(e.Project));

            // TUnit and TUnit.Engine are MTP, by package alone (no restore is needed to evaluate a PackageReference).
            Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, evaluated["UsesTUnit"].Protocol);
            Assert.Equal(TestRunnerProtocol.MicrosoftTestingPlatform, evaluated["UsesTUnitEngine"].Protocol);
            Assert.Equal(Path.Combine(dir, "UsesTUnit", "bin", "Release", "net10.0", "UsesTUnit.dll"), evaluated["UsesTUnit"].Outputs[0].Program);
            Assert.Equal(CoverageCollector.None, evaluated["UsesTUnit"].Outputs[0].Coverage);

            // One output per target framework, each from its own evaluation.
            Assert.Equal(TestRunnerProtocol.VsTest, evaluated["Multi"].Protocol);
            Assert.Equal(["net8.0", "net10.0"], evaluated["Multi"].Outputs.Select(o => o.TargetFramework));
            Assert.Equal(Path.Combine(dir, "Multi", "bin", "Release", "net8.0", "Multi.dll"), evaluated["Multi"].Outputs[0].Program);
            Assert.Equal(CoverageCollector.Microsoft, evaluated["Multi"].Outputs[1].Coverage);

            // RuntimeIdentifier is part of the output path; UseVSTest wins over an MTP package.
            Assert.Equal(TestRunnerProtocol.VsTest, evaluated["WithRid"].Protocol);
            Assert.Equal(Path.Combine(dir, "WithRid", "bin", "Release", "net10.0", "linux-x64", "WithRid.dll"), evaluated["WithRid"].Outputs[0].Program);

            // IsTestProject false wins; a library with only the tree's MTP property is not a test project.
            Assert.False(evaluated.ContainsKey("NotATest"));
            Assert.False(evaluated.ContainsKey("Library"));
        }
        finally
        {
            TestDirectory.Delete(dir);
        }
    }
}
