using System.Text.Json;
using Eludite.Host.Legacy;
using Eludite.Host.Projects;
using Eludite.Host.Rpc;
using Nerdbank.Streams;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0012: <c>eludite/solution/tree</c>. The wire contract through a host without a language server, the real
/// in-process MSBuild evaluation on an SDK-style fixture, on <c>dotnet/Eludite.slnx</c> and on a legacy WebForms
/// project (synthetic, plus the corpus project when <c>corpus/legacy/fetch.sh</c> has run), and the per-generation
/// caching with a scripted evaluator.
/// </summary>
public sealed class SolutionTreeTests : IDisposable
{
    private readonly DirectoryInfo _dir = Directory.CreateTempSubdirectory("eludite-tree-");

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    public void Dispose()
    {
        try
        {
            _dir.Delete(recursive: true);
        }
        catch (IOException)
        {
        }
    }

    private string Write(string relative, string text)
    {
        var path = Path.Combine(_dir.FullName, relative.Replace('/', Path.DirectorySeparatorChar));
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text);
        return path;
    }

    /// <summary>Two SDK-style projects: a multi-targeted library with folders and an excluded file, and a web app.</summary>
    private string WriteSdkFixture()
    {
        Write("src/Lib/Lib.csproj", """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup><TargetFrameworks>net8.0;net10.0</TargetFrameworks></PropertyGroup>
              <ItemGroup>
                <Compile Remove="Excluded.cs" />
                <Compile Include="../Shared/Version.cs" Link="Properties/Version.cs" />
              </ItemGroup>
            </Project>
            """);
        Write("src/Lib/Widget.cs", "namespace Lib; public class Widget { }");
        Write("src/Lib/Models/Order.cs", "namespace Lib.Models; public class Order { }");
        Write("src/Lib/Models/Order.Designer.cs", "namespace Lib.Models; public partial class OrderDesigner { }");
        Write("src/Lib/Excluded.cs", "this is not compiled");
        Write("src/Lib/obj/Debug/Generated.cs", "// generated");
        Write("src/Shared/Version.cs", "static class Version { }");
        Write("src/Web/Web.csproj", """
            <Project Sdk="Microsoft.NET.Sdk.Web">
              <PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup>
            </Project>
            """);
        Write("src/Web/Program.cs", "System.Console.WriteLine();");
        Write("src/Web/wwwroot/site.css", "body {}");
        Write("src/Web/appsettings.json", "{}");
        return Write("App.slnx", """
            <Solution>
              <Project Path="src/Lib/Lib.csproj" />
              <Project Path="src/Web/Web.csproj" />
            </Solution>
            """);
    }

    private string WriteLegacyWebForms()
    {
        Write("LegacyShop/LegacyShop.csproj", """
            <?xml version="1.0" encoding="utf-8"?>
            <Project ToolsVersion="15.0" DefaultTargets="Build" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
              <Import Project="$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props" Condition="Exists('$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props')" />
              <PropertyGroup>
                <ProjectTypeGuids>{349c5851-65df-11da-9384-00065b846f21};{fae04ec0-301f-11d3-bf4b-00c04f79efbc}</ProjectTypeGuids>
                <OutputType>Library</OutputType>
                <AssemblyName>LegacyShop</AssemblyName>
                <TargetFrameworkVersion>v4.7.2</TargetFrameworkVersion>
              </PropertyGroup>
              <ItemGroup>
                <Content Include="Default.aspx" />
                <Content Include="Web.config" />
                <Compile Include="Default.aspx.cs"><DependentUpon>Default.aspx</DependentUpon><SubType>ASPXCodeBehind</SubType></Compile>
                <Compile Include="Default.aspx.designer.cs"><DependentUpon>Default.aspx</DependentUpon></Compile>
                <Compile Include="App_Code\Cart.cs" />
              </ItemGroup>
              <PropertyGroup>
                <VisualStudioVersion Condition="'$(VisualStudioVersion)' == ''">10.0</VisualStudioVersion>
                <VSToolsPath Condition="'$(VSToolsPath)' == ''">$(MSBuildExtensionsPath32)\Microsoft\VisualStudio\v$(VisualStudioVersion)</VSToolsPath>
              </PropertyGroup>
              <Import Project="$(MSBuildBinPath)\Microsoft.CSharp.targets" />
              <Import Project="$(VSToolsPath)\WebApplications\Microsoft.WebApplication.targets" />
            </Project>
            """);
        Write("LegacyShop/Default.aspx", "<%@ Page Language=\"C#\" CodeBehind=\"Default.aspx.cs\" Inherits=\"LegacyShop.Default\" %>");
        Write("LegacyShop/Default.aspx.cs", "namespace LegacyShop { public partial class Default : System.Web.UI.Page { } }");
        Write("LegacyShop/Default.aspx.designer.cs", "namespace LegacyShop { public partial class Default { } }");
        Write("LegacyShop/App_Code/Cart.cs", "namespace LegacyShop { class Cart { } }");
        Write("LegacyShop/Web.config", "<configuration />");
        return Write("LegacyShop.sln", """
            Microsoft Visual Studio Solution File, Format Version 12.00
            Project("{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}") = "LegacyShop", "LegacyShop\LegacyShop.csproj", "{6A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}"
            EndProject
            """);
    }

    private static string[] Relative(TreeProject project) =>
        [.. project.Files.Select(f => Path.GetRelativePath(Path.GetDirectoryName(project.Path)!, f.Path).Replace('\\', '/'))];

    [Fact]
    public void SdkFixture_ListsCompileItemsFoldersFrameworksAndWebContent()
    {
        var sln = WriteSdkFixture();
        var tree = new MsBuildProjectTreeEvaluator().Evaluate(SolutionProjects.Read(sln), Ct);

        Assert.Equal(["Lib", "Web"], tree.Select(p => p.Name));
        var lib = tree[0];
        Assert.Null(lib.Error);
        Assert.Equal("sdk", lib.Kind);
        Assert.False(lib.Web);
        Assert.Equal(["net8.0", "net10.0"], lib.TargetFrameworks);
        var files = Relative(lib);
        Assert.Contains("Widget.cs", files);
        Assert.Contains("Models/Order.cs", files);
        Assert.Contains("Models/Order.Designer.cs", files);
        Assert.Contains("../Shared/Version.cs", files);
        Assert.DoesNotContain("Excluded.cs", files);
        Assert.DoesNotContain(files, f => f.StartsWith("obj/", StringComparison.Ordinal));
        Assert.All(lib.Files, f => Assert.Equal("compile", f.ItemType));
        Assert.All(lib.Files, f => Assert.True(Path.IsPathFullyQualified(f.Path)));
        Assert.Equal("Properties/Version.cs", lib.Files.Single(f => f.Path.EndsWith("Version.cs", StringComparison.Ordinal)).Link);

        var web = tree[1];
        Assert.True(web.Web);
        Assert.Equal(["net10.0"], web.TargetFrameworks);
        Assert.Contains(web.Files, f => f.ItemType == "compile" && f.Path.EndsWith("Program.cs", StringComparison.Ordinal));
        Assert.Contains(web.Files, f => f.ItemType == "content" && f.Path.EndsWith("site.css", StringComparison.Ordinal));
        Assert.Contains(web.Files, f => f.ItemType == "content" && f.Path.EndsWith("appsettings.json", StringComparison.Ordinal));
    }

    [Fact]
    public void LegacyWebForms_ListsCodeBehindNestedUnderMarkup()
    {
        var sln = WriteLegacyWebForms();
        var shop = Assert.Single(new MsBuildProjectTreeEvaluator().Evaluate(SolutionProjects.Read(sln), Ct));

        Assert.Null(shop.Error);
        Assert.Equal("legacy", shop.Kind);
        Assert.True(shop.Web);
        Assert.Equal(["net472"], shop.TargetFrameworks);
        var dir = Path.GetDirectoryName(shop.Path)!;
        var aspx = Path.Combine(dir, "Default.aspx");
        Assert.Contains(shop.Files, f => f.Path == aspx && f.ItemType == "content");
        Assert.Equal(aspx, shop.Files.Single(f => f.Path.EndsWith("Default.aspx.cs", StringComparison.Ordinal)).DependentUpon);
        Assert.Equal(aspx, shop.Files.Single(f => f.Path.EndsWith("Default.aspx.designer.cs", StringComparison.Ordinal)).DependentUpon);
        Assert.Contains(Path.Combine(dir, "App_Code", "Cart.cs"), shop.Files.Select(f => f.Path));
    }

    [Fact]
    public void UnreadableProject_IsListedWithAnError()
    {
        var bad = Write("Bad/Bad.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup>");
        var result = Assert.Single(new MsBuildProjectTreeEvaluator().Evaluate([bad], Ct));
        Assert.Equal("Bad", result.Name);
        Assert.Empty(result.Files);
        Assert.False(string.IsNullOrEmpty(result.Error));
    }

    [Theory]
    [InlineData("v4.8", "net48")]
    [InlineData("v4.7.2", "net472")]
    [InlineData("v3.5", "net35")]
    [InlineData("", null)]
    [InlineData("vNext", null)]
    public void ShortFramework(string version, string? expected) =>
        Assert.Equal(expected, MsBuildProjectTreeEvaluator.ShortFramework(version));

    [Fact]
    public void EluditeSolution_ListsItsTwelveProjects()
    {
        var slnx = FindRepoFile("dotnet/Eludite.slnx");
        Assert.SkipWhen(slnx is null, "dotnet/Eludite.slnx not found above the test directory.");
        var tree = new MsBuildProjectTreeEvaluator().Evaluate(SolutionProjects.Read(slnx!), Ct);

        Assert.Equal(12, tree.Count);
        Assert.All(tree, p => Assert.Null(p.Error));
        var host = tree.Single(p => p.Name == "Eludite.Host");
        Assert.Equal(["net10.0"], host.TargetFrameworks);
        Assert.Contains(host.Files, f => f.Path.EndsWith(Path.Combine("Rpc", "HostRpcTarget.cs"), StringComparison.Ordinal));
    }

    [Fact]
    public void LegacyCorpusProject_ListsWebFormsPagesWithCodeBehind()
    {
        var project = FindCorpusProject();
        Assert.SkipWhen(project is null, "Corpus checkout not found; run corpus/legacy/fetch.sh (or set ELUDITE_LEGACY_CORPUS).");
        var result = Assert.Single(new MsBuildProjectTreeEvaluator().Evaluate([project!], Ct));

        Assert.Null(result.Error);
        Assert.Equal("legacy", result.Kind);
        Assert.True(result.Web);
        Assert.Equal(["net45"], result.TargetFrameworks);
        var dir = Path.GetDirectoryName(project)!;
        var login = Path.Combine(dir, "Account", "Login.aspx");
        Assert.Contains(result.Files, f => f.Path == login && f.ItemType == "content");
        Assert.Equal(login, result.Files.Single(f => f.Path == login + ".cs").DependentUpon);
        Assert.Equal(login, result.Files.Single(f => f.Path == Path.Combine(dir, "Account", "Login.aspx.designer.cs")).DependentUpon);
        Assert.True(result.Files.Count(f => f.ItemType == "compile") > 10);
    }

    // ------------------------------------------------------------------ the request over the wire

    [Fact]
    public async Task Request_BeforeInitialize_IsServerNotInitialized()
    {
        await using var host = new WireHost(new ScriptedEvaluator());
        var (code, _) = await TestRpc.ErrorOfAsync(() => host.Client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct));
        Assert.Equal(HostErrors.ServerNotInitialized, code);
    }

    [Fact]
    public async Task Request_WithoutSolution_ReturnsEmptyTree()
    {
        await using var host = new WireHost(new ScriptedEvaluator());
        await host.InitializeAsync();
        var tree = await host.Client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct);
        Assert.Equal(0, tree.GetProperty("generation").GetInt64());
        Assert.False(tree.TryGetProperty("path", out var path) && path.ValueKind != JsonValueKind.Null);
        Assert.Equal(0, tree.GetProperty("projects").GetArrayLength());
    }

    [Fact]
    public async Task Request_ReturnsTheTreeOfTheOpenSolution_OncePerGeneration()
    {
        var sln = WriteSdkFixture();
        var evaluator = new ScriptedEvaluator();
        await using var host = new WireHost(evaluator);
        await host.InitializeAsync();
        var generation = (await host.Client.InvokeWithParameterObjectAsync<GenerationResult>("eludite/solution/open", new { path = sln }, Ct)).Generation;

        var first = await host.Client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct);
        var second = await host.Client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct);

        Assert.Equal(generation, first.GetProperty("generation").GetInt64());
        Assert.Equal(sln, first.GetProperty("path").GetString());
        var projects = first.GetProperty("projects");
        Assert.Equal(["Lib", "Web"], projects.EnumerateArray().Select(p => p.GetProperty("name").GetString()));
        var lib = projects[0];
        Assert.Equal("sdk", lib.GetProperty("kind").GetString());
        Assert.False(lib.TryGetProperty("web", out _), "web is omitted when false");
        Assert.Equal("compile", lib.GetProperty("files")[0].GetProperty("itemType").GetString());
        Assert.True(projects[1].GetProperty("web").GetBoolean());
        Assert.Equal(first.GetRawText(), second.GetRawText());
        Assert.Equal(1, host.Target.Tree.Evaluations);
    }

    [Fact]
    public async Task Request_WhenTheGenerationMovesOn_IsContentModified()
    {
        var sln = WriteSdkFixture();
        var evaluator = new ScriptedEvaluator { Gate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously) };
        await using var host = new WireHost(evaluator);
        await host.InitializeAsync();
        await host.Client.InvokeWithParameterObjectAsync<GenerationResult>("eludite/solution/open", new { path = sln }, Ct);

        var pending = host.Client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct);
        await evaluator.Started.Task.WaitAsync(Ct);
        await host.Client.InvokeWithCancellationAsync<GenerationResult>("eludite/solution/close", [], Ct);
        var (code, data) = await TestRpc.ErrorOfAsync(() => pending);

        Assert.Equal(HostErrors.ContentModified, code);
        Assert.Equal(1, data!.Value.GetProperty("requestedGeneration").GetInt64());
        Assert.Equal(2, data.Value.GetProperty("currentGeneration").GetInt64());
        evaluator.Gate.SetResult();
    }

    [Fact]
    public async Task Request_Canceled_IsRequestCancelled_AndTheEvaluationIsKeptForTheNextRequest()
    {
        var sln = WriteSdkFixture();
        var evaluator = new ScriptedEvaluator { Gate = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously) };
        await using var host = new WireHost(evaluator);
        await host.InitializeAsync();
        await host.Client.InvokeWithParameterObjectAsync<GenerationResult>("eludite/solution/open", new { path = sln }, Ct);

        using var cts = new CancellationTokenSource();
        var pending = host.Client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], cts.Token);
        await evaluator.Started.Task.WaitAsync(Ct);
        await cts.CancelAsync();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => pending);

        evaluator.Gate.SetResult();
        var tree = await host.Client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct);
        Assert.Equal(2, tree.GetProperty("projects").GetArrayLength());
        Assert.Equal(1, host.Target.Tree.Evaluations);
    }

    private static string? FindRepoFile(string relative)
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, relative);
            if (File.Exists(candidate))
            {
                return candidate;
            }
        }

        return null;
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

        return FindRepoFile(Path.Combine("corpus", "legacy", ".checkout", relative));
    }

    /// <summary>Lists each project with one Compile item (its own name) without MSBuild; optionally waits on a gate.</summary>
    private sealed class ScriptedEvaluator : IProjectTreeEvaluator
    {
        public TaskCompletionSource Started { get; } = new(TaskCreationOptions.RunContinuationsAsynchronously);

        public TaskCompletionSource? Gate { get; init; }

        public IReadOnlyList<TreeProject> Evaluate(IReadOnlyList<string> projectPaths, CancellationToken cancellationToken)
        {
            Started.TrySetResult();
            Gate?.Task.Wait(TimeSpan.FromSeconds(30));
            return [.. projectPaths.Select(p => new TreeProject(
                Path.GetFileNameWithoutExtension(p), p, "sdk", ["net10.0"],
                [new TreeFile(Path.ChangeExtension(p, ".cs"), "compile")])
            {
                Web = p.Contains("Web", StringComparison.Ordinal),
            })];
        }
    }

    /// <summary>A host without a language server over an in-memory stream.</summary>
    private sealed class WireHost : IAsyncDisposable
    {
        private readonly Task<int> _server;

        public WireHost(IProjectTreeEvaluator evaluator)
        {
            var (clientStream, serverStream) = FullDuplexStream.CreatePair();
            Target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null, tree: new SolutionTreeProvider(evaluator, TextWriter.Null));
            _server = HostServer.RunAsync(serverStream, serverStream, Target);
            Client = TestRpc.Create(clientStream);
            Client.StartListening();
        }

        public HostRpcTarget Target { get; }

        public JsonRpc Client { get; }

        public Task InitializeAsync() =>
            Client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "test", clientVersion = "0" }, Ct);

        public async ValueTask DisposeAsync()
        {
            Client.Dispose();
            await _server.WaitAsync(TimeSpan.FromSeconds(10)).ConfigureAwait(false);
        }
    }
}
