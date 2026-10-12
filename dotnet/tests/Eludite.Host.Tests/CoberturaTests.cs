using System.Text.Json;
using Eludite.Host.Rpc;
using Eludite.Host.Testing;

namespace Eludite.Host.Tests;

/// <summary>
/// Brief 0076: <see cref="Cobertura"/> on two checked-in samples of one source file, in the shapes the collectors write:
/// Microsoft's (absolute file names, no sources) and coverlet's (a relative file name under a source). Each alone, then
/// merged (hits summed per file and line, branch counts the larger, counts over distinct lines), and the wire JSON.
/// </summary>
public sealed class CoberturaTests
{
    private static readonly string Source = OperatingSystem.IsWindows() ? @"C:\src\" : "/src/";

    private static readonly string File = Path.Combine(Source, "Calc", "Calculator.cs");

    // Microsoft.Testing.Extensions.CodeCoverage's shape: Add covered except its b == 0 branch, Divide never called.
    private static string Microsoft => $"""
        <?xml version="1.0" encoding="utf-8" standalone="yes"?>
        <coverage line-rate="0.6" branch-rate="0.5" version="1.9" lines-covered="6" lines-valid="10" branches-covered="1" branches-valid="2">
          <packages>
            <package name="Calc" line-rate="0.6">
              <classes>
                <class name="Calc.Arithmetic.Calculator" filename="{File}" line-rate="0.6">
                  <methods>
                    <method name="Add" signature="(int, int)" line-rate="0.7">
                      <lines>
                        <line number="9" hits="1" branch="False" />
                        <line number="10" hits="1" branch="True" condition-coverage="50% (1/2)" />
                        <line number="11" hits="0" branch="False" />
                        <line number="15" hits="1" branch="False" />
                      </lines>
                    </method>
                    <method name="Subtract" signature="(int, int)" line-rate="1">
                      <lines>
                        <line number="21" hits="2" branch="False" />
                        <line number="22" hits="2" branch="False" />
                      </lines>
                    </method>
                    <method name="Divide" signature="(int, int)" line-rate="0">
                      <lines>
                        <line number="26" hits="0" branch="False" />
                        <line number="27" hits="0" branch="False" />
                      </lines>
                    </method>
                  </methods>
                  <lines>
                    <line number="9" hits="1" branch="False" />
                    <line number="10" hits="1" branch="True" condition-coverage="50% (1/2)" />
                    <line number="11" hits="0" branch="False" />
                    <line number="15" hits="1" branch="False" />
                    <line number="21" hits="2" branch="False" />
                    <line number="22" hits="2" branch="False" />
                    <line number="26" hits="0" branch="False" />
                    <line number="27" hits="0" branch="False" />
                  </lines>
                </class>
              </classes>
            </package>
          </packages>
        </coverage>
        """;

    // coverlet's shape: the same file relative to its source; this run took the b == 0 branch too and called Divide.
    private static string Coverlet => $"""
        <?xml version="1.0" encoding="utf-8"?>
        <coverage line-rate="1" branch-rate="1" version="1.9">
          <sources>
            <source>{Source}</source>
          </sources>
          <packages>
            <package name="Calc" line-rate="1">
              <classes>
                <class name="Calc.Arithmetic.Calculator" filename="Calc/Calculator.cs" line-rate="1">
                  <methods>
                    <method name="Add" signature="(System.Int32,System.Int32)" line-rate="1">
                      <lines>
                        <line number="9" hits="3" branch="False" />
                        <line number="10" hits="3" branch="True" condition-coverage="100% (2/2)" />
                        <line number="11" hits="1" branch="False" />
                        <line number="15" hits="2" branch="False" />
                      </lines>
                    </method>
                    <method name="Divide" signature="(System.Int32,System.Int32)" line-rate="1">
                      <lines>
                        <line number="26" hits="1" branch="False" />
                      </lines>
                    </method>
                  </methods>
                  <lines />
                </class>
                <class name="Calc.Arithmetic.Calculator/&lt;&gt;c" filename="Calc/Calculator.cs" line-rate="1">
                  <methods />
                  <lines>
                    <line number="30" hits="4" branch="False" />
                  </lines>
                </class>
              </classes>
            </package>
          </packages>
        </coverage>
        """;

