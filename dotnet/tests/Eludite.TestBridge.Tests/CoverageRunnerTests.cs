using System.Xml.Linq;

namespace Eludite.TestBridge.Tests;

/// <summary>
/// Brief 0076: the runners collecting Cobertura coverage against the built variants: Microsoft.Testing.Platform's
/// <c>--coverage</c> (Corpus.Coverage), VSTest's <c>XPlat Code Coverage</c> (coverlet, Corpus.Coverlet) and
/// <c>Code Coverage</c> (Microsoft.CodeCoverage, Corpus.Platform64) data collectors, each writing its report under the
/// container's coverage folder; discovery collects nothing; and the run settings a VSTest coverage run sends.
/// </summary>
public sealed class CoverageRunnerTests
{
    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    private static readonly TimeSpan Limit = TimeSpan.FromMinutes(3);

    [Fact]
    public async Task Mtp_Coverage_WritesItsCoberturaReport_AndDiscoveryDoesNot()
    {
        var container = await ContainerAsync("Corpus.Coverage", TestRunnerProtocol.MicrosoftTestingPlatform, CoverageCollector.Mtp);
        try
        {
            var runner = new MtpRunner();
            var discovery = new RecordingSink();
            await runner.DiscoverAsync(container, discovery, Ct).WaitAsync(Limit, Ct);
            Assert.Empty(CoverageCollection.Reports(container.CoverageDirectory));
            var run = new RecordingSink();
            await runner.RunAsync(container, discovery.Found, debug: false, run, Ct).WaitAsync(Limit, Ct);
            Assert.All(run.Final.Values, r => Assert.Equal(TestOutcomes.Passed, r.Outcome));
            Assert.Contains("--coverage-output-format cobertura", run.LogText, StringComparison.Ordinal);
            var report = Assert.Single(CoverageCollection.Reports(container.CoverageDirectory));
            Assert.Equal(Path.Combine(container.CoverageDirectory!, CoverageCollection.MtpReportName), report);
            AssertCalculator(report);
        }
        finally
        {
            if (Directory.Exists(container.CoverageDirectory))
            {
                Directory.Delete(container.CoverageDirectory, recursive: true);
            }
        }
    }

    [Theory]
    [InlineData("Corpus.Coverlet", CoverageCollector.Coverlet)]
    [InlineData("Corpus.Platform64", CoverageCollector.Microsoft)]
    public async Task VsTest_Coverage_WritesACoberturaReport(string project, CoverageCollector collector)
    {
        var container = await ContainerAsync(project, TestRunnerProtocol.VsTest, collector);
        try
        {
            var run = new RecordingSink();
            await new VsTestRunner().RunAsync(container, null, debug: false, run, Ct).WaitAsync(Limit, Ct);
            Assert.True(run.Final.Count == 2, run.LogText);
            Assert.All(run.Final.Values, r => Assert.Equal(TestOutcomes.Passed, r.Outcome));
            var report = Assert.Single(CoverageCollection.Reports(container.CoverageDirectory));
            if (collector == CoverageCollector.Coverlet)
            {
                AssertCalculator(report);
            }
            else
            {
                // Microsoft's collector leaves the test assembly out, and Corpus.Platform64 references nothing else.
                Assert.Equal("coverage", XDocument.Load(report).Root!.Name.LocalName);
            }
        }
        finally
        {
            if (Directory.Exists(container.CoverageDirectory))
            {
                Directory.Delete(container.CoverageDirectory, recursive: true);
            }
        }
    }

