using Eludite.Host.Legacy;

namespace Eludite.Host.Tests;

/// <summary>Brief 0003: the legacy design-time evaluators, on a synthetic non-SDK WebForms project.</summary>
public sealed class LegacyEvaluatorTests : IDisposable
{
    private readonly DirectoryInfo _dir = Directory.CreateTempSubdirectory("eludite-legacy-");

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

    // A WebForms project as Visual Studio writes it: VSToolsPath falls back to MSBuildExtensionsPath32, which only
    // Visual Studio and Mono populate with Microsoft.WebApplication.targets.
    private const string LegacyWebProject = """
        <?xml version="1.0" encoding="utf-8"?>
        <Project ToolsVersion="15.0" DefaultTargets="Build" xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
          <Import Project="$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props" Condition="Exists('$(MSBuildExtensionsPath)\$(MSBuildToolsVersion)\Microsoft.Common.props')" />
          <PropertyGroup>
            <Configuration Condition=" '$(Configuration)' == '' ">Debug</Configuration>
            <ProjectTypeGuids>{349c5851-65df-11da-9384-00065b846f21};{fae04ec0-301f-11d3-bf4b-00c04f79efbc}</ProjectTypeGuids>
            <OutputType>Library</OutputType>
            <AssemblyName>LegacyShop</AssemblyName>
            <TargetFrameworkVersion>v4.7.2</TargetFrameworkVersion>
            <OutputPath>bin\</OutputPath>
            <DefineConstants>DEBUG;TRACE</DefineConstants>
          </PropertyGroup>
          <ItemGroup>
            <Reference Include="System" />
            <Reference Include="System.Web" />
            <Reference Include="Missing.Lib"><HintPath>..\packages\Missing.Lib.1.0.0\lib\net45\Missing.Lib.dll</HintPath></Reference>
          </ItemGroup>
          <ItemGroup>
            <Content Include="Default.aspx" />
            <Compile Include="Default.aspx.cs"><DependentUpon>Default.aspx</DependentUpon></Compile>
            <Compile Include="Default.aspx.designer.cs"><DependentUpon>Default.aspx</DependentUpon></Compile>
          </ItemGroup>
          <PropertyGroup>
            <VisualStudioVersion Condition="'$(VisualStudioVersion)' == ''">10.0</VisualStudioVersion>
            <VSToolsPath Condition="'$(VSToolsPath)' == ''">$(MSBuildExtensionsPath32)\Microsoft\VisualStudio\v$(VisualStudioVersion)</VSToolsPath>
          </PropertyGroup>
          <Import Project="$(MSBuildBinPath)\Microsoft.CSharp.targets" />
          <Import Project="$(VSToolsPath)\WebApplications\Microsoft.WebApplication.targets" />
        </Project>
        """;

    private string WriteProject()
    {
        var dir = Path.Combine(_dir.FullName, "LegacyShop");
        Directory.CreateDirectory(dir);
        File.WriteAllText(Path.Combine(dir, "LegacyShop.csproj"), LegacyWebProject);
        File.WriteAllText(Path.Combine(dir, "Default.aspx"), """
            <%@ Page Language="C#" CodeBehind="Default.aspx.cs" Inherits="LegacyShop.Default" %>
            <form id="form1" runat="server"><asp:Button ID="Save" runat="server" /></form>
            """);
        File.WriteAllText(Path.Combine(dir, "Default.aspx.cs"), "namespace LegacyShop { public partial class Default : System.Web.UI.Page { } }");
        File.WriteAllText(Path.Combine(dir, "Default.aspx.designer.cs"), "namespace LegacyShop { public partial class Default { protected global::System.Web.UI.HtmlControls.HtmlForm form1; } }");
        return Path.Combine(dir, "LegacyShop.csproj");
    }

    private static void SkipWithoutReferenceAssemblies() =>
        Assert.SkipWhen(new ReferenceAssemblies().RootFor("v4.7.2") is null, "Microsoft.NETFramework.ReferenceAssemblies.net472 is not in the NuGet cache.");

