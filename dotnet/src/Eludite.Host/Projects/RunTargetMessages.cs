namespace Eludite.Host.Projects;

// Wire shapes of eludite/project/runTarget (brief 0074; protocol/schemas/host/run-target.json). camelCase, nulls omitted.

public sealed record RunTargetParams(string? Project)
{
    public string? Configuration { get; init; }

    public string? Platform { get; init; }

    public string? Framework { get; init; }
}

public sealed record RunTargetResult(
    long Generation,
    string Project,
    string Configuration,
    string Platform,
    IReadOnlyList<string> TargetFrameworks,
    string OutputType,
    string TargetPath,
    string RunCommand,
    string RunArguments,
    string RunWorkingDirectory,
    bool IsWebProject,
    bool Exists)
{
    /// <summary>The target framework evaluated; null for a legacy project without <c>TargetFramework</c>.</summary>
    public string? Framework { get; init; }

    public string? TargetFrameworkIdentifier { get; init; }
}
