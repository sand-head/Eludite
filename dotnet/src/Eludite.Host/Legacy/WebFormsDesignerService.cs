using System.Security;
using System.Security.Cryptography;
using System.Text;
using Niello.Web;

namespace Niello.Host.Legacy;

/// <summary>A designer partial generated for one markup file.</summary>
/// <param name="Markup">The <c>.aspx</c>/<c>.ascx</c>/<c>.master</c> file.</param>
/// <param name="GeneratedFile">The generated C# file.</param>
/// <param name="ReplacedDesignerFile">The project's own <c>.designer.cs</c> Compile item it replaces (<see cref="DesignerMode.Replace"/> only).</param>
/// <param name="Fields">Field names written to <paramref name="GeneratedFile"/>.</param>
/// <param name="Unresolved">Controls whose type could not be resolved (<c>prefix:Tag#Id</c>).</param>
public sealed record GeneratedDesigner(string Markup, string GeneratedFile, string? ReplacedDesignerFile, IReadOnlyList<string> Fields, IReadOnlyList<string> Unresolved)
{
    /// <summary>The project's own <c>.designer.cs</c> Compile item for the markup, if any (kept or replaced).</summary>
    public string? ExistingDesignerFile { get; init; }

    /// <summary>Every field the markup declares, with its resolved type (whether or not it was written).</summary>
    public IReadOnlyList<DesignerField> MarkupFields { get; init; } = [];
}

/// <summary>What to do with a checked-in <c>.designer.cs</c>.</summary>
public enum DesignerMode
{
    /// <summary>
    /// Keep it and generate only the fields it lacks (default). Checked-in designers are what the user's real build
    /// compiles, and they are sometimes stale (fields for controls no longer in the markup, brief 0003 corpus), so
    /// replacing them would add errors the real build does not have.
    /// </summary>
    Supplement,

    /// <summary>Remove it from the Compile items and generate every field from the markup.</summary>
    Replace,
}

/// <summary>
/// Generates designer partial classes from WebForms markup (Niello.Web) and feeds them to MSBuild-based loaders
/// (the Roslyn language server's build host, or the evaluators) through an injected targets file that adds the
/// generated file beside each project's <c>.designer.cs</c> (<see cref="DesignerMode.Supplement"/>) or swaps it
/// (<see cref="DesignerMode.Replace"/>). Brief 0003 spike; nothing is written into the project.
/// </summary>
public sealed partial class WebFormsDesignerService
{
    private readonly string _outputDirectory;
    private readonly DesignerMode _mode;

    public WebFormsDesignerService(string outputDirectory, DesignerMode mode = DesignerMode.Supplement)
    {
        _outputDirectory = outputDirectory;
        _mode = mode;
    }

