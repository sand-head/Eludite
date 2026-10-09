using System.Diagnostics;
using System.Text;
using System.Text.Json;
using Eludite.Host.Projects;
using Eludite.Host.Rpc;
using Nerdbank.Streams;
using StreamJsonRpc;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0049: the project property pages' evaluation and edit on <c>corpus/projects</c> (copied per test): round trips
/// byte for byte, every catalog property read and written, the condition rule with and without existing groups, the
/// inherited rule, the second evaluation per configuration, the build events, a legacy project read-only, the cache per
/// generation, cancellation and the wire with the generation rule.
/// </summary>
public sealed class ProjectPropertiesTests : IDisposable
{
    private readonly ProjectCorpus _corpus = new();
    private readonly ProjectPropertyEvaluator _evaluator = new();

    private static CancellationToken Ct => TestContext.Current.CancellationToken;

    public void Dispose()
    {
        _evaluator.Dispose();
        _corpus.Dispose();
    }

    private ProjectEditOutcome Edit(string project, params PropertyEdit[] edits) => _evaluator.Edit(_corpus.Path(project), edits, Ct);

    private ProjectPropertiesResult Read(string project, string? configuration = null, string? platform = null, string? framework = null)
    {
        _evaluator.Reset();
        return _evaluator.Evaluate(_corpus.Path(project), configuration, platform, framework, 1, Ct);
    }

    private static PropertyValue Prop(ProjectPropertiesResult r, string name) => r.Properties.Single(p => p.Name == name);

    private static string Lf(string s) => s.Replace("\r\n", "\n", StringComparison.Ordinal);

    [Theory]
    [InlineData("Console/Console.csproj")]
    [InlineData("Tabs/Tabs.csproj")]
    [InlineData("Multi/Multi.csproj")]
    [InlineData("Inherited/Lib/Lib.csproj")]
    [InlineData("Inherited/Directory.Build.props")]
    [InlineData("Empty/Empty.csproj")]
    [InlineData("Web/Web.csproj")]
    [InlineData("Legacy/Legacy.csproj")]
    public void Corpus_RoundTripsByteForByte(string file)
    {
        var bytes = _corpus.Bytes(file);
        var format = TextFileFormat.Detect(bytes, out var text);
        Assert.Equal(bytes, format.Encode(ProjectPropertyEvaluator.Reserialize(text)));
    }

    [Fact]
    public void AnEdit_ChangesExactlyOneElement_AndKeepsTabsCrlfBomAndTheDeclaration()
    {
        var before = _corpus.Bytes("Tabs/Tabs.csproj");
        var outcome = Edit("Tabs/Tabs.csproj", new PropertyEdit("AssemblyName", "Tabs.Renamed"));

        Assert.True(outcome.Written);
        Assert.Equal("written", outcome.Results[0].Status);
        Assert.Equal(5, outcome.Results[0].Line);
        var expected = Encoding.UTF8.GetString(before).Replace("<AssemblyName>Tabs.Odd</AssemblyName>", "<AssemblyName>Tabs.Renamed</AssemblyName>", StringComparison.Ordinal);
        Assert.Equal(Encoding.UTF8.GetBytes(expected), _corpus.Bytes("Tabs/Tabs.csproj"));
        Assert.Equal([0xEF, 0xBB, 0xBF], _corpus.Bytes("Tabs/Tabs.csproj")[..3]);
    }

    [Fact]
    public void AddingAnUnconditionedProperty_AppendsToTheFirstUnconditionedGroup_WithItsIndentation()
    {
        var before = _corpus.Text("Tabs/Tabs.csproj");
        Edit("Tabs/Tabs.csproj", new PropertyEdit("Nullable", "enable"));

        var expected = before.Replace("\t\t<LangVersion>latest</LangVersion>\r\n", "\t\t<LangVersion>latest</LangVersion>\r\n\t\t<Nullable>enable</Nullable>\r\n", StringComparison.Ordinal);
        Assert.Equal(expected, _corpus.Text("Tabs/Tabs.csproj"));
        Assert.Equal("project", Prop(Read("Tabs/Tabs.csproj"), "Nullable").Source);
    }

