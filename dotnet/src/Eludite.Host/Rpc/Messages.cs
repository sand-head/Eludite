using System.Text.Json.Serialization;

namespace Eludite.Host.Rpc;

// Wire shapes for the eludite-host JSON-RPC contract shared with the Rust shell (protocol/schemas/host-rpc.md and
// protocol/schemas/host/*.json). Property names are serialized camelCase; null members are omitted.

public sealed record InitializeParams(string ClientName, string ClientVersion);

public sealed record InitializeResult(string HostName, string HostVersion, HostCapabilities Capabilities);

/// <summary>Capabilities advertised by the host.</summary>
/// <param name="LanguageServer">True when a language server is configured.</param>
public sealed record HostCapabilities(bool LanguageServer);

public sealed record PingResult(bool Pong, string Timestamp);

public sealed record HostInfoResult(IReadOnlyList<SdkInfo> DotnetSdks, string Runtime, string Os);

public sealed record SdkInfo(string Version, string Path);

public sealed record SolutionOpenParams(string? Path);

/// <summary>Result of <c>eludite/solution/open</c> and <c>eludite/solution/close</c>.</summary>
public sealed record GenerationResult(long Generation);

/// <summary><c>eludite/solution/status</c> state names.</summary>
public static class SolutionStates
{
    public const string Loading = "loading";
    public const string Loaded = "loaded";
    public const string Failed = "failed";
    public const string Closed = "closed";
}

/// <summary><c>eludite/solution/status</c> params.</summary>
public sealed record SolutionStatus(long Generation, string Path, string State)
{
    /// <summary><c>legacyEvaluation</c> or <c>projectLoad</c>; only with state loading.</summary>
    public string? Phase { get; init; }

    public ProjectCounts? Counts { get; init; }

    public MsBuildInfo? Msbuild { get; init; }

    public IReadOnlyList<Correction>? Corrections { get; init; }

    public IReadOnlyList<HostDiagnostic>? Diagnostics { get; init; }

    public double? ElapsedMs { get; init; }
}

public sealed record ProjectCounts(int Projects, int LegacyProjects, int LegacyEvaluationFailures);

/// <summary>The MSBuild used for legacy projects. <see cref="Kind"/> is <c>mono</c>, <c>buildTools</c> or <c>sdk</c>.</summary>
public sealed record MsBuildInfo(string Kind, string? Path, string? Source);

/// <summary><see cref="Kind"/> is <c>designerPartials</c>, <c>caseFixups</c> or <c>comReferencesRemoved</c>.</summary>
public sealed record Correction(string Kind, string Project, int Count);

/// <summary>A project-load diagnostic. <see cref="Severity"/> is <c>error</c>, <c>warning</c> or <c>info</c>.</summary>
public sealed record HostDiagnostic(string Severity, string Code, string Message)
{
    public string? Project { get; init; }

    public string? Class { get; init; }
}

/// <summary><c>eludite/languageServer/status</c> params.</summary>
public sealed record LanguageServerStatus(string State)
{
    [JsonPropertyName("serverInfo")]
    public object? ServerInfo { get; init; }

    public object? Capabilities { get; init; }

    public string? Message { get; init; }
}
