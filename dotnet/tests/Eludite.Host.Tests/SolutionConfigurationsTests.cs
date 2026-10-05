using System.Diagnostics;
using System.Text;
using Eludite.Host.Projects;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0049: the solution configurations, platforms and Configuration Manager's mapping from <c>.sln</c> and
/// <c>.slnx</c> files in <c>corpus/projects</c>, and mapping edits that keep every other byte of the solution file; the
/// selection's budget on a generated 100-project solution.
/// </summary>
public sealed class SolutionConfigurationsTests : IDisposable
{
    private readonly ProjectCorpus _corpus = new();

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    public void Dispose() => _corpus.Dispose();

    private static ConfigurationMapping Mapping(SolutionConfigurationsResult s, string project, string configuration, string platform) =>
        s.Projects.Single(p => p.Name == project).Mappings.Single(m => m.SolutionConfiguration == configuration && m.SolutionPlatform == platform);

    [Fact]
    public void Sln_ListsConfigurationsPlatformsAndTheMapping()
    {
        var s = SolutionConfigurationFile.Read(_corpus.Path("Corpus.sln"));

        Assert.Equal("sln", s.Format);
        Assert.Equal(["Debug", "Release"], s.Configurations);
        Assert.Equal(["Any CPU", "x64"], s.Platforms);
        Assert.Equal(["Console", "Tabs", "Multi", "Lib", "Web"], s.Projects.Select(p => p.Name));
        Assert.Equal(_corpus.Path("Inherited/Lib/Lib.csproj"), s.Projects[3].Path);
        Assert.Equal(new ConfigurationMapping("Release", "x64", "Release", "Any CPU", true), Mapping(s, "Console", "Release", "x64"));
        Assert.False(Mapping(s, "Lib", "Release", "Any CPU").Build);
        Assert.True(Mapping(s, "Lib", "Debug", "Any CPU").Build);
        Assert.Equal(4, s.Projects[0].Mappings.Count);
    }

    [Fact]
    public void Slnx_ListsConfigurationsPlatformsAndTheRules()
    {
        var s = SolutionConfigurationFile.Read(_corpus.Path("Corpus.slnx"));

        Assert.Equal("slnx", s.Format);
        Assert.Equal(["Debug", "Release"], s.Configurations);
        Assert.Equal(["Any CPU", "x64"], s.Platforms);
        Assert.Equal(["Console", "Tabs", "Multi", "Lib", "Web"], s.Projects.Select(p => p.Name));
        Assert.False(Mapping(s, "Lib", "Release", "x64").Build);
        Assert.True(Mapping(s, "Lib", "Debug", "x64").Build);
        // A project without x64 in its Platforms builds Any CPU there.
        Assert.Equal(new ConfigurationMapping("Debug", "x64", "Debug", "Any CPU", true), Mapping(s, "Console", "Debug", "x64"));
    }

    [Fact]
    public void AProjectFile_MapsToItself()
    {
        var s = SolutionConfigurationFile.Read(_corpus.Path("Console/Console.csproj"));
        Assert.Equal(("project", 1), (s.Format, s.Projects.Count));
        Assert.Equal(["Debug", "Release"], s.Configurations);
        Assert.Throws<LocalRpcException>(() => SolutionConfigurationFile.Edit(_corpus.Path("Console/Console.csproj"), [new MappingEdit(_corpus.Path("Console/Console.csproj"), "Debug", "Any CPU") { Build = false }]));
    }

    [Fact]
    public void SlnEdits_ChangeOnlyTheirLines_KeepingTabsCrlfAndTheBom()
    {
        var path = _corpus.Path("Corpus.sln");
        var before = File.ReadAllBytes(path);
        var lib = _corpus.Path("Inherited/Lib/Lib.csproj");
        const string Guid = "{1D6F4A70-0004-4C00-9000-000000000004}";

        Assert.True(SolutionConfigurationFile.Edit(path, [new MappingEdit(lib, "Release", "Any CPU") { Build = true }]));
        var text = Encoding.UTF8.GetString(before);
        var expected = text.Replace(
            $"\t\t{Guid}.Release|Any CPU.ActiveCfg = Release|Any CPU\r\n",
            $"\t\t{Guid}.Release|Any CPU.ActiveCfg = Release|Any CPU\r\n\t\t{Guid}.Release|Any CPU.Build.0 = Release|Any CPU\r\n",
            StringComparison.Ordinal);
        Assert.Equal(Encoding.UTF8.GetBytes(expected), File.ReadAllBytes(path));
        Assert.True(Mapping(SolutionConfigurationFile.Read(path), "Lib", "Release", "Any CPU").Build);

        // Clearing Build and mapping to Debug edits the same lines back and the ActiveCfg value.
        Assert.True(SolutionConfigurationFile.Edit(path, [new MappingEdit(lib, "Release", "Any CPU") { Build = false, Configuration = "Debug" }]));
        Assert.Equal(
            Encoding.UTF8.GetBytes(text.Replace($"{Guid}.Release|Any CPU.ActiveCfg = Release|Any CPU", $"{Guid}.Release|Any CPU.ActiveCfg = Debug|Any CPU", StringComparison.Ordinal)),
            File.ReadAllBytes(path));
        Assert.True(SolutionConfigurationFile.Edit(path, [new MappingEdit(lib, "Release", "Any CPU") { Configuration = "Release" }]));
        Assert.Equal(before, File.ReadAllBytes(path));
        Assert.False(SolutionConfigurationFile.Edit(path, [new MappingEdit(lib, "Release", "Any CPU") { Configuration = "Release" }]));

        Assert.Throws<LocalRpcException>(() => SolutionConfigurationFile.Edit(path, [new MappingEdit(lib, "Staging", "Any CPU") { Build = true }]));
        Assert.Throws<LocalRpcException>(() => SolutionConfigurationFile.Edit(path, [new MappingEdit(_corpus.Path("Empty/Empty.csproj"), "Debug", "Any CPU") { Build = true }]));
    }

