using System.Text;
using System.Text.Json;
using Eludite.Host.Projects;
using Eludite.Host.Rpc;
using Nerdbank.Streams;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0073: <c>eludite/project/editItems</c> and <c>eludite/solution/reload</c>. Include and exclude in SDK-style
/// projects (a glob's Remove, an explicit element) and legacy projects byte for byte, a legacy Compile rename and remove
/// with DependentUpon, an F# project's compile order, addFolder, and the two methods over the wire with the generation.
/// </summary>
public sealed class ProjectItemsTests : IDisposable
{
    private readonly DirectoryInfo _dir = Directory.CreateTempSubdirectory("eludite-items-");

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    public void Dispose()
    {
        try
        {
            TestDirectory.Delete(_dir.FullName);
        }
        catch (IOException)
        {
        }
    }

    private string Full(string relative) => Path.Combine(_dir.FullName, relative.Replace('/', Path.DirectorySeparatorChar));

    private string Write(string relative, string text)
    {
        var path = Full(relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text);
        return path;
    }

    /// <summary>Writes the project with CRLF line endings and a byte order mark, as Visual Studio saves legacy projects.</summary>
    private string WriteCrlfBom(string relative, string text)
    {
        var path = Full(relative);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllBytes(path, [0xEF, 0xBB, 0xBF, .. Encoding.UTF8.GetBytes(text.Replace("\r\n", "\n", StringComparison.Ordinal).Replace("\n", "\r\n", StringComparison.Ordinal))]);
        return path;
    }

    private static byte[] CrlfBom(string text) =>
        [0xEF, 0xBB, 0xBF, .. Encoding.UTF8.GetBytes(text.Replace("\r\n", "\n", StringComparison.Ordinal).Replace("\n", "\r\n", StringComparison.Ordinal))];

    private static string Lf(string text) => text.Replace("\r\n", "\n", StringComparison.Ordinal);

    private static ProjectItemsOutcome Edit(string project, params ProjectItemEdit[] edits) => ProjectItemsService.Edit(project, edits, Ct);

    private ProjectItemEdit Op(string op, string relative, string? to = null, string? itemType = null) =>
        new(op, Full(relative)) { To = to is null ? null : Full(to), ItemType = itemType };

    private const string SdkProject = """
        <Project Sdk="Microsoft.NET.Sdk">

          <PropertyGroup>
            <OutputType>Exe</OutputType>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>

          <ItemGroup>
            <Compile Remove="Old.cs" />
            <None Update="appsettings.json" CopyToOutputDirectory="PreserveNewest" />
          </ItemGroup>

        </Project>

        """;

    private string WriteSdk()
    {
        var project = Write("App/App.csproj", SdkProject);
        Write("App/Program.cs", "System.Console.WriteLine();");
        Write("App/Old.cs", "class Old { }");
        Write("App/appsettings.json", "{}");
        Write("App/Sub/One.cs", "class One { }");
        Write("App/Sub/notes.txt", "notes");
        Write("App/Sub/Deeper/Two.cs", "class Two { }");
        return project;
    }

    // ------------------------------------------------------------------ SDK-style projects

    [Fact]
    public void Sdk_IncludeDropsTheRemoveThatExcludedIt()
    {
        var project = WriteSdk();
        var outcome = Edit(project, Op("include", "App/Old.cs"));

        Assert.True(outcome.Changed);
        Assert.True(outcome.Results[0].Applied);
        Assert.Equal(SdkProject.Replace("    <Compile Remove=\"Old.cs\" />\n", string.Empty, StringComparison.Ordinal), File.ReadAllText(project));
    }

    [Fact]
    public void Sdk_IncludeOfAFileTheGlobsAlreadyBringIn_ChangesNothing()
    {
        var project = WriteSdk();
        var outcome = Edit(project, Op("include", "App/Program.cs"), Op("addFolder", "App/Sub"));

        Assert.False(outcome.Changed);
        Assert.All(outcome.Results, r => Assert.False(r.Applied));
        Assert.Equal(SdkProject, File.ReadAllText(project));
    }

