using System.Text;
using Eludite.Host.Rpc;
using Eludite.Host.Templates;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0072: <c>eludite/project/addItem</c> and <c>eludite/project/addExisting</c>: the namespace rule, the built-in
/// items, an SDK item template through the real <c>dotnet new</c>, the legacy project's <c>Compile</c> item written
/// byte for byte around it, and copies and links.
/// </summary>
public sealed class ItemTests : IDisposable
{
    private const string LegacyProject =
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n"
        + "<Project ToolsVersion=\"15.0\" xmlns=\"http://schemas.microsoft.com/developer/msbuild/2003\">\r\n"
        + "\t<!-- A legacy project, tabs and CRLF. -->\r\n"
        + "\t<PropertyGroup>\r\n"
        + "\t\t<RootNamespace>Legacy.App</RootNamespace>\r\n"
        + "\t\t<OutputType>Library</OutputType>\r\n"
        + "\t</PropertyGroup>\r\n"
        + "\t<ItemGroup>\r\n"
        + "\t\t<Compile Include=\"Program.cs\" />\r\n"
        + "\t\t<!-- keep me -->\r\n"
        + "\t\t<Compile Include=\"Zed.cs\" />\r\n"
        + "\t</ItemGroup>\r\n"
        + "\t<ItemGroup>\r\n"
        + "\t\t<Reference Include=\"System\" />\r\n"
        + "\t</ItemGroup>\r\n"
        + "\t<Import Project=\"$(MSBuildToolsPath)\\Microsoft.CSharp.targets\" />\r\n"
        + "</Project>\r\n";

    private const string SdkProject =
        "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>net10.0</TargetFramework>\n    <RootNamespace>My.App</RootNamespace>\n  </PropertyGroup>\n</Project>\n";

    private readonly string _root = Directory.CreateTempSubdirectory("eludite-items-").FullName;
    private (long Generation, string? Path) _current = (3, null);
    private readonly List<string> _reloads = [];

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    public void Dispose() => TestDirectory.Delete(_root);

    private ItemService Service(string? dotnetRoot = null) =>
        new(
            new TemplateCatalog(dotnetRoot ?? TemplateCatalog.LocateDotnetRoot() ?? _root, userPackages: Path.Combine(_root, "no-user-packages")),
            () => (_current.Generation, _current.Path, CancellationToken.None),
            path =>
            {
                _reloads.Add(path);
                return _current.Generation + 1;
            },
            TextWriter.Null);