    [Fact]
    public void AProjectWithoutAPropertyGroup_GetsOne()
    {
        Edit("Empty/Empty.csproj", new PropertyEdit("AssemblyName", "Renamed"));

        Assert.Equal(
            "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <AssemblyName>Renamed</AssemblyName>\n  </PropertyGroup>\n</Project>\n",
            _corpus.Text("Empty/Empty.csproj"));
    }

    [Fact]
    public void ConditionRule_AConfigurationWithoutAGroup_GetsOneAfterTheLastUnconditionedGroup()
    {
        var before = _corpus.Text("Console/Console.csproj");
        var outcome = Edit("Console/Console.csproj", new PropertyEdit("DefineConstants", "$(DefineConstants);DEBUG_ONLY") { Configuration = "Debug", Platform = "AnyCPU" });

        Assert.Equal("'$(Configuration)|$(Platform)'=='Debug|AnyCPU'", outcome.Results[0].Condition);
        Assert.Equal(12, outcome.Results[0].Line);
        var expected = before.Replace(
            "  </PropertyGroup>\n\n  <PropertyGroup Condition=\" '$(Configuration)",
            "  </PropertyGroup>\n  <PropertyGroup Condition=\"'$(Configuration)|$(Platform)'=='Debug|AnyCPU'\">\n    <DefineConstants>$(DefineConstants);DEBUG_ONLY</DefineConstants>\n  </PropertyGroup>\n\n  <PropertyGroup Condition=\" '$(Configuration)",
            StringComparison.Ordinal);
        Assert.Equal(expected, _corpus.Text("Console/Console.csproj"));

        // The second evaluation per configuration sees each configuration's own value and its source.
        var debug = Prop(Read("Console/Console.csproj", "Debug", "AnyCPU"), "DefineConstants");
        Assert.Contains("DEBUG_ONLY", debug.Value, StringComparison.Ordinal);
        Assert.DoesNotContain("RELEASE_ONLY", debug.Value, StringComparison.Ordinal);
        Assert.Equal("conditioned", debug.Source);
        var release = Prop(Read("Console/Console.csproj", "Release", "AnyCPU"), "DefineConstants");
        Assert.Contains("RELEASE_ONLY", release.Value, StringComparison.Ordinal);
        Assert.Equal("$(DefineConstants);RELEASE_ONLY", release.Raw);
        Assert.Equal(2, release.Conditions!.Count);
    }

    [Fact]
    public void ConditionRule_AnExistingGroupMatchesWhateverItsSpacing_AndADefaultRemovesTheElement()
    {
        var before = _corpus.Text("Console/Console.csproj");
        Edit("Console/Console.csproj", new PropertyEdit("DefineConstants", "$(DefineConstants);CHANGED") { Configuration = "Release", Platform = "Any CPU" });
        Assert.Equal(before.Replace(";RELEASE_ONLY<", ";CHANGED<", StringComparison.Ordinal), _corpus.Text("Console/Console.csproj"));

        // Release's default DebugType is portable: writing it removes the element; the group keeps DefineConstants.
        var outcome = Edit("Console/Console.csproj", new PropertyEdit("DebugType", "portable") { Configuration = "Release", Platform = "AnyCPU" });
        Assert.Equal("removed", outcome.Results[0].Status);
        Assert.DoesNotContain("<DebugType>", _corpus.Text("Console/Console.csproj"), StringComparison.Ordinal);
        Assert.Contains("CHANGED", _corpus.Text("Console/Console.csproj"), StringComparison.Ordinal);

        // null removes the last element, and the group with it.
        Edit("Console/Console.csproj", new PropertyEdit("DefineConstants", null) { Configuration = "Release", Platform = "AnyCPU" });
        Assert.DoesNotContain("Release|AnyCPU", _corpus.Text("Console/Console.csproj"), StringComparison.Ordinal);
        Assert.Equal("default", Prop(Read("Console/Console.csproj", "Release", "AnyCPU"), "DefineConstants").Source);
    }

