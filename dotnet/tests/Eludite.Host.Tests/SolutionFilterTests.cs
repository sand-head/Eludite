using Eludite.Host.Legacy;

namespace Eludite.Host.Tests;

/// <summary>Brief 0072: a solution filter (<c>.slnf</c>) stands for the projects it names that its solution lists, in the filter's order.</summary>
public sealed class SolutionFilterTests : IDisposable
{
    private const string Sln = """
        Microsoft Visual Studio Solution File, Format Version 12.00
        # Visual Studio Version 17
        Project("{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}") = "A", "src\A\A.csproj", "{11111111-1111-1111-1111-111111111111}"
        EndProject
        Project("{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}") = "B", "src\B\B.csproj", "{22222222-2222-2222-2222-222222222222}"
        EndProject
        Project("{F184B08F-C81C-45F6-A57F-5ABD9991F28F}") = "C", "src\C\C.vbproj", "{33333333-3333-3333-3333-333333333333}"
        EndProject
        Global
        EndGlobal

        """;

    private readonly string _root = Directory.CreateTempSubdirectory("eludite-slnf-").FullName;

    public SolutionFilterTests()
    {
        foreach (var project in new[] { "src/A/A.csproj", "src/B/B.csproj", "src/C/C.vbproj", "src/D/D.csproj" })
        {
            var path = Path.Combine(_root, project);
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(path, "<Project Sdk=\"Microsoft.NET.Sdk\" />\n");
        }

        File.WriteAllText(Path.Combine(_root, "All.sln"), Sln);
    }

    public void Dispose() => TestDirectory.Delete(_root);

    private string P(string relative) => Path.Combine(_root, relative.Replace('/', Path.DirectorySeparatorChar));

    [Fact]
    public void Read_ReturnsTheFiltersProjectsTheSolutionListsInFilterOrder()
    {
        Directory.CreateDirectory(P("filters"));
        var filter = P("filters/Some.slnf");
        File.WriteAllText(filter, """
            {
              "solution": {
                "path": "..\\All.sln",
                "projects": [
                  "src\\C\\C.vbproj",
                  "src\\Missing\\Missing.csproj",
                  "src\\D\\D.csproj",
                  "src\\A\\A.csproj"
                ]
              }
            }
            """);

        Assert.True(SolutionProjects.IsSolutionFilter(filter));
        Assert.Equal(P("All.sln"), SolutionProjects.FilterSolution(filter));
        // Missing is not on disk and D is not in the solution: both skipped; C before A as the filter has them.
        Assert.Equal([P("src/C/C.vbproj"), P("src/A/A.csproj")], SolutionProjects.Read(filter));
        Assert.Equal(SolutionProjects.Read(filter), SolutionProjects.ReadFilter(filter));
    }

    [Fact]
    public void Read_OfABrokenFilterIsEmpty()
    {
        var noSolution = P("None.slnf");
        File.WriteAllText(noSolution, """{ "solution": { "path": "Gone.sln", "projects": ["src\\A\\A.csproj"] } }""");
        Assert.Empty(SolutionProjects.Read(noSolution));

        var notJson = P("Bad.slnf");
        File.WriteAllText(notJson, "not json");
        Assert.Empty(SolutionProjects.Read(notJson));
        Assert.Null(SolutionProjects.FilterSolution(notJson));
    }
}
