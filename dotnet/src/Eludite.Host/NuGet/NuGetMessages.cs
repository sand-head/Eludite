using System.Text.Json.Serialization;

namespace Eludite.Host.NuGet;

// Wire shapes of eludite/nuget/* (protocol/schemas/host/nuget-*.json, host-rpc.md "NuGet"). camelCase; null members
// are omitted.

/// <summary>A known vulnerability of a package version. <see cref="Severity"/> is low, moderate, high or critical.</summary>
public sealed record NuGetVulnerability(string Severity, string AdvisoryUrl);

/// <summary>The package a deprecated one points to.</summary>
public sealed record NuGetAlternatePackage(string Id)
{
    public string? Range { get; init; }
}

/// <summary>Why a version is deprecated: <c>legacy</c>, <c>criticalBugs</c>, <c>other</c>.</summary>
public sealed record NuGetDeprecation(IReadOnlyList<string> Reasons)
{
    public string? Message { get; init; }

    public NuGetAlternatePackage? AlternatePackage { get; init; }
}

/// <summary>How one source answered a search, an updates scan or a metadata lookup.</summary>
public sealed record NuGetSourceResult(string Name, string Url, int Count)
{
    public double? ElapsedMs { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool Cached { get; init; }

    public string? Error { get; init; }
}

public sealed record NuGetSearchParams(long? Generation)
{
    public long? Operation { get; init; }

    public string? Query { get; init; }

    public string? Source { get; init; }

    public bool? Prerelease { get; init; }

    public int? Skip { get; init; }

    public int? Take { get; init; }

    public bool? Interactive { get; init; }
}

public sealed record NuGetSearchPackage(string Id, string Version, string Source)
{
    public IReadOnlyList<string>? Versions { get; init; }

    public string? Title { get; init; }

    public string? Description { get; init; }

    public string? Authors { get; init; }

    public string? IconUrl { get; init; }

    public string? LicenseUrl { get; init; }

    public string? LicenseExpression { get; init; }

    public string? ProjectUrl { get; init; }

    public long? Downloads { get; init; }

    public IReadOnlyList<NuGetVulnerability>? Vulnerabilities { get; init; }

    public NuGetDeprecation? Deprecation { get; init; }
}

public sealed record NuGetSearchResult(long Generation, IReadOnlyList<NuGetSearchPackage> Results, IReadOnlyList<NuGetSourceResult> Sources, double ElapsedMs);

public sealed record NuGetInstalledParams(long? Generation)
{
    public long? Operation { get; init; }

    public IReadOnlyList<string>? Projects { get; init; }

    public bool? IncludeTransitive { get; init; }

    public bool? Metadata { get; init; }

    public bool? Interactive { get; init; }
}

public sealed record NuGetPackageDependency(string Id)
{
    public string? Range { get; init; }
}

public sealed record NuGetInstalledPackage(string Id, IReadOnlyList<string> TargetFrameworks, bool Transitive)
{
    public string? Requested { get; init; }

    public string? Version { get; init; }

    public string? Source { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool AutoReferenced { get; init; }

    public IReadOnlyList<NuGetPackageDependency>? Dependencies { get; init; }

    public IReadOnlyList<NuGetVulnerability>? Vulnerabilities { get; init; }

    public NuGetDeprecation? Deprecation { get; init; }
}

/// <summary>A project's packages. <see cref="Format"/> is packageReference, packagesConfig or none.</summary>
public sealed record NuGetInstalledProject(string Path, string Name, string Format, bool Restored, bool CentralPackageManagement, IReadOnlyList<string> TargetFrameworks, IReadOnlyList<NuGetInstalledPackage> Packages)
{
    public string? AssetsFile { get; init; }

    public string? PropsFile { get; init; }

    public string? LockFile { get; init; }

    public string? Error { get; init; }

    public string? Note { get; init; }
}

public sealed record NuGetInstalledResult(long Generation, IReadOnlyList<NuGetInstalledProject> Projects, double ElapsedMs)
{
    public IReadOnlyList<NuGetSourceResult>? Sources { get; init; }
}

public sealed record NuGetUpdatesParams(long? Generation)
{
    public long? Operation { get; init; }

    public IReadOnlyList<string>? Projects { get; init; }

    public bool? Prerelease { get; init; }

    public string? Source { get; init; }

    public bool? Interactive { get; init; }
}

public sealed record NuGetUpdateRow(string Project, string Id, string Installed, string Latest, string Source)
{
    public string? Requested { get; init; }

    public IReadOnlyList<string>? Versions { get; init; }

    public IReadOnlyList<NuGetVulnerability>? Vulnerabilities { get; init; }