    [Fact]
    public void Sdk_ExcludeOfAGlobbedFile_AddsARemoveBesideTheItemsOfItsType()
    {
        var project = WriteSdk();
        var outcome = Edit(project, Op("exclude", "App/Program.cs"));

        Assert.True(outcome.Changed);
        Assert.Equal(
            SdkProject.Replace("    <Compile Remove=\"Old.cs\" />\n    <None Update=\"appsettings.json\" CopyToOutputDirectory=\"PreserveNewest\" />\n", "    <Compile Remove=\"Old.cs\" />\n    <None Update=\"appsettings.json\" CopyToOutputDirectory=\"PreserveNewest\" />\n    <Compile Remove=\"Program.cs\" />\n", StringComparison.Ordinal),
            File.ReadAllText(project));
    }

    [Fact]
    public void Sdk_ExcludeOfAFolder_RemovesItsFilesOfEveryType_AndIncludeBringsThemBack()
    {
        var project = WriteSdk();
        Edit(project, Op("exclude", "App/Sub"));
        var excluded = SdkProject.Replace(
            "    <None Update=\"appsettings.json\" CopyToOutputDirectory=\"PreserveNewest\" />\n",
            "    <None Update=\"appsettings.json\" CopyToOutputDirectory=\"PreserveNewest\" />\n    <Compile Remove=\"Sub\\**\" />\n    <None Remove=\"Sub\\**\" />\n",
            StringComparison.Ordinal);
        Assert.Equal(excluded, File.ReadAllText(project));

        var outcome = Edit(project, Op("include", "App/Sub"));
        Assert.True(outcome.Changed);
        Assert.Equal(SdkProject, File.ReadAllText(project));
    }

    [Fact]
    public void Sdk_ExcludeOfAnExplicitItem_DeletesTheElement_AndItsEmptyGroup()
    {
        var text = """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework>
              </PropertyGroup>
              <ItemGroup>
                <Compile Include="..\Shared\Version.cs" Link="Properties\Version.cs" />
              </ItemGroup>
            </Project>

            """;
        var project = Write("App/App.csproj", text);
        Write("Shared/Version.cs", "static class Version { }");
        Write("App/Program.cs", "System.Console.WriteLine();");

        var outcome = Edit(project, Op("exclude", "Shared/Version.cs"));

        Assert.True(outcome.Changed);
        Assert.Equal("""
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework>
              </PropertyGroup>
            </Project>

            """, File.ReadAllText(project));
    }

    [Fact]
    public void Sdk_IncludeWithoutDefaultItems_AddsAnItemOfTheExtensionsType()
    {
        var text = """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework>
                <EnableDefaultItems>false</EnableDefaultItems>
              </PropertyGroup>
              <ItemGroup>
                <Compile Include="Program.cs" />
              </ItemGroup>
            </Project>

            """;
        var project = Write("App/App.csproj", text);
        Write("App/Program.cs", "System.Console.WriteLine();");
        Write("App/Util.cs", "class Util { }");
        Write("App/Strings.resx", "<root />");
        Write("App/readme.txt", "read me");

        var outcome = Edit(project, Op("include", "App/Util.cs"), Op("include", "App/Strings.resx"), Op("include", "App/readme.txt", itemType: "Content"));

        Assert.True(outcome.Changed);
        Assert.Equal("""
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <TargetFramework>net10.0</TargetFramework>
                <EnableDefaultItems>false</EnableDefaultItems>
              </PropertyGroup>
              <ItemGroup>
                <Compile Include="Program.cs" />
                <Compile Include="Util.cs" />
              </ItemGroup>
              <ItemGroup>
                <EmbeddedResource Include="Strings.resx" />
              </ItemGroup>
              <ItemGroup>
                <Content Include="readme.txt" />
              </ItemGroup>
            </Project>

            """, File.ReadAllText(project));
    }

    [Fact]
    public void Sdk_AddFolder_AddsAFolderItem_AndLegacyProjectsSayWhyNot()
    {
        var project = WriteSdk();
        Directory.CreateDirectory(Full("App/Empty"));
        var outcome = Edit(project, Op("addFolder", "App/Empty"));

        Assert.True(outcome.Results[0].Applied);
        Assert.Equal(
            SdkProject.Replace("\n\n</Project>", "\n\n  <ItemGroup>\n    <Folder Include=\"Empty\\\" />\n  </ItemGroup>\n\n</Project>", StringComparison.Ordinal),
            File.ReadAllText(project));
        Assert.False(Edit(project, Op("addFolder", "App/Empty")).Changed);

        var legacy = WriteCrlfBom("Legacy/Legacy.csproj", LegacyProject);
        var refused = Edit(legacy, Op("addFolder", "Legacy/Empty"));
        Assert.False(refused.Changed);
        Assert.False(string.IsNullOrEmpty(refused.Results[0].Message));
    }

