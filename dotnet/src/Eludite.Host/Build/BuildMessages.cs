using System.Text.Json.Serialization;

namespace Eludite.Host.Build;

// Wire shapes of eludite/build/* (protocol/schemas/host/build-*.json, host-rpc.md "Build"). camelCase; null members
// are omitted.

/// <summary><c>build</c>, <c>rebuild</c> or <c>clean</c>.</summary>
public static class BuildTargets
{
    public const string Build = "build";
    public const string Rebuild = "rebuild";
    public const string Clean = "clean";

    public static bool IsValid(string? target) => target is Build or Rebuild or Clean;
}

/// <summary><c>succeeded</c>, <c>failed</c> or <c>canceled</c>.</summary>
public static class BuildResults
{
    public const string Succeeded = "succeeded";
    public const string Failed = "failed";
    public const string Canceled = "canceled";
}

/// <summary><c>eludite/build/start</c> params.</summary>
public sealed record BuildStartParams(string? Target)
{
    public string? Project { get; init; }

    public string? Configuration { get; init; }

    public string? Platform { get; init; }
}

/// <summary>Which MSBuild runs the build. <see cref="Kind"/> is <c>dotnet</c>, <c>mono</c> or <c>buildTools</c>.</summary>
public sealed record BuildToolchain(string Kind)
{
    public string? Path { get; init; }

    public string? Source { get; init; }
}

/// <summary><c>eludite/build/start</c> result.</summary>
public sealed record BuildStartResult(long BuildId, long Generation, string Path, string Target, string Configuration, BuildToolchain Toolchain, string CommandLine)
{
    public string? Platform { get; init; }

    public string? Binlog { get; init; }
}

/// <summary><c>eludite/build/cancel</c> params.</summary>
public sealed record BuildCancelParams(long? BuildId);

/// <summary><c>eludite/build/cancel</c> result.</summary>
public sealed record BuildCancelResult(bool Canceled)
{
    public long? BuildId { get; init; }
}

/// <summary><c>eludite/build/output</c> params.</summary>
public sealed record BuildOutputParams(long BuildId, long Seq, string Text);

/// <summary><c>eludite/build/progress</c> params.</summary>
public sealed record BuildProgressParams(long BuildId, double ElapsedMs, int ProjectsTotal, int ProjectsCompleted, int Errors, int Warnings)
{
    public string? CurrentProject { get; init; }
}

/// <summary>A build diagnostic. <see cref="Severity"/> is <c>error</c>, <c>warning</c> or <c>message</c>; line and column are 1-based.</summary>
public sealed record BuildDiagnostic(string Severity, string Code, string Message)
{
    public string? File { get; init; }

    public int? Line { get; init; }

    public int? Column { get; init; }

    public int? EndLine { get; init; }

    public int? EndColumn { get; init; }

    /// <summary>Absolute path of the project file that reported it.</summary>
    public string? Project { get; init; }
}

public sealed record BuildSummary(int ProjectsSucceeded, int ProjectsFailed, int Errors, int Warnings);

/// <summary>One project of <see cref="BuildFinishedParams"/>.</summary>
public sealed record BuildProjectResult(string Name, string Path, string Result, int Errors, int Warnings)
{
    public double? ElapsedMs { get; init; }
}

/// <summary><c>eludite/build/finished</c> params.</summary>
public sealed record BuildFinishedParams(
    long BuildId,
    long Generation,
    string Target,
    string Path,
    string Result,
    double ElapsedMs,
    BuildSummary Summary,
    IReadOnlyList<BuildProjectResult> Projects,
    IReadOnlyList<BuildDiagnostic> Diagnostics)
{
    public int? ExitCode { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool DiagnosticsTruncated { get; init; }

    public string? Binlog { get; init; }

    public string? Message { get; init; }
}