    public NuGetDeprecation? Deprecation { get; init; }
}

public sealed record NuGetUpdatesResult(long Generation, IReadOnlyList<NuGetUpdateRow> Updates, IReadOnlyList<NuGetSourceResult> Sources, double ElapsedMs);

public sealed record NuGetPackageArg(string Id)
{
    public string? Version { get; init; }
}

public sealed record NuGetChangeParams(long? Generation, string? Action, IReadOnlyList<NuGetPackageArg>? Packages)
{
    public long? Operation { get; init; }

    public IReadOnlyList<string>? Projects { get; init; }

    public bool? Prerelease { get; init; }

    public string? Source { get; init; }

    public bool? IncludeTransitive { get; init; }

    public bool? Restore { get; init; }

    public string? LockFiles { get; init; }

    public bool? Interactive { get; init; }
}

/// <summary>A file a change wrote. <see cref="Kind"/> is project or centralPackageVersions.</summary>
public sealed record NuGetEditedFile(string Path, string Kind, IReadOnlyList<string> Changes);

/// <summary>A restore error or warning. <see cref="Severity"/> is error, warning or message.</summary>
public sealed record NuGetDiagnostic(string Severity, string Code, string Message)
{
    public string? File { get; init; }

    public int? Line { get; init; }

    public int? Column { get; init; }

    public string? Project { get; init; }
}

/// <summary>How a restore went. <see cref="Result"/> is succeeded, failed or canceled.</summary>
public sealed record NuGetRestoreOutcome(string Result, int? ExitCode, double ElapsedMs, string CommandLine, bool LockedMode, IReadOnlyList<string> LockFiles, IReadOnlyList<NuGetDiagnostic> Diagnostics);

public sealed record NuGetChangeResult(long Generation, string Action, IReadOnlyList<NuGetPackageArg> Packages, IReadOnlyList<string> Projects, IReadOnlyList<NuGetEditedFile> Edited, double ElapsedMs)
{
    public NuGetRestoreOutcome? Restore { get; init; }

    public string? Message { get; init; }
}

public sealed record NuGetSourcesParams(long? Generation)
{
    public long? Operation { get; init; }

    public string? Action { get; init; }

    public string? Name { get; init; }

    public string? Url { get; init; }
}

/// <summary>A package source. <see cref="Scope"/> is machine, user, solution or other.</summary>
public sealed record NuGetSourceInfo(string Name, string Url, bool Enabled, bool Local, string Scope)
{
    public string? ConfigFile { get; init; }
}

public sealed record NuGetSourcesResult(IReadOnlyList<NuGetSourceInfo> Sources, IReadOnlyList<string> ConfigFiles, string UserConfig, bool Changed);

public sealed record NuGetRestoreParams(long? Generation)
{
    public long? Operation { get; init; }

    public IReadOnlyList<string>? Projects { get; init; }

    public string? LockFiles { get; init; }

    public bool? Force { get; init; }

    public bool? Interactive { get; init; }
}

public sealed record NuGetRestoreResult(long Generation, string Result, int? ExitCode, double ElapsedMs, string CommandLine, bool LockedMode, IReadOnlyList<string> LockFiles, IReadOnlyList<NuGetDiagnostic> Diagnostics)
{
    public static NuGetRestoreResult From(long generation, NuGetRestoreOutcome o) =>
        new(generation, o.Result, o.ExitCode, o.ElapsedMs, o.CommandLine, o.LockedMode, o.LockFiles, o.Diagnostics);
}

/// <summary>A package's vulnerability and deprecation data, for the <c>metadata</c> update.</summary>
public sealed record NuGetPackageMetadata(string Id, string Version)
{
    public IReadOnlyList<NuGetVulnerability>? Vulnerabilities { get; init; }

    public NuGetDeprecation? Deprecation { get; init; }
}

/// <summary><c>eludite/nuget/update</c>. <see cref="Kind"/> is output, progress or metadata.</summary>
public sealed record NuGetUpdate(long Operation, long Generation, long Seq, string Kind)
{
    public string? Text { get; init; }

    public string? Message { get; init; }

    public IReadOnlyList<NuGetPackageMetadata>? Packages { get; init; }
}

/// <summary><c>eludite/nuget/credentials</c> params (host to shell).</summary>
public sealed record NuGetCredentialsParams(string Source, string Url, string Host, bool Proxy, bool IsRetry)
{
    public long? Operation { get; init; }

    public string? Message { get; init; }
}

/// <summary>The shell's answer to <c>eludite/nuget/credentials</c>.</summary>
public sealed record NuGetCredentialsAnswer
{
    public string? Username { get; init; }

    public string? Password { get; init; }

    public bool? Remember { get; init; }

    public bool? Canceled { get; init; }
}