    /// <summary>Generates a partial for every markup item of <paramref name="evaluation"/> that has an <c>Inherits</c>.</summary>
    public IReadOnlyList<GeneratedDesigner> Generate(LegacyProjectEvaluation evaluation)
    {
        ArgumentNullException.ThrowIfNull(evaluation);
        var projectDir = Path.GetDirectoryName(evaluation.ProjectPath)!;
        var catalog = new MetadataTypeCatalog(evaluation.ReferencePaths);
        var webConfig = Directory.EnumerateFiles(projectDir, "*.config")
            .FirstOrDefault(f => Path.GetFileName(f).Equals("web.config", StringComparison.OrdinalIgnoreCase));
        var configRegistrations = webConfig is null ? [] : WebConfigControls.Read(File.ReadAllText(webConfig));
        var compile = evaluation.CompileItems.ToHashSet(OperatingSystem.IsWindows() ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal);
        var outDir = Path.Combine(_outputDirectory, ProjectKey(evaluation.ProjectPath));

        var result = new List<GeneratedDesigner>();
        foreach (var markupPath in evaluation.MarkupItems.Where(File.Exists))
        {
            var markup = File.ReadAllText(markupPath);
            var markupDir = Path.GetDirectoryName(markupPath)!;
            var resolver = new ControlTypeResolver(
                catalog,
                [.. RegisterDirectiveParser.Parse(markup), .. configRegistrations],
                src => UserControlType(src, projectDir, markupDir));
            if (DesignerPartialGenerator.Build(markup, resolver) is not { } partial)
            {
                continue;
            }

            var designerPath = markupPath + ".designer.cs";
            // Projects written on Windows may spell the designer item with different letter case than the disk.
            var existingItem = compile.Contains(designerPath)
                ? designerPath
                : evaluation.CompileItems.FirstOrDefault(c => string.Equals(c, designerPath, StringComparison.OrdinalIgnoreCase));
            var existing = existingItem is null ? null : File.Exists(existingItem) ? existingItem : PathCaseFixups.Resolve(existingItem);
            // Like Visual Studio's designer generator: no field for a control the code-behind declares itself, and no
            // partial at all for an old-style (ASP.NET 1.x) code-behind class that is not partial.
            var codeBehind = CodeBehindFile(markup, markupPath);
            var codeBehindText = codeBehind is null ? null : File.ReadAllText(codeBehind);
            if (codeBehindText is not null && !DeclaresPartial(codeBehindText, partial.ClassName))
            {
                continue;
            }

            var declared = new HashSet<string>(codeBehindText is null ? [] : DeclaredFieldNames(codeBehindText), StringComparer.Ordinal);
            if (existing is not null && _mode == DesignerMode.Supplement)
            {
                declared.UnionWith(DeclaredFieldNames(File.ReadAllText(existing)));
            }

            var written = partial with { Fields = [.. partial.Fields.Where(f => !declared.Contains(f.Name))] };

            var relative = Path.GetRelativePath(projectDir, markupPath);
            var generated = Path.Combine(outDir, relative + ".g.cs");
            Directory.CreateDirectory(Path.GetDirectoryName(generated)!);
            File.WriteAllText(generated, DesignerPartialGenerator.Render(written, relative.Replace('\\', '/')));
            result.Add(new GeneratedDesigner(
                markupPath,
                generated,
                _mode == DesignerMode.Replace ? existing : null,
                [.. written.Fields.Select(f => f.Name)],
                [.. partial.Unresolved.Select(u => $"{u.TagPrefix}:{u.TagName}#{u.Id}")])
            {
                ExistingDesignerFile = existing,
                MarkupFields = partial.Fields,
            });
        }

        return result;
    }

