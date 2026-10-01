using System.Runtime.CompilerServices;
using Eludite.Host.Legacy;
using Microsoft.Build.Framework;
using Microsoft.Build.Logging;

namespace Eludite.Host.Build;

/// <summary>One project file's outcome in a binary log (all its builds: inner target-framework builds, references).</summary>
public sealed record BinlogProject(string Path, bool Succeeded, DateTime Started, DateTime Finished);

/// <summary>A diagnostic from the log with the target that reported it.</summary>
public sealed record BinlogDiagnostic(BuildDiagnostic Diagnostic, string? Target);

/// <summary>What <see cref="BinlogReader.Read"/> found.</summary>
public sealed record BinlogResult(IReadOnlyList<BinlogProject> Projects, IReadOnlyList<BinlogDiagnostic> Diagnostics, bool? Succeeded);

/// <summary>
/// Reads an MSBuild binary log (<c>-bl</c>) with MSBuild's own reader, <c>BinaryLogReplayEventSource</c> (package
/// Microsoft.Build, MIT; the .NET SDK's copy, loaded through Microsoft.Build.Locator as the evaluators do). It reads
/// every format version up to the SDK's own, so it reads Mono's MSBuild 16 logs too.
/// </summary>
public static class BinlogReader
{
    /// <summary>Replays <paramref name="path"/>. Throws when the log cannot be read (missing, truncated, newer format).</summary>
    public static BinlogResult Read(string path, CancellationToken cancellationToken)
    {
        if (InProcessMsBuildEvaluator.EnsureRegistered() is null)
        {
            throw new InvalidOperationException("no .NET SDK MSBuild found by Microsoft.Build.Locator to read the binary log");
        }

        return Replay(path, cancellationToken);
    }

    // Separate and not inlined: Microsoft.Build is loaded (from the located SDK) only when this method is compiled,
    // after the locator registration above.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static BinlogResult Replay(string path, CancellationToken cancellationToken)
    {
        var projects = new Dictionary<string, (bool Succeeded, DateTime Started, DateTime Finished)>(StringComparer.Ordinal);
        var contexts = new Dictionary<(int Node, int Context), (string File, DateTime Started)>();
        var targets = new Dictionary<(int Node, int Context, int Target), string>();
        var diagnostics = new List<BinlogDiagnostic>();
        bool? succeeded = null;

        string? TargetOf(BuildEventContext? c) =>
            c is not null && targets.TryGetValue((c.NodeId, c.ProjectContextId, c.TargetId), out var t) ? t : null;

        string? Rooted(string? file, string? project)
        {
            if (string.IsNullOrEmpty(file))
            {
                return null;
            }

            if (Path.IsPathRooted(file) || project is null)
            {
                return file;
            }

            return Path.GetFullPath(Path.Combine(Path.GetDirectoryName(project)!, file));
        }

        int? Positive(int n) => n > 0 ? n : null;

        var source = new BinaryLogReplayEventSource();
        source.AnyEventRaised += (_, e) =>
        {
            switch (e)
            {
                case ProjectStartedEventArgs s when s.BuildEventContext is { } c && s.ProjectFile is { } file:
                    contexts[(c.NodeId, c.ProjectContextId)] = (file, s.Timestamp);
                    break;
                case ProjectFinishedEventArgs f when f.BuildEventContext is { } c && f.ProjectFile is { } file:
                    var started = contexts.TryGetValue((c.NodeId, c.ProjectContextId), out var ctx) ? ctx.Started : f.Timestamp;
                    if (!IsProject(file))
                    {
                        break;
                    }

                    projects[file] = projects.TryGetValue(file, out var p)
                        ? (p.Succeeded && f.Succeeded, p.Started < started ? p.Started : started, p.Finished > f.Timestamp ? p.Finished : f.Timestamp)
                        : (f.Succeeded, started, f.Timestamp);
                    break;
                case TargetStartedEventArgs t when t.BuildEventContext is { } c:
                    targets[(c.NodeId, c.ProjectContextId, c.TargetId)] = t.TargetName;
                    break;
                case BuildErrorEventArgs err:
                    diagnostics.Add(new BinlogDiagnostic(
                        new BuildDiagnostic("error", err.Code ?? string.Empty, err.Message ?? string.Empty)
                        {
                            File = Rooted(err.File, err.ProjectFile),
                            Line = Positive(err.LineNumber),
                            Column = Positive(err.ColumnNumber),
                            EndLine = Positive(err.EndLineNumber),
                            EndColumn = Positive(err.EndColumnNumber),
                            Project = err.ProjectFile,
                        },
                        TargetOf(err.BuildEventContext)));
                    break;
                case BuildWarningEventArgs w:
                    diagnostics.Add(new BinlogDiagnostic(
                        new BuildDiagnostic("warning", w.Code ?? string.Empty, w.Message ?? string.Empty)
                        {
                            File = Rooted(w.File, w.ProjectFile),
                            Line = Positive(w.LineNumber),
                            Column = Positive(w.ColumnNumber),
                            EndLine = Positive(w.EndLineNumber),
                            EndColumn = Positive(w.EndColumnNumber),
                            Project = w.ProjectFile,
                        },
                        TargetOf(w.BuildEventContext)));
                    break;
                case BuildFinishedEventArgs b:
                    succeeded = b.Succeeded;
                    break;
            }
        };
        source.Replay(path, cancellationToken);
        return new BinlogResult(
            [.. projects.Select(kv => new BinlogProject(kv.Key, kv.Value.Succeeded, kv.Value.Started, kv.Value.Finished))],
            diagnostics,
            succeeded);
    }

    /// <summary>True for a real project file (not a solution, a generated metaproject or a traversal).</summary>
    public static bool IsProject(string file) =>
        file.EndsWith(".csproj", StringComparison.OrdinalIgnoreCase)
        || file.EndsWith(".vbproj", StringComparison.OrdinalIgnoreCase)
        || file.EndsWith(".fsproj", StringComparison.OrdinalIgnoreCase);
}
