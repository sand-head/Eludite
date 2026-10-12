using System.Text;

namespace Eludite.Host.Templates;

/// <summary>
/// Eludite's own item templates (brief 0072): C#'s Class, Interface, Record, Enum and Struct and Visual Basic's Class,
/// Interface, Module, Enum and Structure, identities <c>eludite.cs.*</c> and <c>eludite.vb.*</c>. They write the file
/// themselves with the namespace of <see cref="ProjectNamespace"/>: C# a file-scoped <c>namespace X;</c>; Visual Basic
/// a <c>Namespace</c> block for the folders below the project only (the project's <c>RootNamespace</c> applies
/// implicitly), none in the project's folder. Files are UTF-8 with a byte order mark and <c>\n</c> line endings.
/// </summary>
public static class BuiltInItems
{
    /// <summary>The <c>source</c> of the built-in templates.</summary>
    public const string Source = "eludite";

    private static readonly (string Identity, string Name, string ShortName, string Language, string Keyword)[] Kinds =
    [
        ("eludite.cs.class", "Class", "class", "C#", "class"),
        ("eludite.cs.interface", "Interface", "interface", "C#", "interface"),
        ("eludite.cs.record", "Record", "record", "C#", "record"),
        ("eludite.cs.enum", "Enum", "enum", "C#", "enum"),
        ("eludite.cs.struct", "Struct", "struct", "C#", "struct"),
        ("eludite.vb.class", "Class", "class", "VB", "Class"),
        ("eludite.vb.interface", "Interface", "interface", "VB", "Interface"),
        ("eludite.vb.module", "Module", "module", "VB", "Module"),
        ("eludite.vb.enum", "Enum", "enum", "VB", "Enum"),
        ("eludite.vb.structure", "Structure", "structure", "VB", "Structure"),
    ];

    /// <summary>The built-in templates as <c>eludite/templates/list</c> lists them.</summary>
    public static IReadOnlyList<TemplateInfo> Templates { get; } = Kinds
        .Select(k => new TemplateInfo(k.Identity, k.Name, [k.ShortName], "item", Source, [])
        {
            GroupIdentity = "eludite." + k.ShortName,
            Language = k.Language,
            Classifications = ["Common", "Code"],
            Author = "Eludite",
            Description = k.Language == "C#" ? $"An empty C# {k.Keyword} in the folder's namespace." : $"An empty Visual Basic {k.Keyword}.",
            DefaultName = k.Name + "1",
        })
        .ToList();

    /// <summary>The built-in template with this identity, or null.</summary>
    public static TemplateInfo? Find(string identity) =>
        Templates.FirstOrDefault(t => string.Equals(t.Identity, identity, StringComparison.OrdinalIgnoreCase));

    /// <summary>The built-in template of <paramref name="language"/> with this short name, or null.</summary>
    public static TemplateInfo? FindShortName(string shortName, string? language) =>
        Templates.FirstOrDefault(t => t.Language == language && t.ShortNames.Contains(shortName, StringComparer.OrdinalIgnoreCase));

    /// <summary>The file extension of a language (<c>.cs</c>, <c>.vb</c>, <c>.fs</c>), or null.</summary>
    public static string? Extension(string? language) => language switch
    {
        "C#" => ".cs",
        "VB" => ".vb",
        "F#" => ".fs",
        _ => null,
    };

    /// <summary>The language of a project file by its extension (<c>C#</c>, <c>VB</c>, <c>F#</c>), or null.</summary>
    public static string? LanguageOf(string projectPath)
    {
        ArgumentNullException.ThrowIfNull(projectPath);
        return Path.GetExtension(projectPath).ToUpperInvariant() switch
        {
            ".CSPROJ" => "C#",
            ".VBPROJ" => "VB",
            ".FSPROJ" => "F#",
            _ => null,
        };
    }

    /// <summary>
    /// The text of <paramref name="template"/> for type <paramref name="typeName"/>: <paramref name="ns"/> is the C#
    /// namespace (root plus folders) or the Visual Basic one (folders only, empty for none).
    /// </summary>
    public static string Render(TemplateInfo template, string typeName, string ns)
    {
        ArgumentNullException.ThrowIfNull(template);
        ArgumentNullException.ThrowIfNull(typeName);
        ArgumentNullException.ThrowIfNull(ns);
        var kind = Kinds.First(k => k.Identity == template.Identity);
        var text = new StringBuilder();
        if (kind.Language == "C#")
        {
            if (ns.Length > 0)
            {
                text.Append("namespace ").Append(ns).Append(";\n\n");
            }

            text.Append("public ").Append(kind.Keyword).Append(' ').Append(typeName).Append("\n{\n}\n");
            return text.ToString();
        }

        var indent = ns.Length > 0 ? "    " : string.Empty;
        if (ns.Length > 0)
        {
            text.Append("Namespace ").Append(ns).Append('\n');
        }

        text.Append(indent).Append("Public ").Append(kind.Keyword).Append(' ').Append(typeName).Append("\n\n");
        text.Append(indent).Append("End ").Append(kind.Keyword).Append('\n');
        if (ns.Length > 0)
        {
            text.Append("End Namespace\n");
        }

        return text.ToString();
    }

    /// <summary>The bytes written: UTF-8 with a byte order mark, as Visual Studio writes code files.</summary>
    public static byte[] Bytes(string text) => [.. Encoding.UTF8.GetPreamble(), .. Encoding.UTF8.GetBytes(text)];
}