    /// <summary>
    /// Writes an MSBuild file that, per project, adds the generated Compile items (removing replaced <c>.designer.cs</c>
    /// items in <see cref="DesignerMode.Replace"/>), and corrects Compile items whose letter case differs from the file on disk
    /// (<see cref="PathCaseFixups"/>). Off Windows it also drops <c>COMReference</c> items: COM type libraries
    /// cannot be resolved without the Windows registry and tlbimp/AxImp, and Roslyn's build host drops the whole
    /// project when <c>ResolveComReference</c> fails. Imported through <c>CustomAfterMicrosoftCommonTargets</c>.
    /// </summary>
    public static void WriteInjectionTargets(
        string path,
        IEnumerable<(string ProjectPath, IReadOnlyList<GeneratedDesigner> Designers)> projects,
        IReadOnlyDictionary<string, IReadOnlyList<PathCaseFixup>>? caseFixups = null)
    {
        ArgumentNullException.ThrowIfNull(projects);
        var sb = new StringBuilder("<Project>\n  <!-- Generated by niello-host (brief 0003): design-time corrections for legacy projects. -->\n");
        sb.Append("  <ItemGroup Condition=\"'$(OS)' != 'Windows_NT'\">\n    <COMReference Remove=\"@(COMReference)\" />\n  </ItemGroup>\n");
        var byProject = new Dictionary<string, (IReadOnlyList<GeneratedDesigner> Designers, IReadOnlyList<PathCaseFixup> Fixups)>(StringComparer.Ordinal);
        foreach (var (project, designers) in projects)
        {
            byProject[project] = (designers, []);
        }

        foreach (var (project, fixups) in caseFixups ?? new Dictionary<string, IReadOnlyList<PathCaseFixup>>())
        {
            byProject[project] = (byProject.TryGetValue(project, out var existing) ? existing.Designers : [], fixups);
        }

        foreach (var (project, (designers, fixups)) in byProject)
        {
            if (!designers.Any(d => d.Fields.Count > 0 || d.ReplacedDesignerFile is not null) && fixups.Count == 0)
            {
                continue;
            }

            sb.Append("  <ItemGroup Condition=\"'$(MSBuildProjectFullPath)' == '").Append(Escape(project)).Append("'\">\n");
            foreach (var f in fixups)
            {
                sb.Append("    <Compile Remove=\"").Append(Escape(f.Declared)).Append("\" />\n");
                sb.Append("    <Compile Include=\"").Append(Escape(f.OnDisk)).Append("\" />\n");
            }

            // Nothing to add next to a complete checked-in designer: inject nothing (an empty partial whose
            // Inherits differs in case from the code-behind class would declare a second type).
            foreach (var d in designers.Where(d => d.Fields.Count > 0 || d.ReplacedDesignerFile is not null))
            {
                if (d.ReplacedDesignerFile is not null)
                {
                    sb.Append("    <Compile Remove=\"").Append(Escape(d.ReplacedDesignerFile)).Append("\" />\n");
                }

                sb.Append("    <Compile Include=\"").Append(Escape(d.GeneratedFile)).Append("\" />\n");
            }

            sb.Append("  </ItemGroup>\n");
        }

        sb.Append("</Project>\n");
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, sb.ToString());
    }

    [System.Text.RegularExpressions.GeneratedRegex(@"\b(?:protected|public|internal|private)\s+(?:global::)?[\w.]+(?:<[^;]*?>)?\s+(?<name>\w+)\s*;")]
    private static partial System.Text.RegularExpressions.Regex FieldDeclaration();

    /// <summary>Field names declared in a checked-in designer file (a regex over the generated shape, enough for the spike).</summary>
    public static IReadOnlySet<string> DeclaredFieldNames(string designerSource)
    {
        ArgumentNullException.ThrowIfNull(designerSource);
        return FieldDeclaration().Matches(designerSource).Select(m => m.Groups["name"].Value).ToHashSet(StringComparer.Ordinal);
    }

    /// <summary>The code-behind file named by <c>CodeBehind</c>/<c>CodeFile</c>, else <c>markup.cs</c>, if it exists.</summary>
    private static string? CodeBehindFile(string markup, string markupPath)
    {
        var directive = AspxDirectiveParser.Parse(markup);
        var named = directive?.CodeBehind ?? directive?.CodeFile;
        var candidate = named is { Length: > 0 }
            ? Path.Combine(Path.GetDirectoryName(markupPath)!, named.Replace('\\', '/'))
            : markupPath + ".cs";
        return File.Exists(candidate) ? candidate : PathCaseFixups.Resolve(candidate);
    }

    /// <summary>True unless the source declares <paramref name="className"/> without <c>partial</c>.</summary>
    internal static bool DeclaresPartial(string source, string className)
    {
        var m = System.Text.RegularExpressions.Regex.Match(source, @"(?<mods>(?:\b\w+\s+)*)class\s+" + System.Text.RegularExpressions.Regex.Escape(className) + @"\b");
        return !m.Success || m.Groups["mods"].Value.Contains("partial", StringComparison.Ordinal);
    }

    private static string Escape(string path) =>
        SecurityElement.Escape(path)!.Replace("%", "%25", StringComparison.Ordinal).Replace("$", "%24", StringComparison.Ordinal)
            .Replace("@", "%40", StringComparison.Ordinal).Replace(";", "%3B", StringComparison.Ordinal)
            .Replace("*", "%2A", StringComparison.Ordinal).Replace("?", "%3F", StringComparison.Ordinal);

    private static string ProjectKey(string projectPath)
    {
        var hash = Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(projectPath)))[..12];
        return Path.GetFileNameWithoutExtension(projectPath) + "-" + hash;
    }

    private static string? UserControlType(string src, string projectDir, string markupDir)
    {
        var relative = src.Replace('\\', '/');
        var path = relative.StartsWith("~/", StringComparison.Ordinal)
            ? Path.Combine(projectDir, relative[2..])
            : Path.Combine(markupDir, relative);
        if (!File.Exists(path))
        {
            return null;
        }

        var inherits = AspxDirectiveParser.Parse(File.ReadAllText(path))?.Inherits;
        return inherits?.Split(',')[0].Trim();
    }
}