    [Fact]
    public void InProcessSdk_IgnoresVisualStudioOnlyImports_AndResolvesFromReferencePackages()
    {
        SkipWithoutReferenceAssemblies();
        var project = WriteProject();

        var result = Assert.Single(new InProcessMsBuildEvaluator(new ReferenceAssemblies(), Path.Combine(_dir.FullName, "work")).Evaluate([project], Ct));

        Assert.True(result.Loaded, result.FailureReason);
        Assert.Equal(EvaluatorKind.Sdk, result.Evaluator);
        Assert.Equal("v4.7.2", result.TargetFrameworkVersion);
        Assert.Equal(["DEBUG", "TRACE"], result.DefineConstants);
        Assert.Equal(["Default.aspx.cs", "Default.aspx.designer.cs"], result.CompileItems.Select(Path.GetFileName));
        Assert.Contains(result.ReferencePaths, r => r.EndsWith("System.Web.dll", StringComparison.Ordinal) && r.Contains("v4.7.2", StringComparison.Ordinal));
        Assert.Contains(result.ReferencePaths, r => r.EndsWith("mscorlib.dll", StringComparison.Ordinal));
        Assert.Equal(["Missing.Lib"], result.UnresolvedReferences);
        Assert.Equal("Default.aspx", Path.GetFileName(Assert.Single(result.MarkupItems)));
        Assert.Contains(result.MissingImports, m => m.Contains("Microsoft.WebApplication.targets", StringComparison.Ordinal));
    }

    [Fact]
    public void InProcessSdk_Strict_FailsOnWebTargets_AsADiagnostic()
    {
        var project = WriteProject();

        var result = Assert.Single(new InProcessMsBuildEvaluator(new ReferenceAssemblies(), Path.Combine(_dir.FullName, "work")) { IgnoreMissingImports = false }.Evaluate([project], Ct));

        Assert.False(result.Loaded);
        Assert.Equal(FailureClassifier.WebTargets, result.FailureClass);
        Assert.Contains("Microsoft.WebApplication.targets", result.FailureReason, StringComparison.Ordinal);
    }

    [Fact]
    public async Task DotnetMsBuildCommandLine_FailsOnWebTargets_AsADiagnostic()
    {
        var project = WriteProject();
        var evaluator = new CommandLineMsBuildEvaluator(MsBuildCommand.ForDotnetSdk(), new ReferenceAssemblies(), Path.Combine(_dir.FullName, "work"));

        var result = Assert.Single(await evaluator.EvaluateAsync([project], Ct));

        Assert.False(result.Loaded);
        Assert.Equal(FailureClassifier.WebTargets, result.FailureClass);
        Assert.Contains(result.Diagnostics, d => d.Code == "MSB4019");
    }