    [Fact]
    public void OneReport_CountsLinesAndBranches_ByClassAndMethod()
    {
        var merged = Cobertura.Merge([Cobertura.Parse(Microsoft)], []);
        Assert.Equal((5, 8, 1, 2), (merged.LinesCovered, merged.LinesValid, merged.BranchesCovered, merged.BranchesValid));
        var module = Assert.Single(merged.Modules);
        Assert.Equal(("Calc", 5, 8), (module.Name, module.LinesCovered, module.LinesValid));
        var cls = Assert.Single(module.Classes);
        Assert.Equal(("Calc.Arithmetic.Calculator", "Calc.Arithmetic", File), (cls.Name, cls.Namespace, cls.File));
        Assert.Equal(["Add", "Subtract", "Divide"], cls.Methods.Select(m => m.Name));
        var add = cls.Methods[0];
        Assert.Equal(("(int, int)", 9, 3, 4, 1, 2), (add.Signature, add.Line, add.LinesCovered, add.LinesValid, add.BranchesCovered, add.BranchesValid));
        Assert.Null(cls.Methods[1].BranchesValid);
        Assert.Equal((0, 2), (cls.Methods[2].LinesCovered, cls.Methods[2].LinesValid));

        var file = Assert.Single(merged.Files);
        Assert.Equal(File, file.Path);
        Assert.Equal(new long[] { 9, 1 }, file.Lines[0]);
        Assert.Equal(new long[] { 10, 1, 1, 2 }, file.Lines[1]);
        Assert.Equal(new long[] { 27, 0 }, file.Lines[^1]);
    }

    [Fact]
    public void TwoReports_MergeByFileAndLine_HitsSummed_BranchesTheLarger()
    {
        var coverlet = Cobertura.Parse(Coverlet);
        var merged = Cobertura.Merge([Cobertura.Parse(Microsoft), coverlet], []);

        // The relative file name resolved against the source: one file.
        var file = Assert.Single(merged.Files);
        Assert.Equal(File, file.Path);
        var lines = file.Lines.ToDictionary(l => l[0]);
        Assert.Equal(new long[] { 9, 4 }, lines[9]);
        Assert.Equal(new long[] { 10, 4, 2, 2 }, lines[10]);
        Assert.Equal(new long[] { 11, 1 }, lines[11]);
        Assert.Equal(new long[] { 26, 1 }, lines[26]);
        Assert.Equal(new long[] { 27, 0 }, lines[27]);
        Assert.Equal(new long[] { 30, 4 }, lines[30]);
        Assert.Equal(9, file.Lines.Count);

        // Totals over the merged lines: everything but line 27.
        Assert.Equal((8, 9, 2, 2), (merged.LinesCovered, merged.LinesValid, merged.BranchesCovered, merged.BranchesValid));
        var module = Assert.Single(merged.Modules);
        Assert.Equal((8, 9), (module.LinesCovered, module.LinesValid));
        Assert.Equal(["Calc.Arithmetic.Calculator", "Calc.Arithmetic.Calculator/<>c"], module.Classes.Select(c => c.Name));
        var cls = module.Classes[0];
        Assert.Equal((7, 8), (cls.LinesCovered, cls.LinesValid));
        Assert.Equal("Calc.Arithmetic", module.Classes[1].Namespace);
        // The signatures differ between the collectors: a method per signature, each counted on the merged lines.
        Assert.Equal(2, cls.Methods.Count(m => m.Name == "Add"));
        Assert.All(cls.Methods.Where(m => m.Name == "Add"), m => Assert.Equal((4, 4, 2, 2), (m.LinesCovered, m.LinesValid, m.BranchesCovered, m.BranchesValid)));
    }

    [Fact]
    public void TheWireShape_IsCamelCase_WithNullsLeftOut()
    {
        var merged = Cobertura.Merge([Cobertura.Parse(Microsoft)], [new CoverageContainerReport("/s/T.csproj|net10.0", "mtp") { Report = "/tmp/r.cobertura.xml" }]);
        var json = JsonSerializer.SerializeToElement(new TestUpdateParams(1, 2, 3, TestUpdateKinds.Coverage) { Coverage = merged }, HostServer.SerializerOptions);
        Assert.Equal("coverage", json.GetProperty("kind").GetString());
        var coverage = json.GetProperty("coverage");
        Assert.Equal(5, coverage.GetProperty("linesCovered").GetInt32());
        Assert.Equal(2, coverage.GetProperty("branchesValid").GetInt32());
        var container = Assert.Single(coverage.GetProperty("containers").EnumerateArray());
        Assert.Equal("mtp", container.GetProperty("collector").GetString());
        Assert.False(container.TryGetProperty("message", out _));
        var method = coverage.GetProperty("modules")[0].GetProperty("classes")[0].GetProperty("methods")[1];
        Assert.Equal("Subtract", method.GetProperty("name").GetString());
        Assert.False(method.TryGetProperty("branchesCovered", out _));
        Assert.Equal(4, coverage.GetProperty("files")[0].GetProperty("lines")[1].GetArrayLength());
    }

    [Fact]
    public void ConditionCoverage_AndNotCobertura()
    {
        Assert.Equal((1, 2), Cobertura.ConditionCoverage("50% (1/2)"));
        Assert.Equal((0, 4), Cobertura.ConditionCoverage("0% (0/4)"));
        Assert.Null(Cobertura.ConditionCoverage("50%"));
        Assert.Null(Cobertura.ConditionCoverage(null));
        Assert.Throws<InvalidDataException>(() => Cobertura.Parse("<CoverageSession />"));
        Assert.Throws<System.Xml.XmlException>(() => Cobertura.Parse("<coverage"));
    }
}