    // ------------------------------------------------------------------ legacy projects

    private const string LegacyProject = """
        <?xml version="1.0" encoding="utf-8"?>
        <Project ToolsVersion="15.0" DefaultTargets="Build" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
          <PropertyGroup>
            <OutputType>WinExe</OutputType>
            <TargetFrameworkVersion>v4.7.2</TargetFrameworkVersion>
          </PropertyGroup>
          <ItemGroup>
            <Reference Include="System" />
          </ItemGroup>
          <ItemGroup>
            <Compile Include="Forms\Main.cs">
              <SubType>Form</SubType>
            </Compile>
            <Compile Include="Forms\Main.Designer.cs">
              <DependentUpon>Main.cs</DependentUpon>
            </Compile>
            <Compile Include="Program.cs" />
          </ItemGroup>
          <ItemGroup>
            <EmbeddedResource Include="Forms\Main.resx">
              <DependentUpon>Main.cs</DependentUpon>
            </EmbeddedResource>
          </ItemGroup>
          <Import Project="$(MSBuildToolsPath)\Microsoft.CSharp.targets" />
        </Project>
        """;

    [Fact]
    public void Legacy_IncludeAndExclude_KeepCrlfAndTheByteOrderMark()
    {
        var project = WriteCrlfBom("Legacy/Legacy.csproj", LegacyProject);
        Write("Legacy/Util.cs", "class Util { }");
        Write("Legacy/readme.txt", "read me");

        var outcome = Edit(project, Op("include", "Legacy/Util.cs"), Op("include", "Legacy/readme.txt"), Op("include", "Legacy/Program.cs"), Op("exclude", "Legacy/Program.cs"));

        Assert.True(outcome.Changed);
        Assert.Equal([true, true, false, true], outcome.Results.Select(r => r.Applied));
        var expected = LegacyProject
            .Replace("    <Compile Include=\"Program.cs\" />\n", "    <Compile Include=\"Util.cs\" />\n", StringComparison.Ordinal)
            .Replace("  <Import Project=", "  <ItemGroup>\n    <None Include=\"readme.txt\" />\n  </ItemGroup>\n  <Import Project=", StringComparison.Ordinal);
        Assert.Equal(CrlfBom(expected), File.ReadAllBytes(project));
    }

    [Fact]
    public void Legacy_RenameOfAFile_RewritesItsIncludeAndTheDependentUpons()
    {
        var project = WriteCrlfBom("Legacy/Legacy.csproj", LegacyProject);
        var outcome = Edit(project, Op("rename", "Legacy/Forms/Main.cs", to: "Legacy/Forms/Start.cs"));

        Assert.True(outcome.Changed);
        var expected = LegacyProject
            .Replace("Include=\"Forms\\Main.cs\"", "Include=\"Forms\\Start.cs\"", StringComparison.Ordinal)
            .Replace("<DependentUpon>Main.cs</DependentUpon>", "<DependentUpon>Start.cs</DependentUpon>", StringComparison.Ordinal);
        Assert.Equal(CrlfBom(expected), File.ReadAllBytes(project));
    }

    [Fact]
    public void Legacy_RenameOfAFolder_RewritesEveryPathUnderIt_AndAMoveRewritesDependentUpon()
    {
        var project = WriteCrlfBom("Legacy/Legacy.csproj", LegacyProject);
        Edit(project, Op("rename", "Legacy/Forms", to: "Legacy/Views"));
        var renamed = LegacyProject.Replace("Include=\"Forms\\", "Include=\"Views\\", StringComparison.Ordinal);
        Assert.Equal(CrlfBom(renamed), File.ReadAllBytes(project));

        // Program.cs moved into Views: nothing depends on it; Main.cs moved to the root: its dependents point up a folder.
        Edit(project, Op("rename", "Legacy/Views/Main.cs", to: "Legacy/Main.cs"));
        var moved = renamed
            .Replace("Include=\"Views\\Main.cs\"", "Include=\"Main.cs\"", StringComparison.Ordinal)
            .Replace("<DependentUpon>Main.cs</DependentUpon>", "<DependentUpon>..\\Main.cs</DependentUpon>", StringComparison.Ordinal);
        Assert.Equal(CrlfBom(moved), File.ReadAllBytes(project));
    }

