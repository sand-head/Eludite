using System.Text.Json;

namespace Eludite.TestBridge.Tests;

/// <summary>Brief 0035: both protocols' shapes map to one model (host-rpc.md, "Tests", the mapping table).</summary>
public sealed class MappingTests
{
    [Fact]
    public void MtpNode_MapsToTheModel()
    {
        var node = JsonDocument.Parse("""
            {"uid": "u1", "display-name": "N.C.M(a: 1)", "location.file": "/s/C.cs", "location.line-start": 12,
             "location.type": "N.Sub.C+Inner", "location.method": "M(System.Int32)", "traits": [{"Category": "Math"}, {"Rows": ""}],
             "node-type": "action", "execution-state": "failed", "time.duration-ms": 3.5, "error.message": "boom",
             "error.stacktrace": "at N.C.M() in /s/C.cs:line 14", "standardOutput": "out\n", "standardError": "err"}
            """).RootElement;
        var item = MtpMapping.Item(node);
        Assert.Equal("u1", item.Id);
        Assert.Equal("N.C.M(a: 1)", item.DisplayName);
        Assert.Equal("N.Sub.C+Inner.M", item.FullyQualifiedName);
        Assert.Equal("N.Sub", item.Namespace);
        Assert.Equal("C+Inner", item.ClassName);
        Assert.Equal("M", item.Method);
        Assert.Equal("/s/C.cs", item.Source);
        Assert.Equal(12, item.Line);
        Assert.Equal([new TestTrait("Category", "Math"), new TestTrait("Category", "Rows")], item.Traits);

        var result = MtpMapping.Result(node, MtpMapping.Outcome("failed")!);
        Assert.Equal(TestOutcomes.Failed, result.Outcome);
        Assert.Equal(3.5, result.DurationMs);
        Assert.Equal("boom", result.Message);
        Assert.Equal("out\nerr", result.Output);

        Assert.Equal(TestOutcomes.Running, MtpMapping.Outcome("in-progress"));
        Assert.Equal(TestOutcomes.Failed, MtpMapping.Outcome("timed-out"));
        Assert.Equal(TestOutcomes.Failed, MtpMapping.Outcome("error"));
        Assert.Equal(TestOutcomes.NotRun, MtpMapping.Outcome("cancelled"));
        Assert.Null(MtpMapping.Outcome("discovered"));
    }

    [Fact]
    public void VsTestCaseAndResult_MapToTheModel()
    {
        var testCase = """
            {"Id": "id-1", "FullyQualifiedName": "N.C.M(1,2)", "DisplayName": "M(1,2)", "CodeFilePath": "/s/C.cs", "LineNumber": 9,
             "Properties": [
               {"Key": {"Id": "TestCase.ManagedType"}, "Value": "N.C"},
               {"Key": {"Id": "TestCase.ManagedMethod"}, "Value": "M(System.Int32,System.Int32)"},
               {"Key": {"Id": "TestObject.Traits"}, "Value": [{"Key": "Category", "Value": "Pairs"}]}]}
            """;
        var item = VsTestRunner.Item(JsonDocument.Parse(testCase).RootElement);
        Assert.Equal("id-1", item.Id);
        Assert.Equal("N.C.M", item.FullyQualifiedName);
        Assert.Equal(("N", "C", "M"), (item.Namespace, item.ClassName, item.Method));
        Assert.Equal(9, item.Line);
        Assert.Equal([new TestTrait("Category", "Pairs")], item.Traits);

        var result = VsTestRunner.Result(JsonDocument.Parse($$"""
            {"TestCase": {{testCase}}, "Outcome": 3, "Duration": "00:00:01.5000000", "ErrorMessage": null,
             "Messages": [{"Category": "StdOutMsgs", "Text": "hello"}, {"Category": "AdditionalInfo", "Text": "later\n"}]}
            """).RootElement)!;
        Assert.Equal(TestOutcomes.Skipped, result.Outcome);
        Assert.Equal(1500, result.DurationMs);
        Assert.Equal("later", result.Message);
        Assert.Equal("hello\nlater\n", result.Output);

        // No managed names: the fully qualified name is split before its arguments.
        var bare = VsTestRunner.Item(JsonDocument.Parse("""{"Id": "x", "FullyQualifiedName": "A.B.C(1)"}""").RootElement);
        Assert.Equal("A.B.C", bare.FullyQualifiedName);
        Assert.Equal(("A", "B", "C"), (bare.Namespace, bare.ClassName, bare.Method));
    }

    [Fact]
    public void Helpers_SplitNamesArgumentsAndSdkLists()
    {
        Assert.Equal(((string?)null, "C", "M"), TestNames.Split("C", "M(int)"));
        Assert.Equal("fallback", TestNames.FullName(null, null, "fallback(1)"));
        Assert.Equal(["exec", "--a", "b c", "d\"e"], VsTestRunner.SplitArguments("exec --a \"b c\" d\\\"e"));
        var sdks = VsTestConsoleLocator.ParseListSdks("9.0.100 [/usr/share/dotnet/sdk]\n10.0.302 [/root/.dotnet/sdk]\n11.0.100-preview.1 [/x/sdk]\n");
        Assert.Equal([new Version(9, 0, 100), new Version(10, 0, 302), new Version(11, 0, 100)], sdks.Select(s => s.Version));
        Assert.Equal("/root/.dotnet/sdk", sdks[1].Directory);
        Assert.True(TestProjectInspector.IsNetFramework("net472"));
        Assert.False(TestProjectInspector.IsNetFramework("net10.0"));
        Assert.False(TestProjectInspector.IsNetFramework("netstandard2.0"));
        Assert.Null(VsTestConsoleLocator.Locate("/does/not/exist/vstest.console.dll"));
    }

    [Fact]
    public void RunSettings_FollowTheParallelSettingOrTheFile()
    {
        var c = new TestContainer("p|net10.0", "/p.csproj", "net10.0", TestRunnerProtocol.VsTest, "/p.dll", TestRuntime.Dotnet);
        Assert.Contains("<MaxCpuCount>1</MaxCpuCount>", VsTestRunner.RunSettings(c), StringComparison.Ordinal);
        Assert.Contains("<MaxCpuCount>0</MaxCpuCount>", VsTestRunner.RunSettings(c with { Parallel = true }), StringComparison.Ordinal);
        var file = Path.GetTempFileName();
        File.WriteAllText(file, "<RunSettings><Mine/></RunSettings>");
        try
        {
            Assert.Equal("<RunSettings><Mine/></RunSettings>", VsTestRunner.RunSettings(c with { RunSettings = file }));
            var (_, args) = MtpRunner.CommandLine(c with { Protocol = TestRunnerProtocol.MicrosoftTestingPlatform, RunSettings = file });
            Assert.Equal(["--settings", file], args.TakeLast(2));
        }
        finally
        {
            File.Delete(file);
        }
    }
}
