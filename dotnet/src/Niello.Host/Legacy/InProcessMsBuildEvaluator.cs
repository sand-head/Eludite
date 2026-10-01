using System.Diagnostics;
using System.Runtime.CompilerServices;
using Microsoft.Build.Evaluation;
using Microsoft.Build.Exceptions;
using Microsoft.Build.Execution;
using Microsoft.Build.Framework;
using Microsoft.Build.Locator;

namespace Niello.Host.Legacy;

/// <summary>
/// Evaluates projects in-process with the .NET SDK's MSBuild, located by Microsoft.Build.Locator
/// (Project evaluation, then <c>ResolveReferences</c> on a ProjectInstance; no compile).
/// With <see cref="IgnoreMissingImports"/> it survives imports that only exist in Visual Studio
/// (Microsoft.WebApplication.targets and friends), which the command-line MSBuild treats as fatal.
/// </summary>
public sealed class InProcessMsBuildEvaluator
{
    private static readonly Lock RegistrationLock = new();
    private readonly ReferenceAssemblies _referenceAssemblies;
    private readonly string _workDirectory;

    public InProcessMsBuildEvaluator(ReferenceAssemblies referenceAssemblies, string workDirectory)
    {
        _referenceAssemblies = referenceAssemblies;
        _workDirectory = workDirectory;
    }

    /// <summary>Ignore missing, empty and invalid imports (Visual Studio-only targets). Default true.</summary>
    public bool IgnoreMissingImports { get; init; } = true;

    /// <summary>Extra targets file imported into every project (designer-partial injection).</summary>
    public string? ExtraTargets { get; init; }

    /// <summary>The MSBuild directory registered with the locator, or null if none was found.</summary>
    public static string? EnsureRegistered()
    {
        lock (RegistrationLock)
        {
            if (MSBuildLocator.IsRegistered)
            {
                return RegisteredPath;
            }

            var instance = MSBuildLocator.QueryVisualStudioInstances(new VisualStudioInstanceQueryOptions { DiscoveryTypes = DiscoveryType.DotNetSdk })
                .OrderByDescending(i => i.Version)
                .FirstOrDefault();
            if (instance is null)
            {
                return null;
            }

            MSBuildLocator.RegisterInstance(instance);
            RegisteredPath = instance.MSBuildPath;
            return RegisteredPath;
        }
    }

    private static string? RegisteredPath { get; set; }

    /// <summary>Evaluates each project in one shared ProjectCollection, in order.</summary>
    public IReadOnlyList<LegacyProjectEvaluation> Evaluate(IReadOnlyList<string> projectPaths, CancellationToken cancellationToken)
    {
        ArgumentNullException.ThrowIfNull(projectPaths);
        if (EnsureRegistered() is null)
        {
            return [.. projectPaths.Select(p => new LegacyProjectEvaluation
            {
                ProjectPath = Path.GetFullPath(p),
                Evaluator = Kind,
                Loaded = false,
                FailureReason = "no .NET SDK found by Microsoft.Build.Locator",
                FailureClass = FailureClassifier.Other,
            })];
        }

        return EvaluateCore(projectPaths, cancellationToken);
    }

    private string Kind => IgnoreMissingImports ? EvaluatorKind.Sdk : EvaluatorKind.Sdk + "-strict";

    // Kept out of Evaluate so no Microsoft.Build type is touched before the locator has registered.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private List<LegacyProjectEvaluation> EvaluateCore(IReadOnlyList<string> projectPaths, CancellationToken cancellationToken)
    {
        var merged = _referenceAssemblies.MergedRoot(_workDirectory);
        var results = new List<LegacyProjectEvaluation>();
        var logger = new CollectingLogger();
        using var collection = new ProjectCollection(null, [logger], ToolsetDefinitionLocations.Default);
        var settings = IgnoreMissingImports
            ? ProjectLoadSettings.IgnoreMissingImports | ProjectLoadSettings.IgnoreEmptyImports | ProjectLoadSettings.IgnoreInvalidImports
            : ProjectLoadSettings.Default;

        foreach (var path in projectPaths)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var full = Path.GetFullPath(path);
            logger.Reset();
            var sw = Stopwatch.StartNew();
            var root = merged ?? (DesignTimeProperties.ReadTargetFrameworkVersion(full) is { } tfv ? _referenceAssemblies.RootFor(tfv) : null);
            var globals = new Dictionary<string, string>(DesignTimeProperties.Create(root), StringComparer.OrdinalIgnoreCase);
            if (ExtraTargets is not null)
            {
                globals["CustomAfterMicrosoftCommonTargets"] = ExtraTargets;
            }

            try
            {
                var project = new Project(full, globals, toolsVersion: null, collection, settings);
                var instance = project.CreateProjectInstance();
                var built = instance.Build(["ResolveReferences"], [logger]);
                collection.UnloadProject(project);
                sw.Stop();
                results.Add(Read(instance, full, built, logger, sw.Elapsed));
            }
            catch (InvalidProjectFileException ex)
            {
                sw.Stop();
                results.Add(new LegacyProjectEvaluation
                {
                    ProjectPath = full,
                    Evaluator = Kind,
                    Loaded = false,
                    FailureReason = ex.BaseMessage,
                    FailureClass = FailureClassifier.Classify(ex.ErrorCode, ex.BaseMessage),
                    Diagnostics = [.. logger.Diagnostics, new EvaluationDiagnostic("error", ex.ErrorCode, ex.BaseMessage, ex.ProjectFile)],
                    MissingImports = [.. logger.MissingImports],
                    UsesPackagesConfig = File.Exists(Path.Combine(Path.GetDirectoryName(full)!, "packages.config")),
                    ElapsedMs = sw.Elapsed.TotalMilliseconds,
                });
            }
        }

