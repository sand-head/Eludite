using System.Diagnostics;
using System.Text.Json;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using Eludite.Host.Templates;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0072: <c>eludite/templates/list</c> from the real SDK's template packages (the common templates with their
/// parameters, the built-ins, the cache per package set) and <c>eludite/templates/create</c> running the real
/// <c>dotnet new</c> and <c>dotnet sln add</c> into a temporary folder.
/// </summary>
public sealed class TemplatesTests : IDisposable
{
    private readonly string _root = Directory.CreateTempSubdirectory("eludite-templates-").FullName;

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    public void Dispose() => TestDirectory.Delete(_root);

    private static TemplateService Service()
    {
        var root = TemplateCatalog.LocateDotnetRoot();
        Assert.SkipWhen(root is null, "No .NET SDK found (DOTNET_ROOT, PATH or the usual install folders).");
        return new TemplateService(new TemplateCatalog(root, userPackages: Path.Combine(Path.GetTempPath(), "eludite-no-user-templates")), TextWriter.Null);
    }

    [Fact]
    public void List_HoldsTheSdksCommonTemplatesWithParametersAndTheBuiltIns()
    {
        var service = Service();
        var sw = Stopwatch.StartNew();
        var result = service.List(null, Ct);
        TestContext.Current.SendDiagnosticMessage($"first list: {result.Templates.Count} templates in {sw.ElapsedMilliseconds} ms from {result.TemplatesFolder}");

        Assert.NotNull(result.TemplatesFolder);
        foreach (var shortName in new[] { "console", "classlib", "xunit", "web" })
        {
            Assert.Contains(result.Templates, t => t.Source == "sdk" && t.Type == "project" && t.ShortNames.Contains(shortName));
        }

        var console = Assert.Single(result.Templates, t => t.ShortNames.Contains("console") && t.Language == "C#");
        Assert.Equal("Microsoft.Common.Console", console.GroupIdentity);
        Assert.Contains("Console", console.Classifications!);
        Assert.Equal("ConsoleApp1", console.DefaultName);
        var framework = Assert.Single(console.Parameters, p => string.Equals(p.Name, "Framework", StringComparison.OrdinalIgnoreCase));
        Assert.Equal("choice", framework.DataType);
        Assert.NotEmpty(framework.Choices!);
        Assert.StartsWith("net", framework.DefaultValue, StringComparison.Ordinal);
        Assert.Contains(console.Parameters, p => p.Name == "use-program-main" && p.DataType == "bool" && p.DefaultValue == "false");
        Assert.DoesNotContain(console.Parameters, p => p.Name is "target-framework-override" or "TargetFrameworkOverride");
        Assert.Contains(result.Templates, t => t.ShortNames.Contains("console") && t.Language == "VB");
        Assert.Contains(result.Templates, t => t.ShortNames.Contains("class") && t.Type == "item" && t.Source == "sdk");

        foreach (var identity in new[] { "eludite.cs.class", "eludite.cs.interface", "eludite.cs.record", "eludite.cs.enum", "eludite.cs.struct", "eludite.vb.class", "eludite.vb.interface", "eludite.vb.module", "eludite.vb.enum", "eludite.vb.structure" })
        {
            var builtIn = Assert.Single(result.Templates, t => t.Identity == identity);
            Assert.Equal("eludite", builtIn.Source);
            Assert.Equal("item", builtIn.Type);
            Assert.Empty(builtIn.Parameters);
        }

        var items = service.List(new TemplatesListParams { Type = "item" }, Ct);
        Assert.All(items.Templates, t => Assert.Equal("item", t.Type));
        Assert.DoesNotContain(items.Templates, t => t.ShortNames.Contains("console"));
    }

    [Fact]
    public void List_IsReadOncePerPackageSet()
    {
        var service = Service();
        var first = service.Catalog.Read(cancellationToken: Ct);
        var second = service.Catalog.Read(cancellationToken: Ct);
        service.List(null, Ct);
        Assert.Same(first, second);
        Assert.Equal(1, service.Catalog.Reads);

        var refreshed = service.Catalog.Read(refresh: true, Ct);
        Assert.NotSame(first, refreshed);
        Assert.Equal(2, service.Catalog.Reads);
    }