    [Fact]
    public void ConditionRule_AConfigurationAloneMatchesItsGroup_AndAFrameworkGroupIsUsed()
    {
        var before = _corpus.Text("Tabs/Tabs.csproj");
        Edit("Tabs/Tabs.csproj", new PropertyEdit("DefineConstants", "$(DefineConstants);TABS_DEBUG;MORE") { Configuration = "Debug" });
        Assert.Equal(before.Replace(";TABS_DEBUG<", ";TABS_DEBUG;MORE<", StringComparison.Ordinal), _corpus.Text("Tabs/Tabs.csproj"));

        var multi = Read("Multi/Multi.csproj", framework: "net8.0");
        Assert.Equal(["net8.0", "net10.0"], multi.Frameworks);
        Assert.Contains("LEGACY_RUNTIME", Prop(multi, "DefineConstants").Value, StringComparison.Ordinal);
        Assert.DoesNotContain("LEGACY_RUNTIME", Prop(Read("Multi/Multi.csproj", framework: "net10.0"), "DefineConstants").Value, StringComparison.Ordinal);
        var outcome = Edit("Multi/Multi.csproj", new PropertyEdit("DefineConstants", "$(DefineConstants);LEGACY_RUNTIME;NET8") { Framework = "net8.0" });
        Assert.Equal("written", outcome.Results[0].Status);
        Assert.Contains("<DefineConstants>$(DefineConstants);LEGACY_RUNTIME;NET8</DefineConstants>", _corpus.Text("Multi/Multi.csproj"), StringComparison.Ordinal);
        Assert.Equal(2, _corpus.Text("Multi/Multi.csproj").Split("<PropertyGroup").Length - 1);
    }

    [Fact]
    public void AllConfigurations_WritesTheUnconditionedValue_AndRemovesTheConditionedOnes()
    {
        var outcome = Edit("Console/Console.csproj", new PropertyEdit("DebugType", "embedded") { AllConfigurations = true });

        Assert.Equal("written", outcome.Results[0].Status);
        Assert.Equal([" '$(Configuration)|$(Platform)' == 'Release|AnyCPU' "], outcome.Results[0].RemovedConditions);
        var text = _corpus.Text("Console/Console.csproj");
        Assert.Contains("    <RootNamespace>Corpus.Console</RootNamespace>\n    <DebugType>embedded</DebugType>\n  </PropertyGroup>", text, StringComparison.Ordinal);
        Assert.Single(text.Split("<DebugType>").Skip(1));
        Assert.Equal("embedded", Prop(Read("Console/Console.csproj", "Release", "AnyCPU"), "DebugType").Value);
        Assert.Throws<LocalRpcException>(() => Edit("Console/Console.csproj", new PropertyEdit("DebugType", "full") { AllConfigurations = true, Configuration = "Debug" }));
    }

    [Fact]
    public void InheritedRule_AnInheritedPropertyIsWrittenOnlyWithOverride()
    {
        var lib = Read("Inherited/Lib/Lib.csproj");
        var lang = Prop(lib, "LangVersion");
        Assert.Equal(("inherited", "12.0", true), (lang.Source, lang.Value, lang.Inherited));
        Assert.Equal(_corpus.Path("Inherited/Directory.Build.props"), lang.InheritedFrom);
        Assert.Equal(4, lang.DefinedIn!.Line);
        var before = _corpus.Bytes("Inherited/Lib/Lib.csproj");

        var refused = Edit("Inherited/Lib/Lib.csproj", new PropertyEdit("LangVersion", "13.0"));
        Assert.False(refused.Written);
        Assert.Equal("inherited", refused.Results[0].Status);
        Assert.Equal(_corpus.Path("Inherited/Directory.Build.props"), refused.Results[0].InheritedFrom);
        Assert.Equal(before, _corpus.Bytes("Inherited/Lib/Lib.csproj"));

        var written = Edit("Inherited/Lib/Lib.csproj", new PropertyEdit("LangVersion", "13.0") { Override = true });
        Assert.Equal("written", written.Results[0].Status);
        var after = Prop(Read("Inherited/Lib/Lib.csproj"), "LangVersion");
        Assert.Equal(("project", "13.0"), (after.Source, after.Value));
        Assert.Contains("<LangVersion>13.0</LangVersion>", _corpus.Text("Inherited/Lib/Lib.csproj"), StringComparison.Ordinal);

        // The inherited value is the default: overriding with it removes nothing and writes nothing.
        var same = Edit("Inherited/Lib/Lib.csproj", new PropertyEdit("Authors", "Corpus Authors") { Override = true });
        Assert.Equal("unchanged", same.Results[0].Status);
    }

