using Eludite.Host.Legacy;
using Eludite.Host.Rpc;
using Microsoft.Build.Evaluation;
using Microsoft.Build.Exceptions;

namespace Eludite.Host.Projects;

/// <summary>
/// What <c>dotnet run</c> would run for a project (brief 0074; host-rpc.md, "Run target"): the project evaluated with
/// <c>Configuration</c>, <c>Platform</c> and <c>TargetFramework</c> as global properties, then its <c>TargetPath</c>,
/// <c>RunCommand</c>, <c>RunArguments</c> and <c>RunWorkingDirectory</c> read as <c>Microsoft.NET.Sdk.targets</c> sets
/// them at evaluation. Called by <see cref="ProjectPropertyEvaluator.EvaluateRunTarget"/> under its lock, on its shared
/// collection; only that evaluator touches Microsoft.Build types before the locator registers.
/// </summary>
internal static class RunTarget
{
    /// <summary>
    /// Evaluates <paramref name="full"/>. The framework rule: <paramref name="framework"/> when the project lists it, else
    /// a multi-targeted project's first framework; a single-targeted project evaluates as it is.
    /// </summary>
    /// <param name="globals">The global properties for a framework (null: none), as the property evaluation builds them.</param>
    public static RunTargetResult Evaluate(ProjectCollection collection, string full, string? framework, long generation, Func<string?, Dictionary<string, string>> globals)
    {
        if (!File.Exists(full))
        {
            throw HostErrors.BadParams($"{full} does not exist");
        }

        var legacy = SolutionProjects.IsLegacy(full);
        var project = Load(collection, full, globals(null));
        try
        {
            var listed = ProjectPropertyEvaluator.SplitList(project.GetPropertyValue("TargetFrameworks"));
            if (listed.Count > 0)
            {
                // Multi-targeted: the outer evaluation has no TargetPath of its own; evaluate the chosen framework.
                var chosen = framework is { Length: > 0 } && listed.Contains(framework, StringComparer.OrdinalIgnoreCase)
                    ? listed.First(f => string.Equals(f, framework, StringComparison.OrdinalIgnoreCase))
                    : listed[0];
                collection.UnloadProject(project);
                project = Load(collection, full, globals(chosen));
            }
            else
            {
                listed = ProjectPropertyEvaluator.SplitList(project.GetPropertyValue("TargetFramework"));
                if (legacy && listed.Count == 0 && MsBuildProjectTreeEvaluator.ShortFramework(project.GetPropertyValue("TargetFrameworkVersion")) is { } f)
                {
                    listed = [f];
                }
            }

            var dir = project.DirectoryPath;
            var targetPath = Absolute(dir, project.GetPropertyValue("TargetPath"));
            var evaluatedFramework = project.GetPropertyValue("TargetFramework");
            var identifier = project.GetPropertyValue("TargetFrameworkIdentifier");
            return new RunTargetResult(
                generation,
                full,
                project.GetPropertyValue("Configuration"),
                project.GetPropertyValue("Platform"),
                listed,
                project.GetPropertyValue("OutputType"),
                targetPath,
                project.GetPropertyValue("RunCommand"),
                project.GetPropertyValue("RunArguments"),
                Absolute(dir, project.GetPropertyValue("RunWorkingDirectory")),
                string.Equals(project.GetPropertyValue("UsingMicrosoftNETSdkWeb"), "true", StringComparison.OrdinalIgnoreCase),
                targetPath.Length > 0 && File.Exists(targetPath))
            {
                Framework = evaluatedFramework.Length > 0 ? evaluatedFramework : null,
                TargetFrameworkIdentifier = identifier.Length > 0 ? identifier : null,
            };
        }
        finally
        {
            collection.UnloadProject(project);
        }
    }

    private static Project Load(ProjectCollection collection, string full, Dictionary<string, string> globals)
    {
        try
        {
            return new Project(full, globals, toolsVersion: null, collection, ProjectFileEditor.LoadSettings);
        }
        catch (InvalidProjectFileException ex)
        {
            throw HostErrors.BadParams($"{full} does not evaluate: {ex.Message}");
        }
    }

    /// <summary>A path the project evaluated, made absolute against the project folder; empty stays empty.</summary>
    private static string Absolute(string dir, string value) =>
        value.Length == 0 ? value : Path.GetFullPath(Path.IsPathRooted(value) ? value : Path.Combine(dir, value));
}