    [Fact]
    public void List_SerializesWithTheSchemasNames()
    {
        var result = Service().List(new TemplatesListParams { Type = "project" }, Ct);
        using var json = JsonDocument.Parse(JsonSerializer.Serialize(result, HostServer.SerializerOptions));
        var console = json.RootElement.GetProperty("templates").EnumerateArray()
            .First(t => t.GetProperty("shortNames").EnumerateArray().Any(s => s.GetString() == "console") && t.GetProperty("language").GetString() == "C#");
        Assert.Equal("sdk", console.GetProperty("source").GetString());
        var framework = console.GetProperty("parameters").EnumerateArray().First(p => p.GetProperty("name").GetString() == "framework");
        Assert.Equal("choice", framework.GetProperty("dataType").GetString());
        Assert.True(framework.GetProperty("choices")[0].TryGetProperty("value", out _));
        Assert.False(framework.TryGetProperty("required", out _));
    }

    [Fact]
    public async Task Create_ConsoleWithANewSolution()
    {
        var service = Service();
        var output = Path.Combine(_root, "App");
        var solution = Path.Combine(_root, "App.slnx");
        var result = await service.CreateAsync(new TemplatesCreateParams("console", "App", output) { NewSolution = solution }, Ct);

        Assert.True(result.ExitCode == 0, result.Output);
        var project = Path.Combine(output, "App.csproj");
        Assert.Equal(project, result.Project);
        Assert.True(File.Exists(project));
        Assert.Equal(Path.Combine(output, "Program.cs"), result.PrimaryFile);
        Assert.Contains(project, result.Files);
        Assert.Contains(Path.Combine(output, "Program.cs"), result.Files);
        Assert.DoesNotContain(result.Files, f => f.Contains($"{Path.DirectorySeparatorChar}obj{Path.DirectorySeparatorChar}", StringComparison.Ordinal));
        Assert.Equal(result.Files.Order(StringComparer.Ordinal), result.Files);
        Assert.Equal(solution, result.Solution);
        Assert.True(File.Exists(solution));
        Assert.Equal([project], SolutionProjects.Read(solution));
        Assert.Equal(3, result.CommandLines.Count);
        Assert.StartsWith("dotnet new sln", result.CommandLines[0], StringComparison.Ordinal);
        Assert.StartsWith("dotnet new console --name App", result.CommandLines[1], StringComparison.Ordinal);
        Assert.EndsWith("--no-update-check", result.CommandLines[1], StringComparison.Ordinal);
        Assert.StartsWith("dotnet sln", result.CommandLines[2], StringComparison.Ordinal);
    }

    [Fact]
    public async Task Create_ClassLibraryIntoAnExistingSolution()
    {
        var service = Service();
        var solution = Path.Combine(_root, "Existing.slnx");
        await File.WriteAllTextAsync(solution, "<Solution>\n</Solution>\n", Ct);
        var output = Path.Combine(_root, "src", "Lib");
        var result = await service.CreateAsync(
            new TemplatesCreateParams("classlib", "Lib", output) { Solution = solution, Language = "C#", Parameters = new Dictionary<string, string> { ["Framework"] = "net10.0" } },
            Ct);

        Assert.True(result.ExitCode == 0, result.Output);
        var project = Path.Combine(output, "Lib.csproj");
        Assert.Equal(project, result.Project);
        Assert.Equal(Path.Combine(output, "Class1.cs"), result.PrimaryFile);
        Assert.Equal([project], SolutionProjects.Read(solution));
        // The symbol's name maps to the option dotnetcli.host.json gives it.
        Assert.Contains("--framework net10.0", result.CommandLines[0], StringComparison.Ordinal);
    }

    [Fact]
    public async Task Create_AFailingTemplateReturnsItsOutputAndExitCode()
    {
        var service = Service();
        var result = await service.CreateAsync(new TemplatesCreateParams("no-such-template-eludite", "X", Path.Combine(_root, "X")), Ct);
        Assert.NotEqual(0, result.ExitCode);
        Assert.NotEmpty(result.Output);
        Assert.Null(result.Project);
        Assert.Single(result.CommandLines);
    }

    [Fact]
    public async Task Create_RefusesARelativeOutput()
    {
        var service = Service();
        var error = await Assert.ThrowsAsync<StreamJsonRpc.LocalRpcException>(() => service.CreateAsync(new TemplatesCreateParams("console", "App", "relative/App"), Ct));
        Assert.Equal(HostErrors.InvalidParams, error.ErrorCode);
    }
}