    [Fact]
    public void EveryCatalogProperty_IsReadAndWritten_AndRemovingItRestoresTheFile()
    {
        var file = _corpus.Path("Empty/Empty.csproj");
        var original = File.ReadAllBytes(file);
        var defaults = Read("Empty/Empty.csproj");
        var debug = Read("Empty/Empty.csproj", "Debug", "AnyCPU");
        foreach (var entry in PropertyCatalog.Entries.Where(e => !e.WindowsOnly || OperatingSystem.IsWindows()))
        {
            File.WriteAllBytes(file, original);
            _evaluator.Reset();
            var current = Prop(entry.PerConfiguration ? debug : defaults, entry.Name).Value;
            var value = entry.Type switch
            {
                PropertyCatalog.Bool => string.Equals(entry.TrueValue ?? "true", current, StringComparison.OrdinalIgnoreCase) ? entry.FalseValue ?? "false" : entry.TrueValue ?? "true",
                PropertyCatalog.Enum => entry.Values!.Select(v => v.Value).First(v => !string.Equals(v, current, StringComparison.OrdinalIgnoreCase)),
                _ => entry.Name == "TargetFramework" ? "net8.0" : entry.Name == "TargetFrameworks" ? "net8.0;net10.0" : "corpus-" + entry.Name.ToLowerInvariant(),
            };
            var edit = entry.PerConfiguration ? new PropertyEdit(entry.Name, value) { Configuration = "Debug", Platform = "AnyCPU" } : new PropertyEdit(entry.Name, value);
            var outcome = Edit("Empty/Empty.csproj", edit);
            Assert.True(outcome.Written, entry.Name);
            Assert.Equal("written", outcome.Results[0].Status);
            Assert.NotNull(outcome.Results[0].Line);
            var read = Prop(_evaluator.Evaluate(file, entry.PerConfiguration ? "Debug" : null, entry.PerConfiguration ? "AnyCPU" : null, null, 1, Ct), entry.Name);
            if (entry.Name is not ("OutputPath" or "DefineConstants" or "NoWarn" or "WarningsAsErrors" or "TargetFrameworks"))
            {
                Assert.Equal(value, read.Value);
            }

            Assert.Equal(value, read.Raw);
            Assert.Equal(entry.PerConfiguration ? "conditioned" : "project", read.Source);

            var removed = Edit("Empty/Empty.csproj", entry.PerConfiguration ? new PropertyEdit(entry.Name, null) { Configuration = "Debug", Platform = "AnyCPU" } : new PropertyEdit(entry.Name, null));
            Assert.Equal("removed", removed.Results[0].Status);
            if (entry.PerConfiguration || entry.Target is not null)
            {
                // The conditioned group (or the target) goes with its last element: the file is as it was.
                Assert.Equal(original, File.ReadAllBytes(file));
            }
            else
            {
                Assert.DoesNotContain($"<{entry.Name}>", File.ReadAllText(file), StringComparison.Ordinal);
            }
        }
    }

    [Fact]
    public void BuildEvents_AreWrittenAsVisualStudiosTargets()
    {
        Edit("Console/Console.csproj", new PropertyEdit("PostBuildEvent", "echo built > out.txt"));
        var text = _corpus.Text("Console/Console.csproj");
        Assert.Contains("  <Target Name=\"PostBuild\" AfterTargets=\"PostBuildEvent\">\n    <Exec Command=\"echo built &gt; out.txt\" />\n  </Target>\n", text, StringComparison.Ordinal);
        var read = Prop(Read("Console/Console.csproj"), "PostBuildEvent");
        Assert.Equal(("echo built > out.txt", "project", "PostBuild"), (read.Value, read.Source, read.Target));

        Edit("Console/Console.csproj", new PropertyEdit("PostBuildEvent", "echo again"));
        Assert.Contains("<Exec Command=\"echo again\" />", _corpus.Text("Console/Console.csproj"), StringComparison.Ordinal);
        Edit("Console/Console.csproj", new PropertyEdit("PostBuildEvent", string.Empty));
        Assert.DoesNotContain("PostBuild", _corpus.Text("Console/Console.csproj"), StringComparison.Ordinal);
    }

    [Fact]
    public void ALegacyProject_IsReadOnly()
    {
        var legacy = Read("Legacy/Legacy.csproj", "Debug", "AnyCPU");
        Assert.Equal("legacy", legacy.Kind);
        Assert.All(legacy.Properties, p => Assert.True(p.ReadOnly));
        Assert.Equal("Legacy", Prop(legacy, "AssemblyName").Value);
        Assert.Equal(["net472"], legacy.Frameworks);
        var ex = Assert.Throws<LocalRpcException>(() => Edit("Legacy/Legacy.csproj", new PropertyEdit("AssemblyName", "Other")));
        Assert.Equal(HostErrors.InvalidParams, ex.ErrorCode);
    }

