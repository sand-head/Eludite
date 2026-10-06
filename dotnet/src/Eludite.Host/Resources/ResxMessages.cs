using System.Text.Json.Serialization;

namespace Eludite.Host.Resources;

// The records of eludite/resx/sets and eludite/resx/designer (protocol/schemas/host/resx-sets.json,
// resx-designer.json; proposal 0005). Serialized with HostServer.SerializerOptions: camelCase, nulls omitted.

public sealed record ResxSetsParams(long? Generation, IReadOnlyList<string>? Projects);

/// <summary>A culture file of a set; <see cref="Item"/> is false when it sits beside the neutral file but no item lists it.</summary>
public sealed record ResxCulture(string Name, string Path, bool Item);

/// <summary>A neutral <c>.resx</c> a project lists, with what the editor and the designer generator need.</summary>
public sealed record ResxSet(
    string Project,
    string ProjectName,
    string Kind,
    string Path,
    string BaseName,
    string RootNamespace,
    IReadOnlyList<ResxCulture> Cultures,
    string AccessModifier,
    string ManifestName,
    string Namespace)
{
    public string? NeutralLanguage { get; init; }

    public string? Generator { get; init; }

    public string? CustomToolNamespace { get; init; }

    public string? LastGenOutput { get; init; }

    public string? Designer { get; init; }
}

public sealed record ResxSkippedProject(string Project, string Reason);

public sealed record ResxSetsResult(long Generation, IReadOnlyList<ResxSet> Sets)
{
    public IReadOnlyList<ResxSkippedProject>? Skipped { get; init; }
}

/// <summary><see cref="Action"/> is <c>generate</c>, <c>setModifier</c> or <c>delete</c>; <see cref="Modifier"/> <c>internal</c>, <c>public</c> or <c>none</c>.</summary>
public sealed record ResxDesignerParams(long? Generation, string? Path, string? Action)
{
    public string? Modifier { get; init; }
}

/// <summary><see cref="Status"/> is <c>written</c>, <c>unchanged</c>, <c>deleted</c> or <c>none</c>.</summary>
public sealed record ResxDesignerResult(long Generation, string Path, string Status, string Modifier)
{
    public string? Designer { get; init; }

    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)]
    public bool ProjectWritten { get; init; }

    public string? ClassName { get; init; }

    public string? Namespace { get; init; }
}