    [Fact]
    public async Task MonoMsBuild_LoadsWebProject_AndDesignerInjectionSwapsCompileItems()
    {
        SkipWithoutReferenceAssemblies();
        var mono = MonoInstallation.Locate();
        Assert.SkipWhen(mono is null, "Mono MSBuild not found; see tools/legacy-load/README.md.");
        var project = WriteProject();
        var work = Path.Combine(_dir.FullName, "work");
        var evaluator = new CommandLineMsBuildEvaluator(MsBuildCommand.ForMono(mono!), new ReferenceAssemblies(), work);

        var first = Assert.Single(await evaluator.EvaluateAsync([project], Ct));
        Assert.True(first.Loaded, first.FailureReason);
        Assert.Equal(EvaluatorKind.Mono, first.Evaluator);
        Assert.Contains(first.ReferencePaths, r => r.EndsWith("System.Web.dll", StringComparison.Ordinal));
        Assert.Empty(first.MissingImports);

        // Default (supplement): the checked-in designer stays, the generated partial adds only the markup-only field.
        var designers = new WebFormsDesignerService(Path.Combine(work, "generated")).Generate(first);
        var generated = Assert.Single(designers);
        Assert.Equal(["Save"], generated.Fields);
        Assert.Equal(["form1", "Save"], generated.MarkupFields.Select(f => f.Name));
        Assert.Null(generated.ReplacedDesignerFile);
        Assert.EndsWith("Default.aspx.designer.cs", generated.ExistingDesignerFile, StringComparison.Ordinal);
        var text = File.ReadAllText(generated.GeneratedFile);
        Assert.Contains("global::System.Web.UI.WebControls.Button Save;", text, StringComparison.Ordinal);
        Assert.DoesNotContain("form1", text, StringComparison.Ordinal);

        var targets = Path.Combine(work, "inject.targets");
        WebFormsDesignerService.WriteInjectionTargets(targets, [(first.ProjectPath, designers)]);
        var injected = new CommandLineMsBuildEvaluator(MsBuildCommand.ForMono(mono!), new ReferenceAssemblies(), work) { ExtraTargets = targets };
        var second = Assert.Single(await injected.EvaluateAsync([project], Ct));
        Assert.Equal(["Default.aspx.cs", "Default.aspx.designer.cs", "Default.aspx.g.cs"], second.CompileItems.Select(Path.GetFileName));

        // Replace: the generated partial swaps the checked-in designer.
        var replacing = new WebFormsDesignerService(Path.Combine(work, "replaced"), DesignerMode.Replace).Generate(first);
        Assert.Equal(["form1", "Save"], Assert.Single(replacing).Fields);
        WebFormsDesignerService.WriteInjectionTargets(targets, [(first.ProjectPath, replacing)]);
        var third = Assert.Single(await injected.EvaluateAsync([project], Ct));
        Assert.Equal(["Default.aspx.cs", "Default.aspx.g.cs"], third.CompileItems.Select(Path.GetFileName));
    }

    [Fact]
    public void Generate_SkipsFieldsDeclaredInCodeBehind_AndNonPartialCodeBehind()
    {
        var project = WriteProject();
        var dir = Path.GetDirectoryName(project)!;
        File.Delete(Path.Combine(dir, "Default.aspx.designer.cs"));
        var evaluation = new LegacyProjectEvaluation
        {
            ProjectPath = project,
            Evaluator = "test",
            Loaded = true,
            CompileItems = [Path.Combine(dir, "Default.aspx.cs")],
            MarkupItems = [Path.Combine(dir, "Default.aspx")],
        };
        var service = new WebFormsDesignerService(Path.Combine(_dir.FullName, "gen"));

        // Partial code-behind that declares one control itself: only the other control gets a field.
        File.WriteAllText(Path.Combine(dir, "Default.aspx.cs"), "namespace LegacyShop { public partial class Default : System.Web.UI.Page { protected System.Web.UI.WebControls.Button Save; } }");
        Assert.Equal(["form1"], Assert.Single(service.Generate(evaluation)).Fields);

        // ASP.NET 1.x style: the class is not partial and declares its controls; a partial would be a second type.
        File.WriteAllText(Path.Combine(dir, "Default.aspx.cs"), "namespace LegacyShop {\n  /// <summary>Default page.</summary>\n  public class Default : System.Web.UI.Page { } }");
        Assert.Empty(service.Generate(evaluation));
    }