    [Fact]
    public void UnknownAndWindowsOnlyProperties_AreRefused()
    {
        Assert.Throws<LocalRpcException>(() => Edit("Console/Console.csproj", new PropertyEdit("NotACatalogProperty", "x")));
        if (!OperatingSystem.IsWindows())
        {
            Assert.Throws<LocalRpcException>(() => Edit("Console/Console.csproj", new PropertyEdit("ApplicationIcon", "app.ico")));
            Assert.True(Prop(Read("Console/Console.csproj"), "ApplicationIcon").ReadOnly);
        }
    }

    [Fact]
    public async Task Service_CachesPerGeneration_ReloadsAfterAWrite_AndRefusesAStaleGeneration()
    {
        long generation = 3;
        var reloaded = new List<string>();
        var solution = _corpus.Path("Corpus.slnx");
        using var cts = new CancellationTokenSource();
        using var service = new ProjectPropertiesService(
            () => (generation, solution, cts.Token),
            () => generation,
            path =>
            {
                reloaded.Add(path);
                return ++generation;
            },
            TextWriter.Null);
        var console = _corpus.Path("Console/Console.csproj");

        var sw = Stopwatch.StartNew();
        var first = await service.PropertiesAsync(new ProjectPropertiesParams(console), Ct);
        var cold = sw.Elapsed;
        sw.Restart();
        var again = await service.PropertiesAsync(new ProjectPropertiesParams(console), Ct);
        var cached = sw.Elapsed;
        Assert.Same(first, again);
        Assert.Equal(1, service.Evaluations);
        Budget.Assert("cached properties", cached, TimeSpan.FromMilliseconds(150));
        TestContext.Current.TestOutputHelper?.WriteLine($"[budget] properties: cold {cold.TotalMilliseconds:F1} ms, cached {cached.TotalMilliseconds:F2} ms");
        Assert.Equal(("Debug", "AnyCPU", 3L), (first.Configuration, first.Platform, first.Generation));
        Assert.Equal(PropertyCatalog.Entries.Count, first.Properties.Count);
        Assert.Equal(["application", "build", "package", "debug", "codeAnalysis", "resources", "settings", "signing"], first.Pages.Select(p => p.Id));

        // A stale generation writes nothing.
        var stale = await Assert.ThrowsAsync<LocalRpcException>(() => service.SetPropertyAsync(new ProjectSetPropertyParams(console, 2, [new PropertyEdit("AssemblyName", "X")]), Ct));
        Assert.Equal(HostErrors.ContentModified, stale.ErrorCode);
        Assert.Empty(reloaded);

        sw.Restart();
        var set = await service.SetPropertyAsync(new ProjectSetPropertyParams(console, 3, [new PropertyEdit("AssemblyName", "Saved")]), Ct);
        TestContext.Current.TestOutputHelper?.WriteLine($"[budget] save: {sw.Elapsed.TotalMilliseconds:F1} ms (without the reload)");
        Assert.True(set.Written);
        Assert.Equal(4, set.Generation);
        Assert.Equal([solution], reloaded);
        var after = await service.PropertiesAsync(new ProjectPropertiesParams(console), Ct);
        Assert.Equal("Saved", after.Properties.Single(p => p.Name == "AssemblyName").Value);
        Assert.Equal(4, after.Generation);
        Assert.Equal(2, service.Evaluations);

        // Unchanged edits do not reload.
        var none = await service.SetPropertyAsync(new ProjectSetPropertyParams(console, 4, [new PropertyEdit("AssemblyName", "Saved")]), Ct);
        Assert.False(none.Written);
        Assert.Equal(4, none.Generation);
        Assert.Single(reloaded);

        // Not a project of the solution.
        await Assert.ThrowsAsync<LocalRpcException>(() => service.PropertiesAsync(new ProjectPropertiesParams(_corpus.Path("Legacy/Legacy.csproj")), Ct));
    }

