namespace Eludite.Host.Legacy;

/// <summary>A warning or error reported while evaluating a project. Failures are diagnostics, never dialogs.</summary>
public sealed record EvaluationDiagnostic(string Severity, string? Code, string Message, string? File = null);

/// <summary>
/// The design-time inputs of one project, as a project system would hand them to Roslyn (brief 0003 spike).
/// Produced by evaluation plus reference resolution (<c>ResolveReferences</c>); never a compile.
/// </summary>
public sealed record LegacyProjectEvaluation
{
    public required string ProjectPath { get; init; }

    /// <summary>Which evaluator produced this (<see cref="EvaluatorKind"/> names).</summary>
    public required string Evaluator { get; init; }

    /// <summary>True when evaluation and reference resolution completed (errors may still be listed).</summary>
    public required bool Loaded { get; init; }

    /// <summary>First error that stopped evaluation, when <see cref="Loaded"/> is false.</summary>
    public string? FailureReason { get; init; }

    /// <summary>Failure class from <see cref="FailureClassifier"/>: missing targets, COM, packages, web targets, other.</summary>
    public string? FailureClass { get; init; }

    public string? TargetFrameworkVersion { get; init; }
    public string? TargetFrameworkMoniker { get; init; }
    public string? AssemblyName { get; init; }
    public string? OutputType { get; init; }
    public string? LangVersion { get; init; }
    public bool AllowUnsafeBlocks { get; init; }
    public IReadOnlyList<string> DefineConstants { get; init; } = [];

    /// <summary>Full paths of <c>Compile</c> items.</summary>
    public IReadOnlyList<string> CompileItems { get; init; } = [];

    /// <summary>Full paths of resolved references (<c>ReferencePath</c>).</summary>
    public IReadOnlyList<string> ReferencePaths { get; init; } = [];

    /// <summary><c>Reference</c> items that did not resolve.</summary>
    public IReadOnlyList<string> UnresolvedReferences { get; init; } = [];

    /// <summary>Full paths of <c>ProjectReference</c> items.</summary>
    public IReadOnlyList<string> ProjectReferences { get; init; } = [];

    /// <summary><c>PackageReference</c> ids (non-SDK projects can use them too).</summary>
    public IReadOnlyList<string> PackageReferences { get; init; } = [];

    /// <summary>True when the project directory has a <c>packages.config</c>.</summary>
    public bool UsesPackagesConfig { get; init; }

    /// <summary>Full paths of WebForms markup items (<c>.aspx</c>, <c>.ascx</c>, <c>.master</c>) from <c>Content</c>.</summary>
    public IReadOnlyList<string> MarkupItems { get; init; } = [];

    /// <summary>Imports MSBuild could not find (ignored by the in-process evaluator, fatal for the command line).</summary>
    public IReadOnlyList<string> MissingImports { get; init; } = [];

    public IReadOnlyList<EvaluationDiagnostic> Diagnostics { get; init; } = [];

    /// <summary>Wall time of evaluation plus reference resolution, in milliseconds.</summary>
    public double ElapsedMs { get; init; }
}

/// <summary>Names of the evaluators, as they appear in results.</summary>
public static class EvaluatorKind
{
    /// <summary>Mono's MSBuild, run as a command line (Linux, macOS).</summary>
    public const string Mono = "mono-msbuild";

    /// <summary>Visual Studio Build Tools MSBuild.exe, located with vswhere (Windows).</summary>
    public const string BuildTools = "buildtools-msbuild";

    /// <summary>The .NET SDK's MSBuild in-process through Microsoft.Build.Locator, missing imports ignored.</summary>
    public const string Sdk = "sdk-msbuild";

    /// <summary>The .NET SDK's MSBuild as <c>dotnet msbuild</c> (missing imports are fatal).</summary>
    public const string SdkCli = "sdk-msbuild-cli";
}