    [Fact]
    public void VsTestRunSettings_AddTheCollector_TheResultsDirectory_AndTheAdapterPath_KeepingTheUsersSettings()
    {
        var container = new TestContainer("p|net10.0", "/s/p.csproj", "net10.0", TestRunnerProtocol.VsTest, "/s/bin/p.dll", TestRuntime.Dotnet)
        {
            Coverage = CoverageCollector.Coverlet,
            CoverageDirectory = "/tmp/cov/0",
            CoverageAdapterPath = "/pkgs/coverlet.collector/6.0.4/build/netstandard2.0",
        };

        var defaults = XDocument.Parse(CoverageCollection.VsTestRunSettings(container, VsTestRunner.RunSettings(container)));
        Assert.Equal("1", defaults.Root!.Element("RunConfiguration")!.Element("MaxCpuCount")!.Value);
        Assert.Equal("/tmp/cov/0", defaults.Root.Element("RunConfiguration")!.Element("ResultsDirectory")!.Value);
        Assert.Equal(container.CoverageAdapterPath, defaults.Root.Element("RunConfiguration")!.Element("TestAdaptersPaths")!.Value);
        var collector = Assert.Single(defaults.Root.Element("DataCollectionRunSettings")!.Element("DataCollectors")!.Elements("DataCollector"));
        Assert.Equal("XPlat Code Coverage", (string?)collector.Attribute("friendlyName"));
        Assert.Equal("cobertura", collector.Element("Configuration")!.Element("Format")!.Value);

        // The user's settings: their collector configuration is kept, its format made Cobertura; their adapter paths kept.
        const string mine = """
            <RunSettings>
              <RunConfiguration><TestAdaptersPaths>/mine</TestAdaptersPaths></RunConfiguration>
              <DataCollectionRunSettings><DataCollectors>
                <DataCollector friendlyName="Code Coverage"><Configuration><Format>coverage</Format><IncludeTestAssembly>true</IncludeTestAssembly></Configuration></DataCollector>
              </DataCollectors></DataCollectionRunSettings>
            </RunSettings>
            """;
        var merged = XDocument.Parse(CoverageCollection.VsTestRunSettings(container with { Coverage = CoverageCollector.Microsoft }, mine));
        Assert.Equal("/mine;" + container.CoverageAdapterPath, merged.Root!.Element("RunConfiguration")!.Element("TestAdaptersPaths")!.Value);
        var code = Assert.Single(merged.Root.Element("DataCollectionRunSettings")!.Element("DataCollectors")!.Elements("DataCollector"));
        Assert.Equal("Cobertura", code.Element("Configuration")!.Element("Format")!.Value);
        Assert.Equal("true", code.Element("Configuration")!.Element("IncludeTestAssembly")!.Value);

        // No coverage folder, or no collector: the settings as they are; MTP switches likewise.
        Assert.Equal(mine, CoverageCollection.VsTestRunSettings(container with { CoverageDirectory = null }, mine));
        Assert.Equal(mine, CoverageCollection.VsTestRunSettings(container with { Coverage = CoverageCollector.None }, mine));
        Assert.Empty(CoverageCollection.MtpArguments(container));
        Assert.Equal(
            ["--coverage", "--coverage-output-format", "cobertura", "--coverage-output", Path.Combine("/tmp/cov/0", "coverage.cobertura.xml")],
            CoverageCollection.MtpArguments(container with { Protocol = TestRunnerProtocol.MicrosoftTestingPlatform, Coverage = CoverageCollector.Mtp }));
    }

    /// <summary>A variant's container with a fresh coverage folder (and, for VSTest, its collector's package folder).</summary>
    private static async Task<TestContainer> ContainerAsync(string name, TestRunnerProtocol protocol, CoverageCollector collector)
    {
        var root = await Corpus.VariantsAsync();
        var dir = Path.Combine(root, "Variants", name);
        var program = name == "Corpus.Platform64"
            ? Path.Combine(dir, "bin", "x64", "Debug", "net10.0", name + ".dll")
            : Path.Combine(dir, "bin", "Debug", "net10.0", name + ".dll");
        return new TestContainer($"{dir}|net10.0", Path.Combine(dir, name + ".csproj"), "net10.0", protocol, program, TestRuntime.Dotnet)
        {
            DotnetExecutable = Corpus.Dotnet(),
            Coverage = collector,
            CoverageDirectory = Path.Combine(Path.GetTempPath(), "eludite-test-coverage-" + Guid.NewGuid().ToString("N")[..12]),
            CoverageAdapterPath = collector switch
            {
                CoverageCollector.Coverlet => PackageBuildFolder(root, "coverlet.collector", "coverlet.collector"),
                CoverageCollector.Microsoft => PackageBuildFolder(root, "microsoft.codecoverage", "Microsoft.NET.Test.Sdk"),
                _ => null,
            },
        };
    }

    /// <summary>
    /// <c>&lt;packages&gt;/&lt;id&gt;/&lt;version&gt;/build/netstandard2.0</c>, the version the corpus pins for
    /// <paramref name="versionOf"/> (Microsoft.CodeCoverage moves with Microsoft.NET.Test.Sdk). The host reads it from the
    /// evaluation instead (TestProjectEvaluator).
    /// </summary>
    private static string PackageBuildFolder(string root, string id, string versionOf)
    {
        var version = XDocument.Load(Path.Combine(root, "Directory.Packages.props")).Descendants("PackageVersion")
            .Single(p => (string?)p.Attribute("Include") == versionOf).Attribute("Version")!.Value;
        var packages = Environment.GetEnvironmentVariable("NUGET_PACKAGES") is { Length: > 0 } p
            ? p
            : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".nuget", "packages");
        return Path.Combine(packages, id, version, "build", "netstandard2.0");
    }

    /// <summary>The report covers Calculator.Add and Subtract and not Divide.</summary>
    private static void AssertCalculator(string report)
    {
        var cls = Assert.Single(XDocument.Load(report).Descendants("class"), c => (string?)c.Attribute("name") == "Corpus.Arithmetic.Calculator");
        var methods = cls.Descendants("method").ToDictionary(m => (string)m.Attribute("name")!);
        Assert.Contains(methods["Add"].Descendants("line"), l => (int)l.Attribute("hits")! > 0);
        Assert.All(methods["Subtract"].Descendants("line"), l => Assert.True((int)l.Attribute("hits")! > 0));
        Assert.All(methods["Divide"].Descendants("line"), l => Assert.Equal(0, (int)l.Attribute("hits")!));
    }
}