    [Fact]
    public async Task Service_UsesTheActiveSelectionsMapping_AndCancels()
    {
        long generation = 1;
        var solution = _corpus.Path("Corpus.sln");
        using var cts = new CancellationTokenSource();
        using var service = new ProjectPropertiesService(() => (generation, solution, cts.Token), () => generation, _ => ++generation, TextWriter.Null);
        var select = await service.SetConfigurationAsync(new SolutionSetConfigurationParams(1) { Select = new Selection("Release", "x64") }, Ct);
        Assert.False(select.Written);
        Assert.Equal(new Selection("Release", "x64"), select.Active);

        // Release|x64 maps Console to Release|Any CPU.
        var console = await service.PropertiesAsync(new ProjectPropertiesParams(_corpus.Path("Console/Console.csproj")), Ct);
        Assert.Equal(("Release", "AnyCPU"), (console.Configuration, console.Platform));
        Assert.Contains("RELEASE_ONLY", console.Properties.Single(p => p.Name == "DefineConstants").Value, StringComparison.Ordinal);

        using var canceled = new CancellationTokenSource();
        await canceled.CancelAsync();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => service.PropertiesAsync(new ProjectPropertiesParams(_corpus.Path("Tabs/Tabs.csproj")), canceled.Token));

        // The generation moving on makes a request in flight ContentModified.
        await cts.CancelAsync();
        var ex = await Assert.ThrowsAsync<LocalRpcException>(() => service.PropertiesAsync(new ProjectPropertiesParams(_corpus.Path("Multi/Multi.csproj")), Ct));
        Assert.Equal(HostErrors.ContentModified, ex.ErrorCode);
    }

    [Fact]
    public async Task Wire_PropertiesAndSetProperty_FollowTheGenerationRule()
    {
        var (clientStream, serverStream) = FullDuplexStream.CreatePair();
        var target = new HostRpcTarget(new FakeSdkDiscoverer(), TextWriter.Null);
        var server = HostServer.RunAsync(serverStream, serverStream, target);
        using var client = TestRpc.Create(clientStream);
        client.StartListening();
        var console = _corpus.Path("Console/Console.csproj");

        var (code, _) = await TestRpc.ErrorOfAsync(() => client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/properties", new { project = console }, Ct));
        Assert.Equal(HostErrors.ServerNotInitialized, code);
        await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/host/initialize", new { clientName = "test", clientVersion = "0" }, Ct);
        var open = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/open", new { path = _corpus.Path("Corpus.slnx") }, Ct);
        var generation = open.GetProperty("generation").GetInt64();

        var props = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/properties", new { project = console, configuration = "Release", platform = "AnyCPU" }, Ct);
        Assert.Equal(generation, props.GetProperty("generation").GetInt64());
        var define = props.GetProperty("properties").EnumerateArray().Single(p => p.GetProperty("name").GetString() == "DefineConstants");
        Assert.Equal("conditioned", define.GetProperty("source").GetString());
        Assert.True(define.GetProperty("perConfiguration").GetBoolean());
        Assert.Equal(13, define.GetProperty("definedIn").GetProperty("line").GetInt32());

        (code, var data) = await TestRpc.ErrorOfAsync(() => client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/setProperty",
            new { project = console, generation = generation - 1, edits = new[] { new { name = "AssemblyName", value = "Wire" } } }, Ct));
        Assert.Equal(HostErrors.ContentModified, code);
        Assert.Equal(generation, data!.Value.GetProperty("currentGeneration").GetInt64());

        var set = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/project/setProperty",
            new { project = console, generation, edits = new object[] { new { name = "AssemblyName", value = "Wire" }, new { name = "Optimize", value = "true", configuration = "Debug", platform = "AnyCPU" } } }, Ct);
        Assert.True(set.GetProperty("written").GetBoolean());
        Assert.Equal(generation + 1, set.GetProperty("generation").GetInt64());
        Assert.Equal(["written", "written"], set.GetProperty("results").EnumerateArray().Select(r => r.GetProperty("status").GetString()));
        Assert.Equal(generation + 1, target.LanguageServer.Generation);

        var configurations = await client.InvokeWithParameterObjectAsync<JsonElement>("eludite/solution/configurations", new { }, Ct);
        Assert.Equal("slnx", configurations.GetProperty("format").GetString());

        client.Dispose();
        await server.WaitAsync(TimeSpan.FromSeconds(10), Ct);
    }
}