    [Fact]
    public void Supplement_KeepsStaleDesignerFields_AndAddsOnlyMarkupOnlyFields()
    {
        var project = WriteProject();
        var dir = Path.GetDirectoryName(project)!;
        // A stale checked-in designer: declares a field whose control is no longer in the markup (seen in the corpus).
        File.WriteAllText(Path.Combine(dir, "Default.aspx.designer.cs"), """
            namespace LegacyShop {
                public partial class Default {
                    protected global::System.Web.UI.HtmlControls.HtmlForm form1;
                    protected global::System.Web.UI.WebControls.Label Removed;
                }
            }
            """);
        var evaluation = new LegacyProjectEvaluation
        {
            ProjectPath = project,
            Evaluator = "test",
            Loaded = true,
            CompileItems = [Path.Combine(dir, "Default.aspx.cs"), Path.Combine(dir, "Default.aspx.designer.cs")],
            MarkupItems = [Path.Combine(dir, "Default.aspx")],
        };

        var generated = Assert.Single(new WebFormsDesignerService(Path.Combine(_dir.FullName, "gen")).Generate(evaluation));

        Assert.Equal(["Save"], generated.Fields);
        Assert.Null(generated.ReplacedDesignerFile);
        Assert.Equal(["Removed", "form1"], WebFormsDesignerService.DeclaredFieldNames(File.ReadAllText(generated.ExistingDesignerFile!)).Order(StringComparer.Ordinal));
    }

    [Fact]
    public void ReadDump_ParsesItemsAndComputesUnresolvedReferences()
    {
        var project = Path.Combine(_dir.FullName, "P", "P.csproj");
        var result = CommandLineMsBuildEvaluator.ReadDump(
            [
                "project\t" + project,
                "prop\tTargetFrameworkVersion\tv4.8",
                "prop\tDefineConstants\tDEBUG;TRACE",
                "prop\tAllowUnsafeBlocks\ttrue",
                "prop\tLangVersion\t",
                "compile\t/src/A.cs",
                "compile\t/src/A.cs",
                "refpath\t/ref/System.dll\tSystem",
                "reference\tSystem",
                "reference\tNewtonsoft.Json",
                "content\t/src/Default.aspx",
                "content\t/src/site.css",
                "package\tZXing.Net",
            ],
            project,
            EvaluatorKind.Mono,
            [],
            TimeSpan.FromMilliseconds(12));

        Assert.True(result.Loaded);
        Assert.Equal("v4.8", result.TargetFrameworkVersion);
        Assert.True(result.AllowUnsafeBlocks);
        Assert.Null(result.LangVersion);
        Assert.Equal(["/src/A.cs"], result.CompileItems);
        Assert.Equal(["Newtonsoft.Json"], result.UnresolvedReferences);
        Assert.Equal(["/src/Default.aspx"], result.MarkupItems);
        Assert.Equal(["ZXing.Net"], result.PackageReferences);
    }

    [Fact]
    public void ParseDiagnostics_AttributesToProjects()
    {
        const string output = """
            /p/A.csproj(375,3): error MSB4019: The imported project "/x/WebApplications/Microsoft.WebApplication.targets" was not found. [/p/A.csproj]
            /usr/lib/mono/msbuild/Current/bin/Microsoft.Common.CurrentVersion.targets(2101,5): warning MSB3245: Could not resolve this reference. [/p/B.csproj]
            Build started.
            """;

        var diagnostics = CommandLineMsBuildEvaluator.ParseDiagnostics(output);

        Assert.Equal(2, diagnostics.Count);
        Assert.Equal(("/p/A.csproj", "error", "MSB4019"), (diagnostics[0].Project, diagnostics[0].Diagnostic.Severity, diagnostics[0].Diagnostic.Code));
        Assert.Equal(("/p/B.csproj", "warning", "MSB3245"), (diagnostics[1].Project, diagnostics[1].Diagnostic.Severity, diagnostics[1].Diagnostic.Code));
    }

    [Theory]
    [InlineData("MSB4019", "The imported project \"/x/Microsoft/VisualStudio/v17.0/WebApplications/Microsoft.WebApplication.targets\" was not found.", FailureClassifier.WebTargets)]
    [InlineData("MSB4019", "The imported project \"/x/Microsoft.TypeScript.targets\" was not found.", FailureClassifier.MissingTargets)]
    [InlineData("MSB4019", "The imported project \"/s/packages/AutoMapper.3.3.1/tools/AutoMapper.targets\" was not found.", FailureClassifier.Packages)]
    [InlineData("MSB3283", "Cannot find wrapper assembly for type library \"Shell32\".", FailureClassifier.Com)]
    [InlineData(null, "Something else entirely.", FailureClassifier.Other)]
    public void Classifier_Buckets(string? code, string message, string expected) =>
        Assert.Equal(expected, FailureClassifier.Classify(code, message));