    [Fact]
    public void Legacy_RemoveOfAFileAndOfAFolder_DeletesTheirElements_AndEmptyGroups()
    {
        var project = WriteCrlfBom("Legacy/Legacy.csproj", LegacyProject);
        var outcome = Edit(project, Op("remove", "Legacy/Program.cs"), Op("remove", "Legacy/Forms"), Op("remove", "Legacy/Missing.cs"));

        Assert.Equal([true, true, false], outcome.Results.Select(r => r.Applied));
        var expected = """
            <?xml version="1.0" encoding="utf-8"?>
            <Project ToolsVersion="15.0" DefaultTargets="Build" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
              <PropertyGroup>
                <OutputType>WinExe</OutputType>
                <TargetFrameworkVersion>v4.7.2</TargetFrameworkVersion>
              </PropertyGroup>
              <ItemGroup>
                <Reference Include="System" />
              </ItemGroup>
              <Import Project="$(MSBuildToolsPath)\Microsoft.CSharp.targets" />
            </Project>
            """;
        Assert.Equal(Lf(expected), Lf(Encoding.UTF8.GetString(File.ReadAllBytes(project)[3..])));
        Assert.Equal(CrlfBom(expected), File.ReadAllBytes(project));
    }

    // ------------------------------------------------------------------ F# compile order

    private const string FSharpProject = """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
          </PropertyGroup>
          <ItemGroup>
            <Compile Include="Types.fs" />
            <Compile Include="Logic.fs" />
            <Compile Include="Program.fs" />
          </ItemGroup>
        </Project>

        """;

    [Fact]
    public void FSharp_MoveUpAndDown_ReorderTheCompileElements()
    {
        var project = Write("Fs/Fs.fsproj", FSharpProject);

        var up = Edit(project, Op("moveUp", "Fs/Logic.fs"));
        Assert.True(up.Changed);
        Assert.Equal(FSharpProject.Replace(
            "    <Compile Include=\"Types.fs\" />\n    <Compile Include=\"Logic.fs\" />\n",
            "    <Compile Include=\"Logic.fs\" />\n    <Compile Include=\"Types.fs\" />\n",
            StringComparison.Ordinal), File.ReadAllText(project));

        var first = Edit(project, Op("moveUp", "Fs/Logic.fs"));
        Assert.False(first.Changed);
        Assert.Contains("already first", first.Results[0].Message, StringComparison.Ordinal);

        Assert.True(Edit(project, Op("moveDown", "Fs/Logic.fs"), Op("moveDown", "Fs/Logic.fs")).Changed);
        Assert.Equal(FSharpProject.Replace(
            "    <Compile Include=\"Logic.fs\" />\n    <Compile Include=\"Program.fs\" />\n",
            "    <Compile Include=\"Program.fs\" />\n    <Compile Include=\"Logic.fs\" />\n",
            StringComparison.Ordinal), File.ReadAllText(project));

        var last = Edit(project, Op("moveDown", "Fs/Logic.fs"));
        Assert.Contains("already last", last.Results[0].Message, StringComparison.Ordinal);
        Assert.False(Edit(project, Op("moveUp", "Fs/Missing.fs")).Results[0].Applied);
    }

    [Fact]
    public void ItemTypeFor_FollowsTheProjectsLanguageAndWebContent()
    {
        Assert.Equal("Compile", ProjectItemEditor.ItemTypeFor("/p/A.csproj", "/p/x.cs", web: false));
        Assert.Equal("None", ProjectItemEditor.ItemTypeFor("/p/A.fsproj", "/p/x.cs", web: false));
        Assert.Equal("Compile", ProjectItemEditor.ItemTypeFor("/p/A.fsproj", "/p/x.fs", web: false));
        Assert.Equal("Compile", ProjectItemEditor.ItemTypeFor("/p/A.vbproj", "/p/x.vb", web: false));
        Assert.Equal("EmbeddedResource", ProjectItemEditor.ItemTypeFor("/p/A.csproj", "/p/x.resx", web: false));
        Assert.Equal("Content", ProjectItemEditor.ItemTypeFor("/p/A.csproj", "/p/x.cshtml", web: true));
        Assert.Equal("None", ProjectItemEditor.ItemTypeFor("/p/A.csproj", "/p/x.json", web: false));
        Assert.Equal("Content", ProjectItemEditor.ItemTypeFor("/p/A.csproj", "/p/x.aspx", web: true));
    }

