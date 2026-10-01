using Eludite.Host.Rpc;

namespace Eludite.Host.Lsp;

/// <summary>
/// Work the host does on a solution before the language server opens it (brief 0003: legacy design-time
/// evaluation, designer partials, path-case fixups). Problems are reported as diagnostics, never thrown.
/// </summary>
public interface ISolutionPreparer
{
    Task<SolutionPreparation> PrepareAsync(string solutionPath, CancellationToken cancellationToken);
}

/// <summary>What <see cref="ISolutionPreparer"/> did, as reported in <c>eludite/solution/status</c>.</summary>
public sealed record SolutionPreparation(
    MsBuildInfo? MsBuild,
    IReadOnlyList<Correction> Corrections,
    IReadOnlyList<HostDiagnostic> Diagnostics,
    int LegacyEvaluationFailures)
{
    public static SolutionPreparation None { get; } = new(null, [], [], 0);
}
