using System.Globalization;
using System.Text.RegularExpressions;

namespace Eludite.Host.Build;

/// <summary>
/// Recognizes MSBuild console lines (minimal verbosity, <c>ForceNoAlign</c>): canonical errors and warnings
/// (<c>file(line,col): error CS0103: message [project]</c>), a project's <c>Name -&gt; output</c> line, and the stack
/// frames of a task that failed unexpectedly. Used for progress while the build runs and as the fallback when there
/// is no readable binary log.
/// </summary>
public static partial class ConsoleLines
{
    [GeneratedRegex(@"^\s*(?<origin>.*?)(?:\((?<line>\d+)(?:,(?<col>\d+))?(?:,(?<eline>\d+),(?<ecol>\d+))?\))?\s*:\s*(?:[^:]*\s)?(?<sev>error|warning)\s*(?<code>[A-Za-z]*\d+)?\s*:\s*(?<msg>.*?)(?:\s+\[(?<proj>[^\[\]]+)\])?\s*$")]
    private static partial Regex Canonical();

    [GeneratedRegex(@"^\s*(?<name>[^\s\->:][^>:]*?) -> (?<out>\S.*)$")]
    private static partial Regex ProjectOutput();

    [GeneratedRegex(@"^\s+at \S+.*\)( in .*)?$|^\s*--- End of (inner exception|stack trace from previous location).*---\s*$")]
    private static partial Regex StackFrame();

    /// <summary>A canonical error or warning line as a diagnostic, or null.</summary>
    public static BuildDiagnostic? ParseDiagnostic(string line)
    {
        ArgumentNullException.ThrowIfNull(line);
        if (!line.Contains("error", StringComparison.Ordinal) && !line.Contains("warning", StringComparison.Ordinal))
        {
            return null;
        }

        var m = Canonical().Match(line);
        if (!m.Success)
        {
            return null;
        }

        var origin = m.Groups["origin"].Value.Trim();
        int? Int(string group) => m.Groups[group].Success ? int.Parse(m.Groups[group].Value, CultureInfo.InvariantCulture) : null;
        var project = m.Groups["proj"].Success ? m.Groups["proj"].Value.Trim() : null;
        var file = origin.Length == 0 || origin.Equals("MSBUILD", StringComparison.OrdinalIgnoreCase) || origin.Equals("CSC", StringComparison.OrdinalIgnoreCase)
            ? null
            : origin;
        return new BuildDiagnostic(m.Groups["sev"].Value, m.Groups["code"].Value, m.Groups["msg"].Value)
        {
            File = file,
            Line = Int("line"),
            Column = Int("col"),
            EndLine = Int("eline"),
            EndColumn = Int("ecol"),
            Project = project,
        };
    }

    /// <summary>The project name of a <c>Name -&gt; output path</c> line (a project finished), or null.</summary>
    public static string? ParseProjectOutput(string line)
    {
        ArgumentNullException.ThrowIfNull(line);
        if (!line.Contains(" -> ", StringComparison.Ordinal))
        {
            return null;
        }

        var m = ProjectOutput().Match(line);
        return m.Success ? m.Groups["name"].Value.Trim() : null;
    }

    /// <summary>True for a stack frame line (<c>   at Namespace.Type.Method(...)</c>) or an exception separator.</summary>
    public static bool IsStackFrame(string line) => StackFrame().IsMatch(line);
}