    // ------------------------------------------------------------------ the wire

    [Fact]
    public async Task Wire_EditItemsAndReload_AdvanceTheGeneration_AndTheNextTreeSeesTheChange()
    {
        var project = WriteSdk();
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        var target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null);
        var server = HostServer.RunAsync(serverStream, serverStream, target);
        using var client = TestRpc.Create(clientStream);
        client.StartListening();

        var (code, _) = await TestRpc.ErrorOfAsync(() => client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/reload", new { projectFiles = true }, Ct));
        Assert.Equal(HostErrors.ServerNotInitialized, code);
        await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "test", clientVersion = "0" }, Ct);

        // No solution open: the current generation, nothing reloaded.
        var none = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/reload", new { projectFiles = true }, Ct);
        Assert.Equal(0, none.GetProperty("generation").GetInt64());

        var generation = (await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = project }, Ct)).GetProperty("generation").GetInt64();

        var edit = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/editItems",
            new { project, edits = new object[] { new { op = "exclude", path = Full("App/Program.cs") }, new { op = "moveUp", path = Full("App/Program.cs") } } }, Ct);
        Assert.True(edit.GetProperty("changed").GetBoolean());
        Assert.Equal(generation + 1, edit.GetProperty("generation").GetInt64());
        var results = edit.GetProperty("edits").EnumerateArray().ToList();
        Assert.Equal(["exclude", "moveUp"], results.Select(r => r.GetProperty("op").GetString()));
        Assert.Equal(Full("App/Program.cs"), results[0].GetProperty("path").GetString());
        Assert.True(results[0].GetProperty("applied").GetBoolean());
        Assert.False(results[0].TryGetProperty("message", out _), "message is omitted when null");
        Assert.False(results[1].GetProperty("applied").GetBoolean());
        Assert.Equal(generation + 1, target.LanguageServer.Generation);

        var unchanged = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/editItems",
            new { project, edits = new[] { new { op = "remove", path = Full("App/Nothing.cs") } } }, Ct);
        Assert.False(unchanged.GetProperty("changed").GetBoolean());
        Assert.Equal(generation + 1, unchanged.GetProperty("generation").GetInt64());

        var tree = await client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct);
        Assert.DoesNotContain(Files(tree), f => f.EndsWith("Program.cs", StringComparison.Ordinal));

        // A file added on disk: reload without project files advances the generation and the tree evaluates the globs again.
        Write("App/Added.cs", "class Added { }");
        var files = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/reload", new { projectFiles = false }, Ct);
        Assert.Equal(generation + 2, files.GetProperty("generation").GetInt64());
        tree = await client.InvokeWithCancellationAsync<JsonElement>("eludite/solution/tree", [], Ct);
        Assert.Equal(generation + 2, tree.GetProperty("generation").GetInt64());
        Assert.Contains(Full("App/Added.cs"), Files(tree));

        var reopened = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/reload", new { projectFiles = true }, Ct);
        Assert.Equal(generation + 3, reopened.GetProperty("generation").GetInt64());
        Assert.Equal(generation + 3, target.LanguageServer.Generation);

        (code, _) = await TestRpc.ErrorOfAsync(() => client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/editItems",
            new { project, edits = new[] { new { op = "rename", path = Full("App/Added.cs") } } }, Ct));
        Assert.Equal(HostErrors.InvalidParams, code);
        (code, _) = await TestRpc.ErrorOfAsync(() => client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/editItems",
            new { project = Full("App/Program.cs"), edits = new[] { new { op = "include", path = Full("App/Added.cs") } } }, Ct));
        Assert.Equal(HostErrors.InvalidParams, code);

        client.Dispose();
        await server.WaitAsync(TimeSpan.FromSeconds(10), Ct);
    }

    private static List<string> Files(JsonElement tree) =>
        [.. tree.GetProperty("projects")[0].GetProperty("files").EnumerateArray().Select(f => f.GetProperty("path").GetString()!)];
}