    private string Write(string relative, string text)
    {
        var path = Path.Combine(_root, relative.Replace('/', Path.DirectorySeparatorChar));
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text, new UTF8Encoding(false));
        return path;
    }

    private static void SkipWithoutSdk() =>
        Assert.SkipWhen(TemplateCatalog.LocateDotnetRoot() is null, "No .NET SDK found (DOTNET_ROOT, PATH or the usual install folders).");

    [Fact]
    public void Namespace_IsTheRootNamespacePlusTheFolders()
    {
        var project = Write("App/App.csproj", SdkProject);
        Assert.Equal("My.App", ProjectNamespace.RootNamespace(project));
        Assert.Equal("My.App.Folder.Sub", ProjectNamespace.For(project, Path.Combine(_root, "App", "Folder", "Sub")));
        Assert.Equal("My.App", ProjectNamespace.For(project, Path.Combine(_root, "App")));
        Assert.Equal("My.App._1Bad_Name.x_y", ProjectNamespace.For(project, Path.Combine(_root, "App", "1Bad-Name", "x y")));
    }

    [Fact]
    public void Namespace_FallsBackToTheProjectsNameMadeAnIdentifier()
    {
        var project = Write("1Bad-Name/1Bad-Name.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\" />\n");
        Assert.Equal("_1Bad_Name", ProjectNamespace.RootNamespace(project));
        var dotted = Write("Company.Tool/Company.Tool.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <RootNamespace>$(MSBuildProjectName)</RootNamespace>\n  </PropertyGroup>\n</Project>\n");
        Assert.Equal("Company.Tool", ProjectNamespace.RootNamespace(dotted));
        Assert.Equal("_1Bad_Name", ProjectNamespace.Identifier("1Bad-Name"));
        Assert.Equal("_", ProjectNamespace.Identifier(string.Empty));
    }

    [Fact]
    public async Task AddItem_BuiltInClassWritesTheFileWithAFileScopedNamespace()
    {
        var project = Write("App/App.csproj", SdkProject);
        var before = await File.ReadAllBytesAsync(project, Ct);
        using var service = Service();
        var folder = Path.Combine(_root, "App", "Folder", "Sub");
        var result = await service.AddItemAsync(new ProjectAddItemParams(project, folder, "class", "Customer.cs"), Ct);

        var file = Path.Combine(folder, "Customer.cs");
        Assert.Equal([file], result.Files);
        Assert.Equal("My.App.Folder.Sub", result.Namespace);
        Assert.False(result.ProjectWritten);
        Assert.Equal(3, result.Generation);
        Assert.Null(result.Output);
        var bytes = await File.ReadAllBytesAsync(file, Ct);
        Assert.Equal(new byte[] { 0xEF, 0xBB, 0xBF }, bytes[..3]);
        Assert.Equal("namespace My.App.Folder.Sub;\n\npublic class Customer\n{\n}\n", Encoding.UTF8.GetString(bytes, 3, bytes.Length - 3));
        Assert.Equal(before, await File.ReadAllBytesAsync(project, Ct));
        Assert.Empty(_reloads);
    }

    [Fact]
    public async Task AddItem_BuiltInVisualBasicItemsNameOnlyTheFolders()
    {
        var project = Write("Vb/Vb.vbproj", "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <RootNamespace>Vb.Root</RootNamespace>\n  </PropertyGroup>\n</Project>\n");
        using var service = Service();
        var atRoot = await service.AddItemAsync(new ProjectAddItemParams(project, Path.Combine(_root, "Vb"), "module", "Helpers"), Ct);
        Assert.Equal("Vb.Root", atRoot.Namespace);
        Assert.Equal("Public Module Helpers\n\nEnd Module\n", Encoding.UTF8.GetString(await File.ReadAllBytesAsync(atRoot.Files[0], Ct))[1..]);

        var nested = await service.AddItemAsync(new ProjectAddItemParams(project, Path.Combine(_root, "Vb", "Models"), "eludite.vb.class", "Customer.vb"), Ct);
        Assert.Equal(Path.Combine(_root, "Vb", "Models", "Customer.vb"), nested.Files[0]);
        Assert.Equal("Namespace Models\n    Public Class Customer\n\n    End Class\nEnd Namespace\n", Encoding.UTF8.GetString(await File.ReadAllBytesAsync(nested.Files[0], Ct))[1..]);
    }

    [Fact]
    public async Task AddItem_RefusesAnExistingFileAFolderOutsideAndAnUnknownTemplate()
    {
        var project = Write("App/App.csproj", SdkProject);
        var existing = Write("App/Customer.cs", "// mine\n");
        using var service = Service();
        var exists = await Assert.ThrowsAsync<LocalRpcException>(() => service.AddItemAsync(new ProjectAddItemParams(project, Path.Combine(_root, "App"), "class", "Customer"), Ct));
        Assert.Equal(HostErrors.InvalidParams, exists.ErrorCode);
        Assert.Equal("// mine\n", await File.ReadAllTextAsync(existing, Ct));

        var outside = await Assert.ThrowsAsync<LocalRpcException>(() => service.AddItemAsync(new ProjectAddItemParams(project, _root, "class", "Other"), Ct));
        Assert.Equal(HostErrors.InvalidParams, outside.ErrorCode);

        var unknown = await Assert.ThrowsAsync<LocalRpcException>(() => service.AddItemAsync(new ProjectAddItemParams(project, Path.Combine(_root, "App"), "no-such-item-eludite", "Other"), Ct));
        Assert.Equal(HostErrors.InvalidParams, unknown.ErrorCode);
    }

    [Fact]
    public async Task AddItem_LegacyProjectGainsACompileItemAndKeepsEveryOtherByte()
    {
        SkipWithoutSdk();
        var project = Path.Combine(_root, "Legacy", "Legacy.csproj");
        Directory.CreateDirectory(Path.GetDirectoryName(project)!);
        await File.WriteAllTextAsync(project, LegacyProject, new UTF8Encoding(false), Ct);
        _current = (5, project);
        using var service = Service();
        var result = await service.AddItemAsync(new ProjectAddItemParams(project, Path.Combine(_root, "Legacy", "Models"), "class", "Customer"), Ct);

        Assert.True(result.ProjectWritten);
        Assert.Equal("Legacy.App.Models", result.Namespace);
        Assert.Equal(6, result.Generation);
        Assert.Equal([project], _reloads);
        var after = await File.ReadAllTextAsync(project, Ct);
        var lines = after.Split("\r\n");
        var inserted = Assert.Single(lines, l => l.Contains("Models\\Customer.cs", StringComparison.Ordinal));
        Assert.Equal("\t\t<Compile Include=\"Models\\Customer.cs\" />", inserted);
        Assert.Equal(LegacyProject, string.Join("\r\n", lines.Where(l => l != inserted)));
        Assert.DoesNotContain("\n", after.Replace("\r\n", string.Empty, StringComparison.Ordinal), StringComparison.Ordinal);
    }

    [Fact]
    public async Task AddItem_SdkItemTemplateRunsDotnetNew()
    {
        SkipWithoutSdk();
        var project = Write("App/App.csproj", SdkProject);
        using var service = Service();
        var identity = new TemplateCatalog(TemplateCatalog.LocateDotnetRoot()).Read(cancellationToken: Ct).Entries
            .Select(e => e.Info).First(t => t.Source == "sdk" && t.Type == "item" && t.Language == "C#" && t.ShortNames.Contains("interface")).Identity;
        var folder = Path.Combine(_root, "App", "Contracts");
        var result = await service.AddItemAsync(new ProjectAddItemParams(project, folder, identity, "IStore"), Ct);

        Assert.Equal(Path.Combine(folder, "IStore.cs"), result.Files[0]);
        Assert.True(File.Exists(result.Files[0]));
        Assert.Contains("IStore", await File.ReadAllTextAsync(result.Files[0], Ct), StringComparison.Ordinal);
        Assert.False(result.ProjectWritten);
        Assert.NotNull(result.Output);
    }

    [Fact]
    public async Task AddExisting_CopiesIntoAnSdkProjectWithoutWritingIt()
    {
        var project = Write("App/App.csproj", SdkProject);
        var before = await File.ReadAllBytesAsync(project, Ct);
        var source = Write("elsewhere/Util.cs", "class Util { }\n");
        var notes = Write("elsewhere/notes.txt", "notes\n");
        using var service = Service();
        var folder = Path.Combine(_root, "App", "Shared");
        var result = await service.AddExistingAsync(new ProjectAddExistingParams(project, folder, [source, notes]), Ct);

        Assert.Equal([Path.Combine(folder, "Util.cs"), Path.Combine(folder, "notes.txt")], result.Files);
        Assert.False(result.ProjectWritten);
        Assert.Equal(3, result.Generation);
        Assert.Equal("class Util { }\n", await File.ReadAllTextAsync(result.Files[0], Ct));
        Assert.Equal(before, await File.ReadAllBytesAsync(project, Ct));

        // Again: the copies exist now.
        var again = await Assert.ThrowsAsync<LocalRpcException>(() => service.AddExistingAsync(new ProjectAddExistingParams(project, folder, [source]), Ct));
        Assert.Equal(HostErrors.InvalidParams, again.ErrorCode);

        // A file already in the folder is left as it is.
        var kept = await service.AddExistingAsync(new ProjectAddExistingParams(project, folder, [result.Files[0]]), Ct);
        Assert.Equal([result.Files[0]], kept.Files);
    }

    [Fact]
    public async Task AddExisting_LinksIntoALegacyProject()
    {
        SkipWithoutSdk();
        var project = Path.Combine(_root, "Legacy", "Legacy.csproj");
        Directory.CreateDirectory(Path.GetDirectoryName(project)!);
        await File.WriteAllTextAsync(project, LegacyProject, new UTF8Encoding(false), Ct);
        var source = Write("Shared/Util.cs", "class Util { }\n");
        var readme = Write("Shared/readme.md", "# shared\n");
        using var service = Service();
        var result = await service.AddExistingAsync(new ProjectAddExistingParams(project, Path.Combine(_root, "Legacy", "Linked"), [source, readme]) { Link = true }, Ct);

        Assert.Equal([source, readme], result.Files);
        Assert.True(result.ProjectWritten);
        Assert.Equal(3, result.Generation);
        Assert.Empty(_reloads);
        Assert.False(Directory.Exists(Path.Combine(_root, "Legacy", "Linked")));
        var after = await File.ReadAllTextAsync(project, Ct);
        Assert.Contains("<Compile Include=\"..\\Shared\\Util.cs\" Link=\"Linked\\Util.cs\" />", after, StringComparison.Ordinal);
        Assert.Contains("<None Include=\"..\\Shared\\readme.md\" Link=\"Linked\\readme.md\" />", after, StringComparison.Ordinal);
        var kept = string.Join("\r\n", after.Split("\r\n").Where(l => !l.Contains("..\\Shared\\", StringComparison.Ordinal)));
        Assert.Contains("\t\t<!-- keep me -->\r\n", kept, StringComparison.Ordinal);
        Assert.StartsWith("<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n", after, StringComparison.Ordinal);
    }

    [Fact]
    public async Task AddExisting_CopyIntoALegacyProjectAddsItems()
    {
        SkipWithoutSdk();
        var project = Path.Combine(_root, "Legacy", "Legacy.csproj");
        Directory.CreateDirectory(Path.GetDirectoryName(project)!);
        await File.WriteAllTextAsync(project, LegacyProject, new UTF8Encoding(false), Ct);
        var source = Write("elsewhere/Util.cs", "class Util { }\n");
        var page = Write("elsewhere/page.html", "<p></p>\n");
        using var service = Service();
        var result = await service.AddExistingAsync(new ProjectAddExistingParams(project, Path.Combine(_root, "Legacy"), [source, page]), Ct);

        Assert.True(result.ProjectWritten);
        var after = await File.ReadAllTextAsync(project, Ct);
        Assert.Contains("<Compile Include=\"Util.cs\" />", after, StringComparison.Ordinal);
        Assert.Contains("<Content Include=\"page.html\" />", after, StringComparison.Ordinal);
    }
}
