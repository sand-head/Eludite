using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using Microsoft.Build.Evaluation;
using Microsoft.Build.Exceptions;

namespace Eludite.Host.Projects;

/// <summary>
/// Evaluates SDK-style and legacy projects with the .NET SDK's MSBuild in-process (Microsoft.Build.Locator, as the
/// brief 0003 <see cref="InProcessMsBuildEvaluator"/> does): evaluation only, no targets, missing imports ignored.
/// Legacy projects get the brief 0003 design-time properties so Visual Studio-only imports and the .NET Framework
/// reference-assembly root do not stop the evaluation.
/// </summary>
public sealed class MsBuildProjectTreeEvaluator : IProjectTreeEvaluator
{
    // GUIDs of web project types: Web Application, MVC 1 to 5, Web Site.
    private static readonly string[] WebProjectTypes =
    [
        "349c5851-65df-11da-9384-00065b846f21",
        "603c0e0b-db56-11dc-be95-000d561079b0",
        "f85e285d-a4e0-4152-9332-ab1d724d3325",
        "e53f8fea-eae0-44a6-8774-ffd645390401",
        "e3e379df-f4c6-4180-9b81-6769533abe47",
        "349c5853-65df-11da-9384-00065b846f21",
        "e24c65dc-7377-472b-9aba-bc803b73c61a",
    ];

    private static readonly Lock EvaluationLock = new();
    private readonly ReferenceAssemblies _referenceAssemblies;

    public MsBuildProjectTreeEvaluator(ReferenceAssemblies? referenceAssemblies = null)
    {
        _referenceAssemblies = referenceAssemblies ?? new ReferenceAssemblies();
    }

    public IReadOnlyList<TreeProject> Evaluate(IReadOnlyList<string> projectPaths, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPaths);
        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            return [.. projectPaths.Select(p => Unavailable(Path.GetFullPath(p), "no .NET SDK found by Microsoft.Build.Locator"))];
        }

        // One evaluation at a time: MSBuild's in-process state (SDK resolution caches) is shared.
        lock (EvaluationLock)
        {
            return EvaluateCore(projectPaths, cancellationToken);
        }
    }

    /// <summary><c>v4.7.2</c> to <c>net472</c>, <c>v3.5</c> to <c>net35</c>; null when not a version.</summary>
    public static string? ShortFramework(string? targetFrameworkVersion)
    {
        if (string.IsNullOrWhiteSpace(targetFrameworkVersion))
        {
            return null;
        }

        var digits = targetFrameworkVersion.Trim().TrimStart('v', 'V').Replace(".", string.Empty, StringComparison.Ordinal);
        return digits.Length > 0 && digits.All(char.IsAsciiDigit) ? "net" + digits : null;
    }

    internal static TreeProject Unavailable(string fullPath, string error) =>
        new(Path.GetFileNameWithoutExtension(fullPath), fullPath, SolutionProjects.IsLegacy(fullPath) ? "legacy" : "sdk", [], [])
        {
            Error = error,
        };

    // Kept out of Evaluate so no Microsoft.Build type is touched before the locator has registered.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private List<TreeProject> EvaluateCore(IReadOnlyList<string> projectPaths, CancellationToken cancellationToken)
    {
        var results = new List<TreeProject>(projectPaths.Count);
        using var collection = new ProjectCollection(null, null, ToolsetDefinitionLocations.Default);
        const ProjectLoadSettings settings = ProjectLoadSettings.IgnoreMissingImports | ProjectLoadSettings.IgnoreEmptyImports | ProjectLoadSettings.IgnoreInvalidImports;
        foreach (var path in projectPaths)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var full = Path.GetFullPath(path);
            var legacy = SolutionProjects.IsLegacy(full);
            var globals = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            if (legacy)
            {
                var root = DesignTimeProperties.ReadTargetFrameworkVersion(full) is { } tfv ? _referenceAssemblies.RootFor(tfv) : null;
                foreach (var (k, v) in DesignTimeProperties.Create(root))
                {
                    globals[k] = v;
                }
            }

            try
            {
                var project = new Project(full, globals, toolsVersion: null, collection, settings);
                var frameworks = legacy ? null : FrameworksOf(project);
                if (frameworks is { Count: > 0 } && project.GetPropertyValue("TargetFramework").Length == 0)
                {
                    // A multi-targeted project's outer evaluation has no default items; read them from the first
                    // inner evaluation, as Visual Studio's Solution Explorer shows the active target framework.
                    collection.UnloadProject(project);
                    globals["TargetFramework"] = frameworks[0];
                    project = new Project(full, globals, toolsVersion: null, collection, settings);
                }

                results.Add(Read(project, full, legacy, frameworks));
                collection.UnloadProject(project);
            }
            catch (InvalidProjectFileException ex)
            {
                results.Add(Unavailable(full, ex.Message));
            }
        }

        return results;
    }

    private static TreeProject Read(Project project, string full, bool legacy, List<string>? sdkFrameworks)
    {
        var dir = Path.GetDirectoryName(full)!;
        var frameworks = sdkFrameworks
            ?? (ShortFramework(project.GetPropertyValue("TargetFrameworkVersion")) is { } f ? [f] : []);
        var typeGuids = project.GetPropertyValue("ProjectTypeGuids");
        var content = project.GetItems("Content");
        var web = string.Equals(project.GetPropertyValue("UsingMicrosoftNETSdkWeb"), "true", StringComparison.OrdinalIgnoreCase)
            || WebProjectTypes.Any(g => typeGuids.Contains(g, StringComparison.OrdinalIgnoreCase))
            || content.Any(i => DesignTimeProperties.IsMarkup(i.EvaluatedInclude));

        var files = new List<TreeFile>();
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var excluded = new[] { "obj", "bin" }.Select(d => Path.Combine(dir, d) + Path.DirectorySeparatorChar).ToArray();
        void Add(ProjectItem item, string itemType)
        {
            var path = Path.GetFullPath(item.GetMetadataValue("FullPath"));
            if (excluded.Any(x => path.StartsWith(x, StringComparison.Ordinal)) || !seen.Add(path))
            {
                return;
            }

            var dependentUpon = item.GetMetadataValue("DependentUpon");
            var link = item.GetMetadataValue("Link");
            files.Add(new TreeFile(path, itemType)
            {
                DependentUpon = dependentUpon.Length > 0
                    ? Path.GetFullPath(Path.Combine(Path.GetDirectoryName(path)!, dependentUpon.Replace('\\', Path.DirectorySeparatorChar)))
                    : null,
                Link = link.Length > 0 ? link.Replace('\\', '/') : null,
            });
        }

        foreach (var item in project.GetItems("Compile"))
        {
            Add(item, "compile");
        }

        if (web)
        {
            foreach (var item in content)
            {
                Add(item, "content");
            }
        }

        return new TreeProject(Path.GetFileNameWithoutExtension(full), full, legacy ? "legacy" : "sdk", frameworks, files) { Web = web };
    }

    private static List<string> FrameworksOf(Project project)
    {
        var many = project.GetPropertyValue("TargetFrameworks");
        var value = many.Length > 0 ? many : project.GetPropertyValue("TargetFramework");
        return [.. value.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)];
    }
}