    [Fact]
    public void SolutionProjects_ReadsSlnAndSlnx_AndDetectsLegacy()
    {
        var project = WriteProject();
        var sdk = Path.Combine(_dir.FullName, "Modern", "Modern.csproj");
        Directory.CreateDirectory(Path.GetDirectoryName(sdk)!);
        File.WriteAllText(sdk, "<Project Sdk=\"Microsoft.NET.Sdk\" />");
        // Brief 0057: Visual Basic and F# projects are listed alike (a legacy WebForms site in VB among them).
        var vb = Path.Combine(_dir.FullName, "Basic", "Basic.vbproj");
        Directory.CreateDirectory(Path.GetDirectoryName(vb)!);
        File.WriteAllText(vb, """<Project ToolsVersion="15.0" xmlns="http://schemas.microsoft.com/developer/msbuild/2003"><ItemGroup><Content Include="Default.aspx" /></ItemGroup></Project>""");
        var fs = Path.Combine(_dir.FullName, "Functional", "Functional.fsproj");
        Directory.CreateDirectory(Path.GetDirectoryName(fs)!);
        File.WriteAllText(fs, "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><Compile Include=\"Library.fs\" /></ItemGroup></Project>");
        var sln = Path.Combine(_dir.FullName, "All.sln");
        File.WriteAllText(sln, """
            Project("{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}") = "LegacyShop", "LegacyShop\LegacyShop.csproj", "{6A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}"
            EndProject
            Project("{9A19103F-16F7-4668-BE54-9A1E7A4F7556}") = "Modern", "Modern\Modern.csproj", "{1A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}"
            EndProject
            Project("{2150E333-8FDC-42A3-9474-1A3956D46DE8}") = "Solution Items", "Solution Items", "{515655AC-4716-420E-BBA9-318680DBF355}"
            EndProject
            Project("{F184B08F-C81C-45F6-A57F-5ABD9991F28F}") = "Basic", "Basic\Basic.vbproj", "{3A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}"
            EndProject
            Project("{6EC3EE1D-3C4E-46DD-8F32-0CC8E7565705}") = "Functional", "Functional\Functional.fsproj", "{4A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}"
            EndProject
            Project("{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}") = "Gone", "Gone\Gone.csproj", "{2A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}"
            EndProject
            Project("{54435603-DBB4-11D2-8724-00A0C9A8B90C}") = "Setup", "Setup\Setup.vdproj", "{5A1F4B39-8E2C-4D1A-9F3B-2C7E5D8A1B0C}"
            EndProject
            """);
        var slnx = Path.Combine(_dir.FullName, "All.slnx");
        File.WriteAllText(slnx, "<Solution><Project Path=\"Functional/Functional.fsproj\" /><Project Path=\"Modern/Modern.csproj\" /><Project Path=\"Basic/Basic.vbproj\" /><Project Path=\"LegacyShop/LegacyShop.csproj\" /><Project Path=\"Setup/Setup.vdproj\" /></Solution>");

        Assert.Equal([project, sdk, vb, fs], SolutionProjects.Read(sln));
        Assert.Equal([fs, sdk, vb, project], SolutionProjects.Read(slnx));
        Assert.Equal([project], SolutionProjects.Read(project));
        Assert.Equal([vb], SolutionProjects.Read(vb));
        Assert.Equal([fs], SolutionProjects.Read(fs));
        Assert.Empty(SolutionProjects.Read(Path.Combine(_dir.FullName, "Gone", "Gone.fsproj")));
        Assert.True(SolutionProjects.IsProjectFile("A.csproj"));
        Assert.True(SolutionProjects.IsProjectFile("A.VbProj"));
        Assert.True(SolutionProjects.IsProjectFile("A.fsproj"));
        Assert.False(SolutionProjects.IsProjectFile("A.vdproj"));
        Assert.False(SolutionProjects.IsProjectFile("A.sln"));
        Assert.True(SolutionProjects.IsLegacy(project));
        Assert.False(SolutionProjects.IsLegacy(sdk));
        Assert.True(SolutionProjects.IsLegacy(vb));
        Assert.False(SolutionProjects.IsLegacy(fs));
        Assert.True(SolutionProjects.MentionsMarkup(project));
        Assert.True(SolutionProjects.MentionsMarkup(vb));
        Assert.False(SolutionProjects.MentionsMarkup(fs));
    }