        return results;
    }

    private LegacyProjectEvaluation Read(ProjectInstance instance, string full, bool built, CollectingLogger logger, TimeSpan elapsed)
    {
        static List<string> FullPaths(ProjectInstance i, string type) =>
            i.GetItems(type).Select(x => x.GetMetadataValue("FullPath")).Distinct(StringComparer.Ordinal).ToList();

        var resolved = instance.GetItems("ReferencePath").Select(r => r.GetMetadataValue("OriginalItemSpec")).ToHashSet(StringComparer.OrdinalIgnoreCase);
        var firstError = logger.Diagnostics.FirstOrDefault(d => d.Severity == "error");
        return new LegacyProjectEvaluation
        {
            ProjectPath = full,
            Evaluator = Kind,
            Loaded = true,
            FailureReason = built ? null : firstError?.Message ?? "ResolveReferences failed",
            FailureClass = built ? null : FailureClassifier.Classify(firstError?.Code, firstError?.Message ?? string.Empty),
            TargetFrameworkVersion = instance.GetPropertyValue("TargetFrameworkVersion"),
            TargetFrameworkMoniker = instance.GetPropertyValue("TargetFrameworkMoniker"),
            AssemblyName = instance.GetPropertyValue("AssemblyName"),
            OutputType = instance.GetPropertyValue("OutputType"),
            LangVersion = instance.GetPropertyValue("LangVersion") is { Length: > 0 } lv ? lv : null,
            AllowUnsafeBlocks = string.Equals(instance.GetPropertyValue("AllowUnsafeBlocks"), "true", StringComparison.OrdinalIgnoreCase),
            DefineConstants = CommandLineMsBuildEvaluator.SplitDefines(instance.GetPropertyValue("DefineConstants")),
            CompileItems = FullPaths(instance, "Compile"),
            ReferencePaths = FullPaths(instance, "ReferencePath"),
            UnresolvedReferences = instance.GetItems("Reference").Select(r => r.EvaluatedInclude).Where(r => !resolved.Contains(r)).ToList(),
            ProjectReferences = FullPaths(instance, "ProjectReference"),
            PackageReferences = instance.GetItems("PackageReference").Select(p => p.EvaluatedInclude).ToList(),
            UsesPackagesConfig = File.Exists(Path.Combine(Path.GetDirectoryName(full)!, "packages.config")),
            MarkupItems = FullPaths(instance, "Content").Where(DesignTimeProperties.IsMarkup).ToList(),
            MissingImports = [.. logger.MissingImports],
            Diagnostics = [.. logger.Diagnostics],
            ElapsedMs = elapsed.TotalMilliseconds,
        };
    }

    private sealed class CollectingLogger : ILogger
    {
        private readonly List<EvaluationDiagnostic> _diagnostics = [];
        private readonly List<string> _missingImports = [];

        public LoggerVerbosity Verbosity { get; set; } = LoggerVerbosity.Diagnostic;

        public string? Parameters { get; set; }

        public IReadOnlyList<EvaluationDiagnostic> Diagnostics { get { lock (_diagnostics) { return [.. _diagnostics]; } } }

        public IReadOnlyList<string> MissingImports { get { lock (_diagnostics) { return [.. _missingImports]; } } }

        public void Reset()
        {
            lock (_diagnostics)
            {
                _diagnostics.Clear();
                _missingImports.Clear();
            }
        }

        public void Initialize(IEventSource eventSource)
        {
            eventSource.ErrorRaised += (_, e) => Add(new EvaluationDiagnostic("error", e.Code, e.Message ?? string.Empty, e.File));
            eventSource.WarningRaised += (_, e) => Add(new EvaluationDiagnostic("warning", e.Code, e.Message ?? string.Empty, e.File));
            eventSource.MessageRaised += (_, e) =>
            {
                // "Project X was not imported by Y at (l,c), due to the file not existing." Empty and false-condition
                // imports are also reported as ignored; only missing files matter here.
                if (e is ProjectImportedEventArgs { ImportIgnored: true } imported
                    && (imported.Message?.Contains("not exist", StringComparison.OrdinalIgnoreCase) ?? false))
                {
                    lock (_diagnostics)
                    {
                        _missingImports.Add(imported.Message);
                    }
                }
            };
        }

        private void Add(EvaluationDiagnostic d)
        {
            lock (_diagnostics)
            {
                _diagnostics.Add(d);
            }
        }

        public void Shutdown()
        {
        }
    }
}