    [Fact]
    public void SlnxEdits_AddAndRemoveTheProjectsRules_AndRestoreTheFile()
    {
        var path = _corpus.Path("Corpus.slnx");
        var before = File.ReadAllText(path);
        var console = _corpus.Path("Console/Console.csproj");
        var lib = _corpus.Path("Inherited/Lib/Lib.csproj");

        Assert.True(SolutionConfigurationFile.Edit(path, [new MappingEdit(console, "Release", "x64") { Build = false }]));
        Assert.Equal(
            before.Replace("  <Project Path=\"Console/Console.csproj\" />\n", "  <Project Path=\"Console/Console.csproj\">\n    <Build Solution=\"Release|x64\" Project=\"false\" />\n  </Project>\n", StringComparison.Ordinal),
            File.ReadAllText(path));
        Assert.False(Mapping(SolutionConfigurationFile.Read(path), "Console", "Release", "x64").Build);

        Assert.True(SolutionConfigurationFile.Edit(path, [new MappingEdit(console, "Release", "x64") { Build = true }]));
        Assert.Equal(before, File.ReadAllText(path));

        // A second rule in a project that has one goes after it, with its indentation.
        Assert.True(SolutionConfigurationFile.Edit(path, [new MappingEdit(lib, "Debug", "x64") { Configuration = "Release" }]));
        Assert.Contains(
            "    <Project Path=\"Inherited/Lib/Lib.csproj\">\n      <Build Solution=\"Release|*\" Project=\"false\" />\n      <BuildType Solution=\"Debug|x64\" Project=\"Release\" />\n    </Project>",
            File.ReadAllText(path),
            StringComparison.Ordinal);
        Assert.Equal("Release", Mapping(SolutionConfigurationFile.Read(path), "Lib", "Debug", "x64").Configuration);
        Assert.True(SolutionConfigurationFile.Edit(path, [new MappingEdit(lib, "Debug", "x64") { Configuration = "Debug" }]));
        Assert.Equal(before, File.ReadAllText(path));
    }

    [Fact]
    public async Task Budget_ConfigurationListChangeOnA100ProjectSolution()
    {
        // 100 SDK projects in one .slnx.
        var root = _corpus.Path("Hundred");
        var slnx = new StringBuilder("<Solution>\n");
        for (var i = 0; i < 100; i++)
        {
            var dir = Path.Combine(root, $"P{i:D3}");
            Directory.CreateDirectory(dir);
            await File.WriteAllTextAsync(Path.Combine(dir, $"P{i:D3}.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>net10.0</TargetFramework>\n  </PropertyGroup>\n</Project>\n", Ct);
            slnx.Append($"  <Project Path=\"P{i:D3}/P{i:D3}.csproj\" />\n");
        }

        slnx.Append("</Solution>\n");
        var solution = Path.Combine(root, "Hundred.slnx");
        await File.WriteAllTextAsync(solution, slnx.ToString(), Ct);
        long generation = 1;
        using var cts = new CancellationTokenSource();
        using var service = new ProjectPropertiesService(() => (generation, solution, cts.Token), () => generation, _ => ++generation, TextWriter.Null);
        var first = service.Configurations();
        Assert.Equal(100, first.Projects.Count);

        // The toolbar's change: select, read the lists again (the toolbar and Configuration Manager), and re-evaluate the
        // open property pages' project in the new configuration.
        var p0 = Path.Combine(root, "P000", "P000.csproj");
        await service.PropertiesAsync(new ProjectPropertiesParams(p0), Ct);
        var times = new List<double>();
        foreach (var configuration in new[] { "Release", "Debug", "Release" })
        {
            var sw = Stopwatch.StartNew();
            await service.SetConfigurationAsync(new SolutionSetConfigurationParams(generation) { Select = new Selection(configuration, "Any CPU") }, Ct);
            var lists = service.Configurations();
            var props = await service.PropertiesAsync(new ProjectPropertiesParams(p0), Ct);
            times.Add(sw.Elapsed.TotalMilliseconds);
            Assert.Equal(configuration, lists.Active.Configuration);
            Assert.Equal(configuration, props.Configuration);
        }

        TestContext.Current.TestOutputHelper?.WriteLine($"[budget] configuration change, 100 projects: {string.Join(", ", times.Select(t => t.ToString("F1", System.Globalization.CultureInfo.InvariantCulture)))} ms");
        // The first switch pays the JIT and the caches (842 ms seen on a hosted runner); the budget is the warm ones'.
        Assert.True(times.Skip(1).Max() < 500, $"{string.Join(", ", times)} ms");
    }
}
