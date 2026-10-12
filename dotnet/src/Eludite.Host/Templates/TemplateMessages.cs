namespace Eludite.Host.Templates;

// The records of eludite/templates/list, eludite/templates/create, eludite/project/addItem and
// eludite/project/addExisting (protocol/schemas/host/templates-list.json, templates-create.json, project-add-item.json,
// project-add-existing.json; brief 0072). Serialized with HostServer.SerializerOptions: camelCase, nulls omitted.

public sealed record TemplatesListParams
{
    /// <summary>Only templates of this <c>tags.type</c> (<c>project</c>, <c>item</c>, <c>solution</c>).</summary>
    public string? Type { get; init; }

    public bool? Refresh { get; init; }
}

public sealed record TemplateChoice(string Value)
{
    public string? DisplayName { get; init; }

    public string? Description { get; init; }
}

/// <summary>A <c>parameter</c> symbol of a template; <see cref="DataType"/> is lower case, <c>text</c> when unsaid.</summary>
public sealed record TemplateParameter(string Name, string DataType)
{
    public string? DisplayName { get; init; }

    public string? Description { get; init; }

    public string? DefaultValue { get; init; }

    public IReadOnlyList<TemplateChoice>? Choices { get; init; }

    public bool? Required { get; init; }
}

/// <summary>One template; <see cref="Source"/> is <c>sdk</c>, <c>user</c> or <c>eludite</c>.</summary>
public sealed record TemplateInfo(string Identity, string Name, IReadOnlyList<string> ShortNames, string Type, string Source, IReadOnlyList<TemplateParameter> Parameters)
{
    public string? GroupIdentity { get; init; }

    public string? Language { get; init; }

    public IReadOnlyList<string>? Classifications { get; init; }

    public string? Author { get; init; }

    public string? Description { get; init; }

    public string? DefaultName { get; init; }
}

public sealed record TemplatesListResult(IReadOnlyList<TemplateInfo> Templates)
{
    public string? TemplatesFolder { get; init; }

    public IReadOnlyList<string>? Skipped { get; init; }
}

public sealed record TemplatesCreateParams(string? ShortName, string? Name, string? Output)
{
    public string? Language { get; init; }

    public IReadOnlyDictionary<string, string>? Parameters { get; init; }

    public string? Solution { get; init; }

    public string? NewSolution { get; init; }
}

public sealed record TemplatesCreateResult(int ExitCode, IReadOnlyList<string> Files, string Output, IReadOnlyList<string> CommandLines)
{
    public string? Project { get; init; }

    public string? PrimaryFile { get; init; }

    public string? Solution { get; init; }
}

public sealed record ProjectAddItemParams(string? Project, string? Folder, string? Template, string? Name)
{
    public IReadOnlyDictionary<string, string>? Parameters { get; init; }
}

public sealed record ProjectAddItemResult(IReadOnlyList<string> Files, string Namespace, bool ProjectWritten, long Generation)
{
    public string? Output { get; init; }
}

public sealed record ProjectAddExistingParams(string? Project, string? Folder, IReadOnlyList<string>? Files)
{
    public bool? Link { get; init; }
}

public sealed record ProjectAddExistingResult(IReadOnlyList<string> Files, bool ProjectWritten, long Generation);