    [Fact]
    public void InjectionTargets_AreScopedPerProjectAndEscaped()
    {
        var path = Path.Combine(_dir.FullName, "inject.targets");
        WebFormsDesignerService.WriteInjectionTargets(path,
        [
            ("/p/A.csproj", [new GeneratedDesigner("/p/Default.aspx", "/cache/A/Default.aspx.g.cs", "/p/Default.aspx.designer.cs", ["form1"], [])]),
            ("/p/B;C.csproj", [new GeneratedDesigner("/p/x.ascx", "/cache/B/x%.ascx.g.cs", null, ["pager"], [])]),
            ("/p/Empty.csproj", []),
            ("/p/Complete.csproj", [new GeneratedDesigner("/p/y.aspx", "/cache/C/y.aspx.g.cs", null, [], [])]),
        ]);

        var text = File.ReadAllText(path);
        Assert.Contains("<ItemGroup Condition=\"'$(MSBuildProjectFullPath)' == '/p/A.csproj'\">", text, StringComparison.Ordinal);
        Assert.Contains("<Compile Remove=\"/p/Default.aspx.designer.cs\" />", text, StringComparison.Ordinal);
        Assert.Contains("<Compile Include=\"/cache/A/Default.aspx.g.cs\" />", text, StringComparison.Ordinal);
        Assert.Contains("'/p/B%3BC.csproj'", text, StringComparison.Ordinal);
        Assert.Contains("x%25.ascx.g.cs", text, StringComparison.Ordinal);
        Assert.DoesNotContain("Empty.csproj", text, StringComparison.Ordinal);
        // A checked-in designer that already declares every field gets nothing injected.
        Assert.DoesNotContain("Complete.csproj", text, StringComparison.Ordinal);
    }

    [Fact]
    public void ReferenceAssemblies_FindsPackageRoots_AndBuildsMergedRoot()
    {
        var packages = Path.Combine(_dir.FullName, "packages");
        Directory.CreateDirectory(Path.Combine(packages, "microsoft.netframework.referenceassemblies.net472", "1.0.3", "build", ".NETFramework", "v4.7.2"));
        Directory.CreateDirectory(Path.Combine(packages, "microsoft.netframework.referenceassemblies.net40", "1.0.3", "build", ".NETFramework", "v4.0"));
        var refs = new ReferenceAssemblies(packages);

        Assert.Equal(Path.Combine(packages, "microsoft.netframework.referenceassemblies.net472", "1.0.3", "build") + Path.DirectorySeparatorChar, refs.RootFor("v4.7.2"));
        Assert.Null(refs.RootFor("v4.8"));
        var merged = refs.MergedRoot(Path.Combine(_dir.FullName, "cache"));
        Assert.NotNull(merged);
        Assert.True(Directory.Exists(Path.Combine(merged, ".NETFramework", "v4.7.2")));
        Assert.True(Directory.Exists(Path.Combine(merged, ".NETFramework", "v4.0")));
        Assert.Equal(merged, refs.MergedRoot(Path.Combine(_dir.FullName, "cache"))); // idempotent
    }
}
