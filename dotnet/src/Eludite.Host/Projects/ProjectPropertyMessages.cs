using System.Text.Json.Serialization;

namespace Eludite.Host.Projects;

// Wire shapes of eludite/project/* and eludite/solution/configurations, setConfiguration (brief 0049;
// protocol/schemas/host/project-properties.json, project-set-property.json, launch-profiles.json,
// launch-profile-set.json, solution-configurations.json, solution-set-configuration.json). camelCase, nulls omitted.

public sealed record ProjectPropertiesParams(string? Project)
{
    public string? Configuration { get; init; }

    public string? Platform { get; init; }

    public string? Framework { get; init; }
}

/// <summary>A property's defining element.</summary>
public sealed record DefinedIn(string File, int Line)
{
    public string? Condition { get; init; }
}

public sealed record PropertyValue(
    string Name,
    string Page,
    string Label,
    string Type,
    bool PerConfiguration,
    string Value,
    string Source,
    bool Conditioned,
    bool Inherited)
{
    public string? Section { get; init; }

    public string? Description { get; init; }

    public IReadOnlyList<CatalogValue>? Values { get; init; }

    public string? TrueValue { get; init; }

    public string? FalseValue { get; init; }

    public string? Target { get; init; }

    public string? Raw { get; init; }

    public DefinedIn? DefinedIn { get; init; }

    public string? InheritedFrom { get; init; }

    public IReadOnlyList<string>? Conditions { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool ReadOnly { get; init; }

    public string? ReadOnlyReason { get; init; }
}

public sealed record PageInfo(string Id, string Title, string State)
{
    public string? Note { get; init; }
}

public sealed record ProjectPropertiesResult(
    long Generation,
    string Project,
    string Kind,
    string Configuration,
    string Platform,
    IReadOnlyList<string> Configurations,
    IReadOnlyList<string> Platforms,
    IReadOnlyList<string> Frameworks,
    IReadOnlyList<PageInfo> Pages,
    IReadOnlyList<PropertyValue> Properties)
{
    public string? Framework { get; init; }
}

public sealed record PropertyEdit(string Name, string? Value)
{
    public string? Configuration { get; init; }

    public string? Platform { get; init; }

    public string? Framework { get; init; }

    public bool AllConfigurations { get; init; }

    public bool Override { get; init; }
}

public sealed record ProjectSetPropertyParams(string? Project, long? Generation, IReadOnlyList<PropertyEdit>? Edits);

/// <summary><see cref="Status"/> is <c>written</c>, <c>removed</c>, <c>unchanged</c> or <c>inherited</c>.</summary>
public sealed record PropertyEditResult(string Name, string Status)
{
    public string? Condition { get; init; }

    public int? Line { get; init; }

    public string? InheritedFrom { get; init; }

    public IReadOnlyList<string>? RemovedConditions { get; init; }
}

public sealed record ProjectSetPropertyResult(long Generation, string Project, bool Written, IReadOnlyList<PropertyEditResult> Results);

public sealed record EnvironmentVariable(string Name, string Value);

public sealed record LaunchProfileInfo(string Name, string CommandName, IReadOnlyList<EnvironmentVariable> EnvironmentVariables)
{
    public string? CommandLineArgs { get; init; }

    public string? WorkingDirectory { get; init; }

    public bool? LaunchBrowser { get; init; }

    public string? LaunchUrl { get; init; }

    public string? ApplicationUrl { get; init; }

    public bool? DotnetRunMessages { get; init; }

    public bool? HotReloadEnabled { get; init; }

    public string? ExecutablePath { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool ReadOnly { get; init; }

    public IReadOnlyList<string>? Unknown { get; init; }
}

public sealed record LaunchProfilesParams(string? Project);

public sealed record LaunchProfilesResult(long Generation, string Project, string File, bool Exists, IReadOnlyList<LaunchProfileInfo> Profiles);

/// <summary>
/// The members <c>eludite/project/setLaunchProfile</c> sets. A member present with null removes it; absent leaves it.
/// Kept as raw JSON so "absent" and "null" stay apart.
/// </summary>
public sealed record SetLaunchProfileParams(string? Project, long? Generation, string? Action, string? Profile)
{
    public string? NewName { get; init; }

    public System.Text.Json.JsonElement? Values { get; init; }
}

public sealed record Selection(string Configuration, string Platform);

public sealed record ConfigurationMapping(string SolutionConfiguration, string SolutionPlatform, string Configuration, string Platform, bool Build)
{
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool Deploy { get; init; }
}

public sealed record SolutionProjectConfigurations(
    string Name,
    string Path,
    IReadOnlyList<string> Configurations,
    IReadOnlyList<string> Platforms,
    IReadOnlyList<ConfigurationMapping> Mappings);

public sealed record SolutionConfigurationsResult(
    long Generation,
    string? Path,
    IReadOnlyList<string> Configurations,
    IReadOnlyList<string> Platforms,
    Selection Active,
    IReadOnlyList<SolutionProjectConfigurations> Projects)
{
    public string? Format { get; init; }
}

public sealed record MappingEdit(string? Project, string? SolutionConfiguration, string? SolutionPlatform)
{
    public string? Configuration { get; init; }

    public string? Platform { get; init; }

    public bool? Build { get; init; }
}

public sealed record SolutionSetConfigurationParams(long? Generation)
{
    public Selection? Select { get; init; }

    public IReadOnlyList<MappingEdit>? Mappings { get; init; }
}

public sealed record SolutionSetConfigurationResult(long Generation, string? Path, bool Written, Selection Active);
